//! Streaming scoped search: fans out per-scope and per-plugin, emits
//! per-scope ScopeResultsMsg events with partial-deadline + hard-floor
//! timing.
//!
//! Timing model:
//! - `partial_deadline` (default 500ms) starts on the FIRST plugin response
//!   within a scope. When it expires, emit a partial ScopeResultsMsg with
//!   results collected so far. Late plugins continue in the background.
//! - `hard_floor` (default 2000ms) starts at dispatch. If no plugin has
//!   responded by then, emit an empty partial ScopeResultsMsg so the UI
//!   shows *something* instead of a blank column.
//! - When all plugins have responded or timed out, emit a finalized
//!   ScopeResultsMsg (partial=false). If every plugin failed, set
//!   error=Some(ScopeError::AllFailed).
//! - A scope with no declared plugins emits an immediate finalized empty
//!   message with error=Some(ScopeError::NoPluginsConfigured).

use std::pin::Pin;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::Instrument as _;

use stui_plugin_sdk::SearchScope;

use crate::engine::{Engine, PluginCallError};
use crate::ipc::v1::{MediaEntry, ScopeError, ScopeResultsMsg};
use crate::ipc::v1::stream::{emit, Event, EventSender};

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ScopedSearchConfig {
    pub partial_deadline: Duration,
    pub hard_floor: Duration,
}

