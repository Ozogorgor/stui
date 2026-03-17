//! Config loader — merges TOML file with environment variable overrides.
//!
//! Load order (later entries win):
//!   1. Compiled-in `RuntimeConfig::default()`
//!   2. `~/.stui/config/stui.toml` (if present, silently skipped otherwise)
//!   3. Environment variable overrides (`STUI_*`)

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use super::types::RuntimeConfig;

/// Load the runtime configuration.
///
/// Reads from the default config file location and applies any
/// `STUI_*` environment variable overrides on top.
pub fn load() -> RuntimeConfig {
    let path = default_config_path();
    load_from(path.as_deref())
}

/// Load from a specific path (useful for tests).
pub fn load_from(path: Option<&Path>) -> RuntimeConfig {
    let mut cfg = RuntimeConfig::default();

    if let Some(p) = path {
        match load_toml(p) {
            Ok(file_cfg) => {
                cfg = file_cfg;
                debug!("config loaded from {}", p.display());
            }
            Err(e) if p.exists() => {
                warn!("failed to parse config {}: {e}", p.display());
            }
            _ => {
                debug!("no config file at {} — using defaults", p.display());
            }
        }
    }

    apply_env_overrides(&mut cfg);
    cfg
}

fn load_toml(path: &Path) -> anyhow::Result<RuntimeConfig> {
    let text = std::fs::read_to_string(path)?;
    let cfg: RuntimeConfig = toml::from_str(&text)?;
    Ok(cfg)
}

fn default_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".stui").join("config").join("stui.toml"))
}

/// Apply `STUI_*` environment variable overrides to an existing config.
fn apply_env_overrides(cfg: &mut RuntimeConfig) {
    if let Ok(v) = std::env::var("STUI_PLUGIN_DIR") {
        cfg.plugin_dir = PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("STUI_CACHE_DIR") {
        cfg.cache_dir = PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("STUI_DATA_DIR") {
        cfg.data_dir = PathBuf::from(v);
    }
    if let Ok(v) = std::env::var("STUI_THEME_MODE") {
        cfg.theme_mode = v;
    }
    if let Ok(v) = std::env::var("STUI_LOG") {
        cfg.logging.level = v;
    }
    // STUI_STREMIO_ADDONS — comma-separated list of manifest URLs
    if let Ok(v) = std::env::var("STUI_STREMIO_ADDONS") {
        let addons: Vec<String> = v
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !addons.is_empty() {
            cfg.stremio_addons = addons;
        }
    }
}
