//! IPC wire schema **v1** — the current protocol version.
//!
//! Imported exclusively via `crate::ipc` re-exports; do not import this
//! module directly.  If you need to add breaking changes, create `v2/mod.rs`
//! and update `ipc/mod.rs` to re-export from it instead.
//!
//! # Design rules
//!
//! - All types derive `Serialize` + `Deserialize`.
//! - Enums use `#[serde(tag = "type", rename_all = "snake_case")]` so the
//!   JSON discriminant is a `"type"` field — readable and debuggable.
//! - All optional fields use `#[serde(default)]` so older clients don't break
//!   when new fields are added.
//! - New request/response variants are always backward-compatible additions.

#![allow(dead_code)]

pub mod metadata;
pub mod stream;

pub use metadata::{
    ArtworkData, ArtworkVariantWire, CastWire, CreditsData, CrewWire, DetailMetadataPartial,
    EnrichData, GetDetailMetadataRequest, MetadataPayload, RatingsAggregatorData, RelatedData,
    RelatedItemWire,
};

use serde::{Deserialize, Serialize};
use stui_plugin_sdk::{EntryKind, SearchScope};

// ── Streaming-event scaffold ─────────────────────────────────────────────────

/// Streaming result frame for a single search scope.
///
/// Sent once per scope per fan-out as results arrive.  `partial = true`
/// means the runtime is still collecting from other providers; the TUI
/// should accumulate and re-render.  `partial = false` is the terminal
/// frame for this scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeResultsMsg {
    /// Correlation ID echoing the originating `SearchRequest::query_id`.
    pub query_id: u64,
    /// Which scope this frame covers.
    pub scope: SearchScope,
    /// Results collected so far for this scope.
    pub entries: Vec<MediaEntry>,
    /// `true` if more frames for this scope may follow.
    pub partial: bool,
    /// Set when all providers for this scope failed or none were configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ScopeError>,
}

/// Error variants for a single-scope search failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScopeError {
    /// Every provider attempted returned an error.
    AllFailed,
    /// No plugins are configured that cover this scope.
    NoPluginsConfigured,
}

// ── Requests (Go → Rust) ─────────────────────────────────────────────────────

