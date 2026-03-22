//! `ConfigManager` — runtime configuration with live-update support.
//!
//! The `ConfigManager` owns the authoritative `RuntimeConfig` and is the
//! single place where configuration changes happen.  When the TUI sends a
//! `SetConfig` IPC request, the IPC handler calls
//! `config_manager.set(key, value)` instead of mutating config directly.
//!
//! # Live updates
//!
//! When a value changes, `ConfigManager` broadcasts a `ConfigChanged` event
//! on the `EventBus`.  Subscribers (player, providers, ranking engine) can
//! react without restarting:
//!
//! ```text
//! TUI  ─── SetConfig("player.default_volume", 80) ──►  IPC handler
//!                                                            │
//!                                                    ConfigManager.set(…)
//!                                                            │
//!                                                    RuntimeConfig updated
//!                                                            │
//!                                                    EventBus.emit(ConfigChanged)
//!                                                            │
//!                                              ┌────────────┴─────────────┐
//!                                         PlayerManager              ProviderEngine
//!                                       (adjusts volume)         (re-reads timeout)
//! ```
//!
//! # Supported key paths
//!
//! The key is a dot-separated path matching the `RuntimeConfig` field layout:
//!
//! | Key                              | Type     | Effect                     |
//! |----------------------------------|----------|----------------------------|
//! | `player.default_volume`          | `u32`    | Sets mpv default volume    |
//! | `player.hwdec`                   | `String` | Sets mpv hwdec on next play|
//! | `player.cache_secs`              | `u32`    | Updates cache setting      |
//! | `streaming.prefer_http`          | `bool`   | Adjusts stream selection   |
//! | `streaming.auto_fallback`        | `bool`   | Enables/disables fallback  |
//! | `streaming.max_candidates`       | `usize`  | Updates resolve limit      |
//! | `subtitles.auto_download`        | `bool`   | Enables subtitle fetch     |
//! | `subtitles.preferred_language`   | `String` | Updates subtitle priority  |
//! | `subtitles.default_delay`        | `f64`    | Changes default delay      |
//! | `providers.enable_tmdb`          | `bool`   | Toggles TMDB               |
//! | `providers.enable_torrentio`     | `bool`   | Toggles Torrentio          |
//! | `app.theme_mode`                 | `String` | Changes theme              |

#![allow(dead_code)]

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{info, warn};

fn default_config_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".stui").join("config").join("stui.toml"))
}

use crate::config::types::RuntimeConfig;
use crate::error::{Result, StuidError};
use crate::events::{EventBus, RuntimeEvent};

// ── ConfigManager ─────────────────────────────────────────────────────────────

/// Thread-safe wrapper around `RuntimeConfig` with live-update broadcasting.
///
/// Cheap to clone — wraps `Arc`.
#[derive(Clone)]
pub struct ConfigManager {
    config: Arc<RwLock<RuntimeConfig>>,
    bus:    Arc<EventBus>,
}

impl ConfigManager {
    /// Create a new `ConfigManager` from an initial config.
    pub fn new(config: RuntimeConfig, bus: Arc<EventBus>) -> Self {
        ConfigManager {
            config: Arc::new(RwLock::new(config)),
            bus,
        }
    }

    // ── Read ──────────────────────────────────────────────────────────────

    /// Return a snapshot of the current config.
    ///
    /// Cheap — clones the struct (all fields are `Clone`).
    pub async fn snapshot(&self) -> RuntimeConfig {
        self.config.read().await.clone()
    }

    // ── Write ─────────────────────────────────────────────────────────────

