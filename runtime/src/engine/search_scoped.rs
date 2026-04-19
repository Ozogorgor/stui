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
    let _enter = span.enter();

    let plugins = engine.plugins_for_scope(scope).await;

    if plugins.is_empty() {
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
    let handles: Vec<JoinHandle<Result<Vec<MediaEntry>, PluginCallError>>> = plugins
        .iter()
        .map(|pid| {
            let pid = pid.clone();
            let q = query.clone();
            let engine = engine.clone();
            tokio::spawn(async move {
                engine.supervisor_search(&pid, &q, scope).await
            })
        })
        .collect();

    run_scope_timing(handles, query_id, scope, cfg, out).await;
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
                    collected.extend(entries);
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
                    entries: collected.clone(),
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
        entries: collected,
        partial: false,
        error,
    })).await;
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
            id:           id.into(),
            title:        id.into(),
            year:         None,
            genre:        None,
            rating:       None,
            description:  None,
            poster_url:   None,
            provider:     "test".into(),
            tab:          MediaTab::Music,
            media_type:   MediaType::default(),
            ratings:      std::collections::HashMap::new(),
            imdb_id:      None,
            tmdb_id:      None,
            kind:         Default::default(),
            source:       "test".into(),
            artist_name:  None,
            album_name:   None,
            track_number: None,
            season:       None,
            episode:      None,
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
            tx_artist,
        ));
        let track_task = tokio::spawn(run_scope_timing(
            vec![slow_handle],
            4,
            SearchScope::Track,
            cfg,
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
}
