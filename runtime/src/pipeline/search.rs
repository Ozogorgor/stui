//! Search pipeline — fan-out to engine plugins, fallback to catalog grid filter.

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::catalog_engine::SortOrder;
use crate::config::ConfigManager;
use crate::engine::{Engine, SearchOptions, TraceEmitter};
use crate::ipc::{self, MediaEntry, Response, SearchRequest, SearchResponse};

/// Handle a `search` IPC request.
///
/// Tries engine plugins first; if they return no results, falls back to
/// a local full-text filter over the catalog grid.
pub async fn run_search(
    engine: &Arc<Engine>,
    catalog: &Arc<Catalog>,
    trace: &Arc<TraceEmitter>,
    config: &Arc<ConfigManager>,
    r: SearchRequest,
) -> Response {
    let t0 = std::time::Instant::now();

    let adult_content_enabled = config.snapshot().await.adult_content_enabled;

    let sort = match r.sort.as_deref() {
        Some("newest")       => SortOrder::Newest,
        Some("oldest")       => SortOrder::Oldest,
        Some("alphabetical") => SortOrder::Alphabetical,
        Some("relevance")    => SortOrder::Relevance,
        _                    => SortOrder::Rating,
    };
    let options = SearchOptions {
        sort,
        genre:      r.genre.clone(),
        min_rating: r.min_rating,
        year_from:  r.year_from,
        year_to:    r.year_to,
        adult_content_enabled,
    };

    let results = engine.search(
        &r.id,
        &r.query,
        &r.tab,
        r.provider.as_deref(),
        r.limit.unwrap_or(50),
        r.offset.unwrap_or(0),
        options,
    ).await;

    let elapsed_ms = t0.elapsed().as_millis() as u64;

    if let Response::SearchResult(ref sr) = results {
        if sr.items.is_empty() {
            let fallback = catalog_search(catalog, &r.id, &r.query, &r.tab).await;
            if let Response::SearchResult(ref fr) = fallback {
                trace.search(0, elapsed_ms);
                trace.resolve(fr.items.len());
            }
            return fallback;
        }
        // Count distinct providers that returned results
        let n_providers = {
            use std::collections::HashSet;
            sr.items.iter().map(|e| e.provider.as_str()).collect::<HashSet<_>>().len()
        };
        trace.search(n_providers, elapsed_ms);
        trace.resolve(sr.items.len());
    } else {
        trace.fallback("search error");
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
            imdb_id: None,
            tmdb_id: None,
        })
        .collect();
    let total = matched.len();
    Response::SearchResult(SearchResponse { id: req_id.to_string(), items: matched, total, offset: 0 })
}
