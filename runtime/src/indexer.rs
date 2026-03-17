/// Indexer — in-memory content cache with LRU eviction.
///
/// Results from provider plugins are cached here so that:
///   - Repeated identical searches are served instantly
///   - The Go TUI can paginate without re-hitting the provider
///   - Plugin results survive short disconnects / redraws
use std::sync::Arc;

use lru::LruCache;
use std::num::NonZeroUsize;
use tokio::sync::Mutex;

use crate::ipc::{MediaEntry, MediaTab};

// ── Cache key ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub provider: String,
    pub tab: String,
    pub query: String,
}

impl CacheKey {
    pub fn new(provider: &str, tab: &MediaTab, query: &str) -> Self {
        Self {
            provider: provider.to_string(),
            tab: format!("{tab:?}"),
            query: query.to_lowercase().trim().to_string(),
        }
    }
}

// ── Cache entry ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub items: Vec<MediaEntry>,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

impl CacheEntry {
    pub fn new(items: Vec<MediaEntry>) -> Self {
        Self {
            items,
            fetched_at: chrono::Utc::now(),
        }
    }

    /// Returns true if this entry is older than `ttl_secs` seconds.
    pub fn is_stale(&self, ttl_secs: i64) -> bool {
        let age = chrono::Utc::now() - self.fetched_at;
        age.num_seconds() > ttl_secs
    }
}

// ── Indexer ───────────────────────────────────────────────────────────────────

const DEFAULT_CAPACITY: usize = 512; // max cached query results
const DEFAULT_TTL_SECS: i64 = 300;   // 5 minutes

#[derive(Clone)]
pub struct Indexer {
    cache: Arc<Mutex<LruCache<CacheKey, CacheEntry>>>,
    ttl_secs: i64,
}

impl Indexer {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(cap: usize) -> Self {
        let capacity = NonZeroUsize::new(cap).expect("capacity must be > 0");
        Self {
            cache: Arc::new(Mutex::new(LruCache::new(capacity))),
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }

    /// Retrieve cached results for a key, if present and not stale.
    pub async fn get(&self, key: &CacheKey) -> Option<Vec<MediaEntry>> {
        let mut cache = self.cache.lock().await;
        if let Some(entry) = cache.get(key) {
            if !entry.is_stale(self.ttl_secs) {
                return Some(entry.items.clone());
            }
        }
        None
    }

    /// Store results in the cache.
    pub async fn put(&self, key: CacheKey, items: Vec<MediaEntry>) {
        let mut cache = self.cache.lock().await;
        cache.put(key, CacheEntry::new(items));
    }

    /// Invalidate all cached entries for a specific provider.
    pub async fn invalidate_provider(&self, provider: &str) {
        let mut cache = self.cache.lock().await;
        let stale_keys: Vec<CacheKey> = cache
            .iter()
            .filter(|(k, _)| k.provider == provider)
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale_keys {
            cache.pop(&key);
        }
    }

    /// Flush the entire cache.
    pub async fn flush(&self) {
        let mut cache = self.cache.lock().await;
        cache.clear();
    }

    /// Return the number of entries currently in the cache.
    pub async fn len(&self) -> usize {
        self.cache.lock().await.len()
    }
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new()
    }
}
