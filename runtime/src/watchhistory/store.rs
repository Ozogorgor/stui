//! Watch history storage using SQLite.
//!
//! Uses SQLite for efficient indexed lookups and persistent storage.
//! SQLite operations run in a dedicated blocking thread to avoid blocking the async runtime.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

const COMPLETED_THRESHOLD: f64 = 0.90;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistoryEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub year: Option<String>,
    pub tab: String,
    pub provider: String,
    #[serde(default)]
    pub imdb_id: Option<String>,
    #[serde(default)]
    pub position: f64,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub completed: bool,
    pub last_watched: i64,
    #[serde(default)]
    pub season: u32,
    #[serde(default)]
    pub episode: u32,
    #[serde(default)]
    pub file_path: Option<String>,
}

impl WatchHistoryEntry {
    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub fn progress(&self) -> f64 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        let f = self.position / self.duration;
        f.min(1.0)
    }
}

// ── SQLite Backend ─────────────────────────────────────────────────────────────

struct SqliteBackend {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteBackend {
    fn new(path: &PathBuf) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn init_schema(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS watch_history (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                year        TEXT,
                tab         TEXT NOT NULL,
                provider    TEXT NOT NULL,
                imdb_id     TEXT,
                position    REAL DEFAULT 0,
                duration    REAL DEFAULT 0,
                completed   INTEGER DEFAULT 0,
                last_watched INTEGER NOT NULL,
                season      INTEGER DEFAULT 0,
                episode     INTEGER DEFAULT 0,
                file_path   TEXT
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_watch_history_tab 
             ON watch_history(tab, last_watched DESC)",
            [],
        )?;
        Ok(())
    }

    fn upsert(&self, entry: &WatchHistoryEntry) -> Result<(), rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        conn.execute(
            "INSERT INTO watch_history 
             (id, title, year, tab, provider, imdb_id, position, duration, completed, last_watched, season, episode, file_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                year = excluded.year,
                tab = excluded.tab,
                provider = excluded.provider,
                imdb_id = excluded.imdb_id,
                position = excluded.position,
                duration = excluded.duration,
                completed = excluded.completed,
                last_watched = excluded.last_watched,
                season = excluded.season,
                episode = excluded.episode,
                file_path = COALESCE(excluded.file_path, watch_history.file_path)",
            params![
                entry.id,
                entry.title,
                entry.year,
                entry.tab,
                entry.provider,
                entry.imdb_id,
                entry.position,
                entry.duration,
                entry.completed as i32,
                entry.last_watched,
                entry.season,
                entry.episode,
                entry.file_path,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<WatchHistoryEntry>, rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, title, year, tab, provider, imdb_id, position, duration, completed, last_watched, season, episode, file_path
             FROM watch_history WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_entry(row)))
        } else {
            Ok(None)
        }
    }

    fn remove(&self, id: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        let affected = conn.execute(
            "DELETE FROM watch_history WHERE id = ?1",
            params![id],
        )?;
        Ok(affected > 0)
    }

