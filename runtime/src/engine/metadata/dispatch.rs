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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use stui_plugin_sdk::EntryKind;

use crate::abi::types::{
    ArtworkRequest, ArtworkResponse, CreditsRequest, CreditsResponse, EnrichRequest,
    EnrichResponse, PluginEntry, RelatedRequest,
};
use crate::cache::metadata::MetadataCache;
use crate::cache::metadata_key::MetadataVerb;
use crate::config::types::MetadataSources;
use crate::engine::Engine;
use crate::plugin::manifest::PluginMetaExt;
use crate::tvdb::{SearchKind, TvdbClient, TvdbEntry};

use super::sources::{SourceCapabilityProbe, SourceResolver};
use super::MetadataDispatch;

/// Source name reserved for the runtime-native TVDB client. Not a WASM
/// plugin; routed inside [`EngineMetadataDispatch`] through the
/// [`TvdbClient`] held on `Engine`.
const TVDB_SOURCE: &str = "tvdb";

/// Capability probe that consults the live plugin registry.
///
/// Current heuristic: a plugin supports a metadata verb iff its manifest
/// advertises `metadata-provider`-shaped plugin_type and the plugin is
/// currently loaded.  We don't yet discriminate between verbs at the
/// manifest level — every metadata-provider is assumed to implement all
/// four (enrich / credits / artwork / related).  That over-approximates
/// (a plugin that only implements `credits` still gets asked about
/// `artwork`), but the downstream per-verb fan-out silently drops
/// plugin errors via `filter_map(Result::ok)` so the cost is one extra
/// plugin call on miss — cheaper than adding manifest schema for each
/// verb before we know which providers actually need it.
pub struct ManifestCapabilityProbe {
    /// Plugin ids whose manifest reports metadata-provider shape.
    /// Captured at probe construction time so we don't need to hold
    /// the registry lock across the entire fan-out.
    metadata_plugin_ids: Vec<String>,
    /// Whether the runtime-native TVDB client is available (key
    /// resolved from env or embed at startup).  When `true`, the
    /// special "tvdb" source resolves through [`EngineMetadataDispatch`]
    /// instead of the WASM supervisor path.
    tvdb_available: bool,
}

impl ManifestCapabilityProbe {
    /// Snapshot the engine's registry and record every plugin id that
    /// advertises a metadata-provider-shaped plugin_type.
    pub async fn from_engine(engine: &Engine) -> Self {
        let reg = engine.registry().read().await;
        let metadata_plugin_ids = reg
            .all()
            .filter(|p| p.manifest.plugin.is_metadata_provider())
            // Accept match by either canonical id (UUID) or by manifest
            // name — the SourceResolver config keys plugins by name, so
            // a supports() call against the manifest name has to resolve.
            .flat_map(|p| vec![p.id.clone(), p.manifest.plugin.name.clone()])
            .collect();
        ManifestCapabilityProbe {
            metadata_plugin_ids,
            tvdb_available: engine.tvdb().is_some(),
        }
    }
}

impl SourceCapabilityProbe for ManifestCapabilityProbe {
    fn supports(&self, plugin: &str, verb: MetadataVerb, _kind_hint: &str) -> bool {
        if plugin == TVDB_SOURCE {
            // TVDB only contributes to enrich today (cross-provider id
            // resolution + scalar fields).  Credits/artwork/related
            // would need new TVDB endpoints — track separately.
            return self.tvdb_available && matches!(verb, MetadataVerb::Enrich);
        }
        self.metadata_plugin_ids.iter().any(|p| p == plugin)
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
            return tvdb_enrich(&client, req).await;
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
            // TVDB credits/artwork/related not implemented yet.  The
            // capability probe already filters these out, but guard in
            // case sources end up calling us anyway.
            return Err("tvdb: credits not implemented".into());
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
            return Err("tvdb: artwork not implemented".into());
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

// ── TVDB enrich adapter ──────────────────────────────────────────────────────

/// Run an enrich request through the runtime-native TVDB client and
/// reshape the result into the same `EnrichResponse` the supervisor
/// path would produce.
///
/// Lookup order: imdb id (precise) → title+year search (best-effort).
/// Confidence reflects which path produced the hit.
async fn tvdb_enrich(client: &TvdbClient, req: EnrichRequest) -> Result<EnrichResponse, String> {
    let kind = match req.partial.kind {
        EntryKind::Movie => SearchKind::Movie,
        EntryKind::Series | EntryKind::Episode => SearchKind::Series,
        _ => return Err("tvdb: unsupported kind".into()),
    };

    // Prefer imdb-id lookup when available — TVDB's /search/remoteid is
    // much more precise than free-text search.
    let imdb_lookup = req
        .partial
        .imdb_id
        .clone()
        .or_else(|| req.partial.external_ids.get("imdb").cloned());

    let (entry, confidence) = if let Some(imdb) = imdb_lookup {
        // /search returns an array; for a remote-id query we expect at
        // most one match, so take the first.
        let items = client
            .search(&imdb, kind, 1)
            .await
            .map_err(|e| e.to_string())?;
        match items.into_iter().next() {
            Some(t) => (t, 1.0_f32),
            None => return Err("tvdb: no match for imdb id".into()),
        }
    } else {
        let title = req.partial.title.trim();
        if title.is_empty() {
            return Err("tvdb: empty title and no imdb id".into());
        }
        let mut query = title.to_string();
        if let Some(y) = req.partial.year {
            query.push(' ');
            query.push_str(&y.to_string());
        }
        let items = client
            .search(&query, kind, 1)
            .await
            .map_err(|e| e.to_string())?;
        match items.into_iter().next() {
            Some(t) => (t, 0.7_f32),
            None => return Err("tvdb: no title match".into()),
        }
    };

    Ok(EnrichResponse {
        entry: tvdb_entry_to_plugin_entry(entry, req.partial.kind),
        confidence,
    })
}

/// Convert a [`TvdbEntry`] into the runtime's [`PluginEntry`] shape.
///
/// The whole point of running TVDB through enrich is to harvest its
/// cross-provider ids — `tvdb` for itself, plus `imdb` and `tmdb` when
/// TVDB knows them.  Those land in `external_ids` so the orchestrator's
/// per-plugin id router can dispatch credits/artwork/related to the
/// matching provider.
fn tvdb_entry_to_plugin_entry(t: TvdbEntry, kind: EntryKind) -> PluginEntry {
    let mut external_ids = HashMap::new();
    external_ids.insert("tvdb".to_string(), t.tvdb_id.clone());
    if let Some(ref imdb) = t.imdb_id {
        external_ids.insert("imdb".to_string(), imdb.clone());
    }
    if let Some(ref tmdb) = t.tmdb_id {
        external_ids.insert("tmdb".to_string(), tmdb.clone());
    }

    PluginEntry {
        id: format!("tvdb-{}", t.tvdb_id),
        kind,
        title: t.title,
        source: TVDB_SOURCE.to_string(),
        year: t.year.as_deref().and_then(|s| s.parse::<u32>().ok()),
        genre: if t.genres.is_empty() {
            None
        } else {
            Some(t.genres.join(", "))
        },
        description: t.overview,
        poster_url: t.image_url,
        imdb_id: t.imdb_id,
        external_ids,
        original_language: t.original_language,
        ..Default::default()
    }
}
