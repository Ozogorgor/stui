//! Catalog — cache-first trending content manager.
//!
//! On startup the catalog:
//!   1. Loads the on-disk cache for each tab → sends results to Go immediately
//!   2. Uses the Engine's search interface to fetch from WASM plugin providers
//!   3. Streams new results back to Go as they arrive
//!   4. Writes the merged, deduped results back to disk cache
//!
//! Cache lives at: ~/.stui/cache/grid/{tab}.json
//! TTL: 30 minutes (configurable via STUI_CACHE_TTL_SECS)
//!
//! ## Decoupled Architecture
//!
//! This catalog uses the Engine's search() method to fetch from WASM plugin providers,
//! instead of directly depending on built-in provider implementations. This allows
//! providers to be updated independently without recompiling the runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

use crate::engine::Engine;
use crate::ipc::{MediaTab, MediaType};

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cache_ttl() -> u64 {
    std::env::var("STUI_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800)
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

fn cache_path(cache_dir: &Path, tab: &MediaTab) -> PathBuf {
    let name = format!("{}.json", tab_key(tab));
    cache_dir.join("grid").join(name)
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

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    fetched_at: u64,
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GridUpdate {
    pub tab: String,
    pub entries: Vec<CatalogEntry>,
    pub source: GridUpdateSource,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GridUpdateSource {
    Cache,
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub title: String,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub rating: Option<String>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub poster_art: Option<String>,
    pub provider: String,
    pub tab: String,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<u64>,
    #[serde(default)]
    pub media_type: MediaType,
    #[serde(default)]
    pub ratings: HashMap<String, f64>,
}

impl CatalogEntry {
    pub fn dedup_key(&self) -> String {
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

pub struct Catalog {
    cache_dir: PathBuf,
    engine: Arc<Engine>,
    #[allow(clippy::type_complexity)]
    grids: Arc<RwLock<HashMap<String, Vec<CatalogEntry>>>>,
    tx: broadcast::Sender<GridUpdate>,
}

impl Catalog {
    pub fn new(cache_dir: PathBuf, engine: Arc<Engine>) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            cache_dir,
            engine,
            grids: Arc::new(RwLock::new(HashMap::new())),
            tx,
        }
    }

    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<GridUpdate> {
        self.tx.subscribe()
    }

    pub async fn start(self: Arc<Self>) {
        let tabs = vec![MediaTab::Movies, MediaTab::Series, MediaTab::Music];
        
        let handles: Vec<_> = tabs
            .into_iter()
            .map(|tab| {
                let catalog = Arc::clone(&self);
                tokio::spawn(async move {
                    catalog.init_tab(tab).await;
                })
            })
            .collect();
        
        for handle in handles {
            if let Err(e) = handle.await {
                warn!(error = %e, "catalog tab task panicked");
            }
        }
    }

    async fn init_tab(self: Arc<Self>, tab: MediaTab) {
        let path = cache_path(&self.cache_dir, &tab);

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

            if age < ttl {
                debug!(tab = tab_key(&tab), "cache is fresh, skipping refresh");
                return;
            }
        }

        self.refresh_tab(tab).await;
    }

    pub async fn refresh_tab(self: Arc<Self>, tab: MediaTab) {
        let tab_str = tab_key(&tab);
        info!(tab = tab_str, "refreshing grid via engine search");

        // Use engine's search with empty query for trending
        // The engine fans out to all WASM plugins with Catalog capability
        let response = self.engine.search(
            "",              // empty query = trending
            "",              // req_id
            &tab,
            None,            // provider_filter (all providers)
            50,              // limit
            0,               // offset
        ).await;

        let entries = match response {
            crate::ipc::Response::SearchResult(sr) => {
                sr.items.into_iter().map(|e| CatalogEntry {
                    id: e.id,
                    title: e.title,
                    year: e.year,
                    genre: e.genre,
                    rating: e.rating,
                    description: e.description,
                    poster_url: e.poster_url,
                    poster_art: None,
                    provider: e.provider,
                    tab: tab_str.to_string(),
                    imdb_id: None,
                    tmdb_id: None,
                    media_type: e.media_type,
                    ratings: std::collections::HashMap::new(),
                }).collect::<Vec<_>>()
            }
            _ => {
                warn!(tab = tab_str, "unexpected response from engine search");
                vec![]
            }
        };

        let merged = dedup_and_merge(entries);

        if merged.is_empty() {
            warn!(tab = tab_str, "engine search returned empty results");
            return;
        }

        info!(tab = tab_str, count = merged.len(), "grid refresh complete");

        {
            let mut grids = self.grids.write().await;
            grids.insert(tab_str.to_string(), merged.clone());
        }

        let path = cache_path(&self.cache_dir, &tab);
        if let Err(e) = write_cache(&path, &merged) {
            warn!(tab = tab_str, error = %e, "failed to write cache");
        }

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
        let _ = self.tx.send(update);
    }

    pub async fn get_grid(&self, tab: &MediaTab) -> Vec<CatalogEntry> {
        let grids = self.grids.read().await;
        grids.get(tab_key(tab)).cloned().unwrap_or_default()
    }
}

fn dedup_and_merge(entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
    use crate::catalog_engine::CatalogAggregator;
    let aggregator = CatalogAggregator::new();
    aggregator.merge(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_preserves_unique_entries() {
        let entries = vec![
            CatalogEntry {
                id: "1".to_string(),
                title: "Movie".to_string(),
                year: Some("2024".to_string()),
                genre: None,
                rating: None,
                description: None,
                poster_url: None,
                poster_art: None,
                provider: "tmdb".to_string(),
                tab: "movies".to_string(),
                imdb_id: Some("tt0001".to_string()),
                tmdb_id: None,
                media_type: MediaType::default(),
                ratings: HashMap::new(),
            },
            CatalogEntry {
                id: "2".to_string(),
                title: "Movie".to_string(),
                year: Some("2024".to_string()),
                genre: None,
                rating: None,
                description: None,
                poster_url: None,
                poster_art: None,
                provider: "anilist".to_string(),
                tab: "movies".to_string(),
                imdb_id: Some("tt0001".to_string()),
                tmdb_id: None,
                media_type: MediaType::default(),
                ratings: HashMap::new(),
            },
        ];
        
        let result = dedup_and_merge(entries);
        assert_eq!(result.len(), 1, "dedup should keep only one entry");
    }

    #[test]
    fn test_dedup_key_imdb_priority() {
        let entry = CatalogEntry {
            id: "1".to_string(),
            title: "Movie".to_string(),
            year: Some("2024".to_string()),
            genre: None,
            rating: None,
            description: None,
            poster_url: None,
            poster_art: None,
            provider: "tmdb".to_string(),
            tab: "movies".to_string(),
            imdb_id: Some("tt0001".to_string()),
            tmdb_id: None,
            media_type: MediaType::default(),
            ratings: HashMap::new(),
        };
        
        assert_eq!(entry.dedup_key(), "tt0001");
    }

    #[test]
    fn test_dedup_key_fallback_title_year() {
        let entry = CatalogEntry {
            id: "1".to_string(),
            title: "Movie".to_string(),
            year: Some("2024".to_string()),
            genre: None,
            rating: None,
            description: None,
            poster_url: None,
            poster_art: None,
            provider: "tmdb".to_string(),
            tab: "movies".to_string(),
            imdb_id: None,
            tmdb_id: None,
            media_type: MediaType::default(),
            ratings: HashMap::new(),
        };
        
        assert_eq!(entry.dedup_key(), "movie:2024");
    }
}
