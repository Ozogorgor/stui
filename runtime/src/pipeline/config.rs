//! Config pipeline — live config updates, provider settings, plugin repos.

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::config::ConfigManager;
use crate::ipc::{
    ErrorCode, PluginReposResponse, ProviderSchema, ProviderSettingsResponse,
    Response, SetConfigRequest, SetPluginReposRequest,
};

// ── SetConfig ─────────────────────────────────────────────────────────────────

/// Apply a live `SetConfig` request.
///
/// Validates the key/value via `ConfigManager`, persists API key changes, and
/// broadcasts `ConfigChanged` on the event bus.
pub async fn run_set_config(config: &Arc<ConfigManager>, r: SetConfigRequest) -> Response {
    match config.set(&r.key, r.value).await {
        Ok(()) => Response::ConfigUpdated { key: r.key },
        Err(e) => Response::error(None, ErrorCode::InvalidRequest, e.to_string()),
    }
}

// ── Provider settings ─────────────────────────────────────────────────────────

/// Return the self-declared config schema for every active catalog provider.
///
/// Each provider implementing the `Provider` trait advertises its own fields —
/// no hardcoded list here. New providers appear automatically.
pub async fn run_get_provider_settings(catalog: &Arc<Catalog>) -> Response {
    let providers: Vec<ProviderSchema> = catalog
        .providers()
        .iter()
        .map(|p| ProviderSchema {
            id:          p.name().to_string(),
            name:        p.display_name().to_string(),
            description: p.description().to_string(),
            active:      p.is_active(),
            fields:      p.config_schema(),
        })
        .collect();
    Response::ProviderSettings(ProviderSettingsResponse { providers })
}

// ── Plugin repos ──────────────────────────────────────────────────────────────

/// Return the current plugin repository list (built-in always first).
pub async fn run_get_plugin_repos(config: &Arc<ConfigManager>) -> Response {
    let repos = config.snapshot().await.plugin_repos;
    Response::PluginRepos(PluginReposResponse { repos })
}

/// Replace the plugin repository list.
///
/// The built-in repo is automatically preserved as the first entry.
/// Change is persisted to `~/.stui/config/stui.toml` immediately.
pub async fn run_set_plugin_repos(config: &Arc<ConfigManager>, r: SetPluginReposRequest) -> Response {
    match config.set_plugin_repos(r.repos).await {
        Ok(()) => {
            let repos = config.snapshot().await.plugin_repos;
            Response::PluginRepos(PluginReposResponse { repos })
        }
        Err(e) => Response::error(None, ErrorCode::InvalidRequest, e.to_string()),
    }
}
