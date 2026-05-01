//! Per-verb metadata cache.
//!
//! Keys are `MetadataCacheKey { verb, id_source, id }`. Each verb has its
//! own TTL:
//!   Credits / Artwork: 30 days
//!   Enrich:             7 days
//!   Related:            3 days
//!
//! Values are the real wire-typed [`MetadataPayload`] defined in
//! `ipc::v1::metadata` — cache and IPC share one payload shape so the
//! runtime never has to convert between cache-internal and wire forms.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::debug;

use crate::cache::metadata_key::{MetadataCacheKey, MetadataVerb};
use crate::cache::Ttl;

pub use crate::ipc::v1::MetadataPayload;

pub const CREDITS_TTL: Duration = Duration::from_secs(30 * 86_400);
pub const ARTWORK_TTL: Duration = Duration::from_secs(30 * 86_400);
pub const ENRICH_TTL:  Duration = Duration::from_secs( 7 * 86_400);
pub const RELATED_TTL: Duration = Duration::from_secs( 3 * 86_400);
pub const RATINGS_AGGREGATOR_TTL: Duration = Duration::from_secs(86_400);

fn ttl_for(verb: MetadataVerb) -> Duration {
    match verb {
        MetadataVerb::Credits => CREDITS_TTL,
        MetadataVerb::Artwork => ARTWORK_TTL,
        MetadataVerb::Enrich  => ENRICH_TTL,
        MetadataVerb::Related => RELATED_TTL,
        MetadataVerb::RatingsAggregator => RATINGS_AGGREGATOR_TTL,
    }
}

/// TTL for an empty / failed-fan-out result. Short enough that a
/// transient upstream outage (TMDB rate-limiting, TVDB blip) recovers
/// quickly on the user's next detail open, long enough that rapid
/// re-opens during the outage don't re-hammer the source. Without
/// this, an empty result wasn't cached at all — every re-open
/// re-fired the full fan-out and re-hit the throttled provider.
const NEGATIVE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct MetadataCache {
    inner: Arc<RwLock<HashMap<MetadataCacheKey, Ttl<MetadataPayload>>>>,
    override_ttl: Option<Duration>,
}

impl MetadataCache {
    pub fn new() -> Self {
        MetadataCache {
            inner: Arc::new(RwLock::new(HashMap::new())),
            override_ttl: None,
        }
    }

    /// Constructor for tests where the standard TTLs (days) are impractical.
    #[cfg(test)]
    pub fn with_custom_ttl(ttl: Duration) -> Self {
        MetadataCache {
            inner: Arc::new(RwLock::new(HashMap::new())),
            override_ttl: Some(ttl),
        }
    }

    pub async fn get(&self, key: &MetadataCacheKey) -> Option<MetadataPayload> {
        let map = self.inner.read().await;
        let entry = map.get(key)?;
        if entry.is_valid() {
            debug!(verb = ?key.verb, id = %key.id, "metadata cache HIT");
            Some(entry.value.clone())
        } else {
            debug!(verb = ?key.verb, id = %key.id, "metadata cache EXPIRED");
            None
        }
    }

    pub async fn insert(&self, key: MetadataCacheKey, payload: MetadataPayload) {
        let ttl = self.override_ttl.unwrap_or_else(|| ttl_for(key.verb));
        debug!(verb = ?key.verb, id = %key.id, "metadata cache INSERT");
        self.inner.write().await.insert(key, Ttl::new(payload, ttl));
    }

    /// Cache a placeholder result (typically `MetadataPayload::Empty`)
    /// with a short TTL. Used when the fan-out timed out / errored on
    /// every source, so we don't keep beating on a throttled upstream
    /// every time the user re-opens the detail.
    pub async fn insert_negative(&self, key: MetadataCacheKey, payload: MetadataPayload) {
        let ttl = self.override_ttl.unwrap_or(NEGATIVE_TTL);
        debug!(verb = ?key.verb, id = %key.id, "metadata cache INSERT (negative)");
        self.inner.write().await.insert(key, Ttl::new(payload, ttl));
    }

    pub async fn evict_expired(&self) {
        self.inner.write().await.retain(|_, v| v.is_valid());
    }

    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

impl Default for MetadataCache {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::metadata_key::IdSource;
    use std::time::Duration;

    fn k(verb: MetadataVerb, id: &str) -> MetadataCacheKey {
        MetadataCacheKey { verb, id_source: IdSource::Imdb, id: id.into() }
    }

    #[tokio::test]
    async fn credits_insert_get_round_trip() {
        let c = MetadataCache::new();
        let payload = MetadataPayload::Empty;
        c.insert(k(MetadataVerb::Credits, "tt1"), payload.clone()).await;
        assert_eq!(c.get(&k(MetadataVerb::Credits, "tt1")).await, Some(payload));
    }

    #[tokio::test]
    async fn distinct_verbs_distinct_slots() {
        let c = MetadataCache::new();
        c.insert(k(MetadataVerb::Credits, "tt1"), MetadataPayload::Empty).await;
        c.insert(k(MetadataVerb::Artwork, "tt1"), MetadataPayload::Empty).await;
        c.evict_expired().await;
        assert!(c.get(&k(MetadataVerb::Credits, "tt1")).await.is_some());
        assert!(c.get(&k(MetadataVerb::Artwork, "tt1")).await.is_some());
    }

    #[tokio::test]
    async fn expired_entry_not_served() {
        let c = MetadataCache::with_custom_ttl(Duration::from_millis(1));
        c.insert(k(MetadataVerb::Enrich, "tt1"), MetadataPayload::Empty).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(c.get(&k(MetadataVerb::Enrich, "tt1")).await.is_none());
    }
}
