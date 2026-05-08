// mpv.rs — async mpv process manager for stui.
//
// Responsibilities:
//   1. Spawn mpv with --input-ipc-server=/tmp/stui-mpv.sock and the right flags
//   2. Connect to the Unix socket after mpv starts
//   3. Send JSON IPC commands (pause, seek, quit, cycle fullscreen …)
//   4. Receive property-change events, map to PlayerProgress / PlayerEnded
//   5. Notify the player_bridge via a tokio broadcast channel
//
// mpv JSON IPC wire format:
//   Commands  (we send):  {"command":["property_name","value"], "request_id": N}
//   Responses (we recv):  {"request_id":N,"error":"success","data":...}
//   Events    (we recv):  {"event":"property-change","name":"time-pos","data":42.1}
//                         {"event":"end-file","reason":"eof"}
//
// Flags we always pass to mpv:
//   --input-ipc-server     Unix socket for JSON IPC
//   --no-terminal          Don't claim stdin/stdout
//   --really-quiet         Suppress all console output
//   --keep-open=no         Exit when playback ends
//   --idle=no              Don't idle after EOF
//   --script-opts=ytdl_hook-ytdl_path=yt-dlp  (if yt-dlp is available)
//   --sub-auto=fuzzy       Auto-load subtitles from the same directory
//   --sub-file-paths       Search path for downloaded subtitle files

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::state::TrackInfo;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};
use tokio::time::timeout;
use tracing::{debug, error, info};

// ── Public event types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlayerStartedEvent {
    pub title: String,
    pub path: String,
    pub duration: f64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // pub API: used by TUI / IPC layer
pub struct PlayerProgressEvent {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub cache_percent: f64,
    // Extended fields (populated once mpv reports them)
    pub audio_track: Option<i64>,
    pub subtitle_track: Option<i64>,
    pub subtitle_delay: f64,
    pub audio_delay: f64,
    pub volume: f64,
    pub muted: bool,
}

#[derive(Debug, Clone)]
pub enum PlayerEndedReason {
    Eof,
    Quit,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum MpvEvent {
    Started(PlayerStartedEvent),
    Progress(PlayerProgressEvent),
    Ended(PlayerEndedReason),
    /// Emitted when mpv's track-list property changes (e.g. after file load).
    TracksUpdated(Vec<TrackInfo>),
}

// ── MpvPlayer ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct MpvPlayer {
    inner: Arc<MpvInner>,
}

struct MpvInner {
    sock_path: PathBuf,
    req_id: AtomicU64,
    // locked while a process is alive
    proc: Mutex<Option<Child>>,
    // write half of the Unix socket — None until connected
    sock_tx: Mutex<Option<tokio::net::unix::OwnedWriteHalf>>,
    event_tx: broadcast::Sender<MpvEvent>,
}

impl Default for MpvPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl MpvPlayer {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(64);
        MpvPlayer {
            inner: Arc::new(MpvInner {
                sock_path: PathBuf::from("/tmp/stui-mpv.sock"),
                req_id: AtomicU64::new(1),
                proc: Mutex::new(None),
                sock_tx: Mutex::new(None),
                event_tx,
            }),
        }
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub fn subscribe(&self) -> broadcast::Receiver<MpvEvent> {
        self.inner.event_tx.subscribe()
    }

