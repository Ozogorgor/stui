//! Media cache — persists catalog grid data locally so stui can show a
//! browseable offline library when providers are unreachable or the runtime
//! fails to start.

pub mod album_art;
pub mod normalize;
mod store;
pub mod tag_write_job;
pub mod tag_writer;

pub use store::MediaCacheStore;

pub fn default_cache_path() -> std::path::PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join("stui").join("mediacache.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("mediacache.json"))
}