/// Every message sent from the TUI to the runtime.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Full-text search across active providers.
    Search(SearchRequest),
    /// Force-refresh a catalog tab — clears the in-mem SearchCache for that
    /// (tab, scope) space and re-dispatches provider searches. Used by the
    /// `R` hotkey on the grid and by future offline→online transitions.
    CatalogRefresh(CatalogRefreshRequest),
    /// Resolve a catalog entry into a stream URL (without playing).
    Resolve(ResolveRequest),
    /// Fetch all ranked stream candidates for a catalog entry.
    GetStreams(GetStreamsRequest),
    /// Fetch enriched metadata for a media entry.
    Metadata(MetadataRequest),
    /// Fetch enriched detail metadata, fanning out the four metadata verbs
    /// (enrich, credits, artwork, related) through the source-priority list.
    /// Partials stream back as `Response::DetailMetadataPartial` events.
    GetDetailMetadata(GetDetailMetadataRequest),
    /// Resolve + hand off to the player (torrent engine → mpv, or direct mpv).
    Play(PlayRequest),
    /// Stop current playback; kills mpv and any active torrent.
    PlayerStop,
    /// Send a raw mpv IPC command (e.g. `{"cmd":"cycle","args":["pause"]}`).
    PlayerCommand(PlayerCommandRequest),
    /// List all currently loaded plugins.
    ListPlugins,
    /// Dynamically load a plugin by filesystem path.
    LoadPlugin(LoadPluginRequest),
    /// Unload a loaded plugin by its ID.
    UnloadPlugin(UnloadPluginRequest),
    /// Toggle whether a loaded plugin participates in dispatch. The
    /// plugin stays in the registry either way — this is a soft
    /// enable/disable, not an uninstall.
    SetPluginEnabled(SetPluginEnabledRequest),
    /// Health-check ping; runtime replies with `Response::Pong`.
    ///
    /// `ipc_version` — the TUI's protocol version number.  If absent (old
    /// client) the runtime still responds but logs a warning.
    Ping {
        #[serde(default)]
        ipc_version: Option<u32>,
    },
    /// Graceful shutdown request.
    Shutdown,
    /// Typed player command (preferred over `PlayerCommand` for new code).
    Cmd(PlayerCmd),
    /// Live-update a runtime config value without restart.
    SetConfig(SetConfigRequest),
    /// Fetch provider settings schema (names, key slots, configured status).
    GetProviderSettings,
    /// Fetch the list of MPD audio outputs.
    GetMpdOutputs,

    // ── MPD library / browse queries ──────────────────────────────────────────
    /// Fetch the current MPD playback queue.
    MpdGetQueue(MpdGetQueueRequest),
    /// List MPD library entities (artists / albums / songs).
    MpdList(MpdListRequest),
    /// Browse the MPD music database by path.
    MpdBrowse(MpdBrowseRequest),
    /// List saved MPD playlists.
    MpdGetPlaylists(MpdGetPlaylistsRequest),
    /// Fetch tracks in a saved MPD playlist.
    MpdGetPlaylist(MpdGetPlaylistRequest),
    /// Search the MPD library by artist, album, or track.
    MpdSearch(MpdSearchRequest),

    /// Fetch a lastfm album's tracklist (album.getInfo). Used by
    /// the AlbumDetail screen in Music Browse — lastfm-sourced
    /// albums have no MPD library backing, so the runtime hits
    /// last.fm directly to enumerate tracks.
    LastfmAlbumTracks(LastfmAlbumTracksRequest),

    /// List the metadata-source plugins that the runtime would
    /// consult for `(verb, kind)` — both the user-curated priority
    /// list and the auto-discovered plugins (manifest-tagged for
    /// the kind). Drives the Settings → Metadata Sources screen so
    /// the user can see who's contributing and toggle entries on /
    /// off via the disabled list. See `MetadataPluginsForKindRequest`.
    MetadataPluginsForKind(MetadataPluginsForKindRequest),

    /// Fetch the current plugin repository list.
    GetPluginRepos,
    /// Replace the plugin repository list (built-in repo is always prepended by the runtime).
    SetPluginRepos(SetPluginReposRequest),
    /// Fetch the merged plugin index from all configured registries.
    BrowseRegistry,
    /// Download and install a plugin from a registry entry.
    InstallPlugin(InstallPluginRequest),
    /// Rank streams according to a user policy, returning scored results with explanations.
    RankStreams(RankStreamsRequest),

    // ── Watch history requests ──────────────────────────────────────────────────
    /// Get watch history entry by ID.
    GetWatchHistoryEntry(GetWatchHistoryEntryRequest),
    /// Get all in-progress entries for a tab.
    GetWatchHistoryInProgress(GetWatchHistoryInProgressRequest),
    /// Upsert (create or update) a watch history entry.
    UpsertWatchHistoryEntry(UpsertWatchHistoryEntryRequest),
    /// Update playback position for an entry.
    UpdateWatchHistoryPosition(UpdateWatchHistoryPositionRequest),
    /// Mark an entry as completed.
    MarkWatchHistoryCompleted(MarkWatchHistoryCompletedRequest),
    /// Remove a watch history entry.
    RemoveWatchHistoryEntry(RemoveWatchHistoryEntryRequest),

    // ── Media cache requests ──────────────────────────────────────────────────
    /// Get cached entries for a specific tab.
    GetMediaCacheTab(GetMediaCacheTabRequest),
    /// Get all cached entries across all tabs.
    GetMediaCacheAll(GetMediaCacheAllRequest),
    /// Get media cache statistics.
    GetMediaCacheStats(GetMediaCacheStatsRequest),
    /// Clear the entire media cache.
    ClearMediaCache(ClearMediaCacheRequest),

    // ── Storage paths requests ────────────────────────────────────────────────
    /// Get current storage directory paths for all media types.
    GetStoragePaths,
    /// Update storage directory paths.
    SetStoragePaths(SetStoragePathsRequest),

    // ── Stream policy requests ────────────────────────────────────────────────
    /// Fetch the persisted stream selection policy.
    GetStreamPolicy,
    /// Persist the stream selection policy.
    SetStreamPolicy(SetStreamPolicyRequest),

    /// Enable or disable the pipeline trace (stderr output for debugging).
    /// Sent by the TUI when `-v` / `--debug` is passed.
    SetTrace { enabled: bool },

    // ── DSP requests ───────────────────────────────────────────────────────────
    /// Get current DSP pipeline status.
    GetDspStatus,
    /// Update DSP configuration at runtime.
    SetDspConfig(SetDspConfigRequest),
    /// Load a convolution filter from file.
    LoadConvolutionFilter(LoadConvolutionFilterRequest),
    /// Bind DSP to MPD audio output.
    BindDspToMpd,
    /// List all saved DSP profiles.
    ListDspProfiles,
    /// Save the current DSP config as a named profile.
    SaveDspProfile(SaveDspProfileRequest),
    /// Load a named DSP profile.
    LoadDspProfile(LoadDspProfileRequest),
    /// Delete a named DSP profile.
    DeleteDspProfile(DeleteDspProfileRequest),

    // ── Album art ─────────────────────────────────────────────────────────
    /// Extract embedded album art from an audio file.
    GetAlbumArt(GetAlbumArtRequest),

    // ── Tag normalization ────────────────────────────────────────────────────
    /// Mark a raw tag value as an exception (protected from normalization).
    MarkTagException(MarkTagExceptionRequest),
    /// Compute the normalize-vs-raw diff for a scope, without writing.
    ActionATagsPreview(ActionATagsPreviewRequest),
    /// Apply a pre-computed Action A write set.
    ActionATagsApply(ActionATagsApplyRequest),
    /// Cancel an in-progress Action A run by job ID.
    ActionATagsCancel(ActionATagsCancelRequest),

    // ── Plugin verb requests ──────────────────────────────────────────────
    /// Look up a single entry by external ID via a named plugin.
    Lookup(LookupIpcRequest),
    /// Enrich a partial entry with additional metadata via a named plugin.
    Enrich(EnrichIpcRequest),
    /// Fetch artwork for an entry via a named plugin.
    GetArtwork(ArtworkIpcRequest),
    /// Fetch credits (cast + crew) for an entry via a named plugin.
    GetCredits(CreditsIpcRequest),
    /// Fetch related entries for an entry via a named plugin.
    Related(RelatedIpcRequest),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PlayRequest {
    /// Correlation ID echoed back in the player_started event.
    pub id: String,
    /// Catalog entry ID (e.g. `"tt0816692"`).
    pub entry_id: String,
    /// Provider that owns this entry.
    pub provider: String,
    /// IMDB ID for subtitle/metadata enrichment.
    #[serde(default)]
    pub imdb_id: String,
    /// UI tab the play request originated from (used to route audio to MPD).
    #[serde(default)]
    pub tab: Option<MediaTab>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PlayerCommandRequest {
    /// mpv property-based command name (e.g. `"cycle"`, `"seek"`).
    /// Prefer `PlayerCmd` for new code; this raw form is kept for
    /// forward-compatibility with older TUI versions.
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
}

/// Typed player command — the preferred IPC form for all new player requests.
///
/// Mirrors `crate::player::PlayerCommand`; defined here separately so the
/// IPC layer compiles without a direct player crate dependency.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum PlayerCmd {
    Pause,
    Resume,
    TogglePause,
    Seek {
        seconds: f64,
    },
    SeekAbsolute {
        seconds: f64,
    },
    Stop,
    SetVolume {
        level: f64,
    },
    AdjustVolume {
        delta: f64,
    },
    ToggleMute,
    SetSubtitleTrack {
        id: i64,
    },
    DisableSubtitles,
    CycleSubtitles,
    AdjustSubtitleDelay {
        delta: f64,
    },
    ResetSubtitleDelay,
    LoadSubtitle {
        path: String,
    },
    SetAudioTrack {
        id: i64,
    },
    CycleAudioTracks,
    AdjustAudioDelay {
        delta: f64,
    },
    ResetAudioDelay,
    SwitchStream {
        url: String,
        /// Optional media kind hint from the picker that emitted this command.
        /// Used by the playback router to distinguish a music-torrent magnet
        /// (→ librqbit album-stream + mpd queue) from the default video path
        /// (→ librqbit single-file stream + mpv). Absent for video pickers
        /// and any pre-existing call site, so the field is a strict addition.
        #[serde(default)]
        kind: Option<String>,
    },
    NextStreamCandidate,
    ToggleFullscreen,
    Screenshot,

    // ── MPD audio commands ─────────────────────────────────────────────────
    /// Skip to next track in MPD queue.
    MpdNext,
    /// Go back to previous track in MPD queue.
    MpdPrev,
    /// Shuffle the MPD queue.
    MpdShuffle,
    /// Clear the MPD queue.
    MpdClear,
    /// Enable or disable consume mode (remove after play).
    MpdConsume {
        enabled: bool,
    },
    /// Set ReplayGain mode: `"off"` | `"track"` | `"album"` | `"auto"`.
    ReplayGainMode {
        mode: String,
    },
    /// Toggle an MPD audio output on/off by its numeric ID.
    ToggleMpdOutput {
        id: u32,
    },
    /// Seek to an absolute position within the current track (seconds).
    MpdSeekAbsolute {
        seconds: f64,
    },
    /// Set MPD crossfade duration in seconds (0 = disabled).
    MpdCrossfade {
        secs: u32,
    },

    // ── MPD playlist commands ─────────────────────────────────────────────
    /// Save the current queue as a named playlist.
    MpdPlaylistSave {
        name: String,
    },
    /// Load a saved playlist into the queue (clear + load).
    MpdPlaylistLoad {
        name: String,
    },
    /// Append a saved playlist to the end of the queue.
    MpdPlaylistAppend {
        name: String,
    },
    /// Delete a saved playlist.
    MpdPlaylistDelete {
        name: String,
    },
    /// Add a track (by URI) to a saved playlist.
    MpdPlaylistAddTrack {
        name: String,
        uri: String,
    },
    /// Create a new empty playlist (clears if exists), then add URIs.
    MpdPlaylistCreate {
        name: String,
        uris: Vec<String>,
    },
    /// Remove a track from a saved playlist by position (0-based).
    MpdPlaylistRemoveTrack {
        name: String,
        pos: u32,
    },

    // ── MPD queue manipulation ────────────────────────────────────────────
    /// Add a URI to the MPD queue.
    MpdAdd {
        uri: String,
    },
    /// Remove a track from the queue by its MPD song ID.
    MpdRemove {
        id: u32,
    },
    /// Start playback of a specific track by its MPD song ID.
    MpdPlayId {
        id: u32,
    },
    /// Set MPD volume (0–100).
    MpdSetVolume {
        volume: u32,
    },
    /// Seek to a position within a track by song ID.
    MpdSeek {
        id: u32,
        time: f64,
    },
    /// Toggle MPD play/pause.
    MpdTogglePause,
    /// Stop MPD playback.
    MpdStop,
    /// Trigger MPD database rescan.
    MpdUpdate,
    /// Toggle MPD repeat mode.
    MpdToggleRepeat,
    /// Toggle MPD single mode (single-track repeat).
    MpdToggleSingle,
    /// Toggle MPD random/shuffle mode.
    MpdToggleRandom,
}

/// Live-update a runtime config value without restarting.
#[derive(Debug, Deserialize, Serialize)]
pub struct SetConfigRequest {
    /// Dot-separated config key, e.g. `"player.default_volume"`.
    pub key: String,
    /// JSON-encoded new value (will be validated against the config schema).
    pub value: serde_json::Value,
}

