//! Runtime cache layer.
//!
//! Three independent TTL caches plus an optional SQLite-backed persistent
//! tier (see `persistent.rs`) wired under `SearchCache`:
//!
//! | Cache             | Key                                | Value              | TTL    | Disk-persisted |
//! |-------------------|------------------------------------|--------------------|--------|----------------|
//! | `SearchCache`     | (plugin_id, query, scope, page)    | Vec<MediaEntry>    | 2 hr   | yes            |
//! | `MetadataCache`   | imdb_id / entry_id                 | DetailEntry        | 24 hr  | no (unused)    |
//! | `StreamCache`     | entry_id                           | Vec<Stream>        | 10 min | no (unused)    |
//!
//! `SearchCache` is the only one exercised by real call sites today — the
//! other two are infrastructure waiting on their respective callers. The
//! disk tier at `~/.cache/stui/response.db` survives daemon restarts; mem
//! evicts on a 5-minute interval spawned from `main.rs`.
//!
//! All caches are cheap to clone (they share the underlying `Arc`).
//! The runtime holds a single `RuntimeCache` that groups all three.
//!
//! See individual cache modules for detailed usage examples.

pub mod search;
pub mod metadata;
pub mod streams;
pub mod persistent;

pub use search::SearchCache;
pub use metadata::MetadataCache;
pub use streams::StreamCache;
pub use persistent::{default_cache_db_path, SqliteKv};

/// Grouped handle — clone freely, all fields share the underlying Arc storage.
#[allow(dead_code)] // pub API: used by engine cache layer
#[derive(Clone)]
pub struct RuntimeCache {
    pub search:   SearchCache,
    pub metadata: MetadataCache,
    pub streams:  StreamCache,
    /// On-disk persistence tier shared with SearchCache. Exposed so the
    /// daemon can run periodic `purge_expired` against the physical DB
    /// without going through the per-cache API.
    pub disk:     Option<std::sync::Arc<SqliteKv>>,
}

impl RuntimeCache {
    /// Mem-only cache. Use when the disk tier is unavailable or undesired
    /// (tests, inline-mode one-shots).
    pub fn new() -> Self {
        RuntimeCache {
            search:   SearchCache::new(),
            metadata: MetadataCache::new(),
            streams:  StreamCache::new(),
            disk:     None,
        }
    }

    /// Cache with the SQLite persistence tier wired into SearchCache.
    /// MetadataCache and StreamCache remain mem-only until they have real
    /// callers worth persisting for.
    pub fn with_disk(disk: std::sync::Arc<SqliteKv>) -> Self {
        RuntimeCache {
            search:   SearchCache::with_disk(std::sync::Arc::clone(&disk)),
            metadata: MetadataCache::new(),
            streams:  StreamCache::new(),
            disk:     Some(disk),
        }
    }
}

impl Default for RuntimeCache {
    fn default() -> Self { Self::new() }
}

// ── Shared TTL helper ─────────────────────────────────────────────────────────

use std::time::{Duration, Instant};

/// A single cache entry wrapping a value with an expiry timestamp.
#[allow(dead_code)] // pub API: used by engine cache layer
#[derive(Clone)]
pub(crate) struct Ttl<V> {
    pub value:      V,
    pub expires_at: Instant,
}

impl<V: Clone> Ttl<V> {
    #[allow(dead_code)] // pub API: used by engine cache layer
    pub fn new(value: V, ttl: Duration) -> Self {
        Ttl { value, expires_at: Instant::now() + ttl }
    }

    #[allow(dead_code)] // pub API: used by engine cache layer
    pub fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

// ── CachePolicy ───────────────────────────────────────────────────────────────

/// TTL configuration for each cache tier.
#[allow(dead_code)] // pub API: used by engine cache layer
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
    #[allow(dead_code)] // pub API: used by engine cache layer
    pub fn for_testing() -> Self {
        CachePolicy {
            search_ttl:   Duration::from_secs(1),
            metadata_ttl: Duration::from_secs(1),
            streams_ttl:  Duration::from_secs(1),
            catalog_ttl:  Duration::from_secs(1),
        }
    }

    /// Aggressive caching for low-bandwidth / offline scenarios.
    #[allow(dead_code)] // pub API: used by engine cache layer
    pub fn offline() -> Self {
        CachePolicy {
            search_ttl:   Duration::from_secs(60 * 60),       // 1 hour
            metadata_ttl: Duration::from_secs(7 * 24 * 60 * 60), // 1 week
            streams_ttl:  Duration::from_secs(60 * 60),       // 1 hour
            catalog_ttl:  Duration::from_secs(2 * 60 * 60),   // 2 hours
        }
    }
}