impl Default for ScopedSearchConfig {
    fn default() -> Self {
        Self {
            partial_deadline: Duration::from_millis(500),
            hard_floor: Duration::from_millis(2000),
        }
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Fan out a search across multiple scopes concurrently.
///
/// Spawns one task per scope; each task fans out to all plugins registered
/// for that scope via `Engine::supervisor_search`. Results are streamed back
/// as `Event::ScopeResults` messages over `out`.
pub async fn search_scoped(
    engine: Engine,
    query: String,
    scopes: Vec<SearchScope>,
    query_id: u64,
    cfg: ScopedSearchConfig,
    out: EventSender,
) {
    for scope in scopes {
        let engine = engine.clone();
        let query = query.clone();
        let out = out.clone();
        tokio::spawn(async move {
            run_one_scope(engine, query, scope, query_id, cfg, out).await;
        });
    }
}

// ── Per-scope orchestration ───────────────────────────────────────────────────

async fn run_one_scope(
    engine: Engine,
    query: String,
    scope: SearchScope,
    query_id: u64,
    cfg: ScopedSearchConfig,
    out: EventSender,
) {
    let span = tracing::info_span!("search_scoped::scope", query_id, ?scope);

    async move {
        let plugins = engine.plugins_for_scope(scope).await;

        // TVDB joins as a runtime-native source for Movie / Series scopes.
        // Decided here so an empty plugin set doesn't short-circuit when
        // TVDB can still answer alone.
        let tvdb_kind: Option<crate::tvdb::SearchKind> = match scope {
            SearchScope::Movie => Some(crate::tvdb::SearchKind::Movie),
            SearchScope::Series => Some(crate::tvdb::SearchKind::Series),
            _ => None,
        };
        let tvdb_handle =
            tvdb_kind.and_then(|kind| engine.tvdb().map(|client| (client, kind)));

        if plugins.is_empty() && tvdb_handle.is_none() {
            emit(&out, Event::ScopeResults(ScopeResultsMsg {
                query_id,
                scope,
                entries: Vec::new(),
                partial: false,
                error: Some(ScopeError::NoPluginsConfigured),
            })).await;
            return;
        }

        // Spawn per-plugin tasks. Engine::supervisor_search acquires the shared
        // plugin-call semaphore internally, so we don't acquire it here again.
        // Each task is instrumented with the parent scope span so tracing
        // hierarchies (e.g. tokio-console, Jaeger) show them as children.
        let mut handles: Vec<JoinHandle<Result<Vec<MediaEntry>, PluginCallError>>> = plugins
            .iter()
            .map(|pid| {
                let pid = pid.clone();
                let q = query.clone();
                let engine = engine.clone();
                let plugin_span = tracing::info_span!(
                    parent: &tracing::Span::current(),
                    "search_scoped::plugin",
                    plugin_id = %pid,
                );
                tokio::spawn(
                    async move {
                        engine.supervisor_search(&pid, &q, scope).await
                    }
                    .instrument(plugin_span),
                )
            })
            .collect();

        // Runtime-native TVDB task. Returns the same Result shape as the
        // plugin handles so the timing state machine treats it uniformly.
        // Errors are wrapped in PluginCallError::Other — TVDB is best-effort,
        // never fatal to the scope.
        if let Some((client, kind)) = tvdb_handle {
            let q = query.clone();
            let tvdb_span = tracing::info_span!(
                parent: &tracing::Span::current(),
                "search_scoped::tvdb",
            );
            handles.push(tokio::spawn(
                async move {
                    match client.search(&q, kind, 30).await {
                        Ok(items) => Ok(tvdb_items_to_entries(items, scope)),
                        Err(e) => Err(PluginCallError::Other(format!("tvdb: {e}"))),
                    }
                }
                .instrument(tvdb_span),
            ));
        }

        run_scope_timing(handles, query_id, scope, cfg, Some(engine.anime_bridge()), out).await;
    }
    .instrument(span)
    .await
}

/// Map TVDB's native `TvdbEntry` shape onto `MediaEntry` so its results
/// mix freely with plugin output. Mirrors the conversion used by
/// `search_catalog_entries` (engine/mod.rs ~1027) — kept duplicated for
/// now since the two paths diverge in non-trivial ways; consolidate
/// when one of them retires.
fn tvdb_items_to_entries(items: Vec<crate::tvdb::TvdbEntry>, scope: SearchScope) -> Vec<MediaEntry> {
    let tab = match scope {
        SearchScope::Movie => crate::ipc::MediaTab::Movies,
        SearchScope::Series => crate::ipc::MediaTab::Series,
        _ => crate::ipc::MediaTab::Library,
    };
    items
        .into_iter()
        .map(|e| MediaEntry {
            id: format!("tvdb-{}", e.tvdb_id),
            title: e.title,
            year: e.year,
            genre: (!e.genres.is_empty()).then(|| e.genres.join(", ")),
            rating: None,
            description: e.overview,
            poster_url: e.image_url,
            provider: "tvdb".to_string(),
            tab: tab.clone(),
            media_type: crate::ipc::MediaType::default(),
            ratings: std::collections::HashMap::new(),
            imdb_id: e.imdb_id,
            tmdb_id: e.tmdb_id,
            mal_id: None,
            original_language: e.original_language,
            kind: stui_plugin_sdk::EntryKind::default(),
            source: "tvdb".to_string(),
            artist_name: None,
            album_name: None,
            track_number: None,
            season: None,
            episode: None,
            season_count: None,
        })
        .collect()
}

// ── Timing state machine (testable inner core) ────────────────────────────────

/// Drive the partial-deadline + hard-floor timing state machine for a single
/// scope, given a pre-built set of plugin `JoinHandle`s.
///
/// This is the only function that touches the timers; `run_one_scope` is pure
/// glue that creates the handles from the Engine. Extracting the timing here
/// makes deterministic testing possible via `tokio::time::pause()` +
/// `tokio::time::advance()` without needing a mock Engine.
pub(crate) async fn run_scope_timing(
    handles: Vec<JoinHandle<Result<Vec<MediaEntry>, PluginCallError>>>,
    query_id: u64,
    scope: SearchScope,
    cfg: ScopedSearchConfig,
    bridge: Option<std::sync::Arc<crate::anime_bridge::AnimeBridge>>,
    out: EventSender,
) {
    if handles.is_empty() {
        // Caller should have already handled this, but be defensive.
        emit(&out, Event::ScopeResults(ScopeResultsMsg {
            query_id,
            scope,
            entries: Vec::new(),
            partial: false,
            error: Some(ScopeError::NoPluginsConfigured),
        })).await;
        return;
    }

    // Channel to collect plugin results in completion order.
    let n = handles.len();
    let (tx, mut rx) = mpsc::channel::<Result<Vec<MediaEntry>, PluginCallError>>(n);

    // Bridge: forward each JoinHandle result through the local channel so the
    // select! loop sees a uniform stream of plugin outcomes.
    for handle in handles {
        let tx = tx.clone();
        tokio::spawn(async move {
            let result = match handle.await {
                Ok(inner) => inner,
                Err(_join_err) => Err(PluginCallError::Other("plugin task panicked".into())),
            };
            let _ = tx.send(result).await;
        });
    }
    drop(tx); // drop original sender so channel closes when all bridges finish

    // Hard floor: fires from dispatch time if no plugin has responded yet.
    let hard_floor_fut = sleep(cfg.hard_floor);
    tokio::pin!(hard_floor_fut);

    // Partial deadline: fires `cfg.partial_deadline` after the FIRST response.
    // Represented as an Option<Pin<Box<Sleep>>> — None until first response.
    let mut partial_timer: Option<Pin<Box<tokio::time::Sleep>>> = None;

    let mut collected: Vec<MediaEntry> = Vec::new();
    let mut pending = n;
    let mut any_error = false;
    let mut emitted_partial = false;

    while pending > 0 {
        tokio::select! {
            biased;

            // ── Plugin result received ─────────────────────────────────────
            maybe = rx.recv() => match maybe {
                Some(Ok(entries)) => {
                    let n_new = entries.len();
                    collected.extend(entries);
                    // Cross-tier id enrichment. Fills missing mal_id /
                    // imdb_id / tmdb_id from the Fribb-fed anime bridge
                    // so both the partial-emission and final-emission
                    // `merge_dedupe` calls below see the same enriched
                    // view (one enrichment per entry, not per emit).
                    // `bridge` is `None` only in unit tests that drive
                    // `run_scope_timing` directly without an Engine.
                    if n_new > 0 {
                        if let Some(b) = bridge.as_deref() {
                            let start = collected.len() - n_new;
                            for entry in &mut collected[start..] {
                                crate::anime_bridge::enrich::enrich_entry(entry, b);
                            }
                        }
                    }
                    pending -= 1;
                    // Start the partial deadline timer on the first response.
                    if partial_timer.is_none() {
                        partial_timer = Some(Box::pin(sleep(cfg.partial_deadline)));
                    }
                }
                Some(Err(_e)) => {
                    any_error = true;
                    pending -= 1;
                    // Also start the partial timer on first error so we don't
                    // hold the partial open forever after all plugins fail.
                    if partial_timer.is_none() {
                        partial_timer = Some(Box::pin(sleep(cfg.partial_deadline)));
                    }
                }
                None => break, // all senders dropped (shouldn't happen while pending > 0)
            },

            // ── Partial deadline fired (has data or all-failed) ────────────
            _ = async {
                if let Some(ref mut t) = partial_timer {
                    t.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if !emitted_partial => {
                emit(&out, Event::ScopeResults(ScopeResultsMsg {
                    query_id,
                    scope,
                    entries: merge_dedupe(collected.clone()),
                    partial: true,
                    error: None,
                })).await;
                emitted_partial = true;
            }

            // ── Hard floor: no plugin responded within hard_floor ──────────
            _ = &mut hard_floor_fut, if !emitted_partial => {
                emit(&out, Event::ScopeResults(ScopeResultsMsg {
                    query_id,
                    scope,
                    entries: Vec::new(),
                    partial: true,
                    error: None,
                })).await;
                emitted_partial = true;
            }
        }

        // Early exit: once partial is emitted and all plugins are done, break
        // instead of spinning. The final emit happens outside the loop.
        if emitted_partial && pending == 0 {
            break;
        }
    }

    // ── Finalized emission ────────────────────────────────────────────────────
    let error = if collected.is_empty() && any_error {
        Some(ScopeError::AllFailed)
    } else {
        None
    };
    emit(&out, Event::ScopeResults(ScopeResultsMsg {
        query_id,
        scope,
        entries: merge_dedupe(collected),
        partial: false,
        error,
    })).await;
}

// ── Cross-provider dedup / merge ──────────────────────────────────────────────

/// Group key for collapsing the same entity across providers.
///
/// **Precedence:**
/// 1. `mal_id` — anime tier (AniList exposes `idMal`; Kitsu exposes
///    via `?include=mappings`).
/// 2. `imdb_id` — western tier (TVDB and OMDb both surface it at
///    search time; TMDB doesn't, so TMDB falls through to fallback).
/// 3. `normalize_title:year` — fallback for entries without a foreign
///    id (TMDB, AniList originals without MAL, Kitsu without mappings).
///
/// Empty-string foreign ids are treated as missing — defensive, prevents
/// `"mal:"` from collapsing all empty entries into one bucket.
///
/// Cross-tier merges (anime↔western) don't happen here — entries select
/// different keys. That's intentional; the cross-mapping bridge that
/// would unify them is milestone β.
fn dedup_key(e: &MediaEntry) -> String {
    if let Some(mal) = e.mal_id.as_deref().filter(|s| !s.is_empty()) {
        return format!("mal:{mal}");
    }
    if let Some(imdb) = e.imdb_id.as_deref().filter(|s| !s.is_empty()) {
        return format!("imdb:{imdb}");
    }
    format!(
        "title:{}:{}",
        crate::catalog::normalize_title(&e.title),
        e.year.as_deref().unwrap_or("?"),
    )
}

/// Collapse cross-provider duplicates into one entry per logical title.
/// Preserves first-seen order so streaming partials don't reshuffle the
/// grid as new providers respond.
///
/// Within a duplicate group we keep the highest-priority provider's
/// entry as the spine, then fold in fields from the others where the
/// spine is missing them — that way an OMDb-only `imdb_id` upgrades a
/// TMDB winner instead of being dropped.
fn merge_dedupe(entries: Vec<MediaEntry>) -> Vec<MediaEntry> {
    use std::collections::HashMap;

    let entries_count = entries.len();

    // First pass: bucket by key, preserving the order keys are first seen.
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<MediaEntry>> = HashMap::new();
    for e in entries {
        let k = dedup_key(&e);
        if !buckets.contains_key(&k) {
            order.push(k.clone());
        }
        buckets.entry(k).or_default().push(e);
    }

    // Second pass: collapse each bucket.
    let merged: Vec<MediaEntry> = order
        .into_iter()
        .filter_map(|k| {
            let mut group = buckets.remove(&k)?;
            // Lowest priority value wins. Stable sort preserves relative
            // order for ties (= same-provider duplicates).
            group.sort_by_key(|e| crate::anime_bridge::enrich::provider_priority_for_key(&e.provider, &k));
            let mut primary = group.remove(0);
            // Fold non-empty fields from secondaries into the primary's
            // None-shaped slots. Skip provider-distinguishing fields
            // (`id`, `provider`, `source`) — those identify the spine.
            for s in group {
                if primary.imdb_id.is_none() {
                    primary.imdb_id = s.imdb_id;
                }
                if primary.tmdb_id.is_none() {
                    primary.tmdb_id = s.tmdb_id;
                }
                if primary.mal_id.is_none() {
                    primary.mal_id = s.mal_id;
                }
                if primary.year.is_none() {
                    primary.year = s.year;
                }
                if primary.genre.is_none() {
                    primary.genre = s.genre;
                }
                if primary.rating.is_none() {
                    primary.rating = s.rating;
                }
                if primary.description.is_none() {
                    primary.description = s.description;
                }
                if primary.poster_url.is_none() {
                    primary.poster_url = s.poster_url;
                }
                if primary.original_language.is_none() {
                    primary.original_language = s.original_language;
                }
                // Per-provider raw scores merge — handy for the detail
                // ratings panel even after the duplicates collapse.
                for (k, v) in s.ratings {
                    primary.ratings.entry(k).or_insert(v);
                }
            }
            Some(primary)
        })
        .collect();

    tracing::debug!(
        input = entries_count,
        output = merged.len(),
        "merge_dedupe collapsed {} entries into {}",
        entries_count,
        merged.len(),
    );
    merged
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use stui_plugin_sdk::SearchScope;
    use crate::ipc::v1::{MediaEntry, MediaTab, MediaType, ScopeError};
    use crate::ipc::v1::stream::{Event, EventSender};

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_entry(id: &str) -> MediaEntry {
        MediaEntry {
            id: id.into(),
            title: id.into(),
            provider: "test".into(),
            tab: MediaTab::Music,
            source: "test".into(),
            ..Default::default()
        }
    }

    /// Drain all messages from an event receiver, parse them, and collect
    /// `ScopeResultsMsg` payloads.
    async fn collect_scope_results(
        mut rx: mpsc::Receiver<String>,
        wait_ms: u64,
    ) -> Vec<ScopeResultsMsg> {
        // Give tasks a moment to finish, then drain.
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        rx.close();
        let mut msgs = Vec::new();
        while let Ok(line) = rx.try_recv() {
            let event: Event = serde_json::from_str(line.trim()).expect("valid JSON");
            match event {
                Event::ScopeResults(m) => msgs.push(m),
            }
        }
        msgs
    }

    fn make_event_channel(cap: usize) -> (EventSender, mpsc::Receiver<String>) {
        mpsc::channel::<String>(cap)
    }

    // ── Test 1: all plugins return → produces one finalized message ───────────

    /// Two plugins respond quickly with one entry each; expect a single
    /// finalized ScopeResultsMsg with both entries merged.
    #[tokio::test]
    async fn all_plugins_return_produces_finalized() {
        tokio::time::pause();

        let cfg = ScopedSearchConfig {
            partial_deadline: Duration::from_millis(500),
            hard_floor:       Duration::from_millis(2000),
        };

        // Build two fast handles: one returns immediately, one after 10ms.
        let entry_a = make_entry("a");
        let entry_b = make_entry("b");
        let handle_a: JoinHandle<Result<Vec<MediaEntry>, PluginCallError>> =
            tokio::spawn(async move { Ok(vec![entry_a]) });
        let handle_b: JoinHandle<Result<Vec<MediaEntry>, PluginCallError>> = {
            let entry_b = entry_b.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(vec![entry_b])
            })
        };

        let (tx, rx) = make_event_channel(32);

        // Run the timing core and advance time past both plugin delays + partial deadline.
        let timing_fut = run_scope_timing(
            vec![handle_a, handle_b],
            1,
            SearchScope::Track,
            cfg,
            None,
            tx.clone(),
        );
        tokio::pin!(timing_fut);

        // Advance time: both plugins respond, partial deadline fires, finalize.
        tokio::time::advance(Duration::from_millis(600)).await;
        timing_fut.await;

        drop(tx);
        let mut rx_drain = rx;
        rx_drain.close();
        let mut msgs: Vec<ScopeResultsMsg> = Vec::new();
        while let Ok(line) = rx_drain.try_recv() {
            if let Ok(Event::ScopeResults(m)) = serde_json::from_str(line.trim()) {
                msgs.push(m);
            }
        }

        // Should have at least a finalized message.
        let finalized: Vec<_> = msgs.iter().filter(|m| !m.partial).collect();
        assert!(!finalized.is_empty(), "expected a finalized ScopeResultsMsg");
        let fin = &finalized[0];
        assert_eq!(fin.query_id, 1);
        // Both entries should appear in the finalized snapshot.
        let ids: Vec<&str> = fin.entries.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"a"), "entry 'a' missing from finalized: {ids:?}");
        assert!(ids.contains(&"b"), "entry 'b' missing from finalized: {ids:?}");
        assert!(fin.error.is_none(), "finalized should have no error: {:?}", fin.error);
    }

