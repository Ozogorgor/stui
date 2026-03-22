//! `Pipeline` — the single orchestration entry point for the stui runtime.
//!
//! The `Pipeline` struct owns every runtime subsystem and exposes a clean,
//! linear API that mirrors the actual data flow:
//!
//! ```text
//! Pipeline::search()          → catalog entries (via engine + cache)
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
//! See the Pipeline implementation for construction details.

#![allow(dead_code)]

use std::sync::Arc;
use std::collections::HashMap;
use tracing::info;

use crate::cache::RuntimeCache;
use crate::catalog::Catalog;
use crate::config::RuntimeConfig;
use super::Engine;
use crate::ipc::MediaTab;
use crate::player::PlayerBridge;
use crate::events::{EventBus, RuntimeEvent};
use crate::config::ConfigManager;
use crate::providers::{HealthRegistry, ProviderThrottle, CircuitBreaker, StreamBenchmarker};
use crate::plugin_rpc::PluginRpcManager;
use crate::quality::{RankingPolicy, StreamCandidate};

/// The top-level orchestration struct.
///
/// Owns every runtime subsystem.  Passed by reference to the IPC loop.
pub struct Pipeline {
    pub engine:   Engine,
    pub catalog:  Arc<Catalog>,
    pub cache:    RuntimeCache,
    pub policy:   RankingPolicy,

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

    /// Circuit breaker — prevents cascading failures by disabling failing providers.
    pub circuit_breaker: CircuitBreaker,

    /// Live-updatable runtime configuration.
    pub config: ConfigManager,

    /// Stream benchmarker — measures HTTP throughput and latency.
    pub bench: StreamBenchmarker,
}

impl Pipeline {
    /// Construct the pipeline from config.
    ///
    /// The engine and catalog are created with Engine providing WASM plugin access.
    pub fn new(
        cfg: &RuntimeConfig,
        player: Arc<PlayerBridge>,
    ) -> Self {
        let engine  = Engine::new(cfg.cache_dir.clone(), cfg.data_dir.clone());
        let catalog = Arc::new(Catalog::new(cfg.cache_dir.clone(), Arc::new(engine.clone())));
        let cache   = RuntimeCache::new();
        let policy  = RankingPolicy::default();

        info!(
            "pipeline ready, cache_dir={}",
            cfg.cache_dir.display()
        );

        let bus      = Arc::new(EventBus::new());
        let health   = HealthRegistry::new();
        let throttle = ProviderThrottle::new();
        let circuit_breaker = CircuitBreaker::new();
        let config   = ConfigManager::new(cfg.clone(), bus.clone());
        let bench    = StreamBenchmarker::new();

        Pipeline { engine, catalog, cache, policy, player,
                   rpc: Arc::new(PluginRpcManager::new()),
                   bus, health, throttle, circuit_breaker, config, bench }
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
    /// 
    /// Uses health-blended ranking when provider reliability data is available.
    /// Uses stream benchmarking when `streaming.benchmark_streams` config is enabled.
    pub async fn resolve_streams(&self, entry_id: &str) -> Vec<StreamCandidate> {
        let cfg = self.config.snapshot().await;
        let benchmark_enabled = cfg.streaming.benchmark_streams;
        let health_map = self.health.all_reliability_scores();
        
        if health_map.is_empty() && !benchmark_enabled {
            self.engine.ranked_streams(entry_id, &self.policy, &[]).await
        } else if benchmark_enabled {
            self.resolve_streams_with_benchmark(entry_id, health_map).await
        } else {
            self.engine.ranked_streams_with_health(entry_id, &self.policy, &[], health_map).await
        }
    }

    /// Resolve streams with benchmarking enabled.
    /// Probes HTTP streams to measure throughput, then re-ranks by speed.
    async fn resolve_streams_with_benchmark(
        &self,
        entry_id: &str,
        health_map: HashMap<String, f64>,
    ) -> Vec<StreamCandidate> {
        use crate::quality::rank_with_health_and_speed;
        
        let candidates = if health_map.is_empty() {
            self.engine.ranked_streams(entry_id, &self.policy, &[]).await
        } else {
            self.engine.ranked_streams_with_health(entry_id, &self.policy, &[], health_map.clone()).await
        };

        if candidates.is_empty() {
            return candidates;
        }

        let streams: Vec<_> = candidates.iter().map(|c| c.stream.clone()).collect();
        let probed_streams = self.bench.probe_all(&streams).await;
        
        let mut speed_map: HashMap<String, f64> = HashMap::new();
        for stream in &probed_streams {
            if let Some(speed) = stream.speed_mbps {
                speed_map.insert(stream.url.clone(), speed);
            }
        }

        rank_with_health_and_speed(
            probed_streams,
            &self.policy,
            if health_map.is_empty() { None } else { Some(&health_map) },
            if speed_map.is_empty() { None } else { Some(&speed_map) },
        )
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
        media_type: Option<crate::ipc::MediaType>,
        year: Option<u32>,
    ) {
        self.player.play(entry_id, provider, imdb_id, None, media_type, year).await;
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
