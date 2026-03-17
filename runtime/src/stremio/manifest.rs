//! Stremio addon manifest (`manifest.json`) types.
//!
//! Spec: https://github.com/Stremio/stremio-addon-sdk/blob/master/docs/api/responses/manifest.md

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioManifest {
    pub id:          String,
    pub name:        String,
    pub version:     String,
    pub description: Option<String>,
    pub logo:        Option<String>,
    pub resources:   Vec<serde_json::Value>, // "catalog", "stream", "subtitles", …
    pub types:       Vec<String>,            // "movie", "series", "anime", "music"
    pub catalogs:    Vec<StremioManifestCatalog>,
    #[serde(default)]
    pub behavior_hints: BehaviorHints,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioManifestCatalog {
    pub r#type: String,   // "movie", "series"
    pub id:     String,   // e.g. "top"
    pub name:   String,   // human-readable
    #[serde(default)]
    pub extra:  Vec<StremioExtra>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioExtra {
    pub name:     String,
    #[serde(default)]
    pub is_required: bool,
    #[serde(default)]
    pub options:  Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorHints {
    #[serde(default)]
    pub adult:           bool,
    #[serde(default)]
    pub p2p:             bool,
    #[serde(default)]
    pub configurable:    bool,
    #[serde(default)]
    pub configuration_required: bool,
}

// ── Stremio resource response types ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioMeta {
    pub id:          String,
    pub r#type:      String,
    pub name:        String,
    pub year:        Option<serde_json::Value>, // sometimes u32, sometimes String
    pub description: Option<String>,
    pub poster:      Option<String>,
    pub background:  Option<String>,
    pub logo:        Option<String>,
    pub rating:      Option<f64>,
    pub genres:      Option<Vec<String>>,
    #[serde(rename = "imdbRating")]
    pub imdb_rating: Option<String>,
    #[serde(rename = "imdb_id")]
    pub imdb_id:     Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioCatalogResponse {
    pub metas: Vec<StremioMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioStream {
    /// Direct URL, magnet, or infoHash
    pub url:          Option<String>,
    pub title:        Option<String>,
    pub name:         Option<String>,
    #[serde(rename = "infoHash")]
    pub info_hash:    Option<String>,
    #[serde(rename = "fileIdx")]
    pub file_idx:     Option<u32>,
    pub behavior_hints: Option<StreamBehaviorHints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamBehaviorHints {
    #[serde(rename = "bingeGroup")]
    pub binge_group:   Option<String>,
    #[serde(rename = "notWebReady")]
    pub not_web_ready: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioStreamResponse {
    pub streams: Vec<StremioStream>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioSubtitle {
    pub id:  String,
    pub url: String,
    pub lang: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StremioSubtitleResponse {
    pub subtitles: Vec<StremioSubtitle>,
}
