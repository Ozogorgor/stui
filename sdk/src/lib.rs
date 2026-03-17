//! # stui-plugin-sdk
//!
//! The Rust SDK for building stui plugins.
//!
//! ## Quick start
//!
//! ```rust
//! use stui_plugin_sdk::prelude::*;
//!
//! pub struct MyProvider;
//!
//! impl StuiPlugin for MyProvider {
//!     fn name(&self) -> &str { "my-provider" }
//!     fn version(&self) -> &str { "1.0.0" }
//!     fn plugin_type(&self) -> PluginType { PluginType::Provider }
//!
//!     fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
//!         // ... fetch content ...
//!         PluginResult::Ok(SearchResponse { items: vec![], total: 0 })
//!     }
//!
//!     fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse> {
//!         PluginResult::Ok(ResolveResponse {
//!             stream_url: "https://...".into(),
//!             quality: Some("1080p".into()),
//!             subtitles: vec![],
//!         })
//!     }
//! }
//!
//! // Register the plugin — generates all required WASM exports
//! stui_export_plugin!(MyProvider);
//! ```
//!
//! ## Compile to WASM
//!
//! ```bash
//! rustup target add wasm32-wasip1
//! cargo build --target wasm32-wasip1 --release
//! # Output: target/wasm32-wasip1/release/my_provider.wasm
//! ```

// ── ABI types (re-exported for plugin authors) ────────────────────────────────

pub const STUI_ABI_VERSION: i32 = 1;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub tab: String,
    pub page: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub items: Vec<PluginEntry>,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub id: String,
    pub title: String,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub rating: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveResponse {
    pub stream_url: String,
    pub quality: Option<String>,
    pub subtitles: Vec<SubtitleTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub language: String,
    pub url: String,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PluginResult<T> {
    Ok(T),
    Err(PluginError),
}

impl<T> PluginResult<T> {
    pub fn ok(value: T) -> Self { Self::Ok(value) }
    pub fn err(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Err(PluginError { code: code.into(), message: message.into() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginType {
    Provider,
    Resolver,
    Metadata,
    Auth,
    Subtitle,
    Indexer,
}

impl PluginType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Provider => "provider",
            Self::Resolver => "resolver",
            Self::Metadata => "metadata",
            Self::Auth     => "auth",
            Self::Subtitle => "subtitle",
            Self::Indexer  => "indexer",
        }
    }
}

// ── StuiPlugin trait ─────────────────────────────────────────────────────────

/// The trait every stui plugin implements.
///
/// Implement this trait, then call `stui_export_plugin!(YourPlugin)` to
/// generate the WASM ABI glue automatically.
pub trait StuiPlugin {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn plugin_type(&self) -> PluginType;

    /// Search for content matching `req.query` in the given `req.tab`.
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse>;

    /// Resolve an entry ID into a playable stream URL.
    fn resolve(&self, req: ResolveRequest) -> PluginResult<ResolveResponse>;
}

// ── Host function imports (called by plugin at runtime) ───────────────────────

/// Log a message through the stui host logger.
/// Use the `log!` / `info!` / `warn!` macros instead of calling this directly.
#[cfg(target_arch = "wasm32")]
extern "C" {
    pub fn stui_log(level: i32, ptr: *const u8, len: i32);
    pub fn stui_http_get(url_ptr: *const u8, url_len: i32) -> i64;
    pub fn stui_cache_get(key_ptr: *const u8, key_len: i32) -> i64;
    pub fn stui_cache_set(
        key_ptr: *const u8, key_len: i32,
        val_ptr: *const u8, val_len: i32,
    );
}

/// Log a message at the given level through the host logger.
pub fn host_log(level: i32, msg: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        stui_log(level, msg.as_ptr(), msg.len() as i32);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        eprintln!("[stui-plugin level={}] {}", level, msg);
    }
}

/// Convenience macros for logging from plugins.
#[macro_export] macro_rules! plugin_info  { ($($t:tt)*) => { $crate::host_log(2, &format!($($t)*)) }; }
#[macro_export] macro_rules! plugin_warn  { ($($t:tt)*) => { $crate::host_log(3, &format!($($t)*)) }; }
#[macro_export] macro_rules! plugin_error { ($($t:tt)*) => { $crate::host_log(4, &format!($($t)*)) }; }
#[macro_export] macro_rules! plugin_debug { ($($t:tt)*) => { $crate::host_log(1, &format!($($t)*)) }; }

/// Make an HTTP GET request through the sandboxed host.
/// Returns the response body as a String, or an error message.
pub fn http_get(url: &str) -> Result<String, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let packed = unsafe { stui_http_get(url.as_ptr(), url.len() as i32) };
        if packed == 0 { return Err("http_get returned null".into()); }
        let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
        let len = (packed & 0xFFFFFFFF) as usize;
        let json = unsafe { std::str::from_utf8(std::slice::from_raw_parts(ptr, len)) }
            .map_err(|e| e.to_string())?;
        let resp: crate::HttpResponse = serde_json::from_str(json)
            .map_err(|e| e.to_string())?;
        if resp.status >= 200 && resp.status < 300 {
            Ok(resp.body)
        } else {
            Err(format!("HTTP {}: {}", resp.status, resp.body))
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Err(format!("http_get only available in WASM context (url: {url})"))
    }
}