/// Wire-format search request from the TUI to the runtime.
///
/// Replaces the old flat-filter form (`tab`, `provider`, `sort`, `genre`,
/// `min_rating`, `year_from`, `year_to`).  Scope-based fan-out is handled
/// by the engine; per-scope results stream back as `Event::ScopeResults`.
///
/// Legacy fields were dropped; call sites in `pipeline::search` now use
/// defaults / stubs that will be rewritten in Task 2.9.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchRequest {
    /// Correlation ID echoed back in every `ScopeResultsMsg` for this query.
    pub id: String,
    pub query: String,
    /// Which scopes to search.  Empty = engine decides (all capabilities).
    pub scopes: Vec<SearchScope>,
    pub limit: u32,
    pub offset: u32,
    /// Monotonically increasing query counter used for in-flight de-duplication.
    pub query_id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ResolveRequest {
    pub id: String,
    pub entry_id: String,
    pub provider: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetStreamsRequest {
    pub id: String,
    pub entry_id: String,
    // ── New fields used by the StreamProvider find_streams flow. All
    //    optional with `#[serde(default)]` so old callers (the legacy
    //    stream picker) keep working — they pass entry_id only and
    //    the runtime falls back to the resolve_raw fan-out. New
    //    callers (Episodes tab streams column) populate these so
    //    stream providers can run torznab queries.
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub external_ids: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MetadataRequest {
    pub id: String,
    pub entry_id: String,
    /// Legacy field kept for the original "fetch the bag of metadata for
    /// this entry" callers. The episodes path leaves it empty and routes
    /// via `id_source` instead.
    #[serde(default)]
    pub provider: String,
    /// Discriminator selecting the metadata sub-flow. Today only
    /// `"episodes"` is honoured; legacy callers omit this and receive the
    /// generic detail-bag flow.
    #[serde(default)]
    pub kind: String,
    /// Plugin id to route the request to (e.g. `"tmdb"`). When empty the
    /// runtime peels a `<provider>-<id>` prefix from `entry_id`, falling
    /// back to `"tmdb"` for the episodes verb.
    #[serde(default)]
    pub id_source: String,
    /// Season number for the episodes verb. Ignored otherwise.
    #[serde(default)]
    pub season: u32,
    /// Cross-provider id map from the catalog entry, e.g.
    /// `{"imdb": "tt12345", "tvdb": "67890"}`. Used by the episodes
    /// fallback chain — when TMDB fails, the runtime reuses
    /// `external_ids["tvdb"]` to retry against the TVDB-native client
    /// without needing a roundtrip back to TMDB to discover the id.
    #[serde(default)]
    pub external_ids: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LoadPluginRequest {
    /// Filesystem path to the plugin directory (must contain `plugin.toml`).
    pub path: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UnloadPluginRequest {
    pub plugin_id: String,
}

/// Force-refresh a catalog tab. The runtime clears its in-mem SearchCache
/// entries for this tab's scope, then re-dispatches provider searches as
/// if the TTL had expired.
#[derive(Debug, Deserialize, Serialize)]
pub struct CatalogRefreshRequest {
    #[serde(default)]
    pub id: Option<String>,
    pub tab: MediaTab,
}

// ── Plugin verb IPC request structs ──────────────────────────────────────────

/// IPC wrapper for a `Lookup` request routed to a specific plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupIpcRequest {
    /// Correlation ID echoed back in `LookupIpcResponse`.
    pub query_id: u64,
    /// Plugin name to route this request to.
    pub plugin: String,
    #[serde(flatten)]
    pub inner: crate::abi::types::LookupRequest,
}

/// IPC wrapper for an `Enrich` request routed to a specific plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichIpcRequest {
    /// Correlation ID echoed back in `EnrichIpcResponse`.
    pub query_id: u64,
    /// Plugin name to route this request to.
    pub plugin: String,
    #[serde(flatten)]
    pub inner: crate::abi::types::EnrichRequest,
}

/// IPC wrapper for a `GetArtwork` request routed to a specific plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkIpcRequest {
    /// Correlation ID echoed back in `ArtworkIpcResponse`.
    pub query_id: u64,
    /// Plugin name to route this request to.
    pub plugin: String,
    #[serde(flatten)]
    pub inner: crate::abi::types::ArtworkRequest,
}

/// IPC wrapper for a `GetCredits` request routed to a specific plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsIpcRequest {
    /// Correlation ID echoed back in `CreditsIpcResponse`.
    pub query_id: u64,
    /// Plugin name to route this request to.
    pub plugin: String,
    #[serde(flatten)]
    pub inner: crate::abi::types::CreditsRequest,
}

/// IPC wrapper for a `Related` request routed to a specific plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedIpcRequest {
    /// Correlation ID echoed back in `RelatedIpcResponse`.
    pub query_id: u64,
    /// Plugin name to route this request to.
    pub plugin: String,
    #[serde(flatten)]
    pub inner: crate::abi::types::RelatedRequest,
}

// ── Responses (Rust → Go, in-band) ───────────────────────────────────────────

