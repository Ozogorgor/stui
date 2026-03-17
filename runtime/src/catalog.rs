//! Catalog — cache-first trending content manager.
//!
//! On startup the catalog:
//!   1. Loads the on-disk cache for each tab → sends results to Go immediately
//!   2. Spawns background refresh tasks for every registered provider
//!   3. Streams new results back to Go as they arrive
//!   4. Writes the merged, deduped results back to disk cache
//!
//! Cache lives at: ~/.stui/cache/grid/{tab}.json
//! TTL: 30 minutes (configurable via STUI_CACHE_TTL_SECS)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

use crate::ipc::{MediaTab, MediaType};
use crate::providers::Provider;

// ── CatalogEntry ─────────────────────────────────────────────────────────────

/// A single item in the content grid — richer than MediaEntry with poster data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id:          String,
    pub title:       String,
    pub year:        Option<String>,
    pub genre:       Option<String>,
    /// Weighted composite rating string (e.g. "8.3") — computed by the aggregator.
    pub rating:      Option<String>,
    pub description: Option<String>,
    /// Remote URL for the poster image (fetched separately for rendering)
    pub poster_url:  Option<String>,
    /// ANSI/block-art cached render of the poster (stored after first fetch)
    pub poster_art:  Option<String>,
    pub provider:    String,
    pub tab:         String,
    pub imdb_id:     Option<String>,
    pub tmdb_id:     Option<u64>,
    /// Fine-grained media type (movie, series, episode, music, …)
    #[serde(default)]
    pub media_type:  MediaType,
    /// Per-source raw scores, e.g. "tomatometer"→87.0, "imdb"→8.1.
    /// Values are in their native scale (RT: 0–100, IMDB/TMDB: 0–10, AniList: 0–100).
    #[serde(default)]
    pub ratings:     HashMap<String, f64>,
}

impl CatalogEntry {
    pub fn dedup_key(&self) -> String {
        // Prefer IMDB id for dedup, fall back to normalised title+year
        if let Some(ref id) = self.imdb_id {
            return id.clone();
        }
        format!(
            "{}:{}",
            self.title.to_lowercase().trim().replace(' ', "-"),
            self.year.as_deref().unwrap_or("?")
        )
    }
}

// ── Disk cache ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    fetched_at: u64, // unix timestamp
    entries: Vec<CatalogEntry>,
}

fn cache_path(cache_dir: &Path, tab: &MediaTab) -> PathBuf {
    let name = format!("{}.json", tab_key(tab));
    cache_dir.join("grid").join(name)
}

