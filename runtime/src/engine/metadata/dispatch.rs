//! Production [`MetadataDispatch`] implementation that routes through
//! the real [`Engine`].
//!
//! Constructed per-request by the IPC layer (`main.rs`) with:
//!   * an `Arc<Engine>` for plugin calls + cache access
//!   * a [`SourceResolver`] built from the user's current config snapshot
//!
//! Keeps source-list plumbing out of the `Engine` struct so the engine
//! doesn't need to know about config-manager lifecycles — the IPC
//! dispatcher just builds the resolver from whatever config is current
//! at request time.

use std::sync::Arc;

use async_trait::async_trait;

use crate::abi::types::{
    ArtworkRequest, ArtworkResponse, CreditsRequest, CreditsResponse, EnrichRequest,
    EnrichResponse, PluginEntry, RelatedRequest,
};
use crate::cache::metadata::MetadataCache;
use crate::cache::metadata_key::MetadataVerb;
use crate::config::types::MetadataSources;
use crate::engine::Engine;
use crate::plugin::manifest::PluginMetaExt;

use super::sources::{SourceCapabilityProbe, SourceResolver};
use super::MetadataDispatch;

/// Source name reserved for the runtime-native TVDB client. Not a WASM
/// plugin; routed inside [`EngineMetadataDispatch`] through the
/// [`TvdbClient`] held on `Engine`.
const TVDB_SOURCE: &str = "tvdb";

/// Capability probe that consults the live plugin registry.
///
/// Tracks two facets per plugin: which verbs the manifest declares
/// (search/lookup/enrich/credits/artwork/related) and which kind tags
/// it advertises (`tags = ["movies", "anime", …]` in `[plugin]`).
/// `supports()` answers the verb question; `discover()` answers
/// "which plugins should auto-join the fan-out for kind X" by
/// matching tags against the kind-hint.
pub struct ManifestCapabilityProbe {
    /// Plugin entries keyed by both display name and UUID. Both keys
    /// point at the same `PluginCaps` so callers can use whichever id
    /// they have.
    plugin_caps: std::collections::HashMap<String, PluginCaps>,
    /// Whether the runtime-native TVDB client is available.
    tvdb_available: bool,
}

/// Set of metadata verbs a plugin supports.
#[derive(Default, Clone)]
pub(super) struct VerbSet {
    pub(super) search: bool,
    pub(super) lookup: bool,
    pub(super) enrich: bool,
    pub(super) credits: bool,
    pub(super) artwork: bool,
    pub(super) related: bool,
}

impl VerbSet {
    fn supports(&self, verb: MetadataVerb) -> bool {
        // Verb is supported if declared (bool:true, stub, or typed config).
        // Stubs count as "supported but returns NOT_IMPLEMENTED" - they still
        // get asked so the system can properly fall through.
        match verb {
            MetadataVerb::Enrich => self.enrich,
            MetadataVerb::Credits => self.credits,
            MetadataVerb::Artwork => self.artwork,
            MetadataVerb::Related => self.related,
        }
    }
}

/// Per-plugin capability bundle: declared verbs + kind tags. The tag
/// set is used by [`ManifestCapabilityProbe::discover`] to decide
/// whether a plugin should auto-join a kind's fan-out.
#[derive(Default, Clone)]
struct PluginCaps {
    /// Canonical plugin name (display name from `[plugin] name`). The
    /// HashMap may register the same `PluginCaps` under both name and
    /// UUID; this field carries the name for `discover()` callers
    /// (priority/disabled lists are stored as names, not UUIDs).
    name: String,
    verbs: VerbSet,
    /// Lowercased manifest `tags` (e.g. `["movies", "anime"]`). Used
    /// for kind-based discovery.
    tags: Vec<String>,
}

