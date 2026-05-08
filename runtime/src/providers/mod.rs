//! Content providers for stream resolution.
//!
//! # Architecture
//!
//! - **Catalog/Metadata providers** (TMDB, IMDB, OMDB, AniList, etc.) are now
//!   loaded as WASM plugins via the Engine. See `plugins/` directory.
//!
//! - **Stream providers** provide playable stream URLs. These are currently
//!   built-in for performance but could also be WASM plugins in the future.
//!
//! # Provider Trait
//!
//! Built-in stream providers implement the `Provider` trait:
//!
//! ```text
//! streams(id)       -> Vec<Stream>          # resolve playable URLs
//! subtitles(id)     -> Vec<SubtitleTrack>   # optional subtitle tracks
//! ```
//!
#![allow(dead_code)]

/// Bridge between stream benchmark results and provider health scoring.
pub mod bench_health_bridge;
/// Stream benchmarking — measures HTTP throughput and latency for stream ranking.
pub mod benchmark;
/// Provider capability declarations — what each provider can do.
pub mod capabilities;
/// Circuit breaker — prevents cascading failures by disabling failing providers.
pub mod circuit_breaker;
/// Provider health tracking — reliability metrics for smart stream ranking.
pub mod health;
/// Stream providers — resolve entry IDs to playable URLs.
pub mod streams;
/// Provider rate-limit throttle — per-provider token-bucket and backoff.
pub mod throttle;

#[allow(unused_imports)]
pub use bench_health_bridge::BenchHealthBridge;
#[allow(unused_imports)]
pub use benchmark::StreamBenchmarker;
pub use capabilities::ProviderCapabilities;
#[allow(unused_imports)]
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerStats};
#[allow(unused_imports)]
pub use health::{blend_score, FailureKind, HealthRegistry, ProviderStats};
#[allow(unused_imports)]
pub use throttle::ProviderThrottle;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, SubtitleTrack};

// ── HDR format ────────────────────────────────────────────────────────────────

/// HDR format carried by a stream.
/// Ordered by visual quality: `None` < `Hdr10` < `Hdr10Plus` < `DolbyVision`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum HdrFormat {
    #[default]
    None,
    Hdr10,
    Hdr10Plus,
    DolbyVision,
}

impl HdrFormat {
    /// Parse an HDR format from a stream name / label string.
    pub fn from_name(name: &str) -> Self {
        let n = name.to_uppercase();
        if n.contains("DOLBY VISION") || n.contains(" DV ") || n.ends_with(" DV") {
            HdrFormat::DolbyVision
        } else if n.contains("HDR10+") || n.contains("HDR10PLUS") {
            HdrFormat::Hdr10Plus
        } else if n.contains("HDR") {
            HdrFormat::Hdr10
        } else {
            HdrFormat::None
        }
    }

    /// Ranking bonus points contributed to `QualityScore`.
    pub fn score(&self) -> u32 {
        match self {
            HdrFormat::DolbyVision => 50,
            HdrFormat::Hdr10Plus => 45,
            HdrFormat::Hdr10 => 40,
            HdrFormat::None => 0,
        }
    }
}

// ── Stream ────────────────────────────────────────────────────────────────────

/// A single playable stream returned by [`Provider::streams`].
///
/// The six core fields (`id`, `name`, `url`, `mime`, `quality`, `provider`)
/// are always present.  The remaining metadata fields are optional — providers
/// fill in what they know; the ranking engine falls back to name-parsing for
/// anything left as `None`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stream {
    /// Unique identifier within this provider (e.g. magnet hash, URL).
    pub id: String,
    /// Human-readable label (e.g. "1080p BluRay HEVC", "HDTV 720p").
    pub name: String,
    /// The actual playable URL or magnet link.
    pub url: String,
    /// Mime type hint: "video/x-matroska", "application/x-bittorrent", "magnet", …
    pub mime: Option<String>,
    /// Coarse quality bucket used for sort ordering.
    pub quality: StreamQuality,
    /// Which provider produced this stream.
    pub provider: String,

    // ── Extended metadata ─────────────────────────────────────────────────
    /// Transport protocol: "http", "https", "magnet", "torrent", "hls", "dash", …
    #[serde(default)]
    pub protocol: Option<String>,
    /// Torrent swarm size at resolution time.
    #[serde(default)]
    pub seeders: Option<u32>,
    /// Video bitrate in kilobits per second.
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    /// Video codec, e.g. `"H264"`, `"HEVC"`, `"AV1"`.
    #[serde(default)]
    pub codec: Option<String>,
    /// Freeform resolution string for display, e.g. `"1920×1080"`, `"4K UHD"`.
    /// `quality` is the canonical sort key; this is the human label.
    #[serde(default)]
    pub resolution: Option<String>,
    /// HDR format of the video stream.
    #[serde(default)]
    pub hdr: HdrFormat,
    /// Approximate encoded file size in bytes.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Last measured round-trip latency to the source in milliseconds.
    /// Populated by the stream benchmarking pass when enabled.
    #[serde(default)]
    pub latency_ms: Option<u32>,
    /// Measured download throughput in megabits per second.
    /// Populated by the stream benchmarking pass when enabled.
    #[serde(default)]
    pub speed_mbps: Option<f64>,
    /// Audio channel layout, e.g. `"2.0"`, `"5.1"`, `"7.1 Atmos"`.
    #[serde(default)]
    pub audio_channels: Option<String>,
    /// Primary audio language (ISO 639-1 or 639-2 tag, e.g. `"en"`, `"spa"`).
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum StreamQuality {
    Sd,
    #[default]
    Unknown,
    Hd720,
    Hd1080,
    Uhd4k,
}

