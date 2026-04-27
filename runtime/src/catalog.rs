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
use serde::de::Deserializer;
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
    match serde_json::from_str::<CacheFile>(&raw) {
        Ok(cache) => Some(cache),
        Err(e) => {
            warn!("catalog: failed to deserialize cache {:?}: {}", path, e);
            None
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridUpdate {
    pub tab: String,
    pub entries: Vec<CatalogEntry>,
    pub source: GridUpdateSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GridUpdateSource {
    Cache,
    Live,
}

fn tmdb_id_from_num_or_str<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TmdbId {
        Num(u64),
        Str(String),
    }

    let tmdb: Option<TmdbId> = Deserialize::deserialize(deserializer)?;
    match tmdb {
        Some(TmdbId::Num(n)) => Ok(Some(n.to_string())),
        Some(TmdbId::Str(s)) if !s.is_empty() => Ok(Some(s)),
        Some(TmdbId::Str(_)) => Ok(None),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Artist / creator name. Populated for music tab entries (album artist,
    /// track artist) from `PluginEntry.artist_name`. None for movies/series.
    #[serde(default)]
    pub artist: Option<String>,
    pub imdb_id: Option<String>,
    #[serde(default, deserialize_with = "tmdb_id_from_num_or_str")]
    pub tmdb_id: Option<String>,
    /// MyAnimeList id, populated from `PluginEntry.external_ids["myanimelist"]`
    /// at the MediaEntry→CatalogEntry conversion in `engine::search`. Absent
    /// for catalog/tvdb-derived entries that have no MAL mapping.
    #[serde(default)]
    pub mal_id: Option<String>,
    #[serde(default)]
    pub media_type: MediaType,
    #[serde(default)]
    pub ratings: HashMap<String, f64>,
    /// ISO 639-1 code of the entry's original language (e.g. "ja" for anime
    /// shipped from TMDB). Used by the runtime's anime-mix classifier.
    #[serde(default)]
    pub original_language: Option<String>,
}

impl CatalogEntry {
    /// Group key for collapsing the same entity across providers.
    ///
    /// **Precedence:**
    /// 1. `mal_id` — anime tier (AniList exposes `idMal`; Kitsu exposes via
    ///    `?include=mappings`). Catches AniList↔Kitsu duplicates of the same
    ///    cour even when titles differ (English vs romaji).
    /// 2. `imdb_id` — western tier (TVDB and OMDb both surface it at search
    ///    time; TMDB doesn't, so TMDB falls through to fallback).
    /// 3. `normalize_title:year` — fallback for entries without a foreign id.
    ///
    /// Empty-string foreign ids are treated as missing — defensive, prevents
    /// `"mal:"` from collapsing all empty entries into one bucket. Keys are
    /// prefixed with their tier (`mal:`, `imdb:`, `title:`) so a numeric
    /// fallback title can't accidentally collide with a foreign id.
    ///
    /// Cross-tier merges (anime↔western) don't happen here — different keys
    /// stay separate. The cross-mapping bridge is milestone β.
    pub fn dedup_key(&self) -> String {
        if let Some(mal) = self.mal_id.as_deref().filter(|s| !s.is_empty()) {
            return format!("mal:{mal}");
        }
        if let Some(imdb) = self.imdb_id.as_deref().filter(|s| !s.is_empty()) {
            return format!("imdb:{imdb}");
        }
        format!(
            "title:{}:{}",
            normalize_title(&self.title),
            self.year.as_deref().unwrap_or("?"),
        )
    }
}

/// Normalize a title for fuzzy-equality keying. Collapses typographic
/// differences that cause the same show to dedup-miss across providers:
///
///   - lowercase
///   - curly quotes → ASCII (`’` → `'`, `“`/`”` → `"`)
///   - strip all punctuation (Unicode-aware) so "Journey's" == "Journeys"
///   - collapse runs of whitespace into a single `-`
///
/// Kept title-suffix-sensitive on purpose: "Show Name" and "Show Name Season 2"
/// MUST remain distinct keys. Only surface-level typography is folded.
pub fn normalize_title(title: &str) -> String {
    // 1. Lowercase + unify curly quotes.
    let mut s: String = title
        .chars()
        .map(|c| match c {
            '’' | '‘' | '\u{02BC}' => '\'',
            '“' | '”' => '"',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase();
    // 2. Drop all punctuation — keeps letters/digits/whitespace only.
    s.retain(|c| c.is_alphanumeric() || c.is_whitespace());
    // 3. Collapse whitespace runs to a single `-`.
    s.split_whitespace().collect::<Vec<_>>().join("-")
}

pub struct Catalog {
    cache_dir: PathBuf,
    engine: Arc<Engine>,
    #[allow(clippy::type_complexity)]
    grids: Arc<RwLock<HashMap<String, Vec<CatalogEntry>>>>,
    tx: broadcast::Sender<GridUpdate>,
    /// Broadcast channel for "refresh attempted but got zero entries" — the
    /// signal the TUI uses to flag "offline / cached only" in the status bar.
    stale_tx: broadcast::Sender<CatalogStale>,
    /// Set by main.rs once `discovery.scan_and_load().await?` finishes.
    /// `init_tab` emits the cached grid synchronously (fast path) but then
    /// awaits this signal before calling `refresh_tab`, so the very first
    /// refresh doesn't race plugin loading and return empty results.
    plugins_ready: Arc<tokio::sync::Notify>,
    /// Plain flag mirroring the Notify so late-arriving tasks don't hang
    /// on Notified if the signal already fired. Checked once; if true the
    /// task skips the await; if false it awaits the Notify.
    plugins_ready_flag: Arc<std::sync::atomic::AtomicBool>,
}

/// Pushed on `Catalog::stale_tx` when refresh_tab returns zero entries.
/// Mirrors the IPC `CatalogStaleEvent` shape so the forwarder in main.rs
/// is a trivial field-copy. The split (internal struct vs IPC struct) keeps
/// the catalog module free of IPC schema dependencies.
#[derive(Debug, Clone)]
pub struct CatalogStale {
    pub tab: String,
    pub reason: String,
}

impl Catalog {
    pub fn new(cache_dir: PathBuf, engine: Arc<Engine>) -> Self {
        let (tx, _) = broadcast::channel(64);
        let (stale_tx, _) = broadcast::channel(16);
        Self {
            cache_dir,
            engine,
            grids: Arc::new(RwLock::new(HashMap::new())),
            tx,
            stale_tx,
            plugins_ready: Arc::new(tokio::sync::Notify::new()),
            plugins_ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Subscribe to stale-refresh notifications. Used by main.rs to forward
    /// over IPC as `catalog_stale` messages.
    pub fn subscribe_stale(&self) -> broadcast::Receiver<CatalogStale> {
        self.stale_tx.subscribe()
    }

    /// Main signals this once the plugin registry is fully populated.
    /// Subsequent calls are no-ops.
    pub fn mark_plugins_ready(&self) {
        self.plugins_ready_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.plugins_ready.notify_waiters();
    }

    /// Wait until `mark_plugins_ready` fires. Uses the Notify-before-flag
    /// idiom: register the waiter (`enable()`) BEFORE loading the flag, so a
    /// concurrent `store(true) + notify_waiters()` either (a) sees us as a
    /// registered waiter and wakes us, or (b) wrote the flag before we
    /// registered — in which case our post-registration flag load observes
    /// `true` and we return immediately. Without the explicit `enable`,
    /// Notified only registers on first poll and the notifier could fire
    /// between our flag-check and our await → permanent hang.
    async fn await_plugins_ready(&self) {
        let notified = self.plugins_ready.notified();
        tokio::pin!(notified);
        // Force registration now so the notify side can't fire "between"
        // our flag check and our await point.
        notified.as_mut().enable();
        if self
            .plugins_ready_flag
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        notified.await;
    }

    #[allow(dead_code)] // pub API: used by TUI catalog grid
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

            // Store in the grids snapshot map so clients connecting AFTER
            // this emission (typical — the TUI takes ~200ms to set up its
            // IPC reader) can be replayed the cached state on subscribe.
            // Without this, late-joining clients miss the startup broadcast
            // entirely and show whatever mediacache.json had on disk.
            {
                let mut grids = self.grids.write().await;
                grids.insert(tab_key(&tab).to_string(), cached.entries.clone());
            }

            self.emit_update(&tab, cached.entries.clone(), GridUpdateSource::Cache).await;

            // Stale-while-revalidate threshold: if the cache is within the
            // first half of its TTL, treat it as genuinely fresh and don't
            // refresh. Between soft_ttl and hard ttl, we still serve the
            // cached result (already emitted above), but fall through to
            // kick an async background refresh so the next frame shows
            // fresher data. After hard ttl, same fall-through applies.
            let soft_ttl = ttl / 2;
            if age < soft_ttl {
                debug!(tab = tab_key(&tab), age_secs = age, "cache is fresh, skipping refresh");
                return;
            }
            if age < ttl {
                debug!(tab = tab_key(&tab), age_secs = age, "cache is soft-stale — kicking background refresh");
            }
        }

        // Wait for plugins to finish loading before attempting a refresh.
        // Without this, the very first refresh on daemon boot races the
        // plugin scan, hits an empty registry, returns no results, and the
        // tab stays empty (or shows whatever stale cache we had) until the
        // next trigger. init_tab emission above already ran synchronously
        // from disk, so the user sees cached data immediately; this await
        // is just gating the live-refresh step.
        self.await_plugins_ready().await;
        self.refresh_tab(tab).await;
    }

    /// Snapshot of the latest entries known for every tab. Used by the IPC
    /// loop to replay cached state to a newly-connected client so it doesn't
    /// miss the broadcast that fired during daemon startup. Marked Cache so
    /// downstream can distinguish replayed-from-memory from live refresh.
    pub async fn snapshot_all(&self) -> Vec<GridUpdate> {
        let grids = self.grids.read().await;
        grids
            .iter()
            .map(|(tab, entries)| GridUpdate {
                tab: tab.clone(),
                entries: entries.clone(),
                source: GridUpdateSource::Cache,
            })
            .collect()
    }

    pub async fn refresh_tab(self: Arc<Self>, tab: MediaTab) {
        let tab_str = tab_key(&tab);
        info!(tab = tab_str, "refreshing grid via engine search");

        // Fan out an empty query (= trending) across all Catalog-capable plugins.
        // search_catalog_entries already deduplicates and merges provider results.
        let merged = self.engine.search_catalog_entries(
            "",    // empty query = trending
            &tab,
            crate::engine::SearchOptions::default(),
        ).await;

        if merged.is_empty() {
            warn!(tab = tab_str, "engine search returned empty results");
            // Fire a catalog_stale event so the TUI can flag "offline /
            // cached only" in the status bar. Broadcast is fire-and-forget;
            // if no subscribers are attached the message is dropped, which
            // is fine — the log entry above still records the condition.
            let _ = self.stale_tx.send(CatalogStale {
                tab: tab_str.to_string(),
                reason: "no entries from providers (offline or empty trending)".to_string(),
            });
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

        self.emit_update(&tab, merged.clone(), GridUpdateSource::Live).await;

        // ── Progressive enrichment pass ─────────────────────────────
        // Music: lastfm albums arrive without year/rating; fan out to
        //   MB+Discogs+lastfm for year, rating, genre, description.
        // Movies/Series: TMDB/TVDB entries carry a single-source
        //   rating; fan out to OMDb to fill in IMDb + Rotten Tomatoes
        //   + Metacritic into the per-source ratings map, then
        //   recompute the weighted composite via the aggregator.
        // Both paths stream snapshots back every PROGRESS_BATCH_SIZE
        // entries so cards repaint in waves rather than after the
        // whole pass completes; HTTP responses are cached in sqlite,
        // so subsequent boots are effectively free.
        enum EnrichKind { Music, Video }
        let enrich_kind = match tab {
            MediaTab::Music => Some(EnrichKind::Music),
            MediaTab::Movies | MediaTab::Series => Some(EnrichKind::Video),
            _ => None,
        };
        if let Some(kind) = enrich_kind {
            let this = self.clone();
            let tab_clone = tab.clone();
            tokio::spawn(async move {
                let cb_this = this.clone();
                let cb_tab = tab_clone.clone();
                let on_progress = move |snapshot: Vec<CatalogEntry>| {
                    let this = cb_this.clone();
                    let tab = cb_tab.clone();
                    async move {
                        let tab_str = tab_key(&tab).to_string();
                        {
                            let mut grids = this.grids.write().await;
                            grids.insert(tab_str.clone(), snapshot.clone());
                        }
                        let path = cache_path(&this.cache_dir, &tab);
                        if let Err(e) = write_cache(&path, &snapshot) {
                            warn!(tab = %tab_str, error = %e, "failed to write enriched cache");
                        }
                        info!(
                            tab = %tab_str,
                            count = snapshot.len(),
                            "enrich: progressive snapshot",
                        );
                        this.emit_update(&tab, snapshot, GridUpdateSource::Live).await;
                    }
                };
                match kind {
                    EnrichKind::Music => {
                        crate::engine::music_enrich::enrich_grid_progressive(
                            this.engine.clone(),
                            merged,
                            on_progress,
                        ).await;
                    }
                    EnrichKind::Video => {
                        crate::engine::video_enrich::enrich_grid_progressive(
                            this.engine.clone(),
                            merged,
                            on_progress,
                        ).await;
                    }
                }
            });
        }
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
}

#[cfg(test)] // only used in tests
fn dedup_and_merge(entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
    use crate::catalog_engine::CatalogAggregator;
    let aggregator = CatalogAggregator::new();
    aggregator.merge(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_title_folds_curly_apostrophes() {
        // TMDB emits the ASCII form; AniList emits the Unicode right
        // single-quotation-mark form. Both must key to the same string so
        // the aggregator deduplicates "Frieren: Beyond Journey's End"
        // across providers.
        let tmdb = "Frieren: Beyond Journey's End";
        let anilist = "Frieren: Beyond Journey\u{2019}s End";
        assert_eq!(normalize_title(tmdb), normalize_title(anilist));
    }

    #[test]
    fn normalize_title_distinguishes_season_suffix() {
        // Different seasons of the same show MUST produce distinct keys.
        let s1 = "Frieren: Beyond Journey's End";
        let s2 = "Frieren: Beyond Journey's End Season 2";
        assert_ne!(normalize_title(s1), normalize_title(s2));
    }

    #[test]
    fn normalize_title_strips_punctuation_and_case() {
        assert_eq!(
            normalize_title("ATTACK ON TITAN: Final Season"),
            normalize_title("attack on titan final season")
        );
    }

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
                artist: None,
                imdb_id: Some("tt0001".to_string()),
                tmdb_id: None,
                mal_id: None,
                media_type: MediaType::default(),
                ratings: HashMap::new(),
                original_language: None,
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
                artist: None,
                imdb_id: Some("tt0001".to_string()),
                tmdb_id: None,
                mal_id: None,
                media_type: MediaType::default(),
                ratings: HashMap::new(),
                original_language: None,
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
                artist: None,
            imdb_id: Some("tt0001".to_string()),
            tmdb_id: None,
            mal_id: None,
            media_type: MediaType::default(),
            ratings: HashMap::new(),
            original_language: None,
        };

        assert_eq!(entry.dedup_key(), "imdb:tt0001");
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
                artist: None,
            imdb_id: None,
            tmdb_id: None,
            mal_id: None,
            media_type: MediaType::default(),
            ratings: HashMap::new(),
            original_language: None,
        };

        assert_eq!(entry.dedup_key(), "title:movie:2024");
    }
}
