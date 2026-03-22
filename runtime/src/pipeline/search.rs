//! Search pipeline — fan-out to engine plugins, fallback to catalog grid filter.

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::engine::Engine;
use crate::ipc::{self, MediaEntry, Response, SearchRequest, SearchResponse};

/// Handle a `search` IPC request.
///
/// Tries engine plugins first; if they return no results, falls back to
/// a local full-text filter over the catalog grid.
pub async fn run_search(engine: &Arc<Engine>, catalog: &Arc<Catalog>, r: SearchRequest) -> Response {
    let results = engine.search(
        &r.id,
        &r.query,
        &r.tab,
        r.provider.as_deref(),
        r.limit.unwrap_or(50),
        r.offset.unwrap_or(0),
    ).await;

    if let Response::SearchResult(ref sr) = results {
        if sr.items.is_empty() {
            return catalog_search(catalog, &r.id, &r.query, &r.tab).await;
        }
    }
    results
}

/// Filter the in-memory catalog grid by a query string.
///
/// Used as a zero-latency fallback when no plugin claims the search.
pub async fn catalog_search(
    catalog: &Arc<Catalog>,
    req_id: &str,
    query: &str,
    tab: &ipc::MediaTab,
) -> Response {
    let grid = catalog.get_grid(tab).await;
    let q = query.to_lowercase();
    let matched: Vec<MediaEntry> = grid.into_iter()
        .filter(|e| e.title.to_lowercase().contains(&q))
        .map(|e| MediaEntry {
            id: e.id, title: e.title, year: e.year,
            genre: e.genre, rating: e.rating,
            description: e.description, poster_url: e.poster_url,
            provider: e.provider, tab: tab.clone(),
            media_type: e.media_type,
            ratings: std::collections::HashMap::new(),
        })
        .collect();
    let total = matched.len();
    Response::SearchResult(SearchResponse { id: req_id.to_string(), items: matched, total, offset: 0 })
}
