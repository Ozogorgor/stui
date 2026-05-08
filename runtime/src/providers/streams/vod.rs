//! VOD service bridge — future home for Jellyfin, Plex, Emby, etc.
//!
//! These services expose HTTP APIs that return direct video URLs.  The bridge
//! authenticates with the service, resolves the item ID to a transcoded or
//! direct-play URL, and returns it as a `Stream`.
//!
//! Currently a stub — no built-in VOD service is implemented.
//! Community VOD adapters can be added as WASM plugins.

#![allow(dead_code)]

use anyhow::Result;
use async_trait::async_trait;

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, SubtitleTrack};
use crate::providers::{Provider, Stream};

pub struct VodProvider;

impl VodProvider {
    pub fn new() -> Self {
        VodProvider
    }
}

impl Default for VodProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for VodProvider {
    fn name(&self) -> &str {
        "vod"
    }

    fn has_streams(&self) -> bool {
        false
    } // stub — no streams yet

    async fn fetch_trending(&self, _tab: &MediaTab, _page: u32) -> Result<Vec<CatalogEntry>> {
        Ok(vec![])
    }

    async fn search(&self, _tab: &MediaTab, _query: &str, _page: u32) -> Result<Vec<CatalogEntry>> {
        Ok(vec![])
    }

    async fn streams(&self, _id: &str) -> Result<Vec<Stream>> {
        // TODO: Jellyfin / Plex / Emby bridges
        Ok(vec![])
    }

    async fn subtitles(&self, _id: &str) -> Result<Vec<SubtitleTrack>> {
        Ok(vec![])
    }
}
