//! Process-wide `ExceptionStore` singleton.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::exceptions::ExceptionStore;

static STORE: OnceLock<Arc<ExceptionStore>> = OnceLock::new();

pub fn init(bundled_path: PathBuf, user_path: PathBuf) -> Arc<ExceptionStore> {
    STORE
        .get_or_init(|| Arc::new(ExceptionStore::new(bundled_path, user_path)))
        .clone()
}

pub fn global() -> Option<Arc<ExceptionStore>> { STORE.get().cloned() }

/// Default bundled path:
///   1. `<CARGO_MANIFEST_DIR>/../config/exceptions.toml` (dev checkout).
///   2. `/usr/share/stui/exceptions.toml` (installed).
/// First existing one wins; otherwise returns (1) so error messages are useful.
pub fn default_bundled_path() -> PathBuf {
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("config").join("exceptions.toml"))
        .unwrap_or_else(|| PathBuf::from("config/exceptions.toml"));
    if dev.exists() { return dev; }
    let installed = PathBuf::from("/usr/share/stui/exceptions.toml");
    if installed.exists() { return installed; }
    dev
}

pub fn default_user_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join("stui").join("exceptions.toml"))
        .unwrap_or_else(|| PathBuf::from("exceptions.toml"))
}