impl StreamQuality {
    pub fn from_label(s: &str) -> Self {
        let s = s.to_lowercase();
        if s.contains("4k") || s.contains("2160") {
            return StreamQuality::Uhd4k;
        }
        if s.contains("1080") {
            return StreamQuality::Hd1080;
        }
        if s.contains("720") {
            return StreamQuality::Hd720;
        }
        if s.contains("480") || s.contains("sd") {
            return StreamQuality::Sd;
        }
        StreamQuality::Unknown
    }

    pub fn label(&self) -> &'static str {
        match self {
            StreamQuality::Uhd4k => "4K",
            StreamQuality::Hd1080 => "1080p",
            StreamQuality::Hd720 => "720p",
            StreamQuality::Sd => "SD",
            StreamQuality::Unknown => "?",
        }
    }
}

// ── Provider trait ────────────────────────────────────────────────────────────

/// Unified interface implemented by every built-in provider.
///
/// All methods have default no-op implementations so providers can opt in
/// to only what they support (e.g. a subtitle-only provider).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Returns this provider's capability profile.
    /// The engine uses this to skip providers that can't serve a given request type.
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Machine identifier (lowercase, no spaces). Used as a config key prefix.
    fn name(&self) -> &str;

    /// Human-readable display name shown in the UI.
    /// Default: same as `name()`.
    fn display_name(&self) -> &str {
        self.name()
    }

    /// One-line description of what this provider covers.
    /// Shown in the Plugin Settings screen below the provider name.
    fn description(&self) -> &str {
        ""
    }

    /// Configuration schema — one entry per required credential/API key.
    /// Default: empty (no credentials needed; provider is always ready).
    ///
    /// Each entry describes a config key path (e.g. `"api_keys.tmdb"`), its
    /// label, a usage hint, and whether the value is currently set.
    fn config_schema(&self) -> Vec<crate::ipc::ProviderField> {
        vec![]
    }

    /// Whether this provider is currently active (all required credentials
    /// are present and the provider is ready to serve requests).
    /// Default: `true` — keyless providers are always active.
    fn is_active(&self) -> bool {
        true
    }

    // ── Catalog ───────────────────────────────────────────────────────────

    /// Fetch the trending / popular catalog for a tab. `page` is 1-indexed.
    async fn fetch_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>>;

    /// Full-text search within a tab.
    async fn search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>>;

    // ── Streams ───────────────────────────────────────────────────────────

    /// Resolve a catalog entry ID into zero or more playable streams.
    ///
    /// Providers that don't support stream resolution return `Ok(vec![])`.
    /// The runtime merges results from all providers and lets the user pick.
    async fn streams(&self, id: &str) -> Result<Vec<Stream>> {
        let _ = id;
        Ok(vec![])
    }

    // ── Subtitles ─────────────────────────────────────────────────────────

    /// Fetch subtitle tracks for a media entry.
    ///
    /// Providers that don't offer subtitles return `Ok(vec![])`.
    async fn subtitles(&self, id: &str) -> Result<Vec<SubtitleTrack>> {
        let _ = id;
        Ok(vec![])
    }

    // ── Capabilities ──────────────────────────────────────────────────────

    /// Which tabs this provider has data for.
    /// Returning `None` means "all tabs".
    fn supported_tabs(&self) -> Option<Vec<MediaTab>> {
        None
    }

    /// True if this provider can resolve streams (not just catalog).
    fn has_streams(&self) -> bool {
        false
    }

    /// Which `MediaSource` types this provider can supply content for.
    ///
    /// The engine uses this to skip providers that can't help for a given
    /// source type — no wasted round trips.  Returning `None` means the
    /// provider supports all source types (opt-in to everything).
    fn supported_sources(&self) -> Option<&[crate::media::MediaSource]> {
        None
    }

    /// True if this provider can supply subtitles.
    fn has_subtitles(&self) -> bool {
        false
    }
}
