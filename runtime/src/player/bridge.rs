// player_bridge.rs — orchestrates the full play pipeline with adaptive buffering.
//
// Play flow:
//
//   Go sends {"type":"play","entry_id":"tt1234|magnet:?xt=…","provider":"prowlarr-provider"}
//     │
//     ▼
//   engine.resolve_raw(entry_id, provider)
//     → StreamResult { stream_url, quality, subtitles }
//     │
//     ▼
//   classify stream_url:
//     magnet/torrent → aria2 start  →  Streamer.wait_for_preroll()
//                                         measures speed, computes pre-roll,
//                                         waits until buffer is safe to play
//                                   →  mpv.play(local_file)
//                                   →  Streamer.run_stall_guard() (concurrent)
//     http/https direct → mpv.play(url)
//     yt-dlp URL        → mpv.play(url --ytdl)
//
// Unsolicited NDJSON pushed to Go:
//   {"type":"player_buffering",   "reason":"initial"|"stall_guard",
//                                 "fill_percent":42.0, "speed_mbps":3.1,
//                                 "pre_roll_secs":30.0, "eta_secs":8.2}
//   {"type":"player_buffer_ready","pre_roll_secs":30.0,"speed_mbps":3.1,"slack":1.4}
//   {"type":"player_started",     "title":"…","path":"…","duration":5400.0}
//   {"type":"player_progress",    "position":42.1,"duration":5400.0,"paused":false,"cache_percent":100}
//   {"type":"player_ended",       "reason":"eof"|"quit"|"error","error":"…"}

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::aria2_bridge::Aria2Bridge;
use crate::config::types::PlaybackConfig;
use crate::dsp::DspPipeline;
use crate::engine::Engine;
use crate::mpd_bridge::MpdBridge;
use crate::ipc::{MediaTab, MediaType};
use crate::storage::aria2_translator::MediaType as Aria2MediaType;
use super::mpv::{MpvEvent, MpvPlayer, PlayerEndedReason};
use crate::streamer::Streamer;

// ── Wire shapes ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PlayerStartedWire<'a> {
    r#type:   &'static str,
    title:    &'a str,
    path:     &'a str,
    duration: f64,
}

#[derive(Serialize)]
struct PlayerProgressWire {
    r#type:        &'static str,
    position:      f64,
    duration:      f64,
    paused:        bool,
    cache_percent: f64,
}

#[derive(Serialize)]
struct PlayerEndedWire {
    r#type:  &'static str,
    reason:  &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    error:   String,
}

// ── PlayerBridge ──────────────────────────────────────────────────────────────

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct PlayerBridge {
    mpv:          MpvPlayer,
    aria2:        Option<Aria2Bridge>,
    mpd:          Option<MpdBridge>,
    engine:       Arc<Engine>,
    storage:      Arc<crate::storage::MediaStorage>,
    watch_history: Arc<crate::watchhistory::WatchHistoryStore>,
    ipc_tx:       mpsc::Sender<String>,
    data_dir:     String,
    playback_cfg: PlaybackConfig,
    /// DSP pipeline reference. When present and `config.enabled`, mpv receives
    /// equivalent `--af` / `--audio-samplerate` flags so movie presets apply.
    dsp:          Option<Arc<tokio::sync::Mutex<DspPipeline>>>,
}

impl PlayerBridge {
    pub fn new(
        engine:       Arc<Engine>,
        aria2:        Option<Aria2Bridge>,
        mpd:          Option<MpdBridge>,
        storage:      Arc<crate::storage::MediaStorage>,
        watch_history: Arc<crate::watchhistory::WatchHistoryStore>,
        ipc_tx:       mpsc::Sender<String>,
        data_dir:     String,
        playback_cfg: PlaybackConfig,
        dsp:          Option<Arc<tokio::sync::Mutex<DspPipeline>>>,
    ) -> Self {
        let mpv = MpvPlayer::new();

        // Forward mpv events → Go IPC channel
        let tx   = ipc_tx.clone();
        let mpv2 = mpv.clone();
        tokio::spawn(async move {
            run_mpv_event_forwarder(mpv2, tx).await;
        });

        PlayerBridge { mpv, aria2, mpd, engine, storage, watch_history, ipc_tx, data_dir, playback_cfg, dsp }
    }

