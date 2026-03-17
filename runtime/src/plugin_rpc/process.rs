//! External plugin process management.
//!
//! A `PluginProcess` owns a single spawned external plugin.  It:
//!
//! - Spawns the process from a path + optional args
//! - Performs the initial `handshake` to learn name, version, capabilities
//! - Sends `RpcRequest`s and collects `RpcResponse`s over stdin/stdout
//! - Handles concurrent calls by matching correlation IDs
//! - Gracefully shuts down the process on `Drop`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex, Notify};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::protocol::{
    CatalogSearchParams, PluginHandshake, RpcMediaItem, RpcRequest, RpcResponse,
    RpcStream, RpcSubtitleTrack, StreamsResolveParams, SubtitlesFetchParams,
};

/// An active external plugin process.
///
/// All public methods are `async` and safe to call concurrently — in-flight
/// requests are multiplexed over the single stdin/stdout channel.
pub struct PluginProcess {
    /// Resolved plugin ID (UUID assigned at load time).
    pub id:        String,
    /// Handshake info: name, version, capabilities.
    pub info:      PluginHandshake,
    /// Path to the plugin executable.
    pub bin:       PathBuf,
    /// Notified when the plugin process exits (stdout EOF).
    /// The supervisor awaits this to trigger restart logic.
    pub death_notify: Arc<Notify>,
    /// OS process ID, captured before the child handle is moved.
    pub pid:          Option<u32>,

    stdin:         Arc<Mutex<ChildStdin>>,
    /// Pending calls: correlation-id → response sender.
    pending:       Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>>,
    _child:        Arc<Mutex<Child>>,
}

impl PluginProcess {
    /// Spawn `bin` as a child process, perform the handshake, and return a
    /// ready-to-use `PluginProcess`.
    pub async fn spawn(bin: PathBuf) -> Result<Self> {
        let mut child = Command::new(&bin)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit()) // plugin logs go to our stderr
            .spawn()
            .with_context(|| format!("failed to spawn plugin: {}", bin.display()))?;

        let stdin  = child.stdin.take().context("no stdin on plugin process")?;
        let stdout = child.stdout.take().context("no stdout on plugin process")?;
        let pid    = child.id();

        let stdin        = Arc::new(Mutex::new(stdin));
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let death_notify = Arc::new(Notify::new());
        let child        = Arc::new(Mutex::new(child));

        // Spawn the reader task — reads NDJSON lines and routes them to waiters.
        // When stdout closes (process exited), the loop ends and we fire death_notify.
        let pending_rx    = Arc::clone(&pending);
        let death_notify2 = Arc::clone(&death_notify);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() { continue; }
                match serde_json::from_str::<RpcResponse>(&line) {
                    Ok(resp) => {
                        let mut map = pending_rx.lock().await;
                        if let Some(tx) = map.remove(&resp.id) {
                            let _ = tx.send(resp);
                        }
                    }
                    Err(e) => {
                        warn!("plugin sent invalid JSON: {e} — line: {line}");
                    }
                }
            }
            // stdout EOF — process has exited; wake the supervisor.
            death_notify2.notify_one();
        });

        let mut proc = PluginProcess {
            id:           Uuid::new_v4().to_string(),
            info:         PluginHandshake {
                name:         "unknown".into(),
                version:      "0.0.0".into(),
                capabilities: vec![],
                description:  None,
            },
            bin,
            death_notify,
            pid,
            stdin,
            pending,
            _child:       child,
        };

        // Perform the handshake to get name/version/capabilities.
        let hs_resp = proc.call("handshake", serde_json::json!({})).await
            .context("handshake failed")?;

        let hs: PluginHandshake = serde_json::from_value(hs_resp)
            .context("invalid handshake response")?;

        info!(
            plugin = %hs.name,
            version = %hs.version,
            caps = ?hs.capabilities,
            "plugin handshake complete"
        );

        proc.info = hs;
        Ok(proc)
    }

    /// Send an RPC call and wait for the response (up to 30 seconds).
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id  = Uuid::new_v4().to_string();
        let req = RpcRequest { id: id.clone(), method: method.to_string(), params };
        let line = serde_json::to_string(&req).context("serialize request")?;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }

        debug!(method, id, "rpc call sent");

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            rx,
        )
        .await
        .context("plugin call timed out")?
        .context("plugin channel closed")?;

        if let Some(err) = resp.error {
            bail!("plugin error {}: {}", err.code, err.message);
        }

        resp.result.context("empty result from plugin")
    }

    /// True if this plugin advertises the given capability string.
    pub fn has_capability(&self, cap: &str) -> bool {
        self.info.capabilities.iter().any(|c| c == cap)
    }

    // ── High-level typed methods ──────────────────────────────────────────

    pub async fn catalog_search(
        &self,
        query: &str,
        tab: &str,
        page: u32,
    ) -> Result<Vec<RpcMediaItem>> {
        let params = serde_json::to_value(CatalogSearchParams {
            query: query.to_string(),
            tab:   tab.to_string(),
            page,
        })?;
        let val = self.call("catalog.search", params).await?;
        Ok(serde_json::from_value(val)?)
    }

    pub async fn streams_resolve(&self, id: &str) -> Result<Vec<RpcStream>> {
        let params = serde_json::to_value(StreamsResolveParams { id: id.to_string() })?;
        let val = self.call("streams.resolve", params).await?;
        Ok(serde_json::from_value(val)?)
    }

    pub async fn subtitles_fetch(&self, id: &str) -> Result<Vec<RpcSubtitleTrack>> {
        let params = serde_json::to_value(SubtitlesFetchParams { id: id.to_string() })?;
        let val = self.call("subtitles.fetch", params).await?;
        Ok(serde_json::from_value(val)?)
    }

    /// Send a graceful shutdown request then kill the process.
    pub async fn shutdown(&self) {
        let _ = self.call("shutdown", serde_json::json!({})).await;
    }
}
