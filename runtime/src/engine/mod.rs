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

pub mod dispatch_map;
pub use dispatch_map::{DispatchMap, PluginEntryInfo};

pub mod pipeline;
#[allow(unused_imports)]
pub use pipeline::Pipeline;

pub mod search_scoped;
pub use search_scoped::{search_scoped, ScopedSearchConfig};

pub mod subtitles;

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
};
use crate::plugin::{ExecutionMode, LoadedPlugin};
use crate::plugin;
use crate::plugin::PluginMetaExt;
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

    /// Resolve a plugin identifier that may be either a registry UUID
    /// (preferred) or a manifest `name` (fallback). Returns the canonical
    /// UUID so callers can hit UUID-keyed maps (`sandbox`, `wasm_supervisors`).
    ///
    /// UUIDs take priority — they're guaranteed unique per load, whereas
    /// two installed plugins CAN share a manifest name (edge case; we
    /// return the first match). This lets ad-hoc IPC clients send a
    /// human-readable `"tmdb"` without having to round-trip `ListPlugins`
    /// first to cache the UUID map.
    pub fn resolve_id<'a>(&'a self, id_or_name: &'a str) -> Option<&'a str> {
        if self.plugins.contains_key(id_or_name) {
            return Some(id_or_name);
        }
        self.plugins
            .values()
            .find(|p| p.manifest.plugin.name == id_or_name)
            .map(|p| p.id.as_str())
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

    /// Find all plugins that have a given capability and are currently
    /// enabled. Disabled plugins stay in the registry (supervisor + sandbox
    /// intact) but are skipped by dispatch so the user can quickly
    /// pause/resume a plugin without a reload cycle.
    pub fn find_by_capability(&self, cap: crate::plugin::PluginCapability) -> Vec<&LoadedPlugin> {
        self.plugins.values()
            .filter(|p| p.enabled && p.has_capability(cap.clone()))
            .collect()
    }

    /// Mutable access to a plugin's `enabled` flag. Returns true if the
    /// plugin id was found. Callers should rebuild dispatch after flipping.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> bool {
        match self.plugins.get_mut(id) {
            Some(p) => {
                p.enabled = enabled;
                true
            }
            None => false,
        }
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

// ── SearchOptions ─────────────────────────────────────────────────────────────

/// Optional sort and filter parameters forwarded from the TUI search request.
///
/// All fields are optional; the defaults produce rating-sorted, unfiltered output.
/// Callers that do not need filtering (e.g. catalog trending refresh) should pass
/// `SearchOptions::default()`.
#[derive(Debug, Default)]
pub struct SearchOptions {
    /// Sort order applied after merging. Defaults to `SortOrder::Rating`.
    pub sort: crate::catalog_engine::SortOrder,
    /// Keep only entries whose genre contains this string (case-insensitive).
    pub genre: Option<String>,
    /// Exclude entries with a composite rating below this threshold (0.0–10.0).
    pub min_rating: Option<f64>,
    /// Keep only entries released from this year onward (inclusive).
    pub year_from: Option<u32>,
    /// Keep only entries released up to this year (inclusive).
    pub year_to: Option<u32>,
    /// When `false`, plugins tagged `"adult"` are skipped entirely.
    /// Defaults to `false` (adult content off by default).
    pub adult_content_enabled: bool,
}

/// Apply sort and filters from `options` to a list of already-merged entries.
/// Normalize genre + original_language for entries that come from anime-only
/// providers (kitsu, anilist). These providers' API responses are 100% anime
/// by definition, so we force the fields the classifier depends on — this
/// way the downstream dictionary rule (`is_anime_dominant`) catches them
/// even if the plugin didn't emit either field. For other providers the
/// plugin's emissions pass through unchanged.
///
/// Genre: preserved if it already contains any "anim" substring (so
/// "Anime, Action" stays intact); replaced with "Anime" otherwise.
/// Language: always forced to "ja" — kitsu/anilist catalogs are Japanese.
fn stamp_anime_fields(
    provider: &str,
    genre: Option<String>,
    original_language: Option<String>,
) -> (Option<String>, Option<String>) {
    if !matches!(provider, "kitsu" | "anilist") {
        return (genre, original_language);
    }
    let stamped_genre = match genre {
        Some(g) if g.to_lowercase().contains("anim") => Some(g),
        _ => Some("Anime".to_string()),
    };
    (stamped_genre, Some("ja".to_string()))
}

/// True when this entry is East-Asian animation ("anime" in the broad sense
/// — Japanese anime, Korean aeni, Chinese donghua). Classifier is
/// dictionary-only: `genre` contains "animation" or "anime" (case-
/// insensitive) AND `original_language` is one of `ja`/`jp`/`ko`/`zh`.
///
/// Responsibility shifts to plugins: every plugin that can emit anime MUST
/// populate both `genre` and `original_language` on those entries, or they
/// will be classified as general and escape the anime quota. Kitsu/anilist
/// entries are normalized upstream by `stamp_anime_fields` since those
/// providers ship 100% anime and their plugins don't always populate both
/// fields explicitly.
fn is_anime_dominant(entry: &crate::catalog::CatalogEntry) -> bool {
    let Some(genre) = entry.genre.as_deref() else {
        return false;
    };
    let genre_lower = genre.to_lowercase();
    let is_animation_genre =
        genre_lower.contains("animation") || genre_lower.contains("anime");
    if !is_animation_genre {
        return false;
    }
    let Some(lang) = entry.original_language.as_deref() else {
        return false;
    };
    let l = lang.to_ascii_lowercase();
    // Accept Japanese, Korean, Chinese variants. TMDB emits "ja" / "ko" /
    // "zh"; some sources use "jp" or region-tagged codes like "zh-cn".
    matches!(l.as_str(), "ja" | "jp" | "ko" | "zh" | "zh-cn" | "zh-tw" | "zh-hk")
}

/// Interleave anime-dominant entries with general ones so the final grid
/// follows the configured `anime_ratio` throughout — not just in aggregate.
/// Pattern derived dynamically: `round(ratio * 10)` anime slots per 10-slot
/// batch, remainder general. Once either bucket drains, the remainder fills
/// from the other. Preserves per-bucket order so the aggregator's merit
/// ranking isn't lost.
fn balance_anime_mix(
    entries: Vec<crate::catalog::CatalogEntry>,
    anime_ratio: f32,
) -> Vec<crate::catalog::CatalogEntry> {
    let ratio = anime_ratio.clamp(0.0, 1.0);
    let anime_per_batch = (ratio * 10.0).round() as usize;
    let general_per_batch = 10 - anime_per_batch;

    let (anime, general): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(is_anime_dominant);

    // Degenerate cases. If ratio excludes a bucket, don't emit from it; if
    // one bucket is naturally empty, emit the other straight through.
    if anime_per_batch == 0 {
        return general;
    }
    if general_per_batch == 0 {
        return anime;
    }
    if anime.is_empty() {
        return general;
    }
    if general.is_empty() {
        return anime;
    }

    let mut gi = general.into_iter();
    let mut ai = anime.into_iter();
    let mut out = Vec::new();
    // Enforce the configured ratio globally, not just per-batch. When either
    // bucket can't fill its share for the current batch, stop — otherwise
    // the surplus of the other bucket spills into the tail as "rows full of
    // anime" (or rows full of general), which is exactly what the ratio is
    // supposed to prevent. Tradeoff: entries beyond the ratio cap are
    // dropped from the grid. Users who want more anime can raise
    // `[catalog] anime_ratio` in stui.toml.
    'outer: loop {
        let mut gen_batch = Vec::with_capacity(general_per_batch);
        for _ in 0..general_per_batch {
            match gi.next() {
                Some(e) => gen_batch.push(e),
                None => {
                    // Emit partial general batch then stop.
                    out.append(&mut gen_batch);
                    break 'outer;
                }
            }
        }
        out.append(&mut gen_batch);

        let mut anime_batch = Vec::with_capacity(anime_per_batch);
        for _ in 0..anime_per_batch {
            match ai.next() {
                Some(e) => anime_batch.push(e),
                None => {
                    // Emit partial anime batch then stop.
                    out.append(&mut anime_batch);
                    break 'outer;
                }
            }
        }
        out.append(&mut anime_batch);
    }
    out
}

