//! IPC layer — wire protocol between the Go TUI and the Rust runtime.
//!
//! # Versioning
//!
//! The protocol is versioned to allow backward-compatible evolution.
//! All current types live in `v1/`.  A future `v2/` can introduce breaking
//! changes while `v1/` stays importable for clients that haven't upgraded.
//!
//! ```
//! ipc/
//!   mod.rs    ← this file; re-exports the active version (v1)
//!   v1/       ← all current wire types
//!     mod.rs  ← Request, Response, events, domain types
//! ```
//!
//! # Selecting a protocol version
//!
//! The version is negotiated during the initial handshake (`Ping` /
//! `Pong` exchange).  The TUI sends `{"type":"ping","version":1}` and the
//! runtime replies `{"type":"pong","version":1}`.  Mismatched versions
//! result in an error response and graceful degradation.
//!
//! # Transport
//!
//! Newline-delimited JSON (NDJSON) over either:
//! - `stdin` / `stdout` — when launched by the TUI directly
//! - Unix domain socket (`~/.local/run/stui.sock`) — in daemon mode
//!
//! # Adding a new message
//!
//! 1. Add the struct to `v1/mod.rs`.
//! 2. Add a variant to `Request` or `Response` in `v1/mod.rs`.
//! 3. Handle it in `main.rs`'s `handle_request()`.
//! 4. Add a matching case in `tui/internal/ipc/ipc.go`.
//! 5. Update `docs/runtime-ipc.md`.

pub mod v1;

/// Current active protocol version.  Bump when introducing breaking changes
/// (and add a new `v2` module to maintain backward compat with old clients).
pub const CURRENT_VERSION: u32 = 1;

// Re-export everything from v1 so existing `use crate::ipc::Foo` imports
// continue to compile with zero changes throughout the codebase.
pub use v1::{
    // Requests
    Request,
    PlayRequest,
    PlayerCommandRequest,
    SearchRequest,
    ResolveRequest,
    GetStreamsRequest,
    MetadataRequest,
    LoadPluginRequest,
    UnloadPluginRequest,

    // Responses
    Response,
    SearchResponse,
    ResolveResponse,
    StreamsResponse,
    StreamInfoWire,
    MetadataResponse,
    PluginListResponse,
    PluginLoadedResponse,
    PluginUnloadedResponse,
    ErrorResponse,

    // Out-of-band events
    GridUpdateEvent,
    PluginToastEvent,
    PlayerStartedEvent,
    PlayerProgressEvent,
    PlayerEndedEvent,

    // Domain types
    MediaType,
    MediaTab,
    MediaEntry,
    SubtitleTrack,
    PluginInfo,
    PluginStatus,
    ErrorCode,

    // Backward-compat aliases
    GridUpdateMsg,
    DetailEntry,

    // New typed command types
    PlayerCmd,
    SetConfigRequest,

    // Provider settings schema
    ProviderField,
    ProviderSchema,
    ProviderSettingsResponse,

    // MPD output types
    MpdOutputInfo,
    MpdOutputsResponse,

    // Plugin repo types
    SetPluginReposRequest,
    PluginReposResponse,

    // Registry types
    InstallPluginRequest,
    RegistryEntryWire,
    RegistryIndexResponse,
    PluginInstalledResponse,
};
