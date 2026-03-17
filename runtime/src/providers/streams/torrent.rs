//! Torrent stream provider.
//!
//! Aggregates magnet/torrent results from loaded WASM plugins (e.g. prowlarr)
//! and exposes them as `Stream` objects ranked by the quality module.
//!
//! This provider does not do its own scraping — it is an adapter that bridges
//! the plugin system output into the unified `Provider::streams()` interface.

use anyhow::Result;
use async_trait::async_trait;
use tracing::debug;

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, SubtitleTrack};
use crate::providers::{Provider, Stream, StreamQuality};

/// Aggregates torrent streams from all loaded stream-capable plugins.
pub struct TorrentProvider;

impl TorrentProvider {
    pub fn new() -> Self { TorrentProvider }
}

impl Default for TorrentProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Provider for TorrentProvider {
    fn name(&self) -> &str { "torrent" }

    fn has_streams(&self) -> bool { true }

    fn supported_tabs(&self) -> Option<Vec<MediaTab>> {
        Some(vec![MediaTab::Movies, MediaTab::Series])
    }

    async fn fetch_trending(&self, _tab: &MediaTab, _page: u32) -> Result<Vec<CatalogEntry>> {
        // Torrent providers don't have a trending feed — they resolve on demand
        Ok(vec![])
    }

    async fn search(&self, _tab: &MediaTab, _query: &str, _page: u32) -> Result<Vec<CatalogEntry>> {
        // Search is handled by the plugin system; this provider only does streams
        Ok(vec![])
    }

    async fn streams(&self, id: &str) -> Result<Vec<Stream>> {
        // In practice this is called by the engine after collecting plugin results.
        // The plugin system (prowlarr, etc.) populates streams; this stub allows
        // the provider to be registered without a plugin.
        debug!("TorrentProvider::streams called for id={id} (no built-in scraper)");
        Ok(vec![])
    }

    async fn subtitles(&self, _id: &str) -> Result<Vec<SubtitleTrack>> {
        Ok(vec![])
    }
}
