//! Search result cache — TTL 2 hours.
//!
//! Key: `(plugin_id, normalised_query, scope, page)`
//! Value: `Vec<CatalogEntry>`
//!
//! Normalisation: lowercase + trim + collapse whitespace.
//! This makes "Dune " and "dune" hit the same cache slot.
//!
//! Provider catalogs update slowly — a 2-hour window covers a full browsing
//! session without ever re-fetching the same query twice.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use stui_plugin_sdk::SearchScope;
use tokio::sync::RwLock;
use tracing::debug;

use crate::cache::Ttl;
use crate::catalog::CatalogEntry;

#[allow(dead_code)] // pub API: used by engine search result cache
const TTL: Duration = Duration::from_secs(2 * 60 * 60);

/// Cache key for a search request.
#[allow(dead_code)] // pub API: used by engine search result cache
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchKey {
    plugin_id:  String,
    query_norm: String,
    scope:      SearchScope,
    page:       u32,
}

impl SearchKey {
    #[allow(dead_code)] // pub API: used by engine search result cache
    pub fn new(plugin_id: &str, query: &str, scope: SearchScope, page: u32) -> Self {
        SearchKey {
            plugin_id:  plugin_id.to_string(),
            query_norm: normalise(query),
            scope,
            page,
        }
    }
}

fn normalise(q: &str) -> String {
    q.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Thread-safe search result cache.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct SearchCache {
    inner: Arc<RwLock<HashMap<SearchKey, Ttl<Vec<CatalogEntry>>>>>,
}

impl SearchCache {
    pub fn new() -> Self {
        SearchCache { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    #[allow(dead_code)] // pub API: used by engine search result cache
    /// Try to retrieve cached results for this key.
    /// Returns `None` if the key is absent or the entry has expired.
    pub async fn get(&self, key: &SearchKey) -> Option<Vec<CatalogEntry>> {
        let map = self.inner.read().await;
        let entry = map.get(key)?;
        if entry.is_valid() {
            debug!("search cache HIT plugin={} scope={:?} q={:?} page={}", key.plugin_id, key.scope, key.query_norm, key.page);
            Some(entry.value.clone())
        } else {
            debug!("search cache EXPIRED plugin={} scope={:?} q={:?}", key.plugin_id, key.scope, key.query_norm);
            None
        }
    }

    #[allow(dead_code)] // pub API: used by engine search result cache
    /// Store results for this key, replacing any existing entry.
    pub async fn insert(&self, key: SearchKey, items: Vec<CatalogEntry>) {
        debug!("search cache INSERT plugin={} scope={:?} q={:?} n={}", key.plugin_id, key.scope, key.query_norm, items.len());
        self.inner.write().await.insert(key, Ttl::new(items, TTL));
    }

    #[allow(dead_code)] // pub API: used by engine search result cache
    /// Evict all expired entries. Call periodically to reclaim memory.
    pub async fn evict_expired(&self) {
        let mut map = self.inner.write().await;
        map.retain(|_, v| v.is_valid());
    }

    #[allow(dead_code)] // pub API: used by engine search result cache
    /// Drop everything — useful on plugin reload when data may be stale.
    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

impl Default for SearchCache {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_includes_scope_and_plugin() {
        let k1 = SearchKey::new("discogs", "creep", SearchScope::Track, 0);
        let k2 = SearchKey::new("discogs", "creep", SearchScope::Artist, 0);
        assert_ne!(k1, k2, "same plugin+query but different scope → different key");
    }

    #[test]
    fn key_normalizes_query() {
        let k1 = SearchKey::new("discogs", "  Creep  ", SearchScope::Track, 0);
        let k2 = SearchKey::new("discogs", "creep", SearchScope::Track, 0);
        assert_eq!(k1, k2, "whitespace and case collapse to same key");
    }

    #[test]
    fn key_per_plugin_isolation() {
        let k1 = SearchKey::new("discogs", "creep", SearchScope::Track, 0);
        let k2 = SearchKey::new("lastfm", "creep", SearchScope::Track, 0);
        assert_ne!(k1, k2);
    }
}