#[derive(Debug, serde::Deserialize)]
struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Make an HTTP POST request with a JSON body through the sandboxed host.
///
/// The host function `stui_http_post` takes the URL and the JSON payload.
/// Internally the host adds any required CORS/auth headers from the plugin
/// manifest's `network_permissions` list.
///
/// Returns the response body as a String on 2xx, or an Err with the status+body.
pub fn http_post_json(url: &str, body: &str) -> Result<String, String> {
    // Encode request as a single JSON object the host can parse.
    // Format: {"url":"...","body":"..."}
    let payload = format!(
        "{{\"url\":{},\"body\":{}}}",
        serde_json::to_string(url).unwrap_or_default(),
        serde_json::to_string(body).unwrap_or_default(),
    );
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn stui_http_post(ptr: *const u8, len: i32) -> i64;
        }
        let packed = unsafe { stui_http_post(payload.as_ptr(), payload.len() as i32) };
        if packed == 0 { return Err("http_post returned null".into()); }
        let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
        let len = (packed & 0xFFFFFFFF) as usize;
        let json = unsafe { std::str::from_utf8(std::slice::from_raw_parts(ptr, len)) }
            .map_err(|e| e.to_string())?;
        let resp: HttpResponse = serde_json::from_str(json)
            .map_err(|e| e.to_string())?;
        if resp.status >= 200 && resp.status < 300 {
            Ok(resp.body)
        } else {
            Err(format!("HTTP {}: {}", resp.status, resp.body))
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = payload;
        Err(format!("http_post only available in WASM context (url: {url})"))
    }
}

/// Retrieve a value from the host-managed key-value cache.
/// Returns None if the key is missing or expired.
pub fn cache_get(key: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let packed = unsafe { stui_cache_get(key.as_ptr(), key.len() as i32) };
        if packed == 0 { return None; }
        let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
        let len = (packed & 0xFFFFFFFF) as usize;
        let s = unsafe { std::str::from_utf8(std::slice::from_raw_parts(ptr, len)) }.ok()?;
        Some(s.to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        None
    }
}

/// Store a value in the host-managed key-value cache.
/// The cache is persistent across plugin calls within a session.
pub fn cache_set(key: &str, value: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        stui_cache_set(
            key.as_ptr(), key.len() as i32,
            value.as_ptr(), value.len() as i32,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        eprintln!("[stui-plugin cache_set] key={key} value_len={}", value.len());
    }
}

// ── ABI glue macro ────────────────────────────────────────────────────────────