    /// Launch mpv with the given URL/path and subtitle file (optional).
    /// Kills any currently running instance first.
    ///
    /// `extra_flags`   — additional flags appended verbatim (from `playback.mpv_extra_flags`)
    /// `terminal_vo`   — if non-empty, use this as the `--vo` driver (e.g. `"kitty"`,
    ///                   `"sixel"`, `"tct"`).  Terminal-mode omits `--no-terminal` and
    ///                   leaves stdout/stderr connected so mpv can render to the terminal.
    pub async fn play(
        &self,
        url: &str,
        title: &str,
        subtitle_path: Option<&str>,
        stui_data_dir: &str,
        extra_flags: &[String],
        terminal_vo: &str,
    ) -> Result<(), String> {
        self.stop().await;

        // Remove stale socket
        let _ = tokio::fs::remove_file(&self.inner.sock_path).await;

        let terminal_mode = !terminal_vo.is_empty();

        let mut args: Vec<String> = vec![
            url.to_string(),
            format!("--input-ipc-server={}", self.inner.sock_path.display()),
            "--really-quiet".into(),
            "--keep-open=no".into(),
            "--idle=no".into(),
            // Auto-load .srt/.ass from the same dir as the file
            "--sub-auto=fuzzy".into(),
            // Also search in stui subtitle cache
            format!("--sub-file-paths={}/subtitles", stui_data_dir),
            // Use yt-dlp if available for web URLs
            "--script-opts=ytdl_hook-ytdl_path=yt-dlp".into(),
            // Observe properties — mpv will push events when they change
            "--observe-properties=yes".into(),
        ];

        // Terminal mode: use specified VO driver; graphical mode: suppress terminal UI
        if terminal_mode {
            args.push(format!("--vo={}", terminal_vo));
        } else {
            args.push("--no-terminal".into());
        }

        if let Some(sub) = subtitle_path {
            args.push(format!("--sub-file={}", sub));
        }

        // title for the window / OSD
        if !title.is_empty() {
            args.push(format!("--force-media-title={}", title));
            if !terminal_mode {
                args.push(format!("--title=stui — {}", title));
            }
        }

        // User-supplied extra flags (e.g. --hwdec=vaapi, custom shaders)
        args.extend_from_slice(extra_flags);

        let mut cmd = Command::new("mpv");
        cmd.args(&args).stdin(Stdio::null());

        if terminal_mode {
            // Leave stdout/stderr connected so mpv can render video to the terminal
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn mpv: {e} — is mpv installed?"))?;

        *self.inner.proc.lock().await = Some(child);

        info!("mpv: spawned for {:?}", &url[..url.len().min(80)]);

        // Spawn socket connector + event reader
        let player = self.clone();
        let url_owned = url.to_string();
        let title_owned = title.to_string();
        tokio::spawn(async move {
            player.run_ipc_loop(url_owned, title_owned).await;
        });

        Ok(())
    }

    /// Stop mpv gracefully (sends quit command, then kills if needed).
    pub async fn stop(&self) {
        // Send quit command over socket if connected
        let _ = self.send_command(&json!(["quit"])).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Kill the process if still running
        if let Some(mut child) = self.inner.proc.lock().await.take() {
            let _ = child.kill().await;
        }
        *self.inner.sock_tx.lock().await = None;
    }

    /// Check whether an mpv child process exists AND its stdin pipe is still
    /// alive. Used by the SwitchStream IPC handler to decide between
    /// loadfile-into-existing-mpv vs cold-starting a fresh playback.
    /// Side effect: if the child has exited, clears the proc slot so
    /// the next `play()` call gets a clean spawn.
    pub async fn is_running(&self) -> bool {
        let mut guard = self.inner.proc.lock().await;
        let Some(child) = guard.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true, // still running
            Ok(Some(_)) => {
                // exited; clean up
                *guard = None;
                false
            }
            Err(_) => true, // assume alive on error
        }
    }

