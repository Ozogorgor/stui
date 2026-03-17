//! Stable ABI types — versioned JSON contract between the stui host and plugins.
//!
//! ## Versioning
//! `STUI_ABI_VERSION` is embedded in every manifest and checked at load time.
//! A plugin compiled against ABI v1 will refuse to load on a v2 host (and vice
//! versa) unless the host explicitly declares backward-compatibility.
//!
//! ## Memory model
//! All data crosses the WASM boundary as UTF-8 JSON written into WASM linear
//! memory. The plugin owns its memory; the host reads through a shared view.
//!
//!   host → plugin:  host calls stui_alloc(len), writes JSON, calls fn(ptr,len)
//!   plugin → host:  fn returns (ptr, len) pointing into plugin memory;
//!                   host reads, then calls stui_free(ptr, len)
//!
//! ## Function exports (plugin must provide)
//! ```text
//! stui_abi_version() -> i32          version guard — must equal STUI_ABI_VERSION
//! stui_alloc(len: i32) -> i32        allocate len bytes, return ptr
//! stui_free(ptr: i32, len: i32)      free previously allocated region
//! stui_search(ptr: i32, len: i32) -> i64   packed (ptr<<32)|len of result JSON
//! stui_resolve(ptr: i32, len: i32) -> i64  packed (ptr<<32)|len of result JSON
//! ```
//!
//! ## Host imports (host provides, plugin may call)
//! ```text
//! stui_log(level: i32, ptr: i32, len: i32)
//! stui_http_get(url_ptr: i32, url_len: i32) -> i64   packed result ptr/len
//! stui_cache_get(key_ptr: i32, key_len: i32) -> i64
//! stui_cache_set(kp: i32, kl: i32, vp: i32, vl: i32)
//! ```

use serde::{Deserialize, Serialize};

/// Current ABI version. Bump this when making breaking changes.
pub const STUI_ABI_VERSION: i32 = 1;

// ── Requests (host → plugin, serialized to JSON in WASM memory) ──────────────

/// Payload passed to `stui_search`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub tab: String,       // "movies" | "series" | "music" | "library"
    pub page: u32,
    pub limit: u32,
}

/// Payload passed to `stui_resolve`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub entry_id: String,
}

// ── Responses (plugin → host, serialized to JSON in WASM memory) ─────────────

/// Returned by `stui_search`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    pub items: Vec<PluginEntry>,
    pub total: u32,
}

/// A single media entry returned by a plugin search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    /// Provider-scoped unique id (used for resolve calls).
    pub id: String,
    pub title: String,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub rating: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub imdb_id: Option<String>,
}

/// Returned by `stui_resolve`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveResponse {
    pub stream_url: String,
    pub quality: Option<String>,
    pub subtitles: Vec<SubtitleTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub language: String,
    pub url: String,
    pub format: String, // "srt" | "vtt" | "ass"
}

/// Generic error envelope — plugins return this on failure.
#[derive(Debug, Serialize, Deserialize)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

/// A result type that plugins return — either success payload or an error.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PluginResult<T> {
    Ok(T),
    Err(PluginError),
}

// ── Host import payloads ──────────────────────────────────────────────────────

/// HTTP response returned by the `stui_http_get` host import.
#[derive(Debug, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Log levels for the `stui_log` host import.
#[repr(i32)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info  = 2,
    Warn  = 3,
    Error = 4,
}

impl LogLevel {
    pub fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Trace,
            1 => Self::Debug,
            3 => Self::Warn,
            4 => Self::Error,
            _ => Self::Info,
        }
    }
}

// ── ABI version check ─────────────────────────────────────────────────────────

/// Error returned when a plugin's ABI version doesn't match the host.
#[derive(Debug, thiserror::Error)]
pub enum AbiError {
    #[error("ABI version mismatch: plugin={plugin}, host={host}")]
    VersionMismatch { plugin: i32, host: i32 },

    #[error("plugin is missing required export: {0}")]
    MissingExport(String),

    #[error("WASM execution error: {0}")]
    Execution(String),

    #[error("JSON serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("memory error: {0}")]
    Memory(String),
}
