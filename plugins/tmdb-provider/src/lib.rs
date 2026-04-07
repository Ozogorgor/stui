//! tmdb-provider — stui plugin for The Movie Database API v3.
//!
//! ## API Overview
//!
//! Base URL: https://api.themoviedb.org/3
//!
//! Endpoints used:
//!   GET /trending/movie/week    → trending movies
//!   GET /trending/tv/week       → trending series
//!   GET /search/movie           → movie search
//!   GET /search/tv              → series search
//!
//! API key: set TMDB_API_KEY env var or add to config.toml.
//! Free tier: ~40 requests/10 seconds.
//!
//! ## Plugin Interface
//!
//! This plugin implements the UPP search interface:
//!   search(query, tab, page) → returns catalog entries
//!
//! Empty query + tab → returns trending content for that tab.
//! Non-empty query → returns search results for that tab.

use serde::Deserialize;
use std::sync::OnceLock;
use stui_plugin_sdk::prelude::*;

const BASE_URL: &str = "https://api.themoviedb.org/3";
const POSTER_SIZE: &str = "w342";
const IMAGE_BASE_URL: &str = "https://image.tmdb.org/t/p/";

pub struct TmdbProvider {
    api_key: OnceLock<String>,
}

impl TmdbProvider {
    pub fn new() -> Self {
        Self {
            api_key: OnceLock::new(),
        }
    }
}

impl Default for TmdbProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StuiPlugin for TmdbProvider {
    fn name(&self) -> &str {
        "tmdb"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let api_key = match self.api_key.get() {
            Some(k) => k,
            None => {
                let key = env_or("TMDB_API_KEY", "");
                if key.is_empty() {
                    return PluginResult::err("NO_API_KEY", "TMDB_API_KEY not configured");
                }
                self.api_key.get_or_init(|| key)
            }
        };

        let tab = req.tab.as_str();
        let query = req.query.trim();

        // Determine endpoint: trending if empty query, search otherwise
        let endpoint = if query.is_empty() {
            match tab {
                "movies" => "/trending/movie/week".to_string(),
                "series" => "/trending/tv/week".to_string(),
                _ => "/trending/movie/week".to_string(),
            }
        } else {
            let encoded = urlencoding::encode(query);
            match tab {
                "movies" => format!("/search/movie?query={}", encoded),
                "series" => format!("/search/tv?query={}", encoded),
                _ => format!("/search/movie?query={}", encoded),
            }
        };

        let tab_for_entry = if query.is_empty() {
            match tab {
                "movies" => "movies",
                "series" => "series",
                _ => "movies",
            }
        } else {
            match tab {
                "movies" => "movies",
                "series" => "series",
                _ => "movies",
            }
        };

        let url = format!(
            "{BASE_URL}{}?api_key={}&page={}",
            endpoint,
            api_key,
            req.page.max(1)
        );

        plugin_info!("tmdb: fetching {} (query='{}')", url, query);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let paged: PagedResponse = match serde_json::from_str(&body) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("tmdb: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let items: Vec<PluginEntry> = paged
            .results
            .into_iter()
            .take(req.limit as usize)
            .map(|item| match item {
                SearchResult::Movie(m) => m.into_entry(IMAGE_BASE_URL),
                SearchResult::Tv(t) => t.into_entry(IMAGE_BASE_URL),
            })
            .collect();

        let total = paged.total_results.unwrap_or(items.len() as u32);
        plugin_info!("tmdb: {} entries", items.len());

        PluginResult::ok(SearchResponse { items, total })
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "tmdb provider does not resolve streams")
    }
}

fn env_or(var: &str, default: &str) -> String {
    let cache_key = format!("__env:{}", var);
    cache_get(&cache_key).unwrap_or_else(|| default.to_string())
}

// ── API Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SearchResult {
    Movie(MovieItem),
    Tv(TvItem),
}

#[derive(Debug, Deserialize)]
struct PagedResponse {
    results: Vec<SearchResult>,
    #[serde(default)]
    total_results: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MovieItem {
    id: u64,
    title: String,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    genre_ids: Vec<u32>,
    vote_average: f32,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TvItem {
    id: u64,
    name: String,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    genre_ids: Vec<u32>,
    vote_average: f32,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
}

impl MovieItem {
    fn into_entry(self, image_base: &str) -> PluginEntry {
        let year = self
            .release_date
            .as_deref()
            .and_then(|d| d.split('-').next())
            .map(|y| y.to_string());
        let genre = self.genre_ids.first().map(|&g| genre_name(g).to_string());
        let rating = format!("{:.1}", self.vote_average);
        let poster_url = self
            .poster_path
            .as_deref()
            .map(|p| format!("{}{}{}", image_base, POSTER_SIZE, p));

        PluginEntry {
            id: format!("tmdb-movie-{}", self.id),
            title: self.title,
            year,
            genre,
            rating: Some(rating),
            description: self.overview,
            poster_url,
            imdb_id: None,
            duration: None,
        }
    }
}

impl TvItem {
    fn into_entry(self, image_base: &str) -> PluginEntry {
        let year = self
            .first_air_date
            .as_deref()
            .and_then(|d| d.split('-').next())
            .map(|y| y.to_string());
        let genre = self.genre_ids.first().map(|&g| genre_name(g).to_string());
        let rating = format!("{:.1}", self.vote_average);
        let poster_url = self
            .poster_path
            .as_deref()
            .map(|p| format!("{}{}{}", image_base, POSTER_SIZE, p));

        PluginEntry {
            id: format!("tmdb-tv-{}", self.id),
            title: self.name,
            year,
            genre,
            rating: Some(rating),
            description: self.overview,
            poster_url,
            imdb_id: None,
            duration: None,
        }
    }
}

fn genre_name(id: u32) -> &'static str {
    match id {
        28 => "Action",
        12 => "Adventure",
        16 => "Animation",
        35 => "Comedy",
        80 => "Crime",
        99 => "Documentary",
        18 => "Drama",
        10751 => "Family",
        14 => "Fantasy",
        36 => "History",
        27 => "Horror",
        10402 => "Music",
        9648 => "Mystery",
        10749 => "Romance",
        878 => "Sci-Fi",
        10770 => "TV Movie",
        53 => "Thriller",
        10752 => "War",
        37 => "Western",
        10759 => "Action & Adventure",
        10762 => "Kids",
        10763 => "News",
        10764 => "Reality",
        10765 => "Sci-Fi & Fantasy",
        10766 => "Soap",
        10767 => "Talk",
        10768 => "War & Politics",
        _ => "Other",
    }
}

// ── WASM Exports ──────────────────────────────────────────────────────────────

stui_export_plugin!(TmdbProvider);
