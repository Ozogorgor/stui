//! HTTP client for Stremio addon resource endpoints.

use anyhow::{Context, Result};
use reqwest::Client;
use tracing::debug;
use urlencoding::encode;

use super::manifest::{
    StremioManifest, StremioCatalogResponse, StremioStreamResponse, StremioSubtitleResponse,
};

/// HTTP client bound to one addon's base URL.
#[allow(dead_code)]
#[derive(Clone)]
pub struct StremioClient {
    http:     Client,
    base_url: String,
}

impl StremioClient {
    /// Create a client from a manifest URL like
    /// `https://torrentio.strem.fun/manifest.json`
    pub fn from_manifest_url(manifest_url: &str) -> Self {
        let base_url = manifest_url
            .trim_end_matches("/manifest.json")
            .to_string();
        StremioClient {
            http:     Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .user_agent("stui/0.1")
                .build()
                .expect("reqwest client"),
            base_url,
        }
    }

    #[allow(dead_code)]
    pub fn base_url(&self) -> &str { &self.base_url }

    // ── Manifest ──────────────────────────────────────────────────────────

    pub async fn fetch_manifest(&self) -> Result<StremioManifest> {
        let url = format!("{}/manifest.json", self.base_url);
        debug!("stremio: GET {url}");
        self.http
            .get(&url)
            .send().await
            .context("manifest fetch")?
            .json().await
            .context("manifest parse")
    }

    // ── Catalog ───────────────────────────────────────────────────────────

    /// Fetch a catalog listing: `/catalog/{type}/{id}.json`
    #[allow(dead_code)]
    pub async fn catalog(
        &self,
        media_type: &str, // "movie" | "series"
        catalog_id: &str,
        extra:      &[(&str, &str)],
    ) -> Result<StremioCatalogResponse> {
        let extra_str = if extra.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = extra.iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect();
            format!("/{}", parts.join("&"))
        };

        let url = format!(
            "{}/catalog/{}/{}{}.json",
            self.base_url, media_type, catalog_id, extra_str
        );
        debug!("stremio: GET {url}");
        self.http.get(&url).send().await?.json().await
            .context("catalog parse")
    }

    // ── Streams ───────────────────────────────────────────────────────────

    /// Fetch streams for an item: `/stream/{type}/{id}.json`
    /// `id` is typically the IMDB id: `tt0816692`
    pub async fn streams(
        &self,
        media_type: &str,
        item_id:    &str,
    ) -> Result<StremioStreamResponse> {
        let url = format!("{}/stream/{}/{}.json", self.base_url, media_type, item_id);
        debug!("stremio: GET {url}");
        self.http.get(&url).send().await?.json().await
            .context("stream parse")
    }

    // ── Subtitles ─────────────────────────────────────────────────────────

    /// Fetch subtitle tracks: `/subtitles/{type}/{id}.json`
    #[allow(dead_code)]
    pub async fn subtitles(
        &self,
        media_type: &str,
        item_id:    &str,
    ) -> Result<StremioSubtitleResponse> {
        let url = format!("{}/subtitles/{}/{}.json", self.base_url, media_type, item_id);
        debug!("stremio: GET {url}");
        self.http.get(&url).send().await?.json().await
            .context("subtitle parse")
    }
}
