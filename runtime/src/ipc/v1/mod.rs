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

use serde::{Deserialize, Serialize};

// ── Requests (Go → Rust) ─────────────────────────────────────────────────────

/// Every message sent from the TUI to the runtime.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Full-text search across active providers.
    Search(SearchRequest),
    /// Resolve a catalog entry into a stream URL (without playing).
    Resolve(ResolveRequest),
    /// Fetch all ranked stream candidates for a catalog entry.
    GetStreams(GetStreamsRequest),
    /// Fetch enriched metadata for a media entry.
    Metadata(MetadataRequest),
    /// Resolve + hand off to the player (aria2 → mpv, or direct mpv).
    Play(PlayRequest),
    /// Stop current playback; kills mpv and the active aria2 GID.
    PlayerStop,
    /// Send a raw mpv IPC command (e.g. `{"cmd":"cycle","args":["pause"]}`).
    PlayerCommand(PlayerCommandRequest),
    /// List all currently loaded plugins.
    ListPlugins,
    /// Dynamically load a plugin by filesystem path.
    LoadPlugin(LoadPluginRequest),
    /// Unload a loaded plugin by its ID.
    UnloadPlugin(UnloadPluginRequest),
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
    SetTrace {
        enabled: bool,
    },
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
}

/// Live-update a runtime config value without restarting.
#[derive(Debug, Deserialize, Serialize)]
pub struct SetConfigRequest {
    /// Dot-separated config key, e.g. `"player.default_volume"`.
    pub key: String,
    /// JSON-encoded new value (will be validated against the config schema).
    pub value: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchRequest {
    /// Correlation ID echoed back in the `SearchResponse`.
    pub id: String,
    pub query: String,
    pub tab: MediaTab,
    /// `None` = fan out to all loaded providers.
    pub provider: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
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
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MetadataRequest {
    pub id: String,
    pub entry_id: String,
    pub provider: String,
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

// ── Responses (Rust → Go, in-band) ───────────────────────────────────────────

/// Every in-band response sent from the runtime to the TUI.
/// Out-of-band events (player progress, grid updates) use their own structs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    SearchResult(SearchResponse),
    ResolveResult(ResolveResponse),
    StreamsResult(StreamsResponse),
    MetadataResult(MetadataResponse),
    PluginList(PluginListResponse),
    PluginLoaded(PluginLoadedResponse),
    PluginUnloaded(PluginUnloadedResponse),
    /// Response to `Ping`.  Always carries version metadata so the TUI can
    /// detect mismatches and warn the user.
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MediaTab {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default)]
    pub media_type: MediaType,
    /// Per-source raw scores forwarded to the TUI for detail display.
    #[serde(default)]
    pub ratings: std::collections::HashMap<String, f64>,
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
    /// Tags for organizing plugins (e.g., "movies", "music", "anime", "tv", "subtitles")
    #[serde(default)]
    pub tags: Vec<String>,
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
            message: msg.into(),
        })
    }
}
