//! Search pipeline — fan-out to engine plugins via scoped streaming.

use crate::engine::{Engine, search_scoped, ScopedSearchConfig};
use crate::ipc::SearchRequest;
use crate::ipc::v1::stream::EventSender;
use tracing::Instrument;

/// Drive a scoped search: delegates to `search_scoped`, which emits per-scope
/// `ScopeResultsMsg` events over `event_tx`.  No synchronous return — results
/// flow back as streaming events keyed by `query_id`.
///
/// # Streaming protocol
///
/// Each scope that completes (or times out at the partial deadline) emits one
/// `Event::ScopeResults` message on `event_tx`.  The Go TUI (Task 4.3) listens
/// for those events and feeds them into the search result view.
///
/// # Cache
///
/// Per-plugin result caching is deferred to a follow-up task (Chunk 7 /
/// Task 2.9 follow-up).  The cache stores `Vec<CatalogEntry>` while
/// `supervisor_search` returns `Vec<MediaEntry>`; adapting that boundary is
/// non-trivial and is out of scope here.
///
/// The legacy `Engine::search` synchronous path (used by `catalog.rs` and
/// `engine/pipeline.rs`) was retired in Task 7.0 #3.  This streaming path
/// is now the sole search entry point for user-initiated queries.
pub async fn run_search(engine: Engine, req: SearchRequest, event_tx: EventSender) {
    let span = tracing::info_span!(
        "run_search",
        query_id = req.query_id,
        scopes   = ?req.scopes,
    );

    async {
        // TODO(Task 2.9 follow-up / Chunk 7): add per-plugin cache lookup here
        // once SearchCache is extended to store Vec<MediaEntry> or a conversion
        // helper is in place.  Tracking issue: cache stores Vec<CatalogEntry>
        // but supervisor_search returns Vec<MediaEntry>.

        let cfg = ScopedSearchConfig::default();
        search_scoped(engine, req.query, req.scopes, req.query_id, cfg, event_tx).await;
    }
    .instrument(span)
    .await;
}