    /// Apply a live config update from a `SetConfig` IPC request.
    ///
    /// `key` is a dot-separated path (e.g. `"player.default_volume"`).
    /// `value` is a `serde_json::Value` that will be type-checked.
    ///
    /// Returns `Ok(())` on success and broadcasts `ConfigChanged`.
    /// Returns `Err(StuidError::Config)` if the key is unknown or the value
    /// cannot be coerced to the expected type.
    ///
    /// API key changes (`api_keys.*`) are automatically persisted to disk.
    pub async fn set(&self, key: &str, value: Value) -> Result<()> {
        {
            let mut cfg = self.config.write().await;
            apply_key(&mut cfg, key, &value)?;
        }

        info!(key, value = %value, "config updated");

        self.bus.emit(RuntimeEvent::ConfigChanged {
            key:   key.to_string(),
            value: value.to_string(),
        });

        // Persist API key and plugin config changes immediately so they survive restarts.
        if key.starts_with("api_keys.") || key.starts_with("plugins.") {
            self.persist().await.map_err(|e| {
                warn!(key, error = %e, "failed to persist config after plugin config update");
                e
            })?;
        }

        Ok(())
    }

    /// Write the current config snapshot to `~/.stui/config/stui.toml`.
    pub async fn persist(&self) -> Result<()> {
        let cfg = self.config.read().await.clone();

        let Some(path) = default_config_path() else {
            return Err(StuidError::config("cannot determine config path (no home dir)"));
        };

        let text = toml::to_string_pretty(&cfg)
            .map_err(|e| StuidError::config(format!("config serialize: {e}")))?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| StuidError::config(format!("create config dir: {e}")))?;
        }

        tokio::fs::write(&path, text).await
            .map_err(|e| StuidError::config(format!("write config {}: {e}", path.display())))?;

        info!(path = %path.display(), "config persisted");
        Ok(())
    }

    /// Convenience: set a `bool` value.
    pub async fn set_bool(&self, key: &str, v: bool) -> Result<()> {
        self.set(key, Value::Bool(v)).await
    }

    /// Convenience: set a numeric value.
    pub async fn set_number(&self, key: &str, v: f64) -> Result<()> {
        self.set(key, serde_json::json!(v)).await
    }

    /// Convenience: set a string value.
    pub async fn set_str(&self, key: &str, v: &str) -> Result<()> {
        self.set(key, Value::String(v.to_string())).await
    }

    /// Replace the plugin repo list.
    ///
    /// Always ensures the built-in repo is present as the first entry.
    /// Persists the change to disk and broadcasts `ConfigChanged`.
    pub async fn set_plugin_repos(&self, mut repos: Vec<String>) -> Result<()> {
        const BUILTIN: &str = "https://plugins.stui.dev";
        
        // Normalize URLs: strip trailing slashes, deduplicate
        repos = repos
            .into_iter()
            .map(|r| r.trim_end_matches('/').to_string())
            .collect();
        
        // Remove any existing copy of the built-in URL so we can prepend it once.
        repos.retain(|r| r != BUILTIN);
        repos.insert(0, BUILTIN.to_string());

        {
            let mut cfg = self.config.write().await;
            cfg.plugin_repos = repos.clone();
        }

        info!(?repos, "plugin repos updated");

        self.bus.emit(RuntimeEvent::ConfigChanged {
            key:   "plugin_repos".to_string(),
            value: repos.join(","),
        });

        self.persist().await.map_err(|e| {
            warn!(error = %e, "failed to persist config after plugin_repos update");
            e
        })?;

        Ok(())
    }
}

// ── Key application logic ─────────────────────────────────────────────────────

