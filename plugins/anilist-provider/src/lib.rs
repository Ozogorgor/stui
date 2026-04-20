//! anilist-provider — stui plugin for AniList GraphQL API.
//!
//! ## API Overview
//!
//! AniList uses GraphQL at https://graphql.anilist.co
//!
//! Endpoints:
//!   query trending anime   → trending anime list
//!   query search anime    → search results
//!
//! No API key required for basic usage (rate limited).
//!
//! ## Plugin Interface
//!
//! This plugin implements the stui search interface:
//!   search(query, scope, page) → returns catalog entries
//!
//! Empty query + scope Movie/Series → returns trending anime.
//! Non-empty query → returns anime search results.

use serde::{Deserialize, Serialize};
use stui_plugin_sdk::prelude::*;
use stui_plugin_sdk::{error_codes, EntryKind, SearchScope};

const GRAPHQL_URL: &str = "https://graphql.anilist.co";

pub struct AnilistProvider;

impl AnilistProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AnilistProvider {
    fn default() -> Self {
        Self
    }
}

impl StuiPlugin for AnilistProvider {
    fn name(&self) -> &str {
        "anilist"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Metadata
    }

    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        // Anilist only covers anime (series/movies) — reject other scopes
        let entry_kind = match req.scope {
            SearchScope::Movie => EntryKind::Movie,
            SearchScope::Series => EntryKind::Series,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "anilist only supports movie and series scopes",
                );
            }
        };

        let query = req.query.trim();
        let page = req.page.max(1) as i32;
        let per_page = req.limit.min(50) as i32;

        let gql_query = if query.is_empty() {
            TRENDING_QUERY
        } else {
            SEARCH_QUERY
        };

        let variables = if query.is_empty() {
            serde_json::json!({
                "page": page,
                "perPage": per_page
            })
        } else {
            serde_json::json!({
                "search": query,
                "page": page,
                "perPage": per_page
            })
        };

        let body = serde_json::to_string(&GraphQLRequest {
            query: gql_query,
            variables,
        })
        .unwrap_or_default();

        plugin_info!("anilist: searching '{}' (page {})", query, page);

        let response = match http_post_json(GRAPHQL_URL, &body) {
            Ok(r) => r,
            Err(e) => return PluginResult::err("HTTP_ERROR", &e),
        };

        let gql_resp: GraphQLResponse = match serde_json::from_str(&response) {
            Ok(r) => r,
            Err(e) => {
                plugin_error!("anilist: parse error: {}", e);
                return PluginResult::err("PARSE_ERROR", &e.to_string());
            }
        };

        let entries: Vec<PluginEntry> = if query.is_empty() {
            gql_resp
                .data
                .trending
                .media
                .into_iter()
                .take(per_page as usize)
                .map(|m| m.into_entry(entry_kind))
                .collect()
        } else {
            gql_resp
                .data
                .media
                .search
                .into_iter()
                .take(per_page as usize)
                .map(|m| m.into_entry(entry_kind))
                .collect()
        };

        let total = entries.len() as u32;
        plugin_info!("anilist: {} entries", entries.len());

        PluginResult::ok(SearchResponse {
            items: entries,
            total,
        })
    }

    fn resolve(&self, _req: ResolveRequest) -> PluginResult<ResolveResponse> {
        PluginResult::err("NOT_SUPPORTED", "anilist provider does not resolve streams")
    }
}

const TRENDING_QUERY: &str = r#"
query ($page: Int, $perPage: Int) {
    trending {
        media(type: ANIME, sort: TRENDING_DESC) {
            id
            title { romaji english native }
            seasonYear
            averageScore
            episodes
            coverImage { large extraLarge }
            description(asHtml: false)
            type
        }
    }
}
"#;

const SEARCH_QUERY: &str = r#"
query ($search: String, $page: Int, $perPage: Int) {
    Media(search: $search, type: ANIME) {
        id
        title { romaji english native }
        seasonYear
        averageScore
        episodes
        coverImage { large extraLarge }
        description(asHtml: false)
        type
    }
}
"#;

#[derive(Debug, Serialize)]
struct GraphQLRequest {
    query: &'static str,
    variables: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GraphQLResponse {
    data: GraphQLData,
}

#[derive(Debug, Deserialize)]
struct GraphQLData {
    #[serde(default)]
    trending: TrendingData,
    #[serde(default)]
    media: MediaWrapper,
}

#[derive(Debug, Deserialize, Default)]
struct TrendingData {
    #[serde(default)]
    media: Vec<AnimeMedia>,
}

#[derive(Debug, Deserialize, Default)]
struct MediaWrapper {
    #[serde(default)]
    search: Vec<AnimeMedia>,
}

#[derive(Debug, Deserialize)]
struct AnimeMedia {
    id: u64,
    title: AnimeTitle,
    #[serde(default)]
    season_year: Option<u32>,
    #[serde(rename = "averageScore", default)]
    average_score: Option<f32>,
    #[serde(default)]
    episodes: Option<u32>,
    cover_image: CoverImage,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnimeTitle {
    #[serde(default)]
    romaji: Option<String>,
    #[serde(default)]
    english: Option<String>,
    #[serde(default)]
    native: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    #[serde(default)]
    large: Option<String>,
    #[serde(default)]
    extra_large: Option<String>,
}

impl AnimeMedia {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let title = self
            .title
            .english
            .or(self.title.romaji)
            .or(self.title.native)
            .unwrap_or_default();

        let year = self.season_year;
        // averageScore is 0–100; scale to 0.0–10.0
        let rating = self.average_score.map(|s| s / 10.0);
        let poster_url = self.cover_image.extra_large.or(self.cover_image.large);

        PluginEntry {
            id: format!("anilist-{}", self.id),
            kind,
            source: "anilist".to_string(),
            title,
            year,
            genre: self.episodes.map(|e| format!("{} eps", e)),
            rating,
            description: self.description,
            poster_url,
            imdb_id: None,
            duration: None,
            ..Default::default()
        }
    }
}

// ── WASM Exports ──────────────────────────────────────────────────────────────

stui_export_plugin!(AnilistProvider);
