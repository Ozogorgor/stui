// mpv.rs — async mpv process manager for stui.
//
// Responsibilities:
//   1. Spawn mpv with --input-ipc-server=$XDG_RUNTIME_DIR/stui-$PID-mpv.sock and the right flags
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
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

/// stui's mpv-side overlay. Loaded with `--script=…`. See the file for
/// the IPC contract (`script-message stui-status …` etc.). Embedded so
/// the script version is locked to the runtime that ships it.
const OVERLAY_LUA: &str = include_str!("stui-overlay.lua");

/// Per-process socket path for mpv IPC. PID-suffixed so two stui
/// instances don't collide on the same Unix socket. Falls back to
/// `/tmp/` when `XDG_RUNTIME_DIR` isn't set.
fn mpv_socket_path() -> PathBuf {
    let pid = std::process::id();
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join(format!("stui-{pid}-mpv.sock"))
}

/// Per-process mpv log path. Same multi-instance hazard as the socket.
/// Lives under `$XDG_CACHE_HOME/stui/` so it's discoverable for the
/// "why did mpv exit?" debugging loop.
fn mpv_log_path() -> PathBuf {
    let pid = std::process::id();
    let base = dirs::cache_dir()
        .map(|d| d.join("stui"))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let _ = std::fs::create_dir_all(&base);
    base.join(format!("mpv-{pid}.log"))
}

/// Resolve (and lazily materialise on disk) the path mpv loads our
/// overlay script from. Idempotent: if the file already matches the
/// embedded content, we reuse it; otherwise we rewrite it. Returns
/// `None` if we can't find a writeable spot, which makes the caller
/// silently skip `--script=…` rather than refuse to launch.
fn overlay_script_path() -> Option<&'static PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = dirs::cache_dir()?.join("stui").join("scripts");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            error!("mpv: failed to create overlay script dir {}: {e}", dir.display());
            return None;
        }
        let path = dir.join("stui-overlay.lua");
        // Skip the write if the existing file is byte-identical — saves
        // an inotify event for every other process watching the cache.
        let needs_write = std::fs::read_to_string(&path)
            .map(|cur| cur != OVERLAY_LUA)
            .unwrap_or(true);
        if needs_write {
            if let Err(e) = std::fs::write(&path, OVERLAY_LUA) {
                error!("mpv: failed to write overlay script {}: {e}", path.display());
                return None;
            }
            info!("mpv: overlay script written to {}", path.display());
        }
        Some(path)
    }).as_ref()
}

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
    pub title:    String,
    pub path:     String,
    pub duration: f64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // pub API: used by TUI / IPC layer
pub struct PlayerProgressEvent {
    pub position:       f64,
    pub duration:       f64,
    pub paused:         bool,
    pub cache_percent:  f64,
    // Extended fields (populated once mpv reports them)
    pub audio_track:    Option<i64>,
    pub subtitle_track: Option<i64>,
    pub subtitle_delay: f64,
    pub audio_delay:    f64,
    pub volume:         f64,
    pub muted:          bool,
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
    sock_path:  PathBuf,
    req_id:     AtomicU64,
    // locked while a process is alive
    proc:       Mutex<Option<Child>>,
    // write half of the Unix socket — None until connected.
    // Bumps each time a new mpv is spawned (play / play_idle / stop). The
    // value of `ipc_epoch` at the moment a `run_ipc_loop` task was spawned
    // is stored in its locals; on exit, the loop only nulls `sock_tx` when
    // its captured epoch still matches the current value. Without this,
    // an older loop racing against a newer `play_idle` would clear the
    // writer the new loop just installed — every subsequent IPC command
    // would then fall through to the fresh-connect fallback.
    sock_tx:    Mutex<Option<tokio::net::unix::OwnedWriteHalf>>,
    ipc_epoch:  AtomicU64,
    event_tx:   broadcast::Sender<MpvEvent>,
}

