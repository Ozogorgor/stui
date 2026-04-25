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
        }
    }
}

impl SourceCapabilityProbe for ManifestCapabilityProbe {
    fn supports(&self, plugin: &str, _verb: MetadataVerb, _kind_hint: &str) -> bool {
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
        self.engine
            .supervisor_related(plugin, req)
            .await
            .map_err(|e| e.to_string())
    }
}
