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
//!
//! ## Bulk pass
//!
//! Before the per-entry fan-out, plugins that declare the `bulk_enrich`
//! capability are called once per kind with all eligible entries batched
//! together. Plugins that succeed are removed from the per-entry cohort
//! (no double-work). Plugins that return a transient error fall through
//! back into the per-entry cohort so data isn't silently dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use stui_plugin_sdk::{EntryKind, BulkEnrichRequest, BulkEnrichResponse, BulkEnrichEntry, PluginResult};
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

    let total = entries.len();
    let snapshot = Arc::new(Mutex::new(entries));

    // ── Bulk pass ────────────────────────────────────────────────────────────
    // Discover bulk-capable plugins and run them first. Plugins that succeed
    // are removed from the per-entry cohort. Plugins that fail with a
    // transient error fall through so no data is silently dropped.
    let movie_bulk_plugins  = engine.bulk_enrich_plugins_for_kind(EntryKind::Movie).await;
    let series_bulk_plugins = engine.bulk_enrich_plugins_for_kind(EntryKind::Series).await;

    // Per-entry cohort starts as: enrich plugins MINUS bulk-capable ones.
    let mut movie_per_entry_plugins: Vec<String> = movie_plugins
        .iter()
        .filter(|name| !movie_bulk_plugins.contains(name))
        .cloned()
        .collect();
    let mut series_per_entry_plugins: Vec<String> = series_plugins
        .iter()
        .filter(|name| !series_bulk_plugins.contains(name))
        .cloned()
        .collect();

    if !movie_bulk_plugins.is_empty() || !series_bulk_plugins.is_empty() {
        // Build the dispatch adapter. EngineMetadataDispatch::new requires a
        // MetadataSources config which is only available at IPC time; for the
        // bulk pass we only use call_bulk_enrich which routes directly through
        // supervisor_bulk_enrich — construct a minimal adapter via a local
        // wrapper so we don't need to thread config through here.
        let bulk_dispatch = EngineBulkDispatch(engine.clone());
        let fall_through = run_bulk_pass(
            &bulk_dispatch,
            &movie_bulk_plugins,
            &series_bulk_plugins,
            snapshot.clone(),
        ).await;

        // Plugins that errored transiently go back into per-entry.
        for (name, kind) in fall_through {
            match kind {
                EntryKind::Movie  => movie_per_entry_plugins.push(name),
                EntryKind::Series => series_per_entry_plugins.push(name),
                _ => {}
            }
        }

        // Flush progress immediately so TUI sees bulk results.
        {
            let snap = snapshot.lock().await;
            on_progress(snap.clone()).await;
        }
    }

    // ── Per-entry fan-out ────────────────────────────────────────────────────
    let movie_per_entry_plugins  = Arc::new(movie_per_entry_plugins);
    let series_per_entry_plugins = Arc::new(series_per_entry_plugins);
    let completed = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(ENRICH_CONCURRENCY));
    let on_progress = Arc::new(on_progress);

    let mut tasks = Vec::with_capacity(total);
    for idx in 0..total {
        let engine = engine.clone();
        let movie_plugins  = movie_per_entry_plugins.clone();
        let series_plugins = series_per_entry_plugins.clone();
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
        if merge_enrich_into(&mut entry, p) {
            got_any = true;
        }
    }

    if got_any {
        debug!(title = %entry.title, ratings = ?entry.ratings, "video_enrich: enrichment merged");
        apply_weighted_rating(&mut entry);
    }
    entry
}

// ── Shared merge helper ───────────────────────────────────────────────────────

/// Merge an abi `PluginEntry` into a `CatalogEntry`, using the same
/// fill-not-overwrite semantics as the original `enrich_one` loop body.
///
/// Returns `true` if any field was updated (so callers can decide
/// whether to call `apply_weighted_rating`).
fn merge_enrich_into(entry: &mut CatalogEntry, p: PluginEntry) -> bool {
    let mut got_any = false;

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
            // No source name available from partial — use a generic key.
            "enrich".to_string()
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

    got_any
}

// ── Bulk pass helpers ─────────────────────────────────────────────────────────

/// Minimal dispatch adapter that wraps `Arc<Engine>` and provides
/// `call_bulk_enrich` for the bulk pass, without requiring a full
/// `EngineMetadataDispatch` (which needs a MetadataSources config).
struct EngineBulkDispatch(Arc<Engine>);

