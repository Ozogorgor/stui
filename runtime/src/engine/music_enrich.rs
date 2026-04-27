//! music_enrich — second-pass enrichment of music-tab catalog entries.
//!
//! When the music tab is hydrated from lastfm via tag.gettopalbums, each
//! entry arrives with title + artist + poster but typically no year and
//! no rating. This module fans out per-entry enrich calls to the
//! musicbrainz and discogs plugins to fill those fields in.
//!
//! Designed for the **progressive** flow: the catalog emits the
//! unenriched grid_update first (fast first-paint), then a background
//! task calls [`enrich_grid_progressive`] which streams updated
//! snapshots via a callback as each batch of entries finishes
//! enriching. The TUI's GridUpdateMsg handler is idempotent, so cards
//! repaint with year + rating filling in over time.
//!
//! ## Why title+artist (not external_ids)
//!
//! The PluginEntry → MediaEntry → CatalogEntry conversion drops
//! `external_ids` (only specific named ids — imdb/tmdb/mal — are
//! preserved). Rather than thread musicbrainz_id through three layers
//! of types, we exercise each plugin's title+artist fallback path:
//! both musicbrainz and discogs support enriching from a partial
//! PluginEntry that only carries `title` and `artist_name`, and both
//! cache the resulting HTTP responses via the runtime's sqlite cache —
//! so the second enrichment for a known album is effectively free.
//!
//! ## Performance
//!
//! Per-entry, MB and Discogs are called in **parallel** (different
//! providers, separate rate-limit pools) so the per-entry latency is
//! `max(MB, Discogs)` rather than `MB + Discogs`. Across entries we
//! fan out at concurrency=4 — the per-plugin token bucket (declared
//! in each plugin.toml's `[rate_limit]`) paces out the actual upstream
//! request rate, so concurrency above the rate limit just queues at
//! the supervisor instead of overrunning the API.
//!
//! Snapshots are flushed every [`PROGRESS_BATCH_SIZE`] entries so the
//! user sees year/rating land in waves rather than waiting for the
//! whole pass to complete.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use stui_plugin_sdk::EntryKind;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};

use crate::abi::types::{EnrichRequest, PluginEntry};
use crate::catalog::CatalogEntry;
use crate::engine::Engine;

/// Plugin ids — must match the `[plugin] name` in each provider's
/// plugin.toml. If a plugin isn't loaded, supervisor_enrich returns
/// PluginNotFound and the per-entry enrich call is silently skipped.
const PLUGIN_MUSICBRAINZ: &str = "musicbrainz";
const PLUGIN_DISCOGS: &str = "discogs";
const PLUGIN_LASTFM: &str = "lastfm";

/// Concurrent enrich tasks. Higher values fan out more work, but each
/// per-plugin token bucket throttles upstream requests independently —
/// excess concurrency just queues at the supervisor. 4 is a balance:
/// enough to keep both providers busy in parallel, low enough that
/// task scheduling overhead and rate-limit queueing don't stall.
const ENRICH_CONCURRENCY: usize = 4;

/// Flush a progress snapshot every N entries finished. Each flush ships
/// a fresh grid_update over IPC; cards in the TUI repaint with the
/// current state. Smaller = smoother UX, larger = less IPC overhead.
const PROGRESS_BATCH_SIZE: usize = 8;

