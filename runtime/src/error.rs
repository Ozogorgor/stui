//! Unified error type for the stui runtime.
//!
//! All fallible operations in the runtime should ultimately resolve to
//! `StuidError` (or `anyhow::Error` for ad-hoc chaining).  Using a typed
//! enum instead of bare strings lets callers pattern-match on specific
//! failure modes and produce better user-facing messages.
//!
//! # Design
//!
//! - Leaf errors that originate in this crate use `#[error("…")]` variants.
//! - Foreign errors are wrapped with `#[from]` so `?` works automatically.
//! - `anyhow::Error` is still used freely inside functions; it converts to
//!   `StuidError::Other` at API boundaries.
//!
//! # Example
//!
//! ```rust
//! use stui_runtime::error::StuidError;
//!
//! fn load(path: &str) -> Result<(), StuidError> {
//!     std::fs::read(path).map_err(StuidError::Io)?;
//!     Ok(())
//! }
//! ```

use thiserror::Error;

/// The master error enum for the stui runtime.
#[derive(Debug, Error)]
pub enum StuidError {
    // ── I/O ──────────────────────────────────────────────────────────────
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    // ── JSON / serialisation ──────────────────────────────────────────────
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    // ── HTTP / network ────────────────────────────────────────────────────
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    // ── Plugin system ─────────────────────────────────────────────────────
    #[error("plugin not found: {0}")]
    PluginNotFound(String),

    #[error("plugin load failed ({name}): {reason}")]
    PluginLoad { name: String, reason: String },

    #[error("plugin ABI error: {0}")]
    PluginAbi(String),

    // ── Provider ─────────────────────────────────────────────────────────
    #[error("provider '{provider}' failed: {reason}")]
    Provider { provider: String, reason: String },

    #[error("no stream found for '{entry_id}'")]
    NoStream { entry_id: String },

    // ── Player ───────────────────────────────────────────────────────────
    #[error("mpv error: {0}")]
    Mpv(String),

    #[error("aria2 error: {0}")]
    Aria2(String),

    // ── Config ───────────────────────────────────────────────────────────
    #[error("config error: {0}")]
    Config(String),

    // ── IPC ──────────────────────────────────────────────────────────────
    #[error("IPC protocol error: {0}")]
    Ipc(String),

    // ── Provider health ───────────────────────────────────────────────────

    /// Provider was rate-limited (HTTP 429 or explicit backoff).
    #[error("provider '{provider}' rate limited (retry after {retry_after_secs}s)")]
    RateLimited { provider: String, retry_after_secs: u64 },

    /// Provider did not respond within the allowed timeout.
    #[error("provider '{provider}' timed out after {timeout_ms}ms")]
    ProviderTimeout { provider: String, timeout_ms: u64 },

    // ── Streaming ─────────────────────────────────────────────────────────

    /// A stream that was playing has failed mid-playback.
    #[error("stream failed ({reason}): {url}")]
    StreamFailure { url: String, reason: String },

    /// All available stream candidates have been exhausted.
    #[error("all stream candidates exhausted for '{entry_id}'")]
    AllCandidatesExhausted { entry_id: String },

    // ── Cache ─────────────────────────────────────────────────────────────

    #[error("cache error: {0}")]
    Cache(String),

    // ── Catch-all ─────────────────────────────────────────────────────────
    /// Wraps an `anyhow::Error` at an API boundary where a typed variant
    /// would be excessive.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout the runtime.
pub type Result<T, E = StuidError> = std::result::Result<T, E>;

// ── Helper constructors ───────────────────────────────────────────────────────

impl StuidError {
    pub fn plugin_not_found(name: impl Into<String>) -> Self {
        StuidError::PluginNotFound(name.into())
    }

    pub fn plugin_load(name: impl Into<String>, reason: impl Into<String>) -> Self {
        StuidError::PluginLoad { name: name.into(), reason: reason.into() }
    }

    pub fn provider(provider: impl Into<String>, reason: impl Into<String>) -> Self {
        StuidError::Provider { provider: provider.into(), reason: reason.into() }
    }

    pub fn no_stream(entry_id: impl Into<String>) -> Self {
        StuidError::NoStream { entry_id: entry_id.into() }
    }

    pub fn mpv(msg: impl Into<String>) -> Self {
        StuidError::Mpv(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        StuidError::Config(msg.into())
    }

    pub fn ipc(msg: impl Into<String>) -> Self {
        StuidError::Ipc(msg.into())
    }

    pub fn rate_limited(provider: impl Into<String>, retry_after_secs: u64) -> Self {
        StuidError::RateLimited { provider: provider.into(), retry_after_secs }
    }

    pub fn stream_failure(url: impl Into<String>, reason: impl Into<String>) -> Self {
        StuidError::StreamFailure { url: url.into(), reason: reason.into() }
    }

    // ── Classification ────────────────────────────────────────────────────

    /// True if the operation that failed can be retried without user action.
    ///
    /// Used by the pipeline to decide whether to try the next stream candidate
    /// automatically or surface an error to the TUI.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            StuidError::Provider { .. }
            | StuidError::RateLimited { .. }
            | StuidError::ProviderTimeout { .. }
            | StuidError::StreamFailure { .. }
            | StuidError::NoStream { .. }
        )
    }

    /// True if this error should trigger a provider health penalty.
    pub fn is_provider_failure(&self) -> bool {
        matches!(
            self,
            StuidError::Provider { .. }
            | StuidError::RateLimited { .. }
            | StuidError::ProviderTimeout { .. }
        )
    }

    /// A short, user-friendly message suitable for a TUI toast notification.
    pub fn user_message(&self) -> String {
        match self {
            StuidError::NoStream { entry_id } =>
                format!("No streams found for \"{entry_id}\""),
            StuidError::RateLimited { provider, retry_after_secs } =>
                format!("{provider} rate limited — retrying in {retry_after_secs}s"),
            StuidError::ProviderTimeout { provider, .. } =>
                format!("{provider} timed out"),
            StuidError::StreamFailure { reason, .. } =>
                format!("Stream failed: {reason}"),
            StuidError::AllCandidatesExhausted { .. } =>
                "No working streams found".to_string(),
            StuidError::Mpv(msg) =>
                format!("Player error: {msg}"),
            StuidError::PluginNotFound(name) =>
                format!("Plugin not found: {name}"),
            other =>
                other.to_string(),
        }
    }
}
