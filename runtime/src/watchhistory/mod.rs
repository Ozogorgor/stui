//! Watch history — tracks playback positions so stui can offer
//! "resume from where you left off" on movies and series.

mod store;

pub use store::{WatchHistoryEntry, WatchHistoryStore};

pub fn default_history_path() -> std::path::PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join("stui").join("history.db"))
        .unwrap_or_else(|| std::path::PathBuf::from("history.db"))
}
