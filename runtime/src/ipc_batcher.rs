//! IPC update batching for high-frequency GridUpdate events.
//!
//! When catalog refreshes happen rapidly (e.g., on startup or tab switch),
//! individual GridUpdate events can overwhelm the IPC channel. This module
//! buffers updates and flushes them at regular intervals, merging updates
//! for the same tab to reduce message overhead.
//!
//! # Usage
//!
//! ```rust,ignore
//! let batcher = IpcBatcher::with_interval(catalog.subscribe(), Duration::from_millis(200));
//! // In IPC loop:
//! if let Some(updates) = batcher.recv().await {
//!     for update in updates {
//!         // process batched update
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::time::{sleep, Instant};

use crate::catalog::GridUpdate;

const MAX_BATCH_SIZE: usize = 10;
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 200;

pub struct IpcBatcher {
    rx: broadcast::Receiver<GridUpdate>,
    flush_interval: Duration,
    buffer: HashMap<String, GridUpdate>,
    last_flush: Instant,
}

impl IpcBatcher {
    pub fn new(rx: broadcast::Receiver<GridUpdate>) -> Self {
        Self::with_interval(rx, Duration::from_millis(DEFAULT_FLUSH_INTERVAL_MS))
    }

    pub fn with_interval(rx: broadcast::Receiver<GridUpdate>, flush_interval: Duration) -> Self {
        Self {
            rx,
            flush_interval,
            buffer: HashMap::new(),
            last_flush: Instant::now(),
        }
    }

    fn should_flush(&self) -> bool {
        self.buffer.len() >= MAX_BATCH_SIZE || self.last_flush.elapsed() >= self.flush_interval
    }

    fn drain_buffer(&mut self) -> Vec<GridUpdate> {
        self.last_flush = Instant::now();
        self.buffer.drain().map(|(_, v)| v).collect()
    }

    pub async fn recv(&mut self) -> Option<Vec<GridUpdate>> {
        loop {
            tokio::select! {
                biased;
                
                update = self.rx.recv() => {
                    match update {
                        Ok(u) => {
                            self.buffer_insert(u);
                            if self.should_flush() {
                                return Some(self.drain_buffer());
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("grid update channel lagged {n} messages, clearing buffer");
                            self.buffer.clear();
                            self.last_flush = Instant::now();
                            return Some(Vec::new());
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            if !self.buffer.is_empty() {
                                return Some(self.drain_buffer());
                            }
                            return None;
                        }
                    }
                }
                
                _ = sleep(self.flush_interval.saturating_sub(self.last_flush.elapsed()).max(Duration::from_millis(1))) => {
                    if !self.buffer.is_empty() {
                        return Some(self.drain_buffer());
                    }
                    self.last_flush = Instant::now();
                }
            }
        }
    }

    fn buffer_insert(&mut self, update: GridUpdate) {
        let key = format!("{}_{:?}", update.tab, update.source);
        match self.buffer.get_mut(&key) {
            Some(existing) => {
                existing.entries.extend(update.entries);
            }
            None => {
                self.buffer.insert(key, update);
            }
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogEntry, GridUpdateSource};
    use crate::ipc::MediaType;

    const TEST_FLUSH_INTERVAL: Duration = Duration::from_millis(100);

    fn make_update(tab: &str, source: GridUpdateSource, count: usize) -> GridUpdate {
        GridUpdate {
            tab: tab.to_string(),
            entries: (0..count)
                .map(|i| CatalogEntry {
                    id: format!("{}_{}", tab, i),
                    title: format!("Title {}", i),
                    year: Some("2024".to_string()),
                    genre: None,
                    rating: None,
                    description: None,
                    poster_url: None,
                    poster_art: None,
                    provider: "test".to_string(),
                    tab: tab.to_string(),
                    imdb_id: None,
                    tmdb_id: None,
                    mal_id: None,
                    media_type: MediaType::default(),
                    ratings: Default::default(),
                    original_language: None,
                })
                .collect(),
            source,
        }
    }

    async fn recv_with_timeout(batcher: &mut IpcBatcher) -> Option<Vec<GridUpdate>> {
        tokio::time::timeout(TEST_FLUSH_INTERVAL * 2, batcher.recv()).await.ok().flatten()
    }

    #[tokio::test]
    async fn test_batcher_merges_same_tab() {
        let (tx, rx) = broadcast::channel(64);
        let batcher = IpcBatcher::with_interval(rx, TEST_FLUSH_INTERVAL);
        let mut batcher = batcher;

        tx.send(make_update("movies", GridUpdateSource::Live, 5)).ok();
        tx.send(make_update("movies", GridUpdateSource::Live, 3)).ok();
        
        tokio::time::sleep(Duration::from_millis(50)).await;
        
        let updates = recv_with_timeout(&mut batcher).await.unwrap();
        
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].entries.len(), 8);
        assert_eq!(updates[0].tab, "movies");
    }

    #[tokio::test]
    async fn test_batcher_separates_different_tabs() {
        let (tx, rx) = broadcast::channel(64);
        let batcher = IpcBatcher::with_interval(rx, TEST_FLUSH_INTERVAL);
        let mut batcher = batcher;

        tx.send(make_update("movies", GridUpdateSource::Live, 5)).ok();
        tx.send(make_update("series", GridUpdateSource::Live, 3)).ok();
        
        tokio::time::sleep(Duration::from_millis(50)).await;
        
        let updates = recv_with_timeout(&mut batcher).await.unwrap();
        
        assert_eq!(updates.len(), 2);
    }

    #[tokio::test]
    async fn test_batcher_respects_max_batch_size() {
        let (tx, rx) = broadcast::channel(64);
        // Duration::MAX: prevent time-based flush from interfering; only MAX_BATCH_SIZE should trigger.
        let batcher = IpcBatcher::with_interval(rx, Duration::MAX);
        let mut batcher = batcher;

        for i in 0..12 {
            tx.send(make_update(&format!("tab_{}", i), GridUpdateSource::Live, 1)).ok();
        }
        
        let updates = recv_with_timeout(&mut batcher).await.unwrap();
        
        assert_eq!(updates.len(), MAX_BATCH_SIZE);
    }

    #[tokio::test]
    async fn test_batcher_flushes_on_interval() {
        let (tx, rx) = broadcast::channel(64);
        let batcher = IpcBatcher::with_interval(rx, Duration::from_millis(100));
        let mut batcher = batcher;

        tx.send(make_update("movies", GridUpdateSource::Live, 5)).ok();
        
        let updates = recv_with_timeout(&mut batcher).await.unwrap();
        
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].entries.len(), 5);
    }

    #[tokio::test]
    async fn test_batcher_empty_on_close() {
        let (tx, rx) = broadcast::channel(64);
        drop(tx);
        let batcher = IpcBatcher::with_interval(rx, TEST_FLUSH_INTERVAL);
        let mut batcher = batcher;

        let result = recv_with_timeout(&mut batcher).await;
        
        assert!(result.is_none());
    }
}