    // ── Test 2: hard floor fires when no plugin responds in time ─────────────

    /// One plugin takes 5 seconds (effectively never in test time).
    /// hard_floor=200ms; expect an empty partial emitted within that window,
    /// followed eventually by a finalized message when we advance time fully.
    ///
    /// Uses `tokio::time::pause()` + `advance()`. The timing future is spawned
    /// as a separate task so the test can drive time while it's running.
    #[tokio::test]
    async fn hard_floor_emits_empty_partial_when_nobody_responds() {
        tokio::time::pause();

        let cfg = ScopedSearchConfig {
            partial_deadline: Duration::from_millis(500),
            hard_floor:       Duration::from_millis(200),
        };

        let slow_handle: JoinHandle<Result<Vec<MediaEntry>, PluginCallError>> =
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(vec![])
            });

        let (tx, rx) = make_event_channel(32);

        // Spawn the timing future as an independent task so the test can
        // advance time while it awaits. With `pause()`, `advance()` yields
        // to other tasks before moving the clock.
        let timing_task = tokio::spawn(run_scope_timing(
            vec![slow_handle],
            2,
            SearchScope::Artist,
            cfg,
            None,
            tx.clone(),
        ));

        // Advance past hard floor (200ms) — spawned tasks run during this yield.
        tokio::time::advance(Duration::from_millis(300)).await;

        // Now advance past the slow plugin's 5-second sleep.
        tokio::time::advance(Duration::from_secs(5)).await;

        // Let the timing task complete.
        timing_task.await.expect("timing task should not panic");

        drop(tx);
        let mut rx_drain = rx;
        rx_drain.close();
        let mut msgs: Vec<ScopeResultsMsg> = Vec::new();
        while let Ok(line) = rx_drain.try_recv() {
            if let Ok(Event::ScopeResults(m)) = serde_json::from_str(line.trim()) {
                msgs.push(m);
            }
        }

        // Should have a partial (from hard floor) and a finalized.
        let partials: Vec<_> = msgs.iter().filter(|m| m.partial).collect();
        let finalized: Vec<_> = msgs.iter().filter(|m| !m.partial).collect();
        assert!(!partials.is_empty(), "expected an empty partial from hard floor; got: {msgs:?}");
        assert!(partials[0].entries.is_empty(), "hard-floor partial must be empty");
        assert!(!finalized.is_empty(), "expected a finalized message after slow plugin");
    }

    // ── Test 3: no plugins configured → immediate NoPluginsConfigured ────────

    /// When `handles` is empty (passed via `run_scope_timing` with an empty
    /// vec), expect an immediate finalized message with
    /// `error=NoPluginsConfigured`.
    ///
    /// In production this is guarded by `run_one_scope`'s early-return before
    /// even calling `run_scope_timing`. We test the defensive path directly.
    #[tokio::test]
    async fn scope_with_no_plugins_emits_no_plugins_configured() {
        let cfg = ScopedSearchConfig::default();
        let (tx, mut rx) = make_event_channel(8);

        run_scope_timing(
            vec![], // no plugins
            3,
            SearchScope::Movie,
            cfg,
            None,
            tx,
        )
        .await;

        rx.close();

        let mut msgs: Vec<ScopeResultsMsg> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            if let Ok(Event::ScopeResults(m)) = serde_json::from_str(line.trim()) {
                msgs.push(m);
            }
        }

        assert_eq!(msgs.len(), 1, "expected exactly one message");
        let msg = &msgs[0];
        assert!(!msg.partial, "should be finalized (partial=false)");
        assert_eq!(msg.error, Some(ScopeError::NoPluginsConfigured));
        assert!(msg.entries.is_empty());
    }

    // ── Test 3b: run_one_scope early return for no plugins ────────────────────

    /// End-to-end via `run_one_scope` with a fresh Engine (no plugins loaded).
    /// Must emit exactly one `NoPluginsConfigured` finalized message.
    #[tokio::test]
    async fn run_one_scope_no_plugins_emits_no_plugins_configured() {
        let engine = Engine::new(
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            0.4,
        );
        let cfg = ScopedSearchConfig::default();
        let (tx, mut rx) = make_event_channel(8);

        run_one_scope(engine, "test".into(), SearchScope::Series, 99, cfg, tx.clone()).await;
        drop(tx);
        rx.close();

        let mut msgs: Vec<ScopeResultsMsg> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            if let Ok(Event::ScopeResults(m)) = serde_json::from_str(line.trim()) {
                msgs.push(m);
            }
        }

        assert_eq!(msgs.len(), 1);
        assert!(!msgs[0].partial);
        assert_eq!(msgs[0].error, Some(ScopeError::NoPluginsConfigured));
    }

    // ── Test 4: slow scope does not block fast scope ───────────────────────────

    /// Two scopes: Artist (fast plugin, 10ms) and Track (slow plugin, 800ms).
    /// Both are dispatched via `search_scoped` concurrently.
    /// The Artist finalized message must arrive before the Track finalized one.
    #[tokio::test]
    async fn slow_scope_does_not_block_fast_scope() {
        tokio::time::pause();

        let cfg = ScopedSearchConfig {
            partial_deadline: Duration::from_millis(50),
            hard_floor:       Duration::from_millis(2000),
        };

        // Build Artist scope handles (fast).
        let fast_entry = make_entry("artist-fast");
        let fast_handle: JoinHandle<Result<Vec<MediaEntry>, PluginCallError>> =
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(vec![fast_entry])
            });

        // Build Track scope handles (slow).
        let slow_handle: JoinHandle<Result<Vec<MediaEntry>, PluginCallError>> =
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(800)).await;
                Ok(vec![])
            });

        let (tx, rx) = make_event_channel(64);

        // Spawn both scope timing tasks concurrently.
        let tx_artist = tx.clone();
        let tx_track = tx.clone();
        let artist_task = tokio::spawn(run_scope_timing(
            vec![fast_handle],
            4,
            SearchScope::Artist,
            cfg,
            None,
            tx_artist,
        ));
        let track_task = tokio::spawn(run_scope_timing(
            vec![slow_handle],
            4,
            SearchScope::Track,
            cfg,
            None,
            tx_track,
        ));

        // Advance time: past fast plugin + partial deadline but before slow plugin.
        tokio::time::advance(Duration::from_millis(100)).await;
        // Now advance past slow plugin's delay.
        tokio::time::advance(Duration::from_millis(900)).await;

        let _ = tokio::join!(artist_task, track_task);
        drop(tx);

        let mut rx_drain = rx;
        rx_drain.close();
        let mut msgs_with_order: Vec<(usize, ScopeResultsMsg)> = Vec::new();
        let mut i = 0;
        while let Ok(line) = rx_drain.try_recv() {
            if let Ok(Event::ScopeResults(m)) = serde_json::from_str(line.trim()) {
                msgs_with_order.push((i, m));
                i += 1;
            }
        }

        // Find the index of the finalized Artist message and finalized Track message.
        let artist_final_idx = msgs_with_order
            .iter()
            .position(|(_, m)| m.scope == SearchScope::Artist && !m.partial);
        let track_final_idx = msgs_with_order
            .iter()
            .position(|(_, m)| m.scope == SearchScope::Track && !m.partial);

        assert!(artist_final_idx.is_some(), "no Artist finalized message");
        assert!(track_final_idx.is_some(), "no Track finalized message");
        assert!(
            artist_final_idx.unwrap() < track_final_idx.unwrap(),
            "Artist finalized (idx={}) should precede Track finalized (idx={})",
            artist_final_idx.unwrap(),
            track_final_idx.unwrap(),
        );
    }

    // ── Test 5: all plugins fail → AllFailed error ────────────────────────────

    /// Both plugins return errors. Finalized message should have
    /// `error=Some(AllFailed)` and empty entries.
    #[tokio::test]
    async fn all_plugins_fail_produces_all_failed_error() {
        tokio::time::pause();

        let cfg = ScopedSearchConfig {
            partial_deadline: Duration::from_millis(50),
            hard_floor:       Duration::from_millis(500),
        };

        let h1: JoinHandle<Result<Vec<MediaEntry>, PluginCallError>> =
            tokio::spawn(async { Err(PluginCallError::Timeout) });
        let h2: JoinHandle<Result<Vec<MediaEntry>, PluginCallError>> =
            tokio::spawn(async { Err(PluginCallError::Other("crash".into())) });

        let (tx, rx) = make_event_channel(16);

        let timing_fut = run_scope_timing(
            vec![h1, h2],
            5,
            SearchScope::Album,
            cfg,
            None,
            tx.clone(),
        );
        tokio::pin!(timing_fut);
        tokio::time::advance(Duration::from_millis(200)).await;
        timing_fut.await;

        drop(tx);
        let mut rx_drain = rx;
        rx_drain.close();
        let mut msgs: Vec<ScopeResultsMsg> = Vec::new();
        while let Ok(line) = rx_drain.try_recv() {
            if let Ok(Event::ScopeResults(m)) = serde_json::from_str(line.trim()) {
                msgs.push(m);
            }
        }

        let finalized: Vec<_> = msgs.iter().filter(|m| !m.partial).collect();
        assert!(!finalized.is_empty(), "expected a finalized message");
        assert_eq!(
            finalized[0].error,
            Some(ScopeError::AllFailed),
            "all-fail should set AllFailed error"
        );
        assert!(finalized[0].entries.is_empty());
    }

    // ── dedup_key precedence tests ────────────────────────────────────────────

    #[test]
    fn dedup_key_prefers_mal_over_imdb_when_both_present() {
        let mut e = make_entry("anilist-16498");
        e.title = "AoT".into();
        e.year = Some("2013".into());
        e.mal_id = Some("16498".into());
        e.imdb_id = Some("tt2560140".into());
        assert_eq!(dedup_key(&e), "mal:16498");
    }

    #[test]
    fn dedup_key_falls_back_to_imdb_when_mal_missing() {
        let mut e = make_entry("omdb-tt1375666");
        e.title = "Inception".into();
        e.year = Some("2010".into());
        e.imdb_id = Some("tt1375666".into());
        assert_eq!(dedup_key(&e), "imdb:tt1375666");
    }

    #[test]
    fn dedup_key_falls_back_to_title_year_when_no_foreign_id() {
        let mut e = make_entry("tmdb-27205");
        e.title = "Inception".into();
        e.year = Some("2010".into());
        let k = dedup_key(&e);
        assert!(k.starts_with("title:"), "expected title-keyed, got {k}");
        assert!(k.contains("2010"), "expected year in key, got {k}");
    }

    #[test]
    fn dedup_key_treats_empty_mal_as_missing() {
        let mut e = make_entry("anilist-X");
        e.title = "X".into();
        e.year = Some("2020".into());
        e.mal_id = Some("".into());
        e.imdb_id = Some("tt0".into());
        // Empty MAL → falls through to imdb.
        assert_eq!(dedup_key(&e), "imdb:tt0");
    }

    // ── merge_dedupe collapse tests ───────────────────────────────────────────

    #[test]
    fn merge_dedupe_collapses_anilist_kitsu_via_mal() {
        let mut anilist = make_entry("anilist-16498");
        anilist.provider = "anilist".into();
        anilist.title = "Attack on Titan".into();
        anilist.year = Some("2013".into());
        anilist.mal_id = Some("16498".into());

        let mut kitsu = make_entry("kitsu-7442");
        kitsu.provider = "kitsu".into();
        kitsu.title = "Shingeki no Kyojin".into();
        kitsu.year = Some("2013".into());
        kitsu.mal_id = Some("16498".into());

        let merged = merge_dedupe(vec![anilist, kitsu]);
        assert_eq!(merged.len(), 1, "AniList and Kitsu with same MAL should collapse");
        // AniList wins by priority (3 < 4).
        assert_eq!(merged[0].provider, "anilist");
        // English title preserved (AniList's), not romaji (Kitsu's).
        assert_eq!(merged[0].title, "Attack on Titan");
    }

    #[test]
    fn merge_dedupe_collapses_tvdb_omdb_via_imdb() {
        let mut tvdb = make_entry("tvdb-81189");
        tvdb.provider = "tvdb".into();
        tvdb.title = "Breaking Bad".into();
        tvdb.year = Some("2008".into());
        tvdb.imdb_id = Some("tt0903747".into());

        let mut omdb = make_entry("omdb-tt0903747");
        omdb.provider = "omdb".into();
        omdb.title = "Breaking Bad: The Series".into();
        omdb.year = Some("2008".into());
        omdb.imdb_id = Some("tt0903747".into());

        let merged = merge_dedupe(vec![tvdb, omdb]);
        assert_eq!(merged.len(), 1, "TVDB and OMDb with same imdb should collapse");
        // TVDB wins by priority (1 < 2).
        assert_eq!(merged[0].provider, "tvdb");
        assert_eq!(merged[0].title, "Breaking Bad");
    }

    #[test]
    fn merge_dedupe_collapses_anilist_omdb_via_bridge() {
        use crate::anime_bridge::enrich::enrich_entry;
        use crate::anime_bridge::AnimeBridge;

        let mut anilist = make_entry("anilist-1");
        anilist.provider = "anilist".into();
        anilist.title = "Cowboy Bebop".into();
        anilist.year = Some("1998".into());
        anilist.mal_id = Some("1".into());

        let mut omdb = make_entry("omdb-tt0213338");
        omdb.provider = "omdb".into();
        omdb.title = "Cowboy Bebop".into();
        omdb.year = Some("1998".into());
        omdb.imdb_id = Some("tt0213338".into());

        // Bridge enrichment runs in production via the engine; replicate
        // here so the test exercises the full β collapse behaviour.
        let bridge = AnimeBridge::new();
        enrich_entry(&mut anilist, &bridge);
        enrich_entry(&mut omdb, &bridge);

        let merged = merge_dedupe(vec![anilist, omdb]);
        assert_eq!(
            merged.len(),
            1,
            "AniList (mal-keyed) and OMDb (imdb-keyed) should collapse via the bridge in milestone β",
        );
        // AniList wins as spine on mal-keyed merges per provider_priority_for_key.
        assert_eq!(merged[0].provider, "anilist");
        assert_eq!(merged[0].title, "Cowboy Bebop"); // AniList's title
    }

    #[test]
    fn mal_keyed_merge_picks_anilist_over_tvdb_as_spine() {
        use crate::anime_bridge::enrich::enrich_entry;
        use crate::anime_bridge::AnimeBridge;

        // Use Cowboy Bebop (mal=1) — it has a clean 1:1 cross-mapping
        // in the bundled Fribb snapshot. AOT (mal=16498) shares imdb
        // tt2560140 across 6 season records, so a TVDB→imdb→mal lookup
        // resolves to whichever season HashMap-insert last wrote
        // (non-deterministic mal_id), which would defeat the bridge
        // collapse in this test. Bebop is single-season → unambiguous.
        let mut anilist = make_entry("anilist-1");
        anilist.provider = "anilist".into();
        anilist.title = "Cowboy Bebop".into();
        anilist.year = Some("1998".into());
        anilist.mal_id = Some("1".into());

        let mut tvdb = make_entry("tvdb-76885");
        tvdb.provider = "tvdb".into();
        tvdb.title = "Cowboy Bebop".into();
        tvdb.year = Some("1998".into());
        tvdb.imdb_id = Some("tt0213338".into());

        let bridge = AnimeBridge::new();
        enrich_entry(&mut anilist, &bridge);
        enrich_entry(&mut tvdb, &bridge);

        let merged = merge_dedupe(vec![anilist, tvdb]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].provider, "anilist");
    }

    #[test]
    fn imdb_keyed_merge_keeps_tvdb_over_anilist_as_spine() {
        // Western series — neither entry has mal_id; bridge enrichment is
        // a no-op; merge keys on imdb. TVDB still wins (existing α behaviour).
        let mut tvdb = make_entry("tvdb-81189");
        tvdb.provider = "tvdb".into();
        tvdb.title = "Breaking Bad".into();
        tvdb.year = Some("2008".into());
        tvdb.imdb_id = Some("tt0903747".into());

        let mut anilist = make_entry("anilist-X");
        anilist.provider = "anilist".into();
        anilist.title = "Breaking Bad".into();
        anilist.year = Some("2008".into());
        anilist.imdb_id = Some("tt0903747".into());

        let merged = merge_dedupe(vec![tvdb, anilist]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].provider, "tvdb"); // imdb-keyed → existing priority
    }

    #[test]
    fn merge_dedupe_field_fold_fills_holes_from_secondary() {
        let mut anilist = make_entry("anilist-100");
        anilist.provider = "anilist".into();
        anilist.title = "X".into();
        anilist.year = Some("2020".into());
        anilist.mal_id = Some("100".into());
        anilist.description = None; // missing on spine

        let mut kitsu = make_entry("kitsu-100");
        kitsu.provider = "kitsu".into();
        kitsu.title = "X".into();
        kitsu.year = Some("2020".into());
        kitsu.mal_id = Some("100".into());
        kitsu.description = Some("Filled by secondary".into());

        let merged = merge_dedupe(vec![anilist, kitsu]);
        assert_eq!(merged.len(), 1);
        // AniList is the spine, but the description hole filled from Kitsu.
        assert_eq!(merged[0].provider, "anilist");
        assert_eq!(merged[0].description.as_deref(), Some("Filled by secondary"));
    }
}
