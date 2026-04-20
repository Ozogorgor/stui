//! kitsu — stui plugin for Kitsu JSON:API.
//!
//! ## API Overview
//!
//! Kitsu uses REST JSON:API at https://kitsu.io/api/edge
//!
//! Endpoints:
//!   GET /anime?filter[slug]={slug}    → single anime
//!   GET /anime?filter[text]={query}  → search
//!   GET /trending/anime               → trending
//!
//! No API key required for basic usage (rate limited).
//! Optional API key for higher limits.
//!
//! ## Plugin Interface
//!
//! This plugin implements:
//!   search(query, scope, page) → returns anime entries
//!   resolve(entry_id)          → not supported (returns error)
//!
//! Empty query + scope Movie/Series → returns trending anime.
//! Non-empty query → returns search results.

use serde::Deserialize;
use stui_plugin_sdk::prelude::*;
use stui_plugin_sdk::{error_codes, EntryKind, SearchScope};

const API_BASE: &str = "https://kitsu.io/api/edge";

pub struct KitsuProvider;

impl KitsuProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KitsuProvider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for KitsuProvider {
    fn name(&self) -> &str {
        "kitsu"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // Kitsu covers anime: series and movies
        let entry_kind = match req.scope {
            SearchScope::Movie => EntryKind::Movie,
            SearchScope::Series => EntryKind::Series,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "kitsu only supports movie and series scopes",
                );
            }
        };

        let query = req.query.trim();
        let page = req.page.max(1);
        let per_page = req.limit.min(20).max(1) as u32;
        let offset = (page - 1).saturating_mul(per_page);

        let api_key = cache_get("__config:api_key");

        let url = if query.is_empty() {
            // Use the dedicated trending endpoint on page 1; fall back to
            // popularity sort for subsequent pages (trending has no pagination).
            if page == 1 {
                format!("{}/trending/anime?limit={}", API_BASE, per_page)
            } else {
                format!(
                    "{}/anime?page[limit]={}&page[offset]={}&sort=-userCount",
                    API_BASE, per_page, offset
                )
            }
        } else {
            format!(
                "{}/anime?filter[text]={}&page[limit]={}&page[offset]={}",
                API_BASE,
                url_encode(query),
                per_page,
                offset
            )
        };

        plugin_info!("kitsu: searching '{}' (page {})", query, page);

        let response = match api_key {
            Some(key) => match http_get_with_bearer(&url, &key) {
                Ok(r) => r,
                Err(e) => return PluginResult::err("HTTP_ERROR", &e),
            },
            None => match http_get(&url) {
                Ok(r) => r,
                Err(e) => return PluginResult::err("HTTP_ERROR", &e),
            },
        };

        let anime_resp: AnimeResponse = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("PARSE_ERROR", &e.to_string()),
        };

        let items: Vec<PluginEntry> = anime_resp
            .data
            .into_iter()
            .map(|a| a.into_entry(entry_kind))
            .collect();

        let total = anime_resp.meta.and_then(|m| m.count).unwrap_or_else(|| {
            if items.is_empty() {
                0
            } else {
                offset + items.len() as u32 + 1
            }
        });

        PluginResult::ok(SearchResponse { items, total })
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "kitsu provider does not resolve streams")
    }
}

// ── Kitsu API Types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HttpResponse {
    status: u16,
    body: String,
}

#[derive(Debug, Deserialize)]
struct AnimeResponse {
    data: Vec<Anime>,
    meta: Option<AnimeMeta>,
}

#[derive(Debug, Deserialize)]
struct AnimeMeta {
    count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Anime {
    id: String,
    #[serde(rename = "type")]
    _type: String,
    attributes: AnimeAttributes,
}

#[derive(Debug, Deserialize)]
struct AnimeAttributes {
    slug: String,
    synopsis: Option<String>,
    #[serde(rename = "canonicalTitle")]
    title: String,
    #[serde(rename = "titles")]
    titles: Option<Titles>,
    #[serde(rename = "averageRating")]
    rating: Option<String>,
    #[serde(rename = "startDate")]
    start_date: Option<String>,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
    #[serde(rename = "episodeCount")]
    episode_count: Option<i32>,
    #[serde(rename = "episodeLength")]
    episode_length: Option<i32>,
    #[serde(rename = "totalLength")]
    total_length: Option<i32>,
    #[serde(rename = "ageRating")]
    age_rating: Option<String>,
    #[serde(rename = "posterImage")]
    poster: Option<Image>,
    #[serde(rename = "coverImage")]
    cover: Option<Image>,
    #[serde(rename = "showType")]
    show_type: Option<String>,
    #[serde(rename = "nsfw")]
    nsfw: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Titles {
    #[serde(rename = "en")]
    en: Option<String>,
    #[serde(rename = "en_jp")]
    en_jp: Option<String>,
    #[serde(rename = "ja_jp")]
    ja_jp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Image {
    tiny: Option<String>,
    small: Option<String>,
    large: Option<String>,
    original: Option<String>,
}

impl Anime {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let attrs = self.attributes;

        let year = attrs
            .start_date
            .as_ref()
            .and_then(|d| d.split('-').next())
            .and_then(|y| y.parse::<u32>().ok());

        // Note: show_type contains format ("TV", "movie", "OVA", etc.)
        // Currently unused - PluginEntry doesn't have a media_type field.
        let _ = attrs.show_type;

        let description = attrs.synopsis.clone();

        let poster_url = attrs
            .poster
            .as_ref()
            .and_then(|p| p.large.clone().or(p.small.clone()));

        // Kitsu averageRating is 0–100; scale to 0.0–10.0
        let rating = attrs
            .rating
            .as_deref()
            .and_then(|r| r.parse::<f32>().ok())
            .map(|r| r / 10.0);

        PluginEntry {
            id: self.id,
            kind,
            source: "kitsu".to_string(),
            title: attrs.title,
            year,
            // Genre is intentionally left empty - Kitsu doesn't provide genre in search results
            genre: None,
            rating,
            description,
            poster_url,
            imdb_id: None,
            duration: None,
            ..Default::default()
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// GET with an `Authorization: Bearer` header — mirrors the pattern used by
/// other stui plugins (Spotify, Tidal) that need authenticated requests.
fn http_get_with_bearer(url: &str, token: &str) -> Result<String, String> {
    let payload = serde_json::json!({
        "url": url,
        "body": "",
        "__stui_headers": {
            "Authorization": format!("Bearer {}", token),
        }
    })
    .to_string();

    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn stui_http_post(ptr: *const u8, len: i32) -> i64;
            fn stui_free(ptr: i32, len: i32);
        }
        let packed = unsafe { stui_http_post(payload.as_ptr(), payload.len() as i32) };
        if packed == 0 {
            return Err("http request failed".into());
        }
        let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
        let len = (packed & 0xFFFFFFFF) as usize;

        // Create owned copy before freeing
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        let json_result = std::str::from_utf8(slice).map(|s| s.to_string());

        // Always free the host-allocated memory
        unsafe { stui_free(ptr as i32, len as i32) };

        let json = json_result.map_err(|e| e.to_string())?;

        let resp: HttpResponse = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        if resp.status >= 200 && resp.status < 300 {
            Ok(resp.body)
        } else {
            Err(format!("HTTP {}: {}", resp.status, resp.body))
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        drop(payload);
        Err("http_get_with_bearer only available in WASM context".into())
    }
}

stui_plugin_sdk::stui_export_plugin!(KitsuProvider);
