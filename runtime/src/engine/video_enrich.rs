//! video_enrich — second-pass enrichment for movie / series catalog
//! entries via OMDb's multi-source rating block.
//!
//! Movies and series arrive in the catalog from TMDB / TVDB / kitsu /
//! anilist / etc., each contributing a single headline `rating` (and
//! sometimes one entry in `ratings` keyed by provider). OMDb's
//! `?i=<imdb_id>` endpoint returns IMDb + Rotten Tomatoes (tomatometer)
//! + Metacritic in a single Ratings[] payload — getting that into
//! the per-source `ratings` map is what unlocks the catalog
//! aggregator's weighted composite for movies/series. Without this
//! pass the composite is whatever single-source rating TMDB or TVDB
//! provided, which is exactly the issue the user flagged.
//!
//! Mirrors music_enrich's progressive-snapshot design: spawn a
//! background task, fan out OMDb enrich calls per entry, flush a
//! grid_update every PROGRESS_BATCH_SIZE completions so cards
//! repaint in waves rather than all at once.
//!
//! ## Fast path
//!
//! Entries with `imdb_id` set hit OMDb's id endpoint directly (one
//! round-trip). Entries without imdb_id are skipped — OMDb's title
//! search is unreliable for non-canonical titles and not worth the
//! quota burn for the marginal coverage. TMDB-sourced movies usually
//! carry an imdb_id; TVDB-sourced series do too.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use stui_plugin_sdk::EntryKind;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};

use crate::abi::types::{EnrichRequest, PluginEntry};
use crate::catalog::CatalogEntry;
use crate::catalog_engine::aggregator::apply_weighted_rating;
use crate::engine::Engine;

const PLUGIN_OMDB: &str = "omdb";

/// OMDb's per-IP rate limit on the free tier is 1000 requests/day with
/// no per-second ceiling. Concurrency=4 lets us fan out without
/// hitting any practical limit on typical grid sizes (~50 entries).
const ENRICH_CONCURRENCY: usize = 4;

/// Flush a snapshot every N entries so cards repaint progressively.
const PROGRESS_BATCH_SIZE: usize = 8;

/// Run movies/series enrichment, calling `on_progress(snapshot)`
/// after every batch. Final call carries the fully-enriched grid.
pub async fn enrich_grid_progressive<F, Fut>(
    engine: Arc<Engine>,
    entries: Vec<CatalogEntry>,
    on_progress: F,
)
where
    F: Fn(Vec<CatalogEntry>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    if entries.is_empty() {
        return;
    }

    let total = entries.len();
    let snapshot = Arc::new(Mutex::new(entries));
    let completed = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(ENRICH_CONCURRENCY));
    let on_progress = Arc::new(on_progress);

    let mut tasks = Vec::with_capacity(total);
    for idx in 0..total {
        let engine = engine.clone();
        let snapshot = snapshot.clone();
        let completed = completed.clone();
        let sem = sem.clone();
        let on_progress = on_progress.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            let entry = {
                let snap = snapshot.lock().await;
                snap[idx].clone()
            };
            let enriched = enrich_one(&engine, entry).await;
            let should_flush = {
                let mut snap = snapshot.lock().await;
                snap[idx] = enriched;
                let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
                if done % PROGRESS_BATCH_SIZE == 0 || done == total {
                    Some(snap.clone())
                } else {
                    None
                }
            };
            if let Some(snap) = should_flush {
                on_progress(snap).await;
            }
        }));
    }

    for task in tasks {
        if let Err(e) = task.await {
            warn!("video_enrich: task join error: {e}");
        }
    }
    info!(total, "video_enrich: pass complete");
}

async fn enrich_one(engine: &Engine, mut entry: CatalogEntry) -> CatalogEntry {
    let imdb_id = match entry.imdb_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => id.to_string(),
        None => return entry, // skip — no imdb id, no fast path
    };
    // Pick an EntryKind matching the entry's tab so the OMDb plugin
    // routes the request correctly. The exact kind doesn't change
    // the API call (?i=<id> is identical for movies and series) but
    // gates the kind-check the plugin runs internally.
    let kind = if entry.tab == "series" {
        EntryKind::Series
    } else {
        EntryKind::Movie
    };

    let mut partial = PluginEntry {
        kind,
        title: entry.title.clone(),
        imdb_id: Some(imdb_id.clone()),
        ..Default::default()
    };
    partial
        .external_ids
        .insert("imdb".to_string(), imdb_id);

    let req = EnrichRequest { partial, prefer_id_source: None };
    let resp = match engine.supervisor_enrich(PLUGIN_OMDB, req).await {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("PluginNotFound") {
                debug!("video_enrich: omdb plugin not loaded, skipping");
            } else {
                warn!(title = %entry.title, error = %msg, "video_enrich: omdb enrich failed");
            }
            return entry;
        }
    };

    // Merge OMDb's per-source ratings into the entry. Each key is
    // already keyed to match the aggregator's weight profile names
    // (`imdb`, `tomatometer`, `metacritic`).
    let mut updated_any = false;
    for (key, score) in &resp.ratings {
        entry.ratings.insert(key.clone(), *score as f64);
        updated_any = true;
    }
    if !updated_any {
        return entry;
    }
    debug!(
        title = %entry.title,
        ratings = ?entry.ratings,
        "video_enrich: ratings merged",
    );

    // Recompute the composite headline rating now that the ratings
    // map is fuller. Without this the entry's `rating` field stays
    // at whatever single-source value the original provider set.
    apply_weighted_rating(&mut entry);
    entry
}