/// Apply a dot-separated key path to a mutable `RuntimeConfig`.
///
/// Returns an error if the key is unknown or the value is the wrong type.
fn apply_key(cfg: &mut RuntimeConfig, key: &str, value: &Value) -> Result<()> {
    match key {
        // ── [player] ──────────────────────────────────────────────────────
        "player.default_volume" => {
            cfg.playback.default_volume = as_f64(key, value)?;
        }
        "player.hwdec" => {
            cfg.playback.hwdec = as_string(key, value)?;
        }
        "player.cache_secs" => {
            cfg.playback.cache_secs = as_u32(key, value)?;
        }
        "player.demuxer_max_mb" => {
            cfg.playback.demuxer_max_mb = as_u32(key, value)?;
        }
        "player.keep_open" => {
            cfg.playback.keep_open = as_bool(key, value)?;
        }

        // ── [streaming] ───────────────────────────────────────────────────
        "streaming.prefer_http" => {
            cfg.streaming.prefer_http = as_bool(key, value)?;
        }
        "streaming.prefer_torrent" => {
            cfg.streaming.prefer_torrent = as_bool(key, value)?;
        }
        "streaming.max_candidates" => {
            cfg.streaming.max_candidates = as_usize(key, value)?;
        }
        "streaming.auto_fallback" => {
            cfg.streaming.auto_fallback = as_bool(key, value)?;
        }
        "streaming.benchmark_streams" => {
            cfg.streaming.benchmark_streams = as_bool(key, value)?;
        }

        // ── [subtitles] ───────────────────────────────────────────────────
        "subtitles.auto_download" => {
            cfg.subtitles.auto_download = as_bool(key, value)?;
        }
        "subtitles.preferred_language" => {
            cfg.subtitles.preferred_language = as_string(key, value)?;
        }
        "subtitles.default_delay" => {
            cfg.subtitles.default_delay = as_f64(key, value)?;
        }

        // ── [providers] ───────────────────────────────────────────────────
        "providers.enable_tmdb" => {
            cfg.providers.enable_tmdb = as_bool(key, value)?;
        }
        "providers.enable_omdb" => {
            cfg.providers.enable_omdb = as_bool(key, value)?;
        }
        "providers.enable_imdb" => {
            cfg.providers.enable_imdb = as_bool(key, value)?;
        }
        "providers.enable_torrentio" => {
            cfg.providers.enable_torrentio = as_bool(key, value)?;
        }
        "providers.enable_prowlarr" => {
            cfg.providers.enable_prowlarr = as_bool(key, value)?;
        }
        "providers.enable_opensubtitles" => {
            cfg.providers.enable_opensubtitles = as_bool(key, value)?;
        }

        // ── [api_keys] ────────────────────────────────────────────────────
        "api_keys.tmdb" => {
            cfg.api_keys.tmdb = Some(as_string(key, value)?);
        }
        "api_keys.omdb" => {
            cfg.api_keys.omdb = Some(as_string(key, value)?);
        }

        // ── [app] ─────────────────────────────────────────────────────────
        "app.theme_mode" => {
            cfg.theme_mode = as_string(key, value)?;
        }
        "app.log_level" => {
            cfg.logging.level = as_string(key, value)?;
            // Note: changing log level at runtime requires re-initialising
            // the tracing subscriber, which is not supported here.
            warn!("app.log_level change takes effect on next restart");
        }

        // ── [skipper] ─────────────────────────────────────────────────────
        "skipper.enabled" => {
            cfg.skipper.enabled = as_bool(key, value)?;
        }
        "skipper.auto_skip_intro" => {
            cfg.skipper.auto_skip_intro = as_bool(key, value)?;
        }
        "skipper.auto_skip_credits" => {
            cfg.skipper.auto_skip_credits = as_bool(key, value)?;
        }
        "skipper.similarity_threshold" => {
            cfg.skipper.similarity_threshold = as_f64(key, value)?;
        }
        "skipper.min_episodes" => {
            cfg.skipper.min_episodes = as_usize(key, value)?;
        }
        "skipper.intro_scan_secs" => {
            cfg.skipper.intro_scan_secs = as_u32(key, value)?;
        }
        "skipper.min_intro_secs" => {
            cfg.skipper.min_intro_secs = as_f64(key, value)?;
        }
        "skipper.max_intro_secs" => {
            cfg.skipper.max_intro_secs = as_f64(key, value)?;
        }

        // ── [storage] ──────────────────────────────────────────────────────
        "storage.movies" => {
            cfg.storage.movies = as_pathbuf(key, value)?;
        }
        "storage.series" => {
            cfg.storage.series = as_pathbuf(key, value)?;
        }
        "storage.music" => {
            cfg.storage.music = as_pathbuf(key, value)?;
        }
        "storage.anime" => {
            cfg.storage.anime = as_pathbuf(key, value)?;
        }
        "storage.podcasts" => {
            cfg.storage.podcasts = as_pathbuf(key, value)?;
        }

        // ── [plugins.*] ────────────────────────────────────────────────────
        // Dynamic plugin config keys: "plugins.{plugin_name}.{field_key}"
        other if other.starts_with("plugins.") => {
            apply_plugin_key(cfg, other, value)?;
        }

        other => {
            return Err(StuidError::config(format!("unknown config key: {other}")));
        }
    }

    Ok(())
}

