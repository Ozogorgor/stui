//! Plugin manifest schema — parsed from `plugin.toml` in each plugin directory.
//!
//! Runtime owns the AUTHORITATIVE manifest types: this module carries the
//! richer shape (VerbConfig, RateLimit, PluginConfigField, validate()) while
//! the SDK's `stui_plugin_sdk::capabilities::{PluginManifest, ...}` remains a
//! lighter author-facing parallel shape used by the CLI `stui plugin lint`
//! (see Task 1.3). The two are kept intentionally separate — the SDK stubs
//! do not roundtrip back into the runtime.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use stui_plugin_sdk::EntryKind;

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
    /// New typed form: `[capabilities.catalog]` with kinds + per-verb sub-tables.
    Typed {
        #[serde(default)]
        kinds: Vec<EntryKind>,
        /// `search = true | false | VerbConfig`. The required catalog verb.
        #[serde(default)]
        search: Option<bool>,
        /// Optional verb configs for lookup / enrich / artwork / credits / related.
        #[serde(default)]
        lookup: Option<LookupConfig>,
        #[serde(default)]
        enrich: Option<VerbConfig>,
        #[serde(default)]
        artwork: Option<ArtworkConfig>,
        #[serde(default)]
        credits: Option<VerbConfig>,
        #[serde(default)]
        related: Option<VerbConfig>,
    },
}

impl Default for CatalogCapability {
    fn default() -> Self {
        Self::Typed {
            kinds: Vec::new(),
            search: None,
            lookup: None,
            enrich: None,
            artwork: None,
            credits: None,
            related: None,
        }
    }
}

impl CatalogCapability {
    /// Declared search kinds (empty unless plugin uses the typed form).
    pub fn kinds(&self) -> &[EntryKind] {
        match self {
            Self::Typed { kinds, .. } => kinds.as_slice(),
            Self::Enabled(_) => &[],
        }
    }

    /// True if the plugin has any catalog capability at all (typed or legacy-enabled).
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Typed { kinds, .. } => !kinds.is_empty(),
            Self::Enabled(b) => *b,
        }
    }
}

// ── Per-verb config ───────────────────────────────────────────────────────────

/// A per-verb capability declaration.
///
/// Accepts three TOML forms:
/// - `verb = true` / `verb = false` → enabled / not-declared
/// - `[capabilities.catalog.verb]` `stub = true` + `reason = "…"` → a declared stub
/// - `[capabilities.catalog.verb]` with arbitrary typed fields → full config
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum VerbConfig {
    /// `verb = true` / `verb = false`
    Bool(bool),
    /// Stub: plugin declares the verb but returns NOT_IMPLEMENTED.
    /// `{ stub = true, reason = "…" }`
    Stub { stub: bool, reason: Option<String> },
    /// Full typed config with arbitrary fields.
    Typed(toml::Value),
}

impl VerbConfig {
    /// True if the verb is declared as a stub (always returns NOT_IMPLEMENTED).
    pub fn is_stub(&self) -> bool {
        matches!(self, Self::Stub { stub: true, .. })
    }

    /// True if the verb is enabled at all (bool:true, stub, or typed).
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Stub { stub, .. } => *stub,
            Self::Typed(_) => true,
        }
    }
}

/// Lookup-verb config: declares `id_sources = [...]` of canonical id-sources
/// this plugin supports for `Plugin::lookup`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LookupConfig {
    /// Either `lookup = true` (bool) or a list of id sources.
    #[serde(default)]
    pub id_sources: Vec<String>,
    #[serde(default)]
    pub stub: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

impl LookupConfig {
    pub fn is_stub(&self) -> bool { self.stub }
}

/// Artwork-verb config: declares supported `sizes`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ArtworkConfig {
    #[serde(default)]
    pub sizes: Vec<String>,
    #[serde(default)]
    pub stub: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

