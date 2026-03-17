//! AniList provider — anime catalog via the AniList GraphQL API.
//!
//! Endpoint: `POST https://graphql.anilist.co`
//!
//! No API key required — all queries use the public read-only endpoint.
//! Rate limit: 90 requests per minute (resets per 60 s window).
//!
//! Tabs served: `Series` (anime shows), `Movies` (anime films).

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::{MediaTab, MediaType};
use crate::providers::Provider;

const ENDPOINT: &str = "https://graphql.anilist.co";

// ── GraphQL queries ───────────────────────────────────────────────────────────

const TRENDING_QUERY: &str = r#"
query ($page: Int, $perPage: Int, $type: MediaType) {
  Page(page: $page, perPage: $perPage) {
    media(type: $type, sort: TRENDING_DESC) {
      id
      title { romaji english }
      coverImage { large }
      description(asHtml: false)
      averageScore
      genres
      startDate { year }
      status
    }
  }
}
"#;

const SEARCH_QUERY: &str = r#"
query ($search: String, $page: Int, $perPage: Int, $type: MediaType) {
  Page(page: $page, perPage: $perPage) {
    media(search: $search, type: $type) {
      id
      title { romaji english }
      coverImage { large }
      description(asHtml: false)
      averageScore
      genres
      startDate { year }
      status
    }
  }
}
"#;

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GqlRequest<'a, V: Serialize> {
    query:     &'a str,
    variables: V,
}

#[derive(Debug, Serialize)]
struct TrendingVars {
    page:     u32,
    #[serde(rename = "perPage")]
    per_page: u32,
    #[serde(rename = "type")]
    media_type: &'static str,
}

#[derive(Debug, Serialize)]
struct SearchVars<'a> {
    search:   &'a str,
    page:     u32,
    #[serde(rename = "perPage")]
    per_page: u32,
    #[serde(rename = "type")]
    media_type: &'static str,
}

#[derive(Debug, Deserialize)]
struct GqlResponse {
    data: Option<GqlData>,
}

#[derive(Debug, Deserialize)]
struct GqlData {
    #[serde(rename = "Page")]
    page: PageData,
}

#[derive(Debug, Deserialize)]
struct PageData {
    media: Vec<AniListMedia>,
}

#[derive(Debug, Deserialize)]
struct AniListMedia {
    id:           u64,
    title:        MediaTitle,
    #[serde(rename = "coverImage")]
    cover_image:  Option<CoverImage>,
    description:  Option<String>,
    #[serde(rename = "averageScore")]
    average_score: Option<u32>,
    genres:       Option<Vec<String>>,
    #[serde(rename = "startDate")]
    start_date:   Option<StartDate>,
}

#[derive(Debug, Deserialize)]
struct MediaTitle {
    romaji:  Option<String>,
    english: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    large: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StartDate {
    year: Option<u32>,
}

// ── Provider ──────────────────────────────────────────────────────────────────

pub struct AniListProvider {
    client: Client,
}

impl AniListProvider {
    pub fn new() -> Self {
        AniListProvider {
            client: Client::builder()
                .user_agent(concat!("stui/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
        }
    }

    fn media_to_entry(&self, m: AniListMedia, tab: &'static str, mtype: MediaType) -> CatalogEntry {
        let title = m.title.english
            .filter(|t| !t.is_empty())
            .or(m.title.romaji)
            .unwrap_or_else(|| format!("AniList #{}", m.id));

        let year  = m.start_date.and_then(|d| d.year).map(|y| y.to_string());
        let genre = m.genres.as_deref()
            .and_then(|g| g.first())
            .map(|s| s.clone());
        let rating = m.average_score.map(|s| format!("{:.1}", s as f64 / 10.0));
        let poster_url = m.cover_image.and_then(|c| c.large);

        // Strip HTML tags from description (AniList occasionally leaks them even
        // with asHtml:false on older entries).
        let description = m.description.map(|d| {
            d.replace("<br>", " ").replace("<br/>", " ").replace("<i>", "").replace("</i>", "")
        });

        CatalogEntry {
            id:          format!("anilist-{}", m.id),
            title,
            year,
            genre,
            rating,
            description,
            poster_url,
            poster_art:  None,
            provider:    "anilist".to_string(),
            tab:         tab.to_string(),
            imdb_id:     None,
            tmdb_id:     None,
            media_type:  mtype,
            ratings:     std::collections::HashMap::new(),
        }
    }

    async fn gql_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>> {
        let (al_type, tab_str, mtype) = match tab {
            MediaTab::Movies => ("ANIME", "movies", MediaType::Movie),
            MediaTab::Series => ("ANIME", "series", MediaType::Series),
            _ => return Ok(vec![]),
        };

        let body = GqlRequest {
            query:     TRENDING_QUERY,
            variables: TrendingVars { page, per_page: 25, media_type: al_type },
        };
        debug!(provider = "anilist", tab = tab_str, page, "fetching trending");

        let resp: GqlResponse = self.client
            .post(ENDPOINT)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        let media = resp.data
            .map(|d| d.page.media)
            .unwrap_or_default();

        Ok(media.into_iter().map(|m| self.media_to_entry(m, tab_str, mtype.clone())).collect())
    }

    async fn gql_search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        let (al_type, tab_str, mtype) = match tab {
            MediaTab::Movies => ("ANIME", "movies", MediaType::Movie),
            MediaTab::Series => ("ANIME", "series", MediaType::Series),
            _ => return Ok(vec![]),
        };

        let body = GqlRequest {
            query:     SEARCH_QUERY,
            variables: SearchVars { search: query, page, per_page: 25, media_type: al_type },
        };
        debug!(provider = "anilist", tab = tab_str, q = query, page, "searching");

        let resp: GqlResponse = self.client
            .post(ENDPOINT)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        let media = resp.data
            .map(|d| d.page.media)
            .unwrap_or_default();

        Ok(media.into_iter().map(|m| self.media_to_entry(m, tab_str, mtype.clone())).collect())
    }
}

impl Default for AniListProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Provider for AniListProvider {
    fn name(&self) -> &str { "anilist" }
    fn display_name(&self) -> &str { "AniList" }
    fn description(&self) -> &str { "AniList GraphQL — anime catalog, ratings & art, no API key needed" }

    fn supported_tabs(&self) -> Option<Vec<MediaTab>> {
        Some(vec![MediaTab::Movies, MediaTab::Series])
    }

    async fn fetch_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>> {
        match self.gql_trending(tab, page).await {
            Ok(v) => Ok(v),
            Err(e) => {
                warn!(provider = "anilist", error = %e, "trending fetch failed");
                Ok(vec![])
            }
        }
    }

    async fn search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        match self.gql_search(tab, query, page).await {
            Ok(v) => Ok(v),
            Err(e) => {
                warn!(provider = "anilist", error = %e, "search failed");
                Ok(vec![])
            }
        }
    }
}
