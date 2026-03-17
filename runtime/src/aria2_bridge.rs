// aria2_bridge.rs — integrates the stui-aria2 crate into the runtime IPC loop.
//
// The bridge:
//   1. Connects to a running aria2c daemon on startup (non-fatal if absent)
//   2. Exposes `start_download(uri, opts)` called by the engine on resolve()
//   3. Monitors active downloads, pushing progress updates to Go every 500 ms
//   4. Listens for completion/error notifications over the aria2 WebSocket
//   5. Pushes download_progress / download_complete / download_error to Go IPC
//
// IPC wire messages emitted to Go (NDJSON on stdout):
//
//   { "type": "download_started",  "gid": "…", "uri": "…", "dir": "…" }
//   { "type": "download_progress", "gid": "…", "progress": 0.42,
//     "speed": "1.2 MiB/s", "eta": "34s", "seeders": 12 }
//   { "type": "download_complete", "gid": "…", "files": ["path/to/file.mkv"] }
//   { "type": "download_error",    "gid": "…", "message": "…" }
//
// Configuration (env or ~/.stui/config.toml):
//   ARIA2_URL    = http://127.0.0.1:6800/jsonrpc   (default)
//   ARIA2_SECRET = <rpc-secret>
//   ARIA2_DIR    = ~/Downloads/stui                 (default download directory)

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{broadcast, Mutex};
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

use stui_aria2::{Aria2Client, Aria2Config, Aria2Error, AddOptions, NotificationEvent};

// ── Wire message shapes ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct DownloadStartedWire<'a> {
    r#type: &'static str,
    gid:    &'a str,
    uri:    &'a str,
    dir:    &'a str,
}

#[derive(Serialize)]
struct DownloadProgressWire<'a> {
    r#type:   &'static str,
    gid:      &'a str,
    progress: f64,
    speed:    String,
    eta:      String,
    seeders:  u64,
}

#[derive(Serialize)]
struct DownloadCompleteWire<'a> {
    r#type: &'static str,
    gid:    &'a str,
    files:  Vec<String>,
}

#[derive(Serialize)]
struct DownloadErrorWire<'a> {
    r#type:  &'static str,
    gid:     &'a str,
    message: String,
}

// ── Aria2Bridge ───────────────────────────────────────────────────────────────

/// Holds the live aria2 connection and manages the download lifecycle.
/// Cheaply cloneable — backed by Arc.
#[derive(Clone)]
pub struct Aria2Bridge {
    inner: Arc<BridgeInner>,
}

struct BridgeInner {
    client: Aria2Client,
    cfg:    Aria2Config,
    /// GID → original URI, tracked for the started message
    active: Mutex<HashMap<String, String>>,
}

impl Aria2Bridge {
    /// Expose the underlying Aria2Client for direct calls (used by player_bridge).
    pub fn client(&self) -> &stui_aria2::Aria2Client {
        &self.inner.client
    }

    /// Try to connect to aria2c. Returns None if not reachable (non-fatal).
    pub async fn try_connect() -> Option<Self> {
        let cfg = Aria2Config::from_env();
        match cfg.connect().await {
            Ok(client) => {
                info!("aria2: connected — url={}", cfg.url);
                Some(Aria2Bridge {
                    inner: Arc::new(BridgeInner {
                        client,
                        cfg,
                        active: Mutex::new(HashMap::new()),
                    }),
                })
            }
            Err(e) => {
                warn!("aria2: not available ({e}) — downloads disabled. \
                       Start aria2c with --enable-rpc to enable.");
                None
            }
        }
    }

