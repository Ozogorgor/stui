/// Plugin manifest — parsed from plugin.toml in each plugin directory.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use stui_plugin_sdk::EntryKind;

// ── Manifest schema ──────────────────────────────────────────────────────────

// ── CatalogCapability ─────────────────────────────────────────────────────────

/// Typed or legacy catalog capability declared in `[capabilities]`.
///
/// Two TOML forms are accepted via `#[serde(untagged)]`:
///
/// - **Legacy boolean**: `catalog = true` / `catalog = false`
///   All existing plugin.toml files use this form. The plugin is excluded from
///   scoped search dispatch (no declared kinds) until it migrates to the typed form.
///
/// - **Typed table**: `[capabilities.catalog]` with `kinds = [...]`
///   Used in Chunk 7 migrations; enables scoped dispatch.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CatalogCapability {
    /// Legacy form: `catalog = true` / `catalog = false`.
    /// Carries no scope information; excluded from scoped dispatch.
    Enabled(bool),
    /// New typed form: `[capabilities.catalog] kinds = [...]`.
    Typed {
        #[serde(default)]
        kinds: Vec<EntryKind>,
    },
}

impl Default for CatalogCapability {
    fn default() -> Self { Self::Typed { kinds: Vec::new() } }
}

impl CatalogCapability {
    /// Declared search kinds (empty unless plugin uses the typed form).
    pub fn kinds(&self) -> &[EntryKind] {
        match self {
            Self::Typed { kinds } => kinds.as_slice(),
            Self::Enabled(_) => &[],
        }
    }

    /// True if the plugin has any catalog capability at all (typed or legacy-enabled).
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Typed { kinds } => !kinds.is_empty(),
            Self::Enabled(b) => *b,
        }
    }
}

// ── Capabilities ──────────────────────────────────────────────────────────────

/// Structured `[capabilities]` table from plugin.toml.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub catalog: CatalogCapability,
    #[serde(default)]
    pub streams: bool,
    /// Forward-compat catch-all for unknown capability keys
    /// (e.g. `metadata = true`, `music = true`, `anime = true`,
    /// `search = true`, `resolve = true` seen in existing plugin.toml files).
    /// These remain opaque until they earn a typed field.
    #[serde(flatten)]
    pub _extra: HashMap<String, toml::Value>,
}

// ── PluginManifest ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    pub permissions: Option<Permissions>,
    pub meta: Option<AuthorMeta>,
    /// Environment variable defaults declared in plugin.toml [env] table.
    /// Values can be overridden by the actual env or stui config.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Configuration fields for this plugin.
    /// These are shown in the TUI settings screen and stored in stui.toml.
    /// Accepts both `[[config]]` (array) and `[config]` (ignored as empty table).
    #[serde(default, deserialize_with = "deserialize_config_fields")]
    pub config: Vec<PluginConfigField>,
    /// Structured capabilities declared in `[capabilities]`.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Tolerate unknown top-level sections.
    #[serde(flatten)]
    pub _extra: HashMap<String, toml::Value>,
}

fn deserialize_config_fields<'de, D>(deserializer: D) -> Result<Vec<PluginConfigField>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    // Try as array first, fall back to ignoring (table/empty)
    let value = toml::Value::deserialize(deserializer)?;
    match value {
        toml::Value::Array(arr) => {
            let mut fields = Vec::new();
            for v in arr {
                match v.try_into() {
                    Ok(f) => fields.push(f),
                    Err(_) => {} // skip malformed entries
                }
            }
            Ok(fields)
        }
        _ => Ok(Vec::new()), // [config] as table or other → treat as empty
    }
}

/// A single configuration field for a plugin.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginConfigField {
    /// The config key (e.g., "api_keys.tmdb" or "providers.tmdb.enabled")
    pub key: String,
    /// Human-readable label shown in the TUI
    pub label: String,
    /// Hint text shown below the input field
    pub hint: Option<String>,
    /// If true, the value is masked (for API keys, passwords)
    #[serde(default)]
    pub masked: bool,
    /// If true, this field is required
    #[serde(default)]
    pub required: bool,
    /// Default value (optional)
    pub default: Option<String>,
}

impl PluginConfigField {
    /// Generate the full config key for this field.
    /// Format: "plugins.{plugin_name}.{field_key}"
    pub fn full_key(&self, plugin_name: &str) -> String {
        format!("plugins.{}.{}", plugin_name, self.key)
    }
}

