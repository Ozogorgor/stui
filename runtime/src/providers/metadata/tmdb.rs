//! TMDB provider — uses The Movie Database API v3 (themoviedb.org).
//!
//! API key: set TMDB_API_KEY env var.
//! Free tier: ~1M requests/month.
//!
//! Endpoints used:
//!   GET /trending/movie/week    → trending movies
//!   GET /trending/tv/week       → trending series
//!   GET /search/movie           → movie search
//!   GET /search/tv              → series search
//!   GET /configuration          → image base URL

use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::OnceCell;
use tracing::{debug, warn};

use crate::catalog::CatalogEntry;
use crate::ipc::MediaTab;
use crate::providers::Provider;

const BASE_URL: &str = "https://api.themoviedb.org/3";
// TMDB standard poster sizes: w92, w154, w185, w342, w500, w780, original
const POSTER_SIZE: &str = "w342";

// ── API types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PagedResponse<T> {
    results: Vec<T>,
    total_results: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MovieItem {
    id: u64,
    title: String,
    release_date: Option<String>,
    genre_ids: Vec<u32>,
    vote_average: f32,
    overview: Option<String>,
    poster_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TvItem {
    id: u64,
    name: String,
    first_air_date: Option<String>,
    genre_ids: Vec<u32>,
    vote_average: f32,
    overview: Option<String>,
    poster_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigResponse {
    images: ImageConfig,
}

#[derive(Debug, Deserialize)]
struct ImageConfig {
    secure_base_url: String,
}

// ── Genre map ────────────────────────────────────────────────────────────────

fn genre_name(id: u32) -> &'static str {
    match id {
        28   => "Action",
        12   => "Adventure",
        16   => "Animation",
        35   => "Comedy",
        80   => "Crime",
        99   => "Documentary",
        18   => "Drama",
        10751 => "Family",
        14   => "Fantasy",
        36   => "History",
        27   => "Horror",
        10402 => "Music",
        9648 => "Mystery",
        10749 => "Romance",
        878  => "Sci-Fi",
        10770 => "TV Movie",
        53   => "Thriller",
        10752 => "War",
        37   => "Western",
        // TV genres
        10759 => "Action & Adventure",
        10762 => "Kids",
        10763 => "News",
        10764 => "Reality",
        10765 => "Sci-Fi & Fantasy",
        10766 => "Soap",
        10767 => "Talk",
        10768 => "War & Politics",
        _    => "Other",
    }
}

// ── Provider ──────────────────────────────────────────────────────────────────

pub struct TmdbProvider {
    api_key: String,
    client: reqwest::Client,
    image_base: OnceCell<String>,
}

impl TmdbProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("stui-runtime/0.1.0")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("HTTP client build failed");
        Self {
            api_key: api_key.into(),
            client,
            image_base: OnceCell::new(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let key = std::env::var("TMDB_API_KEY").ok()?;
        if key.is_empty() { return None; }
        Some(Self::new(key))
    }

    /// Construct from config (api_keys.tmdb) or fall back to the env var.
    pub fn from_config(api_keys: &crate::config::types::ApiKeysConfig) -> Option<Self> {
        let key = api_keys.tmdb.clone()
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("TMDB_API_KEY").ok().filter(|k| !k.is_empty()))?;
        Some(Self::new(key))
    }

    async fn image_base_url(&self) -> &str {
        self.image_base
            .get_or_init(|| async {
                let url = format!("{BASE_URL}/configuration?api_key={}", self.api_key);
                match self.client.get(&url).send().await {
                    Ok(r) => match r.json::<ConfigResponse>().await {
                        Ok(cfg) => cfg.images.secure_base_url,
                        Err(_) => "https://image.tmdb.org/t/p/".to_string(),
                    },
                    Err(_) => "https://image.tmdb.org/t/p/".to_string(),
                }
            })
            .await
    }

    fn poster_url_for(&self, base: &str, path: &str) -> String {
        format!("{}{}{}", base, POSTER_SIZE, path)
    }

    async fn fetch_movies(&self, endpoint: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        let url = format!("{BASE_URL}{endpoint}?api_key={}&page={}", self.api_key, page);
        debug!(provider = "tmdb", url = %url, "fetching");

        let resp: PagedResponse<MovieItem> = self.client.get(&url).send().await?.json().await?;
        let base = self.image_base_url().await;

        Ok(resp.results.into_iter().map(|m| {
            let year = m.release_date.as_deref()
                .and_then(|d| d.split('-').next())
                .map(|y| y.to_string());
            let genre = m.genre_ids.first().map(|&g| genre_name(g).to_string());
            let rating = format!("{:.1}", m.vote_average);
            let poster_url = m.poster_path.as_deref().map(|p| self.poster_url_for(base, p));

            CatalogEntry {
                id: format!("tmdb-movie-{}", m.id),
                title: m.title,
                year,
                genre,
                rating: Some(rating),
                description: m.overview,
                poster_url,
                poster_art: None,
                provider: "tmdb".to_string(),
                tab: "movies".to_string(),
                imdb_id: None,
                tmdb_id: Some(m.id),
                media_type: crate::ipc::MediaType::Movie,
                ratings: std::collections::HashMap::new(),
            }
        }).collect())
    }

    async fn fetch_tv(&self, endpoint: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        let url = format!("{BASE_URL}{endpoint}?api_key={}&page={}", self.api_key, page);
        debug!(provider = "tmdb", url = %url, "fetching");

        let resp: PagedResponse<TvItem> = self.client.get(&url).send().await?.json().await?;
        let base = self.image_base_url().await;

        Ok(resp.results.into_iter().map(|t| {
            let year = t.first_air_date.as_deref()
                .and_then(|d| d.split('-').next())
                .map(|y| y.to_string());
            let genre = t.genre_ids.first().map(|&g| genre_name(g).to_string());
            let rating = format!("{:.1}", t.vote_average);
            let poster_url = t.poster_path.as_deref().map(|p| self.poster_url_for(base, p));

            CatalogEntry {
                id: format!("tmdb-tv-{}", t.id),
                title: t.name,
                year,
                genre,
                rating: Some(rating),
                description: t.overview,
                poster_url,
                poster_art: None,
                provider: "tmdb".to_string(),
                tab: "series".to_string(),
                imdb_id: None,
                tmdb_id: Some(t.id),
                media_type: crate::ipc::MediaType::Series,
                ratings: std::collections::HashMap::new(),
            }
        }).collect())
    }
}

