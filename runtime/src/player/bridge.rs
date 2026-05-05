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
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::ConfigManager;
use crate::config::types::PlaybackConfig;
use crate::dsp::DspPipeline;
use crate::engine::Engine;
use crate::mpd_bridge::MpdBridge;
use crate::ipc::{MediaTab, MediaType};
use crate::torrent_engine::TorrentEngine;
use super::mpv::{MpvEvent, MpvPlayer, PlayerEndedReason};

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
    /// Embedded librqbit-backed torrent engine. Always present — librqbit boots
    /// in-process so there is no "is the daemon running?" question that
    /// the old `Option<Aria2Bridge>` had to answer.
    torrents:     Arc<TorrentEngine>,
    mpd:          Option<MpdBridge>,
    engine:       Arc<Engine>,
    config:       Arc<ConfigManager>,
    /// Library-organisation helper. Currently unused after the aria2 → librqbit
    /// swap; will be re-wired in Task 10 when organize-on-complete lands on
    /// `DownloadTranslator`.
    #[allow(dead_code)]
    storage:      Arc<crate::storage::MediaStorage>,
    watch_history: Arc<crate::watchhistory::WatchHistoryStore>,
    ipc_tx:       mpsc::Sender<String>,
    data_dir:     String,
    playback_cfg: PlaybackConfig,
    /// DSP pipeline reference. When present and `config.enabled`, mpv receives
    /// equivalent `--af` / `--audio-samplerate` flags so movie presets apply.
    dsp:          Option<Arc<tokio::sync::Mutex<DspPipeline>>>,
    /// Tracks whether MPD is the active player (true) or mpv (false).
    mpd_active:   Arc<AtomicBool>,
}

impl PlayerBridge {
    pub fn new(
        engine:       Arc<Engine>,
        config:       Arc<ConfigManager>,
        torrents:     Arc<TorrentEngine>,
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

        PlayerBridge { mpv, torrents, mpd, engine, config, storage, watch_history, ipc_tx, data_dir, playback_cfg, dsp, mpd_active: Arc::new(AtomicBool::new(false)) }
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

    #[allow(dead_code)] // pub API: used by PlayerManager
    /// Access the underlying `MpvPlayer` for typed command dispatch.
    pub fn mpv(&self) -> &MpvPlayer {
        &self.mpv
    }

    /// Returns `true` when MPD is the active player (last `play` routed to MPD).
    pub fn is_mpd_active(&self) -> bool {
        self.mpd_active.load(Ordering::Relaxed)
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

        // Subtitle auto-download prelude. Best-effort: any failure falls
        // through to the existing sidecar helper. 5s total cap on fetch so
        // mpv warmup isn't blocked.
        let cfg_snap = self.config.snapshot().await;
        if cfg_snap.subtitles.auto_download && !imdb_id.is_empty() {
            let kind = match tab {
                Some(crate::ipc::MediaTab::Series) =>
                    stui_plugin_sdk::EntryKind::Series,
                _ => stui_plugin_sdk::EntryKind::Movie,
            };
            let lang = cfg_snap.subtitles.preferred_language.clone();
            let engine = self.engine.clone();
            let title_owned = title.to_string();
            let imdb_owned = imdb_id.to_string();
            let data_dir = self.data_dir.clone();
            let tx = self.ipc_tx.clone();

            // 5s cap — on timeout, the unwrap_or_default returns an empty Vec.
            let fetched = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                crate::engine::subtitles::fetch_subtitles(
                    &engine, &title_owned, Some(&imdb_owned), kind, &lang,
                ),
            ).await.unwrap_or_default();

            if let Some(candidate) = fetched.into_iter().next() {
                match Self::download_subtitle_candidate(&engine, &candidate, &imdb_owned, &data_dir).await {
                    Ok(file_name) => {
                        let msg = serde_json::to_string(&serde_json::json!({
                            "type":      "subtitle_fetched",
                            "language":  candidate.language.unwrap_or_else(|| "unknown".into()),
                            "provider":  candidate.plugin_name,
                            "file_name": file_name,
                        })).unwrap_or_default();
                        let _ = tx.send(msg).await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "subtitle download failed");
                        let msg = serde_json::to_string(&serde_json::json!({
                            "type":   "subtitle_search_failed",
                            "reason": e.to_string(),
                        })).unwrap_or_default();
                        let _ = tx.send(msg).await;
                    }
                }
            }
        }
        drop(cfg_snap);

