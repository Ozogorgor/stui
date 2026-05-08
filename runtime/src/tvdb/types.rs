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
    /// TVDB ships this as `source_name` from `/search` and `sourceName`
    /// from `/series|movies/{id}/extended`. Accept both via alias so the
    /// same struct works for both consumers.
    #[serde(default, alias = "sourceName")]
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

/// Wrapper around `/series/{id}/episodes/{season-type}` payloads. TVDB
/// nests the episode list under `episodes` alongside a redundant
/// `series` block (the latter is the same series we already know about,
/// so we drop it via serde's tolerate-extras default).
#[derive(Debug, Deserialize, Default)]
pub struct EpisodesPayload {
    #[serde(default)]
    pub episodes: Vec<EpisodeRecord>,
}

/// One episode row inside `EpisodesPayload.episodes`. TVDB returns a
/// dense object — we keep only what the wire schema needs. Per-episode
/// images / overviews / translation arrays are intentionally dropped
/// since the TUI's grid is text-only today.
#[derive(Debug, Deserialize, Default)]
pub struct EpisodeRecord {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub aired: Option<String>,
    #[serde(default)]
    pub runtime: Option<u32>,
    #[serde(default, rename = "seasonNumber")]
    pub season_number: Option<u32>,
    #[serde(default)]
    pub number: Option<u32>,
}

/// Full series record from `/v4/series/{id}/extended`. Only fields the
/// runtime consumes; serde tolerates extras so upstream additions don't
/// break parsing.
///
/// Cached inside `TvdbClient::extended_cache`; one HTTP call serves
/// `enrich`, `credits`, and `artwork` for the same id.
#[derive(Debug, Deserialize, Default)]
pub struct ExtendedSeries {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
    /// Poster (TVDB's `image` field on series-extended).
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    #[serde(default, rename = "remoteIds")]
    pub remote_ids: Vec<RemoteId>,
    #[serde(default)]
    pub seasons: Vec<Season>,
    #[serde(default)]
    pub characters: Vec<Character>,
    #[serde(default)]
    pub artworks: Vec<Artwork>,
    /// ISO 639-* code for the show's original spoken/produced language.
    /// Mirrors `SearchItem::primary_language` — TVDB v4 extended payloads
    /// expose it as `originalLanguage`. The previous `tvdb_enrich` adapter
    /// at `dispatch.rs:294` populated `PluginEntry.original_language`
    /// from this field; preserve the behaviour to keep the engine's
    /// anime-mix classifier (which keys on language) working.
    #[serde(rename = "originalLanguage", default)]
    pub original_language: Option<String>,
}

/// Movie counterpart. Same shape minus `seasons` (movies don't have them)
/// and minus `banner` (movies use `image` only).
#[derive(Debug, Deserialize, Default)]
pub struct ExtendedMovie {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
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
    #[serde(default, rename = "remoteIds")]
    pub remote_ids: Vec<RemoteId>,
    #[serde(default)]
    pub characters: Vec<Character>,
    #[serde(default)]
    pub artworks: Vec<Artwork>,
    #[serde(rename = "originalLanguage", default)]
    pub original_language: Option<String>,
}

/// One season row inside `ExtendedSeries.seasons`. TVDB ships multiple
/// season-ordering schemes (`type`); we only count the default order.
#[derive(Debug, Deserialize, Default)]
pub struct Season {
    #[serde(default)]
    pub id: u64,
    /// 0 = specials, 1.. = canonical seasons.
    #[serde(default)]
    pub number: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// `type` is reserved in Rust; rename to `season_type`. The `id`
    /// inside the type object is what tells default vs DVD vs absolute.
    #[serde(rename = "type", default)]
    pub season_type: SeasonType,
}

#[derive(Debug, Deserialize, Default)]
pub struct SeasonType {
    /// 1 = "Aired Order" (default), 2 = "DVD Order", 3 = "Absolute Order",
    /// 4 = "Alternate Order", 5 = "Regional Order".
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub name: Option<String>,
}

/// One row inside `extended.characters[]`. TVDB conflates cast and crew
/// here, distinguished by `peopleType`.
#[derive(Debug, Deserialize, Default)]
pub struct Character {
    /// Person's name. May be missing for unfilled roles.
    #[serde(default, rename = "personName")]
    pub person_name: Option<String>,
    /// "Actor" | "Director" | "Writer" | "Producer" | "Guest Star" | …
    #[serde(default, rename = "peopleType")]
    pub people_type: Option<String>,
    /// Character name (when `peopleType == "Actor"`). Empty for crew.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// Billing order — TVDB names this `sort`. Lower = earlier in
    /// credits; we forward it as `billing_order` on cast members.
    #[serde(default)]
    pub sort: Option<u32>,
}