/// Every in-band response sent from the runtime to the TUI.
/// Out-of-band events (player progress, grid updates) use their own structs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Synchronous search result — kept for the Go `dispatchPersonSearch` path
    /// (Task 7.0 item 17).  No Rust-side producers remain after `Engine::search`
    /// retirement (Task 7.0 item 3).  Will be removed alongside the Go consumer.
    // NOTE: #[deprecated] cannot be placed on enum variants in stable Rust; the
    // removal is tracked as Task 7.0 item 17 (dispatchPersonSearch migration).
    SearchResult(SearchResponse),
    ResolveResult(ResolveResponse),
    StreamsResult(StreamsResponse),
    /// Per-provider streaming partial — emitted as each plugin returns
    /// during a `find_streams` fan-out. Unsolicited (no `id`
    /// correlation); the TUI matches on `(entry_id, season, episode)`.
    StreamsPartial(StreamsPartialWire),
    /// Final marker for a `find_streams` fan-out — emitted once after
    /// every provider has either returned a partial or hit the
    /// deadline. The TUI clears the in-flight spinner on receipt.
    StreamsComplete(StreamsCompleteWire),
    MetadataResult(MetadataResponse),
    PluginList(PluginListResponse),
    PluginLoaded(PluginLoadedResponse),
    PluginUnloaded(PluginUnloadedResponse),
    PluginEnabled(PluginEnabledResponse),
    /// Response to `Ping`.  Always carries version metadata so the TUI can
    /// detect mismatches and warn the user.  The correlation `id` is injected
    /// at the dispatcher level (see `inject_id_into_response` in `main.rs`).
    Pong {
        /// The runtime's active IPC protocol version (matches `ipc::CURRENT_VERSION`).
        ipc_version: u32,
        /// Human-readable semver string from `Cargo.toml`, e.g. `"0.8.1"`.
        runtime_version: String,
        /// Whether the TUI's requested version differs from ours.
        /// `true` = versions match; `false` = mismatch (warn but don't abort).
        version_ok: bool,
    },
    Ok,
    Error(ErrorResponse),
    /// Full playback state snapshot — pushed on every state change and
    /// in response to a `Ping` while playback is active.
    PlaybackState {
        position: f64,
        duration: f64,
        paused: bool,
        volume: f64,
        muted: bool,
        cache_percent: f64,
        audio_track: Option<i64>,
        subtitle_track: Option<i64>,
        subtitle_delay: f64,
        audio_delay: f64,
        title: String,
        quality: Option<String>,
        active_candidate: usize,
        candidate_count: usize,
    },
    /// Acknowledgement for a successful `SetConfig` request.
    ConfigUpdated {
        key: String,
    },
    /// Response to `GetProviderSettings`.
    ProviderSettings(ProviderSettingsResponse),
    /// Response to `GetMpdOutputs`.
    MpdOutputs(MpdOutputsResponse),

    // ── MPD library / browse responses ────────────────────────────────────────
    /// Response to `MpdGetQueue` — full queue snapshot.
    MpdGetQueue(MpdGetQueueResponse),
    /// Response to `MpdList` — one of `artists`, `albums`, or `songs` is populated.
    MpdList(MpdListResponse),
    /// Response to `MpdBrowse` — directory listing.
    MpdBrowse(MpdBrowseResponse),
    /// Response to `MpdGetPlaylists` — saved playlist names.
    MpdGetPlaylists(MpdGetPlaylistsResponse),
    /// Response to `MpdGetPlaylist` — tracks in a saved playlist.
    MpdGetPlaylist(MpdGetPlaylistResponse),
    /// Response to `MpdSearch` — search results (artists, albums, tracks) + optional error.
    MpdSearch(MpdSearchResult),

    /// Response to `LastfmAlbumTracks` — tracklist for a lastfm album.
    LastfmAlbumTracks(LastfmAlbumTracksResponse),

    /// Response to `MetadataPluginsForKind` — priority + disabled +
    /// discovered plugins for the kind, used by the settings UI.
    MetadataPluginsForKind(MetadataPluginsForKindResponse),

    /// Response to `GetPluginRepos`.
    PluginRepos(PluginReposResponse),
    /// Response to `BrowseRegistry` — full merged index from all repos.
    RegistryIndex(RegistryIndexResponse),
    /// Response to `InstallPlugin` — installation result.
    PluginInstalled(PluginInstalledResponse),
    /// Response to `RankStreams` — ranked streams with explanations.
    RankStreams(RankStreamsResponse),

    // ── Watch history responses ─────────────────────────────────────────────────
    /// Response to `GetWatchHistoryEntry`.
    WatchHistoryEntry(WatchHistoryEntryResponse),
    /// Response to `GetWatchHistoryInProgress`.
    WatchHistoryInProgress(WatchHistoryInProgressResponse),
    /// Response to `UpsertWatchHistoryEntry`.
    WatchHistoryUpsert(WatchHistoryUpsertResponse),
    /// Response to `UpdateWatchHistoryPosition`.
    WatchHistoryPositionUpdate(WatchHistoryPositionUpdateResponse),
    /// Response to `MarkWatchHistoryCompleted`.
    WatchHistoryCompleted(WatchHistoryUpsertResponse),
    /// Response to `RemoveWatchHistoryEntry`.
    WatchHistoryRemoved(WatchHistoryRemoveResponse),

    // ── Media cache responses ──────────────────────────────────────────────────
    /// Response to `GetMediaCacheTab`.
    MediaCacheTab(MediaCacheTabResponse),
    /// Response to `GetMediaCacheAll`.
    MediaCacheAll(MediaCacheAllResponse),
    /// Response to `GetMediaCacheStats`.
    MediaCacheStats(MediaCacheStatsResponse),
    /// Response to `ClearMediaCache`.
    MediaCacheCleared(MediaCacheClearResponse),

    // ── Stream policy responses ───────────────────────────────────────────────
    /// Response to `GetStreamPolicy`.
    StreamPolicy(StreamPolicyResponse),
    /// Acknowledgement for `SetStreamPolicy`.
    StreamPolicyUpdated,

    // ── Storage paths responses ──────────────────────────────────────────────
    /// Response to `GetStoragePaths`.
    StoragePaths(StoragePathsResponse),
    /// Response to `SetStoragePaths`.
    StoragePathsUpdated {
        success: bool,
    },

    // ── DSP responses ─────────────────────────────────────────────────────────
    /// Response to `GetDspStatus`.
    DspStatus(DspStatusResponse),
    /// Response to `SetDspConfig`.
    DspConfigUpdated {
        success: bool,
    },
    /// Response to `LoadConvolutionFilter`.
    ConvolutionFilterLoaded {
        success: bool,
    },
    /// Response to `BindDspToMpd`.
    DspBoundToMpd {
        success: bool,
        config: String,
    },
    /// Response to `ListDspProfiles`.
    DspProfilesListed {
        profiles: Vec<String>,
    },
    /// Response to `SaveDspProfile`.
    DspProfileSaved {
        success: bool,
    },
    /// Response to `LoadDspProfile`.
    DspProfileLoaded {
        success: bool,
    },
    /// Response to `DeleteDspProfile`.
    DspProfileDeleted {
        success: bool,
    },

    // ── Tag normalization responses ──────────────────────────────────────────
    GetAlbumArt(GetAlbumArtResponse),
    MarkTagException(MarkTagExceptionResponse),
    ActionATagsPreview(ActionATagsPreviewResponse),
    ActionATagsApply(ActionATagsApplyResponse),
    ActionATagsCancel(ActionATagsCancelResponse),

    // ── Plugin verb responses ────────────────────────────────────────────────
    /// Response to `Lookup`.
    Lookup(LookupIpcResponse),
    /// Response to `Enrich`.
    Enrich(EnrichIpcResponse),
    /// Response to `GetArtwork`.
    GetArtwork(ArtworkIpcResponse),
    /// Response to `GetCredits`.
    GetCredits(CreditsIpcResponse),
    /// Response to `Related`.
    Related(RelatedIpcResponse),

    /// One per-verb partial streamed back to the TUI during a
    /// `GetDetailMetadata` fan-out.  Multiple partials arrive per request
    /// (one per completed verb, out-of-order).
    DetailMetadataPartial(DetailMetadataPartial),

    /// Response to a `Metadata { kind = "episodes" }` request.  Carries a
    /// flat list of episodes for the requested season; the TUI's
    /// `EpisodeScreen` decodes the `episodes` array directly.
    EpisodesLoaded(EpisodesLoadedResponse),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    pub id: String,
    pub items: Vec<MediaEntry>,
    pub total: usize,
    pub offset: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveResponse {
    pub id: String,
    pub stream_url: String,
    pub quality: Option<String>,
    pub subtitles: Vec<SubtitleTrack>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamsResponse {
    pub id: String,
    pub entry_id: String,
    pub streams: Vec<StreamInfoWire>,
}

/// One provider's contribution to an in-flight `get_streams` request.
///
/// Streamed unsolicited (no `id` correlation) per provider as soon as
/// it returns. The TUI matches by `(season, episode)` and appends to
/// its per-episode streams cache. Each partial carries the provider
/// label so the TUI can show "from Torrentio" / "from Jackett" when
/// rendering. Late-arriving providers (e.g. Jackett's 25 s Torznab
/// fan-out) keep contributing even after fast providers have
/// already populated the list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamsPartialWire {
    pub entry_id: String,
    /// 1-based season number, 0 if N/A (movies).
    pub season: u32,
    /// 1-based episode number, 0 if N/A (movies).
    pub episode: u32,
    /// Plugin id of the provider this batch came from
    /// (`torrentio-provider`, `jackett-provider`, …). Mostly for
    /// diagnostics — the user-visible per-stream provider label is
    /// inside each `StreamInfoWire.provider`.
    pub provider: String,
    pub streams: Vec<StreamInfoWire>,
}

/// Sent once after all providers have either returned partials or hit
/// the deadline. Marks the end of the streaming phase so the TUI can
/// clear the in-flight spinner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamsCompleteWire {
    pub entry_id: String,
    pub season: u32,
    pub episode: u32,
    /// Optional last-resort error string when no provider returned
    /// any results. The TUI shows this in the streams column instead
    /// of the "no streams found" placeholder.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataResponse {
    pub id: String,
    pub entry: MediaEntry,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginListResponse {
    pub plugins: Vec<PluginInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginLoadedResponse {
    pub plugin_id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginUnloadedResponse {
    pub plugin_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetPluginEnabledRequest {
    pub plugin_id: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginEnabledResponse {
    pub plugin_id: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub id: Option<String>,
    pub code: ErrorCode,
    pub message: String,
}

/// One configurable field for a provider (e.g. an API key slot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderField {
    /// Config key path, e.g. `"plugins.tmdb.api_key"`.
    pub key: String,
    /// Human-readable label shown in the TUI.
    pub label: String,
    /// Short hint shown below the input field.
    pub hint: String,
    /// Whether to mask the value (passwords / API keys).
    pub masked: bool,
    /// Whether a non-empty value is currently configured.
    pub configured: bool,
    /// Whether this field is required for the plugin to work.
    #[serde(default)]
    pub required: bool,
    /// Current value (masked in TUI if masked=true).
    #[serde(default)]
    pub value: String,
}

/// Configuration schema for one provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSchema {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Plugin type: "metadata", "stream", "subtitle", etc.
    pub plugin_type: String,
    /// Provider is active (enabled and fully configured).
    pub active: bool,
    pub fields: Vec<ProviderField>,
}

/// Response payload for `GetProviderSettings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSettingsResponse {
    pub providers: Vec<ProviderSchema>,
}

/// A single MPD audio output device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdOutputInfo {
    pub id: u32,
    pub name: String,
    pub plugin: String,
    pub enabled: bool,
}

/// Response to `GetMpdOutputs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdOutputsResponse {
    pub outputs: Vec<MpdOutputInfo>,
}

// ── MPD library / browse — requests ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdGetQueueRequest {
    pub id: String,
}

/// `what` is one of `"artists"`, `"albums"`, `"songs"`.  `artist` is required
/// when `what == "albums"` or `what == "songs"`; `album` is required when
/// `what == "songs"`.  Empty string means "no filter".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdListRequest {
    pub id: String,
    pub what: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    /// Raw MPD `Date:` value used to disambiguate multiple releases of the
    /// same album (e.g. a 1996 original vs a 2007 remaster sharing the
    /// Album/Artist tags). Empty means no date filter.
    #[serde(default)]
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdBrowseRequest {
    pub id: String,
    /// Relative path inside the MPD music directory.  Empty = root.
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdGetPlaylistsRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdGetPlaylistRequest {
    pub id: String,
    pub name: String,
}

// ── MPD library / browse — wire entities ─────────────────────────────────────

/// One track in the MPD playback queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdQueueTrackWire {
    pub id: u32,
    pub pos: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub file: String,
}