/// Registers your plugin and generates all required WASM ABI exports.
///
/// # Example
/// ```rust
/// stui_export_plugin!(MyProvider);
/// ```
///
/// This generates:
/// - `stui_abi_version() -> i32`
/// - `stui_alloc(len: i32) -> i32`
/// - `stui_free(ptr: i32, len: i32)`
/// - `stui_search(ptr: i32, len: i32) -> i64`
/// - `stui_resolve(ptr: i32, len: i32) -> i64`
#[macro_export]
macro_rules! stui_export_plugin {
    ($plugin_ty:ty) => {
        // Safety: WASM is single-threaded; we use a global instance.
        static PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> = std::sync::OnceLock::new();

        fn get_plugin() -> &'static $plugin_ty {
            PLUGIN_INSTANCE.get_or_init(|| <$plugin_ty>::default())
        }

        /// ABI version — host checks this before calling any other function.
        #[no_mangle]
        pub extern "C" fn stui_abi_version() -> i32 {
            $crate::STUI_ABI_VERSION
        }

        /// Memory allocation — host uses this to write request JSON.
        #[no_mangle]
        pub extern "C" fn stui_alloc(len: i32) -> i32 {
            let mut buf = Vec::<u8>::with_capacity(len as usize);
            let ptr = buf.as_mut_ptr() as i32;
            std::mem::forget(buf);
            ptr
        }

        /// Memory free — host calls this after reading response JSON.
        #[no_mangle]
        pub extern "C" fn stui_free(ptr: i32, len: i32) {
            unsafe {
                let _ = Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize);
            }
        }

        /// Search entry point. Input: SearchRequest JSON. Output: packed (ptr<<32)|len.
        #[no_mangle]
        pub extern "C" fn stui_search(ptr: i32, len: i32) -> i64 {
            let input = unsafe {
                std::slice::from_raw_parts(ptr as *const u8, len as usize)
            };
            let req: $crate::SearchRequest = match serde_json::from_slice(input) {
                Ok(r) => r,
                Err(e) => return $crate::__write_result(
                    &$crate::PluginResult::<$crate::SearchResponse>::err("PARSE_ERROR", e.to_string())
                ),
            };
            let result = get_plugin().search(req);
            $crate::__write_result(&result)
        }

        /// Resolve entry point. Input: ResolveRequest JSON. Output: packed (ptr<<32)|len.
        #[no_mangle]
        pub extern "C" fn stui_resolve(ptr: i32, len: i32) -> i64 {
            let input = unsafe {
                std::slice::from_raw_parts(ptr as *const u8, len as usize)
            };
            let req: $crate::ResolveRequest = match serde_json::from_slice(input) {
                Ok(r) => r,
                Err(e) => return $crate::__write_result(
                    &$crate::PluginResult::<$crate::ResolveResponse>::err("PARSE_ERROR", e.to_string())
                ),
            };
            let result = get_plugin().resolve(req);
            $crate::__write_result(&result)
        }
    };
}

/// Internal helper — serialises a result to WASM memory and returns packed ptr/len.
/// Not part of the public API; used by the `stui_export_plugin!` macro.
#[doc(hidden)]
pub fn __write_result<T: serde::Serialize>(result: &T) -> i64 {
    let json = serde_json::to_vec(result).unwrap_or_else(|e| {
        format!("{{\"status\":\"err\",\"code\":\"SERIALIZE\",\"message\":\"{e}\"}}").into_bytes()
    });
    let len = json.len();
    let ptr = json.as_ptr() as i64;
    std::mem::forget(json);
    (ptr << 32) | (len as i64)
}

// ── Prelude ───────────────────────────────────────────────────────────────────

pub mod prelude {
    pub use crate::{
        PluginEntry, PluginResult, PluginType, ResolveRequest, ResolveResponse,
        SearchRequest, SearchResponse, SubtitleTrack, StuiPlugin,
    };
    pub use crate::{plugin_info, plugin_warn, plugin_error, plugin_debug};
    pub use crate::http_get;
    pub use crate::http_post_json;
    pub use crate::cache_get;
    pub use crate::cache_set;
    pub use crate::stui_export_plugin;
}