// ── Type coercion helpers ─────────────────────────────────────────────────────

fn apply_plugin_key(cfg: &mut RuntimeConfig, key: &str, value: &Value) -> Result<()> {
    // Format: "plugins.{plugin_name}.{field_key}"
    let parts: Vec<&str> = key.splitn(4, '.').collect();
    if parts.len() != 4 || parts[0] != "plugins" {
        return Err(StuidError::config(format!(
            "invalid plugin config key format: {key} (expected plugins.{{name}}.{{field}})"
        )));
    }

    let plugin_name = parts[1];
    let field_key = parts[2];
    let string_value = as_string(key, value)?;

    cfg.plugins
        .entry(plugin_name.to_string())
        .or_default()
        .insert(field_key.to_string(), string_value);

    Ok(())
}

fn as_bool(key: &str, v: &Value) -> Result<bool> {
    v.as_bool().ok_or_else(|| {
        StuidError::config(format!("{key}: expected bool, got {v}"))
    })
}

fn as_f64(key: &str, v: &Value) -> Result<f64> {
    v.as_f64().ok_or_else(|| {
        StuidError::config(format!("{key}: expected number, got {v}"))
    })
}

fn as_u32(key: &str, v: &Value) -> Result<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| {
            v.as_f64().and_then(|f| {
                if f.fract() == 0.0 && f >= 0.0 && f <= u32::MAX as f64 {
                    Some(f as u32)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            StuidError::config(format!("{key}: expected u32, got {v}"))
        })
}

fn as_usize(key: &str, v: &Value) -> Result<usize> {
    v.as_u64()
        .map(|n| n as usize)
        .or_else(|| {
            v.as_f64().and_then(|f| {
                if f.fract() == 0.0 && f >= 0.0 && f <= usize::MAX as f64 {
                    Some(f as usize)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            StuidError::config(format!("{key}: expected usize, got {v}"))
        })
}

fn as_string(key: &str, v: &Value) -> Result<String> {
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            StuidError::config(format!("{key}: expected string, got {v}"))
        })
}

fn as_pathbuf(key: &str, v: &Value) -> Result<std::path::PathBuf> {
    v.as_str()
        .map(|s| std::path::PathBuf::from(s))
        .ok_or_else(|| {
            StuidError::config(format!("{key}: expected path string, got {v}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;

    fn make_manager() -> ConfigManager {
        let bus = Arc::new(EventBus::new());
        ConfigManager::new(RuntimeConfig::default(), bus)
    }

    #[tokio::test]
    async fn set_volume_updates_config() {
        let m = make_manager();
        m.set_number("player.default_volume", 80.0).await.unwrap();
        let snap = m.snapshot().await;
        assert!((snap.playback.default_volume - 80.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn set_bool_updates_config() {
        let m = make_manager();
        m.set_bool("streaming.auto_fallback", false).await.unwrap();
        let snap = m.snapshot().await;
        assert!(!snap.streaming.auto_fallback);
    }

    #[tokio::test]
    async fn unknown_key_returns_error() {
        let m = make_manager();
        let result = m.set_str("player.nonexistent", "x").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn wrong_type_returns_error() {
        let m = make_manager();
        // volume expects a number, not a string
        let result = m.set("player.default_volume", Value::String("loud".into())).await;
        assert!(result.is_err());
    }
}
