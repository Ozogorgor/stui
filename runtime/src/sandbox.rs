/// Sandbox — permission enforcement layer around plugin execution.
///
/// Every plugin call passes through the sandbox, which:
///   1. Checks declared permissions against the requested capability
///   2. Enforces network isolation (blocks undeclared outbound calls)
///   3. Scopes filesystem access to allowed directories only
///   4. (Future) routes WASM plugins through a wasmtime instance
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use tracing::{debug, warn};

use crate::plugin::{ExecutionMode, LoadedPlugin, Permissions};

// ── Capability types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Network,
    FilesystemRead(PathBuf),
    FilesystemWrite(PathBuf),
}

// ── Sandbox context ──────────────────────────────────────────────────────────

/// A per-plugin sandbox context. Constructed when the plugin is loaded.
#[derive(Debug, Clone)]
pub struct SandboxCtx {
    pub plugin_id: String,
    pub plugin_name: String,
    pub permissions: Permissions,
    pub mode: ExecutionMode,
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Default env var values declared in plugin.toml [env].
    /// Key = var name (e.g. "PROWLARR_API_KEY"), value = default string.
    pub env_defaults: std::collections::HashMap<String, String>,
}

impl SandboxCtx {
    pub fn new(plugin: &LoadedPlugin, cache_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            plugin_id: plugin.id.clone(),
            plugin_name: plugin.manifest.plugin.name.clone(),
            permissions: plugin
                .manifest
                .permissions
                .clone()
                .unwrap_or_default(),
            mode: plugin.mode.clone(),
            cache_dir,
            data_dir,
            env_defaults: plugin.manifest.env.clone(),
        }
    }

    /// Check whether this plugin is allowed to use a given capability.
    pub fn check(&self, cap: &Capability) -> Result<()> {
        match cap {
            Capability::Network => {
                if !self.permissions.network {
                    warn!(
                        plugin = %self.plugin_name,
                        "blocked network access — not declared in permissions"
                    );
                    bail!(
                        "Plugin '{}' does not have network permission",
                        self.plugin_name
                    );
                }
                debug!(plugin = %self.plugin_name, "network access granted");
                Ok(())
            }
            Capability::FilesystemRead(path) | Capability::FilesystemWrite(path) => {
                let allowed = self.allowed_fs_roots();
                let permitted = allowed.iter().any(|root| path.starts_with(root));
                if !permitted {
                    warn!(
                        plugin = %self.plugin_name,
                        path = %path.display(),
                        "blocked filesystem access"
                    );
                    bail!(
                        "Plugin '{}' does not have filesystem access to {}",
                        self.plugin_name,
                        path.display()
                    );
                }
                debug!(plugin = %self.plugin_name, path = %path.display(), "fs access granted");
                Ok(())
            }
        }
    }

    fn allowed_fs_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for scope in &self.permissions.filesystem {
            match scope.as_str() {
                "cache" => roots.push(self.cache_dir.clone()),
                "data"  => roots.push(self.data_dir.clone()),
                other   => {
                    warn!(plugin = %self.plugin_name, scope = %other, "unknown fs scope — ignored");
                }
            }
        }
        roots
    }

    pub fn http_client(&self) -> Result<reqwest::Client> {
        self.check(&Capability::Network)?;
        let client = reqwest::Client::builder()
            .user_agent(format!(
                "stui-runtime/{} (plugin: {})",
                env!("CARGO_PKG_VERSION"),
                self.plugin_name
            ))
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(client)
    }

    pub fn plugin_cache_dir(&self) -> PathBuf {
        self.cache_dir.join(&self.plugin_name)
    }

    pub fn plugin_data_dir(&self) -> PathBuf {
        self.data_dir.join(&self.plugin_name)
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        if self.permissions.filesystem.contains(&"cache".to_string()) {
            std::fs::create_dir_all(self.plugin_cache_dir())?;
        }
        if self.permissions.filesystem.contains(&"data".to_string()) {
            std::fs::create_dir_all(self.plugin_data_dir())?;
        }
        Ok(())
    }
}

// ── WASM execution boundary (stub — wasmtime integration in Phase 2) ─────────

pub async fn call_wasm(
    ctx: &SandboxCtx,
    _wasm_path: &Path,
    _fn_name: &str,
    _input: &serde_json::Value,
) -> Result<serde_json::Value> {
    bail!(
        "WASM execution not yet implemented. \
         Plugin '{}' requires wasmtime integration (Phase 2).",
        ctx.plugin_name
    )
}
