//! Configuration type definitions.
//!
//! These structs mirror the sections of `~/.stui/config/stui.toml`.
//! Every field has a `#[serde(default)]` so partial config files are fine.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Top-level runtime configuration, passed to `Engine::new()` and friends.
///
/// Constructed by `loader::load()` which merges:
///   1. Compiled-in defaults
///   2. `~/.stui/config/stui.toml` (if present)
///   3. Environment variable overrides (`STUI_*`)
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Intro/credits skip detection configuration.
    #[serde(default)]
    pub skipper: SkipperConfig,

    /// Plugin repository URLs.
    /// The first entry is always the built-in official repo.
    /// Users can append community repos; they are merged at plugin-discovery time.
    #[serde(default = "defaults::plugin_repos")]
    pub plugin_repos: Vec<String>,

    /// Stream quality ranking preferences.
    #[serde(default)]
    pub stream: StreamPreferences,
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
            level:    defaults::log_level(),
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
            mpv_bin:          None,
            mpv_extra_flags:  vec![],
            min_preroll_secs: None,
            default_volume:   defaults::volume(),
            hwdec:            defaults::hwdec(),
            cache_secs:       defaults::cache_secs(),
            demuxer_max_mb:   defaults::demuxer_max_mb(),
            keep_open:        false,
            terminal_vo:      String::new(),
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
}

impl Default for StreamingConfig {
    fn default() -> Self {
        StreamingConfig {
            prefer_http:       defaults::prefer_http(),
            prefer_torrent:    false,
            max_candidates:    defaults::max_candidates(),
            auto_fallback:     defaults::auto_fallback(),
            benchmark_streams: false,
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
            auto_download:      false,
            preferred_language: defaults::sub_language(),
            download_dir:       None,
            default_delay:      0.0,
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

    /// Last.fm API key (last.fm).
    #[serde(default)]
    pub lastfm: Option<String>,
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
            enable_tmdb:          defaults::yes(),
            enable_omdb:          false,
            enable_imdb:          defaults::yes(),
            enable_anilist:       defaults::yes(),
            enable_jikan:         defaults::yes(),
            enable_musicbrainz:   defaults::yes(),
            enable_torrentio:     defaults::yes(),
            enable_prowlarr:      false,
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
    #[serde(default)]
    pub music_dir: Option<std::path::PathBuf>,
}

impl Default for MpdConfig {
    fn default() -> Self {
        MpdConfig {
            host:           defaults::mpd_host(),
            port:           defaults::mpd_port(),
            password:       None,
            replay_gain:    defaults::mpd_replay_gain(),
            crossfade_secs: 0,
            mixramp_db:     None,
            consume:        false,
            music_dir:      None,
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
            enabled:              true,
            auto_skip_intro:      false,
            auto_skip_credits:    false,
            intro_scan_secs:      300,
            credits_scan_secs:    300,
            min_intro_secs:       20.0,
            max_intro_secs:       120.0,
            min_credits_secs:     20.0,
            max_credits_secs:     300.0,
            similarity_threshold: 0.85,
            min_episodes:         2,
        }
    }
}

/// Stream quality ranking preferences (`[stream]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPreferences {
    /// Preferred protocol: `"http"` | `"torrent"` | `None` for auto.
    pub preferred_protocol: Option<String>,
    /// Maximum resolution to accept, e.g. `"1080p"`.
    pub max_resolution:     Option<String>,
    /// Maximum file size in megabytes.
    pub max_size_mb:        Option<u64>,
    /// Minimum seeder count for torrent streams (0 = no minimum).
    pub min_seeders:        u32,
    /// Labels/tags to avoid, e.g. `["CAM", "TS"]`.
    pub avoid_labels:       Vec<String>,
    /// Prefer HDR streams when available.
    pub prefer_hdr:         bool,
    /// Preferred codec names in priority order, e.g. `["hevc", "avc"]`.
    pub preferred_codecs:   Vec<String>,
    /// Weight applied to seeder count when scoring streams.
    pub seeder_weight:      f64,
    /// Exclude CAM/screener rips from candidates.
    pub exclude_cam:        bool,
}

impl Default for StreamPreferences {
    fn default() -> Self {
        Self {
            preferred_protocol: None,
            max_resolution:     None,
            max_size_mb:        None,
            min_seeders:        0,
            avoid_labels:       vec![],
            prefer_hdr:         false,
            preferred_codecs:   vec![],
            seeder_weight:      1.0,
            exclude_cam:        true,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            plugin_dir:     defaults::plugin_dir(),
            cache_dir:      defaults::cache_dir(),
            data_dir:       defaults::data_dir(),
            theme_mode:     defaults::theme_mode(),
            logging:        LoggingConfig::default(),
            stremio_addons: vec![],
            playback:       PlaybackConfig::default(),
            streaming:      StreamingConfig::default(),
            subtitles:      SubtitlesConfig::default(),
            providers:      ProvidersConfig::default(),
            api_keys:       ApiKeysConfig::default(),
            mpd:            MpdConfig::default(),
            skipper:        SkipperConfig::default(),
            plugin_repos:   defaults::plugin_repos(),
            stream:         StreamPreferences::default(),
        }
    }
}

mod defaults {
    use std::path::PathBuf;

    pub fn plugin_dir() -> PathBuf { base().join("plugins") }
    pub fn cache_dir()  -> PathBuf { base().join("cache") }
    pub fn data_dir()   -> PathBuf { base().join("data") }
    pub fn theme_mode() -> String  { "dark".to_string() }
    pub fn log_level()  -> String  { "info".to_string() }

    // PlaybackConfig defaults
    pub fn volume()         -> f64    { 100.0 }
    pub fn hwdec()          -> String { "auto".to_string() }
    pub fn cache_secs()     -> u32    { 20 }
    pub fn demuxer_max_mb() -> u32    { 50 }

    // StreamingConfig defaults
    pub fn prefer_http()    -> bool   { true }
    pub fn max_candidates() -> usize  { 10 }
    pub fn auto_fallback()  -> bool   { true }

    // SubtitlesConfig defaults
    pub fn sub_language()   -> String { "eng".to_string() }

    // MpdConfig defaults
    pub fn mpd_host()        -> String { "127.0.0.1".to_string() }
    pub fn mpd_port()        -> u16    { 6600 }
    pub fn mpd_replay_gain() -> String { "auto".to_string() }

    // Plugin repos default
    pub fn plugin_repos() -> Vec<String> {
        vec!["https://plugins.stui.dev".to_string()]
    }

    // SkipperConfig defaults
    pub fn skip_scan_secs()        -> u32   { 300 }
    pub fn min_intro_secs()        -> f64   { 20.0 }
    pub fn max_intro_secs()        -> f64   { 120.0 }
    pub fn min_credits_secs()      -> f64   { 20.0 }
    pub fn max_credits_secs()      -> f64   { 300.0 }
    pub fn similarity_threshold()  -> f64   { 0.85 }
    pub fn min_episodes()          -> usize { 2 }

    // Shared bool defaults
    pub fn yes() -> bool { true }

    fn base() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".stui")
    }
}