    /// Send a raw mpv IPC command array, e.g. `["cycle","pause"]`.
    pub async fn send_command(&self, cmd: &Value) -> Result<(), String> {
        let req_id = self.inner.req_id.fetch_add(1, Ordering::Relaxed);
        let msg = serde_json::to_string(&json!({
            "command": cmd,
            "request_id": req_id,
        }))
        .map_err(|e| e.to_string())?;

        let mut guard = self.inner.sock_tx.lock().await;
        if let Some(tx) = guard.as_mut() {
            let mut line = msg;
            line.push('\n');
            timeout(Duration::from_secs(5), tx.write_all(line.as_bytes()))
                .await
                .map_err(|_| "mpv IPC write timed out after 5s".to_string())?
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // ── High-level typed commands ────────────────────────────────────────────

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn set_pause(&self, paused: bool) -> Result<(), String> {
        self.send_command(&json!(["set_property", "pause", paused]))
            .await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn toggle_pause(&self) -> Result<(), String> {
        self.send_command(&json!(["cycle", "pause"])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn seek_relative(&self, delta: f64) -> Result<(), String> {
        self.send_command(&json!(["seek", delta, "relative"])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn seek_absolute(&self, pos: f64) -> Result<(), String> {
        self.send_command(&json!(["seek", pos, "absolute"])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn set_volume(&self, level: f64) -> Result<(), String> {
        self.send_command(&json!(["set_property", "volume", level]))
            .await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn adjust_volume(&self, delta: f64) -> Result<(), String> {
        self.send_command(&json!(["add", "volume", delta])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn toggle_mute(&self) -> Result<(), String> {
        self.send_command(&json!(["cycle", "mute"])).await
    }

    // ── Subtitle control ──────────────────────────────────────────────────

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn set_subtitle_track(&self, id: i64) -> Result<(), String> {
        self.send_command(&json!(["set_property", "sid", id])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn disable_subtitles(&self) -> Result<(), String> {
        self.send_command(&json!(["set_property", "sid", "no"]))
            .await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn cycle_subtitles(&self) -> Result<(), String> {
        self.send_command(&json!(["cycle", "sub"])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn adjust_sub_delay(&self, delta: f64) -> Result<(), String> {
        self.send_command(&json!(["add", "sub-delay", delta])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn reset_sub_delay(&self) -> Result<(), String> {
        self.send_command(&json!(["set_property", "sub-delay", 0.0]))
            .await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn load_subtitle(&self, path: &str) -> Result<(), String> {
        self.send_command(&json!(["sub-add", path, "select"])).await
    }

    // ── Audio track control ───────────────────────────────────────────────

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn set_audio_track(&self, id: i64) -> Result<(), String> {
        self.send_command(&json!(["set_property", "aid", id])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn cycle_audio_tracks(&self) -> Result<(), String> {
        self.send_command(&json!(["cycle", "audio"])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn adjust_audio_delay(&self, delta: f64) -> Result<(), String> {
        self.send_command(&json!(["add", "audio-delay", delta]))
            .await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn reset_audio_delay(&self) -> Result<(), String> {
        self.send_command(&json!(["set_property", "audio-delay", 0.0]))
            .await
    }

    // ── Stream switching ──────────────────────────────────────────────────

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn loadfile_replace(&self, url: &str) -> Result<(), String> {
        self.send_command(&json!(["loadfile", url, "replace"]))
            .await
    }

    // ── Display ───────────────────────────────────────────────────────────

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn toggle_fullscreen(&self) -> Result<(), String> {
        self.send_command(&json!(["cycle", "fullscreen"])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn screenshot(&self) -> Result<(), String> {
        self.send_command(&json!(["screenshot"])).await
    }

    // ── Internal: IPC socket loop ─────────────────────────────────────────────

    async fn run_ipc_loop(&self, url: String, title: String) {
        // Wait for mpv to create the socket (up to 5 seconds)
        let sock = match self.wait_for_socket().await {
            Some(s) => s,
            None => {
                error!("mpv: socket never appeared — mpv may have failed to start");
                let _ = self
                    .inner
                    .event_tx
                    .send(MpvEvent::Ended(PlayerEndedReason::Error(
                        "mpv socket timeout".into(),
                    )));
                return;
            }
        };

        let (rx_half, tx_half) = sock.into_split();
        *self.inner.sock_tx.lock().await = Some(tx_half);

        // Subscribe to all properties we care about.
        // IDs are stable — used to quickly identify events in the loop below.
        for (prop, id) in &[
            ("time-pos", 1u64),
            ("duration", 2),
            ("pause", 3),
            ("cache-buffering-state", 4),
            ("media-title", 5),
            ("aid", 6), // active audio track
            ("sid", 7), // active subtitle track
            ("sub-delay", 8),
            ("audio-delay", 9),
            ("volume", 10),
            ("mute", 11),
            ("track-list", 12), // full track list (parsed separately)
        ] {
            let _ = self
                .send_command(&json!(["observe_property", id, prop]))
                .await;
        }

        info!("mpv: IPC connected — reading events");

        // State tracked across events — mirrors PlaybackState
        let mut position = 0f64;
        let mut duration = 0f64;
        let mut paused = false;
        let mut cache_pct = 100f64;
        let mut audio_track: Option<i64> = None;
        let mut sub_track: Option<i64> = None;
        let mut sub_delay = 0f64;
        let mut audio_delay = 0f64;
        let mut volume = 100f64;
        let mut muted = false;
        let mut media_title = if title.is_empty() {
            url.clone()
        } else {
            title.clone()
        };
        let mut started_sent = false;

        let mut lines = BufReader::new(rx_half).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let val: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // ── Property-change event ──────────────────────────────────────
            if val.get("event").and_then(|e| e.as_str()) == Some("property-change") {
                let name = val.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let data = &val["data"];

                match name {
                    "time-pos" => {
                        if let Some(p) = data.as_f64() {
                            position = p;
                        }
                    }
                    "duration" => {
                        if let Some(d) = data.as_f64() {
                            duration = d;
                            // Emit started once we know the duration
                            if !started_sent {
                                started_sent = true;
                                let _ = self.inner.event_tx.send(MpvEvent::Started(
                                    PlayerStartedEvent {
                                        title: media_title.clone(),
                                        path: url.clone(),
                                        duration: d,
                                    },
                                ));
                            }
                        }
                    }
                    "pause" => {
                        paused = data.as_bool().unwrap_or(false);
                    }
                    "cache-buffering-state" => {
                        cache_pct = data.as_f64().unwrap_or(100.0);
                    }
                    "media-title" => {
                        if let Some(t) = data.as_str() {
                            media_title = t.to_string();
                            if !started_sent {
                                started_sent = true;
                                let _ = self.inner.event_tx.send(MpvEvent::Started(
                                    PlayerStartedEvent {
                                        title: media_title.clone(),
                                        path: url.clone(),
                                        duration,
                                    },
                                ));
                            }
                        }
                    }
                    "aid" => {
                        audio_track = data.as_i64();
                    }
                    "sid" => {
                        // mpv sends "no" when subs are disabled
                        sub_track = data.as_i64();
                    }
                    "sub-delay" => {
                        sub_delay = data.as_f64().unwrap_or(0.0);
                    }
                    "audio-delay" => {
                        audio_delay = data.as_f64().unwrap_or(0.0);
                    }
                    "volume" => {
                        volume = data.as_f64().unwrap_or(100.0);
                    }
                    "mute" => {
                        muted = data.as_bool().unwrap_or(false);
                    }
                    "track-list" => {
                        // Parse the full track list when mpv reports it
                        if let Some(arr) = data.as_array() {
                            let tracks: Vec<TrackInfo> = arr
                                .iter()
                                .filter_map(|t| {
                                    let id = t["id"].as_i64()?;
                                    let track_type = t["type"].as_str()?.to_string();
                                    let lang = t["lang"].as_str().unwrap_or("").to_string();
                                    let title = t["title"].as_str().unwrap_or("").to_string();
                                    let selected = t["selected"].as_bool().unwrap_or(false);
                                    let external = t["external"].as_bool().unwrap_or(false);
                                    Some(TrackInfo {
                                        id,
                                        track_type,
                                        lang,
                                        title,
                                        selected,
                                        external,
                                    })
                                })
                                .collect();
                            // Emit a special tracks event
                            let _ = self.inner.event_tx.send(MpvEvent::TracksUpdated(tracks));
                        }
                    }
                    _ => {}
                }

                // Push a full progress snapshot whenever position or pause state changes
                if name == "time-pos" || name == "pause" {
                    let _ = self
                        .inner
                        .event_tx
                        .send(MpvEvent::Progress(PlayerProgressEvent {
                            position,
                            duration,
                            paused,
                            cache_percent: cache_pct,
                            audio_track,
                            subtitle_track: sub_track,
                            subtitle_delay: sub_delay,
                            audio_delay,
                            volume,
                            muted,
                        }));
                }

                continue;
            }

            // ── end-file event ────────────────────────────────────────────
            if val.get("event").and_then(|e| e.as_str()) == Some("end-file") {
                let reason = val.get("reason").and_then(|r| r.as_str()).unwrap_or("eof");
                let ended = match reason {
                    "eof" => PlayerEndedReason::Eof,
                    "quit" => PlayerEndedReason::Quit,
                    other => PlayerEndedReason::Error(other.to_string()),
                };
                let _ = self.inner.event_tx.send(MpvEvent::Ended(ended));
                break;
            }

            // ── file-loaded — send started if duration wasn't seen yet ────
            if val.get("event").and_then(|e| e.as_str()) == Some("file-loaded") && !started_sent {
                started_sent = true;
                let _ = self
                    .inner
                    .event_tx
                    .send(MpvEvent::Started(PlayerStartedEvent {
                        title: media_title.clone(),
                        path: url.clone(),
                        duration,
                    }));
            }
        }

        // Clean up process entry
        *self.inner.sock_tx.lock().await = None;
        let _ = self.inner.proc.lock().await.take();
        debug!("mpv: IPC loop exited");
    }

    async fn wait_for_socket(&self) -> Option<UnixStream> {
        let path = &self.inner.sock_path;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(s) = UnixStream::connect(path).await {
                return Some(s);
            }
        }
        None
    }
}
