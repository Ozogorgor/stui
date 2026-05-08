//! Per-plugin rate-limited supervisor, wrapping `abi::supervisor::WasmSupervisor`.
//!
//! The `TokenBucket` here is an MVP — it awaits-until-tokens-available; a
//! timeout/nonblocking mode can be layered on later. It satisfies the rate
//! declarations in plugin.toml's `[rate_limit]` section.

#![allow(dead_code)]

use std::sync::Arc;

use super::manifest::RateLimit;
use super::rate_limit::TokenBucket;
use crate::abi::supervisor::WasmSupervisor;

/// Wraps a WASM supervisor with an optional rate-limiter applied before each call.
///
/// Keep this thin — the heavy lifting (timeouts, crash detection, reload) lives
/// in `abi::supervisor::WasmSupervisor`. The supervisor crate owns plugin
/// lifecycle; this module owns access-pacing + future per-plugin throttling.
pub struct PluginSupervisor {
    pub(crate) wasm: Arc<WasmSupervisor>,
    pub(crate) rate_limit: Option<TokenBucket>,
}

impl PluginSupervisor {
    /// Build a supervisor. `rate_limit` pulled from the manifest; `None` means
    /// unlimited.
    pub fn new(wasm: Arc<WasmSupervisor>, rate_limit: Option<&RateLimit>) -> Self {
        let bucket = rate_limit.map(|rl| TokenBucket::new(rl.rps, rl.burst));
        Self {
            wasm,
            rate_limit: bucket,
        }
    }

    /// Acquire a rate-limit token (if configured) before returning. The caller
    /// then performs the actual WasmSupervisor call.
    ///
    /// The acquire-then-call pattern (rather than wrapping the call itself)
    /// keeps this module decoupled from the specific verb signatures on
    /// `WasmSupervisor`, which are being expanded in Task 1.8/1.9.
    pub async fn acquire(&self) {
        if let Some(rl) = &self.rate_limit {
            rl.acquire().await;
        }
    }
}