/// One artist in the MPD library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdArtistWire {
    pub name: String,
}

/// One album in the MPD library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdAlbumWire {
    pub title: String,
    pub artist: String,
    /// Release year as a string (MPD returns `Date:` which may be full date or year).
    pub year: String,
    /// Raw MPD `Date:` value (e.g. "1996-11-01"), kept so the TUI can echo
    /// it back when asking for this specific release's tracks. Empty if the
    /// album has no Date tag.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    /// Pre-normalized artist value, populated only when normalization changed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_artist: String,
    /// Pre-normalized album title, populated only when normalization changed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_title: String,
}

/// One song record (used for library tracks and saved-playlist tracks).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdSongWire {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_artist: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_album: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_title: String,
}

/// One entry returned by `lsinfo` — either a directory, a file, or a playlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdDirEntryWire {
    pub name: String,
    pub is_dir: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub album: String,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub duration: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_artist: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_album: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_title: String,
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

/// A saved MPD playlist descriptor (name + last-modified timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdSavedPlaylistWire {
    pub name: String,
    /// ISO-8601 timestamp as returned by MPD; empty when unknown.
    #[serde(default)]
    pub modified: String,
}

// ── MPD library / browse — responses ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdGetQueueResponse {
    pub id: String,
    pub tracks: Vec<MpdQueueTrackWire>,
}

/// Request for a lastfm album's tracklist via album.getInfo.
/// (artist, album) is sufficient — lastfm's API resolves to a unique
/// release. `id` is the request correlation id used to route the
/// response back to the right pending caller in the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastfmAlbumTracksRequest {
    pub id: String,
    pub artist: String,
    pub album: String,
}

/// Response carrying the tracklist for a lastfm album. `tracks`
/// preserves the order returned by last.fm (which is the album's
/// canonical ordering). Empty when the album wasn't found or the
/// API returned no `tracks.track[]` block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastfmAlbumTracksResponse {
    pub id: String,
    pub artist: String,
    pub album: String,
    pub tracks: Vec<LastfmAlbumTrackWire>,
}

/// One track in a lastfm album response. `number` is 1-based.
/// `duration_secs` is None when last.fm doesn't have a duration
/// (some sparse releases). `mbid` is the recording's MusicBrainz id
/// when available — useful for downstream lookups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastfmAlbumTrackWire {
    pub number: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mbid: Option<String>,
}

/// Request for the metadata-source plugins that contribute to a kind's
/// detail-card fan-out. Drives the Settings → Metadata Sources screen.
/// `kind` is the lowercase TUI tab label: "movies" / "series" /
/// "anime" / "music". `id` correlates the response back to the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataPluginsForKindRequest {
    pub id: String,
    pub kind: String,
}

/// Response describing every plugin that the runtime would route a
/// detail-metadata request to for `kind`. Three lists, mutually
/// disjoint after the dedupe step the runtime applies:
///
///   - `priority`: plugin names from the user's configured priority
///     list, in the order they'll be consulted.
///   - `discovered`: plugin names auto-discovered via manifest tags
///     (`tags = ["<kind>"]`) that aren't already in the priority list.
///   - `disabled`: plugin names the user has explicitly excluded.
///
/// The TUI renders these as a single editable list with status chips
/// per row (priority N / discovered / disabled) and a toggle key that
/// edits the disabled list via `set_config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataPluginsForKindResponse {
    pub id: String,
    pub kind: String,
    pub priority: Vec<String>,
    pub discovered: Vec<String>,
    pub disabled: Vec<String>,
}

/// Exactly one of `artists`, `albums`, `songs` is non-empty per response —
/// matches the `what` in the originating `MpdListRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdListResponse {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<MpdArtistWire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub albums: Vec<MpdAlbumWire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub songs: Vec<MpdSongWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdBrowseResponse {
    pub id: String,
    pub entries: Vec<MpdDirEntryWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdGetPlaylistsResponse {
    pub id: String,
    pub playlists: Vec<MpdSavedPlaylistWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdGetPlaylistResponse {
    pub id: String,
    pub tracks: Vec<MpdSongWire>,
}

// ── MPD search ───────────────────────────────────────────────────────────────

/// Which MPD entity types to search.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MpdScope {
    Artist,
    Album,
    Track,
}

/// Request to search the MPD library by artist, album, or track.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MpdSearchRequest {
    pub id: String,
    pub query: String,
    pub scopes: Vec<MpdScope>,
    pub limit: u32,
    pub query_id: u64,
}

/// Result buckets from an MPD search.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MpdSearchResult {
    pub id: String,
    pub query_id: u64,
    pub artists: Vec<MpdArtistWire>,
    pub albums: Vec<MpdAlbumWire>,
    pub tracks: Vec<MpdSongWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<MpdSearchError>,
}

/// Error variants for MPD search.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MpdSearchError {
    NotConnected,
    CommandFailed { message: String },
}

// ── Tag normalization — requests ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAlbumArtRequest {
    pub id: String,
    /// Relative path to the audio file within the MPD music directory.
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAlbumArtResponse {
    pub id: String,
    /// Absolute path to the cached image file, or empty if no art found.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkTagExceptionRequest {
    pub id: String,
    pub field: String, // "artist" | "album_artist" | "album" | "title" | "genre"
    pub raw_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsPreviewRequest {
    pub id: String,
    pub scope: TagWriteScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TagWriteScope {
    Album {
        artist: String,
        album: String,
        date: String,
    },
    Artist {
        artist: String,
    },
    Library,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsApplyRequest {
    pub id: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsCancelRequest {
    pub id: String,
    pub job_id: String,
}

// ── Tag normalization — responses ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkTagExceptionResponse {
    pub id: String,
    pub added: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagDiffRowWire {
    pub file: String,
    pub field: String,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsPreviewResponse {
    pub id: String,
    pub job_id: String,
    pub rows: Vec<TagDiffRowWire>,
    pub total_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsApplyResponse {
    pub id: String,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_cancelled: usize,
    pub failures: Vec<String>,
    pub rescan_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsCancelResponse {
    pub id: String,
    pub cancelled: bool,
}

// ── Plugin verb IPC response structs ─────────────────────────────────────────

/// Response to a `Lookup` IPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupIpcResponse {
    /// Correlation ID echoing the originating `LookupIpcRequest::query_id`.
    pub query_id: u64,
    pub entry: crate::abi::types::PluginEntry,
}

/// Response to an `Enrich` IPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichIpcResponse {
    /// Correlation ID echoing the originating `EnrichIpcRequest::query_id`.
    pub query_id: u64,
    pub entry: crate::abi::types::PluginEntry,
}

/// Response to a `GetArtwork` IPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkIpcResponse {
    /// Correlation ID echoing the originating `ArtworkIpcRequest::query_id`.
    pub query_id: u64,
    #[serde(flatten)]
    pub inner: crate::abi::types::ArtworkResponse,
}

/// Response to a `GetCredits` IPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsIpcResponse {
    /// Correlation ID echoing the originating `CreditsIpcRequest::query_id`.
    pub query_id: u64,
    #[serde(flatten)]
    pub inner: crate::abi::types::CreditsResponse,
}

/// Response to a `Related` IPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedIpcResponse {
    /// Correlation ID echoing the originating `RelatedIpcRequest::query_id`.
    pub query_id: u64,
    pub items: Vec<crate::abi::types::PluginEntry>,
}

/// Wire shape for a single episode. Mirrors the TUI's `ipc.EpisodeEntry`
/// (and `sdk::EpisodeWire`) field-for-field — Go side decodes by JSON tag
/// so no Rust-level type alias on the TUI is needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeEntryWire {
    pub season: u32,
    pub episode: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mins: Option<u32>,
    pub provider: String,
    pub entry_id: String,
}

/// Response payload for `Response::EpisodesLoaded`. The `id` field is
/// echoed by `inject_id_into_response` only if missing, but we set it
/// explicitly so the TUI's pending-id router always finds a match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodesLoadedResponse {
    pub id: String,
    pub episodes: Vec<EpisodeEntryWire>,
}

impl From<crate::abi::types::EpisodeWire> for EpisodeEntryWire {
    fn from(e: crate::abi::types::EpisodeWire) -> Self {
        EpisodeEntryWire {
            season: e.season,
            episode: e.episode,
            title: e.title,
            air_date: e.air_date,
            runtime_mins: e.runtime_mins,
            provider: e.provider,
            entry_id: e.entry_id,
        }
    }
}

