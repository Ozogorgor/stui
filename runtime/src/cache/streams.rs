//! Resolved stream cache — TTL 30 minutes.
//!
//! Key: entry_id
//! Value: `Vec<Stream>`
//!
//! Stream resolution hits indexers / scrapers that rate-limit aggressively.
//! 30 minutes is long enough to cover multiple plays / retries in a session
//! while short enough that magnet links remain fresh (trackers update peer
//! lists within this window) and stream URLs don't expire.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::debug;

use crate::cache::Ttl;
use crate::providers::Stream;

#[allow(dead_code)]
const TTL: Duration = Duration::from_secs(30 * 60);

/// Thread-safe resolved-stream cache.
#[allow(dead_code)]
#[derive(Clone)]
pub struct StreamCache {
    inner: Arc<RwLock<HashMap<String, Ttl<Vec<Stream>>>>>,
}

impl StreamCache {
    #[allow(dead_code)]
    pub fn new() -> Self {
        StreamCache { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    #[allow(dead_code)]
    /// Return cached streams if present and not expired.
    pub async fn get(&self, id: &str) -> Option<Vec<Stream>> {
        let map = self.inner.read().await;
        let entry = map.get(id)?;
        if entry.is_valid() {
            debug!("stream cache HIT id={}", id);
            Some(entry.value.clone())
        } else {
            debug!("stream cache EXPIRED id={}", id);
            None
        }
    }

    #[allow(dead_code)]
    /// Cache streams for an entry_id, replacing any previous value.
    pub async fn insert(&self, id: impl Into<String>, streams: Vec<Stream>) {
        let key = id.into();
        debug!("stream cache INSERT id={} n={}", key, streams.len());
        self.inner.write().await.insert(key, Ttl::new(streams, TTL));
    }

    #[allow(dead_code)]
    pub async fn evict_expired(&self) {
        self.inner.write().await.retain(|_, v| v.is_valid());
    }

    #[allow(dead_code)]
    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

impl Default for StreamCache {
    fn default() -> Self { Self::new() }
}
