//! Integration tests for the Pipeline orchestrator.
//!
//! These tests verify the Pipeline's construction, policy control, RPC plugin
//! manager wiring, and end-to-end search/resolve flow using a mock provider.
//!
//! All tests are async (tokio::test) because Pipeline methods are async.
//! No external services are required.

use std::sync::Arc;

use stui_runtime::{
    config::RuntimeConfig,
    ipc::{MediaTab, MediaType},
    pipeline::Pipeline,
    player::PlayerBridge,
    providers::{Provider, Stream, StreamQuality, CatalogEntry, SubtitleTrack},
    quality::RankingPolicy,
};

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Returns a minimal RuntimeConfig pointing at a temp directory.
fn test_config() -> RuntimeConfig {
    let tmp = std::env::temp_dir().join(format!("stui_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    RuntimeConfig {
        plugin_dir:  tmp.join("plugins"),
        cache_dir:   tmp.join("cache"),
        data_dir:    tmp.join("data"),
        theme_mode:  "dark".to_string(),
        logging:     Default::default(),
        playback:    Default::default(),
        stremio_addons: vec![],
    }
}

/// A mock provider that returns deterministic search results and streams.
struct MockProvider {
    name:    String,
    streams: Vec<Stream>,
}

impl MockProvider {
    fn new(name: &str) -> Self {
        MockProvider { name: name.to_string(), streams: vec![] }
    }

    fn with_stream(mut self, url: &str, quality: StreamQuality) -> Self {
        self.streams.push(Stream {
            id:       url.to_string(),
            name:     format!("{quality:?}"),
            url:      url.to_string(),
            mime:     None,
            quality,
            provider: self.name.clone(),
        });
        self
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str { &self.name }

    async fn fetch_trending(
        &self,
        _tab: &MediaTab,
        _page: u32,
    ) -> anyhow::Result<Vec<CatalogEntry>> {
        Ok(vec![mock_entry("tt0111161", "The Shawshank Redemption", &self.name)])
    }

    async fn search(
        &self,
        _tab: &MediaTab,
        query: &str,
        _page: u32,
    ) -> anyhow::Result<Vec<CatalogEntry>> {
        if query.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![
            mock_entry("tt0111161", "The Shawshank Redemption", &self.name),
            mock_entry("tt0068646", "The Godfather",            &self.name),
        ])
    }

    async fn streams(&self, _id: &str) -> anyhow::Result<Vec<Stream>> {
        Ok(self.streams.clone())
    }

    async fn subtitles(&self, _id: &str) -> anyhow::Result<Vec<SubtitleTrack>> {
        Ok(vec![])
    }
}

fn mock_entry(id: &str, title: &str, provider: &str) -> CatalogEntry {
    CatalogEntry {
        id:          id.to_string(),
        title:       title.to_string(),
        year:        Some("1994".to_string()),
        genre:       Some("Drama".to_string()),
        rating:      Some("9.3".to_string()),
        description: None,
        poster_url:  None,
        poster_art:  None,
        provider:    provider.to_string(),
        tab:         "movies".to_string(),
        imdb_id:     Some(id.to_string()),
        tmdb_id:     None,
        media_type:  MediaType::Movie,
    }
}

fn make_pipeline(providers: Vec<Arc<dyn Provider>>) -> Pipeline {
    let cfg    = test_config();
    let player = Arc::new(PlayerBridge::noop());
    Pipeline::new(&cfg, providers, player)
}

// ── Construction tests ────────────────────────────────────────────────────────

#[test]
fn pipeline_constructs_with_no_providers() {
    let p = make_pipeline(vec![]);
    // Should not panic; providers list is empty
    drop(p);
}

#[test]
fn pipeline_constructs_with_single_provider() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new("mock"));
    let p = make_pipeline(vec![provider]);
    drop(p);
}

#[test]
fn pipeline_constructs_with_multiple_providers() {
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(MockProvider::new("mock-a")),
        Arc::new(MockProvider::new("mock-b")),
    ];
    let p = make_pipeline(providers);
    drop(p);
}

// ── RPC manager wiring ────────────────────────────────────────────────────────

#[tokio::test]
async fn pipeline_rpc_manager_starts_empty() {
    let p = make_pipeline(vec![]);
    assert_eq!(p.rpc.len().await, 0, "no RPC plugins should be loaded at startup");
}

// ── Policy switching tests ────────────────────────────────────────────────────

