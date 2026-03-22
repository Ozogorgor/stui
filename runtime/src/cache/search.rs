//! Search result cache — TTL 2 hours.
//!
//! Key: `(tab_name, normalised_query, page)`
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

use tokio::sync::RwLock;
use tracing::debug;

use crate::cache::Ttl;
use crate::catalog::CatalogEntry;

#[allow(dead_code)]
const TTL: Duration = Duration::from_secs(2 * 60 * 60);

/// Cache key for a search request.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchKey {
    pub tab:   String,
    pub query: String,
    pub page:  u32,
}

impl SearchKey {
    #[allow(dead_code)]
    pub fn new(tab: impl Into<String>, query: &str, page: u32) -> Self {
        SearchKey {
            tab:   tab.into(),
            query: normalise(query),
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
pub struct SearchCache {
    inner: Arc<RwLock<HashMap<SearchKey, Ttl<Vec<CatalogEntry>>>>>,
}

impl SearchCache {
    pub fn new() -> Self {
        SearchCache { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    #[allow(dead_code)]
    /// Try to retrieve cached results for this key.
    /// Returns `None` if the key is absent or the entry has expired.
    pub async fn get(&self, key: &SearchKey) -> Option<Vec<CatalogEntry>> {
        let map = self.inner.read().await;
        let entry = map.get(key)?;
        if entry.is_valid() {
            debug!("search cache HIT tab={} q={:?} page={}", key.tab, key.query, key.page);
            Some(entry.value.clone())
        } else {
            debug!("search cache EXPIRED tab={} q={:?}", key.tab, key.query);
            None
        }
    }

    #[allow(dead_code)]
    /// Store results for this key, replacing any existing entry.
    pub async fn insert(&self, key: SearchKey, items: Vec<CatalogEntry>) {
        debug!("search cache INSERT tab={} q={:?} n={}", key.tab, key.query, items.len());
        self.inner.write().await.insert(key, Ttl::new(items, TTL));
    }

    #[allow(dead_code)]
    /// Evict all expired entries. Call periodically to reclaim memory.
    pub async fn evict_expired(&self) {
        let mut map = self.inner.write().await;
        map.retain(|_, v| v.is_valid());
    }

    #[allow(dead_code)]
    /// Drop everything — useful on plugin reload when data may be stale.
    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

impl Default for SearchCache {
    fn default() -> Self { Self::new() }
}
