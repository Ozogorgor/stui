//! `Pipeline` — the single orchestration entry point for the stui runtime.
//!
//! The `Pipeline` struct owns every runtime subsystem and exposes a clean,
//! linear API that mirrors the actual data flow:
//!
//! ```text
//! Pipeline::search()          → catalog entries (via providers + cache)
//!     ↓
//! Pipeline::get_catalog()     → trending grid (via catalog + cache)
//!     ↓
//! Pipeline::resolve_streams() → ranked stream candidates (via quality module)
//!     ↓
//! Pipeline::play()            → launches mpv via player bridge
//! ```
//!
//! `main.rs` constructs one `Pipeline`, passes it to `run_ipc_loop()`, and
//! never touches individual subsystems directly.  This keeps `main.rs` thin
//! and makes the runtime trivially testable.
//!
//! # Construction
//!
//! ```rust
//! let pipeline = Pipeline::new(config, built_in_providers).await;
//! ```

use std::sync::Arc;
use tracing::info;

use crate::cache::RuntimeCache;
use crate::catalog::Catalog;
use crate::config::RuntimeConfig;
use super::Engine;
use crate::ipc::MediaTab;
use crate::player::PlayerBridge;
use crate::providers::Provider;
use crate::events::{EventBus, RuntimeEvent};
use crate::config::ConfigManager;
use crate::providers::{HealthRegistry, ProviderThrottle};
use crate::plugin_rpc::PluginRpcManager;
use crate::quality::{rank, RankingPolicy, StreamCandidate};

/// The top-level orchestration struct.
///
/// Owns every runtime subsystem.  Passed by reference to the IPC loop.
pub struct Pipeline {
    pub engine:   Engine,
    pub catalog:  Arc<Catalog>,
    pub cache:    RuntimeCache,
    pub policy:   RankingPolicy,

    /// All built-in providers (metadata + stream).
    /// Stremio addon adapters are included here after construction.
    pub providers: Vec<Arc<dyn Provider>>,

    /// Player bridge — routes URLs to aria2 or mpv.
    pub player:   Arc<PlayerBridge>,

    /// Language-agnostic RPC plugin manager (Python, Go, JS, Rust, …).
    /// Runs alongside the WASM plugin system; results are merged before ranking.
    pub rpc: Arc<PluginRpcManager>,

    /// Central event bus — emit events here for any module to observe.
    pub bus: Arc<EventBus>,

    /// Provider health tracker — reliability scores for stream ranking.
    pub health: HealthRegistry,

    /// Per-provider rate-limit throttle — prevents 429 errors.
    pub throttle: ProviderThrottle,

    /// Live-updatable runtime configuration.
    pub config: ConfigManager,
}

impl Pipeline {
    /// Construct the pipeline from config and a pre-built provider list.
    ///
    /// Caller is responsible for building `providers` (including Stremio
    /// addon adapters) before calling this.
    pub fn new(
        cfg:       &RuntimeConfig,
        providers: Vec<Arc<dyn Provider>>,
        player:    Arc<PlayerBridge>,
    ) -> Self {
        let engine  = Engine::new(cfg.cache_dir.clone(), cfg.data_dir.clone());
        let catalog = Arc::new(Catalog::new(cfg.cache_dir.clone(), providers.clone()));
        let cache   = RuntimeCache::new();
        let policy  = RankingPolicy::default();

        info!(
            "pipeline ready: {} provider(s), cache_dir={}",
            providers.len(),
            cfg.cache_dir.display()
        );

        let bus      = Arc::new(EventBus::new());
        let health   = HealthRegistry::new();
        let throttle = ProviderThrottle::new();
        let config   = ConfigManager::new(cfg.clone(), bus.clone());

        Pipeline { engine, catalog, cache, policy, providers, player,
                   rpc: Arc::new(PluginRpcManager::new()),
                   bus, health, throttle, config }
    }

    // ── Stage 1: catalog / search ─────────────────────────────────────────

    /// Trigger a background trending-catalog refresh for `tab`.
    /// Results are broadcast via `Catalog`'s watch channel — non-blocking.
    pub async fn get_catalog(&self, tab: &MediaTab) {
        Arc::clone(&self.catalog).refresh_tab(tab.clone()).await;
    }