    /// Build the DSP-derived mpv flags for the current pipeline configuration.
    ///
    /// Returns an empty vec when the DSP pipeline is absent or disabled.
    async fn mpv_dsp_flags(&self) -> Vec<String> {
        let Some(ref dsp) = self.dsp else { return vec![]; };
        // Clone the inner Arc while briefly holding the pipeline mutex, then drop
        // the guard before the config read await so the future stays Send.
        let config_arc = dsp.lock().await.config_arc();
        let config = config_arc.read().await.clone();
        crate::dsp::dsp_to_mpv_flags(&config)
    }

    // ── Public API ────────────────────────────────────────────────────────────

    #[allow(dead_code)]
    /// Access the underlying `MpvPlayer` for typed command dispatch.
    pub fn mpv(&self) -> &MpvPlayer {
        &self.mpv
    }

    pub async fn play(&self, entry_id: &str, provider: &str, imdb_id: &str, tab: Option<MediaTab>, media_type: Option<MediaType>, year: Option<u32>) {
        info!("player_bridge: play entry_id={} provider={}", entry_id, provider);

        let stream_url = match self.engine.resolve_raw(entry_id, provider).await {
            Ok(r)  => r.stream_url,
            Err(e) => {
                error!("player_bridge: resolve failed: {e}");
                self.push_ended("error", &e).await;
                return;
            }
        };

        let title    = entry_id.split('|').next().unwrap_or(entry_id);
        let is_audio = matches!(tab, Some(MediaTab::Music) | Some(MediaTab::Radio) | Some(MediaTab::Podcasts));

        if is_audio {
            if let Some(ref mpd) = self.mpd {
                self.play_via_mpd(mpd, &stream_url, title).await;
                return;
            }
            warn!("player_bridge: audio tab but MPD not configured — falling back to mpv");
        }

        let sub_path = find_subtitle(&self.data_dir, imdb_id);
        self.start_stream(entry_id, &stream_url, title, sub_path.as_deref(), media_type, year).await;
    }

    pub async fn stop(&self) {
        self.mpv.stop().await;
    }

    /// Download a torrent/magnet URL via aria2 without launching mpv.
    /// Progress events are emitted automatically by the aria2 bridge monitors.
    pub async fn download_only(&self, url: &str, title: &str, media_type: Option<MediaType>, year: Option<u32>) {
        let Some(ref aria2) = self.aria2 else {
            warn!("player_bridge: download_only — aria2 not available");
            return;
        };
        let aria2_media_type = media_type.map(|m| Aria2MediaType::from_ipc(&m));
        let mut sink = std::io::sink();
        match aria2.start_download(url, &mut sink, aria2_media_type, Some(title), year).await {
            Ok(gid) => {
                let msg = serde_json::to_string(&serde_json::json!({
                    "type":  "download_started",
                    "gid":   gid,
                    "title": title,
                    "uri":   url,
                })).unwrap_or_default();
                let _ = self.ipc_tx.send(msg).await;
                info!("player_bridge: download_only gid={gid}");
            }
            Err(e) => warn!("player_bridge: download_only aria2 error: {e}"),
        }
    }

    /// Cancel an active aria2 download by GID.
    pub async fn cancel_download(&self, gid: &str) {
        let Some(ref aria2) = self.aria2 else { return; };
        if let Err(e) = aria2.client().remove(gid).await {
            warn!("player_bridge: cancel_download {gid}: {e}");
        }
    }

    /// Play a local file path directly via mpv (used for completed downloads).
    pub async fn play_local_file(&self, path: &str, title: &str) {
        self.launch_mpv(path, title, None).await;
    }

    pub async fn send_command(&self, cmd: &str, args: &[serde_json::Value]) {
        use serde_json::json;
        let mut full_cmd = vec![json!(cmd)];
        full_cmd.extend_from_slice(args);
        if let Err(e) = self.mpv.send_command(&json!(full_cmd)).await {
            warn!("player_bridge: send_command failed: {e}");
        }
    }

    // ── Routing ───────────────────────────────────────────────────────────────

    async fn start_stream(&self, entry_id: &str, url: &str, title: &str, sub: Option<&str>, media_type: Option<MediaType>, year: Option<u32>) {
        if is_torrent_url(url) || is_magnet(url) {
            self.play_via_aria2(entry_id, url, title, sub, media_type, year).await;
        } else {
            self.launch_mpv(url, title, sub).await;
        }
    }