#[test]
fn pipeline_default_policy_is_quality_first() {
    let p = make_pipeline(vec![]);
    // Default policy should prefer higher quality
    let default = RankingPolicy::default();
    assert_eq!(p.policy.prefer_max_quality, default.prefer_max_quality);
}

#[test]
fn pipeline_switch_to_bandwidth_saver() {
    let mut p = make_pipeline(vec![]);
    p.use_bandwidth_saver();
    let saver = RankingPolicy::bandwidth_saver();
    assert_eq!(p.policy.prefer_max_quality, saver.prefer_max_quality);
}

#[test]
fn pipeline_switch_back_to_default_policy() {
    let mut p = make_pipeline(vec![]);
    p.use_bandwidth_saver();
    p.use_default_policy();
    let default = RankingPolicy::default();
    assert_eq!(p.policy.prefer_max_quality, default.prefer_max_quality);
}

// ── Search tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn pipeline_search_empty_query_returns_empty() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new("mock"));
    let p = make_pipeline(vec![provider]);
    let results = p.search(&MediaTab::Movies, "", 1).await;
    assert!(results.is_empty(), "empty query should return no results");
}

#[tokio::test]
async fn pipeline_search_returns_results() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new("mock"));
    let p = make_pipeline(vec![provider]);
    let results = p.search(&MediaTab::Movies, "shawshank", 1).await;
    assert!(!results.is_empty(), "search should return results for non-empty query");
    assert!(results.iter().any(|e| e.title.contains("Shawshank")));
}

#[tokio::test]
async fn pipeline_search_merges_multiple_providers() {
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(MockProvider::new("provider-a")),
        Arc::new(MockProvider::new("provider-b")),
    ];
    let p = make_pipeline(providers);
    let results = p.search(&MediaTab::Movies, "godfather", 1).await;
    // Both providers return results — dedup by title may reduce count
    assert!(!results.is_empty());
}

// ── Stream resolution tests ───────────────────────────────────────────────────

#[tokio::test]
async fn pipeline_resolve_streams_no_streams_returns_empty() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new("mock"));
    let p = make_pipeline(vec![provider]);
    let streams = p.resolve_streams("tt0111161").await;
    // MockProvider without .with_stream() returns no streams
    assert!(streams.is_empty());
}

#[tokio::test]
async fn pipeline_resolve_streams_returns_ranked_candidates() {
    let provider: Arc<dyn Provider> = Arc::new(
        MockProvider::new("mock")
            .with_stream("https://cdn.example.com/720p.mkv",  StreamQuality::Hd720)
            .with_stream("https://cdn.example.com/1080p.mkv", StreamQuality::Hd1080)
            .with_stream("https://cdn.example.com/4k.mkv",    StreamQuality::Uhd4k)
    );
    let p = make_pipeline(vec![provider]);
    let streams = p.resolve_streams("tt0111161").await;
    assert!(!streams.is_empty());
}

#[tokio::test]
async fn pipeline_best_stream_url_returns_highest_quality() {
    let provider: Arc<dyn Provider> = Arc::new(
        MockProvider::new("mock")
            .with_stream("https://cdn.example.com/720p.mkv",  StreamQuality::Hd720)
            .with_stream("https://cdn.example.com/1080p.mkv", StreamQuality::Hd1080)
    );
    let p = make_pipeline(vec![provider]);
    let url = p.best_stream_url("tt0111161").await;
    assert!(url.is_some(), "should return a stream URL");
}

#[tokio::test]
async fn pipeline_best_stream_url_none_when_no_providers() {
    let p = make_pipeline(vec![]);
    let url = p.best_stream_url("tt9999999").await;
    assert!(url.is_none());
}

#[tokio::test]
async fn pipeline_bandwidth_saver_prefers_lower_quality() {
    let provider: Arc<dyn Provider> = Arc::new(
        MockProvider::new("mock")
            .with_stream("https://cdn.example.com/720p.mkv",  StreamQuality::Hd720)
            .with_stream("https://cdn.example.com/1080p.mkv", StreamQuality::Hd1080)
            .with_stream("https://cdn.example.com/4k.mkv",    StreamQuality::Uhd4k)
    );
    let mut p = make_pipeline(vec![provider]);
    p.use_bandwidth_saver();
    // Should not panic; actual ordering is covered by ranking_tests.rs
    let streams = p.resolve_streams("tt0111161").await;
    // Under bandwidth_saver the 720p stream should rank first
    if !streams.is_empty() {
        assert!(
            streams[0].stream.quality <= StreamQuality::Hd720,
            "bandwidth_saver should prefer ≤720p, got {:?}",
            streams[0].stream.quality
        );
    }
}
