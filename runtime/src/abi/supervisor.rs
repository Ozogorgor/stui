//! WASM plugin supervisor — timeout enforcement, crash detection, and
//! automatic reload on trap/panic.
//!
//! # Design
//!
//! RPC plugins crash by exiting their OS process; their supervisor reacts via
//! `death_notify` (stdout EOF).  WASM plugins crash differently: a wasmtime
//! `Trap` surfaces as an `AbiError::Execution` return value.  This supervisor
//! catches those errors and responds the same way:
//!
//!   - Drop the bad instance immediately so callers see "reloading" rather
//!     than a broken instance.
//!   - Schedule a background reload with exponential backoff (1 s → 60 s).
//!   - Count crashes in a sliding window; permanently fail after
//!     `max_reloads` crashes within `crash_window_secs`.
//!
//! A per-call timeout (default 30 s) is enforced via `tokio::time::timeout`.
//! If a call times out the instance is treated as crashed and reloaded.
//!
//! # Memory limits
//!
//! Memory is capped at the wasmtime `Store` level via `ResourceLimiter` (see
//! `abi/host.rs`).  When a plugin exceeds its limit, wasmtime returns a `Trap`
//! which the supervisor catches here and turns into a reload cycle.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{error, info, warn};

use super::host::{WasmHost, WasmInstance};
use super::types::{
    AbiError, InitError, InitRequest,
    ArtworkRequest, ArtworkResponse,
    CreditsRequest, CreditsResponse,
    EnrichRequest, EnrichResponse,
    LookupRequest, LookupResponse,
    RelatedRequest, RelatedResponse,
    ResolveRequest, ResolveResponse,
    SearchRequest, SearchResponse,
};
use crate::sandbox::SandboxCtx;

// ── Configuration ─────────────────────────────────────────────────────────────

/// Tunable parameters for a single WASM plugin supervisor.
#[derive(Debug, Clone)]
pub struct WasmSupervisorConfig {
    /// Maximum reloads allowed within `crash_window_secs` before giving up.
    pub max_reloads: u32,
    /// Sliding window (seconds) for counting crashes.
    pub crash_window_secs: u64,
    /// Initial backoff delay (milliseconds) before the first reload.
    pub backoff_base_ms: u64,
    /// Maximum backoff delay (milliseconds).
    pub backoff_max_ms: u64,
    /// Kill a call (and treat the plugin as crashed) after this many seconds.
    pub call_timeout_secs: u64,
    /// Maximum RSS the WASM linear memory may occupy, in megabytes.
    /// Enforced by the wasmtime `ResourceLimiter` on the Store.
    pub max_memory_mb: u64,
}

impl Default for WasmSupervisorConfig {
    fn default() -> Self {
        Self {
            max_reloads:       5,
            crash_window_secs: 60,
            backoff_base_ms:   1_000,
            backoff_max_ms:    60_000,
            call_timeout_secs: 30,
            max_memory_mb:     512,
        }
    }
}

// ── Stats ─────────────────────────────────────────────────────────────────────

/// Live health snapshot for a supervised WASM plugin.
#[derive(Debug, Clone, Default)]
pub struct WasmSupervisorStats {
    /// Total trap/timeout crashes since load.
    pub crash_count: u32,
    /// Number of successful reloads.
    pub reload_count: u32,
    /// Whether a live instance is currently available.
    pub is_alive: bool,
    /// Whether the supervisor has given up on this plugin.
    pub permanently_failed: bool,
}

// ── Supervisor ────────────────────────────────────────────────────────────────

/// Wraps a [`WasmInstance`] with timeout enforcement, crash detection,
/// and automatic reload on trap or timeout.
pub struct WasmSupervisor {
    wasm_path:   PathBuf,
    plugin_name: String,
    ctx:         SandboxCtx,
    config:      WasmSupervisorConfig,
    /// The live instance, or `None` while reloading.
    instance:    Arc<Mutex<Option<WasmInstance>>>,
    /// Timestamps of recent crashes — used for the sliding-window check.
    crash_times: Arc<Mutex<Vec<Instant>>>,
    stats:       Arc<Mutex<WasmSupervisorStats>>,
    failed:      Arc<AtomicBool>,
}

impl std::fmt::Debug for WasmSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmSupervisor")
            .field("plugin", &self.plugin_name)
            .field("failed", &self.failed.load(Ordering::Relaxed))
            .finish()
    }
}