    async fn play_via_aria2(&self, entry_id: &str, uri: &str, title: &str, sub: Option<&str>, media_type: Option<MediaType>, year: Option<u32>) {
        let Some(aria2) = &self.aria2 else {
            warn!("player_bridge: aria2 not available");
            self.push_ended(
                "error",
                "aria2 not running — start with: ./scripts/aria2c-start.sh",
            ).await;
            return;
        };

        let aria2_media_type = media_type.map(|m| Aria2MediaType::from_ipc(&m)).unwrap_or(Aria2MediaType::Movie);

        // Start download
        let mut sink = std::io::sink();
        let gid = match aria2.start_download(uri, &mut sink, Some(aria2_media_type.clone()), Some(title), year).await {
            Ok(g)  => g,
            Err(e) => { self.push_ended("error", &e.to_string()).await; return; }
        };

        // Calculate and set organized path based on media type and title
        let organized_base = self.calculate_organized_path(&aria2_media_type, title, year);
        aria2.set_organized_base(&gid, organized_base.clone()).await;

        // Emit download_started with human-readable title to IPC.
        let started = serde_json::to_string(&serde_json::json!({
            "type":  "download_started",
            "gid":   &gid,
            "title": title,
            "uri":   uri,
        })).unwrap_or_default();
        let _ = self.ipc_tx.send(started).await;

        info!("player_bridge: aria2 gid={gid} organized={}", organized_base.display());

        // ── Adaptive pre-roll (blocks until buffer is safe) ───────────────
        let streamer = Streamer::new(self.ipc_tx.clone());

        let (file_path, plan) = match streamer
            .wait_for_preroll(aria2.client(), &gid, 0.0)
            .await
        {
            Some(r) => r,
            None    => {
                self.push_ended("error", "download timed out or failed").await;
                return;
            }
        };

        // Update watch history with the downloaded file path
        self.watch_history.update_file_path(entry_id, &file_path).await;
        debug!(entry_id = %entry_id, path = %file_path, "updated watch history with file path");

        // ── Launch mpv ────────────────────────────────────────────────────
        if !self.playback_cfg.terminal_vo.is_empty() {
            self.push_terminal_takeover().await;
        }
        let mut extra_flags = self.playback_cfg.mpv_extra_flags.clone();
        extra_flags.extend(self.mpv_dsp_flags().await);
        if let Err(e) = self.mpv.play(
            &file_path, title, sub, &self.data_dir,
            &extra_flags,
            &self.playback_cfg.terminal_vo,
        ).await {
            error!("player_bridge: mpv failed: {e}");
            self.push_ended("error", &e).await;
            return;
        }

        // ── Stall guard (background task) ─────────────────────────────────
        let bridge2  = self.clone();
        let gid2     = gid.clone();
        let streamer2 = streamer.clone();
        tokio::spawn(async move {
            let Some(ref aria2) = bridge2.aria2 else {
                warn!("stall guard: aria2 unavailable; skipping stall guard");
                return;
            };
            streamer2.run_stall_guard(
                aria2.client(),
                &gid2,
                &bridge2.mpv,
                plan,
            ).await;
        });

        // ── Feed mpv position into streamer (for stall-guard accuracy) ────
        let streamer3 = streamer.clone();
        let mut rx = self.mpv.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(MpvEvent::Progress(p)) => {
                        streamer3.on_mpv_progress(p.position, p.duration).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        });
    }

    async fn launch_mpv(&self, url: &str, title: &str, sub: Option<&str>) {
        if !self.playback_cfg.terminal_vo.is_empty() {
            self.push_terminal_takeover().await;
        }
        let mut extra_flags = self.playback_cfg.mpv_extra_flags.clone();
        extra_flags.extend(self.mpv_dsp_flags().await);
        if let Err(e) = self.mpv.play(
            url, title, sub, &self.data_dir,
            &extra_flags,
            &self.playback_cfg.terminal_vo,
        ).await {
            error!("player_bridge: mpv launch failed: {e}");
            self.push_ended("error", &e).await;
        }
    }

    async fn play_via_mpd(&self, mpd: &MpdBridge, url: &str, title: &str) {
        if is_torrent_url(url) || is_magnet(url) {
            // Download via aria2 first, then hand local path to MPD
            let Some(aria2) = &self.aria2 else {
                warn!("player_bridge: aria2 not available for audio torrent");
                self.push_ended("error", "aria2 not running").await;
                return;
            };
            let mut sink = std::io::sink();
            let gid = match aria2.start_download(url, &mut sink, Some(Aria2MediaType::Music), Some(title), None).await {
                Ok(g)  => g,
                Err(e) => {
                    let msg = e.to_string();
                    self.push_ended("error", &msg).await;
                    return;
                }
            };
            let started = serde_json::to_string(&serde_json::json!({
                "type":  "download_started",
                "gid":   &gid,
                "title": title,
                "uri":   url,
            })).unwrap_or_default();
            let _ = self.ipc_tx.send(started).await;
            let streamer = crate::streamer::Streamer::new(self.ipc_tx.clone());
            let Some((file_path, _)) = streamer.wait_for_preroll(aria2.client(), &gid, 0.0).await else {
                self.push_ended("error", "download timed out").await;
                return;
            };
            let file_url = format!("file://{file_path}");
            if let Err(e) = mpd.queue_and_play(&file_url).await {
                error!("player_bridge: mpd play failed: {e}");
                let msg = e.to_string();
                self.push_ended("error", &msg).await;
            }
        } else if let Err(e) = mpd.queue_and_play(url).await {
            error!("player_bridge: mpd play failed: {e}");
            let msg = e.to_string();
            self.push_ended("error", &msg).await;
        }
    }

    async fn push_terminal_takeover(&self) {
        let msg = serde_json::to_string(&serde_json::json!({
            "type": "player_terminal_takeover",
            "vo":   &self.playback_cfg.terminal_vo,
        })).unwrap_or_default();
        let _ = self.ipc_tx.send(msg).await;
    }

    async fn push_ended(&self, reason: &'static str, err: &str) {
        let msg = serde_json::to_string(&PlayerEndedWire {
            r#type: "player_ended",
            reason,
            error:  err.to_string(),
        }).unwrap_or_default();
        let _ = self.ipc_tx.send(msg).await;
    }
}