fn apply_search_options(
    options: &SearchOptions,
    entries: Vec<crate::catalog::CatalogEntry>,
) -> Vec<crate::catalog::CatalogEntry> {
    use crate::catalog_engine::{Filter, FilterSet};

    let mut fs = FilterSet::new();
    if let Some(g) = &options.genre {
        fs.add(Filter::genre(g));
    }
    if let Some(min) = options.min_rating {
        fs.add(Filter::min_rating(min));
    }
    match (options.year_from, options.year_to) {
        (Some(from), Some(to)) => fs.add(Filter::year_range(from, to)),
        (Some(from), None) => fs.add(Filter::year_from(from)),
        (None, Some(to)) => fs.add(Filter::year_to(to)),
        (None, None) => {}
    }
    options.sort.apply(fs.apply(entries))
}

/// Convert a list of `CatalogEntry` to `MediaEntry` for the wire response.
fn catalog_entries_to_media(
    entries: Vec<crate::catalog::CatalogEntry>,
    tab: &MediaTab,
) -> Vec<MediaEntry> {
    entries.into_iter().map(|e| MediaEntry {
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
        ratings:     e.ratings,
        imdb_id:     e.imdb_id,
        tmdb_id:     e.tmdb_id,
        original_language: e.original_language,
        kind:        Default::default(),
        source:      String::new(),
        artist_name: None,
        album_name:  None,
        track_number: None,
        season:      None,
        episode:     None,
    }).collect()
}

// ── Engine ───────────────────────────────────────────────────────────────────

use crate::cache::RuntimeCache;

/// Maximum number of concurrent WASM plugin calls allowed process-wide.
///
/// This semaphore is shared across all Engine clones (all clones hold an
/// `Arc` to the same `Semaphore` instance), so the bound is truly global.
///
/// All engine call-sites (search_catalog_entries, search_scoped, supervisor_search)
/// acquire from this shared semaphore before calling into a plugin.
pub const MAX_CONCURRENT_PLUGIN_CALLS: usize = 8;

// ── PluginCallError ───────────────────────────────────────────────────────────

/// Error type for `Engine::supervisor_search`.
#[derive(Debug)]
pub enum PluginCallError {
    /// No plugin with the given id is registered.
    PluginNotFound(String),
    /// The plugin does not support the requested scope.
    UnsupportedScope,
    /// The plugin call exceeded its timeout.
    Timeout,
    /// Any other failure (crash, serialisation error, etc.).
    Other(String),
}

impl std::fmt::Display for PluginCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PluginNotFound(id) => write!(f, "plugin '{}' not found", id),
            Self::UnsupportedScope   => write!(f, "plugin does not support this scope"),
            Self::Timeout            => write!(f, "plugin call timed out"),
            Self::Other(msg)         => write!(f, "{}", msg),
        }
    }
}

#[derive(Clone)]
pub struct Engine {
    registry:     Arc<RwLock<PluginRegistry>>,
    cache_dir:    std::path::PathBuf,
    data_dir:     std::path::PathBuf,
    /// In-memory TTL caches for search results, metadata, and stream URLs.
    pub cache:    RuntimeCache,
    /// Per-scope plugin dispatch map, rebuilt after every load/unload.
    dispatch_map: Arc<RwLock<DispatchMap>>,
    /// Process-wide semaphore limiting concurrent WASM plugin calls.
    ///
    /// All `Engine` clones share the same `Arc<Semaphore>` so the bound is
    /// global regardless of how many clones exist.  Initialised with
    /// `MAX_CONCURRENT_PLUGIN_CALLS` permits.
    plugin_semaphore: Arc<tokio::sync::Semaphore>,
    /// Fraction of Movies/Series grid reserved for anime-dominant entries.
    /// Sourced from `RuntimeConfig.catalog.anime_ratio`, clamped to [0.0, 1.0].
    /// See `balance_anime_mix`.
    anime_ratio: f32,
    /// Runtime-integrated TVDB client. `None` when the user hasn't configured
    /// a key — all plugin search still works; TVDB simply doesn't contribute
    /// to the fan-out. See `crate::tvdb` for storage and auth details.
    tvdb: Option<Arc<crate::tvdb::TvdbClient>>,
}

