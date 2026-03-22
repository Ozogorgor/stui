//! Engine module — plugin lifecycle, dispatch, and pipeline orchestration.
//!
//! # Structure
//!
//! ```text
//! engine/
//!   mod.rs       - Engine struct: plugin registry, search/resolve/metadata dispatch
//!   pipeline.rs  - Pipeline struct: top-level orchestration (search -> resolve -> play)
//! ```
//!
//! Both live here because they are tightly related: the Pipeline *owns* an
//! Engine and delegates all plugin calls to it.  Keeping them in the same
//! module folder makes this dependency clear at a glance.

#![allow(dead_code)]

pub mod pipeline;
#[allow(unused_imports)]
pub use pipeline::Pipeline;

pub mod trace;
pub use trace::TraceEmitter;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::abi::{SearchRequest, WasmSupervisor, WasmSupervisorConfig};
use crate::ipc::{
    ErrorCode, MediaEntry, MediaTab, PluginInfo, PluginListResponse,
    PluginLoadedResponse, PluginStatus, PluginUnloadedResponse, ResolveResponse, Response,
    SearchResponse,
};
use crate::plugin::{ExecutionMode, LoadedPlugin};
use crate::plugin as plugin;
use crate::sandbox::SandboxCtx;
use crate::{resolver, scraper};

// ── Registry ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins:          HashMap<String, LoadedPlugin>,        // id → plugin
    sandbox:          HashMap<String, SandboxCtx>,          // id → sandbox context
    wasm_supervisors: HashMap<String, Arc<WasmSupervisor>>, // id → supervisor (WASM plugins only)
}

impl PluginRegistry {
    pub fn get(&self, id: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &LoadedPlugin> {
        self.plugins.values()
    }

    pub fn sandbox_for(&self, id: &str) -> Option<&SandboxCtx> {
        self.sandbox.get(id)
    }

    pub fn insert(&mut self, plugin: LoadedPlugin, ctx: SandboxCtx) {
        let id = plugin.id.clone();
        self.plugins.insert(id.clone(), plugin);
        self.sandbox.insert(id, ctx);
    }

    pub fn insert_wasm_supervisor(&mut self, plugin_id: &str, sup: Arc<WasmSupervisor>) {
        self.wasm_supervisors.insert(plugin_id.to_string(), sup);
    }

    pub fn wasm_supervisor_for(&self, id: &str) -> Option<Arc<WasmSupervisor>> {
        self.wasm_supervisors.get(id).cloned()
    }

    pub fn remove(&mut self, id: &str) -> Option<LoadedPlugin> {
        self.sandbox.remove(id);
        self.wasm_supervisors.remove(id);
        self.plugins.remove(id)
    }

    /// Find all plugins that have a given capability.
    pub fn find_by_capability(&self, cap: crate::plugin::PluginCapability) -> Vec<&LoadedPlugin> {
        self.plugins.values().filter(|p| p.has_capability(cap.clone())).collect()
    }

    /// Find all plugins that can serve catalog content.
    ///
    /// Currently all `Catalog`-capable plugins are assumed to handle every tab;
    /// per-tab filtering is left to the plugin itself via its manifest capabilities.
    pub fn find_providers_for_tab(&self, _tab: &MediaTab) -> Vec<&LoadedPlugin> {
        self.find_by_capability(crate::plugin::PluginCapability::Catalog)
    }

    /// Find all plugins that can resolve stream URLs.
    pub fn find_stream_providers(&self) -> Vec<&LoadedPlugin> {
        self.find_by_capability(crate::plugin::PluginCapability::Streams)
    }

    /// Find all plugins that can provide subtitle tracks.
    pub fn find_subtitle_providers(&self) -> Vec<&LoadedPlugin> {
        self.find_by_capability(crate::plugin::PluginCapability::Subtitles)
    }

    /// Get all loaded plugins.
    pub fn all_plugins(&self) -> impl Iterator<Item = &LoadedPlugin> {
        self.plugins.values()
    }
}

// ── Engine ───────────────────────────────────────────────────────────────────

use crate::cache::RuntimeCache;

#[derive(Clone)]
pub struct Engine {
    registry:  Arc<RwLock<PluginRegistry>>,
    cache_dir: std::path::PathBuf,
    data_dir:  std::path::PathBuf,
    /// In-memory TTL caches for search results, metadata, and stream URLs.
    pub cache: RuntimeCache,
}

impl Engine {
    pub fn new(cache_dir: std::path::PathBuf, data_dir: std::path::PathBuf) -> Self {
        Self {
            registry:  Arc::new(RwLock::new(PluginRegistry::default())),
            cache_dir,
            data_dir,
            cache:     RuntimeCache::new(),
        }
    }

