//! Anime cross-id bridge — runtime-native cross-mapping fed by the
//! Fribb anime-lists dataset. Used at search time to fill missing
//! foreign ids on `MediaEntry`s before dedup, so the existing α
//! `dedup_key` precedence (mal → imdb → title) collapses cross-tier
//! duplicates that were previously stranded in different key tiers.
//!
//! Loading is two-phase:
//!   1. `AnimeBridge::new()` synchronously decompresses + parses the
//!      bundled snapshot baked into the runtime binary at compile time.
//!      The bridge is usable for queries on return; never blocks an
//!      async runtime.
//!   2. A separate `start_refresh_task()` async hook (called from the
//!      IPC layer or wherever the tokio runtime is already in scope)
//!      spawns the 24h refresh loop. The fetched payload atomically
//!      swaps the in-memory index via `ArcSwap`.
//!
//! Cold start always works (bundled snapshot serves immediately).
//! Refresh happens out-of-band and never blocks search.

pub mod enrich;
pub mod fetch; // Stub created in this task; populated in Task 3
pub mod index; // Stub created in this task; populated in Task 5

use std::io::Read as _;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{info, warn};

pub use index::{AnimeIndex, AnimeRecord};

/// Bundled Fribb snapshot, gzipped, baked into the binary at compile
/// time. Decompressed on first `AnimeBridge::new()`; refresh task may
/// later overwrite the in-memory index with a fresher copy from
/// GitHub.
const SNAPSHOT_GZ: &[u8] = include_bytes!("../../data/anime-bridge-snapshot.json.gz");

pub struct AnimeBridge {
    pub(crate) index: ArcSwap<AnimeIndex>,
}

impl AnimeBridge {
    /// Decompress + parse the bundled snapshot, build the index,
    /// return a usable bridge. On any failure, falls back to an empty
    /// index — the bridge stays operational and degrades gracefully
    /// to milestone-α behaviour.
    ///
    /// Synchronous: safe to call from `Engine::new()` without a tokio
    /// runtime in scope.
    pub fn new() -> Arc<Self> {
        let index = match Self::load_bundled_snapshot() {
            Ok(idx) => {
                info!(
                    entries = idx.by_mal.len(),
                    "anime_bridge: bundled snapshot loaded"
                );
                idx
            }
            Err(e) => {
                warn!(err = %e, "anime_bridge: bundled snapshot failed to load; falling back to empty index");
                AnimeIndex::empty()
            }
        };
        Arc::new(Self {
            index: ArcSwap::from(Arc::new(index)),
        })
    }

    fn load_bundled_snapshot() -> anyhow::Result<AnimeIndex> {
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(SNAPSHOT_GZ)
            .read_to_end(&mut decoded)
            .map_err(|e| anyhow::anyhow!("snapshot gunzip failed: {e}"))?;
        AnimeIndex::from_json(&decoded)
    }

    /// Replace the in-memory index atomically. Called by the refresh
    /// task (Task 3) after a successful fetch + parse.
    pub(crate) fn swap_index(&self, new_index: Arc<AnimeIndex>) {
        self.index.store(new_index);
    }

    /// Snapshot of the current index. Cheap (clones an Arc).
    pub(crate) fn current(&self) -> Arc<AnimeIndex> {
        self.index.load_full()
    }

    pub fn lookup_by_mal(&self, id: &str) -> Option<Arc<AnimeRecord>> {
        if id.is_empty() {
            return None;
        }
        self.current().by_mal.get(id).cloned()
    }
    pub fn lookup_by_anilist(&self, id: &str) -> Option<Arc<AnimeRecord>> {
        if id.is_empty() {
            return None;
        }
        self.current().by_anilist.get(id).cloned()
    }
    pub fn lookup_by_kitsu(&self, id: &str) -> Option<Arc<AnimeRecord>> {
        if id.is_empty() {
            return None;
        }
        self.current().by_kitsu.get(id).cloned()
    }
    pub fn lookup_by_imdb(&self, id: &str) -> Option<Arc<AnimeRecord>> {
        if id.is_empty() {
            return None;
        }
        self.current().by_imdb.get(id).cloned()
    }
    pub fn lookup_by_tmdb(&self, id: &str) -> Option<Arc<AnimeRecord>> {
        if id.is_empty() {
            return None;
        }
        self.current().by_tmdb.get(id).cloned()
    }
    pub fn lookup_by_tvdb(&self, id: &str) -> Option<Arc<AnimeRecord>> {
        if id.is_empty() {
            return None;
        }
        self.current().by_tvdb.get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_loads_bundled_snapshot_with_known_anime() {
        // Cowboy Bebop has stable cross-mappings in Fribb: mal_id=1,
        // imdb_id=tt0213338. If the snapshot loaded, this lookup
        // should hit. (If Fribb ever drops or re-ids Cowboy Bebop
        // this test will need updating — but Bebop is essentially
        // canonical in cross-mapping datasets.)
        let bridge = AnimeBridge::new();
        let r = bridge.lookup_by_mal("1");
        assert!(
            r.is_some(),
            "bundled snapshot should index mal=1 (Cowboy Bebop)"
        );
        let r = r.unwrap();
        assert_eq!(r.imdb_id.as_deref(), Some("tt0213338"));
    }

    #[test]
    fn lookup_returns_none_for_empty_id() {
        let bridge = AnimeBridge::new();
        assert!(bridge.lookup_by_mal("").is_none());
        assert!(bridge.lookup_by_imdb("").is_none());
    }

    #[test]
    fn lookup_returns_none_for_unknown_id() {
        let bridge = AnimeBridge::new();
        assert!(bridge.lookup_by_imdb("tt9999999999").is_none());
    }
}