impl Engine {
    pub fn new(
        cache_dir: std::path::PathBuf,
        data_dir: std::path::PathBuf,
        anime_ratio: f32,
    ) -> Self {
        // Load TVDB using the project-embedded (XOR-obfuscated) key, with a
        // TVDB_API_KEY env var override for dev testing. Failure is non-fatal:
        // the runtime comes up without TVDB and plugin metadata still flows.
        let tvdb = match crate::tvdb::embedded_client() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(err = %e, "tvdb: construct failed — tvdb disabled");
                None
            }
        };
        // On-disk response cache (Phase 2). Survives daemon restart so the
        // catalog grid doesn't re-fetch providers for fresh-TTL keys. If the
        // DB can't be opened, fall back to mem-only — the runtime still works.
        let cache = match crate::cache::SqliteKv::open(&crate::cache::default_cache_db_path()) {
            Ok(kv) => RuntimeCache::with_disk(Arc::new(kv)),
            Err(e) => {
                tracing::warn!(err = %e, "response cache: disk tier unavailable — mem-only");
                RuntimeCache::new()
            }
        };
        Self {
            registry:     Arc::new(RwLock::new(PluginRegistry::default())),
            cache_dir,
            data_dir,
            cache,
            dispatch_map: Arc::new(RwLock::new(DispatchMap::default())),
            plugin_semaphore: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_PLUGIN_CALLS)),
            anime_ratio: anime_ratio.clamp(0.0, 1.0),
            tvdb,
        }
    }

    /// Return the TVDB client if configured. Exposed so the pipeline
    /// enrichment stage can use the same client instance (auth cache shared).
    pub fn tvdb(&self) -> Option<Arc<crate::tvdb::TvdbClient>> {
        self.tvdb.clone()
    }

    // ── Plugin lifecycle ──────────────────────────────────────────────────

    /// Access the plugin registry (read-only).
    pub fn registry(&self) -> &Arc<RwLock<PluginRegistry>> {
        &self.registry
    }

    /// Access the dispatch map (read-only).
    pub fn dispatch_map(&self) -> &Arc<RwLock<DispatchMap>> {
        &self.dispatch_map
    }

    /// Access the process-wide plugin call semaphore.
    ///
    /// `search_scoped` (Task 2.7) calls this to acquire a permit before each
    /// WASM plugin call.  Advanced callers that spawn their own tasks may also
    /// use it directly.
    pub fn plugin_semaphore(&self) -> &Arc<tokio::sync::Semaphore> {
        &self.plugin_semaphore
    }

    // ── Ergonomic dispatch_map wrappers ───────────────────────────────────

    /// Return the ordered list of plugin ids registered for `scope`.
    ///
    /// Plugins that declared no kinds in their manifest (`catalog = true`
    /// legacy form) are excluded — they never appear in any scope.
    pub async fn plugins_for_scope(&self, scope: stui_plugin_sdk::SearchScope) -> Vec<String> {
        self.dispatch_map.read().await.plugins_for(scope)
    }

    /// Return `true` when at least one plugin is registered for `scope`.
    pub async fn scope_has_any_plugins(&self, scope: stui_plugin_sdk::SearchScope) -> bool {
        !self.dispatch_map.read().await.is_empty_for(scope)
    }

    // ── supervisor_search ─────────────────────────────────────────────────

    /// Call a single WASM plugin's search via its supervisor.
    ///
    /// 1. Acquires a permit from the shared `plugin_semaphore` so at most
    ///    `MAX_CONCURRENT_PLUGIN_CALLS` calls run concurrently process-wide.
    /// 2. Looks the plugin up by id in the registry.
    /// 3. Builds `abi::types::SearchRequest` with `scope` directly — the ABI
    ///    now mirrors `sdk::SearchRequest` exactly (Task 7.0), so no tab-string
    ///    shim is needed.
    /// 4. Calls `WasmSupervisor::search` and maps `AbiError` variants to
    ///    `PluginCallError`.
    /// 5. Converts each `abi::types::PluginEntry` to `ipc::v1::MediaEntry`.
    ///
    /// Used by `search_scoped` (Task 2.7).
    pub async fn supervisor_search(
        &self,
        plugin_id: &str,
        query: &str,
        scope: stui_plugin_sdk::SearchScope,
    ) -> Result<Vec<crate::ipc::MediaEntry>, PluginCallError> {
        // Acquire a process-wide permit before touching the plugin.
        let _permit = self.plugin_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PluginCallError::Other("semaphore closed".into()))?;

        // Look up the supervisor under a short read-lock.  We clone the Arc
        // so we can drop the lock before the potentially-long supervisor call.
        // Accepts either a registry UUID or a manifest name via `resolve_id`.
        let sup = {
            let reg = self.registry.read().await;
            match reg.resolve_id(plugin_id) {
                Some(canonical) => reg.wasm_supervisor_for(canonical),
                None => return Err(PluginCallError::PluginNotFound(plugin_id.into())),
            }
        };

        let sup = sup.ok_or_else(|| PluginCallError::Other(
            format!("no WASM supervisor for plugin '{plugin_id}' — non-WASM or load failed"),
        ))?;

        // The ABI SearchRequest now carries scope directly (Task 7.0 sync).
        let req = crate::abi::SearchRequest {
            query: query.to_string(),
            scope,
            page: 0,
            limit: 100,
            per_scope_limit: None,
            locale: None,
        };

        let resp = sup.search(&req).await.map_err(map_abi_error)?;

        // Convert abi::types::PluginEntry → ipc::v1::MediaEntry.
        // Provider name comes from the plugin's display name, which we look
        // up under a second short read-lock.
        let provider_name = {
            let reg = self.registry.read().await;
            reg.get(plugin_id)
                .map(|p| p.manifest.plugin.name.clone())
                .unwrap_or_else(|| plugin_id.to_string())
        };

        let entries = resp.items
            .into_iter()
            .map(|e| abi_entry_to_media_entry(e, &provider_name))
            .collect();

        Ok(entries)
    }

    // ── Generic per-verb helper ───────────────────────────────────────────

    /// Internal helper shared by all per-verb supervisor helpers.
    ///
    /// Acquires the process-wide semaphore permit, looks up the WASM
    /// supervisor for `plugin_id`, invokes the async `call` closure with
    /// the `Arc<WasmSupervisor>`, and maps any `AbiError` to
    /// `PluginCallError`.
    ///
    /// The closure receives an owned `Arc<WasmSupervisor>` so it can hold
    /// a reference across the await point without borrowing `self`.
    async fn call_plugin_verb<F, Fut, R>(
        &self,
        plugin_id: &str,
        call: F,
    ) -> Result<R, PluginCallError>
    where
        F:   FnOnce(Arc<WasmSupervisor>) -> Fut,
        Fut: std::future::Future<Output = Result<R, crate::abi::types::AbiError>>,
    {
        // Acquire a process-wide permit before touching any plugin.
        let _permit = self.plugin_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PluginCallError::Other("semaphore closed".into()))?;

        // Look up the supervisor under a short read-lock.  Clone the Arc so
        // the lock is released before the potentially-long supervisor call.
        // Accepts either a registry UUID or a manifest name via `resolve_id`.
        let sup = {
            let reg = self.registry.read().await;
            match reg.resolve_id(plugin_id) {
                Some(canonical) => reg.wasm_supervisor_for(canonical),
                None => return Err(PluginCallError::PluginNotFound(plugin_id.into())),
            }
        };

        let sup = sup.ok_or_else(|| PluginCallError::Other(
            format!("no WASM supervisor for plugin '{plugin_id}' — non-WASM or load failed"),
        ))?;

        call(sup).await.map_err(map_abi_error)
    }

    // ── supervisor_lookup / enrich / get_artwork / get_credits / related ──

    /// Call a single WASM plugin's `lookup` verb via its supervisor.
    ///
    /// Returns the single [`crate::abi::types::PluginEntry`] from
    /// [`crate::abi::types::LookupResponse::entry`].
    pub async fn supervisor_lookup(
        &self,
        plugin_id: &str,
        req: crate::abi::types::LookupRequest,
    ) -> Result<crate::abi::types::PluginEntry, PluginCallError> {
        self.call_plugin_verb(plugin_id, |sup| async move {
            sup.lookup(&req).await.map(|resp| resp.entry)
        })
        .await
    }

    /// Call a single WASM plugin's `enrich` verb via its supervisor.
    ///
    /// Returns the enriched [`crate::abi::types::PluginEntry`] from
    /// [`crate::abi::types::EnrichResponse::entry`].
    pub async fn supervisor_enrich(
        &self,
        plugin_id: &str,
        req: crate::abi::types::EnrichRequest,
    ) -> Result<crate::abi::types::PluginEntry, PluginCallError> {
        self.call_plugin_verb(plugin_id, |sup| async move {
            sup.enrich(&req).await.map(|resp| resp.entry)
        })
        .await
    }

    /// Call a single WASM plugin's `get_artwork` verb via its supervisor.
    ///
    /// Returns the full [`crate::abi::types::ArtworkResponse`]; callers decide
    /// which artwork variant to use.
    pub async fn supervisor_get_artwork(
        &self,
        plugin_id: &str,
        req: crate::abi::types::ArtworkRequest,
    ) -> Result<crate::abi::types::ArtworkResponse, PluginCallError> {
        self.call_plugin_verb(plugin_id, |sup| async move {
            sup.get_artwork(&req).await
        })
        .await
    }

    /// Call a single WASM plugin's `get_credits` verb via its supervisor.
    ///
    /// Returns the full [`crate::abi::types::CreditsResponse`] (cast + crew).
    pub async fn supervisor_get_credits(
        &self,
        plugin_id: &str,
        req: crate::abi::types::CreditsRequest,
    ) -> Result<crate::abi::types::CreditsResponse, PluginCallError> {
        self.call_plugin_verb(plugin_id, |sup| async move {
            sup.get_credits(&req).await
        })
        .await
    }

    /// Call a single WASM plugin's `related` verb via its supervisor.
    ///
    /// Returns the list of related entries extracted from
    /// [`crate::abi::types::RelatedResponse::items`].
    pub async fn supervisor_related(
        &self,
        plugin_id: &str,
        req: crate::abi::types::RelatedRequest,
    ) -> Result<Vec<crate::abi::types::PluginEntry>, PluginCallError> {
        self.call_plugin_verb(plugin_id, |sup| async move {
            sup.related(&req).await.map(|resp| resp.items)
        })
        .await
    }

    /// Rebuild the dispatch map from the current registry contents.
    ///
    /// Called after every `load_plugin` / `unload_plugin` so that
    /// `dispatch_map` is always consistent with the live plugin set.
    async fn rebuild_dispatch_map(&self, reg: &PluginRegistry) {
        let infos: Vec<PluginEntryInfo> = reg.all().map(|p| PluginEntryInfo {
            id:    p.id.clone(),
            kinds: p.manifest.capabilities.catalog.kinds().to_vec(),
        }).collect();
        *self.dispatch_map.write().await = DispatchMap::build(&infos);
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
            enabled: true,
        };

        let ctx = SandboxCtx::new(
            &loaded,
            self.cache_dir.clone(),
            self.data_dir.clone(),
        );
        ctx.ensure_dirs()?;

        info!(plugin_id = %id, plugin = %name, "plugin loaded");

        // ── Slow path: build the WASM supervisor WITHOUT the registry
        //    write lock held. Instantiating wasmtime + calling `Plugin::init`
        //    can take hundreds of ms to several seconds per plugin; holding
        //    the write lock across those awaits serializes startup and
        //    starves every concurrent `list_plugins` / catalog read behind
        //    the ongoing scan. We do the slow work lock-free, then grab
        //    the write lock just long enough to splice the result in. ──
        let supervisor: Option<Arc<WasmSupervisor>> =
            if matches!(loaded.mode, ExecutionMode::Wasm) {
                let sup_cfg = WasmSupervisorConfig::default();
                let wasm_path = loaded.entrypoint.clone();
                let pname = name.clone();
                let sup_ctx = ctx.clone();
                let sup_rate_limit = loaded.manifest.rate_limit.clone();

                match WasmSupervisor::load(wasm_path, pname.clone(), sup_ctx, sup_cfg, sup_rate_limit.as_ref()).await {
                    Ok(sup) => {
                        // Config/env resolution uses the four-level precedence
                        // from `plugin::state::resolve_config`; TUI overrides
                        // will land in a later chunk via StateStore. The
                        // effective map is surfaced as both `env` (string) and
                        // `config` (JSON string values) so plugins can read
                        // either without pulling the `toml` crate.
                        let resolved = crate::plugin::state::resolve_config(
                            &loaded.manifest,
                            &HashMap::new(),
                            crate::config::secrets::env_lookup,
                        );
                        let init_req = crate::abi::InitRequest {
                            env: resolved.clone(),
                            config: resolved
                                .into_iter()
                                .map(|(k, v)| (k, serde_json::Value::String(v)))
                                .collect(),
                            cache_dir: ctx.cache_dir.clone(),
                        };
                        match sup.init(&init_req).await {
                            Ok(()) => {
                                info!(plugin = %pname, "plugin init ok");
                                Some(Arc::new(sup))
                            }
                            Err(crate::abi::InitError::Plugin(
                                crate::abi::PluginInitError::MissingConfig { fields, hint },
                            )) => {
                                warn!(
                                    plugin  = %pname,
                                    missing = ?fields,
                                    hint    = ?hint,
                                    "plugin init reports missing config — set fields via TUI then reload"
                                );
                                None
                            }
                            Err(crate::abi::InitError::Plugin(
                                crate::abi::PluginInitError::Fatal(msg),
                            )) => {
                                warn!(plugin = %pname, err = %msg, "plugin init fatal — unavailable until reload");
                                None
                            }
                            Err(crate::abi::InitError::Abi(abi_err)) => {
                                warn!(plugin = %pname, err = %abi_err, "plugin init plumbing error — unavailable until reload");
                                None
                            }
                        }
                    }
                    Err(e) => {
                        warn!(plugin = %pname, err = %e, "WASM supervisor load failed — plugin unavailable until reload");
                        None
                    }
                }
            } else {
                None
            };

        // ── Fast path: take the write lock only long enough to splice
        //    the finished plugin + supervisor into the registry. ──
        {
            let mut reg = self.registry.write().await;
            if let Some(sup) = supervisor {
                reg.insert_wasm_supervisor(&id, sup);
            }
            reg.insert(loaded, ctx);
            self.rebuild_dispatch_map(&reg).await;
        }

        Ok(Response::PluginLoaded(PluginLoadedResponse {
            plugin_id: id,
            name,
        }))
    }

    pub async fn unload_plugin(&self, plugin_id: &str) -> Result<Response> {
        // Snapshot the plugin dir before removing, so we can delete it
        // from disk outside the write lock. We keep the lock hold as
        // short as possible; disk I/O happens after release.
        let dir_to_delete = {
            let mut reg = self.registry.write().await;
            match reg.remove(plugin_id) {
                Some(p) => {
                    info!(
                        plugin_id = %plugin_id,
                        plugin = %p.manifest.plugin.name,
                        "plugin unloaded",
                    );
                    self.rebuild_dispatch_map(&reg).await;
                    Some(p.dir)
                }
                None => bail!("Plugin '{}' not found", plugin_id),
            }
        };

        // Uninstall semantics: delete the plugin directory from disk
        // so the next runtime restart doesn't re-scan and reload it,
        // and so browse_registry's "installed" check (keyed on the
        // engine registry) stays consistent with the filesystem. The
        // watcher is set up to drop the path from its `seen` set on
        // the resulting fs remove event, so reinstall works cleanly.
        if let Some(dir) = dir_to_delete {
            if dir.exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                    warn!(
                        dir  = %dir.display(),
                        err  = %e,
                        "failed to delete plugin dir — plugin is unloaded from \
                         memory but the files remain; restart will re-load it",
                    );
                } else {
                    info!(dir = %dir.display(), "plugin dir removed");
                }
            }
        }

        Ok(Response::PluginUnloaded(PluginUnloadedResponse {
            plugin_id: plugin_id.to_string(),
        }))
    }

    /// Read-only access to the plugin registry for callers outside the
    /// engine module that need to query live plugin state (e.g.
    /// `run_browse_registry` marking `installed=true` for currently-loaded
    /// plugins). Keeps `self.registry` private while letting the
    /// pipeline see the same source of truth as `list_plugins`.
    pub async fn registry_read(&self) -> tokio::sync::RwLockReadGuard<'_, PluginRegistry> {
        self.registry.read().await
    }

    pub async fn list_plugins(&self) -> Response {
        let reg = self.registry.read().await;
        let plugins: Vec<PluginInfo> = reg
            .all()
            .map(|p| PluginInfo {
                id: p.id.clone(),
                name: p.manifest.plugin.name.clone(),
                version: p.manifest.plugin.version.clone(),
                plugin_type: p.manifest.plugin.plugin_type_str(),
                status: if p.enabled { PluginStatus::Loaded } else { PluginStatus::Disabled },
                enabled: p.enabled,
                tags: p.manifest.plugin.tags.clone(),
                description: p.manifest.plugin.description.clone().unwrap_or_default(),
                author: p.manifest.meta.as_ref()
                    .and_then(|m| m.author.as_deref())
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect();
        Response::PluginList(PluginListResponse { plugins })
    }

    /// Toggle whether a plugin participates in dispatch. The plugin
    /// stays loaded (supervisor + sandbox intact); only capability
    /// lookups start skipping it. Dispatch map is rebuilt so per-scope
    /// routing tables reflect the new enabled set immediately.
    pub async fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<Response> {
        let mut reg = self.registry.write().await;
        let canonical = match reg.resolve_id(plugin_id) {
            Some(id) => id.to_string(),
            None => bail!("Plugin '{}' not found", plugin_id),
        };
        if !reg.set_enabled(&canonical, enabled) {
            bail!("Plugin '{}' not found", plugin_id);
        }
        info!(plugin_id = %canonical, enabled, "plugin enabled flag toggled");
        self.rebuild_dispatch_map(&reg).await;
        Ok(Response::PluginEnabled(crate::ipc::PluginEnabledResponse {
            plugin_id: canonical,
            enabled,
        }))
    }

    // ── Search (catalog fan-out) ──────────────────────────────────────────

    /// Fan out `query` across all Catalog-capable providers for `tab`, merge,
    /// dedup, and return the sorted `CatalogEntry` list.
    ///
    /// This is the internal helper that `catalog.rs` (trending refresh) and
    /// `engine/pipeline.rs` (paged search) both use.  It replaces the retired
    /// `Engine::search` which wrapped the same logic in `Response::SearchResult`.
    ///
    /// Callers are responsible for their own paging — slice the returned `Vec`
    /// with `skip(offset).take(limit)` as appropriate.
    #[tracing::instrument(
        name = "engine.search_catalog_entries",
        skip(self, tab, options),
        fields(query = %query),
    )]
    pub async fn search_catalog_entries(
        &self,
        query: &str,
        tab: &MediaTab,
        options: SearchOptions,
    ) -> Vec<crate::catalog::CatalogEntry> {
        // Scope used uniformly for cache keying (plugin + TVDB share one tab→
        // scope mapping). Plugins derive the same scope inside their task;
        // keeping the derivation here too lets the cache key be computed
        // before any spawn/dispatch.
        let scope = match tab {
            crate::ipc::MediaTab::Music  => stui_plugin_sdk::SearchScope::Track,
            crate::ipc::MediaTab::Movies => stui_plugin_sdk::SearchScope::Movie,
            crate::ipc::MediaTab::Series => stui_plugin_sdk::SearchScope::Series,
            _ => stui_plugin_sdk::SearchScope::Track,
        };

        // ── Live fan-out ──────────────────────────────────────────────────
        let reg = self.registry.read().await;
        let providers = reg.find_providers_for_tab(tab);

        let mut set = tokio::task::JoinSet::new();
        let sem = Arc::clone(&self.plugin_semaphore);
        // Entries served straight from the in-memory SearchCache. Merged
        // with JoinSet results before the aggregator runs, so a source
        // that hit the cache is indistinguishable downstream from a fresh
        // network response.
        let mut cached_entries: Vec<MediaEntry> = Vec::new();

        // TVDB always-on fallback. Only dispatched for movie/series tabs since
        // TVDB has no music coverage. Also skipped when the query is empty:
        // TVDB's /search endpoint is a free-text endpoint that rejects empty
        // queries with HTTP 400 ("query is required"). Unlike TMDB, TVDB v4
        // has no equivalent of /trending we can hit during the tab's initial
        // catalog refresh. When the user types a real query, TVDB joins the
        // fan-out normally.
        if let Some(tvdb) = self.tvdb.clone().filter(|_| !query.trim().is_empty()) {
            let kind = match tab {
                crate::ipc::MediaTab::Movies => Some(crate::tvdb::SearchKind::Movie),
                crate::ipc::MediaTab::Series => Some(crate::tvdb::SearchKind::Series),
                _ => None,
            };
            if let Some(kind) = kind {
                let cache_key = crate::cache::search::SearchKey::new("tvdb", query, scope, 0);
                if let Some(cached) = self.cache.search.get(&cache_key).await {
                    cached_entries.extend(cached);
                } else {
                    let q = query.to_string();
                    let tab_out = tab.clone();
                    let cache = self.cache.search.clone();
                    let cache_key_for_task = cache_key.clone();
                    set.spawn(async move {
                        // Explicit generic args on Ok/Err pin the closure's return
                        // type for the JoinSet. Without them rustc sees multiple
                        // plausible error types and can't infer the whole chain.
                        let items: Vec<crate::tvdb::TvdbEntry> =
                            match tvdb.search(&q, kind, 30).await {
                                Ok(v) => v,
                                Err(e) => {
                                    return (
                                        "tvdb".to_string(),
                                        Err::<Vec<MediaEntry>, anyhow::Error>(e),
                                    );
                                }
                            };
                        let entries: Vec<MediaEntry> = items
                            .into_iter()
                            .map(|e| MediaEntry {
                                id: format!("tvdb-{}", e.tvdb_id),
                                title: e.title,
                                year: e.year,
                                genre: if e.genres.is_empty() {
                                    None
                                } else {
                                    Some(e.genres.join(", "))
                                },
                                rating: None,
                                description: e.overview,
                                poster_url: e.image_url,
                                provider: "tvdb".to_string(),
                                tab: tab_out.clone(),
                                media_type: crate::ipc::MediaType::default(),
                                ratings: std::collections::HashMap::new(),
                                imdb_id: e.imdb_id,
                                tmdb_id: e.tmdb_id,
                                original_language: e.original_language,
                                kind: stui_plugin_sdk::EntryKind::default(),
                                source: "tvdb".to_string(),
                                artist_name: None,
                                album_name: None,
                                track_number: None,
                                season: None,
                                episode: None,
                            })
                            .collect();
                        cache.insert(cache_key_for_task, entries.clone()).await;
                        ("tvdb".to_string(), Ok::<_, anyhow::Error>(entries))
                    });
                }
            }
        }

        if providers.is_empty() && set.is_empty() && cached_entries.is_empty() {
            return vec![];
        }

        for plugin in &providers {
            // Skip plugins tagged "adult" when adult content is disabled.
            if !options.adult_content_enabled
                && plugin.manifest.plugin.tags.iter().any(|t| t.eq_ignore_ascii_case("adult"))
            {
                continue;
            }
            let plugin_clone = (*plugin).clone();

            // Per-plugin cache lookup. If fresh entries for this (plugin,
            // query, scope) are still in the in-mem cache we bypass the
            // WASM call entirely — no network, no plugin init cost.
            let plugin_name = plugin_clone.manifest.plugin.name.clone();
            let cache_key = crate::cache::search::SearchKey::new(&plugin_name, query, scope, 0);
            if let Some(cached) = self.cache.search.get(&cache_key).await {
                cached_entries.extend(cached);
                continue;
            }

            let q = query.to_string();
            let t = tab.clone();

            match plugin_clone.mode {
                ExecutionMode::Wasm => {
                    let sup = reg.wasm_supervisor_for(&plugin_clone.id);
                    if let Some(sup) = sup {
                        let provider = plugin_clone.manifest.plugin.name.clone();
                        let tab_out  = t.clone();
                        let pname = provider.clone();
                        let sem = Arc::clone(&sem);
                        let cache = self.cache.search.clone();
                        let cache_key_for_task = cache_key.clone();
                        set.spawn(async move {
                            let _permit = sem.acquire_owned().await;
                            use futures::FutureExt as _;
                            let result = std::panic::AssertUnwindSafe(async move {
                                // Derive scope from tab; catalog walk uses Track as default.
                                let scope = match t {
                                    crate::ipc::MediaTab::Music    => stui_plugin_sdk::SearchScope::Track,
                                    crate::ipc::MediaTab::Movies   => stui_plugin_sdk::SearchScope::Movie,
                                    crate::ipc::MediaTab::Series   => stui_plugin_sdk::SearchScope::Series,
                                    _ => stui_plugin_sdk::SearchScope::Track,
                                };
                                let req = SearchRequest {
                                    query: q,
                                    scope,
                                    page: 0,
                                    limit: 50,
                                    per_scope_limit: None,
                                    locale: None,
                                };
                                sup.search(&req).await
                                    .map(|r| r.items.into_iter().map(|e| MediaEntry {
                                        id:          e.id,
                                        title:       e.title,
                                        year:        e.year.map(|y| y.to_string()),
                                        genre:       e.genre,
                                        rating:      e.rating.map(|r| r.to_string()),
                                        description: e.description,
                                        poster_url:  e.poster_url,
                                        provider:    provider.clone(),
                                        tab:         tab_out.clone(),
                                        media_type:  crate::ipc::MediaType::default(),
                                        ratings:     std::collections::HashMap::new(),
                                        imdb_id:     e.imdb_id,
                                        tmdb_id:     None,
                                        original_language: e.original_language,
                                        kind:        e.kind,
                                        source:      e.source,
                                        artist_name: e.artist_name,
                                        album_name:  e.album_name,
                                        track_number: e.track_number,
                                        season:      e.season,
                                        episode:     e.episode,
                                    }).collect::<Vec<_>>())
                                    .map_err(|e| anyhow::anyhow!("{e}"))
                            })
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| Err(anyhow::anyhow!("provider task panicked")));
                            // Write-through on success only. Errors/panics must NOT
                            // poison the cache — next query re-hits the provider.
                            if let Ok(entries) = &result {
                                cache.insert(cache_key_for_task, entries.clone()).await;
                            }
                            (pname, result)
                        });
                    } else {
                        warn!(plugin = %plugin_clone.manifest.plugin.name, "no WASM supervisor — skipping");
                    }
                }
                _ => {
                    let sandbox = reg.sandbox_for(&plugin_clone.id).cloned();
                    if let Some(ctx) = sandbox {
                        let pname = plugin_clone.manifest.plugin.name.clone();
                        let sem = Arc::clone(&sem);
                        let cache = self.cache.search.clone();
                        let cache_key_for_task = cache_key.clone();
                        set.spawn(async move {
                            let _permit = sem.acquire_owned().await;
                            use futures::FutureExt as _;
                            let result = std::panic::AssertUnwindSafe(
                                scraper::search(&ctx, &plugin_clone, &q, &t)
                            )
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| Err(anyhow::anyhow!("provider task panicked")));
                            if let Ok(entries) = &result {
                                cache.insert(cache_key_for_task, entries.clone()).await;
                            }
                            (pname, result)
                        });
                    }
                }
            }
        }
        drop(reg);

        // Collect results in completion order — fastest provider wins the front.
        // Cache-served entries jump to the front since they require no network
        // wait — the aggregator's dedup will handle ordering correctly.
        let mut all_items: Vec<crate::ipc::MediaEntry> = cached_entries;
        while let Some(result) = set.join_next().await {
            match result {
                Ok((_, Ok(mut items))) => all_items.append(&mut items),
                Ok((provider, Err(e))) => warn!(provider = %provider, "provider search error: {e}"),
                Err(e) => warn!("search task aborted: {e}"),
            }
        }

        // ── Aggregate ────────────────────────────────────────────────────
        // Stamp anime-only providers (kitsu/anilist) with normalized
        // genre+language BEFORE per-entry MediaEntry→CatalogEntry conversion.
        // Applied here (post JoinSet collect) so every provider path — WASM
        // plugin, scraper, TVDB, snapshot-replayed entries — flows through
        // the same normalization step. Previously this lived inline in the
        // WASM arm only, leaving the scraper arm unstamped.
        let tab_str = format!("{:?}", tab).to_lowercase();
        let raw_entries: Vec<crate::catalog::CatalogEntry> = all_items.into_iter().map(|e| {
            let (genre, original_language) =
                stamp_anime_fields(&e.provider, e.genre, e.original_language);
            crate::catalog::CatalogEntry {
                id:          e.id,
                title:       e.title,
                year:        e.year,
                genre,
                rating:      e.rating,
                description: e.description,
                poster_url:  e.poster_url,
                poster_art:  None,
                provider:    e.provider,
                tab:         tab_str.clone(),
                imdb_id:     e.imdb_id,
                tmdb_id:     e.tmdb_id,
                media_type:  e.media_type,
                ratings:     e.ratings,
                original_language,
            }
        }).collect();

        // Merge: dedup by IMDB id / title+year, fill sparse fields, compute
        // weighted-median composite rating from all per-source scores.
        let merged = crate::catalog_engine::CatalogAggregator::new().merge(raw_entries);

        // Filter + sort FIRST — the default sort is by rating desc, so we
        // want each bucket ordered by quality before the interleave picks
        // from them. Running balance before sort would let the sort re-cluster
        // entries by rating (TMDB's 8+ scores in front, anilist's placeholders
        // at the back), which is exactly the "rows full of anime at the
        // bottom" symptom we're trying to fix.
        let sorted = apply_search_options(&options, merged);

        // Balance anime vs. general in Movies/Series tabs so plugins like
        // kitsu/anilist can't drown out TMDB/OMDB/TVDB. Applied LAST so the
        // interleave isn't undone by the sort. Music tab and others are
        // untouched — no "anime" classification applies there.
        // Ratio sourced from `RuntimeConfig.catalog.anime_ratio`.
        match tab {
            crate::ipc::MediaTab::Movies | crate::ipc::MediaTab::Series => {
                balance_anime_mix(sorted, self.anime_ratio)
            }
            _ => sorted,
        }
    }

    // ── Resolve ───────────────────────────────────────────────────────────

    #[tracing::instrument(
        name = "engine.resolve_stream",
        skip(self),
        fields(entry_id = %entry_id, provider = %provider_name, req_id = %req_id),
    )]
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
        let sem = Arc::new(tokio::sync::Semaphore::new(8)); // Limit concurrent stream requests
        let mut skipped_providers = vec![];
        let cb_clone = circuit_breaker.cloned();

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
            let sem = Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await;
                use futures::FutureExt as _;
                let result = std::panic::AssertUnwindSafe(p.streams(&id))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("provider task panicked")));
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
                    // Task was aborted (panics are caught inside the task and converted to Err).
                    warn!("stream task aborted: {e}");
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
        let sem = Arc::new(tokio::sync::Semaphore::new(8)); // Limit concurrent stream requests
        for provider in built_in.iter().filter(|p| p.has_streams()) {
            let p             = std::sync::Arc::clone(provider);
            let id            = entry_id.to_string();
            let provider_name = provider.name().to_string();
            let sem = Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await;
                use futures::FutureExt as _;
                let result = std::panic::AssertUnwindSafe(p.streams(&id))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("provider task panicked")));
                (provider_name, result)
            });
        }
        let mut all_streams = vec![];
        while let Some(result) = set.join_next().await {
            match result {
                Ok((_, Ok(mut s)))         => all_streams.append(&mut s),
                Ok((provider, Err(e)))     => warn!(provider = %provider, "stream provider error: {e}"),
                Err(e)                     => warn!("stream task aborted: {e}"),
            }
        }
        self.cache.streams.insert(entry_id, all_streams.clone()).await;
        crate::quality::rank_with_health(all_streams, policy, Some(&health_map))
    }

}