/// Run the music-grid enrichment in the background, calling
/// `on_progress(snapshot)` after every [`PROGRESS_BATCH_SIZE`]
/// entries finish. The final call to `on_progress` carries the fully
/// enriched grid.
///
/// `on_progress` is invoked from inside this future — keep it cheap
/// (it should typically just emit a grid_update and write the disk
/// cache).
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
    // Shared state: the in-progress snapshot. Each task writes its
    // enriched entry into its slot. After every N completions a
    // snapshot is cloned and shipped to the callback.
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
            // Take the entry out of the snapshot under the mutex so
            // we can run enrich without holding the lock.
            let entry = {
                let snap = snapshot.lock().await;
                snap[idx].clone()
            };
            let enriched = enrich_one(&engine, entry).await;
            // Write back, count, maybe flush.
            let should_flush_snapshot = {
                let mut snap = snapshot.lock().await;
                snap[idx] = enriched;
                let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
                // Flush every batch boundary AND on the last entry.
                if done % PROGRESS_BATCH_SIZE == 0 || done == total {
                    Some(snap.clone())
                } else {
                    None
                }
            };
            if let Some(snap) = should_flush_snapshot {
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

async fn enrich_one(engine: &Engine, mut entry: CatalogEntry) -> CatalogEntry {
    let title = entry.title.trim().to_string();
    if title.is_empty() {
        return entry;
    }
    let artist = entry.artist.clone();

    // Run all three enrich providers in parallel — different rate-
    // limit pools, no contention. Per-entry latency is
    // max(MB, Discogs, lastfm) instead of the sum.
    let need_year = entry.year.is_none();
    let need_rating = entry.rating.is_none();
    let need_genre = entry.genre.is_none();
    let need_description = entry.description.is_none();
    // lastfm enrich is worth running whenever we'd benefit from any
    // of its outputs: synthetic rating (only if Discogs ends up
    // empty), genre tag, or wiki description.
    let want_lastfm = need_rating || need_genre || need_description;

    let (mb_res, dg_res, lf_res) = tokio::join!(
        async {
            if need_year {
                call_enrich(engine, PLUGIN_MUSICBRAINZ, &title, artist.as_deref()).await
            } else {
                None
            }
        },
        async {
            if need_rating {
                call_enrich(engine, PLUGIN_DISCOGS, &title, artist.as_deref()).await
            } else {
                None
            }
        },
        async {
            if want_lastfm {
                call_enrich(engine, PLUGIN_LASTFM, &title, artist.as_deref()).await
            } else {
                None
            }
        },
    );

    if let Some(p) = mb_res {
        if let Some(y) = p.year {
            entry.year = Some(y.to_string());
            debug!(album = %title, year = y, "music_enrich: MB year");
        }
    }
    // Rating preference: Discogs (real community 5-star) wins over
    // lastfm (synthetic from listener count). Discogs only returns
    // ratings for releases with actual community votes — most niche
    // albums have none — so the lastfm fallback gets us coverage on
    // the long tail.
    if let Some(p) = dg_res {
        if let Some(r) = p.rating {
            entry.rating = Some(format!("{r:.2}"));
            entry.ratings.insert("discogs".to_string(), r as f64);
            debug!(album = %title, rating = r, "music_enrich: Discogs rating");
        }
    }
    if let Some(p) = lf_res {
        // Genre / description always taken from lastfm if we don't
        // already have them — lastfm's tag-based genre is the
        // best-coverage source for music.
        if entry.genre.is_none() {
            if let Some(g) = p.genre.clone() {
                entry.genre = Some(g);
            }
        }
        if entry.description.is_none() {
            if let Some(d) = p.description.clone() {
                entry.description = Some(d);
            }
        }
        // Synthetic rating fills in only when Discogs left rating
        // empty — see synth_rating_from_listeners in
        // lastfm-provider for the formula.
        if entry.rating.is_none() {
            if let Some(r) = p.rating {
                entry.rating = Some(format!("{r:.2}"));
                entry.ratings.insert("lastfm".to_string(), r as f64);
                debug!(album = %title, rating = r, "music_enrich: lastfm synthetic rating");
            }
        }
    }

    entry
}

async fn call_enrich(
    engine: &Engine,
    plugin: &str,
    title: &str,
    artist: Option<&str>,
) -> Option<PluginEntry> {
    let mut partial = PluginEntry {
        kind: EntryKind::Album,
        title: title.to_string(),
        ..Default::default()
    };
    partial.artist_name = artist.map(|s| s.to_string());

    let req = EnrichRequest {
        partial,
        prefer_id_source: None,
    };
    match engine.supervisor_enrich(plugin, req).await {
        Ok(entry) => Some(entry),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("PluginNotFound") {
                debug!(plugin, "music_enrich: plugin not loaded, skipping");
            } else {
                warn!(plugin, album = %title, error = %msg, "music_enrich: enrich failed");
            }
            None
        }
    }
}