    /// Add a download (magnet URI, torrent URL, or HTTP URL) and return its GID.
    /// Emits a download_started message to the writer immediately.
    pub async fn start_download(
        &self,
        uri: &str,
        out: &mut impl Write,
    ) -> Result<String, Aria2Error> {
        let dir = self.inner.cfg.dir.clone();
        let opts = AddOptions::streaming(&dir);

        let gid = if uri.starts_with("magnet:") {
            self.inner.client.add_magnet(uri, &dir).await?
        } else if uri.ends_with(".torrent") || uri.contains("/torrent/") {
            self.inner.client.add_torrent_url(uri, &dir).await?
        } else {
            self.inner.client.add_uri(&[uri], opts).await?
        };

        // Track the GID
        self.inner.active.lock().await.insert(gid.clone(), uri.to_string());

        // Emit started message
        let msg = serde_json::to_string(&DownloadStartedWire {
            r#type: "download_started",
            gid:    &gid,
            uri,
            dir:    &dir,
        }).unwrap_or_default();
        let _ = writeln!(out, "{}", msg);

        info!("aria2: started download gid={gid} uri={}", &uri[..uri.len().min(80)]);
        Ok(gid)
    }

    /// Spawn two background tasks: the progress poller and the notification listener.
    /// Both tasks write NDJSON to `ipc_tx` (a channel whose receiver drives stdout).
    pub fn spawn_monitors(&self, ipc_tx: tokio::sync::mpsc::Sender<String>) {
        // Progress poller — runs every 500 ms
        let bridge = self.clone();
        let tx1 = ipc_tx.clone();
        tokio::spawn(async move {
            bridge.run_progress_poller(tx1).await;
        });

        // Notification listener — event-driven via WebSocket
        let bridge = self.clone();
        let tx2 = ipc_tx.clone();
        tokio::spawn(async move {
            bridge.run_notification_listener(tx2).await;
        });
    }

    async fn run_progress_poller(&self, tx: tokio::sync::mpsc::Sender<String>) {
        let mut tick = interval(Duration::from_millis(500));
        loop {
            tick.tick().await;
            let active = match self.inner.client.tell_active().await {
                Ok(list) => list,
                Err(e) => {
                    debug!("aria2 poll error: {e}");
                    continue;
                }
            };

            for st in active {
                let progress = st.progress().unwrap_or(0.0);
                let speed    = stui_aria2::format_speed(st.speed_bps());
                let eta      = st.eta_secs().map(stui_aria2::format_eta).unwrap_or_else(|| "—".into());
                let seeders  = st.num_seeders.as_deref()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);

                let msg = serde_json::to_string(&DownloadProgressWire {
                    r#type: "download_progress",
                    gid:    &st.gid,
                    progress,
                    speed,
                    eta,
                    seeders,
                }).unwrap_or_default();
                let _ = tx.send(msg).await;
            }
        }
    }

    async fn run_notification_listener(&self, tx: tokio::sync::mpsc::Sender<String>) {
        let mut rx = self.inner.client.notifications();
        loop {
            match rx.recv().await {
                Ok(notif) => {
                    match notif.event {
                        NotificationEvent::DownloadComplete |
                        NotificationEvent::BtDownloadComplete => {
                            let files = self.collect_files(&notif.gid).await;
                            let msg = serde_json::to_string(&DownloadCompleteWire {
                                r#type: "download_complete",
                                gid:    &notif.gid,
                                files,
                            }).unwrap_or_default();
                            let _ = tx.send(msg).await;
                            self.inner.active.lock().await.remove(&notif.gid);
                        }
                        NotificationEvent::DownloadError => {
                            let err_msg = self.inner.client.tell_status(&notif.gid).await
                                .ok()
                                .and_then(|s| s.error_message)
                                .unwrap_or_else(|| "unknown error".into());
                            let msg = serde_json::to_string(&DownloadErrorWire {
                                r#type:  "download_error",
                                gid:     &notif.gid,
                                message: err_msg,
                            }).unwrap_or_default();
                            let _ = tx.send(msg).await;
                            self.inner.active.lock().await.remove(&notif.gid);
                        }
                        _ => {}
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("aria2 notification channel lagged {n}");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    async fn collect_files(&self, gid: &str) -> Vec<String> {
        match self.inner.client.tell_status(gid).await {
            Ok(st) => st.files.into_iter().map(|f| f.path).collect(),
            Err(_) => vec![],
        }
    }
}
