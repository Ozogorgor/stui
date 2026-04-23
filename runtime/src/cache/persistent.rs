//! SQLite-backed persistent cache layer.
//!
//! Sits under the in-memory `SearchCache` / `MetadataCache` as the "survive
//! daemon restart" tier of Phase 2 caching. API surface is intentionally
//! narrow: namespace + key → bytes with a unix-timestamp expiry. Callers own
//! their own value encoding (serde_json, bincode, etc.) — this module stays
//! agnostic.
//!
//! # Concurrency
//!
//! The SQLite connection is wrapped in a `std::sync::Mutex` and shared via
//! `Arc` — the store is `Send + Sync` and cheap to clone. Holding the lock
//! across SQLite calls is fine because our queries are fast (<1ms for the
//! single-row get/put pattern we use). If pressure ever shows the mutex as
//! a hot spot, switch to `spawn_blocking` for writes or move to a pool.
//!
//! # TTL semantics
//!
//! `expires_at` is a unix timestamp in seconds. `get` returns `None` when
//! the stored row's `expires_at` is in the past; `purge_expired` physically
//! deletes those rows. Consumers should treat the returned bytes as fresh
//! — staleness is filtered out here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use tracing::{debug, info, warn};

/// Monotonic unix-seconds clock. Centralized so tests can swap for a
/// deterministic source if we add that later.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct SqliteKv {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteKv {
    /// Open (or create) the cache DB at `path`. Parent directory is created
    /// if missing. Schema is migrated on first open; subsequent opens are
    /// `CREATE TABLE IF NOT EXISTS` idempotent.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating cache dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite at {}", path.display()))?;
        // Journaling mode tuned for a cache (we don't care about crash-safe
        // durability; losing the last write-through is benign — next lookup
        // just re-hits the network).
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS response_cache (
                namespace   TEXT NOT NULL,
                key         TEXT NOT NULL,
                value       BLOB NOT NULL,
                expires_at  INTEGER NOT NULL,
                PRIMARY KEY (namespace, key)
            )",
            [],
        )
        .context("creating response_cache schema")?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_response_cache_expires
               ON response_cache(expires_at)",
            [],
        )
        .ok();
        info!(path = %path.display(), "sqlite response cache open");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Fetch bytes for (namespace, key). Returns `None` for missing rows and
    /// for rows whose `expires_at` has passed. Expired rows stay on disk
    /// until `purge_expired` reclaims them.
    pub fn get(&self, namespace: &str, key: &str) -> Option<Vec<u8>> {
        let now = now_secs() as i64;
        let conn = self.conn.lock().ok()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT value FROM response_cache
                  WHERE namespace = ?1 AND key = ?2 AND expires_at > ?3",
            )
            .ok()?;
        let mut rows = stmt.query(params![namespace, key, now]).ok()?;
        match rows.next() {
            Ok(Some(row)) => row.get::<_, Vec<u8>>(0).ok(),
            _ => None,
        }
    }

    /// Upsert. Empty-value guard is left to callers — this module serializes
    /// whatever bytes it's handed.
    pub fn put(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
        expires_at: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))?;
        conn.execute(
            "INSERT INTO response_cache (namespace, key, value, expires_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(namespace, key)
             DO UPDATE SET value = excluded.value,
                           expires_at = excluded.expires_at",
            params![namespace, key, value, expires_at as i64],
        )?;
        Ok(())
    }

    /// Delete every row whose `expires_at` has passed. Returns the number
    /// deleted so callers can log disk reclamation.
    pub fn purge_expired(&self) -> Result<usize> {
        let now = now_secs() as i64;
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))?;
        let deleted = conn.execute(
            "DELETE FROM response_cache WHERE expires_at <= ?1",
            params![now],
        )?;
        if deleted > 0 {
            debug!(deleted, "sqlite cache: purged expired rows");
        }
        Ok(deleted)
    }

    /// Drop every row in a namespace. Used by the `R` hotkey / manual
    /// catalog-refresh so a forced refresh also wipes the on-disk cache.
    #[allow(dead_code)] // pub API: wired once Phase 2 integration lands
    pub fn clear_namespace(&self, namespace: &str) -> Result<usize> {
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))?;
        let deleted = conn.execute(
            "DELETE FROM response_cache WHERE namespace = ?1",
            params![namespace],
        )?;
        Ok(deleted)
    }

    /// Drop every row across all namespaces. Used by the admin CLI's
    /// `cache clear` (no --namespace flag).
    pub fn clear_all(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))?;
        let deleted = conn.execute("DELETE FROM response_cache", [])?;
        Ok(deleted)
    }

    /// Aggregated stats per namespace. Used by `cache stats` admin CLI.
    pub fn namespace_stats(&self) -> Result<Vec<NamespaceStats>> {
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT namespace,
                    COUNT(*)              AS rows,
                    COALESCE(SUM(LENGTH(value)), 0) AS total_bytes,
                    MIN(expires_at)       AS oldest_expiry,
                    MAX(expires_at)       AS newest_expiry
               FROM response_cache
              GROUP BY namespace
              ORDER BY namespace",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(NamespaceStats {
                namespace: row.get(0)?,
                rows: row.get(1)?,
                total_bytes: row.get(2)?,
                oldest_expiry: row.get(3)?,
                newest_expiry: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Iterate all keys in a namespace (without values). Useful for `cache
    /// inspect` to enumerate what's available. Sorted lexicographically
    /// so `| head` gives a stable view.
    pub fn list_keys(&self, namespace: &str, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT key FROM response_cache WHERE namespace = ?1 ORDER BY key LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![namespace, limit as i64], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }
}

