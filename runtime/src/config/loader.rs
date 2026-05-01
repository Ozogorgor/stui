//! Config loader — merges TOML file with environment variable overrides.
//!
//! Load order (later entries win):
//!   1. Compiled-in `RuntimeConfig::default()`
//!   2. `~/.stui/config/stui.toml` (if present, silently skipped otherwise)
//!   3. `~/.stui/secrets.env` (API keys and passwords)
//!   4. Environment variable overrides (`STUI_*`, `*_API_KEY`, etc.)

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use super::secrets::Secrets;
use super::types::{MetadataSources, RuntimeConfig};

pub fn load() -> RuntimeConfig {
    let path = default_config_path();
    load_from(path.as_deref())
}

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

    // Order matters: normalize stale toml literals BEFORE env
    // overrides, so an explicit `STUI_PLUGIN_DIR=…` always wins.
    normalize_legacy_paths(&mut cfg);
    apply_secrets(&mut cfg);
    apply_env_overrides(&mut cfg);
    cfg
}

/// Normalise stale legacy `~/.stui/...` path strings that older stui
/// versions auto-wrote into runtime.toml. Earlier defaults pointed
/// `plugin_dir`, `cache_dir`, and `data_dir` under `~/.stui/`; the
/// XDG migration moved those defaults to `~/.config/stui/...` and
/// `~/.cache/stui/`, but a user upgrading in place still has the
/// legacy literals frozen into their runtime.toml — and the runtime
/// faithfully respects whatever's in the file. Result: any
/// freshly-installed plugin drops into the XDG plugin dir but
/// the runtime keeps scanning the legacy one and never sees it.
///
/// Detect that exact stale-default state and silently re-point to
/// the XDG equivalent. We only override values that look like the
/// stale defaults (literal `~/.stui/...` or absolute `$HOME/.stui/...`)
/// — a user who has intentionally pointed `plugin_dir` at
/// `/opt/stui/plugins` for a system install gets left alone.
fn normalize_legacy_paths(cfg: &mut RuntimeConfig) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };
    let legacy_root = home.join(".stui");
    let xdg_config = dirs::config_dir()
        .or_else(|| Some(home.join(".config")))
        .map(|c| c.join("stui"));
    let xdg_cache = dirs::cache_dir()
        .or_else(|| Some(home.join(".cache")))
        .map(|c| c.join("stui"));

    if let Some(xdg) = &xdg_config {
        if cfg.plugin_dir == legacy_root.join("plugins") {
            warn!(
                "config: rewriting stale plugin_dir {} → {}",
                cfg.plugin_dir.display(),
                xdg.join("plugins").display(),
            );
            cfg.plugin_dir = xdg.join("plugins");
        }
        if cfg.data_dir == legacy_root.join("data") {
            warn!(
                "config: rewriting stale data_dir {} → {}",
                cfg.data_dir.display(),
                xdg.join("data").display(),
            );
            cfg.data_dir = xdg.join("data");
        }
    }
    if let Some(xdg) = &xdg_cache {
        if cfg.cache_dir == legacy_root.join("cache") {
            warn!(
                "config: rewriting stale cache_dir {} → {}",
                cfg.cache_dir.display(),
                xdg.display(),
            );
            cfg.cache_dir = xdg.clone();
        }
    }
}

fn load_toml(path: &Path) -> anyhow::Result<RuntimeConfig> {
    let text = std::fs::read_to_string(path)?;
    let mut cfg: RuntimeConfig = toml::from_str(&text)?;
    merge_metadata_source_defaults(&mut cfg);
    Ok(cfg)
}

/// Append any canonical metadata sources missing from the user's per-kind
/// priority lists. Preserves the user's chosen ordering — new defaults are
/// only appended to the tail, never reordered.
///
/// This runs after TOML deserialization so users who upgrade stui don't have
/// to hand-edit their config to pick up newly-bundled sources (e.g. tvdb).
pub(crate) fn merge_metadata_source_defaults(cfg: &mut RuntimeConfig) {
    let canonical = MetadataSources::default();
    append_missing(&mut cfg.metadata.sources.movies, &canonical.movies);
    append_missing(&mut cfg.metadata.sources.series, &canonical.series);
    append_missing(&mut cfg.metadata.sources.anime,  &canonical.anime);
    append_missing(&mut cfg.metadata.sources.music,  &canonical.music);
}

fn append_missing(user: &mut Vec<String>, canonical: &[String]) {
    for item in canonical {
        if !user.iter().any(|u| u == item) {
            user.push(item.clone());
        }
    }
}

fn default_config_path() -> Option<PathBuf> {
    // Runtime config sits next to the TUI's config.toml under
    // ~/.config/stui/. Distinct filename keeps the two schemas
    // (TUI Config vs RuntimeConfig) cleanly separated; merging them
    // is a future call once the overlap (playback/streaming/mpd) is
    // factored out.
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .map(|c| c.join("stui").join("runtime.toml"))
}

fn apply_secrets(cfg: &mut RuntimeConfig) {
    let secrets = Secrets::load();

    if let Some(key) = secrets.tmdb_api_key() {
        cfg.api_keys.tmdb = Some(key);
    }
    if let Some(key) = secrets.omdb_api_key() {
        cfg.api_keys.omdb = Some(key);
    }
    if let Some(pwd) = secrets.mpd_password() {
        cfg.mpd.password = Some(pwd);
    }

    debug!(
        "secrets applied: tmdb={}, omdb={}, mpd={}",
        cfg.api_keys.tmdb.is_some(),
        cfg.api_keys.omdb.is_some(),
        cfg.mpd.password.is_some()
    );
}

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
    if let Ok(v) = std::env::var("STUI_PLUGIN_REPOS") {
        let repos: Vec<String> = v
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !repos.is_empty() {
            cfg.plugin_repos = repos;
        }
    }
}

#[cfg(test)]
mod merge_defaults_tests {
    use super::*;
    use crate::config::types::*;

    #[test]
    fn appends_missing_tvdb_for_movies() {
        let toml = r#"
[metadata.sources]
movies = ["tmdb"]
        "#;
        let mut cfg: RuntimeConfig = toml::from_str(toml).unwrap();
        merge_metadata_source_defaults(&mut cfg);
        assert_eq!(
            cfg.metadata.sources.movies,
            vec!["tmdb", "omdb", "tvdb", "fanart"]
        );
    }

    #[test]
    fn preserves_user_ordering() {
        let toml = r#"
[metadata.sources]
movies = ["omdb", "tmdb"]
        "#;
        let mut cfg: RuntimeConfig = toml::from_str(toml).unwrap();
        merge_metadata_source_defaults(&mut cfg);
        assert_eq!(
            cfg.metadata.sources.movies,
            vec!["omdb", "tmdb", "tvdb", "fanart"]
        );
    }

    #[test]
    fn idempotent_when_already_complete() {
        // "Complete" tracks the canonical default set — if a new source is
        // added (fanart was the last one), the fixture has to grow with it
        // or the merge will append the missing entry and the assertion
        // will fail. Lock the fixture to whatever `MetadataSources::default()`
        // currently returns rather than maintaining a hand-coded mirror.
        let canonical = MetadataSources::default().movies;
        let toml = format!(
            "[metadata.sources]\nmovies = {}\n",
            serde_json::to_string(&canonical).unwrap(),
        );
        let mut cfg: RuntimeConfig = toml::from_str(&toml).unwrap();
        let before = cfg.metadata.sources.movies.clone();
        merge_metadata_source_defaults(&mut cfg);
        assert_eq!(cfg.metadata.sources.movies, before);
    }
}
