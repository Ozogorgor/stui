//! IPC layer — wire protocol between the Go TUI and the Rust runtime.
//!
//! # Versioning
//!
//! The protocol is versioned to allow backward-compatible evolution.
//! All current types live in `v1/`.  A future `v2/` can introduce breaking
//! changes while `v1/` stays importable for clients that haven't upgraded.
//!
//! ```text
//! ipc/
//!   mod.rs    - this file; re-exports the active version (v1)
//!   v1/       - all current wire types
//!     mod.rs  - Request, Response, events, domain types
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

/// Re-export the streaming-event primitive for use by request handlers.
pub use v1::stream::{emit as emit_event, Event, EventSender};
pub use v1::ScopeResultsMsg;

/// Current active protocol version.  Bump when introducing breaking changes
/// (and add a new `v2` module to maintain backward compat with old clients).
pub const CURRENT_VERSION: u32 = 1;

// Re-export everything from v1 so existing `use crate::ipc::Foo` imports
// continue to compile with zero changes throughout the codebase.
#[allow(unused_imports)]
pub use v1::{
    ClearMediaCacheRequest,
    DetailEntry,
    GetAlbumArtRequest,
    GetAlbumArtResponse,

    DspStatusResponse,
    ErrorCode,

    ErrorResponse,

    GetMediaCacheAllRequest,
    GetMediaCacheStatsRequest,
    // Media cache types
    GetMediaCacheTabRequest,
    GetStreamsRequest,
    // Watch history types
    GetWatchHistoryEntryRequest,
    GetWatchHistoryInProgressRequest,
    // Out-of-band events
    GridUpdateEvent,
    // Backward-compat aliases
    GridUpdateMsg,
    // Registry types
    InstallPluginRequest,
    LoadConvolutionFilterRequest,
    LoadPluginRequest,
    MarkWatchHistoryCompletedRequest,
    MediaCacheAllResponse,
    MediaCacheClearResponse,
    MediaCacheStatsResponse,
    MediaCacheTabResponse,
    MediaEntry,
    MediaTab,
    // Domain types
    MediaType,
    MetadataRequest,
    MetadataResponse,
    // MPD output types
    MpdOutputInfo,
    MpdOutputsResponse,
    // MPD library / browse types
    MpdAlbumWire,
    MpdArtistWire,
    MpdBrowseRequest,
    MpdBrowseResponse,
    MpdDirEntryWire,
    MpdGetPlaylistRequest,
    MpdGetPlaylistResponse,
    MpdGetPlaylistsRequest,
    MpdGetPlaylistsResponse,
    MpdGetQueueRequest,
    MpdGetQueueResponse,
    MpdListRequest,
    MpdListResponse,
    MpdQueueTrackWire,
    MpdSavedPlaylistWire,
    MpdSongWire,

    PlayRequest,
    // New typed command types
    PlayerCmd,
    PlayerCommandRequest,
    PlayerEndedEvent,

    PlayerProgressEvent,
    PlayerStartedEvent,
    PluginInfo,
    PluginInstalledResponse,

    PluginListResponse,
    PluginLoadedResponse,
    PluginReposResponse,

    PluginStatus,
    PluginToastEvent,
    PluginUnloadedResponse,
    // Provider settings schema
    ProviderField,
    ProviderSchema,
    ProviderSettingsResponse,

    // Stream ranking types
    RankStreamsRequest,
    RankStreamsResponse,
    RankedStreamWire,
    RegistryEntryWire,
    RegistryIndexResponse,
    RemoveWatchHistoryEntryRequest,
    // Requests
    Request,
    ResolveRequest,
    ResolveResponse,
    // Responses
    Response,
    SearchRequest,
    SearchResponse,
    SetConfigRequest,

    // DSP types
    SetDspConfigRequest,
    // Plugin repo types
    SetPluginReposRequest,
    // Storage paths types
    SetStoragePathsRequest,
    // Stream policy types
    SetStreamPolicyRequest,
    StoragePathsResponse,
    StreamInfoWire,
    StreamPolicyResponse,
    StreamPreferencesWire,
    StreamsResponse,
    SubtitleTrack,
    UnloadPluginRequest,

    UpdateWatchHistoryPositionRequest,
    UpsertWatchHistoryEntryRequest,
    WatchHistoryEntryResponse,
    WatchHistoryEntryWire,
    WatchHistoryInProgressResponse,
    WatchHistoryPositionUpdateResponse,
    WatchHistoryRemoveResponse,
    WatchHistoryUpsertResponse,

    // Tag normalization
    MarkTagExceptionRequest,
    MarkTagExceptionResponse,
    ActionATagsPreviewRequest,
    ActionATagsPreviewResponse,
    ActionATagsApplyRequest,
    ActionATagsApplyResponse,
    ActionATagsCancelRequest,
    ActionATagsCancelResponse,
    TagDiffRowWire,
    TagWriteScope,
};
