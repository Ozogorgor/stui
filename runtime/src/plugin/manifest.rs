//! Runtime-side plugin manifest — now re-exports from the SDK.
//!
//! The authoritative plugin manifest types live in `stui_plugin_sdk::manifest`.
//! Runtime-only concerns (the `PluginType` enum + legacy-migration helpers,
//! `PluginCapability`, and a `PluginMetaExt` trait that carries the
//! runtime-only derived helpers on `PluginMeta`) stay here.

// ── Re-exports from the SDK manifest module ───────────────────────────────────

pub use stui_plugin_sdk::manifest::{
    validate, ArtworkConfig, AuthorMeta, Capabilities, CatalogCapability, LookupConfig,
    ManifestValidationError, NetworkPermission, Permissions, PluginConfigField, PluginManifest,
    PluginMeta, RateLimit, SupervisorTuning, VerbConfig,
};

use serde::{Deserialize, Serialize};

// ── PluginType (runtime-only) ─────────────────────────────────────────────────

/// Runtime-only plugin classification read from the `[plugin] type = "…"` field.
///
/// The SDK's `PluginMeta.plugin_type` is an `Option<String>` — this enum is
/// only used inside the runtime (discovery, pipeline routing, engine helpers)
/// after parsing the string. New canonical manifests don't set `type` at all;
/// `validate` rejects any manifest that does. This type therefore only appears
/// on legacy-migration paths and `plugin_type_or_default()` hands back a
/// `MetadataProvider` default for callers that still read it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginType {
    /// Provides catalog metadata: trending lists, search results, posters, ratings.
    /// Does NOT supply playable stream URLs.
    ///
    /// Wire-format aliases `"metadata"` and `"provider"` deserialize to this
    /// variant — legacy plugin.toml files still parse without re-serialising.
    #[serde(alias = "metadata", alias = "provider")]
    MetadataProvider,
    /// Provides playable stream URLs or magnet links for a given media item.
    StreamProvider,
    /// Provides subtitle tracks (.srt / .vtt) for a given media item.
    ///
    /// Wire-format alias `"subtitle"` deserializes to this variant.
    #[serde(alias = "subtitle")]
    SubtitleProvider,
    /// Resolves a catalog entry ID into a stream URL (legacy — prefer StreamProvider).
    Resolver,
    /// Handles OAuth or token-based authentication for an external service.
    Auth,
    /// Scans and indexes a local library of media files.
    Indexer,
}

impl PluginType {
    /// Returns true if this plugin type can supply playable stream URLs.
    pub fn is_stream_provider(&self) -> bool {
        matches!(self, PluginType::StreamProvider | PluginType::Resolver)
    }

    /// Returns true if this plugin type can supply subtitle tracks.
    pub fn is_subtitle_provider(&self) -> bool {
        matches!(self, PluginType::SubtitleProvider)
    }

    /// Returns true if this plugin type supplies catalog / metadata.
    pub fn is_metadata_provider(&self) -> bool {
        matches!(self, PluginType::MetadataProvider)
    }

    /// Return the set of runtime capabilities this plugin type advertises.
    ///
    /// The engine uses this to route requests to the right plugins without
    /// having to inspect `PluginType` variants directly.
    pub fn capabilities(&self) -> Vec<PluginCapability> {
        match self {
            PluginType::MetadataProvider => vec![PluginCapability::Catalog],
            PluginType::StreamProvider => {
                vec![PluginCapability::Catalog, PluginCapability::Streams]
            }
            PluginType::SubtitleProvider => vec![PluginCapability::Subtitles],
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
        };
        write!(f, "{}", s)
    }
}

impl std::str::FromStr for PluginType {
    type Err = String;

    /// Parse the plugin-type string emitted by `plugin.toml`. Accepts the
    /// canonical names (`metadata-provider`) and the legacy aliases
    /// (`metadata`, `provider`, `subtitle`) — the latter map to the
    /// canonical variants. serde uses `#[serde(alias = …)]` for the same
    /// mapping on TOML deserialise paths; this `FromStr` is the fallback
    /// for non-serde config reads (e.g. CLI arg parsing).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "metadata-provider" | "metadata" | "provider" => Ok(PluginType::MetadataProvider),
            "stream-provider" => Ok(PluginType::StreamProvider),
            "subtitle-provider" | "subtitle" => Ok(PluginType::SubtitleProvider),
            "resolver" => Ok(PluginType::Resolver),
            "auth" => Ok(PluginType::Auth),
            "indexer" => Ok(PluginType::Indexer),
            _ => Err(format!("unknown plugin type: {s}")),
        }
    }
}

// ── PluginMetaExt (runtime helpers) ───────────────────────────────────────────

/// Runtime-only helpers over the SDK's `PluginMeta`. The SDK carries
/// `plugin_type: Option<String>` and doesn't interpret it; this trait gives
/// the runtime its legacy routing/display helpers without polluting the SDK.
pub trait PluginMetaExt {
    /// Parse `plugin_type` into a typed `PluginType`, if present and valid.
    fn plugin_type_enum(&self) -> Option<PluginType>;

    /// Return the plugin_type or `MetadataProvider` for callers that need a
    /// default. The new canonical schema doesn't require it (inferred from
    /// capabilities), but legacy code paths still read it.
    fn plugin_type_or_default(&self) -> PluginType;

