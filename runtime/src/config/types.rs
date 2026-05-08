//! Configuration type definitions.
//!
#![allow(dead_code)]
//! These structs mirror the sections of `~/.stui/config/stui.toml`.
//! Every field has a `#[serde(default)]` so partial config files are fine.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

/// Plugin-specific config: plugin_name -> field_key -> value
pub type PluginConfig = HashMap<String, HashMap<String, String>>;

#[allow(unused_imports)]
use crate::dsp::DspConfig;

/// A string value that should be redacted from logs and config exports.
/// Used for API keys, passwords, and other sensitive data.
#[derive(Clone, Default)]
pub struct SecretString(Option<String>);

impl SecretString {
    pub fn new(value: Option<String>) -> Self {
        SecretString(value)
    }

    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }

    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.0.as_ref().map_or(true, |s| s.is_empty())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_some() {
            write!(f, "SecretString(<redacted>)")
        } else {
            write!(f, "SecretString(None)")
        }
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref s) = self.0 {
            let visible_chars = 4.min(s.chars().count() / 4);
            let masked_chars = s.chars().count() - visible_chars;
            let visible: String = s.chars().take(visible_chars).collect();
            let masked = "*".repeat(masked_chars);
            write!(f, "{}{}", visible, masked)
        } else {
            write!(f, "(not set)")
        }
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let opt = Option::<String>::deserialize(deserializer)?;
        Ok(SecretString(opt))
    }
}

/// Top-level runtime configuration, passed to `Engine::new()` and friends.
///
/// Constructed by `loader::load()` which merges:
///   1. Compiled-in defaults
///   2. `~/.stui/config/stui.toml` (if present)
///   3. Environment variable overrides (`STUI_*`)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Directory where plugins are discovered (`~/.stui/plugins`).
    #[serde(default = "defaults::plugin_dir")]
    pub plugin_dir: PathBuf,

    /// Directory for on-disk caches (`~/.stui/cache`).
    #[serde(default = "defaults::cache_dir")]
    pub cache_dir: PathBuf,

    /// Directory for persistent data (history, watch-later, etc.)
    #[serde(default = "defaults::data_dir")]
    pub data_dir: PathBuf,

    /// Theme palette variant: `"dark"` | `"light"`.
    #[serde(default = "defaults::theme_mode")]
    pub theme_mode: String,

    /// Logging configuration.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Stremio addon URLs (STUI_STREMIO_ADDONS env override also accepted).
    #[serde(default)]
    pub stremio_addons: Vec<String>,

    /// Playback configuration.
    #[serde(default)]
    pub playback: PlaybackConfig,

    /// Streaming strategy configuration.
    #[serde(default)]
    pub streaming: StreamingConfig,

    /// Subtitle configuration.
    #[serde(default)]
    pub subtitles: SubtitlesConfig,

    /// Provider enable/disable flags.
    #[serde(default)]
    pub providers: ProvidersConfig,

    /// Provider API keys (stored in config file, never via env).
    #[serde(default)]
    pub api_keys: ApiKeysConfig,

    /// MPD (Music Player Daemon) configuration.
    #[serde(default)]
    pub mpd: MpdConfig,

    /// Music tag normalization configuration.
    #[serde(default)]
    pub music: MusicConfig,

    /// Catalog grid shaping (anime ratio etc.).
    #[serde(default)]
    pub catalog: CatalogConfig,

    /// Intro/credits skip detection configuration.
    #[serde(default)]
    pub skipper: SkipperConfig,

    /// Metadata enrichment configuration (source priorities + timeouts).
    #[serde(default)]
    pub metadata: MetadataConfig,

    /// mdblist list-slug configuration. The API key lives in `secrets.env`,
    /// not here — only the user-tweakable list pointers are persisted in
    /// `runtime.toml`. Defaults are popular curated lists.
    #[serde(default)]
    pub mdblist: MdblistConfig,

    /// Plugin repository URLs.
    /// The first entry is always the built-in official repo.
    /// Users can append community repos; they are merged at plugin-discovery time.
    #[serde(default = "defaults::plugin_repos")]
    pub plugin_repos: Vec<String>,

    /// Plugin-specific configuration values (API keys, settings).
    /// Stored as: plugins.{plugin_name}.{field_key} = value
    #[serde(default)]
    pub plugins: PluginConfig,

    /// Storage directories for different media types.
    #[serde(default)]
    pub storage: StorageConfig,

    /// DSP audio processing configuration.
    #[serde(default)]
    pub dsp: DspConfig,

    /// Enable debug mode: verbose IPC tracing and debug-level logs.
    /// Changing this at runtime enables IPC tracing immediately;
    /// full log-level change takes effect on restart.
    #[serde(default)]
    pub debug_mode: bool,

    /// Run built-in self-tests at startup to verify subsystem health.
    #[serde(default)]
    pub tests_enabled: bool,

    /// Allow plugins tagged `"adult"` (18+) to return search results.
    /// Off by default — must be explicitly enabled by the user.
    #[serde(default)]
    pub adult_content_enabled: bool,

    /// Per-source rating weights, used by the catalog aggregator to
    /// compose a weighted-median composite score. Keys are source
    /// names (`"discogs"`, `"musicbrainz"`, `"lastfm"`, `"imdb"`,
    /// `"tomatometer"`, `"metacritic"`, `"tmdb"`, …) and
    /// values are weights (typical range 0.0–2.0; 0.0 disables a
    /// source entirely). User overrides override the static
    /// per-tab profile defaults; sources not present in any static
    /// profile (e.g. an installed third-party plugin) become active
    /// when given a non-zero weight here. Mirrors the TUI side's
    /// `Providers.RatingSourceWeights` and is the runtime's source
    /// of truth for aggregator overrides.
    ///
    /// Important: a missing `[rating_weights]` block in runtime.toml
    /// MUST fall back to the curated defaults — not an empty map —
    /// otherwise music sources (discogs/MB/lastfm) sit outside
    /// the static per-tab profile (which only knows imdb/tmdb/etc.)
    /// and `weighted_median` finds zero recognised sources, leaving
    /// `entry.rating` empty even when every plugin returned data.
    /// The function path here ensures defaults survive partial-config
    /// files; they only get overridden when the user explicitly
    /// writes a `[rating_weights]` section.
    #[serde(default = "defaults::rating_weights")]
    pub rating_weights: std::collections::HashMap<String, f64>,
}