impl ManifestCapabilityProbe {
    /// Snapshot the engine's registry and extract per-verb capabilities.
    pub async fn from_engine(engine: &Engine) -> Self {
        let reg = engine.registry().read().await;
        let mut plugin_caps = std::collections::HashMap::new();

        for p in reg.all() {
            if !p.manifest.plugin.is_metadata_provider() {
                continue;
            }

            let caps = &p.manifest.capabilities.catalog;
            let mut verbs = VerbSet::default();

            if let stui_plugin_sdk::manifest::CatalogCapability::Typed {
                search, lookup, enrich, artwork, credits, related, ..
            } = &caps
            {
                verbs.search = search.unwrap_or(false);
                verbs.lookup = lookup.is_some();
                verbs.enrich = enrich.is_some();
                verbs.artwork = artwork.is_some();
                verbs.credits = credits.is_some();
                verbs.related = related.is_some();
            } else {
                // Legacy form: enabled = true means all verbs supported
                verbs.search = true;
                verbs.lookup = true;
                verbs.enrich = true;
                verbs.credits = true;
                verbs.artwork = true;
                verbs.related = true;
            }

            let tags: Vec<String> = p
                .manifest
                .plugin
                .tags
                .iter()
                .map(|t| t.to_ascii_lowercase())
                .collect();

            let entry = PluginCaps {
                name: p.manifest.plugin.name.clone(),
                verbs,
                tags,
            };

            plugin_caps.insert(p.manifest.plugin.name.clone(), entry.clone());
            plugin_caps.insert(p.id.clone(), entry);
        }

        ManifestCapabilityProbe {
            plugin_caps,
            tvdb_available: engine.tvdb().is_some(),
        }
    }
}

impl SourceCapabilityProbe for ManifestCapabilityProbe {
    fn supports(&self, plugin: &str, verb: MetadataVerb, _kind_hint: &str) -> bool {
        if plugin == TVDB_SOURCE {
            // TVDB now contributes to enrich, credits, and artwork via the
            // cached /extended endpoint. Related stays excluded — TVDB has
            // no "similar shows" endpoint worth surfacing.
            return self.tvdb_available
                && matches!(
                    verb,
                    MetadataVerb::Enrich | MetadataVerb::Credits | MetadataVerb::Artwork,
                );
        }
        // Check per-verb capabilities: only return true if the plugin
        // actually advertises support for this specific verb.
        self.plugin_caps
            .get(plugin)
            .map(|c| c.verbs.supports(verb))
            .unwrap_or(false)
    }

    fn discover(&self, verb: MetadataVerb, kind_hint: &str) -> Vec<String> {
        // Walk every plugin in the registry, return the names that:
        //   1. declare support for this verb (verbs.supports(verb)), and
        //   2. carry a manifest tag matching kind_hint
        //      (e.g. `tags = ["movies"]` matches kind_hint = "movies").
        //
        // Iterating the HashMap means we visit each plugin twice (once
        // by name, once by UUID — both keys point at the same entry).
        // De-dupe with a HashSet seeded by name so the result lists
        // canonical names only.
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for caps in self.plugin_caps.values() {
            if !seen.insert(caps.name.clone()) {
                continue;
            }
            if !caps.verbs.supports(verb) {
                continue;
            }
            if !caps.tags.iter().any(|t| t == kind_hint) {
                continue;
            }
            out.push(caps.name.clone());
        }
        out
    }
}

/// The production [`MetadataDispatch`] — owns the Engine handle used to
/// invoke plugin verbs and the source resolver used to pick plugin ids.
///
/// Cheap to clone (both fields are `Arc`s).  One instance lives for the
/// duration of a single `GetDetailMetadata` request.
#[derive(Clone)]
pub struct EngineMetadataDispatch {
    engine: Arc<Engine>,
    sources: Arc<SourceResolver>,
}

impl EngineMetadataDispatch {
    /// Build a dispatcher from the engine handle and a metadata-sources
    /// config snapshot.  Snapshots the plugin registry once (via
    /// [`ManifestCapabilityProbe::from_engine`]) so all four verb
    /// fan-outs share the same view.
    pub async fn new(engine: Arc<Engine>, config: MetadataSources) -> Self {
        let probe = ManifestCapabilityProbe::from_engine(&engine).await;
        let resolver = SourceResolver::new(config, Box::new(probe));
        EngineMetadataDispatch {
            engine,
            sources: Arc::new(resolver),
        }
    }
}

#[async_trait]
impl MetadataDispatch for EngineMetadataDispatch {
    fn cache(&self) -> &MetadataCache {
        &self.engine.cache.metadata
    }

    fn sources(&self) -> &SourceResolver {
        &self.sources
    }

