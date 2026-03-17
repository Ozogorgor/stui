//! Registry pipeline — browse + install from plugin registries.

use std::collections::HashSet;
use std::sync::Arc;

use tracing::info;

use crate::config::ConfigManager;
use crate::ipc::{
    ErrorCode, InstallPluginRequest, PluginInstalledResponse, RegistryEntryWire,
    RegistryIndexResponse, Response,
};
use crate::registry;

// ── Browse ────────────────────────────────────────────────────────────────────

/// Fetch the merged plugin index from every configured registry URL.
///
/// Cross-references the plugin_dir to mark each entry as `installed` when a
/// subdirectory with the same name already exists.
pub async fn run_browse_registry(config: &Arc<ConfigManager>) -> Response {
    let cfg       = config.snapshot().await;
    let repos     = &cfg.plugin_repos;
    let plugin_dir = &cfg.plugin_dir;

    // Collect names of already-installed plugins by listing plugin_dir.
    let installed_names: HashSet<String> = if plugin_dir.exists() {
        std::fs::read_dir(plugin_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let e = e.ok()?;
                if e.path().is_dir() {
                    e.file_name().into_string().ok()
                } else {
                    None
                }
            })
            .collect()
    } else {
        HashSet::new()
    };

    // Fetch from each repo; track failures.
    let mut failed_repos: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries_wire: Vec<RegistryEntryWire> = Vec::new();

    for repo in repos {
        match registry::fetch_index(repo).await {
            Ok(entries) => {
                for e in entries {
                    if seen.insert(e.name.clone()) {
                        let installed = installed_names.contains(&e.name);
                        entries_wire.push(RegistryEntryWire {
                            name:        e.name,
                            version:     e.version,
                            plugin_type: e.plugin_type,
                            description: e.description,
                            author:      e.author,
                            homepage:    e.homepage,
                            binary_url:  e.binary_url,
                            checksum:    e.checksum,
                            installed,
                        });
                    }
                }
            }
            Err(err) => {
                tracing::warn!(%repo, error = %err, "registry fetch failed");
                failed_repos.push(repo.clone());
            }
        }
    }

    entries_wire.sort_by(|a, b| a.name.cmp(&b.name));

    Response::RegistryIndex(RegistryIndexResponse {
        entries: entries_wire,
        failed_repos,
    })
}

// ── Install ───────────────────────────────────────────────────────────────────

/// Download, verify, and install a plugin from a registry entry.
///
/// The plugin is extracted to `{plugin_dir}/{name}/`.
/// The hot-reload watcher in `discovery.rs` picks it up automatically.
pub async fn run_install_plugin(config: &Arc<ConfigManager>, r: InstallPluginRequest) -> Response {
    let plugin_dir = config.snapshot().await.plugin_dir;

    let entry = registry::RegistryEntry {
        name:        r.name.clone(),
        version:     r.version.clone(),
        plugin_type: String::new(),
        description: String::new(),
        author:      String::new(),
        homepage:    None,
        binary_url:  r.binary_url,
        checksum:    r.checksum,
    };

    match registry::download_and_install(&entry, &plugin_dir).await {
        Ok(path) => {
            info!(name = %r.name, version = %r.version, "plugin install complete");
            Response::PluginInstalled(PluginInstalledResponse {
                name:    r.name,
                version: r.version,
                path:    path.to_string_lossy().into_owned(),
            })
        }
        Err(err) => Response::error(None, ErrorCode::PluginLoadFailed, err.to_string()),
    }
}