/// Storage directory configuration for different media types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Directory for movie files.
    #[serde(default = "defaults::movies_dir")]
    pub movies: PathBuf,

    /// Directory for TV series files.
    #[serde(default = "defaults::series_dir")]
    pub series: PathBuf,

    /// Directory for music/audio files.
    #[serde(default = "defaults::music_dir")]
    pub music: PathBuf,

    /// Directory for anime files.
    #[serde(default = "defaults::anime_dir")]
    pub anime: PathBuf,

    /// Directory for podcast files.
    #[serde(default = "defaults::podcasts_dir")]
    pub podcasts: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            movies: defaults::movies_dir(),
            series: defaults::series_dir(),
            music: defaults::music_dir(),
            anime: defaults::anime_dir(),
            podcasts: defaults::podcasts_dir(),
        }
    }
}

/// Logging configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Tracing filter string, e.g. `"stui=debug,warn"`.
    /// Overridden by the `STUI_LOG` environment variable.
    #[serde(default = "defaults::log_level")]
    pub level: String,

    /// Write logs to a file in addition to stderr.
    #[serde(default)]
    pub log_file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: defaults::log_level(),
            log_file: None,
        }
    }
}

/// Playback-related configuration (`[player]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackConfig {
    /// Path to the `mpv` binary. Defaults to searching `$PATH`.
    #[serde(default)]
    pub mpv_bin: Option<PathBuf>,

    /// Extra flags passed to mpv on every invocation.
    #[serde(default)]
    pub mpv_extra_flags: Vec<String>,

    /// Minimum pre-roll in seconds before playback starts (overrides adaptive).
    #[serde(default)]
    pub min_preroll_secs: Option<f64>,

    /// Default volume (0–130). 100 = 100%.
    #[serde(default = "defaults::volume")]
    pub default_volume: f64,

    /// Enable hardware decoding (`auto`, `vaapi`, `nvdec`, `no`).
    #[serde(default = "defaults::hwdec")]
    pub hwdec: String,

    /// Network read-ahead cache in seconds of video.
    #[serde(default = "defaults::cache_secs")]
    pub cache_secs: u32,

    /// Maximum demuxer buffer in megabytes.
    #[serde(default = "defaults::demuxer_max_mb")]
    pub demuxer_max_mb: u32,

    /// Keep mpv running after EOF (useful for debugging; normally false).
    #[serde(default)]
    pub keep_open: bool,

    /// mpv video-output driver for terminal rendering.
    /// Leave empty (default) to use mpv's normal graphical window.
    /// Set to `"kitty"`, `"sixel"`, `"tct"`, or `"chafa"` to render video
    /// inline in a compatible terminal.  When set, the TUI releases the
    /// terminal to mpv and restores it when playback ends.
    #[serde(default)]
    pub terminal_vo: String,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        PlaybackConfig {
            mpv_bin: None,
            mpv_extra_flags: vec![],
            min_preroll_secs: None,
            default_volume: defaults::volume(),
            hwdec: defaults::hwdec(),
            cache_secs: defaults::cache_secs(),
            demuxer_max_mb: defaults::demuxer_max_mb(),
            keep_open: false,
            terminal_vo: String::new(),
        }
    }
}