impl Default for MpvPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl MpvPlayer {
    pub fn new() -> Self {
        // Channel capacity matters: mpv emits ~60 `time-pos` property-change
        // events per second during playback. With the previous 64-slot
        // buffer, a 1-second hiccup in the consumer (IPC forwarder to TUI)
        // would overflow and we'd see `mpv event channel lagged N` warnings.
        // 4096 absorbs ~60 s of position updates — much longer than any
        // realistic consumer stall. The right long-term fix is to coalesce
        // time-pos updates rather than firing one per tick.
        let (event_tx, _) = broadcast::channel(4096);
        MpvPlayer {
            inner: Arc::new(MpvInner {
                sock_path: mpv_socket_path(),
                req_id: AtomicU64::new(1),
                proc: Mutex::new(None),
                sock_tx: Mutex::new(None),
                ipc_epoch: AtomicU64::new(0),
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
            // Suppress the noisy status line but keep errors and warnings visible
            // on stderr (which we capture into a `WARN target=mpv` log line). Earlier
            // we used --really-quiet, which gagged even fatal messages and made it
            // impossible to tell why mpv had died.
            "--msg-level=statusline=no".into(),
            // Log full mpv diagnostics to a per-PID log under
            // $XDG_CACHE_HOME/stui/ so we can see *why* mpv emits
            // end-file=error within ~45 ms of start-file on cold-start.
            // stderr is block-buffered when piped, so routing through a
            // file is the only way to actually see mpv's stream / demux /
            // http chatter.
            format!("--log-file={}", mpv_log_path().display()),
            "--keep-open=no".into(),
            "--idle=no".into(),
            // Auto-load .srt/.ass from the same dir as the file
            "--sub-auto=fuzzy".into(),
            // Also search in stui subtitle cache
            format!("--sub-file-paths={}/subtitles", stui_data_dir),
            // Use yt-dlp if available for web URLs
            "--script-opts=ytdl_hook-ytdl_path=yt-dlp".into(),
            // stui's overlay owns the centred status text now, so kill
            // mpv's default OSC (which draws the "Drop files or URLs
            // here to play" idle prompt and the play-button logo).
            "--osc=no".into(),
            // Users with modernz / modernx installed at
            // ~/.config/mpv/scripts/ get an *additional* idle prompt
            // drawn by that script — `--osc=no` only suppresses the
            // bundled osc.lua. Tell modernz to skip its idle screen so
            // our overlay isn't fighting another "Drop files…" layer.
            // No-op for users without modernz installed.
            "--script-opts-append=modernz-idlescreen=no".into(),
            // NOTE: properties are observed via the IPC `observe_property` command
            // in run_ipc_loop; there is no `--observe-properties` CLI flag in mpv
            // and passing one made mpv exit with status 1 immediately on launch.
        ];

        // Terminal mode: use specified VO driver; graphical mode: keep terminal
        // bookkeeping out of the way. We deliberately do NOT pass --no-terminal:
        // it makes mpv silence ALL stderr (including [ffmpeg] errors), which
        // turns "mpv died — why?" into an unsolvable mystery. Instead we use
        // --input-terminal=no (don't read keys from stdin) and rely on the
        // Stdio::null() on stdin we set up below to keep mpv from grabbing it.
        if terminal_mode {
            args.push(format!("--vo={}", terminal_vo));
        } else {
            args.push("--input-terminal=no".into());
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
        if let Some(script) = overlay_script_path() {
            args.push(format!("--script={}", script.display()));
        }

        args.extend_from_slice(extra_flags);

        let mut cmd = Command::new("mpv");
        // Kill the child if its `Child` handle gets dropped (e.g. because
        // `run_ipc_loop` lost its IPC connection and `take()`d the proc
        // out of `MpvInner` before `stop()` could SIGKILL it). Without
        // this, every IPC-loop exit before `stop()` orphaned an mpv
        // window — the user observed two/three blank windows accreting
        // across consecutive stream picks.
        cmd.args(&args).stdin(Stdio::null()).kill_on_drop(true);

        if terminal_mode {
            // Leave stdout/stderr connected so mpv can render video to the terminal
        } else {
            // Pipe stderr so we can surface mpv's own error messages in our log
            // when it dies early (e.g. URL unreachable, codec missing). Without
            // this, "mpv: socket never appeared" is the only signal the user gets.
            cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn mpv: {e} — is mpv installed?"))?;

        if !terminal_mode {
            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::warn!(target: "mpv", "{line}");
                    }
                });
            }
        }

        *self.inner.proc.lock().await = Some(child);
        let epoch = self.inner.ipc_epoch.fetch_add(1, Ordering::SeqCst) + 1;

        info!("mpv: spawned for {:?}", super::bridge::short(url, 80));

        // Spawn socket connector + event reader
        let player = self.clone();
        let url_owned  = url.to_string();
        let title_owned = title.to_string();
        tokio::spawn(async move {
            player.run_ipc_loop(epoch, url_owned, title_owned).await;
        });

        Ok(())
    }