    async fn call_enrich(
        &self,
        plugin: &str,
        req: EnrichRequest,
    ) -> Result<EnrichResponse, String> {
        if plugin == TVDB_SOURCE {
            let client = self
                .engine
                .tvdb()
                .ok_or_else(|| "tvdb: client not available".to_string())?;
            return crate::tvdb::source::enrich(&client, req).await;
        }
        // `supervisor_enrich` returns the enriched PluginEntry only —
        // re-wrap into EnrichResponse with confidence=1.0 (the plugin-side
        // confidence is discarded by supervisor_enrich; a future ABI rev
        // can surface it here).
        self.engine
            .supervisor_enrich(plugin, req)
            .await
            .map(|entry| EnrichResponse {
                entry,
                confidence: 1.0,
            })
            .map_err(|e| e.to_string())
    }

    async fn call_credits(
        &self,
        plugin: &str,
        req: CreditsRequest,
    ) -> Result<CreditsResponse, String> {
        if plugin == TVDB_SOURCE {
            let client = self
                .engine
                .tvdb()
                .ok_or_else(|| "tvdb: client not available".to_string())?;
            return crate::tvdb::source::credits(&client, req).await;
        }
        self.engine
            .supervisor_get_credits(plugin, req)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_artwork(
        &self,
        plugin: &str,
        req: ArtworkRequest,
    ) -> Result<ArtworkResponse, String> {
        if plugin == TVDB_SOURCE {
            let client = self
                .engine
                .tvdb()
                .ok_or_else(|| "tvdb: client not available".to_string())?;
            return crate::tvdb::source::artwork(&client, req).await;
        }
        self.engine
            .supervisor_get_artwork(plugin, req)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_related(
        &self,
        plugin: &str,
        req: RelatedRequest,
    ) -> Result<Vec<PluginEntry>, String> {
        if plugin == TVDB_SOURCE {
            return Err("tvdb: related not implemented".into());
        }
        self.engine
            .supervisor_related(plugin, req)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    fn probe(tvdb_available: bool) -> ManifestCapabilityProbe {
        let all_verbs = VerbSet {
            search: true,
            lookup: true,
            enrich: true,
            credits: true,
            artwork: true,
            related: true,
        };
        let mut plugin_caps = std::collections::HashMap::new();
        plugin_caps.insert(
            "tmdb".to_string(),
            PluginCaps {
                name: "tmdb".to_string(),
                verbs: all_verbs.clone(),
                tags: vec!["movies".to_string(), "series".to_string()],
            },
        );
        plugin_caps.insert(
            "anilist".to_string(),
            PluginCaps {
                name: "anilist".to_string(),
                verbs: all_verbs,
                tags: vec!["anime".to_string()],
            },
        );
        ManifestCapabilityProbe {
            plugin_caps,
            tvdb_available,
        }
    }

    #[test]
    fn tvdb_supports_enrich_credits_artwork_when_available() {
        let p = probe(true);
        assert!(p.supports("tvdb", MetadataVerb::Enrich, "movies"));
        assert!(p.supports("tvdb", MetadataVerb::Credits, "movies"));
        assert!(p.supports("tvdb", MetadataVerb::Artwork, "movies"));
    }

    #[test]
    fn tvdb_does_not_support_related_even_when_available() {
        let p = probe(true);
        assert!(!p.supports("tvdb", MetadataVerb::Related, "movies"));
    }

    #[test]
    fn tvdb_disabled_when_no_api_key() {
        let p = probe(false);
        assert!(!p.supports("tvdb", MetadataVerb::Enrich, "movies"));
        assert!(!p.supports("tvdb", MetadataVerb::Credits, "movies"));
        assert!(!p.supports("tvdb", MetadataVerb::Artwork, "movies"));
    }

    #[test]
    fn known_plugins_supported_regardless_of_tvdb_availability() {
        let p = probe(false);
        assert!(p.supports("tmdb", MetadataVerb::Enrich, "movies"));
        assert!(p.supports("anilist", MetadataVerb::Credits, "anime"));
    }

    #[test]
    fn unknown_plugin_not_supported() {
        let p = probe(true);
        assert!(!p.supports("madeup", MetadataVerb::Enrich, "movies"));
    }
}