/// Streaming strategy configuration (`[streaming]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    /// Prefer direct HTTP streams over torrents when both are available.
    #[serde(default = "defaults::prefer_http")]
    pub prefer_http: bool,

    /// Prefer torrent streams (higher quality, more seeders).
    #[serde(default)]
    pub prefer_torrent: bool,

    /// Maximum number of stream candidates to resolve per item.
    #[serde(default = "defaults::max_candidates")]
    pub max_candidates: usize,

    /// Enable automatic stream fallback when a stream fails.
    #[serde(default = "defaults::auto_fallback")]
    pub auto_fallback: bool,

    /// Enable stream benchmarking (latency + throughput probing before playback).
    #[serde(default)]
    pub benchmark_streams: bool,

    /// Drop torrent candidates below this seeder count before they reach
    /// the picker. Default 5 — matches the heuristic that a single-digit
    /// swarm rarely produces a usable download. Set to 0 to disable.
    #[serde(default = "defaults::min_seeders")]
    pub min_seeders: u32,

    /// When true, streams whose seeder count is unknown (None — common
    /// for direct HTTP / debrid CDN URLs and for plugins whose feed
    /// shape doesn't surface a seeder field) are also filtered out.
    /// Default false — unknowns pass through, since most non-torrent
    /// streams legitimately don't have a seeder count. Useful as a
    /// debug switch when troubleshooting why a plugin's results show
    /// no `↑N` indicator: flip this on, see which providers' streams
    /// disappear.
    #[serde(default)]
    pub require_seeders: bool,

    /// When true, streams whose resolution couldn't be extracted from
    /// the release title (StreamQuality::Unknown) are filtered out.
    /// Default false — unknowns pass through. Most release titles
    /// include a resolution tag (1080p, 4K, 720p, …) so an unknown
    /// usually signals a mis-tagged or incomplete release; filtering
    /// them out keeps the picker focused on the comparable options.
    #[serde(default)]
    pub require_resolution: bool,

    /// Per-tier resolution allowlist. All four default to `true` — the
    /// picker shows every quality tier out of the box. Users with
    /// limited bandwidth (or a strong "no SD ever" preference) flip the
    /// matching tier off to remove those candidates from the picker
    /// before they're ranked. `StreamQuality::Unknown` is governed by
    /// `require_resolution` instead, so unknown-tier streams pass
    /// through here regardless.
    #[serde(default = "defaults::allow_tier_true")]
    pub allow_4k: bool,

    #[serde(default = "defaults::allow_tier_true")]
    pub allow_1080p: bool,

    #[serde(default = "defaults::allow_tier_true")]
    pub allow_720p: bool,

    #[serde(default = "defaults::allow_tier_true")]
    pub allow_sd: bool,

    /// Stream-ranking preset. One of:
    ///   - `"balanced"` (default): RankingPolicy::default — best quality
    ///     first, balanced seeder weighting.
    ///   - `"bandwidth_saver"`: RankingPolicy::bandwidth_saver — prefers
    ///     720p, demands ≥5 seeders.
    ///   - `"fastest_start"`: RankingPolicy::fastest_start — heavy seeder
    ///     weighting, demands ≥10 seeders, accepts lower resolution to
    ///     minimise buffering.
    /// Unrecognised strings fall back to `balanced` with a warn.
    #[serde(default = "defaults::ranking_preset")]
    pub ranking_preset: String,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        StreamingConfig {
            prefer_http: defaults::prefer_http(),
            prefer_torrent: false,
            max_candidates: defaults::max_candidates(),
            auto_fallback: defaults::auto_fallback(),
            benchmark_streams: false,
            min_seeders: defaults::min_seeders(),
            require_seeders: false,
            require_resolution: false,
            ranking_preset: defaults::ranking_preset(),
            allow_4k: true,
            allow_1080p: true,
            allow_720p: true,
            allow_sd: true,
        }
    }
}

/// Subtitle configuration (`[subtitles]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitlesConfig {
    /// Automatically download subtitles from OpenSubtitles if not found locally.
    #[serde(default)]
    pub auto_download: bool,

    /// Preferred subtitle language (BCP-47 tag, e.g. `"eng"`, `"fra"`).
    #[serde(default = "defaults::sub_language")]
    pub preferred_language: String,

    /// Directory where downloaded subtitle files are stored.
    #[serde(default)]
    pub download_dir: Option<PathBuf>,

    /// Default subtitle delay in seconds (applied on every file open).
    #[serde(default)]
    pub default_delay: f64,
}

