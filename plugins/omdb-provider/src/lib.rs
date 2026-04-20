//! omdb-provider — stui plugin for Open Movie Database API.
//!
//! ## API Overview
//!
//! Base URL: https://www.omdbapi.com/
//!
//! Endpoints used:
//!   GET /?t={title}&apikey={key}        → movie/series info by title
//!   GET /?s={search}&apikey={key}      → search results
//!
//! API key: set OMDB_API_KEY env var or add to config.toml.
//! Free tier: 1000 requests/day.
//!
//! ## Plugin Interface
//!
//! This plugin implements the stui search interface:
//!   search(query, scope, page) → returns catalog entries
//!
//! Supported scopes: Movie, Series
//! Empty query → returns empty (OMDB doesn't have trending).
//! Non-empty query → returns search results.

use serde::Deserialize;
use stui_plugin_sdk::prelude::*;
use stui_plugin_sdk::{error_codes, EntryKind, SearchScope};

const BASE_URL: &str = "https://www.omdbapi.com/";

pub struct OmdbProvider {
    api_key: std::sync::OnceLock<String>,
}

impl OmdbProvider {
    pub fn new() -> Self {
        Self {
            api_key: std::sync::OnceLock::new(),
        }
    }
}

impl Default for OmdbProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StuiPlugin for OmdbProvider {
    fn name(&self) -> &str {
        "omdb"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // omdb supports movie and series scopes
        let entry_kind = match req.scope {
            SearchScope::Movie => EntryKind::Movie,
            SearchScope::Series => EntryKind::Series,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "omdb only supports movie and series scopes",
                );
            }
        };

        let api_key = match self.api_key.get() {
            Some(k) => k,
            None => {
                let key = env_or("OMDB_API_KEY", "");
                if key.is_empty() {
                    return PluginResult::err("NO_API_KEY", "OMDB_API_KEY not configured");
                }
                self.api_key.get_or_init(|| key)
            }
        };

        let query = req.query.trim();

        // OMDB doesn't have trending - return empty for empty query
        if query.is_empty() {
            return PluginResult::ok(SearchResponse {
                items: vec![],
                total: 0,
            });
        }

        // Use search endpoint for finding movies
        let encoded = urlencoding::encode(query);
        let url = format!("{BASE_URL}?s={}&apikey={}", encoded, api_key);

        plugin_info!("omdb: searching {}", query);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let search_resp: SearchResponseRaw = match serde_json::from_str(&body) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("omdb: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        if search_resp.response == "False" {
            return PluginResult::ok(SearchResponse {
                items: vec![],
                total: 0,
            });
        }

        let items: Vec<PluginEntry> = search_resp
            .search
            .unwrap_or_default()
            .into_iter()
            .take(req.limit as usize)
            .map(|s| s.into_entry(entry_kind))
            .collect();

        let total = items.len() as u32;
        plugin_info!("omdb: {} entries", items.len());

        PluginResult::ok(SearchResponse { items, total })
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "omdb provider does not resolve streams")
    }
}

fn env_or(var: &str, default: &str) -> String {
    let cache_key = format!("__env:{}", var);
    cache_get(&cache_key).unwrap_or_else(|| default.to_string())
}

// ── API Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponseRaw {
    #[serde(default)]
    search: Option<Vec<SearchResult>>,
    #[serde(default)]
    total_results: Option<u32>,
    #[serde(default)]
    response: String,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(rename = "Title", default)]
    title: String,
    #[serde(rename = "Year", default)]
    year: String,
    #[serde(rename = "imdbID", default)]
    imdb_id: String,
    #[serde(rename = "Type", default)]
    media_type: String,
    #[serde(rename = "Poster", default)]
    poster: String,
}

impl SearchResult {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        // OMDB year field may be "2023" or "2020–2023" for series; parse first part
        let year = self.year
            .split('–')
            .next()
            .and_then(|y| y.trim().parse::<u32>().ok());

        PluginEntry {
            id: self.imdb_id.clone(),
            kind,
            source: "omdb".to_string(),
            title: self.title,
            year,
            genre: None,
            rating: None,
            description: None,
            poster_url: if self.poster != "N/A" {
                Some(self.poster)
            } else {
                None
            },
            imdb_id: if self.imdb_id != "N/A" {
                Some(self.imdb_id)
            } else {
                None
            },
            duration: None,
            ..Default::default()
        }
    }
}

// ── WASM Exports ──────────────────────────────────────────────────────────────

stui_export_plugin!(OmdbProvider);
