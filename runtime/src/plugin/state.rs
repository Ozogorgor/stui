//! Plugin lifecycle state: `PluginStatus`, `PluginState`, and `StateStore`.
//!
//! The existing `engine::PluginRegistry` (a runtime-only concept that also
//! holds `SandboxCtx` + `WasmSupervisor`) is kept as-is for now — this module
//! adds the 4-state authoritative status tracking on top, for plugins whose
//! lifecycle goes through the new `Plugin::init` flow.

#![allow(dead_code)]

use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::loader::LoadedPlugin;
use super::manifest::PluginManifest;

// ── PluginStatus ──────────────────────────────────────────────────────────────

/// Four-state lifecycle status for a plugin. Used by the TUI plugins panel
/// and by the loader when reporting init() outcomes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PluginStatus {
    Loaded,
    NeedsConfig {
        missing: Vec<String>,
        hint: Option<String>,
    },
    Failed {
        reason: String,
        at: SystemTime,
    },
    Disabled,
}

// ── PluginState ───────────────────────────────────────────────────────────────

/// Live state for one plugin tracked by `StateStore`.
#[derive(Debug, Clone)]
pub struct PluginState {
    pub manifest: PluginManifest,
    pub status: PluginStatus,
    /// Resolved env vars for this plugin (after config precedence).
    pub resolved_env: HashMap<String, String>,
}

// ── StateStore ────────────────────────────────────────────────────────────────

/// Store of per-plugin `PluginState`, keyed by manifest `name`.
#[derive(Debug, Default)]
pub struct StateStore {
    states: HashMap<String, PluginState>,
}

impl StateStore {
    pub fn new() -> Self { Self::default() }

    /// Insert or replace a plugin's state. Called on load / reload.
    pub fn insert(&mut self, state: PluginState) {
        let name = state.manifest.plugin.name.clone();
        self.states.insert(name, state);
    }

    /// Remove a plugin's state. Called on unload.
    pub fn remove(&mut self, name: &str) -> Option<PluginState> {
        self.states.remove(name)
    }

    /// Get a plugin's state by manifest name.
    pub fn get(&self, name: &str) -> Option<&PluginState> {
        self.states.get(name)
    }

    /// Iterate over all registered plugin states.
    pub fn list(&self) -> impl Iterator<Item = &PluginState> {
        self.states.values()
    }

    /// Get the current status of a plugin, or `None` if not registered.
    pub fn status(&self, name: &str) -> Option<&PluginStatus> {
        self.states.get(name).map(|s| &s.status)
    }

    /// Update the status of an existing plugin. Returns true if the plugin
    /// was found and updated.
    pub fn set_status(&mut self, name: &str, status: PluginStatus) -> bool {
        if let Some(s) = self.states.get_mut(name) {
            s.status = status;
            true
        } else {
            false
        }
    }

    /// Reload a plugin's state (replace with a freshly-parsed manifest, etc.).
    pub fn reload(&mut self, state: PluginState) {
        self.insert(state);
    }

    /// Resolve the effective config/env for a plugin by applying the four-level
    /// precedence defined in spec §2:
    ///
    ///   1. user TUI settings    (highest — explicitly set via the TUI)
    ///   2. `[[config]] env_var` (actual value of the referenced env var)
    ///   3. `[env]` manifest default
    ///   4. `[[config]] default` (lowest — from the manifest itself)
    ///
    /// `user_config` supplies (1); `env_lookup` is invoked for (2).
    pub fn resolve_config<F>(
        manifest: &PluginManifest,
        user_config: &HashMap<String, String>,
        env_lookup: F,
    ) -> HashMap<String, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        resolve_config(manifest, user_config, env_lookup)
    }
}

/// Free-standing `resolve_config` so callers that don't own a `StateStore`
/// (e.g. the loader) can use it too.
pub fn resolve_config<F>(
    manifest: &PluginManifest,
    user_config: &HashMap<String, String>,
    env_lookup: F,
) -> HashMap<String, String>
where
    F: Fn(&str) -> Option<String>,
{
    let mut out: HashMap<String, String> = HashMap::new();

    // Walk every declared [[config]] field and pick the highest-precedence
    // value that resolves.
    for field in manifest.config_fields() {
        let full = field.full_key(&manifest.plugin.name);

        // 1. user TUI settings — keyed by full_key (plugins.<name>.<key>) or by bare key.
        if let Some(v) = user_config.get(&full).or_else(|| user_config.get(&field.key)) {
            out.insert(field.key.clone(), v.clone());
            continue;
        }

        // 2. [[config]] env_var referenced — read actual env value.
        if let Some(env_var) = &field.env_var {
            if let Some(v) = env_lookup(env_var) {
                out.insert(field.key.clone(), v);
                continue;
            }
        }

        // 3. [env] manifest default — match by key matching an env var name.
        if let Some(v) = manifest.env.get(&field.key) {
            if !v.is_empty() {
                out.insert(field.key.clone(), v.clone());
                continue;
            }
        }

        // 4. [[config]] default
        if let Some(v) = &field.default {
            out.insert(field.key.clone(), v.clone());
        }
    }

    // Also carry forward any [env] entries not represented as [[config]] fields,
    // so bare env defaults still end up in the resolved map. Applied last so
    // higher-precedence entries above win for keys present in both.
    for (k, v) in &manifest.env {
        out.entry(k.clone()).or_insert_with(|| v.clone());
    }

    out
}