impl Default for SubtitlesConfig {
    fn default() -> Self {
        SubtitlesConfig {
            auto_download: false,
            preferred_language: defaults::sub_language(),
            download_dir: None,
            default_delay: 0.0,
        }
    }
}

/// API keys for providers that require authentication.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiKeysConfig {
    /// TMDB API key (themoviedb.org).
    #[serde(default)]
    pub tmdb: Option<String>,

    /// OMDB API key (omdbapi.com).
    #[serde(default)]
    pub omdb: Option<String>,
}

/// Provider enable/disable flags (`[providers]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Enable TMDB for metadata.
    #[serde(default = "defaults::yes")]
    pub enable_tmdb: bool,

    /// Enable OMDB for metadata fallback.
    #[serde(default)]
    pub enable_omdb: bool,

    /// Enable IMDB for metadata.
    #[serde(default = "defaults::yes")]
    pub enable_imdb: bool,

    /// Enable AniList for anime catalog (no API key required).
    #[serde(default = "defaults::yes")]
    pub enable_anilist: bool,

    /// Enable Jikan (MyAnimeList) for anime catalog (no API key required).
    #[serde(default = "defaults::yes")]
    pub enable_jikan: bool,

    /// Enable MusicBrainz for music catalog (no API key required).
    #[serde(default = "defaults::yes")]
    pub enable_musicbrainz: bool,

    /// Enable Torrentio (via RPC plugin) for stream resolution.
    #[serde(default = "defaults::yes")]
    pub enable_torrentio: bool,

    /// Enable Prowlarr for torrent indexing.
    #[serde(default)]
    pub enable_prowlarr: bool,

    /// Enable OpenSubtitles for subtitle search.
    #[serde(default)]
    pub enable_opensubtitles: bool,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        ProvidersConfig {
            enable_tmdb: defaults::yes(),
            enable_omdb: false,
            enable_imdb: defaults::yes(),
            enable_anilist: defaults::yes(),
            enable_jikan: defaults::yes(),
            enable_musicbrainz: defaults::yes(),
            enable_torrentio: defaults::yes(),
            enable_prowlarr: false,
            enable_opensubtitles: false,
        }
    }
}

/// MPD (Music Player Daemon) connection and playback configuration (`[mpd]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdConfig {
    /// MPD server host. Default: `"127.0.0.1"` (localhost).
    #[serde(default = "defaults::mpd_host")]
    pub host: String,
    /// MPD server port. Default: `6600`.
    #[serde(default = "defaults::mpd_port")]
    pub port: u16,
    /// MPD password. Leave empty if MPD has no password set.
    #[serde(default)]
    pub password: Option<String>,
    /// ReplayGain mode: `"off"` | `"track"` | `"album"` | `"auto"`.
    #[serde(default = "defaults::mpd_replay_gain")]
    pub replay_gain: String,
    /// Crossfade duration in seconds (0 = disabled).
    #[serde(default)]
    pub crossfade_secs: u32,
    /// MixRamp threshold in dB for gapless transitions (None = disabled).
    #[serde(default)]
    pub mixramp_db: Option<f64>,
    /// Remove tracks from queue after playing.
    #[serde(default)]
    pub consume: bool,
    /// Optional path to the MPD music directory (enables library browsing).
    /// Auto-detected from mpd.conf if not set.
    #[serde(default)]
    pub music_dir: Option<std::path::PathBuf>,
    /// Optional path to the MPD playlist directory (where .m3u files live).
    /// Auto-detected from mpd.conf if not set.
    #[serde(default)]
    pub playlist_dir: Option<std::path::PathBuf>,
}

impl Default for MpdConfig {
    fn default() -> Self {
        MpdConfig {
            host: defaults::mpd_host(),
            port: defaults::mpd_port(),
            password: None,
            replay_gain: defaults::mpd_replay_gain(),
            crossfade_secs: 0,
            mixramp_db: None,
            consume: false,
            music_dir: None,
            playlist_dir: None,
        }
    }
}

/// Music tag normalization configuration (`[music.normalize]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicNormalizeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "defaults::normalize_use_lookup")]
    pub use_lookup: bool,
}

impl Default for MusicNormalizeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            use_lookup: defaults::normalize_use_lookup(),
        }
    }
}

/// `[music]` section wrapper.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MusicConfig {
    #[serde(default)]
    pub normalize: MusicNormalizeConfig,
}

