//! `RuntimeEvent` — every internal event the stui runtime can emit.
//!
//! Modules communicate by emitting events onto the [`super::EventBus`] rather
//! than calling each other directly.  This keeps the dependency graph flat:
//!
//! ```text
//! engine  ──▶  EventBus  ◀──  player
//!               │    │
//!               ▼    ▼
//!             ipc   cache   plugin_manager   scrobbler   …
//! ```
//!
//! # Adding a new event
//!
//! 1. Add a variant to `RuntimeEvent` below.
//! 2. Emit it at the right call site via `bus.emit(RuntimeEvent::Foo { … })`.
//! 3. Subscribe and handle it wherever the event should be observed.
//!
//! # Stability
//!
//! `RuntimeEvent` is an internal enum — it does not cross the IPC boundary.
//! For TUI notifications, emit the appropriate `ipc::Response` separately.

use crate::catalog::CatalogEntry;
use crate::quality::StreamCandidate;

/// Every event the runtime can emit.
///
/// Events are cloned to each subscriber, so keep payloads small.  For large
/// data (e.g. full catalog pages) prefer passing a summary or an identifier
/// and letting the subscriber fetch from the cache.
#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    // ── Search & catalog ─────────────────────────────────────────────────

    /// User submitted a search query (before providers are contacted).
    SearchRequested {
        query: String,
        tab:   String,
    },

    /// A provider returned search results.
    SearchResultsReady {
        query:    String,
        tab:      String,
        provider: String,
        count:    usize,
    },

    // ── Stream resolution ─────────────────────────────────────────────────

    /// A media item was selected and stream resolution has started.
    MediaSelected {
        entry_id: String,
        title:    String,
    },

    /// Stream candidates have been resolved and ranked for a media item.
    StreamsResolved {
        entry_id:   String,
        candidates: Vec<StreamCandidate>,
    },

    /// The best candidate was selected for playback.
    StreamSelected {
        entry_id:  String,
        url:       String,
        protocol:  String,
        quality:   Option<String>,
    },

    // ── Playback ──────────────────────────────────────────────────────────

    /// mpv has started playing a file/URL.
    PlaybackStarted {
        title:    String,
        url:      String,
        duration: f64,
    },

    /// Periodic progress tick (~1 Hz) during active playback.
    PlaybackProgress {
        position:      f64,
        duration:      f64,
        paused:        bool,
        cache_percent: f64,
    },

    /// Playback ended (end of file, user quit, or error).
    PlaybackEnded {
        /// `"eof"` | `"quit"` | `"error"`
        reason: String,
        error:  Option<String>,
    },

    /// User requested a stream switch mid-playback (next candidate).
    StreamSwitchRequested {
        entry_id: String,
    },

    // ── Provider health ───────────────────────────────────────────────────

    /// A provider returned an error for a request.
    ProviderError {
        provider: String,
        message:  String,
    },

    /// A provider succeeded — used for health-score tracking.
    ProviderSuccess {
        provider:   String,
        latency_ms: u64,
    },

    /// A provider was rate-limited (HTTP 429).
    ProviderRateLimited {
        provider:          String,
        retry_after_secs:  u64,
    },

    /// A provider timed out.
    ProviderTimedOut {
        provider:   String,
        timeout_ms: u64,
    },

    /// All stream candidates failed or were exhausted.
    AllCandidatesExhausted {
        entry_id: String,
    },

    // ── Plugin lifecycle ──────────────────────────────────────────────────

    /// A plugin was successfully loaded (WASM or RPC).
    PluginLoaded {
        id:           String,
        name:         String,
        version:      String,
        capabilities: Vec<String>,
    },

    /// A plugin was unloaded (directory removed or explicit call).
    PluginUnloaded {
        id:   String,
        name: String,
    },

    /// A plugin emitted an error.
    PluginError {
        name:    String,
        message: String,
    },

    // ── Cache ─────────────────────────────────────────────────────────────

    /// A cache entry was populated (for observability / metrics).
    CachePopulated {
        cache: String, // "search" | "metadata" | "streams" | "catalog"
        key:   String,
    },

    // ── Catalog grid ─────────────────────────────────────────────────────

    /// The catalog grid for a tab was refreshed from a provider.
    CatalogRefreshed {
        tab:     String,
        entries: Vec<CatalogEntry>,
        source:  String, // "cache" | "live"
    },

    // ── Config ───────────────────────────────────────────────────────────

    /// A runtime config value was changed at runtime (via SetConfig IPC).
    ConfigChanged {
        /// Dot-separated config key, e.g. `"player.default_volume"`.
        key:   String,
        /// JSON-serialised new value.
        value: String,
    },

    // ── Shutdown ──────────────────────────────────────────────────────────

    /// A graceful shutdown was requested.
    ShutdownRequested,
}

impl RuntimeEvent {
    /// Short label for logging/tracing.
    pub fn name(&self) -> &'static str {
        match self {
            RuntimeEvent::SearchRequested       { .. } => "search_requested",
            RuntimeEvent::SearchResultsReady    { .. } => "search_results_ready",
            RuntimeEvent::MediaSelected         { .. } => "media_selected",
            RuntimeEvent::StreamsResolved       { .. } => "streams_resolved",
            RuntimeEvent::StreamSelected        { .. } => "stream_selected",
            RuntimeEvent::PlaybackStarted       { .. } => "playback_started",
            RuntimeEvent::PlaybackProgress      { .. } => "playback_progress",
            RuntimeEvent::PlaybackEnded         { .. } => "playback_ended",
            RuntimeEvent::StreamSwitchRequested { .. } => "stream_switch_requested",
            RuntimeEvent::ProviderError         { .. } => "provider_error",
            RuntimeEvent::ProviderSuccess       { .. } => "provider_success",
            RuntimeEvent::PluginLoaded          { .. } => "plugin_loaded",
            RuntimeEvent::PluginUnloaded        { .. } => "plugin_unloaded",
            RuntimeEvent::PluginError           { .. } => "plugin_error",
            RuntimeEvent::CachePopulated        { .. } => "cache_populated",
            RuntimeEvent::CatalogRefreshed      { .. } => "catalog_refreshed",
            RuntimeEvent::ShutdownRequested             => "shutdown_requested",
            RuntimeEvent::ProviderRateLimited   { .. } => "provider_rate_limited",
            RuntimeEvent::ProviderTimedOut      { .. } => "provider_timed_out",
            RuntimeEvent::AllCandidatesExhausted{ .. } => "all_candidates_exhausted",
            RuntimeEvent::ConfigChanged         { .. } => "config_changed",
        }
    }
}
