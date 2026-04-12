//! Metadata (detail page) cache — TTL 24 hours.
//!
//! Key: entry_id (IMDB id preferred, falls back to provider-internal id)
//! Value: `DetailEntry` — the enriched full-detail struct
//!
//! Metadata is expensive (multiple API calls for cast, credits, similar)
//! and changes very rarely — cast lists, genres, and descriptions are
//! essentially immutable once a title is released. 24 hours eliminates
//! redundant round-trips for any title revisited within a day.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::debug;

use crate::cache::Ttl;
use crate::ipc::DetailEntry;

#[allow(dead_code)] // pub API: used by engine metadata cache
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Thread-safe metadata / detail cache.
#[allow(dead_code)] // pub API: used by engine metadata cache
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct MetadataCache {
    inner: Arc<RwLock<HashMap<String, Ttl<DetailEntry>>>>,
}

impl MetadataCache {
    #[allow(dead_code)] // pub API: used by engine metadata cache
    pub fn new() -> Self {
        MetadataCache { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    #[allow(dead_code)] // pub API: used by engine metadata cache
    /// Look up a cached detail entry.
    pub async fn get(&self, id: &str) -> Option<DetailEntry> {
        let map = self.inner.read().await;
        let entry = map.get(id)?;
        if entry.is_valid() {
            debug!("metadata cache HIT id={}", id);
            Some(entry.value.clone())
        } else {
            debug!("metadata cache EXPIRED id={}", id);
            None
        }
    }

    #[allow(dead_code)] // pub API: used by engine metadata cache
    /// Store enriched detail for an entry.
    pub async fn insert(&self, id: impl Into<String>, detail: DetailEntry) {
        let key = id.into();
        debug!("metadata cache INSERT id={}", key);
        self.inner.write().await.insert(key, Ttl::new(detail, TTL));
    }

    #[allow(dead_code)] // pub API: used by engine metadata cache
    pub async fn evict_expired(&self) {
        self.inner.write().await.retain(|_, v| v.is_valid());
    }

    #[allow(dead_code)] // pub API: used by engine metadata cache
    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

impl Default for MetadataCache {
    fn default() -> Self { Self::new() }
}
