//! discogs-provider — stui plugin for Discogs REST API.
//!
//! ## API Overview
//!
//! Discogs provides music data at https://api.discogs.com
//!
//! Endpoints used:
//!   GET /database/search?q={query}&type=release  → search releases
//!   GET /database/search?q={query}&type=artist    → search artists
//!
//! API key required (Personal Access Token from discogs.com/settings/developers)
//!
//! ## Plugin Interface
//!
//! This plugin implements the UPP search interface:
//!   search(query, tab, page) → returns catalog entries
//!
//! Empty query + tab="music" → returns featured/new releases
//! Non-empty query + tab="music" → returns search results

use serde::{Deserialize, Serialize};
use stui_plugin_sdk::prelude::*;

const API_BASE: &str = "https://api.discogs.com";

pub struct DiscogsProvider;

impl DiscogsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DiscogsProvider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for DiscogsProvider {
    fn name(&self) -> &str {
        "discogs"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let tab = req.tab.as_str();
        let query = req.query.trim();

        // Only support music tab
        if tab != "music" {
            return PluginResult::ok(SearchResponse {
                items: vec![],
                total: 0,
            });
        }

        let page = req.page.max(1);
        let per_page = req.limit.min(50) as usize;

        if query.is_empty() {
            self.get_new_releases(page, per_page)
        } else {
            self.search_releases(query, page, per_page)
        }
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "discogs provider does not resolve streams")
    }
}

impl DiscogsProvider {
    fn get_api_key(&self) -> Result<String, String> {
        env_or("DISCOGS_API_KEY", "")
            .or_else(|| cache_get("__config:api_key"))
            .ok_or_else(|| "DISCOGS_API_KEY not set".to_string())
    }

    fn search_releases(
        &self,
        query: &str,
        page: u32,
        per_page: usize,
    ) -> PluginResult<SearchResponse> {
        let api_key = match self.get_api_key() {
            Ok(k) => k,
            Err(e) => return PluginResult::err("CONFIG_ERROR", &e),
        };

        let url = format!(
            "{}/database/search?q={}&type=release&page={}&per_page={}",
            API_BASE,
            url_encode(query),
            page,
            per_page
        );

        plugin_info!("discogs: searching '{}' (page {})", query, page);

        let response = match http_get_auth(&url, &api_key) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let search_resp: SearchResponseWrapper = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("discogs: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let entries: Vec<PluginEntry> = search_resp
            .results
            .into_iter()
            .filter(|r| r.type_ == "release" && r.id > 0)
            .take(per_page)
            .map(|r| r.into_entry())
            .collect();

        let total = search_resp
            .pagination
            .unwrap_or_default()
            .entries
            .unwrap_or(entries.len() as i32) as u32;
        plugin_info!("discogs: {} entries (total: {})", entries.len(), total);

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }

    fn get_new_releases(&self, page: u32, per_page: usize) -> PluginResult<SearchResponse> {
        let api_key = match self.get_api_key() {
            Ok(k) => k,
            Err(e) => return PluginResult::err("CONFIG_ERROR", &e),
        };

        let url = format!(
            "{}/database/search?sort=date_added,desc&type=release&page={}&per_page={}",
            API_BASE, page, per_page
        );

        plugin_info!("discogs: fetching new releases (page {})", page);

        let response = match http_get_auth(&url, &api_key) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let search_resp: SearchResponseWrapper = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("discogs: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let entries: Vec<PluginEntry> = search_resp
            .results
            .into_iter()
            .filter(|r| r.type_ == "release" && r.id > 0)
            .take(per_page)
            .map(|r| r.into_entry())
            .collect();

        let total = entries.len() as u32;
        plugin_info!("discogs: {} new releases", entries.len());

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }
}

// ── API Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponseWrapper {
    #[serde(default)]
    pagination: Option<Pagination>,
    #[serde(default)]
    results: Vec<DiscogsRelease>,
}

#[derive(Debug, Deserialize, Default)]
struct Pagination {
    #[serde(rename = "page", default)]
    page: Option<i32>,
    #[serde(rename = "pages", default)]
    pages: Option<i32>,
    #[serde(rename = "per_page", default)]
    per_page: Option<i32>,
    #[serde(rename = "items", default)]
    entries: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct DiscogsRelease {
    #[serde(default)]
    id: i64,
    #[serde(rename = "type", default)]
    type_: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    format: Vec<String>,
    #[serde(default)]
    label: Vec<String>,
    #[serde(rename = "cover_image", default)]
    cover_image: Option<String>,
    #[serde(rename = "thumb", default)]
    thumb: Option<String>,
    #[serde(rename = "genre", default)]
    genre: Vec<String>,
    #[serde(rename = "style", default)]
    style: Vec<String>,
    #[serde(rename = "resource_url", default)]
    resource_url: Option<String>,
}

impl DiscogsRelease {
    fn into_entry(self) -> PluginEntry {
        let title = self.title;

        // Format year as string if available
        let year = self.year.map(|y| y.to_string());

        // Combine genre and style for genre field
        let mut genres = self.genre.clone();
        genres.extend(self.style);
        let genre = if genres.is_empty() {
            None
        } else {
            Some(genres.into_iter().take(3).collect::<Vec<_>>().join(", "))
        };

        // Use cover_image or thumb
        let poster_url = self.cover_image.or(self.thumb);

        // Build description from format, country, label
        let mut desc_parts = vec![];
        if !self.format.is_empty() {
            desc_parts.push(self.format.join(", "));
        }
        if let Some(ref c) = self.country {
            desc_parts.push(c.clone());
        }
        if !self.label.is_empty() {
            desc_parts.push(format!("Label: {}", self.label.join(", ")));
        }
        let description = if desc_parts.is_empty() {
            None
        } else {
            Some(desc_parts.join(" | "))
        };

        PluginEntry {
            id: format!("discogs-{}", self.id),
            title,
            year,
            genre,
            rating: None,
            description,
            poster_url,
            imdb_id: None,
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────────

fn env_or(var: &str, default: &str) -> Option<String> {
    let cache_key = format!("__env:{}", var);
    cache_get(&cache_key).or_else(|| {
        if default.is_empty() {
            None
        } else {
            Some(default.to_string())
        }
    })
}

fn url_encode(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => vec![c],
            ' ' => vec!['%', '2', '0'],
            c => {
                let mut buf = [0u8; 4];
                let bytes = c.encode_utf8(&mut buf);
                bytes
                    .bytes()
                    .flat_map(|b| {
                        vec![
                            '%',
                            char::from_digit((b >> 4) as u32, 16).unwrap_or('0'),
                            char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'),
                        ]
                    })
                    .collect()
            }
        })
        .collect()
}

/// GET with Discogs auth header
fn http_get_auth(url: &str, api_key: &str) -> Result<String, String> {
    let auth_url = if url.contains('?') {
        format!("{}&key={}", url, api_key)
    } else {
        format!("{}?key={}", url, api_key)
    };
    http_get(&auth_url)
}

// ── WASM Exports ───────────────────────────────────────────────────────────────

stui_export_plugin!(DiscogsProvider);
