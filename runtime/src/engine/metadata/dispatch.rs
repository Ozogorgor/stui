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
use stui_plugin_sdk::{
    TrailersRequest, TrailersResponse,
    ReleaseInfoRequest, ReleaseInfoResponse,
    KeywordsRequest, KeywordsResponse,
    BoxOfficeRequest, BoxOfficeResponse,
    AlternativeTitlesRequest, AlternativeTitlesResponse,
    BulkEnrichRequest, BulkEnrichResponse,
};
use crate::cache::metadata::MetadataCache;
use crate::cache::metadata_key::MetadataVerb;
use crate::config::types::MetadataSources;
use crate::engine::{CallPriority, Engine};
use crate::plugin::PluginMetaExt;

use super::sources::{SourceCapabilityProbe, SourceResolver};
use super::MetadataDispatch;

/// Source name reserved for the runtime-native TVDB client. Not a WASM
/// plugin; routed inside [`EngineMetadataDispatch`] through the
/// [`TvdbClient`] held on `Engine`.
const TVDB_SOURCE: &str = "tvdb";

/// Source name reserved for the runtime-native fanart.tv client. Same
/// pattern as TVDB — not a WASM plugin, routed through `Engine.fanart()`.
/// Currently contributes Artwork only (no enrich / credits / related).
const FANART_SOURCE: &str = "fanart";

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
    /// Whether the runtime-native fanart.tv client is available.
    fanart_available: bool,
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
            // Rating-aggregator is a runtime-native single source, not a
            // plugin-driven verb — no plugin can claim to support it.
            MetadataVerb::RatingsAggregator => false,
        }
    }
}

