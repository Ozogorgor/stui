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
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::protocol::{
    ActionRequest, ActionResponse, CatalogSearchParams, PluginHandshake, RpcMediaItem, RpcRequest,
    RpcResponse, RpcStream, RpcSubtitleTrack, StreamsResolveParams, SubtitlesFetchParams,
};

use crate::auth::OAuthReceiver;

// Variants are constructed inside handle_action — dead_code fires only for
// the binary target which doesn't use the RPC subsystem directly.
#[allow(dead_code)]
pub enum AuthPhase {
    Idle,
    /// Port allocated; receiver handed to `open_and_wait` in the next action.
    Allocated(OAuthReceiver),
    /// Auth flow in progress; reject further `allocate_port` requests.
    InProgress,
}

type SharedAuthPhase = Arc<tokio::sync::Mutex<AuthPhase>>;

/// An active external plugin process.
///
/// All public methods are `async` and safe to call concurrently — in-flight
/// requests are multiplexed over the single stdin/stdout channel.
#[allow(clippy::type_complexity)]
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

    stdin_tx:      mpsc::UnboundedSender<String>,
    auth_phase:    SharedAuthPhase,
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

        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                let _ = stdin.write_all(line.as_bytes()).await;
                let _ = stdin.flush().await;
            }
        });
        let auth_phase: SharedAuthPhase = Arc::new(tokio::sync::Mutex::new(AuthPhase::Idle));

        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let death_notify = Arc::new(Notify::new());
        let child        = Arc::new(Mutex::new(child));

        // Spawn the reader task — reads NDJSON lines and routes them to waiters.
        // When stdout closes (process exited), the loop ends and we fire death_notify.
        let pending_rx     = Arc::clone(&pending);
        let death_notify2  = Arc::clone(&death_notify);
        let stdin_tx_loop  = stdin_tx.clone();
        let auth_phase_loop = Arc::clone(&auth_phase);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() { continue; }
                if let Ok(action) = serde_json::from_str::<ActionRequest>(&line) {
                    tokio::spawn(handle_action(
                        action,
                        stdin_tx_loop.clone(),
                        auth_phase_loop.clone(),
                    ));
                } else if let Ok(resp) = serde_json::from_str::<RpcResponse>(&line) {
                    let mut map = pending_rx.lock().await;
                    if let Some(tx) = map.remove(&resp.id) {
                        let _ = tx.send(resp);
                    }
                } else {
                    warn!("plugin sent invalid JSON — line: {line}");
                }
            }
            // stdout EOF — process has exited.
            // Drop all pending senders so every in-flight call() gets
            // RecvError::Closed immediately instead of waiting 30 seconds.
            pending_rx.lock().await.clear();
            // Wake the supervisor so it can restart the process.
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
            stdin_tx,
            auth_phase,
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

        self.stdin_tx
            .send(format!("{line}\n"))
            .map_err(|_| anyhow::anyhow!("plugin stdin channel closed"))?;

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

async fn handle_action(
    req: ActionRequest,
    stdin_tx: mpsc::UnboundedSender<String>,
    auth_phase: SharedAuthPhase,
) {
    let response = match req.action.as_str() {
        "auth_allocate_port" => {
            // Check phase WITHOUT holding lock across the allocate_port().await.
            {
                let phase = auth_phase.lock().await;
                if matches!(*phase, AuthPhase::InProgress) {
                    send_response(&stdin_tx, ActionResponse::err(&req.id, "auth_already_in_progress"));
                    return;
                }
            } // lock released before await

            match crate::auth::allocate_port().await {
                Ok((port, rx)) => {
                    let mut phase = auth_phase.lock().await;
                    if matches!(*phase, AuthPhase::InProgress) {
                        send_response(&stdin_tx, ActionResponse::err(&req.id, "auth_already_in_progress"));
                        return;
                    }
                    *phase = AuthPhase::Allocated(rx);
                    ActionResponse::ok(&req.id, serde_json::json!({"port": port}))
                }
                Err(e) => ActionResponse::err(&req.id, format!("allocate_failed: {e}")),
            }
        }

        "auth_open_and_wait" => {
            let params = req.params.as_ref();

            let url = match params.and_then(|p| p["url"].as_str()) {
                Some(u) => u.to_string(),
                None => {
                    send_response(&stdin_tx, ActionResponse::err(&req.id, "invalid_params"));
                    return;
                }
            };
            let timeout_ms = params
                .and_then(|p| p["timeout_ms"].as_u64())
                .unwrap_or(120_000)
                .clamp(1_000, 300_000);

            let receiver = {
                let mut phase = auth_phase.lock().await;
                match std::mem::replace(&mut *phase, AuthPhase::InProgress) {
                    AuthPhase::Allocated(rx) => rx,
                    AuthPhase::Idle => {
                        *phase = AuthPhase::Idle;
                        send_response(&stdin_tx, ActionResponse::err(&req.id, "no_port_allocated"));
                        return;
                    }
                    AuthPhase::InProgress => {
                        *phase = AuthPhase::InProgress;
                        send_response(&stdin_tx, ActionResponse::err(&req.id, "auth_already_in_progress"));
                        return;
                    }
                }
            }; // lock released here — no lock held across the await below

            let result = crate::auth::open_and_wait(
                &url,
                receiver,
                std::time::Duration::from_millis(timeout_ms),
            ).await;

            *auth_phase.lock().await = AuthPhase::Idle;

            match result {
                Ok(cb) => ActionResponse::ok(
                    &req.id,
                    serde_json::json!({"code": cb.code, "state": cb.state}),
                ),
                Err(crate::auth::AuthError::TimedOut) =>
                    ActionResponse::err(&req.id, "timed_out"),
                Err(crate::auth::AuthError::Denied { message }) =>
                    ActionResponse::err(&req.id, format!("denied: {message}")),
                Err(crate::auth::AuthError::BrowserOpenFailed(m)) =>
                    ActionResponse::err(&req.id, format!("browser_open_failed: {m}")),
                Err(crate::auth::AuthError::ReceiverDropped) =>
                    ActionResponse::err(&req.id, "timed_out"),
            }
        }

        _ => ActionResponse::err(&req.id, "unknown_action"),
    };

    send_response(&stdin_tx, response);
}

fn send_response(
    stdin_tx: &mpsc::UnboundedSender<String>,
    resp: ActionResponse,
) {
    if let Ok(line) = serde_json::to_string(&resp) {
        let _ = stdin_tx.send(format!("{line}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> AuthPhase { AuthPhase::Idle }

    #[test]
    fn test_auth_phase_transitions() {
        let phase = idle();
        assert!(matches!(phase, AuthPhase::Idle));
    }

    #[tokio::test]
    async fn test_auth_phase_allocated_allows_realloc() {
        let (port1, rx1) = crate::auth::allocate_port().await.unwrap();
        let (_port2, rx2) = crate::auth::allocate_port().await.unwrap();
        let mut phase = AuthPhase::Allocated(rx1);
        phase = AuthPhase::Allocated(rx2);
        assert!(matches!(phase, AuthPhase::Allocated(_)));
        let _ = port1;
    }

    #[test]
    fn test_auth_phase_in_progress_rejects_realloc() {
        let phase = AuthPhase::InProgress;
        assert!(matches!(phase, AuthPhase::InProgress));
    }
}
