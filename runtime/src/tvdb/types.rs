//! TVDB v4 JSON response types. Only the fields we consume are declared;
//! serde tolerates extras so upstream additions don't break parsing.

use serde::Deserialize;

/// Top-level envelope used by every TVDB response. `status` is "success" on a
/// normal response; non-success values carry a `message` we surface to logs.
#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    pub data: Option<T>,
}

/// POST /login response body.
#[derive(Debug, Deserialize)]
pub struct LoginData {
    pub token: String,
}

/// One item in a /search response. Many fields are Option<_> because TVDB
/// returns them sparsely depending on entity type.
#[derive(Debug, Deserialize, Default)]
pub struct SearchItem {
    #[serde(default)]
    pub tvdb_id: Option<String>,
    /// "movie" | "series" | "person" | "episode" | "season" | "company"
    #[serde(default, rename = "type")]
    pub item_type: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub primary_language: Option<String>,
    #[serde(default)]
    pub remote_ids: Vec<RemoteId>,
    #[serde(default)]
    pub genres: Vec<String>,
}

/// External-id cross-reference. `source_name` is the provider ("IMDB",
/// "TheMovieDB.com", "TV Maze", …) and `id` is that provider's id.
#[derive(Debug, Deserialize, Default)]
pub struct RemoteId {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub source_name: Option<String>,
}

/// Extended movie/series response (used for enrichment). Contains the
/// superset of metadata fields TVDB has for an entity. Not yet wired —
/// kept as a stub so the `/movies/{id}/extended` endpoint can land
/// without touching the module surface.
#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct ExtendedRecord {
    #[serde(default)]
    pub id: Option<serde_json::Value>, // int on some endpoints, string on others
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub remote_ids: Vec<RemoteId>,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct Genre {
    #[serde(default)]
    pub name: String,
}