    /// Display-friendly plugin type string. Mirrors the old
    /// `PluginType::Display` behavior for legacy call sites. Returns
    /// the empty string if the manifest omits `[plugin] type`.
    fn plugin_type_str(&self) -> String;

    /// True if the manifest's plugin_type (if present) is metadata-provider-shaped.
    /// Returns true when plugin_type is absent, preserving the old
    /// "catalog = true ⇒ treat as metadata" default-routing semantic.
    fn is_metadata_provider(&self) -> bool;

    fn is_stream_provider(&self) -> bool;

    fn is_subtitle_provider(&self) -> bool;
}

impl PluginMetaExt for PluginMeta {
    fn plugin_type_enum(&self) -> Option<PluginType> {
        self.plugin_type.as_deref().and_then(|s| s.parse().ok())
    }

    fn plugin_type_or_default(&self) -> PluginType {
        self.plugin_type_enum()
            .unwrap_or(PluginType::MetadataProvider)
    }

    fn plugin_type_str(&self) -> String {
        // New manifests deliberately omit `[plugin] type` (it's deprecated —
        // see SDK PluginMeta::plugin_type). Falling back to
        // `plugin_type_or_default()` (MetadataProvider) gives the TUI's
        // Installed table a non-blank Type cell for modern plugins
        // instead of empty string. Legacy manifests that still set the
        // field continue to report their declared type verbatim.
        self.plugin_type_or_default().to_string()
    }

    fn is_metadata_provider(&self) -> bool {
        self.plugin_type_enum()
            .map(|t| t.is_metadata_provider())
            .unwrap_or(true)
    }

    fn is_stream_provider(&self) -> bool {
        self.plugin_type_enum()
            .map(|t| t.is_stream_provider())
            .unwrap_or(false)
    }

    fn is_subtitle_provider(&self) -> bool {
        self.plugin_type_enum()
            .map(|t| t.is_subtitle_provider())
            .unwrap_or(false)
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod plugin_type_tests {
    use super::*;

    #[test]
    fn plugin_type_parses_canonical_names() {
        assert_eq!(
            "metadata-provider".parse::<PluginType>().unwrap(),
            PluginType::MetadataProvider
        );
        assert_eq!(
            "stream-provider".parse::<PluginType>().unwrap(),
            PluginType::StreamProvider
        );
        assert_eq!(
            "subtitle-provider".parse::<PluginType>().unwrap(),
            PluginType::SubtitleProvider
        );
        assert_eq!(
            "resolver".parse::<PluginType>().unwrap(),
            PluginType::Resolver
        );
        assert_eq!("auth".parse::<PluginType>().unwrap(), PluginType::Auth);
        assert_eq!(
            "indexer".parse::<PluginType>().unwrap(),
            PluginType::Indexer
        );
    }

    #[test]
    fn plugin_type_parses_legacy_aliases() {
        // Legacy strings now collapse onto the canonical variants — same
        // wire-format compatibility, no orphan unit-variants in the enum.
        assert_eq!(
            "metadata".parse::<PluginType>().unwrap(),
            PluginType::MetadataProvider
        );
        assert_eq!(
            "provider".parse::<PluginType>().unwrap(),
            PluginType::MetadataProvider
        );
        assert_eq!(
            "subtitle".parse::<PluginType>().unwrap(),
            PluginType::SubtitleProvider
        );
    }

    #[test]
    fn plugin_type_serde_aliases_map_to_canonical() {
        // TOML deserialise path also collapses legacy strings.
        let m: PluginMeta = toml::from_str(
            r#"
name = "legacy"
version = "0.1.0"
type = "provider"
"#,
        )
        .unwrap();
        assert_eq!(m.plugin_type_enum().unwrap(), PluginType::MetadataProvider);

        let m: PluginMeta = toml::from_str(
            r#"
name = "legacy"
version = "0.1.0"
type = "subtitle"
"#,
        )
        .unwrap();
        assert_eq!(m.plugin_type_enum().unwrap(), PluginType::SubtitleProvider);
    }

    #[test]
    fn plugin_type_unknown_rejected() {
        assert!("banana".parse::<PluginType>().is_err());
    }

    #[test]
    fn plugin_meta_ext_maps_string_to_enum() {
        // PluginMeta with plugin_type: None → default MetadataProvider
        let m = toml::from_str::<PluginMeta>(
            r#"
name = "test"
version = "0.1.0"
"#,
        )
        .unwrap();
        assert!(m.plugin_type_enum().is_none());
        assert_eq!(m.plugin_type_or_default(), PluginType::MetadataProvider);
        // `plugin_type_str` falls back to the default-typed enum's string
        // form when the manifest omits `[plugin] type`, so the TUI's
        // Installed table has a non-blank Type cell for modern plugins.
        assert_eq!(m.plugin_type_str(), "metadata-provider");
        assert!(m.is_metadata_provider());
        assert!(!m.is_stream_provider());
        assert!(!m.is_subtitle_provider());

        // Legacy "metadata" string still maps to MetadataProvider
        let m2 = toml::from_str::<PluginMeta>(
            r#"
name = "test"
version = "0.1.0"
type = "metadata"
"#,
        )
        .unwrap();
        assert_eq!(m2.plugin_type_enum().unwrap(), PluginType::MetadataProvider);
        assert_eq!(m2.plugin_type_str(), "metadata-provider");
        assert!(m2.is_metadata_provider());
    }
}