/// Per-kind metadata source priority lists (`[metadata.sources]` section).
///
/// Each kind (movies, series, anime, music) has an ordered list of source
/// identifiers — these are the user's preferences for which plugin should
/// be consulted first, second, etc. **The list is no longer exhaustive**:
/// any plugin in the registry whose manifest tags it for this kind is
/// also discovered and joins the fan-out at the tail (after the priority
/// items). This means a third-party plugin only needs the right
/// `tags = ["movies"]` etc. in its plugin.toml to contribute to the
/// detail-card metadata pipeline — no runtime config edit required.
///
/// To opt a plugin OUT of a specific kind's fan-out (e.g. "I have
/// AniList installed for anime but I don't want it polluting movies"),
/// add it to `<kind>_disabled`. The disabled list takes precedence over
/// both priority and discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSources {
    #[serde(default = "defaults::metadata_sources_movies")]
    pub movies: Vec<String>,
    #[serde(default = "defaults::metadata_sources_series")]
    pub series: Vec<String>,
    #[serde(default = "defaults::metadata_sources_anime")]
    pub anime: Vec<String>,
    #[serde(default = "defaults::metadata_sources_music")]
    pub music: Vec<String>,

    // ── Per-kind opt-out lists ────────────────────────────────────────
    // Plugins listed here are excluded from the corresponding kind's
    // detail-card metadata fan-out, regardless of whether they appear
    // in the priority list or were auto-discovered via manifest tags.
    // Default empty — most users won't touch these.
    #[serde(default)]
    pub movies_disabled: Vec<String>,
    #[serde(default)]
    pub series_disabled: Vec<String>,
    #[serde(default)]
    pub anime_disabled: Vec<String>,
    #[serde(default)]
    pub music_disabled: Vec<String>,
}

impl Default for MetadataSources {
    fn default() -> Self {
        MetadataSources {
            movies: defaults::metadata_sources_movies(),
            series: defaults::metadata_sources_series(),
            anime: defaults::metadata_sources_anime(),
            music: defaults::metadata_sources_music(),
            // Default-disable omdb for fresh installs — xmdb covers
            // IMDb id + IMDb rating + Metacritic, rt-provider covers
            // Rotten Tomatoes scores; OMDB has no unique data
            // contribution and burns ~half its 1k/day quota on a
            // typical catalog refresh. Existing user `runtime.toml`
            // configs are untouched (the merge function preserves
            // explicit user disabled lists).
            movies_disabled: vec!["omdb".to_string()],
            series_disabled: vec!["omdb".to_string()],
            anime_disabled: Vec::new(),
            music_disabled: Vec::new(),
        }
    }
}

/// mdblist list-slug configuration (`[mdblist]` section).
///
/// Slugs are `username/list-slug` strings as they appear in mdblist URLs.
/// Defaults are popular curated public lists. The API key is loaded
/// separately via `secrets.env` (key `MDBLIST_API_KEY`) — keeping it out
/// of `runtime.toml` matches stui's wider secrets-vs-config split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdblistConfig {
    #[serde(default = "defaults::mdblist_movies_list")]
    pub movies_list: String,
    #[serde(default = "defaults::mdblist_series_list")]
    pub series_list: String,
}

impl Default for MdblistConfig {
    fn default() -> Self {
        MdblistConfig {
            movies_list: defaults::mdblist_movies_list(),
            series_list: defaults::mdblist_series_list(),
        }
    }
}

/// Metadata enrichment configuration (`[metadata]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataConfig {
    #[serde(default)]
    pub sources: MetadataSources,
    #[serde(default = "defaults::metadata_per_verb_timeout_ms")]
    pub per_verb_timeout_ms: u64,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        MetadataConfig {
            sources: MetadataSources::default(),
            per_verb_timeout_ms: defaults::metadata_per_verb_timeout_ms(),
        }
    }
}

/// `[catalog]` section — controls post-merge shaping of Movies/Series grids.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogConfig {
    /// Fraction of the Movies/Series grid reserved for anime-dominant
    /// entries (present in kitsu/anilist but NOT also in tmdb/omdb/tvdb).
    ///
    /// Clamped to `[0.0, 1.0]`:
    ///   - `0.0` → no anime-dominant entries shown
    ///   - `0.4` → default: 60% general, 40% anime, interleaved in 10-slot batches
    ///   - `0.5` → 50/50
    ///   - `1.0` → only anime-dominant entries
    ///
    /// Interleave pattern is computed at runtime: `round(ratio * 10)` anime
    /// per 10-slot batch, remainder general. Titles that cross over (present
    /// in both anime and global providers, e.g. "Your Name" in TMDB+AniList)
    /// are treated as general so mainstream hits aren't quota'd down.
    #[serde(default = "defaults::catalog_anime_ratio")]
    pub anime_ratio: f32,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            anime_ratio: defaults::catalog_anime_ratio(),
        }
    }
}

