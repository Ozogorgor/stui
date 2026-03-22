//! Direct HTTP stream provider.
//!
//! Handles streams that are plain HTTP URLs — either direct video files
//! (CDN-hosted .mkv/.mp4) or URLs that yt-dlp can resolve (YouTube,
//! Vimeo, Twitch VODs, etc.).
//!
//! The provider inspects the URL and classifies it:
//!   - Ends with .mkv/.mp4/.webm → direct progressive HTTP
//!   - Otherwise → hand to yt-dlp inside mpv

#![allow(dead_code)]

use anyhow::Result;
use async_trait::async_trait;

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, SubtitleTrack};
use crate::providers::{Provider, Stream, StreamQuality};

pub struct DirectProvider;

impl DirectProvider {
    pub fn new() -> Self { DirectProvider }
}

impl Default for DirectProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Provider for DirectProvider {
    fn name(&self) -> &str { "direct" }

    fn has_streams(&self) -> bool { true }

    async fn fetch_trending(&self, _tab: &MediaTab, _page: u32) -> Result<Vec<CatalogEntry>> {
        Ok(vec![])
    }

    async fn search(&self, _tab: &MediaTab, _query: &str, _page: u32) -> Result<Vec<CatalogEntry>> {
        Ok(vec![])
    }

    /// Wrap a known HTTP URL as a Stream.
    /// Called by the engine when a plugin resolve returns a direct URL.
    async fn streams(&self, id: &str) -> Result<Vec<Stream>> {
        if !id.starts_with("http://") && !id.starts_with("https://") {
            return Ok(vec![]);
        }

        let mime = infer_mime(id);
        let quality = infer_quality(id);

        Ok(vec![Stream {
            id:       id.to_string(),
            name:     url_label(id),
            url:      id.to_string(),
            mime:     Some(mime),
            quality,
            provider: "direct".to_string(),
            ..Default::default()
        }])
    }

    async fn subtitles(&self, _id: &str) -> Result<Vec<SubtitleTrack>> {
        Ok(vec![])
    }
}

fn infer_mime(url: &str) -> String {
    let u = url.to_lowercase();
    if u.ends_with(".mkv")  { return "video/x-matroska".into(); }
    if u.ends_with(".mp4")  { return "video/mp4".into(); }
    if u.ends_with(".webm") { return "video/webm".into(); }
    if u.ends_with(".avi")  { return "video/x-msvideo".into(); }
    "video/*".into()
}

fn infer_quality(url: &str) -> StreamQuality {
    let u = url.to_uppercase();
    if u.contains("4K") || u.contains("2160") { return StreamQuality::Uhd4k; }
    if u.contains("1080")                      { return StreamQuality::Hd1080; }
    if u.contains("720")                       { return StreamQuality::Hd720; }
    StreamQuality::Unknown
}

fn url_label(url: &str) -> String {
    // Use the last path segment without query string as a label
    url.split('?').next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_string()
}