        let sub_path = find_subtitle(&self.data_dir, imdb_id);
        self.start_stream(entry_id, &stream_url, title, sub_path.as_deref(), media_type, year).await;
    }

    pub async fn stop(&self) {
        self.mpv.stop().await;
    }

    /// Download a torrent/magnet URL via the embedded torrent engine without
    /// launching mpv. A background task awaits completion and logs the
    /// outcome.
    ///
    /// NOTE: organize-on-complete wiring (the old aria2 per-GID monitor →
    /// `DownloadTranslator` flow) is intentionally deferred — see Task 9/10
    /// in the librqbit migration plan. For now the file simply lands in
    /// `TorrentEngine::staging_dir()`.
    pub async fn download_only(&self, url: &str, title: &str, _media_type: Option<MediaType>, _year: Option<u32>) {
        let dl = match self.torrents.start_download(url).await {
            Ok(d)  => d,
            Err(e) => {
                warn!("player_bridge: download_only torrent error: {e}");
                return;
            }
        };
        let started = serde_json::to_string(&serde_json::json!({
            "type":  "download_started",
            "title": title,
            "uri":   url,
        })).unwrap_or_default();
        let _ = self.ipc_tx.send(started).await;
        info!("player_bridge: download_only torrent_id={}", dl.torrent_id);

        tokio::spawn(async move {
            match dl.completion.await {
                Ok(Ok(()))  => info!("download_only: completed torrent_id={}", dl.torrent_id),
                Ok(Err(e))  => warn!("download_only: torrent failed: {e}"),
                Err(_)      => warn!("download_only: completion channel dropped"),
            }
        });
    }

    /// Cancel an active torrent download.
    ///
    /// Until the librqbit migration finishes wiring per-torrent IDs through
    /// the IPC layer (see Task 10), this no-ops. The previous aria2 GID
    /// shape isn't a valid librqbit handle, so we'd just log and do nothing
    /// either way.
    pub async fn cancel_download(&self, _id: &str) {
        warn!("player_bridge: cancel_download not yet wired to TorrentEngine");
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

    /// Cold-start playback from a URL when mpv isn't running. Used by
    /// the SwitchStream IPC path so users can pick a stream from the
    /// stream-picker UI without first triggering a `play()` via the
    /// provider row. Title is derived from the URL since we have no
    /// catalog context here; subtitles are skipped (no imdb_id to
    /// drive the auto-download flow).
    pub async fn start_stream_for_switch(&self, url: &str) {
        let title = title_from_url(url);
        let entry_id = format!("switch_stream|{title}");
        info!("player_bridge: cold-starting playback for switch_stream url={}", &url[..url.len().min(80)]);
        self.start_stream(&entry_id, url, &title, None, None, None).await;
    }

    pub async fn switch_stream_mpd(&self, url: &str, title: &str) {
        let Some(ref mpd) = self.mpd else {
            warn!("switch_stream_mpd called but MPD not configured");
            return;
        };
        match mpd.queue_and_play(url).await {
            Ok(()) => {
                self.mpd_active.store(true, Ordering::Relaxed);
                let duration = mpd.current_song_duration_with_retry(10, 50).await;
                self.push_started(title, url, duration).await;
            }
            Err(e) => {
                self.mpd_active.store(false, Ordering::Relaxed);
                error!("player_bridge: switch_stream_mpd failed: {e}");
                self.push_ended("error", &e.to_string()).await;
            }
        }
    }

    // ── Routing ───────────────────────────────────────────────────────────────

    async fn start_stream(&self, entry_id: &str, url: &str, title: &str, sub: Option<&str>, media_type: Option<MediaType>, year: Option<u32>) {
        if is_torrent_url(url) || is_magnet(url) {
            self.play_via_torrent(entry_id, url, title, sub, media_type, year).await;
        } else {
            self.launch_mpv(url, title, sub).await;
        }
    }

    /// Stream a torrent/magnet by handing librqbit's HTTP URL straight to
    /// mpv. mpv handles its own buffering against librqbit's Range-supporting
    /// HTTP server, so the old preroll / stall-guard machinery is gone.
    async fn play_via_torrent(
        &self,
        entry_id: &str,
        uri: &str,
        title: &str,
        sub: Option<&str>,
        _media_type: Option<MediaType>,
        _year: Option<u32>,
    ) {
        self.mpd_active.store(false, Ordering::Relaxed);

        let stream_url = match self.torrents.start_stream(uri).await {
            Ok(u)  => u,
            Err(e) => {
                error!("player_bridge: torrent_engine.start_stream failed: {e:#}");
                self.push_ended("error", &format!("torrent error: {e}")).await;
                return;
            }
        };

        // Emit a UI event so the TUI can show "Buffering…" until mpv
        // reports playback_restart. No preroll wait on our side.
        let started = serde_json::to_string(&serde_json::json!({
            "type":  "download_started",
            "title": title,
            "uri":   uri,
        })).unwrap_or_default();
        let _ = self.ipc_tx.send(started).await;

        info!("player_bridge: streaming {title} via torrent_engine ({stream_url})");

        // Update watch history with the streaming URL so resume works.
        self.watch_history.update_file_path(entry_id, &stream_url).await;

        if !self.playback_cfg.terminal_vo.is_empty() {
            self.push_terminal_takeover().await;
        }
        let mut extra_flags = self.playback_cfg.mpv_extra_flags.clone();
        extra_flags.extend(self.mpv_dsp_flags().await);
        if let Err(e) = self.mpv.play(
            &stream_url, title, sub, &self.data_dir,
            &extra_flags,
            &self.playback_cfg.terminal_vo,
        ).await {
            error!("player_bridge: mpv failed: {e}");
            self.push_ended("error", &e).await;
        }
    }

    async fn launch_mpv(&self, url: &str, title: &str, sub: Option<&str>) {
        self.mpd_active.store(false, Ordering::Relaxed);
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
            // Audio torrents: librqbit's per-file HTTP endpoint isn't a great
            // fit for MPD (which wants a stable file:// or http stream that
            // doesn't reshuffle pieces). Pre-download the whole torrent —
            // music payloads are small enough that the wait is acceptable —
            // then hand the staged path to MPD.
            let dl = match self.torrents.start_download(url).await {
                Ok(d)  => d,
                Err(e) => {
                    self.push_ended("error", &e.to_string()).await;
                    return;
                }
            };
            let started = serde_json::to_string(&serde_json::json!({
                "type":  "download_started",
                "title": title,
                "uri":   url,
            })).unwrap_or_default();
            let _ = self.ipc_tx.send(started).await;

            let final_rel = dl.final_path.clone();
            match dl.completion.await {
                Ok(Ok(()))  => {}
                Ok(Err(e))  => {
                    self.push_ended("error", &format!("torrent failed: {e}")).await;
                    return;
                }
                Err(_) => {
                    self.push_ended("error", "torrent completion channel dropped").await;
                    return;
                }
            }

            let abs_path = self.torrents.staging_dir().join(&final_rel);
            let file_url = format!("file://{}", abs_path.display());
            match mpd.queue_and_play(&file_url).await {
                Ok(()) => {
                    self.mpd_active.store(true, Ordering::Relaxed);
                    let duration = mpd.current_song_duration_with_retry(10, 50).await;
                    self.push_started(title, &file_url, duration).await;
                }
                Err(e) => {
                    self.mpd_active.store(false, Ordering::Relaxed);
                    error!("player_bridge: mpd play failed: {e}");
                    self.push_ended("error", &e.to_string()).await;
                }
            }
        } else {
            match mpd.queue_and_play(url).await {
                Ok(()) => {
                    self.mpd_active.store(true, Ordering::Relaxed);
                    let duration = mpd.current_song_duration_with_retry(10, 50).await;
                    self.push_started(title, url, duration).await;
                }
                Err(e) => {
                    self.mpd_active.store(false, Ordering::Relaxed);
                    error!("player_bridge: mpd play failed: {e}");
                    self.push_ended("error", &e.to_string()).await;
                }
            }
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

    async fn push_started(&self, title: &str, path: &str, duration: f64) {
        let msg = serde_json::to_string(&PlayerStartedWire {
            r#type:   "player_started",
            title,
            path,
            duration,
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

/// Best-effort title extraction from a stream URL. Used by the
/// SwitchStream cold-start path where we have no catalog context.
/// Magnets: parse `dn=` parameter. HTTP: take the last path segment
/// (minus query string). Falls back to a generic label if neither
/// produces something useful.
fn title_from_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("magnet:?") {
        for kv in rest.split('&') {
            if let Some(name) = kv.strip_prefix("dn=") {
                let decoded = urlencoding::decode(name).map(|s| s.into_owned()).unwrap_or_default();
                if !decoded.is_empty() { return decoded; }
            }
        }
        return "Magnet stream".to_string();
    }
    if let Some(last) = url.rsplit('/').next() {
        let segment = last.split('?').next().unwrap_or(last);
        if !segment.is_empty() { return segment.to_string(); }
    }
    "Stream".to_string()
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
    // NOTE: calculate_organized_path was removed alongside play_via_aria2.
    // The download_translator now owns "where does this file end up" once
    // organize-on-complete is wired up against TorrentEngine (Task 10+).

    /// Resolve the candidate's entry_id to a subtitle URL via the plugin,
    /// HTTP-GET the file to the canonical sidecar path. Returns the basename
    /// of the written file.
    ///
    /// Layout: `{data_dir}/subtitles/{imdb_id}/{lang}.srt` matches the
    /// layout that `find_subtitle` already scans, so the file picks up
    /// automatically on the next `find_subtitle` call in the play path.
    async fn download_subtitle_candidate(
        engine: &Engine,
        candidate: &crate::engine::subtitles::SubtitleCandidate,
        imdb_id: &str,
        data_dir: &str,
    ) -> anyhow::Result<String> {
        // 1. Resolve — 10s cap.
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            crate::engine::subtitles::call_plugin_resolve(
                engine, &candidate.plugin_id, &candidate.entry.id,
            ),
        ).await
            .map_err(|_| anyhow::anyhow!("subtitle resolve timeout (10s)"))?
            .map_err(|e| anyhow::anyhow!("subtitle resolve: {e}"))?;

        let url = resp.stream_url;
        if url.is_empty() {
            anyhow::bail!("subtitle resolve returned empty stream_url");
        }

        // 2. Compose sidecar path.
        let lang = candidate.language.as_deref().unwrap_or("unknown");
        let sub_dir = format!("{data_dir}/subtitles/{imdb_id}");
        tokio::fs::create_dir_all(&sub_dir).await
            .map_err(|e| anyhow::anyhow!("mkdir {sub_dir}: {e}"))?;
        let file_name = format!("{lang}.srt");
        let file_path = format!("{sub_dir}/{file_name}");

        // 3. HTTP GET — 15s cap. Subtitle files are typically <100KB;
        // aria2 is overkill for a one-shot fetch.
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            async {
                reqwest::get(&url).await
                    .map_err(|e| anyhow::anyhow!("GET {url}: {e}"))?
                    .error_for_status()
                    .map_err(|e| anyhow::anyhow!("GET {url}: HTTP {e}"))?
                    .bytes().await
                    .map_err(|e| anyhow::anyhow!("read {url}: {e}"))
            },
        ).await
            .map_err(|_| anyhow::anyhow!("subtitle download timeout (15s)"))??;

        tokio::fs::write(&file_path, &bytes).await
            .map_err(|e| anyhow::anyhow!("write {file_path}: {e}"))?;

        tracing::info!(plugin = %candidate.plugin_name, path = %file_path,
                       "subtitle downloaded");
        Ok(file_name)
    }
}