/// Construct a `PluginState` from a just-loaded plugin. Status defaults to
/// `Loaded` — the loader upgrades it to `NeedsConfig`/`Failed` if `init()`
/// reports problems.
impl From<LoadedPlugin> for PluginState {
    fn from(p: LoadedPlugin) -> Self {
        Self {
            manifest: p.manifest,
            status: PluginStatus::Loaded,
            resolved_env: HashMap::new(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::manifest::{
        Capabilities, CatalogCapability, PluginConfigField, PluginManifest, PluginMeta,
    };

    fn manifest_with_field(field: PluginConfigField, env_defaults: Vec<(&str, &str)>) -> PluginManifest {
        PluginManifest {
            plugin: PluginMeta {
                name: "tester".to_string(),
                version: "0.1.0".to_string(),
                plugin_type: None,
                entrypoint: "plugin.wasm".to_string(),
                description: None,
                tags: Vec::new(),
                _author: None,
                _abi_version: None,
            },
            permissions: None,
            meta: None,
            env: env_defaults.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            config: vec![field],
            capabilities: Capabilities { catalog: CatalogCapability::default(), streams: false, _extra: Default::default() },
            rate_limit: None,
            _extra: Default::default(),
        }
    }

    #[test]
    fn resolve_config_precedence_user_wins_over_env_var_and_defaults() {
        let field = PluginConfigField {
            key: "api_key".into(),
            label: "API key".into(),
            hint: None,
            masked: true,
            required: true,
            default: Some("default-value".into()),
            env_var: Some("TEST_API_KEY".into()),
        };
        // env default for the same key
        let m = manifest_with_field(field, vec![("api_key", "env-default-value")]);

        let mut user = HashMap::new();
        user.insert("plugins.tester.api_key".to_string(), "user-value".to_string());

        let env_lookup = |k: &str| {
            if k == "TEST_API_KEY" { Some("env-var-value".to_string()) } else { None }
        };

        let resolved = resolve_config(&m, &user, env_lookup);
        assert_eq!(resolved.get("api_key"), Some(&"user-value".to_string()));
    }

    #[test]
    fn resolve_config_precedence_env_var_wins_over_env_default_and_default() {
        let field = PluginConfigField {
            key: "api_key".into(),
            label: "API key".into(),
            hint: None,
            masked: true,
            required: true,
            default: Some("default-value".into()),
            env_var: Some("TEST_API_KEY".into()),
        };
        let m = manifest_with_field(field, vec![("api_key", "env-default-value")]);

        let user = HashMap::new();
        let env_lookup = |k: &str| {
            if k == "TEST_API_KEY" { Some("env-var-value".to_string()) } else { None }
        };
        let resolved = resolve_config(&m, &user, env_lookup);
        assert_eq!(resolved.get("api_key"), Some(&"env-var-value".to_string()));
    }

    #[test]
    fn resolve_config_precedence_env_default_wins_over_field_default() {
        let field = PluginConfigField {
            key: "api_key".into(),
            label: "API key".into(),
            hint: None,
            masked: true,
            required: true,
            default: Some("field-default".into()),
            env_var: Some("TEST_API_KEY".into()),
        };
        let m = manifest_with_field(field, vec![("api_key", "env-default-value")]);

        let user = HashMap::new();
        let env_lookup = |_: &str| None;
        let resolved = resolve_config(&m, &user, env_lookup);
        assert_eq!(resolved.get("api_key"), Some(&"env-default-value".to_string()));
    }

    #[test]
    fn resolve_config_precedence_field_default_used_as_fallback() {
        let field = PluginConfigField {
            key: "api_key".into(),
            label: "API key".into(),
            hint: None,
            masked: true,
            required: true,
            default: Some("field-default".into()),
            env_var: None,
        };
        // no env default
        let m = manifest_with_field(field, vec![]);

        let user = HashMap::new();
        let env_lookup = |_: &str| None;
        let resolved = resolve_config(&m, &user, env_lookup);
        assert_eq!(resolved.get("api_key"), Some(&"field-default".to_string()));
    }

    #[test]
    fn state_store_tracks_status() {
        let mut store = StateStore::new();
        let m = manifest_with_field(
            PluginConfigField {
                key: "k".into(),
                label: "K".into(),
                hint: None,
                masked: false,
                required: false,
                default: None,
                env_var: None,
            },
            vec![],
        );
        store.insert(PluginState {
            manifest: m,
            status: PluginStatus::Loaded,
            resolved_env: HashMap::new(),
        });
        assert!(matches!(store.status("tester"), Some(PluginStatus::Loaded)));
        assert!(store.set_status(
            "tester",
            PluginStatus::NeedsConfig {
                missing: vec!["k".to_string()],
                hint: Some("set it".to_string()),
            },
        ));
        assert!(matches!(store.status("tester"), Some(PluginStatus::NeedsConfig { .. })));
        assert_eq!(store.list().count(), 1);
        assert!(store.remove("tester").is_some());
        assert!(store.status("tester").is_none());
    }
}
