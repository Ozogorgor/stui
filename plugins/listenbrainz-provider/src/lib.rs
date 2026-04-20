//! listenbrainz-provider — stui plugin for ListenBrainz API.
//!
//! ## API Overview
//!
//! ListenBrainz provides music metadata at https://api.listenbrainz.org
//!
//! Endpoints used:
//!   GET /search/musicbrainz?q={query}     → search for releases/artists
//!   GET /1/stats/user/{user}/listening-history → user's recent listens
//!   GET /1/stats/user/{user}/top-artists   → user's top artists
//!
//! No API key required for basic search.
//! User token is optional but enables personalized features.
//!
//! ## Plugin Interface
//!
//! This plugin implements the stui search interface:
//!   search(query, scope, page) → returns catalog entries
//!
//! Supported scopes: Artist, Album, Track
//! Empty query → returns trending/popular releases
//! Non-empty query → returns search results

use serde::{Deserialize, Serialize};
use stui_plugin_sdk::prelude::*;
use stui_plugin_sdk::{error_codes, EntryKind, SearchScope};

const API_BASE: &str = "https://api.listenbrainz.org";

pub struct ListenbrainzProvider;

impl ListenbrainzProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ListenbrainzProvider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for ListenbrainzProvider {
    fn name(&self) -> &str {
        "listenbrainz"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // listenbrainz supports music scopes
        let entry_kind = match req.scope {
            SearchScope::Artist => EntryKind::Artist,
            SearchScope::Album => EntryKind::Album,
            SearchScope::Track => EntryKind::Track,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "listenbrainz only supports artist, album, and track scopes",
                );
            }
        };

        let query = req.query.trim();
        let limit = req.limit.min(50) as usize;

        if query.is_empty() {
            self.get_charts(limit, entry_kind)
        } else {
            self.search_releases(query, limit, entry_kind)
        }
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err(
            "NOT_SUPPORTED",
            "listenbrainz provider does not resolve streams",
        )
    }
}

impl ListenbrainzProvider {
    fn search_releases(&self, query: &str, limit: usize, entry_kind: EntryKind) -> PluginResult<SearchResponse> {
        let url = format!(
            "{}/search/musicbrainz?query={}&type=release&limit={}",
            API_BASE,
            url_encode(query),
            limit
        );

        plugin_info!("listenbrainz: searching '{}'", query);

        let response = match http_get(&url) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let search_resp: SearchResponseBody = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("listenbrainz: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let entries: Vec<PluginEntry> = search_resp
            .releases
            .into_iter()
            .take(limit)
            .map(|r| r.into_entry(entry_kind))
            .collect();

        let total = entries.len() as u32;
        plugin_info!("listenbrainz: {} entries", entries.len());

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }

    fn get_charts(&self, limit: usize, entry_kind: EntryKind) -> PluginResult<SearchResponse> {
        // Get top releases from the charts endpoint
        let url = format!(
            "{}/1/stats/shifts/top-recordings-for-week?listeners_threshold=10&limit={}",
            API_BASE, limit
        );

        plugin_info!("listenbrainz: fetching charts");

        let response = match http_get(&url) {
            Ok(r) => r,
            Err(e) => {
                // Fallback to search for popular artists if charts fail
                plugin_info!("listenbrainz: charts unavailable, using search fallback");
                let fallback_url = format!(
                    "{}/search/musicbrainz?query=%25&type=release&limit={}",
                    API_BASE, limit
                );
                match http_get(&fallback_url) {
                    Ok(r) => r,
                    Err(e2) => return PluginResult::err("HTTP_ERROR", &e2),
                }
            }
        };

        let search_resp: SearchResponseBody = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("listenbrainz: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let entries: Vec<PluginEntry> = search_resp
            .releases
            .into_iter()
            .take(limit)
            .map(|r| r.into_entry(entry_kind))
            .collect();

        let total = entries.len() as u32;
        plugin_info!("listenbrainz: {} chart entries", entries.len());

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }
}

// ── API Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponseBody {
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Debug, Deserialize)]
struct Release {
    #[serde(rename = "release_mbid", default)]
    release_mbid: Option<String>,
    #[serde(rename = "release_name", default)]
    release_name: Option<String>,
    #[serde(rename = "artist_credit_name", default)]
    artist_credit_name: Option<String>,
    #[serde(rename = "artist_mbids", default)]
    artist_mbids: Vec<String>,
    #[serde(rename = "release_date", default)]
    release_date: Option<String>,
    #[serde(rename = "cover_art_urls", default)]
    cover_art_urls: Option<CoverArtUrls>,
    #[serde(default)]
    genres: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CoverArtUrls {
    #[serde(rename = "250", default)]
    small: Option<String>,
    #[serde(rename = "500", default)]
    medium: Option<String>,
    #[serde(rename = "1200", default)]
    large: Option<String>,
}

impl Release {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let title = self
            .release_name
            .unwrap_or_else(|| "Unknown Release".to_string());
        let artist = self
            .artist_credit_name
            .unwrap_or_else(|| "Unknown Artist".to_string());
        let full_title = format!("{} - {}", artist, title);

        // Extract year from release_date (format: YYYY-MM-DD)
        let year = self
            .release_date
            .as_ref()
            .and_then(|d| d.get(0..4))
            .and_then(|y| y.parse::<u32>().ok());

        let poster_url = self
            .cover_art_urls
            .as_ref()
            .and_then(|c| c.large.clone().or(c.medium.clone()).or(c.small.clone()));

        let genre = if self.genres.is_empty() {
            None
        } else {
            Some(self.genres.join(", "))
        };

        let id = self
            .release_mbid
            .map(|mbid| format!("lbz-{}", mbid))
            .unwrap_or_else(|| format!("lbz-{}", url_encode(&title)));

        PluginEntry {
            id,
            kind,
            source: "listenbrainz".to_string(),
            title: full_title,
            year,
            genre,
            rating: None,
            description: Some(format!("Artist: {}", artist)),
            poster_url,
            imdb_id: None,
            duration: None,
            artist_name: Some(artist),
            album_name: Some(title),
            ..Default::default()
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

// ── WASM Exports ───────────────────────────────────────────────────────────────

stui_export_plugin!(ListenbrainzProvider);