// ── Mpv event forwarder task ──────────────────────────────────────────────────

async fn run_mpv_event_forwarder(mpv: MpvPlayer, tx: mpsc::Sender<String>) {
    loop {
        let mut rx = mpv.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let wire = match event {
                        MpvEvent::Started(e) => serde_json::to_string(&PlayerStartedWire {
                            r#type:   "player_started",
                            title:    &e.title,
                            path:     &e.path,
                            duration: e.duration,
                        }).ok(),
                        MpvEvent::Progress(e) => serde_json::to_string(&PlayerProgressWire {
                            r#type:        "player_progress",
                            position:      e.position,
                            duration:      e.duration,
                            paused:        e.paused,
                            cache_percent: e.cache_percent,
                        }).ok(),
                        MpvEvent::Ended(r) => {
                            let (reason, error) = match r {
                                PlayerEndedReason::Eof        => ("eof",   String::new()),
                                PlayerEndedReason::Quit       => ("quit",  String::new()),
                                PlayerEndedReason::Error(msg) => ("error", msg),
                            };
                            serde_json::to_string(&PlayerEndedWire {
                                r#type: "player_ended",
                                reason,
                                error,
                            }).ok()
                        }
                        MpvEvent::TracksUpdated(tracks) => {
                            serde_json::to_string(&serde_json::json!({
                                "type": "player_tracks_updated",
                                "tracks": tracks.iter().map(|t| serde_json::json!({
                                    "id":         t.id,
                                    "track_type": t.track_type,
                                    "lang":       t.lang,
                                    "title":      t.title,
                                    "selected":   t.selected,
                                    "external":   t.external,
                                })).collect::<Vec<_>>(),
                            })).ok()
                        }
                    };
                    if let Some(msg) = wire {
                        let _ = tx.send(msg).await;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("mpv event channel lagged {n}");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn is_magnet(url: &str)     -> bool { url.starts_with("magnet:") }

fn is_torrent_url(url: &str) -> bool {
    let u = url.to_lowercase();
    u.ends_with(".torrent")
        || u.contains("/download/torrent/")
        || u.contains("/torrent/download")
}

fn find_subtitle(data_dir: &str, imdb_id: &str) -> Option<String> {
    if imdb_id.is_empty() { return None; }
    let sub_dir = format!("{}/subtitles/{}", data_dir, imdb_id);
    for ext in &["en.srt", "srt", "en.ass", "ass"] {
        let path = format!("{}/{}", sub_dir, ext);
        if std::path::Path::new(&path).exists() { return Some(path); }
    }
    if let Ok(mut rd) = std::fs::read_dir(&sub_dir) {
        if let Some(Ok(e)) = rd.next() {
            return Some(e.path().display().to_string());
        }
    }
    None
}

impl PlayerBridge {
    fn calculate_organized_path(&self, media_type: &Aria2MediaType, title: &str, year: Option<u32>) -> std::path::PathBuf {
        use Aria2MediaType::*;
        match media_type {
            Movie | AnimeMovie => self.storage.movie_folder(title, year),
            Series | AnimeSeries => self.storage.series_folder(title, year),
            Music => self.storage.artist_folder(title),
            Podcast => self.storage.podcast_folder(title),
        }
    }
}
