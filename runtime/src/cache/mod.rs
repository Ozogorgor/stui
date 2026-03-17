//! Runtime cache layer.
//!
//! Three independent TTL caches backed by `moka` (a high-performance async
//! Rust cache modelled on Caffeine):
//!
//! | Cache             | Key                   | Value              | TTL   |
//! |-------------------|-----------------------|--------------------|-------|
//! | `SearchCache`     | (tab, query, page)    | Vec<CatalogEntry>  | 5 min |
//! | `MetadataCache`   | imdb_id / entry_id    | DetailEntry        | 1 hr  |
//! | `StreamCache`     | entry_id              | Vec<Stream>        | 10 min|
//!
//! All caches are cheap to clone (they share the underlying `Arc`).
//! The runtime holds a single `RuntimeCache` that groups all three.
//!
//! Usage:
//! ```rust
//! let cache = RuntimeCache::new();
//!
//! // Search cache
//! if let Some(hits) = cache.search.get(&key).await { return hits; }
//! let fresh = provider.search(...).await?;
//! cache.search.insert(key, fresh.clone()).await;
//!
//! // Stream cache
//! if let Some(streams) = cache.streams.get(id).await { return streams; }
//! ```

pub mod search;
pub mod metadata;
pub mod streams;

use std::sync::Arc;

pub use search::SearchCache;
pub use metadata::MetadataCache;
pub use streams::StreamCache;

/// Grouped handle — clone freely, all fields share the underlying Arc storage.
#[derive(Clone)]
pub struct RuntimeCache {
    pub search:   SearchCache,
    pub metadata: MetadataCache,
    pub streams:  StreamCache,
}

impl RuntimeCache {
    pub fn new() -> Self {
        RuntimeCache {
            search:   SearchCache::new(),
            metadata: MetadataCache::new(),
            streams:  StreamCache::new(),
        }
    }
}

impl Default for RuntimeCache {
    fn default() -> Self { Self::new() }
}

// ── Shared TTL helper ─────────────────────────────────────────────────────────

use std::time::{Duration, Instant};

/// A single cache entry wrapping a value with an expiry timestamp.
#[derive(Clone)]
pub(crate) struct Ttl<V> {
    pub value:      V,
    pub expires_at: Instant,
}

impl<V: Clone> Ttl<V> {
    pub fn new(value: V, ttl: Duration) -> Self {
        Ttl { value, expires_at: Instant::now() + ttl }
    }

    pub fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

// ── CachePolicy ───────────────────────────────────────────────────────────────

/// TTL configuration for each cache tier.
///
/// Constructed via `CachePolicy::default()` (production values) or
/// `CachePolicy::for_testing()` (very short TTLs to keep tests fast).
/// Inject into `RuntimeCache::new_with_policy` when non-default TTLs
/// are needed (e.g. different policies per deployment environment).
///
/// # Production defaults
///
/// | Cache tier   | Default TTL | Rationale |
/// |-------------|-------------|-----------|
/// | Search      | 5 minutes   | Providers are queryable; short TTL avoids stale trending |
/// | Metadata    | 24 hours    | Ratings/descriptions rarely change |
/// | Streams     | 10 minutes  | Magnet trackers, direct URLs may go stale |
/// | Catalog     | 30 minutes  | Grid refresh feels live without hammering providers |
#[derive(Debug, Clone)]
pub struct CachePolicy {
    /// How long to cache full-text search results.
    pub search_ttl:   Duration,
    /// How long to cache item metadata (detail page, enriched fields).
    pub metadata_ttl: Duration,
    /// How long to cache resolved stream candidates.
    pub streams_ttl:  Duration,
    /// How long to cache a trending/catalog grid page.
    pub catalog_ttl:  Duration,
}

impl Default for CachePolicy {
    fn default() -> Self {
        CachePolicy {
            search_ttl:   Duration::from_secs(5 * 60),       //  5 minutes
            metadata_ttl: Duration::from_secs(24 * 60 * 60), // 24 hours
            streams_ttl:  Duration::from_secs(10 * 60),      // 10 minutes
            catalog_ttl:  Duration::from_secs(30 * 60),      // 30 minutes
        }
    }
}

impl CachePolicy {
    /// Very short TTLs suitable for integration tests (everything expires in 1s).
    pub fn for_testing() -> Self {
        CachePolicy {
            search_ttl:   Duration::from_secs(1),
            metadata_ttl: Duration::from_secs(1),
            streams_ttl:  Duration::from_secs(1),
            catalog_ttl:  Duration::from_secs(1),
        }
    }

    /// Aggressive caching for low-bandwidth / offline scenarios.
    pub fn offline() -> Self {
        CachePolicy {
            search_ttl:   Duration::from_secs(60 * 60),       // 1 hour
            metadata_ttl: Duration::from_secs(7 * 24 * 60 * 60), // 1 week
            streams_ttl:  Duration::from_secs(60 * 60),       // 1 hour
            catalog_ttl:  Duration::from_secs(2 * 60 * 60),   // 2 hours
        }
    }
}
