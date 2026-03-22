//! Canonical stream model — the single source of truth for playable streams.
//!
//! Every part of the pipeline that touches a playable stream should use these
//! types:
//!
//! ```text
//! resolver / plugin_rpc  →  Vec<StreamCandidate>
//!        ↓
//! quality::rank()        →  Vec<StreamCandidate>  (sorted, scored)
//!        ↓
//! player::bridge         →  consumes StreamCandidate.url
//! ```
//!
//! The `quality` module scores candidates; this module owns the data model.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ── StreamProtocol ────────────────────────────────────────────────────────────

/// Wire protocol of a playable stream.
///
/// The player bridge uses this to decide which path to take:
/// - `Torrent` / `Magnet` → aria2c download → mpv
/// - `Http` / `Hls` / `Dash` → mpv directly (or yt-dlp pre-pass)
/// - `Direct` → passed to mpv unchanged
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamProtocol {
    /// BitTorrent: `.torrent` file URL (aria2 will fetch + seed)
    Torrent,
    /// BitTorrent: `magnet:?xt=urn:btih:…` URI
    Magnet,
    /// Plain HTTP/HTTPS progressive download or direct file URL
    Http,
    /// HTTP Live Streaming (Apple HLS) `.m3u8`
    Hls,
    /// MPEG-DASH `.mpd` manifest
    Dash,
    /// RTMP live stream
    Rtmp,
    /// Protocol unknown or not yet classified — runtime will probe the URL
    Unknown,
}

impl StreamProtocol {
    /// Detect the protocol from a URL string.
    pub fn from_url(url: &str) -> Self {
        if url.starts_with("magnet:") {
            return StreamProtocol::Magnet;
        }
        let lower = url.to_lowercase();
        if lower.ends_with(".torrent") {
            return StreamProtocol::Torrent;
        }
        if lower.contains(".m3u8") {
            return StreamProtocol::Hls;
        }
        if lower.contains(".mpd") {
            return StreamProtocol::Dash;
        }
        if lower.starts_with("rtmp://") {
            return StreamProtocol::Rtmp;
        }
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return StreamProtocol::Http;
        }
        StreamProtocol::Unknown
    }

    /// True if this stream requires aria2c to download before mpv can open it.
    pub fn needs_aria2(&self) -> bool {
        matches!(self, StreamProtocol::Torrent | StreamProtocol::Magnet)
    }

    /// True if mpv can open this URL directly without preprocessing.
    pub fn is_direct(&self) -> bool {
        matches!(
            self,
            StreamProtocol::Http
                | StreamProtocol::Hls
                | StreamProtocol::Dash
                | StreamProtocol::Rtmp
        )
    }
}

// ── StreamCandidate ───────────────────────────────────────────────────────────

/// A single resolved, playable stream — the universal unit passed between
/// resolver, quality ranker, and player bridge.
///
/// This is the canonical definition.  `quality::StreamCandidate` wraps this
/// with a `QualityScore` for ranking purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamCandidate {
    /// Playable URL: HTTP URL, magnet URI, or .torrent URL.
    pub url: String,

    /// Protocol classification (auto-detected from `url` if not set).
    pub protocol: StreamProtocol,

    /// Human-readable quality label, e.g. `"1080p"`, `"720p HDR"`.
    pub quality: Option<String>,

    /// Estimated bitrate in kbps (used for quality scoring).
    pub bitrate_kbps: Option<u32>,

    /// Number of seeders (torrents only, used for quality scoring).
    pub seeders: Option<u32>,

    /// Source type label, e.g. `"BluRay"`, `"WEB-DL"`, `"HDTV"`, `"CAM"`.
    pub source: Option<String>,

    /// Codec string, e.g. `"HEVC"`, `"x264"`, `"AV1"`.
    pub codec: Option<String>,

    /// Name or identifier of the provider that returned this stream.
    pub provider: String,

    /// Raw name / title returned by the provider (used for codec/source extraction).
    pub name: String,

    /// Subtitle tracks bundled with this stream (rare; usually fetched separately).
    #[serde(default)]
    pub subtitles: Vec<BundledSubtitle>,

    // ── Extended fields (populated when available) ────────────────────────
    /// Approximate total file size in bytes (used to estimate download time).
    pub size_bytes: Option<u64>,

    /// Primary audio language tag (e.g. `"en"`, `"ja"`).
    pub audio_lang: Option<String>,

    /// Subtitle language tags available in-stream (e.g. `["en", "es"]`).
    #[serde(default)]
    pub subtitle_langs: Vec<String>,

    /// Measured network latency to the stream source in milliseconds.
    /// Populated by the stream benchmarking pass; `None` before benchmarking.
    pub latency_ms: Option<u32>,

    /// Measured download throughput in kbps during benchmarking.
    /// Higher is better; `None` before benchmarking.
    pub throughput_kbps: Option<u32>,
}

/// A subtitle track bundled directly with a stream (not fetched separately).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledSubtitle {
    pub language: String,
    pub url: String,
    pub format: String, // "srt" | "vtt" | "ass"
}

impl StreamCandidate {
    /// Construct a candidate from a bare URL, inferring protocol automatically.
    pub fn from_url(url: impl Into<String>, provider: impl Into<String>) -> Self {
        let url = url.into();
        let protocol = StreamProtocol::from_url(&url);
        StreamCandidate {
            protocol,
            name: url.clone(),
            url,
            provider: provider.into(),
            quality: None,
            bitrate_kbps: None,
            seeders: None,
            source: None,
            codec: None,
            subtitles: vec![],
            size_bytes: None,
            audio_lang: None,
            subtitle_langs: vec![],
            latency_ms: None,
            throughput_kbps: None,
        }
    }

    /// True if this stream needs aria2c (torrent or magnet).
    pub fn needs_aria2(&self) -> bool {
        self.protocol.needs_aria2()
    }
}