impl PluginManifest {
    /// Get all config fields for this plugin.
    ///
    /// If the plugin declares explicit `[config]` fields, those are returned.
    /// Otherwise, `[env]` fields are auto-converted to config fields.
    pub fn config_fields(&self) -> Vec<PluginConfigField> {
        if !self.config.is_empty() {
            return self.config.clone();
        }
        // Auto-convert [env] fields to config fields
        self.env
            .iter()
            .map(|(key, default_value)| {
                let label = key.replace('_', " ");
                let hint = if key.contains("KEY") || key.contains("PASSWORD") {
                    Some("Keep secret - stored securely".to_string())
                } else if key.contains("URL") {
                    Some("Base URL for the API".to_string())
                } else {
                    None
                };
                let masked =
                    key.contains("KEY") || key.contains("PASSWORD") || key.contains("SECRET");
                let required = key.contains("KEY"); // API keys are typically required

                PluginConfigField {
                    key: key.clone(),
                    label,
                    hint,
                    masked,
                    required,
                    default: if default_value.is_empty() {
                        None
                    } else {
                        Some(default_value.clone())
                    },
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    /// Entrypoint file (default: "plugin.wasm").
    #[serde(default = "default_entrypoint")]
    pub entrypoint: String,
    pub description: Option<String>,
    /// Tags for organizing plugins (e.g., "movies", "music", "anime", "tv", "subtitles")
    #[serde(default)]
    pub tags: Vec<String>,
    // Tolerate extra fields in plugin.toml (author, abi_version, etc.)
    #[serde(default, rename = "author")]
    pub _author: Option<String>,
    #[serde(default, rename = "abi_version")]
    pub _abi_version: Option<u32>,
}

fn default_entrypoint() -> String { "plugin.wasm".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginType {
    /// Provides catalog metadata: trending lists, search results, posters, ratings.
    /// Does NOT supply playable stream URLs.
    #[serde(alias = "metadata")]
    MetadataProvider,
    /// Provides playable stream URLs or magnet links for a given media item.
    StreamProvider,
    /// Provides subtitle tracks (.srt / .vtt) for a given media item.
    SubtitleProvider,
    /// Resolves a catalog entry ID into a stream URL (legacy — prefer StreamProvider).
    Resolver,
    /// Handles OAuth or token-based authentication for an external service.
    Auth,
    /// Scans and indexes a local library of media files.
    Indexer,

    // ── Backward-compatible aliases ───────────────────────────────────────
    // Existing plugin.toml files that use the old type names will still load.
    /// Legacy alias for MetadataProvider.
    Provider,
    /// Legacy alias for SubtitleProvider.
    Subtitle,
    /// Legacy alias for MetadataProvider.
    Metadata,
}

impl PluginType {
    /// Returns true if this plugin type can supply playable stream URLs.
    pub fn is_stream_provider(&self) -> bool {
        matches!(self, PluginType::StreamProvider | PluginType::Resolver)
    }

    /// Returns true if this plugin type can supply subtitle tracks.
    pub fn is_subtitle_provider(&self) -> bool {
        matches!(self, PluginType::SubtitleProvider | PluginType::Subtitle)
    }

    /// Returns true if this plugin type supplies catalog / metadata.
    pub fn is_metadata_provider(&self) -> bool {
        matches!(
            self,
            PluginType::MetadataProvider | PluginType::Provider | PluginType::Metadata
        )
    }

    /// Return the set of runtime capabilities this plugin type advertises.
    ///
    /// The engine uses this to route requests to the right plugins without
    /// having to inspect `PluginType` variants directly.
    pub fn capabilities(&self) -> Vec<PluginCapability> {
        match self {
            PluginType::MetadataProvider | PluginType::Provider | PluginType::Metadata => {
                vec![PluginCapability::Catalog]
            }
            PluginType::StreamProvider => {
                vec![PluginCapability::Catalog, PluginCapability::Streams]
            }
            PluginType::SubtitleProvider | PluginType::Subtitle => {
                vec![PluginCapability::Subtitles]
            }
            PluginType::Resolver => vec![PluginCapability::Streams],
            PluginType::Auth => vec![PluginCapability::Auth],
            PluginType::Indexer => vec![PluginCapability::Index],
        }
    }
}

impl std::fmt::Display for PluginType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PluginType::MetadataProvider => "metadata-provider",
            PluginType::StreamProvider => "stream-provider",
            PluginType::SubtitleProvider => "subtitle-provider",
            PluginType::Resolver => "resolver",
            PluginType::Auth => "auth",
            PluginType::Indexer => "indexer",
            // legacy aliases
            PluginType::Provider => "provider",
            PluginType::Subtitle => "subtitle",
            PluginType::Metadata => "metadata",
        };
        write!(f, "{}", s)
    }
}

// ── PluginCapability ──────────────────────────────────────────────────────────

/// A discrete runtime capability that a plugin can advertise.
///
/// Unlike `PluginType` (which is a classification read from `plugin.toml`),
/// `PluginCapability` is the set of actions the runtime can dispatch to a
/// plugin at runtime.  A plugin may advertise multiple capabilities.
///
/// # Example
///
/// A plugin that provides both catalog search and stream resolution would
/// return `[Catalog, Streams]` from `capabilities()`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PluginCapability {
    /// Can respond to catalog search / trending requests.
    Catalog,
    /// Can resolve a media item ID into playable stream URLs.
    Streams,
    /// Can provide subtitle tracks for a media item.
    Subtitles,
    /// Can authenticate to an external service and refresh tokens.
    Auth,
    /// Can scan and index a local media library.
    Index,
}

impl PluginCapability {
    /// Human-readable label for logging and UI display.
    pub fn label(&self) -> &'static str {
        match self {
            PluginCapability::Catalog => "catalog",
            PluginCapability::Streams => "streams",
            PluginCapability::Subtitles => "subtitles",
            PluginCapability::Auth => "auth",
            PluginCapability::Index => "index",
        }
    }
}