impl WasmSupervisor {
    /// Load the plugin from disk and return a ready supervisor.
    pub async fn load(
        wasm_path:   PathBuf,
        plugin_name: String,
        ctx:         SandboxCtx,
        config:      WasmSupervisorConfig,
    ) -> Result<Self, AbiError> {
        let instance = WasmHost::load(&wasm_path, &plugin_name, &ctx, config.max_memory_mb).await?;
        Ok(Self {
            wasm_path,
            plugin_name,
            ctx,
            config,
            instance:    Arc::new(Mutex::new(Some(instance))),
            crash_times: Arc::new(Mutex::new(Vec::new())),
            stats:       Arc::new(Mutex::new(WasmSupervisorStats { is_alive: true, ..Default::default() })),
            failed:      Arc::new(AtomicBool::new(false)),
        })
    }

    /// `true` if the crash-loop threshold has been reached.
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Snapshot of current health metrics.
    pub async fn stats(&self) -> WasmSupervisorStats {
        self.stats.lock().await.clone()
    }

    /// Call `stui_init` with timeout enforcement.
    ///
    /// Init is one-shot: it runs once after instantiation. Unlike verb calls
    /// we do NOT treat a `PluginInitError` as a crash — a plugin reporting
    /// `MissingConfig` or `Fatal` is behaving correctly at the ABI level,
    /// and the engine translates the outcome into a `PluginStatus`.
    ///
    /// Plumbing failures (timeout, serde, missing export) DO count as
    /// crashes so a chronically broken plugin gets torn down.
    pub async fn init(&self, req: &InitRequest) -> Result<(), InitError> {
        if self.is_failed() {
            return Err(InitError::Abi(AbiError::Execution(format!(
                "plugin '{}' has permanently failed — reload STUI or reinstall the plugin",
                self.plugin_name,
            ))));
        }

        // Serialize before acquiring the instance lock — no borrow of `req`
        // crosses the async boundary.
        let json = serde_json::to_string(req)
            .map_err(|e| InitError::Abi(AbiError::Serde(e)))?;

        let timeout = Duration::from_secs(self.config.call_timeout_secs);
        let result = {
            let mut guard = self.instance.lock().await;
            match guard.as_mut() {
                None => return Err(InitError::Abi(AbiError::Execution(format!(
                    "plugin '{}' is reloading, try again shortly",
                    self.plugin_name,
                )))),
                Some(inst) => {
                    tokio::time::timeout(timeout, inst.call_init_with_json(&json)).await
                }
            }
        };

        match result {
            Ok(Ok(()))  => Ok(()),
            Ok(Err(e))  => {
                // Only count plumbing errors as crashes — a plugin reporting
                // its own InitError::Plugin is working as intended.
                if matches!(&e, InitError::Abi(_)) {
                    self.on_crash(&format!("init abi error: {e}")).await;
                }
                Err(e)
            }
            Err(_elapsed) => {
                let msg = format!(
                    "plugin '{}' init timed out after {}s",
                    self.plugin_name, self.config.call_timeout_secs,
                );
                warn!("{msg}");
                self.on_crash("init timeout").await;
                Err(InitError::Abi(AbiError::Execution(msg)))
            }
        }
    }

    /// Generic helper: enforce timeout + crash tracking for any verb.
    ///
    /// Serializes `req` up-front (before acquiring the instance lock) so the
    /// closure passed to `tokio::time::timeout` captures only owned data — no
    /// lifetime problems cross the await point.
    async fn call_verb<Req, Resp>(
        &self,
        fn_name:   &str,
        verb_name: &str,
        req: &Req,
    ) -> Result<Resp, AbiError>
    where
        Req:  serde::Serialize,
        Resp: for<'de> serde::Deserialize<'de>,
    {
        if self.is_failed() {
            return Err(AbiError::Execution(format!(
                "plugin '{}' has permanently failed — reload STUI or reinstall the plugin",
                self.plugin_name,
            )));
        }

        // Serialize before acquiring the instance lock — no borrow of `req`
        // crosses the async boundary.
        let json = serde_json::to_string(req).map_err(AbiError::Serde)?;

        let timeout = Duration::from_secs(self.config.call_timeout_secs);
        let result = {
            let mut guard = self.instance.lock().await;
            match guard.as_mut() {
                None => return Err(AbiError::Execution(format!(
                    "plugin '{}' is reloading, try again shortly",
                    self.plugin_name,
                ))),
                Some(inst) => {
                    let fn_name = fn_name.to_string();
                    tokio::time::timeout(timeout, inst.call_export_typed(&fn_name, &json)).await
                }
            }
        };

        match result {
            Ok(Ok(r))     => Ok(r),
            Ok(Err(e))    => { self.on_crash(&format!("trap: {e}")).await; Err(e) }
            Err(_elapsed) => {
                let msg = format!(
                    "plugin '{}' {} timed out after {}s",
                    self.plugin_name, verb_name, self.config.call_timeout_secs,
                );
                warn!("{msg}");
                self.on_crash("call timeout").await;
                Err(AbiError::Execution(msg))
            }
        }
    }

