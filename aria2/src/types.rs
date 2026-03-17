//! types.rs — aria2 JSON-RPC type definitions.
//!
//! All method names, parameter shapes, and response types are derived from:
//!   https://aria2.github.io/manual/en/html/aria2c.html#rpc-interface
//!
//! Key points:
//! - GID is a 16-char hex string, e.g. "2089b05ecca3d829"
//! - Numeric values are strings in aria2 responses (e.g. "1073741824" for 1 GiB)
//! - The `token:` prefix is prepended to the secret by the client automatically

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── GID ───────────────────────────────────────────────────────────────────────

/// A download identifier returned by aria2 — 16 hex characters.
pub type Gid = String;

// ── Download status ───────────────────────────────────────────────────────────

/// Status of a single download — returned by aria2.tellStatus.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatus {
    /// The GID of this download.
    pub gid: Gid,

    /// "active" | "waiting" | "paused" | "error" | "complete" | "removed"
    pub status: String,

    /// Total file size in bytes (as a decimal string — use parse_u64()).
    #[serde(default)]
    pub total_length: String,

    /// Downloaded bytes so far.
    #[serde(default)]
    pub completed_length: String,

    /// Upload bytes.
    #[serde(default)]
    pub upload_length: String,

    /// Current download speed in bytes/s.
    #[serde(default)]
    pub download_speed: String,

    /// Current upload speed in bytes/s.
    #[serde(default)]
    pub upload_speed: String,

    /// Number of connected peers (BitTorrent only).
    #[serde(default)]
    pub num_seeders: Option<String>,

    /// Error code if status == "error".
    #[serde(default)]
    pub error_code: Option<String>,

    /// Human-readable error message.
    #[serde(default)]
    pub error_message: Option<String>,

    /// BitTorrent info (only present for .torrent / magnet downloads).
    #[serde(default)]
    pub bittorrent: Option<BittorrentInfo>,

    /// File list.
    #[serde(default)]
    pub files: Vec<FileInfo>,

    /// Directory where files are saved.
    #[serde(default)]
    pub dir: String,
}

impl DownloadStatus {
    /// Progress as a fraction 0.0–1.0, or None if total is unknown.
    pub fn progress(&self) -> Option<f64> {
        let total = parse_u64(&self.total_length)?;
        let done  = parse_u64(&self.completed_length)?;
        if total == 0 { return None; }
        Some(done as f64 / total as f64)
    }

    /// Download speed in bytes/s.
    pub fn speed_bps(&self) -> u64 { parse_u64(&self.download_speed).unwrap_or(0) }

    /// ETA in seconds, None if speed is zero or progress unknown.
    pub fn eta_secs(&self) -> Option<u64> {
        let total   = parse_u64(&self.total_length)?;
        let done    = parse_u64(&self.completed_length)?;
        let speed   = self.speed_bps();
        let remaining = total.saturating_sub(done);
        if speed == 0 { return None; }
        Some(remaining / speed)
    }

    pub fn is_complete(&self) -> bool { self.status == "complete" }
    pub fn is_active(&self)   -> bool { self.status == "active" }
    pub fn is_error(&self)    -> bool { self.status == "error" }
}

/// BitTorrent-specific metadata in a DownloadStatus.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BittorrentInfo {
    #[serde(default)]
    pub info: Option<TorrentInfo>,
    #[serde(default)]
    pub announce_list: Vec<Vec<String>>,
    /// "single" | "multi"
    #[serde(default)]
    pub mode: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TorrentInfo {
    /// Torrent name (utf-8).
    #[serde(default)]
    pub name: String,
}

/// A single file entry inside a download.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub index: String,
    pub path:  String,
    #[serde(default)]
    pub length: String,
    #[serde(default)]
    pub completed_length: String,
    /// "true" | "false"
    #[serde(default)]
    pub selected: String,
    #[serde(default)]
    pub uris: Vec<UriInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UriInfo {
    pub uri:    String,
    /// "used" | "waiting"
    pub status: String,
}