/// Per-plugin capability bundle: declared verbs + kind tags + per-verb
/// id_sources. The tag set is used by [`ManifestCapabilityProbe::discover`]
/// to decide whether a plugin should auto-join a kind's fan-out.
/// `verb_id_sources` is consumed by the detail-screen dispatch to pick
/// which entry id to forward to each plugin (the manifest is the source
/// of truth, replacing the old hardcoded plugin-name → id-source table).
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
    /// Per-verb canonical id_sources declared in the manifest. Empty
    /// list (or missing entry) means "no constraint" — the caller falls
    /// back to plugin-name-as-id-source defaults.
    verb_id_sources: std::collections::HashMap<MetadataVerb, Vec<String>>,
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
            let mut verb_id_sources: std::collections::HashMap<MetadataVerb, Vec<String>> =
                std::collections::HashMap::new();

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

                // Manifest-declared id_sources for the three verbs that the
                // orchestrator fans out by id. Lookup has its own routing
                // path (Dispatcher::by_lookup) so it isn't surfaced here;
                // artwork has no id_sources field in the SDK schema today
                // (fanart is runtime-native + kind-conditional, handled
                // separately inside resolve_id_for_plugin).
                if let Some(vc) = enrich {
                    let s = vc.id_sources();
                    if !s.is_empty() {
                        verb_id_sources.insert(MetadataVerb::Enrich, s.to_vec());
                    }
                }
                if let Some(vc) = credits {
                    let s = vc.id_sources();
                    if !s.is_empty() {
                        verb_id_sources.insert(MetadataVerb::Credits, s.to_vec());
                    }
                }
                if let Some(vc) = related {
                    let s = vc.id_sources();
                    if !s.is_empty() {
                        verb_id_sources.insert(MetadataVerb::Related, s.to_vec());
                    }
                }
                let _ = (lookup, artwork);
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
                verb_id_sources,
            };

            plugin_caps.insert(p.manifest.plugin.name.clone(), entry.clone());
            plugin_caps.insert(p.id.clone(), entry);
        }

        ManifestCapabilityProbe {
            plugin_caps,
            tvdb_available: engine.tvdb().is_some(),
            fanart_available: engine.fanart().is_some(),
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
        if plugin == FANART_SOURCE {
            // fanart only does artwork (posters / backgrounds / logos).
            // No enrich / credits / related — that's TMDB / OMDB / TVDB
            // territory, fanart's data is image URLs only.
            return self.fanart_available && matches!(verb, MetadataVerb::Artwork);
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

    fn id_sources_for(&self, plugin: &str, verb: MetadataVerb) -> Vec<String> {
        self.plugin_caps
            .get(plugin)
            .and_then(|c| c.verb_id_sources.get(&verb))
            .cloned()
            .unwrap_or_default()
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
            // Detail-view metadata fetches are user-driven (the user just
            // opened an entry) — never starve them behind background
            // enrichment sweeps.
            .supervisor_enrich(plugin, req, CallPriority::Foreground)
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
            .supervisor_get_credits(plugin, req, CallPriority::Foreground)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_artwork(
        &self,
        plugin: &str,
        req: ArtworkRequest,
    ) -> Result<ArtworkResponse, String> {
        // (fanart adapter helper defined at the bottom of this module —
        // keeps the dispatch logic readable.)
        if plugin == TVDB_SOURCE {
            let client = self
                .engine
                .tvdb()
                .ok_or_else(|| "tvdb: client not available".to_string())?;
            return crate::tvdb::source::artwork(&client, req).await;
        }
        if plugin == FANART_SOURCE {
            let client = self
                .engine
                .fanart()
                .ok_or_else(|| "fanart: client not available".to_string())?;
            return fanart_artwork_adapter(&client, req).await;
        }
        self.engine
            .supervisor_get_artwork(plugin, req, CallPriority::Foreground)
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
            .supervisor_related(plugin, req, CallPriority::Foreground)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_get_trailers(
        &self,
        plugin: &str,
        req: TrailersRequest,
    ) -> Result<TrailersResponse, String> {
        self.engine
            .supervisor_get_trailers(plugin, req, CallPriority::Background)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_get_release_info(
        &self,
        plugin: &str,
        req: ReleaseInfoRequest,
    ) -> Result<ReleaseInfoResponse, String> {
        self.engine
            .supervisor_get_release_info(plugin, req, CallPriority::Background)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_get_keywords(
        &self,
        plugin: &str,
        req: KeywordsRequest,
    ) -> Result<KeywordsResponse, String> {
        self.engine
            .supervisor_get_keywords(plugin, req, CallPriority::Background)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_get_box_office(
        &self,
        plugin: &str,
        req: BoxOfficeRequest,
    ) -> Result<BoxOfficeResponse, String> {
        self.engine
            .supervisor_get_box_office(plugin, req, CallPriority::Background)
            .await
            .map_err(|e| e.to_string())
    }

    async fn call_get_alternative_titles(
        &self,
        plugin: &str,
        req: AlternativeTitlesRequest,
    ) -> Result<AlternativeTitlesResponse, String> {
        self.engine
            .supervisor_get_alternative_titles(plugin, req, CallPriority::Background)
            .await
            .map_err(|e| e.to_string())
    }

    async fn fetch_ratings_aggregator(
        &self,
        imdb_id: &str,
        kind: &str,
    ) -> Result<Option<crate::ipc::v1::RatingsAggregatorData>, String> {
        let client = match self.engine.rating_aggregator() {
            Some(c) => c,
            None => return Ok(None),
        };
        let block = client
            .fetch(imdb_id, kind)
            .await
            .map_err(|e| e.to_string())?;
        Ok(block.map(|b| crate::ipc::v1::RatingsAggregatorData {
            description: b.description,
            external_url: b.external_url,
        }))
    }

    async fn call_bulk_enrich(
        &self,
        plugin: &str,
        req: BulkEnrichRequest,
    ) -> Result<BulkEnrichResponse, String> {
        self.engine
            .supervisor_bulk_enrich(plugin, req, CallPriority::Background)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Convert an `ArtworkRequest` into a fanart.tv fetch and wrap the
/// returned URLs into `ArtworkVariant`s. Routes by `req.kind`:
/// movies use the `/movies/{tmdb_id}` endpoint, series use
/// `/tv/{tvdb_id}`. `resolve_id_for_plugin` upstream picks the right id
/// type so the request's `id_source` will already match the kind.
///
/// All fanart variants are tagged `ArtworkSize::HiRes` — fanart serves
/// full-resolution PNG/JPEG with no per-size endpoints. Width/height
/// are not in fanart's response, so they stay None. Mime defaults to
/// JPEG; fanart's CDN serves either depending on the source upload.
async fn fanart_artwork_adapter(
    client: &crate::fanart::FanartClient,
    req: ArtworkRequest,
) -> Result<ArtworkResponse, String> {
    use stui_plugin_sdk::EntryKind;
    let urls = match req.kind {
        EntryKind::Movie => client
            .movie_artwork(&req.id, crate::fanart::ArtworkSlot::Poster)
            .await
            .map_err(|e| format!("fanart: movie artwork: {e}"))?,
        EntryKind::Series | EntryKind::Episode => client
            .tv_artwork(&req.id, crate::fanart::ArtworkSlot::Poster)
            .await
            .map_err(|e| format!("fanart: tv artwork: {e}"))?,
        _ => return Err(format!("fanart: unsupported kind {:?}", req.kind)),
    };
    let variants = urls
        .into_iter()
        .map(|url| crate::abi::types::ArtworkVariant {
            size: crate::abi::types::ArtworkSize::HiRes,
            url,
            mime: "image/jpeg".to_string(),
            width: None,
            height: None,
        })
        .collect();
    Ok(ArtworkResponse { variants })
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
                verb_id_sources: std::collections::HashMap::new(),
            },
        );
        plugin_caps.insert(
            "anilist".to_string(),
            PluginCaps {
                name: "anilist".to_string(),
                verbs: all_verbs,
                tags: vec!["anime".to_string()],
                verb_id_sources: std::collections::HashMap::new(),
            },
        );
        ManifestCapabilityProbe {
            plugin_caps,
            tvdb_available,
            // Tests don't exercise the fanart path; default to off so
            // existing assertions about "tvdb-only special source" hold.
            fanart_available: false,
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