/// Runtime-native TVDB episodes flow through this conversion. WASM
/// plugins build `abi::EpisodeWire` themselves with their own title
/// fallback, but `TvdbEpisode` keeps `title: Option<String>` upstream so
/// the fallback lives at the IPC boundary — applied here once.
impl From<crate::tvdb::TvdbEpisode> for EpisodeEntryWire {
    fn from(e: crate::tvdb::TvdbEpisode) -> Self {
        let n = e.episode;
        let title = e
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("Episode {n}"));
        EpisodeEntryWire {
            season: e.season,
            episode: n,
            title,
            air_date: e.air_date,
            runtime_mins: e.runtime_mins,
            provider: "tvdb".to_string(),
            entry_id: format!("tvdb-{}", e.id),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetPluginReposRequest {
    pub repos: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginReposResponse {
    pub repos: Vec<String>,
}

/// Request payload for `InstallPlugin`.
#[derive(Debug, Serialize, Deserialize)]
pub struct InstallPluginRequest {
    /// Exact plugin name from the registry entry.
    pub name: String,
    /// Semver version string from the registry entry.
    pub version: String,
    /// Download URL for the plugin bundle.
    pub binary_url: String,
    /// SHA-256 checksum in the form `"sha256:<hex>"`.
    pub checksum: String,
}

/// A single entry in the registry index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntryWire {
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub description: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub binary_url: String,
    pub checksum: String,
    /// Whether this plugin is already installed (matching name in plugin_dir).
    pub installed: bool,
}

/// Response to `BrowseRegistry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndexResponse {
    pub entries: Vec<RegistryEntryWire>,
    /// Repo URLs that failed to fetch (for user-visible warning).
    pub failed_repos: Vec<String>,
}

/// Response to `InstallPlugin`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstalledResponse {
    pub name: String,
    pub version: String,
    /// Installed directory path.
    pub path: String,
}

// ── Out-of-band events (Rust → Go, pushed asynchronously) ────────────────────

/// Pushed whenever the catalog grid changes (cache hit, live refresh, search).
#[derive(Debug, Serialize, Deserialize)]
pub struct GridUpdateEvent {
    pub tab: String,
    pub entries: Vec<MediaEntry>,
    /// `"cache"` | `"live"` | `"search"`
    pub source: String,
}

impl GridUpdateEvent {
    /// Serialize to a newline-terminated JSON line for stdout.
    pub fn to_wire(&self) -> anyhow::Result<String> {
        let mut map = serde_json::Map::new();
        map.insert(
            "type".to_string(),
            serde_json::Value::String("grid_update".to_string()),
        );
        map.insert(
            "tab".to_string(),
            serde_json::Value::String(self.tab.clone()),
        );
        map.insert("entries".to_string(), serde_json::to_value(&self.entries)?);
        map.insert(
            "source".to_string(),
            serde_json::Value::String(self.source.clone()),
        );
        let mut s = serde_json::to_string(&serde_json::Value::Object(map))?;
        s.push('\n');
        Ok(s)
    }
}

/// Backward-compat alias for `GridUpdateEvent`.
pub type GridUpdateMsg = GridUpdateEvent;

/// Pushed when the catalog attempted to refresh a tab but the provider
/// fan-out returned zero entries (every provider errored, or no provider
/// could respond). Signals "offline / network unreachable" to the TUI so
/// it can flag the grid as stale. Entries already on screen continue to
/// display from the disk grid cache — the runtime just doesn't overwrite
/// them with an empty result.
#[derive(Debug, Serialize, Deserialize)]
pub struct CatalogStaleEvent {
    pub tab: String,
    /// Short human-readable description of why the refresh produced no
    /// entries. E.g. "no active providers", "all providers errored",
    /// "network offline". Meant for status-bar display; not an error code.
    pub reason: String,
}

impl CatalogStaleEvent {
    pub fn to_wire(&self) -> anyhow::Result<String> {
        let mut map = serde_json::Map::new();
        map.insert(
            "type".to_string(),
            serde_json::Value::String("catalog_stale".to_string()),
        );
        map.insert(
            "tab".to_string(),
            serde_json::Value::String(self.tab.clone()),
        );
        map.insert(
            "reason".to_string(),
            serde_json::Value::String(self.reason.clone()),
        );
        let mut s = serde_json::to_string(&serde_json::Value::Object(map))?;
        s.push('\n');
        Ok(s)
    }
}

pub type CatalogStaleMsg = CatalogStaleEvent;

/// Cached enriched detail entry — alias for `MetadataResponse`.
pub type DetailEntry = MetadataResponse;

/// Pushed when a plugin is hot-loaded or hot-unloaded.
#[derive(Debug, Serialize, Deserialize)]
pub struct PluginToastEvent {
    pub plugin_name: String,
    pub version: String,
    pub plugin_type: String,
    pub message: String,
}

/// Pushed when mpv starts playing a file.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerStartedEvent {
    pub title: String,
    pub path: String,
    pub duration: f64,
}

/// Pushed ~once per second during playback.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerProgressEvent {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub cache_percent: f64,
}

/// Pushed when mpv exits.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerEndedEvent {
    /// `"eof"` | `"quit"` | `"error"`
    pub reason: String,
    #[serde(default)]
    pub error: String,
}

// ── Domain types ──────────────────────────────────────────────────────────────

/// Fine-grained media classification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    #[default]
    Movie,
    Series,
    Episode,
    Music,
    Album,
    Track,
    Unknown,
}

impl MediaType {
    pub fn from_tab(tab: &MediaTab) -> Self {
        match tab {
            MediaTab::Movies => MediaType::Movie,
            MediaTab::Series => MediaType::Series,
            MediaTab::Music => MediaType::Music,
            MediaTab::Library => MediaType::Unknown,
            MediaTab::Radio => MediaType::Unknown,
            MediaTab::Podcasts => MediaType::Unknown,
            MediaTab::Videos => MediaType::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            MediaType::Movie => "Movie",
            MediaType::Series => "Series",
            MediaType::Episode => "Episode",
            MediaType::Music => "Music",
            MediaType::Album => "Album",
            MediaType::Track => "Track",
            MediaType::Unknown => "",
        }
    }
}

/// Top-level UI tab (coarse navigation category).
///
/// Tabs map 1:1 to `MediaSource` variants for provider routing.
/// New tabs can be added here; providers declare support via `supported_sources()`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum MediaTab {
    #[default]
    Movies,
    Series,
    Music,
    Library,
    /// Internet radio stations (Icecast, Shoutcast, SomaFM, etc.)
    Radio,
    /// Podcast episodes and feeds.
    Podcasts,
    /// Online video (YouTube, PeerTube, Odysee, etc.)
    Videos,
}

impl MediaTab {
    /// Human-readable label shown in the TUI tab bar.
    pub fn label(&self) -> &'static str {
        match self {
            MediaTab::Movies => "Movies",
            MediaTab::Series => "Series",
            MediaTab::Music => "Music",
            MediaTab::Library => "Library",
            MediaTab::Radio => "Radio",
            MediaTab::Podcasts => "Podcasts",
            MediaTab::Videos => "Videos",
        }
    }

    /// The tabs shown in the main navigation bar by default.
    pub fn default_tabs() -> &'static [MediaTab] {
        &[
            MediaTab::Movies,
            MediaTab::Series,
            MediaTab::Music,
            MediaTab::Library,
        ]
    }
}