    // ── Plugin lifecycle ──────────────────────────────────────────────────

    /// Access the plugin registry (read-only).
    pub fn registry(&self) -> &Arc<RwLock<PluginRegistry>> {
        &self.registry
    }

    pub async fn load_plugin(&self, plugin_dir: &Path) -> Result<Response> {
        let manifest = plugin::load_manifest(plugin_dir)?;
        let (mode, entrypoint) = plugin::resolve_entrypoint(plugin_dir, &manifest)?;

        let id = Uuid::new_v4().to_string();
        let name = manifest.plugin.name.clone();

        let loaded = LoadedPlugin {
            id: id.clone(),
            manifest,
            dir: plugin_dir.to_path_buf(),
            entrypoint,
            mode,
        };

        let ctx = SandboxCtx::new(
            &loaded,
            self.cache_dir.clone(),
            self.data_dir.clone(),
        );
        ctx.ensure_dirs()?;

        info!(plugin_id = %id, plugin = %name, "plugin loaded");

        let mut reg = self.registry.write().await;

        // For WASM plugins, spin up a supervisor so calls get timeout,
        // crash detection, memory limits, and automatic reload.
        if matches!(loaded.mode, ExecutionMode::Wasm) {
            let sup_cfg  = WasmSupervisorConfig::default();
            let wasm_path = loaded.entrypoint.clone();
            let pname     = name.clone();
            let sup_ctx   = ctx.clone();
            let pid       = id.clone();

            // Load happens async; if it fails we log and continue — the
            // plugin is registered but marked unavailable until reload.
            match WasmSupervisor::load(wasm_path, pname.clone(), sup_ctx, sup_cfg).await {
                Ok(sup) => {
                    reg.insert_wasm_supervisor(&pid, Arc::new(sup));
                }
                Err(e) => {
                    warn!(plugin = %pname, err = %e, "WASM supervisor load failed — plugin unavailable until reload");
                }
            }
        }

        reg.insert(loaded, ctx);

        Ok(Response::PluginLoaded(PluginLoadedResponse {
            plugin_id: id,
            name,
        }))
    }

    pub async fn unload_plugin(&self, plugin_id: &str) -> Result<Response> {
        let mut reg = self.registry.write().await;
        match reg.remove(plugin_id) {
            Some(p) => {
                info!(plugin_id = %plugin_id, plugin = %p.manifest.plugin.name, "plugin unloaded");
                Ok(Response::PluginUnloaded(PluginUnloadedResponse {
                    plugin_id: plugin_id.to_string(),
                }))
            }
            None => bail!("Plugin '{}' not found", plugin_id),
        }
    }

    pub async fn list_plugins(&self) -> Response {
        let reg = self.registry.read().await;
        let plugins: Vec<PluginInfo> = reg
            .all()
            .map(|p| PluginInfo {
                id: p.id.clone(),
                name: p.manifest.plugin.name.clone(),
                version: p.manifest.plugin.version.clone(),
                plugin_type: p.manifest.plugin.plugin_type.to_string(),
                status: PluginStatus::Loaded,
                tags: p.manifest.plugin.tags.clone(),
            })
            .collect();
        Response::PluginList(PluginListResponse { plugins })
    }

    // ── Search ────────────────────────────────────────────────────────────