/// Configuration for the intro/credits skip detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipperConfig {
    /// Enable skip detection entirely.
    #[serde(default = "defaults::yes")]
    pub enabled: bool,
    /// Automatically seek past detected intros.
    #[serde(default)]
    pub auto_skip_intro: bool,
    /// Automatically seek past detected credits.
    #[serde(default)]
    pub auto_skip_credits: bool,
    /// Seconds of audio to fingerprint from the beginning for intro detection.
    #[serde(default = "defaults::skip_scan_secs")]
    pub intro_scan_secs: u32,
    /// Seconds of audio to fingerprint from the end for credits detection.
    #[serde(default = "defaults::skip_scan_secs")]
    pub credits_scan_secs: u32,
    /// Minimum intro duration in seconds to accept as a match.
    #[serde(default = "defaults::min_intro_secs")]
    pub min_intro_secs: f64,
    /// Maximum intro duration in seconds.
    #[serde(default = "defaults::max_intro_secs")]
    pub max_intro_secs: f64,
    /// Minimum credits duration in seconds.
    #[serde(default = "defaults::min_credits_secs")]
    pub min_credits_secs: f64,
    /// Maximum credits duration in seconds.
    #[serde(default = "defaults::max_credits_secs")]
    pub max_credits_secs: f64,
    /// Fingerprint similarity threshold 0.0–1.0 (higher = stricter matching).
    #[serde(default = "defaults::similarity_threshold")]
    pub similarity_threshold: f64,
    /// Minimum number of episodes that must be fingerprinted before comparison runs.
    #[serde(default = "defaults::min_episodes")]
    pub min_episodes: usize,
}

impl Default for SkipperConfig {
    fn default() -> Self {
        SkipperConfig {
            enabled: true,
            auto_skip_intro: false,
            auto_skip_credits: false,
            intro_scan_secs: 300,
            credits_scan_secs: 300,
            min_intro_secs: 20.0,
            max_intro_secs: 120.0,
            min_credits_secs: 20.0,
            max_credits_secs: 300.0,
            similarity_threshold: 0.85,
            min_episodes: 2,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            plugin_dir: defaults::plugin_dir(),
            cache_dir: defaults::cache_dir(),
            data_dir: defaults::data_dir(),
            theme_mode: defaults::theme_mode(),
            logging: LoggingConfig::default(),
            stremio_addons: vec![],
            playback: PlaybackConfig::default(),
            streaming: StreamingConfig::default(),
            subtitles: SubtitlesConfig::default(),
            providers: ProvidersConfig::default(),
            api_keys: ApiKeysConfig::default(),
            mpd: MpdConfig::default(),
            music: MusicConfig::default(),
            catalog: CatalogConfig::default(),
            skipper: SkipperConfig::default(),
            metadata: MetadataConfig::default(),
            mdblist: MdblistConfig::default(),
            plugin_repos: defaults::plugin_repos(),
            plugins: std::collections::HashMap::new(),
            storage: StorageConfig::default(),
            dsp: DspConfig::default(),
            debug_mode: false,
            tests_enabled: false,
            adult_content_enabled: false,
            rating_weights: defaults::rating_weights(),
        }
    }
}

mod defaults {
    use std::path::PathBuf;

    /// Plugins live under the user's config dir alongside config.toml
    /// and themes/ — they're user-installed artifacts, not regenerable
    /// caches, and survive uninstall the same way config does.
    pub fn plugin_dir() -> PathBuf {
        config_base().join("plugins")
    }
    /// Caches go to XDG_CACHE_HOME (`~/.cache/stui`). They're
    /// regenerable by definition: grid snapshots, chafa-rendered
    /// posters, sqlite HTTP response cache, album art tiles. Putting
    /// them under config would bloat dotfile-sync setups; the
    /// XDG_CACHE_HOME convention is what backup tools and uninstall
    /// scripts skip by default.
    pub fn cache_dir() -> PathBuf {
        cache_base()
    }
    /// Persistent application data (history db, watchlists,
    /// downloads metadata) lives next to config — it's per-user
    /// state worth backing up but not part of the editable config
    /// surface.
    pub fn data_dir() -> PathBuf {
        config_base().join("data")
    }
    pub fn theme_mode() -> String {
        "dark".to_string()
    }

