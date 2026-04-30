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
#[allow(dead_code)] // pub API: plugin sandbox capability checks
pub enum Capability {
    Network,
    FilesystemRead(PathBuf),
    FilesystemWrite(PathBuf),
}

// ── Sandbox context ──────────────────────────────────────────────────────────

/// A per-plugin sandbox context. Constructed when the plugin is loaded.
#[derive(Debug, Clone)]
#[allow(dead_code)] // pub API: plugin sandbox capability checks
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
    /// User-supplied env var overrides resolved from `runtime.toml [plugins.<name>]`.
    /// Highest precedence: wins over both `secrets.env` and process env when
    /// populating the plugin's `__env:<VAR>` cache. Key = env var name (e.g.
    /// "JACKETT_API_KEY"), value = the user's TUI-entered string.
    pub user_env_overrides: std::collections::HashMap<String, String>,
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
            user_env_overrides: std::collections::HashMap::new(),
        }
    }

    /// Attach user-supplied env overrides (from runtime.toml `[plugins.<name>]`).
    /// Builder-style so call sites can chain after `SandboxCtx::new(...)`.
    pub fn with_user_env_overrides(
        mut self,
        overrides: std::collections::HashMap<String, String>,
    ) -> Self {
        self.user_env_overrides = overrides;
        self
    }

    /// Check whether this plugin is allowed to use a given capability.
    ///
    /// Called from `host.rs` before any outbound HTTP call.  A plugin passes
    /// the coarse network check if it has `network = true` OR a non-empty
    /// `network_hosts` allowlist; the fine-grained per-host check is then
    /// applied by `Permissions::allows_host()`.
    pub fn check(&self, cap: &Capability) -> Result<()> {
        match cap {
            Capability::Network => {
                let has_network = self.permissions.network.is_enabled()
                    || !self.permissions.network_hosts.is_empty();
                if !has_network {
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
                let permitted = allowed.iter().any(|root| {
                    let path_exists = path.exists();
                    let resolved_path = if path_exists {
                        path.canonicalize().ok()
                    } else {
                        // For non-existent paths (e.g., new file writes),
                        // canonicalize the parent and append the filename
                        path.parent()
                            .and_then(|p| p.canonicalize().ok())
                            .map(|parent| {
                                parent.join(path.file_name().unwrap_or_default())
                            })
                    };
                    let resolved_root = root.canonicalize().ok();
                    
                    match (resolved_path, resolved_root) {
                        (Some(resolved), Some(base)) => resolved.starts_with(&base),
                        _ => false,
                    }
                });
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

    pub fn allowed_fs_roots(&self) -> Vec<PathBuf> {
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

    #[allow(dead_code)] // pub API: plugin sandbox capability checks
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{ExecutionMode, NetworkPermission, Permissions};

    fn make_ctx(perms: Permissions) -> SandboxCtx {
        SandboxCtx {
            plugin_id: "test-id".to_string(),
            plugin_name: "test-plugin".to_string(),
            permissions: perms,
            mode: ExecutionMode::Wasm,
            cache_dir: PathBuf::from("/tmp/stui-test/cache"),
            data_dir: PathBuf::from("/tmp/stui-test/data"),
            env_defaults: std::collections::HashMap::new(),
            user_env_overrides: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn network_denied_when_no_permission() {
        // Plugin with network = false and no host allowlist must be blocked.
        let ctx = make_ctx(Permissions {
            network: NetworkPermission::Bool(false),
            network_hosts: vec![],
            filesystem: vec![],
        });
        assert!(
            ctx.check(&Capability::Network).is_err(),
            "network check must fail when network = false and no network_hosts"
        );
    }

    #[test]
    fn network_allowed_with_flag() {
        let ctx = make_ctx(Permissions {
            network: NetworkPermission::Bool(true),
            network_hosts: vec![],
            filesystem: vec![],
        });
        assert!(ctx.check(&Capability::Network).is_ok());
    }

    #[test]
    fn network_allowed_with_host_allowlist_even_without_flag() {
        // network_hosts allowlist is sufficient — network = true not required.
        let ctx = make_ctx(Permissions {
            network: NetworkPermission::Bool(false),
            network_hosts: vec!["api.example.com".to_string()],
            filesystem: vec![],
        });
        assert!(
            ctx.check(&Capability::Network).is_ok(),
            "coarse check should pass when network_hosts is non-empty"
        );
    }

    #[test]
    fn filesystem_denied_outside_allowed_roots() {
        let ctx = make_ctx(Permissions {
            network: NetworkPermission::Bool(false),
            network_hosts: vec![],
            filesystem: vec![],
        });
        let path = PathBuf::from("/etc/passwd");
        assert!(ctx.check(&Capability::FilesystemRead(path)).is_err());
    }
}