    pub async fn search(
        &self,
        req_id: &str,
        query: &str,
        tab: &MediaTab,
        provider_filter: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Response {
        use crate::cache::search::SearchKey;

        // ── Cache lookup ──────────────────────────────────────────────────
        // We only cache when there's no provider filter (i.e. a normal
        // cross-provider search), and only the first page (offset == 0).
        let cache_key = if provider_filter.is_none() && offset == 0 {
            let tab_str = format!("{:?}", tab).to_lowercase();
            Some(SearchKey::new(tab_str, query, 1))
        } else { None };

        if let Some(ref key) = cache_key {
            if let Some(cached) = self.cache.search.get(key).await {
                let total = cached.len();
                let paged: Vec<_> = cached.into_iter().skip(offset).take(limit).collect();
                // Convert CatalogEntry → MediaEntry for the response
                let items: Vec<crate::ipc::MediaEntry> = paged.into_iter().map(|e| {
                    crate::ipc::MediaEntry {
                        id:          e.id,
                        title:       e.title,
                        year:        e.year,
                        genre:       e.genre,
                        rating:      e.rating,
                        description: e.description,
                        poster_url:  e.poster_url,
                        provider:    e.provider,
                        tab:         tab.clone(),
                        media_type:  e.media_type,
                        ratings:     std::collections::HashMap::new(),
                    }
                }).collect();
                return Response::SearchResult(SearchResponse {
                    id: req_id.to_string(),
                    items,
                    total,
                    offset,
                });
            }
        }

        // ── Live fan-out ──────────────────────────────────────────────────
        let reg = self.registry.read().await;
        let providers = reg.find_providers_for_tab(tab);

        if providers.is_empty() {
            return Response::SearchResult(SearchResponse {
                id: req_id.to_string(),
                items: vec![],
                total: 0,
                offset,
            });
        }

        let mut set = tokio::task::JoinSet::new();
        for plugin in &providers {
            if let Some(filter) = provider_filter {
                if plugin.manifest.plugin.name != filter { continue; }
            }
            let plugin_clone = (*plugin).clone();
            let q = query.to_string();
            let t = tab.clone();

            match plugin_clone.mode {
                ExecutionMode::Wasm => {
                    // Route through supervisor: timeout + crash tracking.
                    let sup = reg.wasm_supervisor_for(&plugin_clone.id);
                    if let Some(sup) = sup {
                        let tab_str  = format!("{:?}", t).to_lowercase();
                        let provider = plugin_clone.manifest.plugin.name.clone();
                        let tab_out  = t.clone();
                        set.spawn(async move {
                            let req = SearchRequest { query: q, tab: tab_str, page: 0, limit: 50 };
                            sup.search(&req).await
                                .map(|r| r.items.into_iter().map(|e| MediaEntry {
                                    id:          e.id,
                                    title:       e.title,
                                    year:        e.year,
                                    genre:       e.genre,
                                    rating:      e.rating,
                                    description: e.description,
                                    poster_url:  e.poster_url,
                                    provider:    provider.clone(),
                                    tab:         tab_out.clone(),
                                    media_type:  crate::ipc::MediaType::default(),
                                    ratings:     std::collections::HashMap::new(),
                                }).collect::<Vec<_>>())
                                .map_err(|e| anyhow::anyhow!("{e}"))
                        });
                    } else {
                        warn!(plugin = %plugin_clone.manifest.plugin.name, "no WASM supervisor — skipping");
                    }
                }
                _ => {
                    let sandbox = reg.sandbox_for(&plugin_clone.id).cloned();
                    if let Some(ctx) = sandbox {
                        set.spawn(async move {
                            scraper::search(&ctx, &plugin_clone, &q, &t).await
                        });
                    }
                }
            }
        }
        drop(reg);

        // Collect results in completion order — fastest provider wins the front.
        let mut all_items: Vec<crate::ipc::MediaEntry> = vec![];
        while let Some(result) = set.join_next().await {
            match result {
                Ok(Ok(mut items)) => all_items.append(&mut items),
                Ok(Err(e)) => warn!("provider search error: {e}"),
                Err(e)     => warn!("provider task panicked: {e}"),
            }
        }

        // ── Populate cache ────────────────────────────────────────────────
        if let Some(key) = cache_key {
            // Convert MediaEntry → CatalogEntry for storage
            let catalog_entries: Vec<crate::catalog::CatalogEntry> = all_items.iter().map(|e| {
                crate::catalog::CatalogEntry {
                    id:          e.id.clone(),
                    title:       e.title.clone(),
                    year:        e.year.clone(),
                    genre:       e.genre.clone(),
                    rating:      e.rating.clone(),
                    description: e.description.clone(),
                    poster_url:  e.poster_url.clone(),
                    poster_art:  None,
                    provider:    e.provider.clone(),
                    tab:         format!("{:?}", tab).to_lowercase(),
                    imdb_id:     None,
                    tmdb_id:     None,
                    media_type:  e.media_type.clone(),
                    ratings:     std::collections::HashMap::new(),
                }
            }).collect();
            self.cache.search.insert(key, catalog_entries).await;
        }

        let total = all_items.len();
        let paged: Vec<_> = all_items.into_iter().skip(offset).take(limit).collect();

        Response::SearchResult(SearchResponse {
            id: req_id.to_string(),
            items: paged,
            total,
            offset,
        })
    }

    // ── Resolve ───────────────────────────────────────────────────────────

    pub async fn resolve(
        &self,
        req_id: &str,
        entry_id: &str,
        provider_name: &str,
    ) -> Response {
        let reg = self.registry.read().await;
        let found = reg
            .all()
            .find(|p| p.manifest.plugin.name == provider_name)
            .cloned();
        let ctx = found
            .as_ref()
            .and_then(|p| reg.sandbox_for(&p.id).cloned());
        let wasm_sup = found.as_ref().and_then(|p| {
            if matches!(p.mode, ExecutionMode::Wasm) {
                reg.wasm_supervisor_for(&p.id)
            } else {
                None
            }
        });
        drop(reg);

        match (found, ctx) {
            (Some(plugin), _) if matches!(plugin.mode, ExecutionMode::Wasm) => {
                match wasm_sup {
                    Some(sup) => {
                        let req = crate::abi::ResolveRequest { entry_id: entry_id.to_string() };
                        match sup.resolve(&req).await {
                            Ok(r) => Response::ResolveResult(ResolveResponse {
                                id: req_id.to_string(),
                                stream_url: r.stream_url,
                                quality: r.quality,
                                subtitles: r.subtitles.into_iter().map(|s| crate::ipc::SubtitleTrack {
                                    language: s.language,
                                    url: s.url,
                                    format: s.format,
                                }).collect(),
                            }),
                            Err(e) => Response::error(
                                Some(req_id.to_string()),
                                ErrorCode::ResolveFailed,
                                e.to_string(),
                            ),
                        }
                    }
                    None => Response::error(
                        Some(req_id.to_string()),
                        ErrorCode::ResolveFailed,
                        format!("WASM supervisor unavailable for '{provider_name}'"),
                    ),
                }
            }
            (Some(plugin), Some(ctx)) => {
                match resolver::resolve(&ctx, &plugin, entry_id).await {
                    Ok(resp) => Response::ResolveResult(ResolveResponse {
                        id: req_id.to_string(),
                        stream_url: resp.stream_url,
                        quality: resp.quality,
                        subtitles: resp.subtitles,
                    }),
                    Err(e) => Response::error(
                        Some(req_id.to_string()),
                        ErrorCode::ResolveFailed,
                        e.to_string(),
                    ),
                }
            }
            _ => Response::error(
                Some(req_id.to_string()),
                ErrorCode::PluginNotFound,
                format!("No provider plugin named '{provider_name}'"),
            ),
        }
    }

    /// Like `resolve` but returns the raw StreamResult — used by player_bridge
    /// which needs the stream_url directly without wrapping it in a Response.
    pub async fn resolve_raw(
        &self,
        entry_id: &str,
        provider_name: &str,
    ) -> Result<resolver::StreamResult, String> {
        let reg = self.registry.read().await;
        let found = reg
            .all()
            .find(|p| p.manifest.plugin.name == provider_name)
            .cloned();
        let ctx = found
            .as_ref()
            .and_then(|p| reg.sandbox_for(&p.id).cloned());
        let wasm_sup = found.as_ref().and_then(|p| {
            if matches!(p.mode, ExecutionMode::Wasm) {
                reg.wasm_supervisor_for(&p.id)
            } else {
                None
            }
        });
        drop(reg);

        match found {
            Some(plugin) if matches!(plugin.mode, ExecutionMode::Wasm) => {
                match wasm_sup {
                    Some(sup) => {
                        let req = crate::abi::ResolveRequest { entry_id: entry_id.to_string() };
                        sup.resolve(&req).await
                            .map(|r| resolver::StreamResult {
                                stream_url: r.stream_url,
                                quality:    r.quality,
                                subtitles:  r.subtitles.into_iter().map(|s| crate::ipc::SubtitleTrack {
                                    language: s.language,
                                    url:      s.url,
                                    format:   s.format,
                                }).collect(),
                            })
                            .map_err(|e| e.to_string())
                    }
                    None => Err(format!("WASM supervisor unavailable for '{provider_name}'")),
                }
            }
            Some(plugin) => {
                match ctx {
                    Some(ctx) => resolver::resolve(&ctx, &plugin, entry_id)
                        .await
                        .map_err(|e| e.to_string()),
                    None => Err(format!("No sandbox context for '{provider_name}'")),
                }
            }
            None => Err(format!("No provider plugin named '{provider_name}'")),
        }
    }

    // ── Ranked stream resolution ───────────────────────────────────────────

    /// Collect streams from all built-in providers that support streams for
    /// this entry_id, rank them by quality, and return best-first.
    ///
    /// This is used by the detail panel's provider badge list and the player
    /// bridge when multiple sources are available.
    /// Rank streams with optional provider health blending.
    /// Pass `health` to activate the 75% quality / 25% reliability blend.
    pub async fn ranked_streams(
        &self,
        entry_id: &str,
        policy:   &crate::quality::RankingPolicy,
        built_in: &[std::sync::Arc<dyn crate::providers::Provider>],
    ) -> Vec<crate::quality::StreamCandidate> {
        self.ranked_streams_with_circuit_breaker(entry_id, policy, built_in, None).await
    }

    /// Ranked streams with optional circuit breaker for failure tracking.
    pub async fn ranked_streams_with_circuit_breaker(
        &self,
        entry_id: &str,
        policy:   &crate::quality::RankingPolicy,
        built_in: &[std::sync::Arc<dyn crate::providers::Provider>],
        circuit_breaker: Option<&crate::providers::CircuitBreaker>,
    ) -> Vec<crate::quality::StreamCandidate> {
        // Check stream cache first
        if let Some(cached) = self.cache.streams.get(entry_id).await {
            return crate::quality::rank(cached, policy);
        }

        // Fan out to all stream-capable built-in providers concurrently.
        let mut set = tokio::task::JoinSet::new();
        let mut skipped_providers = vec![];
        let cb_clone = circuit_breaker.map(|cb| cb.clone());

        for provider in built_in.iter() {
            if !provider.has_streams() {
                continue;
            }
            let provider_name = provider.name().to_string();

            // Check circuit breaker if available
            if let Some(cb) = cb_clone.as_ref() {
                if !cb.is_available(&provider_name).await {
                    skipped_providers.push(provider_name);
                    continue;
                }
            }

            let p = std::sync::Arc::clone(provider);
            let id = entry_id.to_string();
            set.spawn(async move {
                let result = p.streams(&id).await;
                (provider_name, result)
            });
        }

        if !skipped_providers.is_empty() {
            info!(providers = ?skipped_providers, "circuit breakers open for providers");
        }

        let mut all_streams = vec![];
        while let Some(result) = set.join_next().await {
            match result {
                Ok((provider_name, Ok(mut streams))) => {
                    all_streams.append(&mut streams);
                    // Record success with circuit breaker
                    if let Some(cb) = &cb_clone {
                        cb.record_success(&provider_name).await;
                    }
                }
                Ok((provider_name, Err(e))) => {
                    warn!(provider = provider_name, err = %e, "stream provider error");
                    if let Some(cb) = &cb_clone {
                        cb.record_failure(&provider_name).await;
                    }
                }
                Err(e) => {
                    warn!("stream task panicked: {e}");
                }
            }
        }

        // Populate stream cache
        self.cache.streams.insert(entry_id, all_streams.clone()).await;

        // Use health-blended ranking when health data is available.
        // The HealthRegistry is injected here if the caller has one.
        // For now we call the plain rank() — Pipeline::resolve_streams records
        // outcomes via health.record_success() after each call.
        crate::quality::rank(all_streams, policy)
    }

    // ── Unified orchestration entry points ───────────────────────────────────
    //
    // These are the clean top-level methods that the IPC loop (and any future
    // client) should call.  They compose the lower-level cache / provider /
    // quality modules into complete pipelines so main.rs stays thin.

    /// Trigger a catalog refresh for `tab` from all registered built-in
    /// providers.  Results are broadcast via the catalog's watch channel
    /// rather than returned directly, keeping this call non-blocking.
    pub async fn get_catalog(
        &self,
        tab:     &crate::ipc::MediaTab,
        catalog: std::sync::Arc<crate::catalog::Catalog>,
    ) {
        catalog.refresh_tab(tab.clone()).await;
    }

    /// Full stream-resolution pipeline for a media item:
    ///
    /// 1. Check the stream cache (10-minute TTL).
    /// 2. Fan out concurrently to all built-in stream-capable providers.
    /// 3. Rank every candidate with the `quality` module.
    /// 4. Return the top-ranked `Stream`, or `None` if nothing was found.
    ///
    /// The caller (player bridge) then classifies the URL as torrent / HTTP
    /// and launches the appropriate playback pipeline.
    pub async fn resolve_best_stream(
        &self,
        entry_id: &str,
        policy:   &crate::quality::RankingPolicy,
        built_in: &[std::sync::Arc<dyn crate::providers::Provider>],
    ) -> Option<crate::providers::Stream> {
        self.ranked_streams(entry_id, policy, built_in)
            .await
            .into_iter()
            .next()
            .map(|c| c.stream)
    }

    /// Like `ranked_streams` but blends quality with provider reliability scores.
    ///
    /// `health_map` maps provider name → reliability (0.0–1.0).
    /// Providers with low scores are penalised even if they offer higher quality.
    pub async fn ranked_streams_with_health(
        &self,
        entry_id:   &str,
        policy:     &crate::quality::RankingPolicy,
        built_in:   &[std::sync::Arc<dyn crate::providers::Provider>],
        health_map: std::collections::HashMap<String, f64>,
    ) -> Vec<crate::quality::StreamCandidate> {
        // Check cache first
        if let Some(cached) = self.cache.streams.get(entry_id).await {
            return crate::quality::rank_with_health(cached, policy, Some(&health_map));
        }

        // Fan out to all stream-capable providers concurrently.
        let mut set = tokio::task::JoinSet::new();
        for provider in built_in.iter().filter(|p| p.has_streams()) {
            let p  = std::sync::Arc::clone(provider);
            let id = entry_id.to_string();
            set.spawn(async move { p.streams(&id).await });
        }
        let mut all_streams = vec![];
        while let Some(result) = set.join_next().await {
            match result {
                Ok(Ok(mut s)) => all_streams.append(&mut s),
                Ok(Err(e))    => warn!("stream provider error: {e}"),
                Err(e)        => warn!("stream task panicked: {e}"),
            }
        }
        self.cache.streams.insert(entry_id, all_streams.clone()).await;
        crate::quality::rank_with_health(all_streams, policy, Some(&health_map))
    }

}