impl std::fmt::Display for PluginCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Network permission: either a boolean (`network = true`) or an allowlist
/// (`network = ["api.example.com", ...]`).
///
/// Both forms appear in existing plugin.toml files.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum NetworkPermission {
    /// `network = true` / `network = false`
    Bool(bool),
    /// `network = ["host1", "host2", ...]`
    Hosts(Vec<String>),
}

impl Default for NetworkPermission {
    fn default() -> Self { Self::Bool(false) }
}

impl NetworkPermission {
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Hosts(h) => !h.is_empty(),
        }
    }

    pub fn hosts(&self) -> &[String] {
        match self {
            Self::Bool(_) => &[],
            Self::Hosts(h) => h.as_slice(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Permissions {
    #[serde(default)]
    pub network: NetworkPermission,
    /// Explicit allowlist of hostnames (from `network_hosts = [...]` in plugin.toml).
    /// When non-empty this takes precedence over the boolean `network` flag.
    #[serde(default)]
    pub network_hosts: Vec<String>,
    #[serde(default)]
    pub filesystem: Vec<String>,
}

impl Permissions {
    /// True if the plugin may reach `host` (bare hostname or IP).
    pub fn allows_host(&self, host: &str) -> bool {
        // network_hosts (legacy separate field) takes precedence
        if !self.network_hosts.is_empty() {
            return self.network_hosts.iter().any(|h| {
                h == host
                    || (h == "localhost" && (host == "127.0.0.1" || host == "::1"))
                    || (host == "localhost" && (h == "127.0.0.1" || h == "::1"))
            });
        }
        // network = [...] allowlist form
        let hosts = self.network.hosts();
        if !hosts.is_empty() {
            return hosts.iter().any(|h| {
                h == host
                    || (h == "localhost" && (host == "127.0.0.1" || host == "::1"))
                    || (host == "localhost" && (h == "127.0.0.1" || h == "::1"))
            });
        }
        // network = true/false
        self.network.is_enabled()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthorMeta {
    pub author: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
}

// ── Loaded plugin record ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)] // pub API: used by engine and registry
pub struct LoadedPlugin {
    pub id: String, // uuid assigned at load time
    pub manifest: PluginManifest,
    pub dir: PathBuf,        // directory containing plugin.toml
    pub entrypoint: PathBuf, // resolved absolute path to .wasm / .so / rpc binary
    pub mode: ExecutionMode,
}

impl LoadedPlugin {
    /// Check whether this plugin advertises a specific capability.
    ///
    /// Use this for all capability-based dispatch instead of matching
    /// on `PluginType` directly — it handles legacy type aliases correctly.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if plugin.has_capability(PluginCapability::Streams) {
    ///     // ask this plugin for stream URLs
    /// }
    /// ```
    pub fn has_capability(&self, cap: PluginCapability) -> bool {
        self.manifest
            .plugin
            .plugin_type
            .capabilities()
            .contains(&cap)
    }

    #[allow(dead_code)] // pub API: used by engine and registry
    /// All capabilities this plugin advertises.
    pub fn capabilities(&self) -> Vec<PluginCapability> {
        self.manifest.plugin.plugin_type.capabilities()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionMode {
    Wasm,
    NativeLib,
    Grpc(String), // address
}

// ── Loader ───────────────────────────────────────────────────────────────────

/// Load and validate a plugin manifest from a directory.
pub fn load_manifest(plugin_dir: &Path) -> Result<PluginManifest> {
    let manifest_path = plugin_dir.join("plugin.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: PluginManifest = toml::from_str(&raw).with_context(|| "parsing plugin.toml")?;
    Ok(manifest)
}

/// Resolve the execution mode and entrypoint path for a loaded manifest.
pub fn resolve_entrypoint(
    plugin_dir: &Path,
    manifest: &PluginManifest,
) -> Result<(ExecutionMode, PathBuf)> {
    let entry = &manifest.plugin.entrypoint;

    // gRPC: entrypoint looks like "grpc://host:port"
    if entry.starts_with("grpc://") {
        return Ok((ExecutionMode::Grpc(entry.clone()), PathBuf::from(entry)));
    }

    let abs = plugin_dir.join(entry);

    let mode = if entry.ends_with(".wasm") {
        ExecutionMode::Wasm
    } else if entry.ends_with(".so") || entry.ends_with(".dylib") || entry.ends_with(".dll") {
        ExecutionMode::NativeLib
    } else {
        anyhow::bail!("Unknown entrypoint format: {}", entry);
    };

    Ok((mode, abs))
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    use stui_plugin_sdk::EntryKind;

    fn meta(body: &str) -> String {
        format!(
            r#"
[plugin]
name = "test"
version = "0.1.0"
type = "metadata"
{body}
"#
        )
    }

    #[test]
    fn legacy_bool_form_parses_and_is_excluded_from_scope_dispatch() {
        let toml_text = meta("\n[capabilities]\ncatalog = true\nmetadata = true\n");
        let m: PluginManifest = toml::from_str(&toml_text).unwrap();
        assert!(m.capabilities.catalog.is_enabled());
        assert!(
            m.capabilities.catalog.kinds().is_empty(),
            "legacy bool form carries no kinds → excluded from scoped dispatch"
        );
        assert!(
            m.capabilities._extra.contains_key("metadata"),
            "other legacy keys fall into _extra"
        );
    }

    #[test]
    fn typed_form_parses_kinds() {
        let toml_text =
            meta("\n[capabilities]\n\n[capabilities.catalog]\nkinds = [\"artist\", \"album\", \"track\"]\n");
        let m: PluginManifest = toml::from_str(&toml_text).unwrap();
        assert_eq!(
            m.capabilities.catalog.kinds(),
            &[EntryKind::Artist, EntryKind::Album, EntryKind::Track]
        );
        assert!(m.capabilities.catalog.is_enabled());
    }

    #[test]
    fn no_capabilities_section_still_parses() {
        let toml_text = meta("");
        let m: PluginManifest = toml::from_str(&toml_text).unwrap();
        assert!(m.capabilities.catalog.kinds().is_empty());
        assert!(!m.capabilities.catalog.is_enabled());
        assert!(!m.capabilities.streams);
    }

    #[test]
    fn catalog_false_parses_as_disabled() {
        let toml_text = meta("\n[capabilities]\ncatalog = false\n");
        let m: PluginManifest = toml::from_str(&toml_text).unwrap();
        assert!(!m.capabilities.catalog.is_enabled());
        assert!(m.capabilities.catalog.kinds().is_empty());
    }

    #[test]
    fn all_real_plugin_manifests_parse() {
        use std::fs;
        // CARGO_MANIFEST_DIR points to runtime/, so ../plugins is the plugins dir
        let plugins_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../plugins");
        let entries = fs::read_dir(&plugins_dir)
            .unwrap_or_else(|e| panic!("plugins/ dir at {}: {e}", plugins_dir.display()));
        let mut checked = 0;
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("plugin.toml");
            if !manifest_path.exists() {
                continue;
            }
            let text = fs::read_to_string(&manifest_path).unwrap();
            let parsed: Result<PluginManifest, _> = toml::from_str(&text);
            assert!(
                parsed.is_ok(),
                "failed to parse {}: {:?}",
                manifest_path.display(),
                parsed.err()
            );
            checked += 1;
        }
        assert!(
            checked >= 10,
            "expected to check at least 10 plugins, got {checked}"
        );
    }
}