impl ArtworkConfig {
    pub fn is_stub(&self) -> bool { self.stub }
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
    /// Optional rate-limit declaration; see `PluginSupervisor`.
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    /// Tolerate unknown top-level sections.
    #[serde(flatten)]
    pub _extra: HashMap<String, toml::Value>,
}

fn deserialize_config_fields<'de, D>(deserializer: D) -> Result<Vec<PluginConfigField>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Try as array first, fall back to ignoring (table/empty)
    let value = toml::Value::deserialize(deserializer)?;
    match value {
        toml::Value::Array(arr) => {
            let mut fields = Vec::new();
            for v in arr {
                if let Ok(f) = v.try_into() {
                    fields.push(f);
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
    /// Optional env var that backs this field (precedence: user config > env var > [env] default > default).
    #[serde(default)]
    pub env_var: Option<String>,
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
                    env_var: None,
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    /// Legacy field: `[plugin] type = "metadata"`. NEW manifests must NOT set this.
    /// Kept as Option so validate() can reject it with an actionable error.
    /// Accepts backward-compat plugin-type strings in manifests from before the
    /// canonical-schema flip. During migration, loading proceeds via
    /// `plugin_type_or_default()` which infers metadata from [capabilities].
    #[serde(default, rename = "type")]
    pub plugin_type: Option<PluginType>,
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

impl PluginMeta {
    /// Return the plugin_type or a default for callers that need it.
    /// The new canonical schema doesn't require it (inferred from capabilities),
    /// but legacy code paths still read it.
    pub fn plugin_type_or_default(&self) -> PluginType {
        self.plugin_type.clone().unwrap_or(PluginType::MetadataProvider)
    }

    /// Display-friendly plugin type string. Mirrors the old
    /// `PluginType::Display` behavior for legacy call sites. Returns
    /// the empty string if the manifest omits [plugin] type.
    pub fn plugin_type_str(&self) -> String {
        self.plugin_type
            .as_ref()
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// True if the manifest's plugin_type (if present) is metadata-provider-shaped.
    /// Returns true when plugin_type is absent AND the manifest declares a
    /// catalog capability, preserving the old "catalog = true ⇒ treat as metadata"
    /// semantic for legacy dispatch.
    pub fn is_metadata_provider(&self) -> bool {
        self.plugin_type
            .as_ref()
            .map(|t| t.is_metadata_provider())
            .unwrap_or(true)
    }

    pub fn is_stream_provider(&self) -> bool {
        self.plugin_type
            .as_ref()
            .map(|t| t.is_stream_provider())
            .unwrap_or(false)
    }

    pub fn is_subtitle_provider(&self) -> bool {
        self.plugin_type
            .as_ref()
            .map(|t| t.is_subtitle_provider())
            .unwrap_or(false)
    }
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

// ── Permissions ───────────────────────────────────────────────────────────────

/// Network permission: either a boolean (`network = true`) or an allowlist
/// (`network = ["api.example.com", ...]`).
///
/// Both forms still parse here so validate() can surface a useful error for
/// legacy `network = true` manifests — but only the allowlist form passes
/// validation in the new canonical schema.
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

    /// True if this permissions block was declared with legacy-form
    /// `network = true|false` rather than an allowlist. Used by
    /// `manifest::validate` to reject legacy forms.
    pub fn network_is_bool_form(&self) -> bool {
        matches!(self.network, NetworkPermission::Bool(_))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthorMeta {
    pub author: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
}

// ── RateLimit ─────────────────────────────────────────────────────────────────

/// Per-plugin rate-limit declaration consumed by `PluginSupervisor::TokenBucket`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimit {
    /// Steady-state requests per second (tokens generated per second).
    pub rps: u32,
    /// Maximum burst capacity — the bucket size.
    #[serde(default = "default_burst")]
    pub burst: u32,
}

fn default_burst() -> u32 { 1 }

// ── Manifest validation ───────────────────────────────────────────────────────

/// Validate a freshly-parsed manifest against the new canonical schema.
///
/// Returns a `ManifestValidationError` describing what's wrong so the loader
/// can surface an actionable message.
pub fn validate(manifest: &PluginManifest) -> Result<(), ManifestValidationError> {
    // 1. Legacy field: [plugin] type = "..."
    if manifest.plugin.plugin_type.is_some() {
        return Err(ManifestValidationError::LegacyField(
            "[plugin] type = \"...\" is no longer supported; plugin type is inferred from [capabilities.*]".to_string(),
        ));
    }
    // 2. network permission must be an allowlist (not bool)
    if let Some(perms) = &manifest.permissions {
        if perms.network_is_bool_form() && perms.network_hosts.is_empty() {
            return Err(ManifestValidationError::LegacyField(
                "[permissions] network = true is no longer supported; use network = [\"host1\", ...]".to_string(),
            ));
        }
        // 3. filesystem permission rejected for metadata plugins
        if !perms.filesystem.is_empty() {
            return Err(ManifestValidationError::LegacyField(
                "[permissions] filesystem is not supported for metadata plugins".to_string(),
            ));
        }
    }
    // 4 & 5. If typed CatalogCapability is declared, enforce id_sources + search.
    if let CatalogCapability::Typed {
        lookup,
        search,
        ..
    } = &manifest.capabilities.catalog
    {
        if let Some(lookup) = lookup {
            for source in &lookup.id_sources {
                if !stui_plugin_sdk::id_sources::is_canonical(source) {
                    return Err(ManifestValidationError::UnknownIdSource(source.clone()));
                }
            }
        }
        // catalog.search must be declared true (required verb).
        if !search.unwrap_or(false) {
            return Err(ManifestValidationError::MissingRequiredVerb("search".to_string()));
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestValidationError {
    #[error("legacy manifest field: {0}")]
    LegacyField(String),
    #[error("unknown id-source: {0} (see sdk::id_sources for canonical set)")]
    UnknownIdSource(String),
    #[error("required verb not declared: {0}")]
    MissingRequiredVerb(String),
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod capability_tests {
    use super::*;
    use stui_plugin_sdk::EntryKind;

    fn meta(body: &str) -> String {
        // Use a manifest WITHOUT `[plugin] type` so validation doesn't reject.
        // Individual tests can override by taking raw TOML.
        format!(
            r#"
[plugin]
name = "test"
version = "0.1.0"
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
            meta("\n[capabilities]\n\n[capabilities.catalog]\nkinds = [\"artist\", \"album\", \"track\"]\nsearch = true\n");
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

#[cfg(test)]
mod validate_tests {
    use super::*;

    /// Build a minimal-valid manifest: typed-catalog with kinds + search=true.
    fn minimal_valid_toml(extra: &str) -> String {
        format!(
            r#"
[plugin]
name = "test"
version = "0.1.0"

[capabilities.catalog]
kinds = ["track"]
search = true
{extra}
"#
        )
    }

    #[test]
    fn legacy_plugin_type_field_rejected() {
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"
type = "metadata-provider"

[capabilities.catalog]
kinds = ["track"]
search = true
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        let err = validate(&m).unwrap_err();
        assert!(matches!(err, ManifestValidationError::LegacyField(_)), "got {err:?}");
    }

    #[test]
    fn legacy_network_bool_rejected() {
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"

[permissions]
network = true

[capabilities.catalog]
kinds = ["track"]
search = true
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        let err = validate(&m).unwrap_err();
        assert!(matches!(err, ManifestValidationError::LegacyField(ref s) if s.contains("network")), "got {err:?}");
    }

    #[test]
    fn legacy_filesystem_permission_rejected() {
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"

[permissions]
network = ["api.example.com"]
filesystem = ["/tmp"]

[capabilities.catalog]
kinds = ["track"]
search = true
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        let err = validate(&m).unwrap_err();
        assert!(
            matches!(err, ManifestValidationError::LegacyField(ref s) if s.contains("filesystem")),
            "got {err:?}"
        );
    }

    #[test]
    fn unknown_id_source_rejected() {
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"

[capabilities.catalog]
kinds = ["track"]
search = true

[capabilities.catalog.lookup]
id_sources = ["not-a-real-id-source"]
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        let err = validate(&m).unwrap_err();
        assert!(matches!(err, ManifestValidationError::UnknownIdSource(_)), "got {err:?}");
    }

    #[test]
    fn catalog_search_false_rejected() {
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"

[capabilities.catalog]
kinds = ["track"]
search = false
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        let err = validate(&m).unwrap_err();
        assert!(
            matches!(err, ManifestValidationError::MissingRequiredVerb(ref v) if v == "search"),
            "got {err:?}"
        );
    }

    #[test]
    fn catalog_search_absent_rejected() {
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"

[capabilities.catalog]
kinds = ["track"]
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        let err = validate(&m).unwrap_err();
        assert!(
            matches!(err, ManifestValidationError::MissingRequiredVerb(ref v) if v == "search"),
            "got {err:?}"
        );
    }

    #[test]
    fn valid_manifest_accepted() {
        let toml_text = minimal_valid_toml("");
        let m: PluginManifest = toml::from_str(&toml_text).unwrap();
        assert!(validate(&m).is_ok());
    }

    #[test]
    fn valid_manifest_with_canonical_id_sources_accepted() {
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"

[capabilities.catalog]
kinds = ["movie"]
search = true

[capabilities.catalog.lookup]
id_sources = ["tmdb", "imdb"]
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        assert!(validate(&m).is_ok());
    }

    #[test]
    fn legacy_bool_catalog_still_validates_ok_no_typed_requirements() {
        // `catalog = true` (legacy bool) doesn't trigger the search-verb check
        // because it's not the typed form. validate() leaves it alone.
        let toml_text = r#"
[plugin]
name = "test"
version = "0.1.0"

[capabilities]
catalog = true
"#;
        let m: PluginManifest = toml::from_str(toml_text).unwrap();
        assert!(validate(&m).is_ok());
    }
}

#[cfg(test)]
mod verb_config_tests {
    use super::*;

    #[test]
    fn verb_config_bool_true_not_stub() {
        let vc: VerbConfig = toml::from_str("v = true").map(|t: toml::Table| t["v"].clone()).unwrap().try_into().unwrap();
        assert!(!vc.is_stub());
        assert!(vc.is_enabled());
    }

    #[test]
    fn verb_config_bool_false_not_stub_not_enabled() {
        let vc: VerbConfig = toml::from_str("v = false").map(|t: toml::Table| t["v"].clone()).unwrap().try_into().unwrap();
        assert!(!vc.is_stub());
        assert!(!vc.is_enabled());
    }

    #[test]
    fn verb_config_stub_is_stub_and_enabled() {
        let tbl: toml::Table = toml::from_str("[v]\nstub = true\nreason = \"upstream lacks it\"").unwrap();
        let vc: VerbConfig = tbl["v"].clone().try_into().unwrap();
        assert!(vc.is_stub());
        assert!(vc.is_enabled());
    }

    #[test]
    fn verb_config_typed_table_enabled_not_stub() {
        // A typed table with no explicit stub flag → Typed variant.
        let tbl: toml::Table = toml::from_str("[v]\nkey = \"value\"").unwrap();
        let vc: VerbConfig = tbl["v"].clone().try_into().unwrap();
        assert!(vc.is_enabled());
        assert!(!vc.is_stub());
    }

    #[test]
    fn lookup_config_is_stub_when_flagged() {
        let lc: LookupConfig = toml::from_str("stub = true\nreason = \"upstream lacks it\"").unwrap();
        assert!(lc.is_stub());
    }

    #[test]
    fn artwork_config_is_stub_when_flagged() {
        let ac: ArtworkConfig = toml::from_str("stub = true").unwrap();
        assert!(ac.is_stub());
    }
}