/// One row inside `extended.artworks[]`.
#[derive(Debug, Deserialize, Default)]
pub struct Artwork {
    pub image: String,
    #[serde(default)]
    pub thumbnail: Option<String>,
    /// TVDB type code: 1=banner, 2=poster, 3=background, 22=clearart, …
    #[serde(default, rename = "type")]
    pub image_type: u32,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_record_parses_realistic_row() {
        // Trimmed sample of TVDB v4's `/series/{id}/episodes/default` shape.
        let raw = r#"{
            "id": 12345,
            "seriesId": 81189,
            "name": "Pilot",
            "aired": "2008-01-20",
            "runtime": 47,
            "nameTranslations": ["eng"],
            "overview": "Walter White begins.",
            "image": "https://x/y.jpg",
            "imageType": 12,
            "isMovie": 0,
            "number": 1,
            "absoluteNumber": 1,
            "seasonNumber": 1,
            "lastUpdated": "2024-01-01"
        }"#;
        let ep: EpisodeRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(ep.id, Some(12345));
        assert_eq!(ep.name.as_deref(), Some("Pilot"));
        assert_eq!(ep.aired.as_deref(), Some("2008-01-20"));
        assert_eq!(ep.runtime, Some(47));
        assert_eq!(ep.season_number, Some(1));
        assert_eq!(ep.number, Some(1));
    }

    #[test]
    fn episodes_payload_tolerates_missing_episodes_field() {
        // TVDB occasionally returns just a series block with no episodes
        // (e.g. when the series exists but the requested season-type
        // isn't populated yet). serde's default keeps that from erroring.
        let raw = r#"{ "series": { "id": 81189 } }"#;
        let p: EpisodesPayload = serde_json::from_str(raw).unwrap();
        assert!(p.episodes.is_empty());
    }

    #[test]
    fn extended_series_parses_realistic_payload() {
        let raw = r#"{
            "id": 81189,
            "name": "Breaking Bad",
            "overview": "Walter White begins.",
            "year": "2008",
            "image": "https://x/p.jpg",
            "score": 9.6,
            "originalLanguage": "eng",
            "genres": [{"name": "Drama"}, {"name": "Crime"}],
            "remoteIds": [
                {"id": "tt0903747", "sourceName": "IMDB"},
                {"id": "1396",      "sourceName": "TheMovieDB.com"}
            ],
            "seasons": [
                {"id": 1, "number": 0, "type": {"id": 1, "name": "Aired Order"}},
                {"id": 2, "number": 1, "type": {"id": 1, "name": "Aired Order"}},
                {"id": 3, "number": 2, "type": {"id": 1, "name": "Aired Order"}},
                {"id": 9, "number": 1, "type": {"id": 2, "name": "DVD Order"}}
            ],
            "characters": [
                {"personName": "Bryan Cranston", "peopleType": "Actor", "name": "Walter White", "sort": 1},
                {"personName": "Vince Gilligan", "peopleType": "Director"}
            ],
            "artworks": [
                {"image": "https://x/poster.jpg", "type": 2, "language": "eng", "score": 5.0},
                {"image": "https://x/bg.jpg",     "type": 3}
            ]
        }"#;
        let s: ExtendedSeries = serde_json::from_str(raw).unwrap();
        assert_eq!(s.id, 81189);
        assert_eq!(s.name, "Breaking Bad");
        assert_eq!(s.year.as_deref(), Some("2008"));
        assert_eq!(s.score, Some(9.6));
        assert_eq!(s.genres.len(), 2);
        assert_eq!(s.remote_ids.len(), 2);
        assert_eq!(s.remote_ids[0].source_name.as_deref(), Some("IMDB"));
        assert_eq!(s.remote_ids[0].id, "tt0903747");
        assert_eq!(
            s.remote_ids[1].source_name.as_deref(),
            Some("TheMovieDB.com")
        );
        assert_eq!(s.seasons.len(), 4);
        // Verify season-type discriminator survives.
        assert_eq!(s.seasons[3].season_type.id, 2);
        assert_eq!(s.characters.len(), 2);
        assert_eq!(s.characters[0].people_type.as_deref(), Some("Actor"));
        assert_eq!(s.characters[0].sort, Some(1));
        assert_eq!(s.artworks.len(), 2);
        assert_eq!(s.artworks[0].image_type, 2);
        assert_eq!(s.original_language.as_deref(), Some("eng"));
    }

    #[test]
    fn extended_movie_omits_seasons_field_cleanly() {
        let raw = r#"{
            "id": 100,
            "name": "Inception",
            "year": "2010",
            "score": 8.8,
            "genres": [{"name": "Sci-Fi"}],
            "remoteIds": [{"id": "tt1375666", "sourceName": "IMDB"}],
            "characters": [],
            "artworks": []
        }"#;
        let m: ExtendedMovie = serde_json::from_str(raw).unwrap();
        assert_eq!(m.id, 100);
        assert_eq!(m.name, "Inception");
        assert_eq!(m.year.as_deref(), Some("2010"));
    }
}
