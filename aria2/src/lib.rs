//! lib.rs — stui-aria2: async aria2c JSON-RPC client.
//!
//! ## aria2c daemon setup
//!
//! Start aria2c with RPC enabled:
//!
//! ```bash
//! aria2c \
//!   --enable-rpc \
//!   --rpc-listen-port=6800 \
//!   --rpc-secret=my-secret-token \
//!   --rpc-allow-origin-all \
//!   --continue \
//!   --max-concurrent-downloads=5 \
//!   --bt-enable-lpd \
//!   --enable-dht \
//!   --enable-peer-exchange \
//!   --seed-time=0 \
//!   --dir="$HOME/Downloads/stui" \
//!   --daemon
//! ```
//!
//! Then configure stui:
//! ```toml
//! [aria2]
//! url    = "http://127.0.0.1:6800/jsonrpc"
//! secret = "my-secret-token"
//! ```
//!
//! Or set env vars:
//!   ARIA2_URL    = http://127.0.0.1:6800/jsonrpc
//!   ARIA2_SECRET = my-secret-token
//!
//! ## Usage from stui runtime
//!
//! ```rust
//! let client = Aria2Client::connect("http://127.0.0.1:6800/jsonrpc", "token").await?;
//!
//! // Add a magnet URI
//! let gid = client.add_uri(
//!     &["magnet:?xt=urn:btih:..."],
//!     AddOptions::streaming("/home/user/stui/downloads"),
//! ).await?;
//!
//! // Poll progress
//! let status = client.tell_status(&gid).await?;
//! println!("{:.0}%  {} eta {}", status.progress().unwrap_or(0.0) * 100.0,
//!          format_speed(status.speed_bps()), format_eta(status.eta_secs().unwrap_or(0)));
//!
//! // Listen for completion events
//! let mut rx = client.notifications();
//! while let Ok(n) = rx.recv().await {
//!     if n.event == NotificationEvent::BtDownloadComplete {
//!         println!("done: {}", n.gid);
//!     }
//! }
//! ```

pub mod types;

pub use types::*;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client as HttpClient;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum Aria2Error {
    #[error("aria2 RPC error {code}: {message}")]
    Rpc { code: i32, message: String },

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("aria2 is not running or not reachable at {url}")]
    NotReachable { url: String },

    #[error("aria2 returned an unexpected response shape")]
    UnexpectedResponse,
}

pub type Result<T> = std::result::Result<T, Aria2Error>;

// ── Aria2Client ───────────────────────────────────────────────────────────────

/// Async client for aria2's JSON-RPC interface.
///
/// Uses HTTP for all command-and-response calls, and a WebSocket connection
/// for real-time download notifications.
///
/// Clone is cheap — the client is backed by an Arc.
#[derive(Clone)]
pub struct Aria2Client {
    inner: Arc<Inner>,
}

struct Inner {
    http_url: String,
    ws_url:   String,
    secret:   Option<String>,
    http:     HttpClient,
    seq:      AtomicU64,
    notif_tx: broadcast::Sender<Notification>,
}

impl Aria2Client {
    /// Connect to a running aria2c RPC daemon.
    ///
    /// `url` is the JSON-RPC endpoint, e.g. `http://127.0.0.1:6800/jsonrpc`.
    /// `secret` is the `--rpc-secret` value (None if aria2 was started without one).
    ///
    /// Starts a background WebSocket listener for notifications immediately.
    pub async fn connect(url: impl Into<String>, secret: impl Into<Option<String>>) -> Result<Self> {
        let url    = url.into();
        let secret = secret.into();

        // Derive WebSocket URL from HTTP URL
        let ws_url = url
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1);

        let http = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(Aria2Error::Http)?;

        let (notif_tx, _) = broadcast::channel::<Notification>(64);

        let inner = Arc::new(Inner {
            http_url: url.clone(),
            ws_url: ws_url.clone(),
            secret,
            http,
            seq: AtomicU64::new(1),
            notif_tx,
        });

        // Verify connectivity
        let client = Aria2Client { inner: Arc::clone(&inner) };
        client.get_version().await.map_err(|_| Aria2Error::NotReachable { url })?;

        // Spawn WebSocket notification listener
        let inner_ws = Arc::clone(&inner);
        tokio::spawn(async move {
            run_ws_listener(inner_ws).await;
        });