    /// Default rating-source weights — one per "well-known" source
    /// the catalog aggregator can blend. Equal-weighted for sane
    /// out-of-the-box behaviour; users tune in runtime.toml's
    /// `rating_weights` block (or the Settings UI).
    /// Plugins emitting unknown source keys (e.g. a future
    /// user-authored rating plugin) become active the moment they
    /// appear here with a non-zero weight.
    pub fn rating_weights() -> std::collections::HashMap<String, f64> {
        [
            // Music sources
            ("discogs", 1.0),
            ("musicbrainz", 0.7),
            ("lastfm", 0.5), // synthetic from listener count — useful but coarser
            // Movie/series sources (existing static profile keys)
            ("imdb", 1.0),
            ("tomatometer", 1.0),
            ("metacritic", 1.0),
            ("audience_score", 0.8),
            ("tmdb", 0.7),
            ("anilist", 0.0),
            ("kitsu", 0.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v as f64))
        .collect()
    }

    /// Default fraction of Movies/Series grid dedicated to anime-dominant
    /// entries (from kitsu/anilist but NOT also in tmdb/omdb/tvdb).
    /// 0.4 = 60% general / 40% anime, interleaved in 10-slot batches.
    pub fn catalog_anime_ratio() -> f32 {
        0.4
    }
    pub fn log_level() -> String {
        "info".to_string()
    }

    // PlaybackConfig defaults
    pub fn volume() -> f64 {
        100.0
    }
    pub fn hwdec() -> String {
        "auto".to_string()
    }
    pub fn cache_secs() -> u32 {
        20
    }
    pub fn demuxer_max_mb() -> u32 {
        50
    }

    // StreamingConfig defaults
    pub fn prefer_http() -> bool {
        true
    }
    pub fn max_candidates() -> usize {
        10
    }
    pub fn auto_fallback() -> bool {
        true
    }
    pub fn min_seeders() -> u32 {
        // Empirical floor: anything ≤5 seeders rarely produces a usable
        // download in practice. Surfacing those streams just clutters
        // the picker — the user has to rifle past dead torrents to
        // find playable ones. Override to 0 in runtime.toml to disable
        // the filter entirely.
        5
    }
    /// Default for the per-tier `allow_*` fields. Lifted into a named
    /// helper so the four `#[serde(default = "...")]` attributes don't
    /// each need their own `fn allow_4k_default`-style trampoline.
    pub fn allow_tier_true() -> bool {
        true
    }
    /// Default ranking-preset string. Mirrors `RankingPolicy::default()`'s
    /// "best quality first" intent.
    pub fn ranking_preset() -> String {
        "balanced".to_string()
    }

    // SubtitlesConfig defaults
    pub fn sub_language() -> String {
        "eng".to_string()
    }

    // MpdConfig defaults
    pub fn mpd_host() -> String {
        "127.0.0.1".to_string()
    }
    pub fn mpd_port() -> u16 {
        6600
    }
    pub fn mpd_replay_gain() -> String {
        "auto".to_string()
    }

    // Plugin repos default
    pub fn plugin_repos() -> Vec<String> {
        vec![
            "https://plugins.stui.dev".to_string(),
            "https://ozogorgor.github.io/stui_plugins".to_string(),
            // Bundled metadata plugins (tmdb, omdb, anilist, kitsu,
            // discogs, lastfm, musicbrainz) — published from the stui
            // monorepo's plugin-release.yml workflow. Listed so that a
            // user who uninstalls a bundled plugin can reinstall it
            // from Plugin Manager → Available.
            "https://ozogorgor.github.io/stui".to_string(),
        ]
    }

    // SkipperConfig defaults
    pub fn skip_scan_secs() -> u32 {
        300
    }
    pub fn min_intro_secs() -> f64 {
        20.0
    }
    pub fn max_intro_secs() -> f64 {
        120.0
    }
    pub fn min_credits_secs() -> f64 {
        20.0
    }
    pub fn max_credits_secs() -> f64 {
        300.0
    }
    pub fn similarity_threshold() -> f64 {
        0.85
    }
    pub fn min_episodes() -> usize {
        2
    }

    // Shared bool defaults
    pub fn yes() -> bool {
        true
    }

    // MusicNormalizeConfig defaults
    pub fn normalize_use_lookup() -> bool {
        true
    }

    // MetadataConfig defaults
    pub(super) fn metadata_sources_movies() -> Vec<String> {
        vec!["tmdb".into(), "omdb".into(), "tvdb".into(), "fanart".into()]
    }
    pub(super) fn metadata_sources_series() -> Vec<String> {
        vec!["tvdb".into(), "tmdb".into(), "omdb".into(), "fanart".into()]
    }
    pub(super) fn metadata_sources_anime() -> Vec<String> {
        vec![
            "anilist".into(),
            "kitsu".into(),
            "tvdb".into(),
            "fanart".into(),
        ]
    }
    pub(super) fn metadata_sources_music() -> Vec<String> {
        vec!["musicbrainz".into(), "discogs".into(), "lastfm".into()]
    }
    pub(super) fn metadata_per_verb_timeout_ms() -> u64 {
        8000
    }
    pub(super) fn mdblist_movies_list() -> String {
        "snoak/latest-movies-digital-release".into()
    }
    pub(super) fn mdblist_series_list() -> String {
        "garycrawfordgc/latest-tv-shows".into()
    }

