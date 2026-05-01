//! video_enrich — second-pass enrichment for movie / series catalog
//! entries.
//!
//! TMDB / TVDB / kitsu / anilist / etc. each contribute a single
//! headline rating (and sometimes one entry in `ratings` keyed by
//! provider). This pass fans out to **every loaded plugin that
//! declares enrich for `EntryKind::Movie` (or Series)** to pull in
//! additional sources — most notably OMDb's `Ratings[]` block which
//! carries IMDb + Rotten Tomatoes + Metacritic in a single payload.
//!
//! Mirrors music_enrich's design: dynamic plugin discovery, parallel
//! per-entry fan-out, progressive grid_update snapshots every
//! [`PROGRESS_BATCH_SIZE`] entries.
//!
//! ## Fast path
//!
//! Entries with `imdb_id` set go straight to id-based enrich
//! (one HTTP round-trip per plugin). Entries without imdb_id are
//! skipped — title-search fallback for movies/series is unreliable
//! and not worth the quota burn.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use stui_plugin_sdk::EntryKind;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};

use crate::abi::types::{EnrichRequest, PluginEntry};
use crate::catalog::CatalogEntry;
use crate::catalog_engine::aggregator::apply_weighted_rating;
use crate::engine::{CallPriority, Engine};

// Bumped 4 → 8 (2026-05-01) after the TMDB bundle-cache (2026-04-30)
// collapsed per-item TMDB calls from 4 to 1 — the old conservative
// concurrency was sized for the pre-bundle worst case. 8 sustained
// requests/sec sits well under TMDB's ~50/sec soft ceiling and OMDB's
// 1k/day budget, while roughly halving wall-clock on a 200-item
// mdblist-driven catalog refresh.
const ENRICH_CONCURRENCY: usize = 8;
const PROGRESS_BATCH_SIZE: usize = 8;

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

    // Discover plugins for both Movie and Series — the catalog tab
    // dictates which kind matters per-entry, but a single plugin
    // commonly supports both (OMDb does), so we collect the union
    // and let per-entry filtering pick the right kind below.
    let movie_plugins = engine.enrich_plugins_for_kind(EntryKind::Movie).await;
    let series_plugins = engine.enrich_plugins_for_kind(EntryKind::Series).await;
    if movie_plugins.is_empty() && series_plugins.is_empty() {
        info!("video_enrich: no plugins declare enrich for Movie/Series — skipping pass");
        return;
    }
    info!(
        movie_plugins = ?movie_plugins,
        series_plugins = ?series_plugins,
        count = entries.len(),
        "video_enrich: starting pass",
    );

    let movie_plugins = Arc::new(movie_plugins);
    let series_plugins = Arc::new(series_plugins);
    let total = entries.len();
    let snapshot = Arc::new(Mutex::new(entries));
    let completed = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(ENRICH_CONCURRENCY));
    let on_progress = Arc::new(on_progress);

    let mut tasks = Vec::with_capacity(total);
    for idx in 0..total {
        let engine = engine.clone();
        let movie_plugins = movie_plugins.clone();
        let series_plugins = series_plugins.clone();
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
            let plugins: &[String] = if entry.tab == "series" {
                &series_plugins
            } else {
                &movie_plugins
            };
            let enriched = enrich_one(&engine, plugins, entry).await;
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

async fn enrich_one(
    engine: &Engine,
    plugins: &[String],
    mut entry: CatalogEntry,
) -> CatalogEntry {
    let imdb_id = match entry.imdb_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => id.to_string(),
        None => return entry, // skip — no imdb id, no fast path
    };
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
        .insert("imdb".to_string(), imdb_id.clone());

    // Fan out to every enrich-capable plugin for this kind.
    let futs: Vec<_> = plugins
        .iter()
        .map(|name| {
            let req = EnrichRequest {
                partial: partial.clone(),
                prefer_id_source: None,
            };
            let name = name.clone();
            async move {
                let res = engine.supervisor_enrich(&name, req, CallPriority::Background).await;
                (name, res)
            }
        })
        .collect();
    let results = futures::future::join_all(futs).await;

    let mut got_any = false;
    for (plugin, res) in results {
        let p = match res {
            Ok(p) => p,
            Err(e) => {
                let msg = e.to_string();
                if !msg.contains("PluginNotFound") {
                    warn!(plugin = %plugin, title = %entry.title, error = %msg, "video_enrich: enrich failed");
                }
                continue;
            }
        };

        // Each plugin's per-source ratings map (OMDb populates
        // imdb / tomatometer / metacritic in one response) merges
        // directly into the entry's per-source map.
        for (k, v) in p.ratings.iter() {
            entry.ratings.insert(k.clone(), *v as f64);
            got_any = true;
        }
        // Vote counts ride alongside ratings under the same source key
        // so the aggregator can apply Bayesian shrinkage to single-source
        // ratings with thin samples (e.g. one TMDB user voting 10/10).
        for (k, v) in p.rating_votes.iter() {
            entry.rating_votes.insert(k.clone(), *v);
        }
        // Single-headline rating goes under the plugin's source name
        // for plugins that don't break out per-source data.
        if let Some(r) = p.rating {
            let source_key = if !p.source.is_empty() {
                p.source.clone()
            } else {
                plugin.clone()
            };
            entry.ratings.insert(source_key, r as f64);
            got_any = true;
        }

        // Backfill visual / textual fields the catalog source didn't
        // provide. Never overwrite a value the source already set —
        // that would let later (lower-priority) plugins clobber
        // higher-priority data. Critical for mdblist-driven catalogs:
        // mdblist returns sparse rows (title + IDs only) and relies on
        // this enrichment pass to populate posters, overviews, etc.
        // Pre-mdblist this path was a no-op because TMDB-trending
        // already shipped rich rows.
        if entry.poster_url.is_none()
            && p.poster_url.as_deref().is_some_and(|s| !s.is_empty())
        {
            entry.poster_url = p.poster_url.clone();
            got_any = true;
        }
        if entry.description.is_none()
            && p.description.as_deref().is_some_and(|s| !s.is_empty())
        {
            entry.description = p.description.clone();
            got_any = true;
        }
        if entry.genre.is_none()
            && p.genre.as_deref().is_some_and(|s| !s.is_empty())
        {
            entry.genre = p.genre.clone();
            got_any = true;
        }
        if entry.year.is_none()
            && p.year.is_some_and(|y| y > 0)
        {
            entry.year = p.year.map(|y| y.to_string());
            got_any = true;
        }
        if entry.original_language.is_none()
            && p.original_language.as_deref().is_some_and(|s| !s.is_empty())
        {
            entry.original_language = p.original_language.clone();
            got_any = true;
        }
        // Cross-provider id backfill — every external_id we don't
        // already have helps downstream calls (other providers can
        // dispatch with a native id instead of title-searching).
        if entry.tmdb_id.is_none() {
            if let Some(id) = p.external_ids.get("tmdb").filter(|s| !s.is_empty()) {
                entry.tmdb_id = Some(id.clone());
                got_any = true;
            }
        }
        if entry.imdb_id.is_none() {
            if let Some(id) = p.external_ids.get("imdb").filter(|s| !s.is_empty()) {
                entry.imdb_id = Some(id.clone());
                got_any = true;
            }
        }
    }

    if got_any {
        debug!(title = %entry.title, ratings = ?entry.ratings, "video_enrich: enrichment merged");
        apply_weighted_rating(&mut entry);
    }
    entry
}