    /// Spawn mpv in idle mode (no file loaded yet) so the window appears
    /// immediately while a slow source (e.g. librqbit metadata fetch)
    /// resolves a real URL in the background. Caller follows up with
    /// [`loadfile_replace`](Self::loadfile_replace) once the URL is ready.
    ///
    /// Same flags as [`play`](Self::play) except:
    /// - no URL positional arg
    /// - `--idle=yes` so mpv stays open without a file
    /// - `--keep-open=yes` so it doesn't bail when an empty playlist hits EOF
    ///
    /// Used by [`PlayerBridge::play_via_torrent`] to give the user instant
    /// visual feedback ("loading" placeholder window) instead of waiting
    /// 1–60 seconds in the dark for librqbit to fetch metadata.
    pub async fn play_idle(
        &self,
        title: &str,
        subtitle_path: Option<&str>,
        stui_data_dir: &str,
        extra_flags: &[String],
        terminal_vo: &str,
    ) -> Result<(), String> {
        self.stop().await;

        let _ = tokio::fs::remove_file(&self.inner.sock_path).await;

        let terminal_mode = !terminal_vo.is_empty();

        let mut args: Vec<String> = vec![
            format!("--input-ipc-server={}", self.inner.sock_path.display()),
            "--msg-level=statusline=no".into(),
            format!("--log-file={}", mpv_log_path().display()),
            "--keep-open=yes".into(),
            "--idle=yes".into(),
            "--sub-auto=fuzzy".into(),
            format!("--sub-file-paths={}/subtitles", stui_data_dir),
            "--script-opts=ytdl_hook-ytdl_path=yt-dlp".into(),
            // stui's overlay owns the centred status text now, so kill
            // mpv's default OSC (which draws the "Drop files or URLs
            // here to play" idle prompt and the play-button logo).
            "--osc=no".into(),
            // Users with modernz / modernx installed at
            // ~/.config/mpv/scripts/ get an *additional* idle prompt
            // drawn by that script — `--osc=no` only suppresses the
            // bundled osc.lua. Tell modernz to skip its idle screen so
            // our overlay isn't fighting another "Drop files…" layer.
            // No-op for users without modernz installed.
            "--script-opts-append=modernz-idlescreen=no".into(),
        ];

        if terminal_mode {
            args.push(format!("--vo={}", terminal_vo));
        } else {
            args.push("--input-terminal=no".into());
        }

        if let Some(sub) = subtitle_path {
            args.push(format!("--sub-file={}", sub));
        }

        if !title.is_empty() {
            args.push(format!("--force-media-title={}", title));
            if !terminal_mode {
                args.push(format!("--title=stui — {}", title));
            }
        }

        if let Some(script) = overlay_script_path() {
            args.push(format!("--script={}", script.display()));
        }

        args.extend_from_slice(extra_flags);

        let mut cmd = Command::new("mpv");
        // Kill the child if its `Child` handle gets dropped (e.g. because
        // `run_ipc_loop` lost its IPC connection and `take()`d the proc
        // out of `MpvInner` before `stop()` could SIGKILL it). Without
        // this, every IPC-loop exit before `stop()` orphaned an mpv
        // window — the user observed two/three blank windows accreting
        // across consecutive stream picks.
        cmd.args(&args).stdin(Stdio::null()).kill_on_drop(true);

        if terminal_mode {
            // Leave stdout/stderr connected for terminal video output.
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn mpv: {e} — is mpv installed?"))?;

        if !terminal_mode {
            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::warn!(target: "mpv", "{line}");
                    }
                });
            }
        }

        *self.inner.proc.lock().await = Some(child);
        let epoch = self.inner.ipc_epoch.fetch_add(1, Ordering::SeqCst) + 1;

        info!("mpv: spawned idle for {title:?}");

        let player = self.clone();
        let title_owned = title.to_string();
        tokio::spawn(async move {
            // run_ipc_loop's url arg is informational only; idle mode
            // means the actual URL arrives later via loadfile_replace.
            player.run_ipc_loop(epoch, String::new(), title_owned).await;
        });

        Ok(())
    }

    /// Stop mpv gracefully (sends quit command, then kills if needed).
    pub async fn stop(&self) {
        // Skip the IPC quit when there's no mpv to quit — otherwise
        // every cold-start logs a misleading "cached writer is None"
        // warning from `send_command` trying to talk to a socket that
        // never existed in the first place.
        let has_proc = self.inner.proc.lock().await.is_some();
        if has_proc {
            let _ = self.send_command(&json!(["quit"])).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // Bump the IPC epoch so any in-flight `run_ipc_loop` from the
        // previous mpv won't clear the writer once it eventually exits.
        self.inner.ipc_epoch.fetch_add(1, Ordering::SeqCst);

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
        let Some(child) = guard.as_mut() else { return false; };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => {
                *guard = None;
                false
            }
            Err(_) => true,
        }
    }

    /// Send a raw mpv IPC command array, e.g. `["cycle","pause"]`.
    pub async fn send_command(&self, cmd: &Value) -> Result<(), String> {
        let req_id = self.inner.req_id.fetch_add(1, Ordering::Relaxed);
        let msg = serde_json::to_string(&json!({
            "command": cmd,
            "request_id": req_id,
        })).map_err(|e| e.to_string())?;
        let mut line = msg;
        line.push('\n');

        // Try the cached writer first. If it's not installed yet (very
        // common in the ~200 ms window between play_idle returning and
        // run_ipc_loop completing wait_for_socket + observe_property
        // round-trips), wait briefly for it to be installed. Sending
        // commands through the same socket that run_ipc_loop has
        // already proven mpv can service avoids the fresh-connect race
        // where a freshly-bound IPC socket accepts a command before
        // mpv's main loop is ready to dispatch it — symptoms include
        // `loadfile` arriving but never producing a file-loaded event.
        let cached_writer_deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let mut guard = self.inner.sock_tx.lock().await;
            if let Some(tx) = guard.as_mut() {
                match timeout(Duration::from_secs(5), tx.write_all(line.as_bytes())).await {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(e)) => {
                        tracing::warn!("mpv IPC write failed via cached writer: {e}; falling back to fresh connection");
                        *guard = None;
                        break;
                    }
                    Err(_) => {
                        tracing::warn!("mpv IPC write timed out via cached writer; falling back to fresh connection");
                        *guard = None;
                        break;
                    }
                }
            }
            drop(guard);
            if std::time::Instant::now() >= cached_writer_deadline {
                tracing::debug!(
                    "mpv IPC: cached writer never appeared within 2s; falling back to fresh connection"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Fallback: open a one-shot connection to the IPC socket. Robust
        // against `run_ipc_loop` having exited and nulled `sock_tx` while
        // mpv itself is still alive and listening on the socket.
        //
        // Poll briefly: when SessionPersistenceConfig::Json restores a
        // torrent, `start_stream` returns in ~200 ms and the bridge
        // immediately calls `loadfile_replace` — but mpv's child process
        // creates the IPC socket asynchronously and the file can lag
        // spawn() by 200–500 ms. Without retry the first loadfile after a
        // cold-start race-loses, the URL never reaches mpv, and the user
        // sees an idle-but-blank player.
        let deadline = std::time::Instant::now() + Duration::from_millis(2000);
        let mut sock = loop {
            match UnixStream::connect(&self.inner.sock_path).await {
                Ok(s) => break s,
                Err(e) if std::time::Instant::now() >= deadline => {
                    return Err(format!("mpv IPC fresh connect failed: {e}"));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };
        timeout(Duration::from_secs(5), sock.write_all(line.as_bytes()))
            .await
            .map_err(|_| "mpv IPC fresh write timed out after 5s".to_string())?
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── High-level typed commands ────────────────────────────────────────────

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn set_pause(&self, paused: bool) -> Result<(), String> {
        self.send_command(&json!(["set_property", "pause", paused])).await
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
        self.send_command(&json!(["set_property", "volume", level])).await
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
        self.send_command(&json!(["set_property", "sid", "no"])).await
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
        self.send_command(&json!(["set_property", "sub-delay", 0.0])).await
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
        self.send_command(&json!(["add", "audio-delay", delta])).await
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn reset_audio_delay(&self) -> Result<(), String> {
        self.send_command(&json!(["set_property", "audio-delay", 0.0])).await
    }

    // ── Stream switching ──────────────────────────────────────────────────

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn loadfile_replace(&self, url: &str) -> Result<(), String> {
        self.send_command(&json!(["loadfile", url, "replace"])).await
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

    async fn run_ipc_loop(&self, my_epoch: u64, url: String, title: String) {
        // Wait for mpv to create the socket (up to 5 seconds)
        let sock = match self.wait_for_socket().await {
            Some(s) => s,
            None => {
                error!("mpv: socket never appeared — mpv may have failed to start");
                let _ = self.inner.event_tx.send(MpvEvent::Ended(
                    PlayerEndedReason::Error("mpv socket timeout".into())
                ));
                return;
            }
        };

        let (rx_half, tx_half) = sock.into_split();
        *self.inner.sock_tx.lock().await = Some(tx_half);

        // Subscribe to all properties we care about.
        // IDs are stable — used to quickly identify events in the loop below.
        for (prop, id) in &[
            ("time-pos",              1u64),
            ("duration",              2),
            ("pause",                 3),
            ("cache-buffering-state", 4),
            ("media-title",           5),
            ("aid",                   6),   // active audio track
            ("sid",                   7),   // active subtitle track
            ("sub-delay",             8),
            ("audio-delay",           9),
            ("volume",               10),
            ("mute",                 11),
            ("track-list",           12),   // full track list (parsed separately)
        ] {
            let _ = self.send_command(&json!(["observe_property", id, prop])).await;
        }

        info!("mpv: IPC connected — reading events");

        // State tracked across events — mirrors PlaybackState
        let mut position      = 0f64;
        let mut duration      = 0f64;
        let mut paused        = false;
        let mut cache_pct     = 100f64;
        let mut audio_track:   Option<i64> = None;
        let mut sub_track:     Option<i64> = None;
        let mut sub_delay      = 0f64;
        let mut audio_delay    = 0f64;
        let mut volume         = 100f64;
        let mut muted          = false;
        let mut media_title   = if title.is_empty() { url.clone() } else { title.clone() };
        let mut started_sent   = false;

        let mut lines = BufReader::new(rx_half).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let val: Value = match serde_json::from_str(&line) {
                Ok(v)  => v,
                Err(_) => continue,
            };

            // Diagnostic log of every "interesting" event we see from
            // mpv. Excludes property-change for noisy properties
            // (time-pos / cache state fire ~60 Hz). Helps confirm that
            // a `loadfile` actually reached mpv and produced a
            // file-loaded event versus silently being dropped.
            if let Some(event_name) = val.get("event").and_then(|e| e.as_str()) {
                let interesting = !matches!(event_name, "property-change");
                if interesting {
                    let reason = val.get("reason").and_then(|r| r.as_str());
                    tracing::info!(
                        target: "mpv-ipc",
                        event = event_name,
                        reason,
                        "mpv event"
                    );
                }
            }

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
                                        title:    media_title.clone(),
                                        path:     url.clone(),
                                        duration: d,
                                    }
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
                                        title:    media_title.clone(),
                                        path:     url.clone(),
                                        duration,
                                    }
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
                            let tracks: Vec<TrackInfo> = arr.iter().filter_map(|t| {
                                let id         = t["id"].as_i64()?;
                                let track_type = t["type"].as_str()?.to_string();
                                let lang       = t["lang"].as_str().unwrap_or("").to_string();
                                let title      = t["title"].as_str().unwrap_or("").to_string();
                                let selected   = t["selected"].as_bool().unwrap_or(false);
                                let external   = t["external"].as_bool().unwrap_or(false);
                                Some(TrackInfo { id, track_type, lang, title, selected, external })
                            }).collect();
                            // Emit a special tracks event
                            let _ = self.inner.event_tx.send(MpvEvent::TracksUpdated(tracks));
                        }
                    }
                    _ => {}
                }

                // Push a full progress snapshot whenever position or pause state changes
                if name == "time-pos" || name == "pause" {
                    let _ = self.inner.event_tx.send(MpvEvent::Progress(PlayerProgressEvent {
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
            // mpv emits this when a file finishes (eof / error / stop /
            // quit / redirect). With `--idle=yes --keep-open=yes` mpv
            // does NOT exit on eof or error — it stays in idle for the
            // next loadfile. We must NOT `break` here: doing so exits
            // run_ipc_loop, which then `take()`s the still-running mpv
            // Child out of MpvInner; with `kill_on_drop(true)` the drop
            // immediately SIGKILLs the live mpv process, leaving the
            // user with a closed window after a single failed URL load.
            // Keep the loop alive — only an actual mpv exit (which
            // closes the IPC socket and makes `lines.next_line` return
            // None) should tear us down.
            if val.get("event").and_then(|e| e.as_str()) == Some("end-file") {
                let reason = val.get("reason").and_then(|r| r.as_str()).unwrap_or("eof");
                let ended = match reason {
                    "eof"  => PlayerEndedReason::Eof,
                    "quit" => PlayerEndedReason::Quit,
                    other  => PlayerEndedReason::Error(other.to_string()),
                };
                let _ = self.inner.event_tx.send(MpvEvent::Ended(ended));
                started_sent = false;
                continue;
            }

            // ── file-loaded — send started if duration wasn't seen yet ────
            if val.get("event").and_then(|e| e.as_str()) == Some("file-loaded")
                && !started_sent {
                    started_sent = true;
                    let _ = self.inner.event_tx.send(MpvEvent::Started(
                        PlayerStartedEvent {
                            title:    media_title.clone(),
                            path:     url.clone(),
                            duration,
                        }
                    ));
                }
        }

        // Clean up process entry — but only if we're still the *current*
        // owner. If a newer `play_idle`/`play` has bumped the epoch since
        // we started, a fresh `run_ipc_loop` is already managing a new
        // mpv; clearing `sock_tx` here would null the writer it just
        // installed and force every subsequent IPC command through the
        // fresh-connect fallback in `send_command`.
        if self.inner.ipc_epoch.load(Ordering::SeqCst) == my_epoch {
            *self.inner.sock_tx.lock().await = None;
            let _ = self.inner.proc.lock().await.take();
        }
        debug!("mpv: IPC loop exited");
    }

    async fn wait_for_socket(&self) -> Option<UnixStream> {
        let path = &self.inner.sock_path;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(s) = UnixStream::connect(path).await {
                return Some(s);
            }
            // Bail early if mpv has already exited — connect() would loop forever
            // returning ECONNREFUSED against a stale socket file otherwise.
            let mut guard = self.inner.proc.lock().await;
            if let Some(child) = guard.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    error!("mpv: exited before IPC socket was ready (status={status})");
                    *guard = None;
                    return None;
                }
            }
        }
        None
    }
}