    /// Fan out a search query to all providers, cache and return results.
    pub async fn search(
        &self,
        tab:   &MediaTab,
        query: &str,
        page:  u32,
    ) -> Vec<crate::catalog::CatalogEntry> {
        self.bus.emit(RuntimeEvent::SearchRequested {
            query: query.to_string(),
            tab:   format!("{tab:?}"),
        });
        let offset = ((page.saturating_sub(1)) as usize) * 50;
        let response = self.engine.search("", query, tab, None, 50, offset).await;
        if let crate::ipc::Response::SearchResult(sr) = response {
            self.health.record_success("engine", 0);
            self.bus.emit(RuntimeEvent::SearchResultsReady {
                query:    query.to_string(),
                tab:      format!("{tab:?}"),
                provider: "all".to_string(),
                count:    sr.items.len(),
            });
            sr.items.into_iter().map(|e| crate::catalog::CatalogEntry {
                id: e.id, title: e.title, year: e.year, genre: e.genre,
                rating: e.rating, description: e.description,
                poster_url: e.poster_url, poster_art: None,
                provider: e.provider,
                tab: format!("{:?}", e.tab).to_lowercase(),
                imdb_id: None, tmdb_id: None,
                media_type: e.media_type,
                ratings: std::collections::HashMap::new(),
            }).collect()
        } else {
            self.health.record_failure("engine", crate::providers::health::FailureKind::Error);
            self.bus.emit(RuntimeEvent::ProviderError {
                provider: "engine".to_string(),
                message:  "search returned unexpected response".to_string(),
            });
            vec![]
        }
    }

    // ── Stage 2: stream resolution ────────────────────────────────────────

    /// Resolve and rank all available streams for `entry_id`.
    /// Returns candidates sorted best-first according to `self.policy`.
    pub async fn resolve_streams(&self, entry_id: &str) -> Vec<StreamCandidate> {
        self.bus.emit(RuntimeEvent::MediaSelected {
            entry_id: entry_id.to_string(),
            title:    entry_id.to_string(), // enriched by caller if available
        });
        let t0 = std::time::Instant::now();
        let candidates = self.engine
            .ranked_streams(entry_id, &self.policy, &self.providers)
            .await;
        let latency_ms = t0.elapsed().as_millis() as u64;

        if candidates.is_empty() {
            self.bus.emit(RuntimeEvent::AllCandidatesExhausted {
                entry_id: entry_id.to_string(),
            });
        } else {
            // Record health for the provider of the top candidate
            if let Some(best) = candidates.first() {
                let provider = &best.stream.provider;
                self.health.record_success(provider, latency_ms);
                self.bus.emit(RuntimeEvent::ProviderSuccess {
                    provider:   provider.clone(),
                    latency_ms,
                });
            }
        }

        self.bus.emit(RuntimeEvent::StreamsResolved {
            entry_id:   entry_id.to_string(),
            candidates: candidates.clone(),
        });
        if let Some(best) = candidates.first() {
            let protocol = if best.stream.url.starts_with("magnet:") {
                "magnet".to_string()
            } else {
                "http".to_string()
            };
            self.bus.emit(RuntimeEvent::StreamSelected {
                entry_id: entry_id.to_string(),
                url:      best.stream.url.clone(),
                protocol,
                quality:  Some(best.stream.quality.label().to_string()),
            });
        }
        candidates
    }

    /// Resolve streams and return only the single best candidate URL.
    pub async fn best_stream_url(&self, entry_id: &str) -> Option<String> {
        self.resolve_streams(entry_id)
            .await
            .into_iter()
            .next()
            .map(|c| c.stream.url)
    }

    // ── Stage 3: playback ─────────────────────────────────────────────────

    /// Full play pipeline: resolve best stream → launch via player bridge.
    pub async fn play(
        &self,
        entry_id: &str,
        provider: &str,
        imdb_id:  &str,
    ) {
        self.player.play(entry_id, provider, imdb_id, None).await;
    }

    // ── Policy control ────────────────────────────────────────────────────

    /// Switch to a bandwidth-saving ranking policy (prefers 720p, high seeds).
    pub fn use_bandwidth_saver(&mut self) {
        self.policy = RankingPolicy::bandwidth_saver();
        info!("pipeline: switched to bandwidth_saver ranking policy");
    }

    /// Switch to the default quality-first ranking policy.
    pub fn use_default_policy(&mut self) {
        self.policy = RankingPolicy::default();
        info!("pipeline: switched to default ranking policy");
    }
}
