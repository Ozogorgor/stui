//! OMDB provider — uses the free OMDB API (omdbapi.com).
//!
//! API key: set OMDB_API_KEY env var or pass via config.
//! Free tier: 1,000 requests/day.
//!
//! Endpoints used:
//!   GET /?apikey=KEY&s=QUERY&type=movie|series&page=N  → search
//!   GET /?apikey=KEY&i=IMDB_ID&plot=short              → details

use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::MediaTab;
use crate::providers::Provider;

const BASE_URL: &str = "https://www.omdbapi.com";

// Curated "trending" seed queries per tab — used on launch to populate
// the grid before a user has typed anything. In a full release these
// would be driven by a curated list or a "trending" endpoint.
const SEED_MOVIES: &[&str] = &["2024 movie", "2025 movie", "blockbuster 2024"];
const SEED_SERIES: &[&str] = &["2024 series", "drama series 2024"];
const SEED_MUSIC:  &[&str] = &[]; // OMDB doesn't cover music

// ── API response types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(rename = "Search")]
    search: Option<Vec<SearchItem>>,
    #[serde(rename = "totalResults")]
    total_results: Option<String>,
    #[serde(rename = "Response")]
    response: String,
    #[serde(rename = "Error")]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    #[serde(rename = "Title")]
    title: String,
    #[serde(rename = "Year")]
    year: String,
    #[serde(rename = "imdbID")]
    imdb_id: String,
    #[serde(rename = "Type")]
    media_type: String,
    #[serde(rename = "Poster")]
    poster: String,
}

#[derive(Debug, Deserialize)]
struct DetailResponse {
    #[serde(rename = "Title")]
    title: String,
    #[serde(rename = "Year")]
    year: String,
    #[serde(rename = "Genre")]
    genre: String,
    #[serde(rename = "imdbRating")]
    imdb_rating: String,
    #[serde(rename = "Plot")]
    plot: String,
    #[serde(rename = "Poster")]
    poster: String,
    #[serde(rename = "imdbID")]
    imdb_id: String,
    #[serde(rename = "Response")]
    response: String,
}

// ── Provider ──────────────────────────────────────────────────────────────────

pub struct OmdbProvider {
    api_key: String,
    client: reqwest::Client,
}

impl OmdbProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("stui-runtime/0.1.0")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");
        Self { api_key: api_key.into(), client }
    }

    /// Try to construct from the OMDB_API_KEY env var.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("OMDB_API_KEY").ok()?;
        if key.is_empty() { return None; }
        Some(Self::new(key))
    }

    /// Construct from config (api_keys.omdb) or fall back to the env var.
    pub fn from_config(api_keys: &crate::config::types::ApiKeysConfig) -> Option<Self> {
        let key = api_keys.omdb.clone()
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("OMDB_API_KEY").ok().filter(|k| !k.is_empty()))?;
        Some(Self::new(key))
    }

    async fn search_query(
        &self,
        query: &str,
        media_type: &str,
        page: u32,
    ) -> Result<Vec<CatalogEntry>> {
        let url = format!(
            "{BASE_URL}/?apikey={}&s={}&type={}&page={}",
            self.api_key,
            urlencoding::encode(query),
            media_type,
            page,
        );
        debug!(provider = "omdb", url = %url, "fetching");

        let resp: SearchResponse = self.client.get(&url).send().await?.json().await?;

        if resp.response == "False" {
            if let Some(err) = &resp.error {
                if err.contains("not found") || err.contains("Too many") {
                    return Ok(vec![]);
                }
                bail!("OMDB error: {}", err);
            }
            return Ok(vec![]);
        }

        let items = resp.search.unwrap_or_default();
        let tab_str = if media_type == "series" { "series" } else { "movies" };

        Ok(items.into_iter().map(|item| {
            let poster_url = if item.poster == "N/A" { None } else { Some(item.poster) };
            CatalogEntry {
                id: item.imdb_id.clone(),
                title: item.title,
                year: Some(item.year),
                genre: None, // populated by detail fetch if needed
                rating: None,
                description: None,
                poster_url,
                poster_art: None,
                provider: "omdb".to_string(),
                tab: tab_str.to_string(),
                imdb_id: Some(item.imdb_id),
                tmdb_id: None,
                media_type: crate::ipc::MediaType::default(),
                ratings: std::collections::HashMap::new(),
            }
        }).collect())
    }
}

#[async_trait]
impl Provider for OmdbProvider {
    fn name(&self) -> &str { "omdb" }
    fn display_name(&self) -> &str { "OMDB" }
    fn description(&self) -> &str { "Open Movie Database — IMDB ratings & plot summaries" }

    fn config_schema(&self) -> Vec<crate::ipc::ProviderField> {
        vec![crate::ipc::ProviderField {
            key:        "api_keys.omdb".to_string(),
            label:      "API Key".to_string(),
            hint:       "Free at omdbapi.com/apikey.aspx".to_string(),
            masked:     true,
            configured: !self.api_key.is_empty(),
        }]
    }

    fn is_active(&self) -> bool { !self.api_key.is_empty() }

    async fn fetch_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>> {
        let (seeds, media_type) = match tab {
            MediaTab::Movies   => (SEED_MOVIES, "movie"),
            MediaTab::Series   => (SEED_SERIES, "series"),
            MediaTab::Music    => return Ok(vec![]),
            MediaTab::Library  => return Ok(vec![]),
            MediaTab::Radio | MediaTab::Podcasts | MediaTab::Videos => return Ok(vec![]),
        };

        let mut all = vec![];
        for seed in seeds {
            match self.search_query(seed, media_type, page).await {
                Ok(mut entries) => all.append(&mut entries),
                Err(e) => warn!(provider = "omdb", seed, error = %e, "seed query failed"),
            }
        }
        Ok(all)
    }

    async fn search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        let media_type = match tab {
            MediaTab::Movies   => "movie",
            MediaTab::Series   => "series",
            MediaTab::Music    => return Ok(vec![]),
            MediaTab::Library  => return Ok(vec![]),
            MediaTab::Radio | MediaTab::Podcasts | MediaTab::Videos => return Ok(vec![]),
        };
        self.search_query(query, media_type, page).await
    }
}