        info!("aria2: connected to {}", &inner.http_url);
        Ok(client)
    }

    /// Subscribe to download notifications.
    /// The returned receiver will yield events as they arrive over WebSocket.
    pub fn notifications(&self) -> broadcast::Receiver<Notification> {
        self.inner.notif_tx.subscribe()
    }

    // ── Download management ──────────────────────────────────────────────

    /// Add one or more URIs as a single download.
    ///
    /// Accepts: HTTP/HTTPS/FTP URLs, BitTorrent magnet URIs.
    /// For magnets, pass a single element in `uris`.
    ///
    /// Returns the GID of the new download.
    pub async fn add_uri(&self, uris: &[&str], opts: AddOptions) -> Result<Gid> {
        let params = json!([
            self.token_param(),
            uris,
            serde_json::to_value(&opts)?,
        ]);
        let result = self.call("aria2.addUri", params).await?;
        result.as_str()
            .map(|s| s.to_string())
            .ok_or(Aria2Error::UnexpectedResponse)
    }

    /// Add a .torrent file as a download.
    ///
    /// `torrent_bytes` is the raw bytes of the .torrent file.
    /// Returns the GID of the new download.
    pub async fn add_torrent(&self, torrent_bytes: &[u8], opts: AddOptions) -> Result<Gid> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(torrent_bytes);
        let params = json!([
            self.token_param(),
            b64,
            [],   // uris (optional additional sources for the torrent)
            serde_json::to_value(&opts)?,
        ]);
        let result = self.call("aria2.addTorrent", params).await?;
        result.as_str()
            .map(|s| s.to_string())
            .ok_or(Aria2Error::UnexpectedResponse)
    }

    /// Query the status of a download by GID.
    pub async fn tell_status(&self, gid: &str) -> Result<DownloadStatus> {
        let params = json!([self.token_param(), gid]);
        let result = self.call("aria2.tellStatus", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// List all active downloads.
    pub async fn tell_active(&self) -> Result<Vec<DownloadStatus>> {
        let params = json!([self.token_param()]);
        let result = self.call("aria2.tellActive", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// List downloads in the queue (waiting).
    pub async fn tell_waiting(&self, offset: i32, limit: u32) -> Result<Vec<DownloadStatus>> {
        let params = json!([self.token_param(), offset, limit]);
        let result = self.call("aria2.tellWaiting", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// List stopped (completed or errored) downloads.
    pub async fn tell_stopped(&self, offset: i32, limit: u32) -> Result<Vec<DownloadStatus>> {
        let params = json!([self.token_param(), offset, limit]);
        let result = self.call("aria2.tellStopped", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// Pause a download.
    pub async fn pause(&self, gid: &str) -> Result<Gid> {
        let params = json!([self.token_param(), gid]);
        let result = self.call("aria2.pause", params).await?;
        result.as_str().map(|s| s.to_string()).ok_or(Aria2Error::UnexpectedResponse)
    }

    /// Resume a paused download.
    pub async fn unpause(&self, gid: &str) -> Result<Gid> {
        let params = json!([self.token_param(), gid]);
        let result = self.call("aria2.unpause", params).await?;
        result.as_str().map(|s| s.to_string()).ok_or(Aria2Error::UnexpectedResponse)
    }

    /// Remove a download (stops it if active).
    pub async fn remove(&self, gid: &str) -> Result<Gid> {
        let params = json!([self.token_param(), gid]);
        let result = self.call("aria2.remove", params).await?;
        result.as_str().map(|s| s.to_string()).ok_or(Aria2Error::UnexpectedResponse)
    }

    /// Change download options on the fly (e.g. speed limit).
    pub async fn change_option(&self, gid: &str, opts: AddOptions) -> Result<()> {
        let params = json!([self.token_param(), gid, serde_json::to_value(&opts)?]);
        self.call("aria2.changeOption", params).await?;
        Ok(())
    }

    /// Get global download/upload stats.
    pub async fn get_global_stat(&self) -> Result<GlobalStat> {
        let params = json!([self.token_param()]);
        let result = self.call("aria2.getGlobalStat", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// Get aria2 version and enabled features.
    pub async fn get_version(&self) -> Result<VersionInfo> {
        let params = json!([self.token_param()]);
        let result = self.call("aria2.getVersion", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// Remove completed/error/removed downloads from memory.
    pub async fn purge_download_result(&self) -> Result<()> {
        let params = json!([self.token_param()]);
        self.call("aria2.purgeDownloadResult", params).await?;
        Ok(())
    }

    // ── Convenience methods ──────────────────────────────────────────────

    /// Add a magnet URI and return the GID.
    /// Downloads to `dir`, stops seeding on completion.
    pub async fn add_magnet(&self, magnet: &str, dir: impl Into<String>) -> Result<Gid> {
        self.add_uri(&[magnet], AddOptions::streaming(dir)).await
    }

    /// Fetch a .torrent file from a URL, then add it to aria2.
    /// Returns the GID.
    pub async fn add_torrent_url(&self, torrent_url: &str, dir: impl Into<String>) -> Result<Gid> {
        let bytes = self.inner.http.get(torrent_url)
            .send().await.map_err(Aria2Error::Http)?
            .bytes().await.map_err(Aria2Error::Http)?;
        self.add_torrent(&bytes, AddOptions::streaming(dir)).await
    }

    /// Poll a GID until it completes or errors, calling `on_progress` periodically.
    /// Returns the final DownloadStatus.
    pub async fn wait_for_gid<F>(&self, gid: &str, on_progress: F) -> Result<DownloadStatus>
    where
        F: Fn(&DownloadStatus) + Send + 'static,
    {
        loop {
            let status = self.tell_status(gid).await?;
            on_progress(&status);
            match status.status.as_str() {
                "complete" | "error" | "removed" => return Ok(status),
                _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
            }
        }
    }

    // ── Internal ─────────────────────────────────────────────────────────

    fn token_param(&self) -> serde_json::Value {
        match &self.inner.secret {
            Some(s) => json!(format!("token:{}", s)),
            None    => json!(null),   // aria2 ignores null token
        }
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let id = self.inner.seq.fetch_add(1, Ordering::Relaxed).to_string();

        // Filter out the null token if no secret is set — some aria2 versions
        // don't accept null in the params array.
        let params = if let Some(arr) = params.as_array() {
            let filtered: Vec<serde_json::Value> = arr.iter()
                .filter(|v| !v.is_null())
                .cloned()
                .collect();
            serde_json::Value::Array(filtered)
        } else {
            params
        };

        let req = RpcRequest {
            jsonrpc: "2.0",
            id:      id.clone(),
            method,
            params,
        };

        debug!("aria2 rpc → {method}");

        let resp = self.inner.http
            .post(&self.inner.http_url)
            .json(&req)
            .send()
            .await
            .map_err(Aria2Error::Http)?
            .json::<RpcResponse>()
            .await
            .map_err(Aria2Error::Http)?;

        if let Some(err) = resp.error {
            return Err(Aria2Error::Rpc { code: err.code, message: err.message });
        }

        resp.result.ok_or(Aria2Error::UnexpectedResponse)
    }
}

// ── WebSocket notification listener ───────────────────────────────────────────

async fn run_ws_listener(inner: Arc<Inner>) {
    loop {
        match connect_async(&inner.ws_url).await {
            Ok((mut ws, _)) => {
                info!("aria2: WebSocket connected for notifications");

                while let Some(msg) = ws.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            handle_notification(&inner, &text);
                        }
                        Ok(Message::Close(_)) => {
                            warn!("aria2: WebSocket closed");
                            break;
                        }
                        Ok(Message::Ping(data)) => {
                            let _ = ws.send(Message::Pong(data)).await;
                        }
                        Err(e) => {
                            error!("aria2: WebSocket error: {e}");
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                debug!("aria2: WebSocket connect failed: {e}");
            }
        }

        // Reconnect after 5 seconds
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

fn handle_notification(inner: &Inner, text: &str) {
    // Notifications look like:
    // {"jsonrpc":"2.0","method":"aria2.onDownloadComplete","params":[{"gid":"abc123"}]}
    let resp: RpcResponse = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => { debug!("aria2: unparseable notification: {e}"); return; }
    };

    let Some(method) = resp.method else { return };
    let gid = resp.params
        .as_ref()
        .and_then(|p| p.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.get("gid"))
        .and_then(|g| g.as_str())
        .unwrap_or("")
        .to_string();

    if gid.is_empty() { return; }

    let event = NotificationEvent::from_method(&method);
    debug!("aria2: notification {:?} gid={}", event, gid);

    let _ = inner.notif_tx.send(Notification { event, gid });
}

// ── Integration with stui runtime ────────────────────────────────────────────

/// Configuration loaded from environment or stui config.
#[derive(Debug, Clone)]
pub struct Aria2Config {
    pub url:    String,
    pub secret: Option<String>,
    /// Default download directory
    pub dir:    String,
}

impl Aria2Config {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self {
            url: std::env::var("ARIA2_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:6800/jsonrpc".into()),
            secret: std::env::var("ARIA2_SECRET").ok().filter(|s| !s.is_empty()),
            dir: std::env::var("ARIA2_DIR")
                .unwrap_or_else(|_| format!("{home}/Downloads/stui")),
        }
    }

    pub async fn connect(&self) -> Result<Aria2Client> {
        Aria2Client::connect(self.url.clone(), self.secret.clone()).await
    }
}