    /// Call `stui_search` with timeout and crash tracking.
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse, AbiError> {
        self.call_verb("stui_search", "search", req).await
    }

    /// Call `stui_resolve` with timeout and crash tracking.
    pub async fn resolve(&self, req: &ResolveRequest) -> Result<ResolveResponse, AbiError> {
        self.call_verb("stui_resolve", "resolve", req).await
    }

    /// Call `stui_lookup` with timeout and crash tracking.
    pub async fn lookup(&self, req: &LookupRequest) -> Result<LookupResponse, AbiError> {
        self.call_verb("stui_lookup", "lookup", req).await
    }

    /// Call `stui_enrich` with timeout and crash tracking.
    pub async fn enrich(&self, req: &EnrichRequest) -> Result<EnrichResponse, AbiError> {
        self.call_verb("stui_enrich", "enrich", req).await
    }

    /// Call `stui_get_artwork` with timeout and crash tracking.
    pub async fn get_artwork(&self, req: &ArtworkRequest) -> Result<ArtworkResponse, AbiError> {
        self.call_verb("stui_get_artwork", "get_artwork", req).await
    }

    /// Call `stui_get_credits` with timeout and crash tracking.
    pub async fn get_credits(&self, req: &CreditsRequest) -> Result<CreditsResponse, AbiError> {
        self.call_verb("stui_get_credits", "get_credits", req).await
    }

    /// Call `stui_related` with timeout and crash tracking.
    pub async fn related(&self, req: &RelatedRequest) -> Result<RelatedResponse, AbiError> {
        self.call_verb("stui_related", "related", req).await
    }

    // ── Internals ─────────────────────────────────────────────────────────

    /// Record a crash, drop the bad instance, and spawn a background reload.
    async fn on_crash(&self, reason: &str) {
        if self.failed.load(Ordering::Relaxed) {
            return;
        }

        // Update crash timestamps — prune events outside the sliding window.
        let now = Instant::now();
        let window = Duration::from_secs(self.config.crash_window_secs);
        {
            let mut times = self.crash_times.lock().await;
            times.retain(|t| now.duration_since(*t) < window);
            times.push(now);
        }

        {
            let mut s = self.stats.lock().await;
            s.crash_count += 1;
            s.is_alive = false;
        }

        // Drop the bad instance before logging so callers get "reloading" instead of trapped state.
        *self.instance.lock().await = None;

        // Check if we've exceeded the crash threshold.
        let crashes_in_window = self.crash_times.lock().await.len();
        if crashes_in_window > self.config.max_reloads as usize {
            error!(
                plugin = %self.plugin_name,
                crashes = crashes_in_window,
                window  = self.config.crash_window_secs,
                "WASM plugin crash loop detected — permanently failing",
            );
            self.failed.store(true, Ordering::Relaxed);
            self.stats.lock().await.permanently_failed = true;
            return;
        }

        warn!(
            plugin = %self.plugin_name,
            reason,
            crashes_in_window,
            max = self.config.max_reloads,
            "WASM plugin crashed — scheduling reload",
        );

        // Compute backoff: 1s, 2s, 4s, …, capped at backoff_max_ms.
        let backoff_ms = std::cmp::min(
            self.config.backoff_base_ms
                * (1u64 << (crashes_in_window as u64).saturating_sub(1)),
            self.config.backoff_max_ms,
        );

        // Spawn the reload so we don't hold up the failing call.
        let instance    = Arc::clone(&self.instance);
        let stats       = Arc::clone(&self.stats);
        let wasm_path   = self.wasm_path.clone();
        let plugin_name = self.plugin_name.clone();
        let ctx         = self.ctx.clone();
        let max_mem     = self.config.max_memory_mb;

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            match WasmHost::load(&wasm_path, &plugin_name, &ctx, max_mem).await {
                Ok(inst) => {
                    info!(plugin = %plugin_name, "WASM plugin reloaded successfully");
                    let mut s = stats.lock().await;
                    s.reload_count += 1;
                    s.is_alive = true;
                    *instance.lock().await = Some(inst);
                }
                Err(e) => {
                    error!(plugin = %plugin_name, err = %e, "WASM plugin reload failed — instance remains unavailable");
                }
            }
        });
    }
}