/// A catalog item as returned by search or trending.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaEntry {
    pub id: String,
    pub title: String,
    pub year: Option<String>,
    pub genre: Option<String>,
    /// Weighted composite rating string (e.g. "8.3").
    pub rating: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub provider: String,
    pub tab: MediaTab,
    /// Grid-layer tab classifier (`Movies`, `Series`, etc.).
    /// See note on `kind` below for how these two fields coexist.
    #[serde(default)]
    pub media_type: MediaType,
    /// Per-source raw scores forwarded to the TUI for detail display.
    #[serde(default)]
    pub ratings: std::collections::HashMap<String, f64>,
    #[serde(default)]
    pub imdb_id: Option<String>,
    #[serde(default)]
    pub tmdb_id: Option<String>,
    #[serde(default)]
    pub mal_id: Option<String>,
    /// AniList catalog id (the integer behind the `anilist-N` provider
    /// prefix). Populated from `external_ids["anilist"]` at the
    /// MediaEntry conversion sites and consumed by the anime-bridge
    /// enrichment as a lookup key — without it, kitsu-only catalog
    /// entries (no MAL mapping) couldn't resolve to a Fribb record
    /// and stayed at title:year dedup, missing the spine-merge bucket.
    #[serde(default)]
    pub anilist_id: Option<String>,
    /// Kitsu catalog id; same role as `anilist_id` for the Kitsu side
    /// of the bridge.
    #[serde(default)]
    pub kitsu_id: Option<String>,
    /// ISO 639-1 code of the entry's original language (e.g. "ja", "en").
    /// Populated by plugins that know it (tmdb, kitsu, anilist). The runtime's
    /// anime-mix classifier uses this together with genre to identify
    /// Japanese animation shipped by mainstream providers.
    #[serde(default)]
    pub original_language: Option<String>,
    // ── Fields added in Task 2.3 ────────────────────────────────────────────
    // `#[serde(default)]` keeps old wire payloads (without these fields) valid.
    /// Typed entry kind for scoped search results (`Artist`, `Album`, `Track`,
    /// `Movie`, `Series`, `Episode`).  This field and `media_type` coexist
    /// intentionally:
    ///
    /// - `kind` is set by the scoped-search path and distinguishes
    ///   Artist/Album/Track within a single search response.
    /// - `media_type` is the tab classifier consumed by the grid layer
    ///   (`Movies`, `Series`, etc.) and predates scoped search.
    ///
    /// Consolidating the two into a single field is tracked as Task 7.0
    /// deferral #6 and will happen once plugin migration is complete.
    ///
    /// Defaults to `EntryKind::Track` for backward-compat with legacy wire data.
    #[serde(default)]
    pub kind: EntryKind,
    /// Originating plugin / provider identifier for scoped-search entries.
    /// Parallel to `provider`; migration to a single field happens in a later task.
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    /// For series entries: total seasons reported by the provider's
    /// lookup. The TUI uses this to populate its episode browser's
    /// season list. `None` (e.g. catalog entries that haven't been
    /// looked up) keeps the TUI on its single-season default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_count: Option<u32>,
    /// True when the provider has a Specials track (e.g. TVDB season 0).
    /// The TUI appends a "Specials" row to its season list when set.
    #[serde(default)]
    pub has_specials: bool,
}

/// A single stream candidate as sent to the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfoWire {
    pub url: String,
    pub name: String,
    pub quality: String,
    pub provider: String,
    pub score: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub hdr: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeders: Option<u32>,
    /// Total file size in bytes (populated when the provider reports it).
    /// Two streams at the same `quality` label can differ wildly in
    /// encoding/bitrate — surfacing size lets the TUI distinguish them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Measured download speed in Mbps (populated when benchmarking is enabled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mbps: Option<f64>,
    /// Measured latency in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
}

/// A subtitle track (language + URL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub language: String,
    pub url: String,
    /// `"srt"` | `"vtt"` | `"ass"`
    pub format: String,
}

/// Plugin metadata as reported to the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub status: PluginStatus,
    /// Mirror of `status == Loaded` but kept as an explicit bool so the
    /// TUI doesn't need to know the full status enum to decide whether
    /// to show the Enable vs Disable action. Stays in sync with
    /// `status` inside `list_plugins`.
    #[serde(default = "pluginfo_default_enabled")]
    pub enabled: bool,
    /// Tags for organizing plugins (e.g., "movies", "music", "anime", "tv", "subtitles")
    #[serde(default)]
    pub tags: Vec<String>,
    /// One-line description from plugin.toml [plugin] description field.
    #[serde(default)]
    pub description: String,
    /// Author from plugin.toml [meta] author field.
    #[serde(default)]
    pub author: String,
}

fn pluginfo_default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Loaded,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    PluginNotFound,
    PluginLoadFailed,
    SearchFailed,
    ResolveFailed,
    MetadataFailed,
    InvalidRequest,
    Internal,
}

/// Request to rank streams according to a user policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankStreamsRequest {
    /// Streams to rank.
    pub streams: Vec<StreamInfoWire>,
    /// User preferences for ranking.
    pub preferences: StreamPreferencesWire,
}

/// User preferences for stream ranking (subset of RankingPolicy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPreferencesWire {
    #[serde(default)]
    pub prefer_protocol: Option<String>,
    #[serde(default)]
    pub max_resolution: Option<String>,
    #[serde(default)]
    pub max_size_mb: u64,
    #[serde(default)]
    pub min_seeders: u32,
    #[serde(default)]
    pub avoid_labels: Vec<String>,
    #[serde(default)]
    pub prefer_hdr: bool,
    #[serde(default)]
    pub prefer_codecs: Vec<String>,
}

/// Response containing ranked streams with scores and explanations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankStreamsResponse {
    pub ranked: Vec<RankedStreamWire>,
}

/// A ranked stream with its score and human-readable explanations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedStreamWire {
    pub stream: StreamInfoWire,
    pub score: i64,
    pub reasons: Vec<String>,
}

// ── Stream policy types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct SetStreamPolicyRequest {
    pub policy: StreamPreferencesWire,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StreamPolicyResponse {
    pub policy: StreamPreferencesWire,
}

// ── Watch history types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryEntryWire {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub year: Option<String>,
    pub tab: String,
    pub provider: String,
    #[serde(default)]
    pub imdb_id: Option<String>,
    #[serde(default)]
    pub position: f64,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub completed: bool,
    pub last_watched: i64,
    #[serde(default)]
    pub season: u32,
    #[serde(default)]
    pub episode: u32,
    #[serde(default)]
    pub file_path: Option<String>,
}

impl From<crate::watchhistory::WatchHistoryEntry> for WatchHistoryEntryWire {
    fn from(e: crate::watchhistory::WatchHistoryEntry) -> Self {
        Self {
            id: e.id,
            title: e.title,
            year: e.year,
            tab: e.tab,
            provider: e.provider,
            imdb_id: e.imdb_id,
            position: e.position,
            duration: e.duration,
            completed: e.completed,
            last_watched: e.last_watched,
            season: e.season,
            episode: e.episode,
            file_path: e.file_path,
        }
    }
}

