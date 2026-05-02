//! music_enrich — second-pass enrichment of music-tab catalog entries.
//!
//! Lastfm-sourced albums arrive in the catalog with title + artist +
//! poster but typically no year, no rating, no genre. This module
//! fans out per-entry `enrich` calls to **every loaded plugin that
//! declares the enrich capability for `EntryKind::Album`** — so
//! adding a new ratings/metadata provider is purely a plugin-install
//! operation, no runtime code change.
//!
//! Each plugin contributes whatever fields it has:
//! - `entry.rating` (single headline f32) → recorded under the
//!   plugin's `source` name in `entry.ratings`.
//! - `entry.ratings` (per-source map for plugins that aggregate, e.g.
//!   OMDb's IMDb+RT+Metacritic block) → merged in directly.
//! - `entry.year` / `entry.genre` / `entry.description` → first
//!   non-empty value wins, since the catalog entry might already
//!   have these from a prior provider.
//!
//! The composite headline rating (`entry.rating` as a string) is
//! recomputed by `apply_weighted_rating` after all plugins have
//! responded — it picks the right weight profile (with user
//! overrides via `RatingSourceWeights`) and computes a weighted
//! median across whatever sources actually populated.
//!
//! Designed for the **progressive** flow: the catalog emits the
//! unenriched grid_update first (fast first-paint), then this task
//! streams snapshots back via `on_progress` every
//! [`PROGRESS_BATCH_SIZE`] entries — cards repaint in waves.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use stui_plugin_sdk::EntryKind;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};

use crate::abi::types::{EnrichRequest, PluginEntry};
use crate::catalog::CatalogEntry;
use crate::catalog_engine::aggregator::apply_weighted_rating;
use crate::engine::{CallPriority, Engine};

/// Concurrent enrich tasks. Higher values fan out more work, but each
/// per-plugin token bucket throttles upstream requests independently —
/// excess concurrency just queues at the supervisor.
const ENRICH_CONCURRENCY: usize = 4;

/// Flush a snapshot every N entries finished.
const PROGRESS_BATCH_SIZE: usize = 8;

/// Run music-grid enrichment, calling `on_progress(snapshot)` after
/// every batch. Final call carries the fully-enriched grid.
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

    // Snapshot the plugin set ONCE at task start. Hot-reload races
    // (a plugin appearing or disappearing mid-pass) are rare and
    // the next refresh picks up the new set anyway.
    let plugins = engine.enrich_plugins_for_kind(EntryKind::Album).await;
    if plugins.is_empty() {
        info!("music_enrich: no plugins declare enrich for Album — skipping pass");
        return;
    }
    info!(plugins = ?plugins, count = entries.len(), "music_enrich: starting pass");

    let plugins = Arc::new(plugins);
    let total = entries.len();
    let snapshot = Arc::new(Mutex::new(entries));
    let completed = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(ENRICH_CONCURRENCY));
    let on_progress = Arc::new(on_progress);

    let mut tasks = Vec::with_capacity(total);
    for idx in 0..total {
        let engine = engine.clone();
        let plugins = plugins.clone();
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
            let enriched = enrich_one(&engine, &plugins, entry).await;
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
            warn!("music_enrich: task join error: {e}");
        }
    }
    info!(total, "music_enrich: pass complete");
}

async fn enrich_one(
    engine: &Engine,
    plugins: &[String],
    mut entry: CatalogEntry,
) -> CatalogEntry {
    let title = entry.title.trim().to_string();
    if title.is_empty() {
        return entry;
    }
    let artist = entry.artist.clone();

    // Build the partial PluginEntry once and clone for each request —
    // every plugin gets the same title+artist+kind shape and decides
    // whether to use it.
    let partial = PluginEntry {
        kind: EntryKind::Album,
        title: title.clone(),
        artist_name: artist.clone(),
        ..Default::default()
    };

    // Fan out to every loaded enrich-capable Album plugin in parallel.
    // Different providers, different rate-limit buckets — no contention.
    let futs: Vec<_> = plugins
        .iter()
        .map(|name| {
            let req = EnrichRequest {
                partial: partial.clone(),
                prefer_id_source: None,
                force_refresh: false,
            };
            let name = name.clone();
            async move {
                let res = engine.supervisor_enrich(&name, req, CallPriority::Background).await;
                (name, res)
            }
        })
        .collect();
    let results = futures::future::join_all(futs).await;

    let mut got_any_rating = false;
    for (plugin, res) in results {
        let p = match res {
            Ok(p) => p,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("PluginNotFound") {
                    debug!(plugin = %plugin, "music_enrich: plugin not loaded");
                } else {
                    warn!(plugin = %plugin, album = %title, error = %msg, "music_enrich: enrich failed");
                }
                continue;
            }
        };
        merge_plugin_response(&mut entry, &plugin, p, &mut got_any_rating);
    }

    if got_any_rating {
        // Recompute headline rating from the per-source map. This
        // lets the user's rating-source weights drive priority
        // (e.g. weight musicbrainz higher than discogs, or vice versa).
        apply_weighted_rating(&mut entry);
    }
    entry
}

fn merge_plugin_response(
    entry: &mut CatalogEntry,
    plugin: &str,
    p: PluginEntry,
    got_any_rating: &mut bool,
) {
    // Year — first non-empty wins. Plugins are best-effort; not all
    // know release year (lastfm doesn't, MB does).
    if entry.year.is_none() {
        if let Some(y) = p.year {
            entry.year = Some(y.to_string());
            debug!(album = %entry.title, plugin = %plugin, year = y, "music_enrich: year");
        }
    }
    // Genre — first non-empty wins. lastfm's tag-derived genre has
    // the highest coverage; MB's primary_type is more authoritative
    // when present.
    if entry.genre.is_none() {
        if let Some(g) = p.genre.clone() {
            entry.genre = Some(g);
        }
    }
    // Description — first non-empty wins.
    if entry.description.is_none() {
        if let Some(d) = p.description.clone() {
            entry.description = Some(d);
        }
    }

    // Single headline rating from the plugin → record under the
    // plugin's `source` field name. Fall back to the plugin name
    // itself if `source` is empty (shouldn't be, but defensive).
    let source_key = if !p.source.is_empty() {
        p.source.clone()
    } else {
        plugin.to_string()
    };
    if let Some(r) = p.rating {
        entry.ratings.insert(source_key.clone(), r as f64);
        *got_any_rating = true;
        debug!(album = %entry.title, source = %source_key, rating = r, "music_enrich: rating");
    }

    // Per-source ratings map (OMDb-style) — merge in as-is.
    for (k, v) in p.ratings {
        entry.ratings.insert(k, v as f64);
        *got_any_rating = true;
    }
}
