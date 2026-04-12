use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::ipc::MediaEntry;

#[allow(dead_code)] // pub API: used by engine and IPC layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTab {
    pub tab: String,
    pub entries: Vec<MediaEntry>,
    #[serde(rename = "updated_at")]
    pub updated_at: i64,
}

#[allow(dead_code)] // pub API: used by engine and IPC layer
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct MediaCache {
    #[serde(rename = "tabs")]
    tabs: HashMap<String, CachedTab>,
}


impl MediaCache {
    #[allow(dead_code)] // pub API: used by engine and IPC layer
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)] // pub API: used by engine and IPC layer
    pub fn load(path: &PathBuf) -> Self {
        let mut cache = Self::default();
        if let Err(e) = cache.load_from_file(path) {
            warn!(path = %path.display(), err = %e, "failed to load media cache");
        }
        cache
    }

    fn load_from_file(&mut self, path: &PathBuf) -> std::io::Result<()> {
        let data = std::fs::read_to_string(path)?;
        let loaded: MediaCache = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.tabs = loaded.tabs;
        Ok(())
    }

    pub fn save(&self, path: &PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)?;
        info!(path = %path.display(), "media cache saved");
        Ok(())
    }

    pub fn update_tab(&mut self, tab: String, entries: Vec<MediaEntry>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.tabs.insert(tab.clone(), CachedTab {
            tab,
            entries,
            updated_at: now,
        });
    }

    #[allow(dead_code)] // pub API: used by engine and IPC layer
    pub fn save_tab(&mut self, tab: String, entries: Vec<MediaEntry>, path: &PathBuf) {
        self.update_tab(tab, entries);
        if let Err(e) = self.save(path) {
            warn!(err = %e, "failed to save media cache");
        }
    }

    pub fn entries_for_tab(&self, tab: &str) -> Vec<MediaEntry> {
        self.tabs
            .get(tab)
            .map(|ct| ct.entries.clone())
            .unwrap_or_default()
    }

    pub fn all_entries(&self) -> Vec<MediaEntry> {
        self.tabs
            .values()
            .flat_map(|ct| ct.entries.clone())
            .collect()
    }

    pub fn total_count(&self) -> usize {
        self.tabs.values().map(|ct| ct.entries.len()).sum()
    }

    #[allow(dead_code)] // pub API: used by engine and IPC layer
    pub fn clear(&mut self, path: &PathBuf) {
        self.tabs.clear();
        let _ = std::fs::remove_file(path);
    }

    pub fn last_updated(&self) -> i64 {
        self.tabs
            .values()
            .map(|ct| ct.updated_at)
            .max()
            .unwrap_or(0)
    }
}

#[allow(dead_code)] // pub API: used by engine and IPC layer
#[derive(Clone)]
pub struct MediaCacheStore {
    inner: Arc<RwLock<MediaCache>>,
    path: PathBuf,
}

impl MediaCacheStore {
    pub async fn new(path: PathBuf) -> Self {
        let load_path = path.clone();
        let cache = tokio::task::spawn_blocking(move || MediaCache::load(&load_path))
            .await
            .unwrap_or_default();
        Self {
            inner: Arc::new(RwLock::new(cache)),
            path,
        }
    }

    pub async fn save_tab(&self, tab: String, entries: Vec<MediaEntry>) {
        // Update cache under write lock
        {
            let mut cache = self.inner.write().await;
            cache.update_tab(tab, entries);
        }
        // Release lock before I/O
        let cache = self.inner.read().await;
        if let Err(e) = cache.save(&self.path) {
            warn!(err = %e, "failed to save media cache");
        }
    }

    pub async fn entries_for_tab(&self, tab: &str) -> Vec<MediaEntry> {
        let cache = self.inner.read().await;
        cache.entries_for_tab(tab)
    }

    pub async fn all_entries(&self) -> Vec<MediaEntry> {
        let cache = self.inner.read().await;
        cache.all_entries()
    }

    pub async fn total_count(&self) -> usize {
        let cache = self.inner.read().await;
        cache.total_count()
    }

    pub async fn clear(&self) {
        let mut cache = self.inner.write().await;
        cache.clear(&self.path);
    }

    pub async fn last_updated(&self) -> i64 {
        let cache = self.inner.read().await;
        cache.last_updated()
    }

    pub async fn tab_updated_at(&self, tab: &str) -> i64 {
        let cache = self.inner.read().await;
        cache.tabs.get(tab).map(|ct| ct.updated_at).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MediaTab;
    use crate::media::MediaType;

    #[test]
    fn test_cache_save_load() {
        let mut cache = MediaCache::new();
        cache.tabs.insert("movies".to_string(), CachedTab {
            tab: "movies".to_string(),
            entries: vec![],
            updated_at: 1000,
        });

        let temp_path = std::env::temp_dir().join("test_mediacache.json");
        cache.save(&temp_path).unwrap();

        let loaded = MediaCache::load(&temp_path);
        assert!(loaded.tabs.contains_key("movies"));

        let _ = std::fs::remove_file(&temp_path);
    }

    #[test]
    fn test_total_count() {
        let mut cache = MediaCache::new();
        let entry = MediaEntry {
            id: "test".to_string(),
            title: "Test".to_string(),
            year: None,
            genre: None,
            rating: None,
            ratings: std::collections::HashMap::new(),
            description: None,
            poster_url: None,
            provider: "test".to_string(),
            tab: MediaTab::Movies,
            media_type: MediaType::Movie,
            imdb_id: None,
            tmdb_id: None,
        };
        cache.tabs.insert("movies".to_string(), CachedTab {
            tab: "movies".to_string(),
            entries: vec![entry.clone(), entry.clone(), entry.clone(), entry.clone(), entry.clone()],
            updated_at: 1000,
        });
        cache.tabs.insert("series".to_string(), CachedTab {
            tab: "series".to_string(),
            entries: vec![entry.clone(), entry.clone(), entry],
            updated_at: 1001,
        });

        assert_eq!(cache.total_count(), 8);
    }
}
