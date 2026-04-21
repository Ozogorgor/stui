//! Config pipeline — live config updates, provider settings, plugin repos.

use std::sync::Arc;

use crate::config::ConfigManager;
use crate::engine::Engine;
use crate::ipc::{
    ErrorCode, PluginReposResponse, ProviderField, ProviderSchema, ProviderSettingsResponse,
    Response, SetConfigRequest, SetPluginReposRequest,
};
use crate::plugin::PluginMetaExt;

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

/// Return the config schema for all loaded plugins.
///
/// Provider settings are loaded from WASM plugins via the Engine's plugin registry.
/// Each plugin declares its config fields in `plugin.toml` under `[config]`,
/// or they are auto-generated from `[env]` variables.
pub async fn run_get_provider_settings(
    engine: &Arc<Engine>,
    config: &Arc<ConfigManager>,
) -> Response {
    let registry = engine.registry().read().await;
    let config_snapshot = config.snapshot().await;
    
    let providers: Vec<ProviderSchema> = registry
        .all_plugins()
        .filter(|p| {
            let meta = &p.manifest.plugin;
            meta.is_metadata_provider() || meta.is_stream_provider() || meta.is_subtitle_provider()
        })
        .map(|plugin| {
            let plugin_name = &plugin.manifest.plugin.name;
            let fields: Vec<ProviderField> = plugin.manifest.config_fields()
                .into_iter()
                .map(|field| {
                    let full_key = field.full_key(plugin_name);
                    // Get current value - first check config, then env var
                    let value = config_snapshot
                        .plugins
                        .get(plugin_name)
                        .and_then(|p| p.get(&field.key).cloned())
                        .or_else(|| {
                            // Check environment variable (e.g., TMDB_API_KEY for tmdb-provider)
                            let env_key = format!("{}_{}", plugin_name.to_uppercase(), field.key.replace('-', "_"));
                            std::env::var(&env_key).ok()
                        })
                        .unwrap_or_default();
                    
                    let configured = !value.is_empty();
                    
                    ProviderField {
                        key: full_key,
                        label: field.label,
                        hint: field.hint.unwrap_or_default(),
                        masked: field.masked,
                        configured,
                        required: field.required,
                        value,
                    }
                })
                .collect();
            
            let active = !fields.is_empty() && fields.iter().all(|f| !f.required || f.configured);
            
            ProviderSchema {
                id: plugin_name.clone(),
                name: plugin.manifest.plugin.name.clone(),
                description: plugin.manifest.plugin.description.clone().unwrap_or_default(),
                plugin_type: plugin.manifest.plugin.plugin_type_str(),
                active,
                fields,
            }
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
