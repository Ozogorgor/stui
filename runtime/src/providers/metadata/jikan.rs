//! Jikan provider — anime catalog via the Jikan v4 REST API.
//!
//! Jikan is an unofficial MyAnimeList (MAL) REST wrapper.
//! Docs: https://docs.api.jikan.moe/
//!
//! No API key required. Rate limit: ~3 req/s (per IP, enforced server-side).
//!
//! Tabs served: `Series` (TV/OVA/ONA), `Movies` (anime films).
//!
//! AniList vs Jikan: AniList returns richer staff/studio data; Jikan
//! complements it with MAL scores, episode counts, and recommendations
//! that AniList doesn't expose directly.

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, MediaType};
use crate::providers::Provider;

const BASE_URL: &str = "https://api.jikan.moe/v4";

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JikanPage<T> {
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct AnimeItem {
    mal_id:       u64,
    title:        String,
    title_english: Option<String>,
    images:       AnimeImages,
    synopsis:     Option<String>,
    score:        Option<f64>,
    genres:       Option<Vec<Genre>>,
    year:         Option<u32>,
    /// "TV" | "Movie" | "OVA" | "Special" | "ONA" | "Music"
    #[serde(rename = "type")]
    media_kind:   Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnimeImages {
    jpg: ImageSet,
}

#[derive(Debug, Deserialize)]
struct ImageSet {
    large_image_url: Option<String>,
    image_url:       Option<String>,
}

#[derive(Debug, Deserialize)]
struct Genre {
    name: String,
}

// ── Provider ──────────────────────────────────────────────────────────────────

pub struct JikanProvider {
    client: Client,
}

impl JikanProvider {
    pub fn new() -> Self {
        JikanProvider {
            client: Client::builder()
                .user_agent(concat!("stui/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
        }
    }

    fn item_to_entry(&self, item: AnimeItem, tab_str: &'static str) -> CatalogEntry {
        let title = item.title_english
            .filter(|t| !t.is_empty())
            .unwrap_or(item.title);

        let poster_url = item.images.jpg.large_image_url
            .or(item.images.jpg.image_url);

        let genre = item.genres
            .as_deref()
            .and_then(|g| g.first())
            .map(|g| g.name.clone());

        let rating = item.score.map(|s| format!("{:.1}", s));
        let year   = item.year.map(|y| y.to_string());

        let mtype = match item.media_kind.as_deref() {
            Some("Movie") => MediaType::Movie,
            _             => MediaType::Series,
        };

        CatalogEntry {
            id:          format!("mal-{}", item.mal_id),
            title,
            year,
            genre,
            rating,
            description: item.synopsis,
            poster_url,
            poster_art:  None,
            provider:    "jikan".to_string(),
            tab:         tab_str.to_string(),
            imdb_id:     None,
            tmdb_id:     None,
            media_type:  mtype,
            ratings:     std::collections::HashMap::new(),
        }
    }
}

impl Default for JikanProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Provider for JikanProvider {
    fn name(&self) -> &str { "jikan" }
    fn display_name(&self) -> &str { "Jikan (MAL)" }
    fn description(&self) -> &str { "MyAnimeList via Jikan — MAL scores, episode counts, no API key" }

    fn supported_tabs(&self) -> Option<Vec<MediaTab>> {
        Some(vec![MediaTab::Movies, MediaTab::Series])
    }

    async fn fetch_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>> {
        let (type_filter, tab_str) = match tab {
            MediaTab::Movies => ("movie", "movies"),
            MediaTab::Series => ("tv",    "series"),
            _ => return Ok(vec![]),
        };

        let url = format!("{BASE_URL}/top/anime?type={type_filter}&page={page}");
        debug!(provider = "jikan", tab = tab_str, page, "fetching top anime");

        let resp: JikanPage<AnimeItem> = match self.client
            .get(&url)
            .send()
            .await?
            .json()
            .await
        {
            Ok(r)  => r,
            Err(e) => {
                warn!(provider = "jikan", error = %e, "trending parse failed");
                return Ok(vec![]);
            }
        };

        Ok(resp.data.into_iter().map(|item| self.item_to_entry(item, tab_str)).collect())
    }

    async fn search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        let tab_str = match tab {
            MediaTab::Movies => "movies",
            MediaTab::Series => "series",
            _ => return Ok(vec![]),
        };

        let q   = urlencoding::encode(query);
        let url = format!("{BASE_URL}/anime?q={q}&page={page}");
        debug!(provider = "jikan", q = query, page, "searching");

        let resp: JikanPage<AnimeItem> = match self.client
            .get(&url)
            .send()
            .await?
            .json()
            .await
        {
            Ok(r)  => r,
            Err(e) => {
                warn!(provider = "jikan", error = %e, "search parse failed");
                return Ok(vec![]);
            }
        };

        Ok(resp.data.into_iter().map(|item| self.item_to_entry(item, tab_str)).collect())
    }
}