#[async_trait]
impl Provider for TmdbProvider {
    fn name(&self) -> &str { "tmdb" }
    fn display_name(&self) -> &str { "TMDB" }
    fn description(&self) -> &str { "The Movie Database — movies & TV metadata, posters, ratings" }

    fn config_schema(&self) -> Vec<crate::ipc::ProviderField> {
        vec![crate::ipc::ProviderField {
            key:        "api_keys.tmdb".to_string(),
            label:      "API Key".to_string(),
            hint:       "Free at themoviedb.org/settings/api".to_string(),
            masked:     true,
            configured: !self.api_key.is_empty(),
        }]
    }

    fn is_active(&self) -> bool { !self.api_key.is_empty() }

    async fn fetch_trending(&self, tab: &MediaTab, page: u32) -> Result<Vec<CatalogEntry>> {
        match tab {
            MediaTab::Movies   => self.fetch_movies("/trending/movie/week", page).await,
            MediaTab::Series   => self.fetch_tv("/trending/tv/week", page).await,
            MediaTab::Music    => Ok(vec![]),
            MediaTab::Library  => Ok(vec![]),
            MediaTab::Radio | MediaTab::Podcasts | MediaTab::Videos => Ok(vec![]),
        }
    }

    async fn search(&self, tab: &MediaTab, query: &str, page: u32) -> Result<Vec<CatalogEntry>> {
        let q = urlencoding::encode(query);
        match tab {
            MediaTab::Movies =>
                self.fetch_movies(&format!("/search/movie?query={q}&"), page).await,
            MediaTab::Series =>
                self.fetch_tv(&format!("/search/tv?query={q}&"), page).await,
            _ => Ok(vec![]),
        }
    }
}