    /// Legacy single-root used before the XDG split. Retained only
    /// for migration helpers (see `migrate_legacy_paths`); all
    /// production paths now go through `config_base()` or
    /// `cache_base()`.
    pub(super) fn legacy_base() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".stui")
    }

    /// `~/.config/stui` (or platform equivalent via `dirs::config_dir`).
    /// Holds: config.toml, runtime.toml, themes/, plugins/, secrets.env,
    /// data/, history.db, mediacache.json.
    pub(super) fn config_base() -> PathBuf {
        dirs::config_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("stui")
    }

    /// `~/.cache/stui` (or `XDG_CACHE_HOME/stui`). Holds: grid/,
    /// chafa/, posters/, art/, response.db. Anything under here is
    /// safe to delete — the runtime regenerates on demand.
    pub(super) fn cache_base() -> PathBuf {
        dirs::cache_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("stui")
    }

    // Storage directory defaults (base paths, files organized into subfolders)
    pub fn movies_dir() -> PathBuf {
        dirs::video_dir()
            .or_else(|| Some(home().join("Videos")))
            .unwrap_or_else(|| PathBuf::from("Videos"))
    }
    pub fn series_dir() -> PathBuf {
        dirs::video_dir()
            .or_else(|| Some(home().join("Videos")))
            .unwrap_or_else(|| PathBuf::from("Videos"))
    }
    pub fn anime_dir() -> PathBuf {
        dirs::video_dir()
            .or_else(|| Some(home().join("Videos")))
            .unwrap_or_else(|| PathBuf::from("Videos"))
    }
    pub fn music_dir() -> PathBuf {
        dirs::audio_dir()
            .or_else(|| Some(home().join("Music")))
            .unwrap_or_else(|| PathBuf::from("Music"))
    }
    pub fn podcasts_dir() -> PathBuf {
        dirs::audio_dir()
            .or_else(|| Some(home().join("Music")))
            .unwrap_or_else(|| PathBuf::from("Music"))
    }

    fn home() -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    }
}

#[cfg(test)]
mod music_normalize_tests {
    use super::*;

    #[test]
    fn music_normalize_defaults() {
        let c: MusicConfig = toml::from_str("").unwrap();
        assert!(!c.normalize.enabled);
        assert!(c.normalize.use_lookup);
    }

    #[test]
    fn music_normalize_round_trip() {
        let s = r#"
            [normalize]
            enabled = true
            use_lookup = false
        "#;
        let c: MusicConfig = toml::from_str(s).unwrap();
        assert!(c.normalize.enabled);
        assert!(!c.normalize.use_lookup);
    }
}

#[cfg(test)]
mod metadata_config_tests {
    use super::*;

    #[test]
    fn metadata_sources_default_disables_omdb() {
        let m = MetadataSources::default();
        assert!(
            m.movies_disabled.iter().any(|s| s == "omdb"),
            "fresh-install movies_disabled should include omdb (rt-provider supersedes); got {:?}",
            m.movies_disabled
        );
        assert!(
            m.series_disabled.iter().any(|s| s == "omdb"),
            "fresh-install series_disabled should include omdb; got {:?}",
            m.series_disabled
        );
    }

    #[test]
    fn metadata_sources_defaults_include_tvdb() {
        let mc = MetadataConfig::default();
        assert_eq!(mc.sources.movies, vec!["tmdb", "omdb", "tvdb", "fanart"]);
        assert_eq!(mc.sources.series, vec!["tvdb", "tmdb", "omdb", "fanart"]);
        assert_eq!(mc.sources.anime, vec!["anilist", "kitsu", "tvdb", "fanart"]);
        assert_eq!(mc.sources.music, vec!["musicbrainz", "discogs", "lastfm"]);
    }

    #[test]
    fn metadata_config_deserializes_missing_sources_to_defaults() {
        let toml = "";
        let mc: MetadataConfig = toml::from_str(toml).unwrap();
        assert!(!mc.sources.movies.is_empty());
    }

    #[test]
    fn metadata_config_per_verb_timeout_has_default() {
        let mc = MetadataConfig::default();
        assert_eq!(mc.per_verb_timeout_ms, 8000);
    }
}
