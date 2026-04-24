//! Search result cache — TTL 2 hours.
//!
//! Key: `(plugin_id, normalised_query, scope, page)`
//! Value: `Vec<MediaEntry>` (the pre-aggregation, per-provider shape the
//! engine fans in from plugin + TVDB tasks before `CatalogAggregator` merges
//! across sources)
//!
//! Normalisation: lowercase + trim + collapse whitespace.
//! This makes "Dune " and "dune" hit the same cache slot.
//!
//! Provider catalogs update slowly — a 2-hour window covers a full browsing
//! session without ever re-fetching the same query twice.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use stui_plugin_sdk::SearchScope;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::cache::{persistent::SqliteKv, Ttl};
use crate::ipc::MediaEntry;

const TTL: Duration = Duration::from_secs(2 * 60 * 60);
const DISK_NAMESPACE: &str = "search";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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

    /// Stable string representation for the on-disk SQLite cache. Format is
    /// `{plugin}|{query}|{scope}|{page}`; the pipe separator avoids ambiguity
    /// with plugin names / queries that contain colons, and `{:?}` on
    /// `SearchScope` is stable enough since the enum is unlikely to rename
    /// variants.
    pub fn disk_key(&self) -> String {
        format!(
            "{}|{}|{:?}|{}",
            self.plugin_id, self.query_norm, self.scope, self.page
        )
    }
}

fn normalise(q: &str) -> String {
    q.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Thread-safe search result cache with an optional on-disk persistence
/// tier. Mem layer serves hot results; disk layer (if configured) survives
/// daemon restarts. `get` warms mem on disk-hit; `insert` writes through to
/// both. `clear` drops mem only — leaves disk alone so an R-refresh within
/// TTL can still re-warm from the cheap disk read.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct SearchCache {
    inner: Arc<RwLock<HashMap<SearchKey, Ttl<Vec<MediaEntry>>>>>,
    disk: Option<Arc<SqliteKv>>,
}

impl SearchCache {
    pub fn new() -> Self {
        SearchCache { inner: Arc::new(RwLock::new(HashMap::new())), disk: None }
    }

    /// Same as `new()` but with an on-disk tier wired underneath. Disk reads
    /// are a fallback after mem miss; writes are write-through.
    pub fn with_disk(disk: Arc<SqliteKv>) -> Self {
        SearchCache { inner: Arc::new(RwLock::new(HashMap::new())), disk: Some(disk) }
    }

    /// Try to retrieve cached results for this key. Mem first, then disk.
    /// Returns `None` if neither layer has a fresh entry for the key.
    pub async fn get(&self, key: &SearchKey) -> Option<Vec<MediaEntry>> {
        {
            let map = self.inner.read().await;
            if let Some(entry) = map.get(key) {
                if entry.is_valid() {
                    debug!("search cache HIT (mem) plugin={} scope={:?} q={:?} page={}", key.plugin_id, key.scope, key.query_norm, key.page);
                    return Some(entry.value.clone());
                }
            }
        }
        // Disk fallback. On hit, warm mem so subsequent reads short-circuit
        // past the sqlite call; the TTL used for the warmed mem entry is
        // the same 2h window — if the disk row had different remaining TTL
        // we'd read stale data, but the disk tier already filtered expired
        // rows in SqliteKv::get, so any bytes we got back are fresh enough.
        if let Some(disk) = &self.disk {
            let bytes = disk.get(DISK_NAMESPACE, &key.disk_key())?;
            match serde_json::from_slice::<Vec<MediaEntry>>(&bytes) {
                Ok(entries) if !entries.is_empty() => {
                    debug!("search cache HIT (disk) plugin={} scope={:?} q={:?} page={} n={}", key.plugin_id, key.scope, key.query_norm, key.page, entries.len());
                    self.inner
                        .write()
                        .await
                        .insert(key.clone(), Ttl::new(entries.clone(), TTL));
                    return Some(entries);
                }
                Ok(_) => return None,
                Err(e) => {
                    warn!(err = %e, "search cache: disk row failed to deserialize — treating as miss");
                    return None;
                }
            }
        }
        debug!("search cache MISS plugin={} scope={:?} q={:?}", key.plugin_id, key.scope, key.query_norm);
        None
    }

    /// Store results for this key. Writes to mem and disk (when configured).
    /// Empty result lists are intentionally NOT cached — an empty response
    /// is often transient (API hiccup / rate-limit fallthrough) and we'd
    /// rather re-query than lock in a vacant cache entry for 2 hours.
    pub async fn insert(&self, key: SearchKey, items: Vec<MediaEntry>) {
        if items.is_empty() {
            debug!("search cache SKIP (empty) plugin={} scope={:?} q={:?}", key.plugin_id, key.scope, key.query_norm);
            return;
        }
        debug!("search cache INSERT plugin={} scope={:?} q={:?} n={}", key.plugin_id, key.scope, key.query_norm, items.len());
        self.inner.write().await.insert(key.clone(), Ttl::new(items.clone(), TTL));
        if let Some(disk) = &self.disk {
            match serde_json::to_vec(&items) {
                Ok(bytes) => {
                    let expires = now_secs() + TTL.as_secs();
                    if let Err(e) = disk.put(DISK_NAMESPACE, &key.disk_key(), &bytes, expires) {
                        warn!(err = %e, "search cache: disk write failed (mem cache intact)");
                    }
                }
                Err(e) => warn!(err = %e, "search cache: serialize failed (mem cache intact)"),
            }
        }
    }

    /// Evict all expired entries. Call periodically to reclaim memory.
    /// Disk purge is handled independently by the daemon's purge task.
    pub async fn evict_expired(&self) {
        let mut map = self.inner.write().await;
        map.retain(|_, v| v.is_valid());
    }

    /// Drop mem AND disk entries. Used when the caller really wants a full
    /// invalidation (plugin reload, user "clear cache" command, tests).
    #[allow(dead_code)] // pub API: for plugin-reload / admin-clear callers
    pub async fn clear_all(&self) {
        self.inner.write().await.clear();
        if let Some(disk) = &self.disk {
            let _ = disk.clear_namespace(DISK_NAMESPACE);
        }
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