// ── Global statistics ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStat {
    /// Overall download speed in bytes/s.
    pub download_speed: String,
    /// Overall upload speed in bytes/s.
    pub upload_speed:   String,
    /// Number of active downloads.
    pub num_active:     String,
    /// Number of waiting downloads.
    pub num_waiting:    String,
    /// Number of stopped downloads (within --max-download-result).
    pub num_stopped:    String,
}

// ── Options ───────────────────────────────────────────────────────────────────

/// Options for adding a download — all fields optional.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct AddOptions {
    /// Directory to save the file(s) in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,

    /// Suggested output filename (ignored for multi-file torrents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,

    /// Maximum download speed in bytes/s (0 = unlimited).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_download_limit: Option<String>,

    /// Maximum upload speed in bytes/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_upload_limit: Option<String>,

    /// Stop seeding when ratio reaches this value (BitTorrent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_ratio: Option<String>,

    /// Stop seeding after this many seconds (0 = seed indefinitely).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_time: Option<String>,

    /// Pause immediately after download completes (don't seed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_metadata: Option<String>,

    /// Select files to download from a multi-file torrent (1-indexed, comma list).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select_file: Option<String>,
}

impl AddOptions {
    /// Create options for a streaming-oriented torrent download.
    /// Downloads to the given directory, stops seeding after completion.
    pub fn streaming(dir: impl Into<String>) -> Self {
        Self {
            dir: Some(dir.into()),
            seed_time: Some("0".into()),
            seed_ratio: Some("0.0".into()),
            ..Default::default()
        }
    }

    /// Options for a full torrent download that seeds afterwards.
    pub fn full_download(dir: impl Into<String>) -> Self {
        Self {
            dir: Some(dir.into()),
            ..Default::default()
        }
    }
}

// ── aria2 version ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub version:          String,
    pub enabled_features: Vec<String>,
}

// ── Notification ──────────────────────────────────────────────────────────────

/// A notification pushed by aria2 over the WebSocket connection.
#[derive(Debug, Clone)]
pub struct Notification {
    pub event: NotificationEvent,
    pub gid:   Gid,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationEvent {
    DownloadStart,
    DownloadPause,
    DownloadStop,
    DownloadComplete,
    DownloadError,
    BtDownloadComplete,
    Unknown(String),
}

impl NotificationEvent {
    pub fn from_method(method: &str) -> Self {
        match method {
            "aria2.onDownloadStart"    => Self::DownloadStart,
            "aria2.onDownloadPause"    => Self::DownloadPause,
            "aria2.onDownloadStop"     => Self::DownloadStop,
            "aria2.onDownloadComplete" => Self::DownloadComplete,
            "aria2.onDownloadError"    => Self::DownloadError,
            "aria2.onBtDownloadComplete" => Self::BtDownloadComplete,
            other => Self::Unknown(other.to_string()),
        }
    }
}

// ── JSON-RPC wire types ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct RpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id:      String,
    pub method:  &'a str,
    pub params:  serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub(crate) struct RpcResponse {
    #[serde(default)]
    pub id: Option<String>,
    pub result: Option<serde_json::Value>,
    pub error:  Option<RpcError>,
    // For WebSocket notifications — has "method" instead of id/result
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct RpcError {
    pub code:    i32,
    pub message: String,
}

// ── Helper ────────────────────────────────────────────────────────────────────

pub fn parse_u64(s: &str) -> Option<u64> {
    s.parse().ok()
}

pub fn format_speed(bps: u64) -> String {
    const MIB: u64 = 1 << 20;
    const KIB: u64 = 1 << 10;
    if bps >= MIB {
        format!("{:.1} MiB/s", bps as f64 / MIB as f64)
    } else if bps >= KIB {
        format!("{:.0} KiB/s", bps as f64 / KIB as f64)
    } else {
        format!("{} B/s", bps)
    }
}

pub fn format_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}