/// One row per namespace in `SqliteKv::namespace_stats()`.
#[derive(Debug)]
pub struct NamespaceStats {
    pub namespace: String,
    pub rows: i64,
    pub total_bytes: i64,
    /// Unix-seconds expiry of the row that expires SOONEST. `None` when
    /// the namespace is empty.
    pub oldest_expiry: Option<i64>,
    /// Unix-seconds expiry of the row that expires LATEST.
    pub newest_expiry: Option<i64>,
}

/// Default DB path: `~/.cache/stui/response.db`. Falls back to a tempdir
/// sibling when `$HOME` / `$XDG_CACHE_HOME` can't be resolved (rare —
/// mostly tests / chroots).
pub fn default_cache_db_path() -> PathBuf {
    if let Some(dir) = dirs::cache_dir() {
        return dir.join("stui").join("response.db");
    }
    warn!("cache_dir unresolved; using /tmp/stui-response.db");
    PathBuf::from("/tmp/stui-response.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_creates_schema() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let kv = SqliteKv::open(&path).unwrap();
        // Second open on same file must not error — schema is idempotent.
        drop(kv);
        SqliteKv::open(&path).unwrap();
    }

    #[test]
    fn roundtrip() {
        let dir = tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("t.db")).unwrap();
        let future = now_secs() + 3600;
        kv.put("ns", "k", b"hello", future).unwrap();
        let got = kv.get("ns", "k").unwrap();
        assert_eq!(got, b"hello");
    }

    #[test]
    fn miss_returns_none() {
        let dir = tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("t.db")).unwrap();
        assert!(kv.get("ns", "missing").is_none());
    }

    #[test]
    fn expired_filtered_on_get() {
        let dir = tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("t.db")).unwrap();
        let past = now_secs().saturating_sub(10);
        kv.put("ns", "k", b"old", past).unwrap();
        assert!(kv.get("ns", "k").is_none(), "expired rows must not leak out via get");
    }

    #[test]
    fn purge_expired_deletes_matching_rows() {
        let dir = tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("t.db")).unwrap();
        kv.put("ns", "stale", b"x", now_secs().saturating_sub(5)).unwrap();
        kv.put("ns", "fresh", b"y", now_secs() + 3600).unwrap();
        let deleted = kv.purge_expired().unwrap();
        assert_eq!(deleted, 1);
        assert!(kv.get("ns", "stale").is_none());
        assert_eq!(kv.get("ns", "fresh").unwrap(), b"y");
    }

    #[test]
    fn upsert_replaces_value_and_expiry() {
        let dir = tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("t.db")).unwrap();
        kv.put("ns", "k", b"v1", now_secs() + 100).unwrap();
        kv.put("ns", "k", b"v2", now_secs() + 7200).unwrap();
        assert_eq!(kv.get("ns", "k").unwrap(), b"v2");
    }

    #[test]
    fn clear_namespace_removes_only_that_namespace() {
        let dir = tempdir().unwrap();
        let kv = SqliteKv::open(&dir.path().join("t.db")).unwrap();
        kv.put("a", "k", b"1", now_secs() + 100).unwrap();
        kv.put("b", "k", b"2", now_secs() + 100).unwrap();
        kv.clear_namespace("a").unwrap();
        assert!(kv.get("a", "k").is_none());
        assert_eq!(kv.get("b", "k").unwrap(), b"2");
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let kv = SqliteKv::open(&path).unwrap();
            kv.put("ns", "k", b"persisted", now_secs() + 3600).unwrap();
        }
        let kv = SqliteKv::open(&path).unwrap();
        assert_eq!(kv.get("ns", "k").unwrap(), b"persisted");
    }
}