impl EngineBulkDispatch {
    async fn call_bulk_enrich(
        &self,
        plugin: &str,
        req: BulkEnrichRequest,
    ) -> Result<BulkEnrichResponse, String> {
        self.0
            .supervisor_bulk_enrich(plugin, req, CallPriority::Background)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Run the bulk-enrichment pass against the given dispatch. Returns
/// the list of (plugin_name, kind) pairs that failed with a
/// non-NOT_IMPLEMENTED error and should fall through to per-entry.
async fn run_bulk_pass<D: BulkDispatch>(
    dispatch: &D,
    movie_bulk_plugins:  &[String],
    series_bulk_plugins: &[String],
    snapshot: Arc<Mutex<Vec<CatalogEntry>>>,
) -> Vec<(String, EntryKind)> {
    let mut fall_through: Vec<(String, EntryKind)> = Vec::new();

    for (kind, plugins) in [
        (EntryKind::Movie,  movie_bulk_plugins),
        (EntryKind::Series, series_bulk_plugins),
    ] {
        if plugins.is_empty() { continue; }

        let kind_str = match kind {
            EntryKind::Movie  => "movies",
            EntryKind::Series => "series",
            _ => continue,
        };

        // Snapshot eligible entries (this kind, has imdb_id).
        let entries_for_kind: Vec<CatalogEntry> = {
            let snap = snapshot.lock().await;
            snap.iter()
                .filter(|e| e.tab == kind_str
                    && e.imdb_id.as_deref().filter(|s| !s.is_empty()).is_some())
                .cloned()
                .collect()
        };
        if entries_for_kind.is_empty() { continue; }

        for plugin in plugins {
            let partials: Vec<stui_plugin_sdk::PluginEntry> = entries_for_kind.iter()
                .map(|e| build_sdk_partial_from_entry(e, kind))
                .collect();

            let req = BulkEnrichRequest {
                partials,
                prefer_id_source: None,
                force_refresh: false,
            };

            match dispatch.call_bulk_enrich(plugin, req).await {
                Ok(resp) => {
                    let mut snap = snapshot.lock().await;
                    for entry in resp.entries {
                        if let PluginResult::Ok(enrich_resp) = entry.result {
                            let p = sdk_plugin_entry_to_abi(enrich_resp.entry);
                            if let Some(idx) = find_snapshot_idx(&snap, &entry.id) {
                                let got_any = merge_enrich_into(&mut snap[idx], p);
                                if got_any {
                                    apply_weighted_rating(&mut snap[idx]);
                                }
                            }
                        }
                    }
                }
                Err(e) if e.starts_with("not_implemented:") || e.contains("NOT_IMPLEMENTED") => {
                    tracing::warn!(plugin = %plugin, "bulk_enrich returned NOT_IMPLEMENTED — skipping plugin entirely");
                }
                Err(other) => {
                    tracing::warn!(plugin = %plugin, error = %other, "bulk_enrich failed; falling through to per-entry");
                    fall_through.push((plugin.clone(), kind));
                }
            }
        }
    }

    fall_through
}

/// Find the snapshot entry by stable id (imdb_id wins; falls back to entry.id).
fn find_snapshot_idx(snap: &[CatalogEntry], id: &str) -> Option<usize> {
    snap.iter().position(|e|
        e.imdb_id.as_deref() == Some(id) || e.id == id
    )
}

/// Build an SDK `PluginEntry` partial from a `CatalogEntry` for use as
/// a `BulkEnrichRequest` element.
fn build_sdk_partial_from_entry(entry: &CatalogEntry, kind: EntryKind) -> stui_plugin_sdk::PluginEntry {
    let imdb_id = entry.imdb_id.clone().unwrap_or_default();
    let mut partial = stui_plugin_sdk::PluginEntry {
        kind,
        title: entry.title.clone(),
        imdb_id: Some(imdb_id.clone()),
        ..Default::default()
    };
    partial.external_ids.insert("imdb".to_string(), imdb_id);
    partial
}

/// Convert an SDK `PluginEntry` into an abi `PluginEntry` for merging.
/// Both structs are wire-compatible (same serde schema); the conversion
/// goes through JSON to avoid a hard dependency between the two crates'
/// type definitions.
fn sdk_plugin_entry_to_abi(sdk: stui_plugin_sdk::PluginEntry) -> PluginEntry {
    // SAFETY: Both types are serde-compatible with identical field names
    // and JSON encodings. A compile error here means the schemas diverged
    // and both sides need to be updated together.
    serde_json::from_value(
        serde_json::to_value(sdk).expect("sdk PluginEntry serialization must not fail")
    ).expect("abi PluginEntry deserialization must not fail — schema diverged?")
}

// ── BulkDispatch trait (test seam) ───────────────────────────────────────────

/// Minimal async dispatch surface needed by `run_bulk_pass`.
/// Implemented by `EngineBulkDispatch` in production and by
/// `test_dispatch::PluginDispatch` in tests.
#[async_trait::async_trait]
trait BulkDispatch: Send + Sync {
    async fn call_bulk_enrich(
        &self,
        plugin: &str,
        req: BulkEnrichRequest,
    ) -> Result<BulkEnrichResponse, String>;
}

#[async_trait::async_trait]
impl BulkDispatch for EngineBulkDispatch {
    async fn call_bulk_enrich(
        &self,
        plugin: &str,
        req: BulkEnrichRequest,
    ) -> Result<BulkEnrichResponse, String> {
        EngineBulkDispatch::call_bulk_enrich(self, plugin, req).await
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod video_enrich_bulk_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use stui_plugin_sdk::{
        BulkEnrichResponse, BulkEnrichEntry, PluginResult,
        EntryKind as SdkEntryKind,
    };

    // A minimal BulkDispatch backed by canned response maps.
    struct CannedDispatch {
        responses: HashMap<String, Result<BulkEnrichResponse, String>>,
    }

    #[async_trait::async_trait]
    impl BulkDispatch for CannedDispatch {
        async fn call_bulk_enrich(
            &self,
            plugin: &str,
            _req: BulkEnrichRequest,
        ) -> Result<BulkEnrichResponse, String> {
            match self.responses.get(plugin) {
                Some(Ok(resp)) => Ok(resp.clone()),
                Some(Err(e)) => Err(e.clone()),
                None => Err(format!("no canned response for plugin {plugin}")),
            }
        }
    }

    fn make_test_entry(id: &str, imdb: &str, kind: &str) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            tab: kind.into(),
            imdb_id: Some(imdb.into()),
            title: format!("Title {id}"),
            ..Default::default()
        }
    }

    fn ok_bulk_entry(imdb: &str, rating: f32) -> BulkEnrichEntry {
        BulkEnrichEntry {
            id: imdb.into(),
            result: PluginResult::ok(stui_plugin_sdk::EnrichResponse {
                entry: stui_plugin_sdk::PluginEntry {
                    id: imdb.into(),
                    kind: SdkEntryKind::Movie,
                    title: format!("Bulk enriched {imdb}"),
                    source: "bulky".into(),
                    rating: Some(rating),
                    ..Default::default()
                },
                confidence: 1.0,
            }),
        }
    }

    #[tokio::test]
    async fn run_bulk_pass_routes_via_bulk_when_capability_declared() {
        let snapshot = Arc::new(Mutex::new(vec![
            make_test_entry("1", "tt0000001", "movies"),
            make_test_entry("2", "tt0000002", "movies"),
            make_test_entry("3", "tt0000003", "movies"),
        ]));

        let mut responses = HashMap::new();
        responses.insert("bulky".to_string(), Ok(BulkEnrichResponse {
            entries: vec![
                ok_bulk_entry("tt0000001", 8.5),
                ok_bulk_entry("tt0000002", 7.5),
                ok_bulk_entry("tt0000003", 9.0),
            ],
        }));
        let dispatch = CannedDispatch { responses };

        let fall_through = run_bulk_pass(
            &dispatch,
            &["bulky".to_string()],
            &[],
            snapshot.clone(),
        ).await;

        assert!(fall_through.is_empty(), "successful bulk = no fall-through");
        let snap = snapshot.lock().await;
        // rating is Option<String>; entries should have been enriched
        // (rating inserted into the ratings map → apply_weighted_rating sets rating).
        for entry in snap.iter() {
            assert!(
                !entry.ratings.is_empty(),
                "entry {} should have ratings from bulk enrich", entry.id
            );
        }
    }

    #[tokio::test]
    async fn run_bulk_pass_skips_plugin_on_top_level_not_implemented() {
        let snapshot = Arc::new(Mutex::new(vec![
            make_test_entry("1", "tt0000001", "movies"),
        ]));

        let mut responses = HashMap::new();
        responses.insert("bulky".to_string(),
            Err("not_implemented: verb not implemented by this plugin".to_string()));
        let dispatch = CannedDispatch { responses };

        let fall_through = run_bulk_pass(
            &dispatch,
            &["bulky".to_string()],
            &[],
            snapshot.clone(),
        ).await;

        assert!(fall_through.is_empty(),
                "NOT_IMPLEMENTED should NOT add plugin to fall_through");
    }

    #[tokio::test]
    async fn run_bulk_pass_adds_plugin_to_fall_through_on_other_error() {
        let snapshot = Arc::new(Mutex::new(vec![
            make_test_entry("1", "tt0000001", "movies"),
        ]));

        let mut responses = HashMap::new();
        responses.insert("bulky".to_string(),
            Err("transient: upstream timeout".to_string()));
        let dispatch = CannedDispatch { responses };

        let fall_through = run_bulk_pass(
            &dispatch,
            &["bulky".to_string()],
            &[],
            snapshot.clone(),
        ).await;

        assert_eq!(fall_through.len(), 1);
        assert_eq!(fall_through[0].0, "bulky");
        assert_eq!(fall_through[0].1, EntryKind::Movie);
    }
}
