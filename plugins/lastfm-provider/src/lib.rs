//! lastfm-provider — stui plugin for Last.fm / Libre.fm API.
//!
//! ## API Overview
//!
//! Last.fm's original API is deprecated; this plugin uses Libre.fm's compatible API:
//!   https://libre.fm/api
//!
//! Endpoints used:
//!   GET /?method=track.search&track={query}  → search tracks
//!   GET /?method=chart.gettoptracks           → trending tracks
//!
//! API key required (free at https://libre.fm/api/register).
//!
//! ## Plugin Interface
//!
//! This plugin implements the stui search interface:
//!   search(query, scope, page) → returns catalog entries
//!
//! Supported scopes: Artist, Album, Track
//! Empty query → returns trending tracks (charts)
//! Non-empty query → returns search results

use serde::{Deserialize, Serialize};
use stui_plugin_sdk::prelude::*;
use stui_plugin_sdk::{error_codes, EntryKind, SearchScope};

const API_BASE: &str = "https://libre.fm/2.0";

pub struct LastfmProvider;

impl LastfmProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LastfmProvider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for LastfmProvider {
    fn name(&self) -> &str {
        "lastfm"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // lastfm supports music scopes: artist, album, track
        let entry_kind = match req.scope {
            SearchScope::Artist => EntryKind::Artist,
            SearchScope::Album => EntryKind::Album,
            SearchScope::Track => EntryKind::Track,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "lastfm only supports artist, album, and track scopes",
                );
            }
        };

        let query = req.query.trim();
        let limit = req.limit.min(50) as usize;

        if query.is_empty() {
            self.get_charts(limit, entry_kind)
        } else {
            self.search_tracks(query, limit, entry_kind)
        }
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "lastfm provider does not resolve streams")
    }
}

impl LastfmProvider {
    fn get_api_key(&self) -> Result<String, String> {
        // Check config first (from plugin.toml [config])
        cache_get("__config:api_key")
            .or_else(|| std::env::var("LASTFM_API_KEY").ok())
            .ok_or_else(|| "LASTFM_API_KEY not configured".to_string())
    }

    fn search_tracks(&self, query: &str, limit: usize, entry_kind: EntryKind) -> PluginResult<SearchResponse> {
        let api_key = match self.get_api_key() {
            Ok(k) => k,
            Err(e) => return PluginResult::err("CONFIG_ERROR", &e),
        };

        let url = format!(
            "{}?method=track.search&track={}&api_key={}&format=json&limit={}",
            API_BASE,
            url_encode(query),
            api_key,
            limit
        );

        plugin_info!("lastfm: searching '{}'", query);

        let response = match http_get(&url) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let search_resp: TrackSearchResponse = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("lastfm: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let entries: Vec<PluginEntry> = search_resp
            .results
            .trackmatches
            .track
            .into_iter()
            .take(limit)
            .filter_map(|t| t.into_entry(entry_kind))
            .collect();

        let total = entries.len() as u32;
        plugin_info!("lastfm: {} entries", entries.len());

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }

    fn get_charts(&self, limit: usize, entry_kind: EntryKind) -> PluginResult<SearchResponse> {
        let api_key = match self.get_api_key() {
            Ok(k) => k,
            Err(e) => return PluginResult::err("CONFIG_ERROR", &e),
        };

        let url = format!(
            "{}?method=chart.gettoptracks&api_key={}&format=json&limit={}",
            API_BASE, api_key, limit
        );

        plugin_info!("lastfm: fetching charts");

        let response = match http_get(&url) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let chart_resp: ChartResponse = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("lastfm: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let entries: Vec<PluginEntry> = chart_resp
            .tracks
            .track
            .into_iter()
            .take(limit)
            .filter_map(|t| t.into_entry(entry_kind))
            .collect();

        let total = entries.len() as u32;
        plugin_info!("lastfm: {} chart entries", entries.len());

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }
}

// ── API Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TrackSearchResponse {
    results: SearchResults,
}

#[derive(Debug, Deserialize)]
struct SearchResults {
    #[serde(rename = "trackmatches")]
    trackmatches: TrackMatches,
}

#[derive(Debug, Deserialize)]
struct TrackMatches {
    track: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Track {
    name: String,
    artist: String,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    image: Vec<TrackImage>,
    #[serde(default)]
    streamable: Option<String>,
    #[serde(default)]
    listeners: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TrackImage {
    #[serde(rename = "#text", default)]
    text: String,
    #[serde(default)]
    size: String,
}

impl Track {
    fn into_entry(self, kind: EntryKind) -> Option<PluginEntry> {
        let track_name = self.name.clone();
        let artist = self.artist.clone();
        let album = self.album.clone();

        let poster_url = self
            .image
            .into_iter()
            .find(|i| i.size == "large" || i.size == "extralarge")
            .map(|i| i.text)
            .filter(|i| !i.is_empty());

        let genre = self.listeners.map(|l| format!("{} listeners", l));

        let description = Some(format!(
            "Artist: {} | Album: {}",
            artist,
            album.unwrap_or_else(|| "Unknown".to_string())
        ));

        Some(PluginEntry {
            id: format!("lastfm-{}", url_encode(&format!("{} - {}", artist, track_name))),
            kind,
            source: "lastfm".to_string(),
            title: track_name.clone(),
            year: None,
            genre,
            rating: None,
            description,
            poster_url,
            imdb_id: None,
            duration: None,
            artist_name: Some(artist),
            ..Default::default()
        })
    }
}

#[derive(Debug, Deserialize)]
struct ChartResponse {
    tracks: ChartTracks,
}

#[derive(Debug, Deserialize)]
struct ChartTracks {
    track: Vec<ChartTrack>,
}

#[derive(Debug, Deserialize)]
struct ChartTrack {
    name: String,
    artist: ChartArtist,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    image: Vec<TrackImage>,
    #[serde(default)]
    listeners: Option<String>,
    #[serde(default)]
    playcount: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChartArtist {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: Option<String>,
}

impl ChartTrack {
    fn into_entry(self, kind: EntryKind) -> Option<PluginEntry> {
        let track_name = self.name.clone();
        let artist = self.artist.name.clone();
        let album = self.album.clone();

        let poster_url = self
            .image
            .into_iter()
            .find(|i| i.size == "large" || i.size == "extralarge")
            .map(|i| i.text)
            .filter(|i| !i.is_empty());

        let mut genre_parts = vec![];
        if let Some(l) = &self.listeners {
            genre_parts.push(format!("{} listeners", l));
        }
        if let Some(p) = &self.playcount {
            genre_parts.push(format!("{} plays", p));
        }
        let genre = if genre_parts.is_empty() {
            None
        } else {
            Some(genre_parts.join(" | "))
        };

        let description = Some(format!(
            "Artist: {} | Album: {}",
            artist,
            album.unwrap_or_else(|| "Unknown".to_string())
        ));

        Some(PluginEntry {
            id: format!(
                "lastfm-chart-{}",
                url_encode(&format!("{} - {}", artist, track_name))
            ),
            kind,
            source: "lastfm".to_string(),
            title: track_name.clone(),
            year: None,
            genre,
            rating: None,
            description,
            poster_url,
            imdb_id: None,
            duration: None,
            artist_name: Some(artist),
            ..Default::default()
        })
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

// ── WASM Exports ───────────────────────────────────────────────────────────────

stui_export_plugin!(LastfmProvider);