fn tab_key(tab: &MediaTab) -> &'static str {
    match tab {
        MediaTab::Movies   => "movies",
        MediaTab::Series   => "series",
        MediaTab::Music    => "music",
        MediaTab::Library  => "library",
        MediaTab::Radio    => "radio",
        MediaTab::Podcasts => "podcasts",
        MediaTab::Videos   => "videos",
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cache_ttl() -> u64 {
    std::env::var("STUI_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800) // 30 minutes
}

fn read_cache(path: &Path) -> Option<CacheFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(path: &Path, entries: &[CatalogEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = CacheFile {
        fetched_at: now_secs(),
        entries: entries.to_vec(),
    };
    let raw = serde_json::to_string_pretty(&file)?;
    std::fs::write(path, raw)?;
    Ok(())
}

// ── Grid update event ────────────────────────────────────────────────────────

/// Sent over the broadcast channel whenever new catalog data arrives.
/// The IPC layer subscribes to this and forwards updates to Go.
#[derive(Debug, Clone, Serialize)]
pub struct GridUpdate {
    pub tab: String,
    pub entries: Vec<CatalogEntry>,
    pub source: GridUpdateSource,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GridUpdateSource {
    Cache,      // instant — served from disk
    Live,       // fresh — arrived from a provider
}

// ── Catalog ───────────────────────────────────────────────────────────────────

pub struct Catalog {
    cache_dir: PathBuf,
    providers: Vec<Arc<dyn Provider>>,
    /// In-memory grid state per tab
    grids: Arc<RwLock<HashMap<String, Vec<CatalogEntry>>>>,
    /// Broadcast channel — subscribers get GridUpdate on every change
    tx: broadcast::Sender<GridUpdate>,
}

impl Catalog {
    pub fn new(cache_dir: PathBuf, providers: Vec<Arc<dyn Provider>>) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            cache_dir,
            providers,
            grids: Arc::new(RwLock::new(HashMap::new())),
            tx,
        }
    }

    /// Returns the built-in provider list — used by the stream resolution pipeline.
    pub fn providers(&self) -> &[Arc<dyn Provider>] {
        &self.providers
    }

    /// Subscribe to grid updates.
    pub fn subscribe(&self) -> broadcast::Receiver<GridUpdate> {
        self.tx.subscribe()
    }

    /// Start the catalog: serve cache immediately, then refresh in background.
    /// Call this once after the IPC loop is ready to receive messages.
    pub async fn start(self: Arc<Self>) {
        let tabs = [MediaTab::Movies, MediaTab::Series, MediaTab::Music];
        for tab in &tabs {
            let catalog = Arc::clone(&self);
            let tab = tab.clone();
            tokio::spawn(async move {
                catalog.init_tab(tab).await;
            });
        }
    }

    async fn init_tab(self: Arc<Self>, tab: MediaTab) {
        let path = cache_path(&self.cache_dir, &tab);

        // ── 1. Serve from cache immediately ──────────────────────────────
        if let Some(cached) = read_cache(&path) {
            let age = now_secs().saturating_sub(cached.fetched_at);
            let ttl = cache_ttl();

            info!(
                tab = tab_key(&tab),
                entries = cached.entries.len(),
                age_secs = age,
                "serving cached grid"
            );

            self.emit_update(&tab, cached.entries.clone(), GridUpdateSource::Cache).await;

            // Cache is fresh — skip refresh
            if age < ttl {
                debug!(tab = tab_key(&tab), "cache is fresh, skipping refresh");
                return;
            }
        }

        // ── 2. Refresh from all providers in parallel ─────────────────────
        self.refresh_tab(tab).await;
    }

    /// Refresh a tab by fetching from all providers concurrently,
    /// merging + deduplicating results, updating in-memory grid and disk cache.
    pub async fn refresh_tab(self: Arc<Self>, tab: MediaTab) {
        info!(tab = tab_key(&tab), providers = self.providers.len(), "refreshing grid");

        let mut handles = vec![];
        for provider in &self.providers {
            let p = Arc::clone(provider);
            let t = tab.clone();
            handles.push(tokio::spawn(async move {
                match p.fetch_trending(&t, 1).await {
                    Ok(entries) => {
                        info!(
                            provider = p.name(),
                            tab = tab_key(&t),
                            count = entries.len(),
                            "provider returned entries"
                        );
                        entries
                    }
                    Err(e) => {
                        warn!(provider = p.name(), tab = tab_key(&t), error = %e, "provider failed");
                        vec![]
                    }
                }
            }));
        }

        let mut all: Vec<CatalogEntry> = vec![];
        for handle in handles {
            if let Ok(mut entries) = handle.await {
                all.append(&mut entries);
            }
        }

        // Deduplicate by IMDB id / title+year key
        let merged = dedup(all);

        if merged.is_empty() {
            warn!(tab = tab_key(&tab), "all providers returned empty results");
            return;
        }

        // Update in-memory grid
        {
            let mut grids = self.grids.write().await;
            grids.insert(tab_key(&tab).to_string(), merged.clone());
        }

        // Persist to disk
        let path = cache_path(&self.cache_dir, &tab);
        if let Err(e) = write_cache(&path, &merged) {
            warn!(tab = tab_key(&tab), error = %e, "failed to write cache");
        }

        // Broadcast live update
        self.emit_update(&tab, merged, GridUpdateSource::Live).await;
    }

    async fn emit_update(
        &self,
        tab: &MediaTab,
        entries: Vec<CatalogEntry>,
        source: GridUpdateSource,
    ) {
        let update = GridUpdate {
            tab: tab_key(tab).to_string(),
            entries,
            source,
        };
        // Ignore send errors — no subscribers yet is fine
        let _ = self.tx.send(update);
    }

    /// Return the current in-memory grid for a tab (for on-demand reads).
    pub async fn get_grid(&self, tab: &MediaTab) -> Vec<CatalogEntry> {
        let grids = self.grids.read().await;
        grids.get(tab_key(tab)).cloned().unwrap_or_default()
    }
}

// ── Dedup ─────────────────────────────────────────────────────────────────────

fn dedup(entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter(|e| seen.insert(e.dedup_key()))
        .collect()
}