    fn mark_completed(&self, id: &str, last_watched: i64) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        let affected = conn.execute(
            "UPDATE watch_history SET completed = 1, last_watched = ?2 WHERE id = ?1",
            params![id, last_watched],
        )?;
        Ok(affected > 0)
    }

    fn update_position(
        &self,
        id: &str,
        position: f64,
        duration: f64,
        last_watched: i64,
    ) -> Result<bool, rusqlite::Error> {
        let completed = if duration > 0.0 && position / duration >= COMPLETED_THRESHOLD {
            1
        } else {
            0
        };
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        let affected = conn.execute(
            "UPDATE watch_history
             SET position = ?2, duration = ?3, completed = ?4, last_watched = ?5
             WHERE id = ?1",
            params![id, position, duration, completed, last_watched],
        )?;
        Ok(affected > 0)
    }

    fn update_file_path(&self, id: &str, file_path: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        let affected = conn.execute(
            "UPDATE watch_history SET file_path = ?2 WHERE id = ?1",
            params![id, file_path],
        )?;
        Ok(affected > 0)
    }

    fn in_progress(&self) -> Result<Vec<WatchHistoryEntry>, rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, title, year, tab, provider, imdb_id, position, duration, completed, last_watched, season, episode, file_path
             FROM watch_history
             WHERE position > 0 AND completed = 0
             ORDER BY last_watched DESC",
        )?;
        let entries = stmt
            .query_map([], |row| Ok(row_to_entry(row)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(entries)
    }

    fn in_progress_for_tab(&self, tab: &str) -> Result<Vec<WatchHistoryEntry>, rusqlite::Error> {
        let conn = self.conn.as_ref().lock().expect("watchhistory db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, title, year, tab, provider, imdb_id, position, duration, completed, last_watched, season, episode, file_path
             FROM watch_history
             WHERE position > 0 AND completed = 0 AND tab = ?1
             ORDER BY last_watched DESC
             LIMIT 5",
        )?;
        let entries = stmt
            .query_map(params![tab], |row| Ok(row_to_entry(row)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(entries)
    }
}

fn row_to_entry(row: &rusqlite::Row) -> WatchHistoryEntry {
    WatchHistoryEntry {
        id: row.get(0).unwrap_or_default(),
        title: row.get(1).unwrap_or_default(),
        year: row.get(2).ok(),
        tab: row.get(3).unwrap_or_default(),
        provider: row.get(4).unwrap_or_default(),
        imdb_id: row.get(5).ok(),
        position: row.get(6).unwrap_or(0.0),
        duration: row.get(7).unwrap_or(0.0),
        completed: row.get::<_, i32>(8).unwrap_or(0) != 0,
        last_watched: row.get(9).unwrap_or(0),
        season: row.get(10).unwrap_or(0),
        episode: row.get(11).unwrap_or(0),
        file_path: row.get(12).ok(),
    }
}

// ── Async Wrapper ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct WatchHistoryStore {
    backend: Arc<SqliteBackend>,
}

impl WatchHistoryStore {
    pub fn new(path: PathBuf) -> Self {
        let backend = SqliteBackend::new(&path).expect("Failed to open watch history database");
        backend.init_schema().expect("Failed to initialize watch history schema");
        info!(path = %path.display(), "watch history store initialized (SQLite)");
        Self { backend: Arc::new(backend) }
    }

    pub async fn upsert(&self, mut entry: WatchHistoryEntry) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        entry.last_watched = now;

        let backend = self.backend.clone();
        let entry_clone = entry.clone();
        
        tokio::task::spawn_blocking(move || {
            if let Err(e) = backend.upsert(&entry_clone) {
                warn!(err = %e, id = %entry_clone.id, "failed to upsert watch history entry");
            }
        })
        .await
        .ok();
    }

    pub async fn get(&self, id: &str) -> Option<WatchHistoryEntry> {
        let backend = self.backend.clone();
        let id_owned = id.to_string();
        
        tokio::task::spawn_blocking(move || {
            backend.get(&id_owned).ok().flatten()
        })
        .await
        .ok()
        .flatten()
    }

    pub async fn remove(&self, id: &str) {
        let backend = self.backend.clone();
        let id_owned = id.to_string();
        
        tokio::task::spawn_blocking(move || {
            if let Err(e) = backend.remove(&id_owned) {
                warn!(err = %e, id = %id_owned, "failed to remove watch history entry");
            }
        })
        .await
        .ok();
    }

    pub async fn mark_completed(&self, id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let backend = self.backend.clone();
        let id_owned = id.to_string();
        
        tokio::task::spawn_blocking(move || {
            if let Err(e) = backend.mark_completed(&id_owned, now) {
                warn!(err = %e, id = %id_owned, "failed to mark watch history entry completed");
            }
        })
        .await
        .ok();
    }

    pub async fn update_position(&self, id: &str, position: f64, duration: f64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let backend = self.backend.clone();
        let id_owned = id.to_string();
        let id_for_warning = id_owned.clone();
        
        let result = tokio::task::spawn_blocking(move || {
            backend.update_position(&id_owned, position, duration, now)
        })
        .await;
        
        match result {
            Ok(Ok(found)) => found,
            Ok(Err(e)) => {
                warn!(err = %e, id = %id_for_warning, "failed to update watch history position");
                false
            }
            Err(e) => {
                warn!(err = %e, "task join error");
                false
            }
        }
    }

    pub async fn update_file_path(&self, id: &str, file_path: &str) -> bool {
        let backend = self.backend.clone();
        let id_owned = id.to_string();
        let path_owned = file_path.to_string();
        let id_for_warning = id_owned.clone();

        let result = tokio::task::spawn_blocking(move || {
            backend.update_file_path(&id_owned, &path_owned)
        })
        .await;

        match result {
            Ok(Ok(found)) => found,
            Ok(Err(e)) => {
                warn!(err = %e, id = %id_for_warning, "failed to update file path");
                false
            }
            Err(e) => {
                warn!(err = %e, "task join error");
                false
            }
        }
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn in_progress(&self) -> Vec<WatchHistoryEntry> {
        let backend = self.backend.clone();
        
        tokio::task::spawn_blocking(move || {
            backend.in_progress().unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }

    #[allow(dead_code)] // pub API: used by TUI / IPC layer
    pub async fn in_progress_for_tab(&self, tab: &str) -> Vec<WatchHistoryEntry> {
        let backend = self.backend.clone();
        let tab_owned = tab.to_string();
        
        tokio::task::spawn_blocking(move || {
            backend.in_progress_for_tab(&tab_owned).unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn create_test_store() -> WatchHistoryStore {
        let path = temp_dir().join(format!("test_watchhistory_{}.db", uuid::Uuid::new_v4()));
        WatchHistoryStore::new(path)
    }

    #[tokio::test]
    async fn test_upsert_and_get() {
        let store = create_test_store();
        
        let entry = WatchHistoryEntry {
            id: "test-1".to_string(),
            title: "Test Movie".to_string(),
            year: Some("2024".to_string()),
            tab: "movies".to_string(),
            provider: "test".to_string(),
            imdb_id: None,
            position: 0.0,
            duration: 0.0,
            completed: false,
            last_watched: 0,
            season: 0,
            episode: 0,
            file_path: None,
        };
        
        store.upsert(entry).await;
        
        let retrieved = store.get("test-1").await;
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.title, "Test Movie");
    }

    #[tokio::test]
    async fn test_update_position() {
        let store = create_test_store();
        
        let entry = WatchHistoryEntry {
            id: "test-2".to_string(),
            title: "Test Movie 2".to_string(),
            year: None,
            tab: "movies".to_string(),
            provider: "test".to_string(),
            imdb_id: None,
            position: 0.0,
            duration: 1000.0,
            completed: false,
            last_watched: 0,
            season: 0,
            episode: 0,
            file_path: None,
        };
        
        store.upsert(entry).await;
        
        // Update position below threshold
        let found = store.update_position("test-2", 500.0, 1000.0).await;
        assert!(found);
        
        // Verify not completed
        let retrieved = store.get("test-2").await.unwrap();
        assert!(!retrieved.completed);
        
        // Update position above threshold (should mark completed)
        let found = store.update_position("test-2", 950.0, 1000.0).await;
        assert!(found);
        
        // Verify completed
        let retrieved = store.get("test-2").await.unwrap();
        assert!(retrieved.completed);
    }

    #[tokio::test]
    async fn test_remove() {
        let store = create_test_store();
        
        let entry = WatchHistoryEntry {
            id: "test-3".to_string(),
            title: "Test Movie 3".to_string(),
            year: None,
            tab: "movies".to_string(),
            provider: "test".to_string(),
            imdb_id: None,
            position: 0.0,
            duration: 0.0,
            completed: false,
            last_watched: 0,
            season: 0,
            episode: 0,
            file_path: None,
        };
        
        store.upsert(entry).await;
        assert!(store.get("test-3").await.is_some());
        
        store.remove("test-3").await;
        assert!(store.get("test-3").await.is_none());
    }

    #[tokio::test]
    async fn test_progress_calculation() {
        let entry = WatchHistoryEntry {
            id: "test".to_string(),
            title: "Test".to_string(),
            year: None,
            tab: "movies".to_string(),
            provider: "test".to_string(),
            imdb_id: None,
            position: 450.0,
            duration: 900.0,
            completed: false,
            last_watched: 0,
            season: 0,
            episode: 0,
            file_path: None,
        };
        assert!((entry.progress() - 0.5).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_progress_zero_duration() {
        let entry = WatchHistoryEntry {
            id: "test".to_string(),
            title: "Test".to_string(),
            year: None,
            tab: "movies".to_string(),
            provider: "test".to_string(),
            imdb_id: None,
            position: 100.0,
            duration: 0.0,
            completed: false,
            last_watched: 0,
            season: 0,
            episode: 0,
            file_path: None,
        };
        assert_eq!(entry.progress(), 0.0);
    }

    #[tokio::test]
    async fn test_file_path_update() {
        let store = create_test_store();
        
        let entry = WatchHistoryEntry {
            id: "test-filepath".to_string(),
            title: "Test File Path".to_string(),
            year: None,
            tab: "movies".to_string(),
            provider: "test".to_string(),
            imdb_id: None,
            position: 0.0,
            duration: 0.0,
            completed: false,
            last_watched: 0,
            season: 0,
            episode: 0,
            file_path: None,
        };
        
        store.upsert(entry).await;
        
        let file_path = "/home/user/Videos/Movies/2024 - Test/movie.mkv";
        let found = store.update_file_path("test-filepath", file_path).await;
        assert!(found);
        
        let retrieved = store.get("test-filepath").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().file_path, Some(file_path.to_string()));
    }
}
