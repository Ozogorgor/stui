/// Plugin manifest — parsed from plugin.toml in each plugin directory.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Manifest schema ──────────────────────────────────────────────────────────

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
    #[serde(default)]
    pub config: Vec<PluginConfigField>,
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
    pub entrypoint: String,
    pub description: Option<String>,
    /// Tags for organizing plugins (e.g., "movies", "music", "anime", "tv", "subtitles")
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginType {
    /// Provides catalog metadata: trending lists, search results, posters, ratings.
    /// Does NOT supply playable stream URLs.
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

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Permissions {
    #[serde(default)]
    pub network: bool,
    /// Explicit allowlist of hostnames (from `network = [...]` in plugin.toml).
    /// When non-empty this takes precedence over the boolean `network` flag.
    #[serde(default)]
    pub network_hosts: Vec<String>,
    #[serde(default)]
    pub filesystem: Vec<String>,
}

impl Permissions {
    /// True if the plugin may reach `host` (bare hostname or IP).
    pub fn allows_host(&self, host: &str) -> bool {
        if !self.network_hosts.is_empty() {
            return self.network_hosts.iter().any(|h| {
                h == host
                    || (h == "localhost" && (host == "127.0.0.1" || host == "::1"))
                    || (host == "localhost" && (h == "127.0.0.1" || h == "::1"))
            });
        }
        self.network
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
#[allow(dead_code)]
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

    #[allow(dead_code)]
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