// ── Free helpers for supervisor_search ───────────────────────────────────────

/// Map an `AbiError` to a `PluginCallError`.
///
/// An `AbiError::Execution` whose message contains the well-known SDK error
/// code `"unsupported_scope"` maps to `PluginCallError::UnsupportedScope`.
/// A message that contains "timed out" maps to `Timeout`.
/// Everything else maps to `Other`.
fn map_abi_error(e: crate::abi::types::AbiError) -> PluginCallError {
    use crate::abi::types::AbiError;
    match e {
        AbiError::Execution(ref msg) => {
            if msg.contains(stui_plugin_sdk::error_codes::UNSUPPORTED_SCOPE) {
                PluginCallError::UnsupportedScope
            } else if msg.contains("timed out") {
                PluginCallError::Timeout
            } else {
                PluginCallError::Other(msg.clone())
            }
        }
        other => PluginCallError::Other(other.to_string()),
    }
}

/// Convert an `abi::types::PluginEntry` to an `ipc::v1::MediaEntry`.
///
/// With Task 7.0 ABI sync, `PluginEntry` now carries `kind` and `source`
/// directly from the plugin, typed numeric `year`/`rating`, and all per-kind
/// optional fields.  `MediaEntry.year` and `.rating` remain `Option<String>`
/// on the wire (Go side), so we stringify the numeric values here.
///
/// The `scope` parameter that was previously needed to derive `kind` is removed
/// — the plugin supplies `kind` directly.  If a pre-7.1 plugin returns a
/// default `EntryKind` (Track), that is a visible bug that forces the plugin
/// author to migrate.
fn abi_entry_to_media_entry(
    e:             crate::abi::types::PluginEntry,
    provider_name: &str,
) -> crate::ipc::MediaEntry {
    // Derive MediaEntry.tab from the plugin-supplied kind so the TUI
    // renders the entry in the correct tab.
    let tab = match e.kind {
        stui_plugin_sdk::EntryKind::Artist
        | stui_plugin_sdk::EntryKind::Album
        | stui_plugin_sdk::EntryKind::Track  => crate::ipc::MediaTab::Music,
        stui_plugin_sdk::EntryKind::Movie    => crate::ipc::MediaTab::Movies,
        stui_plugin_sdk::EntryKind::Series
        | stui_plugin_sdk::EntryKind::Episode => crate::ipc::MediaTab::Series,
    };

    let (genre, original_language) =
        stamp_anime_fields(provider_name, e.genre, e.original_language);

    crate::ipc::MediaEntry {
        id:           e.id,
        title:        e.title,
        year:         e.year.map(|y| y.to_string()),
        genre,
        rating:       e.rating.map(|r| r.to_string()),
        description:  e.description,
        poster_url:   e.poster_url,
        provider:     provider_name.to_string(),
        tab,
        media_type:   crate::ipc::MediaType::default(),
        ratings:      std::collections::HashMap::new(),
        imdb_id:      e.imdb_id,
        tmdb_id:      None,
        original_language,
        kind:         e.kind,
        source:       e.source,
        artist_name:  e.artist_name,
        album_name:   e.album_name,
        track_number: e.track_number,
        season:       e.season,
        episode:      e.episode,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod supervisor_search_tests {
    use super::*;
    use stui_plugin_sdk::SearchScope;

    // ── map_abi_error ─────────────────────────────────────────────────────────

    #[test]
    fn map_abi_error_unsupported_scope() {
        let e = crate::abi::types::AbiError::Execution(
            format!("{}: track scope unsupported", stui_plugin_sdk::error_codes::UNSUPPORTED_SCOPE),
        );
        assert!(matches!(map_abi_error(e), PluginCallError::UnsupportedScope));
    }

    #[test]
    fn map_abi_error_timeout() {
        let e = crate::abi::types::AbiError::Execution(
            "plugin 'foo' search timed out after 30s".into(),
        );
        assert!(matches!(map_abi_error(e), PluginCallError::Timeout));
    }

    #[test]
    fn map_abi_error_other() {
        let e = crate::abi::types::AbiError::Execution("some random failure".into());
        assert!(matches!(map_abi_error(e), PluginCallError::Other(_)));
    }

    #[test]
    fn map_abi_error_version_mismatch_becomes_other() {
        let e = crate::abi::types::AbiError::VersionMismatch { plugin: 1, host: 2 };
        assert!(matches!(map_abi_error(e), PluginCallError::Other(_)));
    }

    // ── abi_entry_to_media_entry ──────────────────────────────────────────────
    // PluginEntry now uses typed numerics (year: Option<u32>, rating: Option<f32>)
    // and carries kind + source directly from the plugin.

    #[test]
    fn abi_entry_maps_fields_correctly() {
        let entry = crate::abi::types::PluginEntry {
            id:          "tt1234".into(),
            kind:        stui_plugin_sdk::EntryKind::Track,
            title:       "Creep".into(),
            source:      "lastfm".into(),
            year:        Some(1993),
            genre:       Some("Rock".into()),
            rating:      Some(9.0),
            description: Some("A song".into()),
            poster_url:  None,
            imdb_id:     Some("tt1234".into()),
            ..Default::default()
        };
        let me = abi_entry_to_media_entry(entry, "lastfm");
        assert_eq!(me.id, "tt1234");
        assert_eq!(me.title, "Creep");
        // year is stringified from the numeric ABI field
        assert_eq!(me.year, Some("1993".into()));
        assert_eq!(me.provider, "lastfm");
        // source comes from the plugin entry directly
        assert_eq!(me.source, "lastfm");
        assert_eq!(me.kind, stui_plugin_sdk::EntryKind::Track);
        assert!(matches!(me.tab, crate::ipc::MediaTab::Music));
        assert_eq!(me.imdb_id, Some("tt1234".into()));
    }

    #[test]
    fn abi_entry_movie_kind_gets_movies_tab() {
        let entry = crate::abi::types::PluginEntry {
            id:    "m1".into(),
            kind:  stui_plugin_sdk::EntryKind::Movie,
            title: "Interstellar".into(),
            source: "tmdb".into(),
            ..Default::default()
        };
        let me = abi_entry_to_media_entry(entry, "tmdb");
        assert_eq!(me.kind, stui_plugin_sdk::EntryKind::Movie);
        assert!(matches!(me.tab, crate::ipc::MediaTab::Movies));
    }

    #[test]
    fn abi_entry_series_kind_gets_series_tab() {
        let entry = crate::abi::types::PluginEntry {
            id:    "s1".into(),
            kind:  stui_plugin_sdk::EntryKind::Series,
            title: "Breaking Bad".into(),
            source: "tmdb".into(),
            ..Default::default()
        };
        let me = abi_entry_to_media_entry(entry, "tmdb");
        assert_eq!(me.kind, stui_plugin_sdk::EntryKind::Series);
        assert!(matches!(me.tab, crate::ipc::MediaTab::Series));
    }

    #[test]
    fn abi_entry_per_kind_fields_forwarded() {
        let entry = crate::abi::types::PluginEntry {
            id:           "t1".into(),
            kind:         stui_plugin_sdk::EntryKind::Track,
            title:        "My Song".into(),
            source:       "musicplugin".into(),
            artist_name:  Some("Radiohead".into()),
            album_name:   Some("OK Computer".into()),
            track_number: Some(3),
            ..Default::default()
        };
        let me = abi_entry_to_media_entry(entry, "musicplugin");
        assert_eq!(me.artist_name, Some("Radiohead".into()));
        assert_eq!(me.album_name,  Some("OK Computer".into()));
        assert_eq!(me.track_number, Some(3));
    }

    // ── PluginRegistry::resolve_id ────────────────────────────────────────────

    fn mini_loaded_plugin(id: &str, name: &str) -> LoadedPlugin {
        use crate::plugin::manifest::{Capabilities, PluginManifest, PluginMeta};
        LoadedPlugin {
            id: id.into(),
            manifest: PluginManifest {
                plugin: PluginMeta {
                    name: name.into(),
                    version: "0.0.0".into(),
                    plugin_type: None,
                    entrypoint: "plugin.wasm".into(),
                    description: None,
                    tags: Vec::new(),
                    _author: None,
                    _abi_version: None,
                },
                permissions: None,
                meta: None,
                env: Default::default(),
                config: Vec::new(),
                capabilities: Capabilities::default(),
                rate_limit: None,
                _extra: Default::default(),
            },
            dir: std::path::PathBuf::from("/tmp"),
            entrypoint: std::path::PathBuf::from("/tmp/plugin.wasm"),
            mode: ExecutionMode::Wasm,
            enabled: true,
        }
    }

    #[test]
    fn resolve_id_returns_uuid_on_direct_match() {
        let mut reg = PluginRegistry::default();
        let loaded = mini_loaded_plugin("uuid-a", "tmdb");
        reg.insert(loaded.clone(), SandboxCtx::new(&loaded, "/tmp".into(), "/tmp".into()));
        assert_eq!(reg.resolve_id("uuid-a"), Some("uuid-a"));
    }

    #[test]
    fn resolve_id_falls_back_to_manifest_name() {
        let mut reg = PluginRegistry::default();
        let loaded = mini_loaded_plugin("uuid-a", "tmdb");
        reg.insert(loaded.clone(), SandboxCtx::new(&loaded, "/tmp".into(), "/tmp".into()));
        // "tmdb" is not a UUID key → fall back to name lookup → returns UUID.
        assert_eq!(reg.resolve_id("tmdb"), Some("uuid-a"));
    }

    #[test]
    fn resolve_id_none_when_neither_matches() {
        let reg = PluginRegistry::default();
        assert_eq!(reg.resolve_id("nope"), None);
    }

    #[test]
    fn resolve_id_uuid_wins_over_name_collision() {
        // Edge case: two plugins share a name. UUID match is preferred.
        let mut reg = PluginRegistry::default();
        let a = mini_loaded_plugin("uuid-a", "tmdb");
        let b = mini_loaded_plugin("uuid-b", "tmdb");
        reg.insert(a.clone(), SandboxCtx::new(&a, "/tmp".into(), "/tmp".into()));
        reg.insert(b.clone(), SandboxCtx::new(&b, "/tmp".into(), "/tmp".into()));
        assert_eq!(reg.resolve_id("uuid-a"), Some("uuid-a"));
        assert_eq!(reg.resolve_id("uuid-b"), Some("uuid-b"));
    }

    // ── Engine::plugins_for_scope / scope_has_any_plugins ─────────────────────

    #[tokio::test]
    async fn plugins_for_scope_returns_empty_on_fresh_engine() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let ids = engine.plugins_for_scope(SearchScope::Artist).await;
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn scope_has_any_plugins_false_on_fresh_engine() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        assert!(!engine.scope_has_any_plugins(SearchScope::Movie).await);
    }

    // ── Engine::plugin_semaphore ──────────────────────────────────────────────

    #[test]
    fn plugin_semaphore_clones_share_same_arc() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let clone = engine.clone();
        // Both point to the same semaphore (Arc identity).
        assert!(Arc::ptr_eq(engine.plugin_semaphore(), clone.plugin_semaphore()));
    }

    #[test]
    fn plugin_semaphore_starts_with_correct_capacity() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        assert_eq!(
            engine.plugin_semaphore().available_permits(),
            MAX_CONCURRENT_PLUGIN_CALLS,
        );
    }

    // ── supervisor_search: unknown plugin id ──────────────────────────────────

    #[tokio::test]
    async fn supervisor_search_unknown_id_returns_not_found() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let result = engine.supervisor_search("nonexistent-id", "test", SearchScope::Track).await;
        assert!(matches!(result, Err(PluginCallError::PluginNotFound(_))));
        if let Err(PluginCallError::PluginNotFound(id)) = result {
            assert_eq!(id, "nonexistent-id");
        }
    }

    // ── supervisor verb helpers: unknown plugin id → PluginNotFound ───────────

    #[tokio::test]
    async fn supervisor_lookup_unknown_id_returns_not_found() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let req = crate::abi::types::LookupRequest {
            id:        "tt1234".into(),
            id_source: "imdb".into(),
            kind:      stui_plugin_sdk::EntryKind::Track,
            locale:    None,
        };
        let result = engine.supervisor_lookup("no-such-plugin", req).await;
        assert!(matches!(result, Err(PluginCallError::PluginNotFound(_))));
        if let Err(PluginCallError::PluginNotFound(id)) = result {
            assert_eq!(id, "no-such-plugin");
        }
    }

    #[tokio::test]
    async fn supervisor_enrich_unknown_id_returns_not_found() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let req = crate::abi::types::EnrichRequest {
            partial:          crate::abi::types::PluginEntry::default(),
            prefer_id_source: None,
        };
        let result = engine.supervisor_enrich("no-such-plugin", req).await;
        assert!(matches!(result, Err(PluginCallError::PluginNotFound(_))));
    }

    #[tokio::test]
    async fn supervisor_get_artwork_unknown_id_returns_not_found() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let req = crate::abi::types::ArtworkRequest {
            id:        "e1".into(),
            id_source: "tmdb".into(),
            kind:      stui_plugin_sdk::EntryKind::Album,
            size:      crate::abi::types::ArtworkSize::Any,
        };
        let result = engine.supervisor_get_artwork("no-such-plugin", req).await;
        assert!(matches!(result, Err(PluginCallError::PluginNotFound(_))));
    }

    #[tokio::test]
    async fn supervisor_get_credits_unknown_id_returns_not_found() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let req = crate::abi::types::CreditsRequest {
            id:        "e1".into(),
            id_source: "tmdb".into(),
            kind:      stui_plugin_sdk::EntryKind::Movie,
        };
        let result = engine.supervisor_get_credits("no-such-plugin", req).await;
        assert!(matches!(result, Err(PluginCallError::PluginNotFound(_))));
    }

    #[tokio::test]
    async fn supervisor_related_unknown_id_returns_not_found() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let req = crate::abi::types::RelatedRequest {
            id:        "e1".into(),
            id_source: "tmdb".into(),
            kind:      stui_plugin_sdk::EntryKind::Track,
            relation:  crate::abi::types::RelationKind::Any,
            limit:     10,
        };
        let result = engine.supervisor_related("no-such-plugin", req).await;
        assert!(matches!(result, Err(PluginCallError::PluginNotFound(_))));
    }

    // ── Type-check: all 5 helpers have the correct fn signatures ─────────────
    //
    // This function is never called — it only ensures the helpers compile
    // with the correct return-type signatures at `cargo test --lib engine`.

    #[allow(dead_code)]
    fn _type_check_verb_signatures(engine: &Engine) {
        use std::future::Future;
        use crate::abi::types::{
            LookupRequest, EnrichRequest, ArtworkRequest, ArtworkSize,
            CreditsRequest, RelatedRequest, RelationKind,
            PluginEntry, ArtworkResponse, CreditsResponse,
        };

        fn _lookup(e: &Engine) -> impl Future<Output = Result<PluginEntry, PluginCallError>> + '_ {
            e.supervisor_lookup("p", LookupRequest {
                id: "".into(), id_source: "".into(),
                kind: stui_plugin_sdk::EntryKind::Track, locale: None,
            })
        }
        fn _enrich(e: &Engine) -> impl Future<Output = Result<PluginEntry, PluginCallError>> + '_ {
            e.supervisor_enrich("p", EnrichRequest {
                partial: PluginEntry::default(),
                prefer_id_source: None,
            })
        }
        fn _artwork(e: &Engine) -> impl Future<Output = Result<ArtworkResponse, PluginCallError>> + '_ {
            e.supervisor_get_artwork("p", ArtworkRequest {
                id: "".into(), id_source: "".into(),
                kind: stui_plugin_sdk::EntryKind::Track,
                size: ArtworkSize::Any,
            })
        }
        fn _credits(e: &Engine) -> impl Future<Output = Result<CreditsResponse, PluginCallError>> + '_ {
            e.supervisor_get_credits("p", CreditsRequest {
                id: "".into(), id_source: "".into(),
                kind: stui_plugin_sdk::EntryKind::Track,
            })
        }
        fn _related(e: &Engine) -> impl Future<Output = Result<Vec<PluginEntry>, PluginCallError>> + '_ {
            e.supervisor_related("p", RelatedRequest {
                id: "".into(), id_source: "".into(),
                kind: stui_plugin_sdk::EntryKind::Track,
                relation: RelationKind::Any,
                limit: 10,
            })
        }
    }
}

