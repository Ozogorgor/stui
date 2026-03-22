//! Media cache — persists catalog grid data locally so stui can show a
//! browseable offline library when providers are unreachable or the runtime
//! fails to start.

mod store;

pub use store::MediaCacheStore;

pub fn default_cache_path() -> std::path::PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join("stui").join("mediacache.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("mediacache.json"))
}
