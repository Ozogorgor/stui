//! JSON-RPC wire protocol for external (out-of-process) plugins.
//!
//! External plugins communicate with the runtime over stdin/stdout using
//! newline-delimited JSON.  This module defines every message type on both
//! sides of that channel.
//!
//! # Plugin lifecycle
//!
//! ```text
//! runtime spawns plugin process
//!      │
//!      ├─▶  {"method":"handshake","id":"1"}
//!      ◀─── {"id":"1","result":{"name":"torrentio","version":"1.0",
//!      │                         "capabilities":["streams"]}}
//!      │
//!      ├─▶  {"method":"streams.resolve","id":"2",
//!      │     "params":{"id":"tt0816692"}}
//!      ◀─── {"id":"2","result":[{"url":"magnet:?xt=...","name":"1080p BluRay"}]}
//!      │
//!      └─▶  {"method":"shutdown","id":"3"}
//! ```
//!
//! # Writing a plugin
//!
//! Any language that can read stdin line-by-line and write JSON to stdout works.
//! See `docs/plugins.md` for language-specific examples (Python, Node, Go, Rust).

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Outbound: runtime → plugin ────────────────────────────────────────────────

/// A request sent from the runtime to a plugin process.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcRequest {
    /// Correlation ID echoed back in the response so concurrent calls match.
    pub id: String,
    /// Dotted method name, e.g. `"catalog.search"`, `"streams.resolve"`.
    pub method: String,
    /// Method-specific parameters (see `RpcMethod` below for shapes).
    #[serde(default)]
    pub params: Value,
}

/// All methods the runtime can call on a plugin.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcMethod {
    /// Initial handshake — plugin replies with its name, version, capabilities.
    Handshake,
    /// Full-text catalog search.
    CatalogSearch,
    /// Trending catalog for a tab.
    CatalogTrending,
    /// Resolve a media entry ID into stream URLs.
    StreamsResolve,
    /// Fetch subtitle tracks for a media entry.
    SubtitlesFetch,
    /// Graceful shutdown — plugin should exit cleanly.
    Shutdown,
}

impl RpcMethod {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            RpcMethod::Handshake => "handshake",
            RpcMethod::CatalogSearch => "catalog.search",
            RpcMethod::CatalogTrending => "catalog.trending",
            RpcMethod::StreamsResolve => "streams.resolve",
            RpcMethod::SubtitlesFetch => "subtitles.fetch",
            RpcMethod::Shutdown => "shutdown",
        }
    }

    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "handshake" => Some(RpcMethod::Handshake),
            "catalog.search" => Some(RpcMethod::CatalogSearch),
            "catalog.trending" => Some(RpcMethod::CatalogTrending),
            "streams.resolve" => Some(RpcMethod::StreamsResolve),
            "subtitles.fetch" => Some(RpcMethod::SubtitlesFetch),
            "shutdown" => Some(RpcMethod::Shutdown),
            _ => None,
        }
    }
}

// ── Inbound: plugin → runtime ─────────────────────────────────────────────────

/// A response sent from a plugin process back to the runtime.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcResponse {
    /// Matches the `id` from the corresponding `RpcRequest`.
    pub id: String,
    /// Present on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Present on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcResponse {
    #[allow(dead_code)]
    pub fn ok(id: impl Into<String>, result: Value) -> Self {
        RpcResponse {
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    #[allow(dead_code)]
    pub fn err(id: impl Into<String>, code: i32, message: impl Into<String>) -> Self {
        RpcResponse {
            id: id.into(),
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }

    #[allow(dead_code)]
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

// ── Handshake payload ─────────────────────────────────────────────────────────

/// The plugin's self-description, returned in response to `handshake`.
///
/// The runtime uses `capabilities` to register the plugin for automatic
/// dispatch — no manual routing code needed.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PluginHandshake {
    /// Plugin display name, e.g. `"Torrentio"`.
    pub name: String,
    /// Semantic version string, e.g. `"1.2.0"`.
    pub version: String,
    /// List of capability strings this plugin supports.
    /// Valid values: `"catalog"`, `"streams"`, `"subtitles"`, `"auth"`, `"index"`.
    pub capabilities: Vec<String>,
    /// Optional human-readable description shown in the plugin list.
    #[serde(default)]
    pub description: Option<String>,
}

// ── catalog.search params / result ───────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct CatalogSearchParams {
    pub query: String,
    pub tab: String,
    #[serde(default = "default_page")]
    pub page: u32,
}

#[allow(dead_code)]
fn default_page() -> u32 {
    1
}

/// A catalog item returned by a plugin.  Matches the runtime's `MediaEntry`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcMediaItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub poster_url: Option<String>,
}

// ── streams.resolve params / result ──────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamsResolveParams {
    /// The media entry ID to resolve (e.g. `"tt0816692"`).
    pub id: String,
}

/// A stream URL returned by a plugin.
///
/// All fields beyond `url` and `name` are optional — fill in whatever the
/// plugin knows.  The runtime's ranking engine uses them directly when
/// present, and falls back to heuristic name-parsing for the rest.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcStream {
    /// Playable URL — HTTP URL, magnet URI, or `.torrent` URL.
    pub url: String,
    /// Human-readable label, e.g. `"1080p BluRay HEVC"`.
    pub name: String,
    /// Coarse quality bucket string: `"4k"`, `"1080p"`, `"720p"`, `"sd"`.
    #[serde(default)]
    pub quality: Option<String>,
    /// Video bitrate in kilobits per second.
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    /// Torrent swarm size at resolution time.
    #[serde(default)]
    pub seeders: Option<u32>,
    /// Video codec, e.g. `"H264"`, `"HEVC"`, `"AV1"`.
    #[serde(default)]
    pub codec: Option<String>,
    /// Freeform resolution string, e.g. `"1920x1080"`, `"4K UHD"`.
    #[serde(default)]
    pub resolution: Option<String>,
    /// HDR format: `"none"`, `"hdr10"`, `"hdr10_plus"`, `"dolby_vision"`.
    #[serde(default)]
    pub hdr: Option<crate::providers::HdrFormat>,
    /// Approximate file size in bytes.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Audio channel layout, e.g. `"5.1"`, `"7.1 Atmos"`.
    #[serde(default)]
    pub audio_channels: Option<String>,
    /// Primary audio language (ISO 639-1/2, e.g. `"en"`, `"spa"`).
    #[serde(default)]
    pub language: Option<String>,
}

// ── subtitles.fetch params / result ──────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct SubtitlesFetchParams {
    pub id: String,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcSubtitleTrack {
    pub language: String,
    pub url: String,
    pub format: String,
}