impl From<WatchHistoryEntryWire> for crate::watchhistory::WatchHistoryEntry {
    fn from(e: WatchHistoryEntryWire) -> Self {
        Self {
            id: e.id,
            title: e.title,
            year: e.year,
            tab: e.tab,
            provider: e.provider,
            imdb_id: e.imdb_id,
            position: e.position,
            duration: e.duration,
            completed: e.completed,
            last_watched: e.last_watched,
            season: e.season,
            episode: e.episode,
            file_path: e.file_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWatchHistoryEntryRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWatchHistoryInProgressRequest {
    pub tab: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertWatchHistoryEntryRequest {
    pub entry: WatchHistoryEntryWire,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWatchHistoryPositionRequest {
    pub id: String,
    pub position: f64,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkWatchHistoryCompletedRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveWatchHistoryEntryRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryEntryResponse {
    pub entry: Option<WatchHistoryEntryWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryInProgressResponse {
    pub entries: Vec<WatchHistoryEntryWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryUpsertResponse {
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryPositionUpdateResponse {
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryRemoveResponse {
    pub success: bool,
}

// ── Media cache types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMediaCacheTabRequest {
    pub tab: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMediaCacheAllRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMediaCacheStatsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearMediaCacheRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaCacheTabResponse {
    pub tab: String,
    pub entries: Vec<MediaEntry>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaCacheAllResponse {
    pub entries: Vec<MediaEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaCacheStatsResponse {
    pub total_count: usize,
    pub last_updated: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaCacheClearResponse {
    pub success: bool,
}

// ── Storage paths types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStoragePathsRequest {
    pub movies: Option<String>,
    pub series: Option<String>,
    pub music: Option<String>,
    pub anime: Option<String>,
    pub podcasts: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePathsResponse {
    pub movies: String,
    pub series: String,
    pub music: String,
    pub anime: String,
    pub podcasts: String,
}

// ── DSP types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDspConfigRequest {
    pub enabled: Option<bool>,
    pub output_sample_rate: Option<u32>,
    pub upsample_ratio: Option<u32>,
    pub filter_type: Option<String>,
    pub resample_enabled: Option<bool>,
    pub dsd_to_pcm_enabled: Option<bool>,
    pub output_mode: Option<String>,
    pub convolution_enabled: Option<bool>,
    pub convolution_bypass: Option<bool>,
    pub buffer_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadConvolutionFilterRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DspStatusResponse {
    pub enabled: bool,
    pub output_sample_rate: u32,
    pub resample_enabled: bool,
    pub dsd_to_pcm_enabled: bool,
    pub convolution_enabled: bool,
    pub convolution_bypass: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveDspProfileRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadDspProfileRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteDspProfileRequest {
    pub name: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

impl Response {
    /// Serialize to a newline-terminated JSON line for stdout.
    pub fn to_wire(&self) -> anyhow::Result<String> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }

    pub fn error(id: Option<String>, code: ErrorCode, msg: impl Into<String>) -> Self {
        Response::Error(ErrorResponse {
            id,
            code,
            message: sanitize_secrets(&msg.into()),
        })
    }
}

/// Replaces secret-bearing query/form params with `<key>=***` so error
/// messages can flow to logs and the TUI without leaking credentials.
/// Triggered by every `Response::error()` call so all error wires get
/// scrubbed at a single chokepoint.
///
/// Recognised keys are scanned case-insensitively. The value runs from
/// the `=` to the next delimiter (`&` / whitespace / quote / `)` / `\n`)
/// or end of string.
pub fn sanitize_secrets(msg: &str) -> String {
    const KEYS: &[&str] = &["api_key", "apikey", "access_token", "token"];
    let mut out = String::with_capacity(msg.len());
    let bytes = msg.as_bytes();
    let lower = msg.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Try to match each key prefix at position i. We require a
        // word-boundary char before the key (or start-of-string) so
        // `apikey=` doesn't false-match inside `notapikey=...`.
        let boundary_ok =
            i == 0 || !matches!(bytes[i - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_');
        let mut matched_key: Option<&str> = None;
        if boundary_ok {
            for k in KEYS {
                let kb = k.as_bytes();
                let end = i + kb.len();
                if end < bytes.len() && bytes[end] == b'=' && lower_bytes[i..end] == *kb {
                    matched_key = Some(k);
                    break;
                }
            }
        }
        if let Some(k) = matched_key {
            out.push_str(&msg[i..i + k.len() + 1]); // include `key=`
            i += k.len() + 1;
            out.push_str("***");
            while i < bytes.len()
                && !matches!(bytes[i], b'&' | b' ' | b'"' | b'\'' | b')' | b'\n' | b'\r')
            {
                i += 1;
            }
        } else {
            // Walk one UTF-8 char so we don't slice mid-codepoint.
            let ch_len = msg[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&msg[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

#[cfg(test)]
mod sanitize_tests {
    use super::sanitize_secrets;

    #[test]
    fn scrubs_api_key() {
        let s = "https://api.example.com/x?api_key=abc123&y=1";
        assert_eq!(
            sanitize_secrets(s),
            "https://api.example.com/x?api_key=***&y=1"
        );
    }

    #[test]
    fn scrubs_token_at_end() {
        assert_eq!(sanitize_secrets("?token=deadbeef"), "?token=***");
    }

    #[test]
    fn case_insensitive_key_match() {
        assert_eq!(sanitize_secrets("?API_KEY=abc"), "?API_KEY=***");
    }

    #[test]
    fn does_not_match_within_word() {
        assert_eq!(sanitize_secrets("notapikey=visible"), "notapikey=visible");
    }

    #[test]
    fn passthrough_when_no_secret() {
        let s = "plain error: connection refused";
        assert_eq!(sanitize_secrets(s), s);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod search_request_tests {
    use super::*;
    use stui_plugin_sdk::{EntryKind, SearchScope};

    #[test]
    fn ipc_search_request_with_scopes_roundtrips() {
        let req = SearchRequest {
            id: "q1".into(),
            query: "creep".into(),
            scopes: vec![SearchScope::Artist, SearchScope::Track],
            limit: 50,
            offset: 0,
            query_id: 42,
        };
        let s = serde_json::to_vec(&req).unwrap();
        let back: SearchRequest = serde_json::from_slice(&s).unwrap();
        assert_eq!(back.scopes, vec![SearchScope::Artist, SearchScope::Track]);
        assert_eq!(back.query_id, 42);
    }

    #[test]
    fn scope_results_msg_has_all_fields() {
        let msg = ScopeResultsMsg {
            query_id: 42,
            scope: SearchScope::Artist,
            entries: vec![],
            partial: true,
            error: None,
        };
        let s = serde_json::to_string(&msg).unwrap();
        assert!(s.contains("\"partial\":true"));
        assert!(s.contains("\"scope\":\"artist\""));
    }

    #[test]
    fn scope_error_tagged_variants() {
        let e = ScopeError::NoPluginsConfigured;
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"type\":\"no_plugins_configured\""));
    }

    #[test]
    fn media_entry_extended_fields_default() {
        // With serde(default), a legacy wire payload (without new fields)
        // should still deserialize successfully.
        let legacy = r#"{
            "id": "x",
            "title": "t",
            "year": null,
            "genre": null,
            "rating": null,
            "description": null,
            "poster_url": null,
            "provider": "test",
            "tab": "movies"
        }"#;
        let entry: Result<MediaEntry, _> = serde_json::from_str(legacy);
        let entry = entry.expect("legacy MediaEntry JSON should deserialize");
        // New fields should default to their zero values.
        assert_eq!(entry.kind, EntryKind::Track);
        assert_eq!(entry.source, "");
        assert!(entry.artist_name.is_none());
        assert!(entry.album_name.is_none());
        assert!(entry.track_number.is_none());
        assert!(entry.season.is_none());
        assert!(entry.episode.is_none());
    }

    #[test]
    fn mpd_search_request_roundtrips() {
        let req = MpdSearchRequest {
            id: "q2".into(),
            query: "radiohead".into(),
            scopes: vec![MpdScope::Artist, MpdScope::Album, MpdScope::Track],
            limit: 200,
            query_id: 7,
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: MpdSearchRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.scopes.len(), 3);
        assert_eq!(back.query_id, 7);
    }

    #[test]
    fn mpd_search_result_has_typed_buckets() {
        let r = MpdSearchResult {
            id: "q2".into(),
            query_id: 7,
            artists: vec![],
            albums: vec![],
            tracks: vec![],
            error: Some(MpdSearchError::NotConnected),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"artists\":[]"));
        assert!(s.contains("\"type\":\"not_connected\""));
    }

    #[test]
    fn mpd_scope_snake_case() {
        assert_eq!(
            serde_json::to_string(&MpdScope::Artist).unwrap(),
            "\"artist\""
        );
        assert_eq!(
            serde_json::to_string(&MpdScope::Album).unwrap(),
            "\"album\""
        );
        assert_eq!(
            serde_json::to_string(&MpdScope::Track).unwrap(),
            "\"track\""
        );
    }
}

#[cfg(test)]
mod detail_metadata_tests {
    use super::*;

    #[test]
    fn get_detail_metadata_request_round_trips() {
        let req = Request::GetDetailMetadata(GetDetailMetadataRequest {
            entry_id: "tt1".into(),
            id_source: "imdb".into(),
            kind: "movies".into(),
            ..Default::default()
        });
        let s = serde_json::to_string(&req).unwrap();
        // Wire tag lives on the Request enum itself.
        assert!(s.contains("\"type\":\"get_detail_metadata\""));
        let back: Request = serde_json::from_str(&s).unwrap();
        match back {
            Request::GetDetailMetadata(r) => {
                assert_eq!(r.entry_id, "tt1");
                assert_eq!(r.id_source, "imdb");
                assert_eq!(r.kind, "movies");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn detail_metadata_partial_credits_round_trips() {
        let resp = Response::DetailMetadataPartial(DetailMetadataPartial {
            entry_id: "tt1".into(),
            verb: "credits".into(),
            payload: MetadataPayload::Empty,
        });
        let s = serde_json::to_string(&resp).unwrap();
        // Outer wire tag is the Response variant.
        assert!(s.contains("\"type\":\"detail_metadata_partial\""));
        let back: Response = serde_json::from_str(&s).unwrap();
        match back {
            Response::DetailMetadataPartial(p) => {
                assert_eq!(p.entry_id, "tt1");
                assert_eq!(p.verb, "credits");
                assert_eq!(p.payload, MetadataPayload::Empty);
            }
            _ => panic!("wrong variant"),
        }
    }
}
