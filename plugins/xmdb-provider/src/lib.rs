//! XMDb metadata provider — movies and series via xmdbapi.com.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup, enrich,
//! get_credits, episodes}`. Sits alongside omdb-provider rather than
//! replacing it: xmdb has no Rotten Tomatoes signal, so omdb stays
//! around for the `tomatometer` rating source. xmdb covers IMDb +
//! Metacritic at a 25,000/day quota — comfortable for normal use plus
//! the heavy enrichment loops that burn through OMDb's 1k/day cap.
//!
//! ## API key
//!
//! Required. Read from `InitContext.config["api_key"]` at `Plugin::init`.
//! Fallback: `XMDB_API_KEY` env var surfaced by the host through
//! `cache_get("__env:XMDB_API_KEY")`.
//!
//! ## Response shape
//!
//! TMDB-style multi-endpoint REST. The plugin uses three endpoints:
//!   - `GET /movies/{id}`        — details, ratings, cast/crew (also accepts imdb tt-ids)
//!   - `GET /search?q=...`       — title search
//!   - `GET /seasons/{series}`   — per-season episode list
//!
//! Authentication is `?apiKey=...` query-parameter form. The header
//! form (`X-API-Key:`) is also supported by the upstream but the SDK's
//! `http_get` doesn't currently take custom headers, so query-param it
//! is.

use std::sync::OnceLock;

use serde::Deserialize;

use stui_plugin_sdk::{
    parse_manifest,
    cache_get, error_codes, http_get,
    id_sources, normalize_crew_role,
    plugin_error, plugin_info,
    stui_export_catalog_plugin,
    CastMember, CastRole, CatalogPlugin, CreditsRequest, CreditsResponse, CrewMember,
    EnrichRequest, EnrichResponse,
    EntryKind,
    EpisodeWire, EpisodesRequest, EpisodesResponse,
    InitContext,
    LookupRequest, LookupResponse,
    Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    SearchRequest, SearchResponse, SearchScope,
};

const BASE_URL: &str = "https://xmdbapi.com/api/v1";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct XmdbPlugin {
    manifest: PluginManifest,
    api_key: OnceLock<String>,
}

impl XmdbPlugin {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self { manifest, api_key: OnceLock::new() }
    }

    #[cfg(test)]
    pub fn new_for_test(api_key: &str) -> Self {
        let inst = Self::new();
        let _ = inst.api_key.set(api_key.to_string());
        inst
    }

    /// Resolve the API key: cached → env fallback. Returns `INVALID_REQUEST`
    /// when unset so the caller can surface it to the user.
    fn api_key(&self) -> Result<&str, PluginError> {
        if let Some(k) = self.api_key.get() {
            return Ok(k.as_str());
        }
        let env_key = cache_get("__env:XMDB_API_KEY").unwrap_or_default();
        if env_key.is_empty() {
            return Err(PluginError {
                code: error_codes::INVALID_REQUEST.to_string(),
                message: "XMDb api_key not configured".to_string(),
            });
        }
        Ok(self.api_key.get_or_init(|| env_key).as_str())
    }
}

impl Default for XmdbPlugin {
    fn default() -> Self { Self::new() }
}

impl Plugin for XmdbPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        let key = ctx
            .config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("XMDB_API_KEY").cloned())
            .unwrap_or_default();
        if key.is_empty() {
            return Err(PluginInitError::MissingConfig {
                fields: vec!["api_key".to_string()],
                hint: Some("Get a key at https://xmdbapi.com — 25k/day free tier".to_string()),
            });
        }
        let _ = self.api_key.set(key);
        Ok(())
    }
}

// ── Error classification ──────────────────────────────────────────────────────

/// The SDK's `http_get` surfaces non-2xx responses as `Err("HTTP {code}: {body}")`.
/// Re-classify the status code into one of our canonical error codes.
fn classify_http_err(err: &str) -> PluginError {
    if let Some(rest) = err.strip_prefix("HTTP ") {
        if let Some((code_str, body)) = rest.split_once(": ") {
            if let Ok(status) = code_str.parse::<u16>() {
                let code = match status {
                    401 | 403   => error_codes::INVALID_REQUEST,   // bad/unauthorized key
                    402         => error_codes::RATE_LIMITED,       // payment/quota
                    404         => error_codes::UNKNOWN_ID,
                    429         => error_codes::RATE_LIMITED,
                    500..=599   => error_codes::TRANSIENT,
                    _           => error_codes::REMOTE_ERROR,
                };
                return PluginError {
                    code: code.to_string(),
                    message: format!("XMDb HTTP {status}: {body}"),
                };
            }
        }
    }
    PluginError {
        code: error_codes::TRANSIENT.to_string(),
        message: err.to_string(),
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, PluginError> {
    serde_json::from_str(body).map_err(|e| {
        plugin_error!("xmdb: parse error: {}", e);
        PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("XMDb JSON parse failure: {e}"),
        }
    })
}

#[cfg(test)]
fn opt_non_empty(s: &str) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s.to_string()) }
}

/// `imdb_url` is `https://www.imdb.com/title/tt1234567/` — extract the tt-id.
fn imdb_id_from_url(url: &str) -> Option<String> {
    url.split('/')
        .find(|seg| seg.starts_with("tt") && seg.len() > 2 && seg[2..].chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

/// XMDb's `title_type` field maps to our `EntryKind`. The exact enum
/// values aren't pinned down in the docs; treat anything tv-like as
/// Series and fall back to the request's expected kind otherwise.
fn kind_from_title_type(s: &str, fallback: EntryKind) -> EntryKind {
    let lc = s.to_ascii_lowercase();
    if lc.contains("series") || lc.contains("tv") || lc == "show" {
        EntryKind::Series
    } else if lc == "movie" || lc == "film" {
        EntryKind::Movie
    } else {
        fallback
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for XmdbPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };

        let entry_kind = match req.scope {
            SearchScope::Movie  => EntryKind::Movie,
            SearchScope::Series => EntryKind::Series,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "xmdb only supports movie and series scopes",
                );
            }
        };

        let query = req.query.trim();
        if query.is_empty() {
            // No documented trending endpoint usable as a "browse" fallback;
            // an empty query yields zero results, same as omdb.
            return PluginResult::ok(SearchResponse { items: vec![], total: 0 });
        }

        let url = format!(
            "{BASE_URL}/search?q={}&apiKey={}",
            urlencoding::encode(query),
            api_key,
        );
        plugin_info!("xmdb: search {}", query);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        // The /search response shape isn't fully documented; the safe
        // assumption is `{"results": [...]}` with each row carrying
        // enough of the movies/{id} shape to populate a card. If the
        // upstream uses a different envelope (`items`, `data.movies`,
        // etc.) the deserializer will fail — wire log will tell us.
        let raw: SearchEnvelope = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };

        let limit = if req.limit == 0 { usize::MAX } else { req.limit as usize };
        let items: Vec<PluginEntry> = raw
            .results
            .into_iter()
            .filter(|s| {
                // When the upstream's title_type doesn't match the requested
                // scope, drop the row. Permissive on missing/unknown values
                // since the field shape isn't pinned.
                match s.title_type.as_deref() {
                    Some(t) => kind_from_title_type(t, entry_kind) == entry_kind,
                    None    => true,
                }
            })
            .take(limit)
            .map(|s| s.into_entry(entry_kind))
            .collect();
        let total = items.len() as u32;
        plugin_info!("xmdb: {} entries", items.len());
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb lookup only supports imdb ids, got: {}", req.id_source),
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };

        let url = format!(
            "{BASE_URL}/movies/{}?apiKey={}",
            urlencoding::encode(&req.id),
            api_key,
        );
        plugin_info!("xmdb: lookup {} (imdb)", req.id);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let detail: MovieDetail = match parse_json(&body) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };

        let kind = detail.title_type
            .as_deref()
            .map(|t| kind_from_title_type(t, req.kind))
            .unwrap_or(req.kind);

        PluginResult::ok(LookupResponse { entry: detail.into_entry(kind) })
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        // Fast path: partial already carries an IMDb id. xmdb has no
        // documented title-search-as-enrich endpoint that returns the
        // full /movies shape, so without an imdb id we bail rather than
        // doing two round-trips (search → resolve top hit → /movies).
        // Add the title-fallback path later once response shape is
        // verified.
        let imdb = req
            .partial
            .external_ids
            .get(id_sources::IMDB)
            .cloned()
            .or_else(|| req.partial.imdb_id.clone());
        let Some(imdb_id) = imdb else {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                "xmdb enrich: imdb_id is required",
            );
        };
        let lookup_req = LookupRequest {
            id: imdb_id,
            id_source: id_sources::IMDB.to_string(),
            kind: req.partial.kind,
            locale: None,
        };
        match self.lookup(lookup_req) {
            PluginResult::Ok(r) => PluginResult::ok(EnrichResponse {
                entry: r.entry,
                confidence: 1.0,
            }),
            PluginResult::Err(e) => PluginResult::Err(e),
        }
    }

    fn get_credits(&self, req: CreditsRequest) -> PluginResult<CreditsResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb credits only supports imdb ids, got: {}", req.id_source),
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let url = format!(
            "{BASE_URL}/movies/{}?apiKey={}",
            urlencoding::encode(&req.id),
            api_key,
        );
        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let detail: MovieDetail = match parse_json(&body) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };

        let cast: Vec<CastMember> = detail
            .full_cast_and_crew
            .iter()
            .filter(|c| c.is_cast())
            .enumerate()
            .map(|(i, c)| CastMember {
                name: c.name.clone(),
                role: CastRole::Actor,
                character: c.character.clone(),
                instrument: None,
                billing_order: Some(i as u32 + 1),
                external_ids: Default::default(),
            })
            .collect();
        let crew: Vec<CrewMember> = detail
            .full_cast_and_crew
            .iter()
            .filter(|c| !c.is_cast())
            .map(|c| CrewMember {
                name: c.name.clone(),
                role: normalize_crew_role(c.role.as_deref().unwrap_or("crew")),
                department: c.department.clone(),
                external_ids: Default::default(),
            })
            .collect();

        PluginResult::ok(CreditsResponse { cast, crew })
    }

    fn episodes(&self, req: EpisodesRequest) -> PluginResult<EpisodesResponse> {
        if req.id_source != id_sources::IMDB && req.id_source != "xmdb" {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb episodes only supports imdb/xmdb id_source, got: {}", req.id_source),
            );
        }
        if req.season < 1 {
            return PluginResult::err(
                error_codes::INVALID_REQUEST,
                "xmdb episodes: season must be >= 1",
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };

        let url = format!(
            "{BASE_URL}/seasons/{}?season={}&apiKey={}",
            urlencoding::encode(&req.series_id),
            req.season,
            api_key,
        );
        plugin_info!("xmdb: episodes id={} season={}", req.series_id, req.season);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let resp: SeasonResponse = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };

        let episodes = build_episodes(&req.series_id, req.season, resp.episodes);
        PluginResult::ok(EpisodesResponse { episodes })
    }
}

// ── Episode builder ───────────────────────────────────────────────────────────

fn build_episodes(series_id: &str, season: u32, raw: Vec<RawEpisode>) -> Vec<EpisodeWire> {
    raw.into_iter()
        .map(|ep| {
            let n = ep.episode_number.unwrap_or(0);
            let title = ep.title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| format!("Episode {n}"));
            let imdb_from_url = ep.imdb_url.as_deref().and_then(imdb_id_from_url);
            let imdb_id = imdb_from_url.or(ep.imdb_id);
            let entry_id = match imdb_id {
                Some(ref id) => format!("xmdb-{id}"),
                None         => format!("xmdb-{series_id}:s{season}e{n}"),
            };
            EpisodeWire {
                season,
                episode: n,
                title,
                air_date: ep.release_date,
                runtime_mins: ep.runtime_minutes,
                provider: "xmdb".to_string(),
                entry_id,
            }
        })
        .collect()
}

// ── API types ─────────────────────────────────────────────────────────────────

/// The `/search` response envelope. The exact wire shape isn't pinned
/// in the public docs; this is the simplest reasonable assumption.
/// Adjust to match the real response on first run.
#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    #[serde(default)]
    results: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    #[serde(default)] id:          Option<String>,
    #[serde(default)] title:       Option<String>,
    #[serde(default)] release_year: Option<u32>,
    #[serde(default)] poster_url:  Option<String>,
    #[serde(default)] imdb_url:    Option<String>,
    #[serde(default)] title_type:  Option<String>,
}

impl SearchHit {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let imdb = self.imdb_url.as_deref().and_then(imdb_id_from_url);
        let id   = self.id.clone().or_else(|| imdb.clone()).unwrap_or_default();
        let mut entry = PluginEntry {
            id,
            kind,
            source:     "xmdb".to_string(),
            title:      self.title.unwrap_or_default(),
            year:       self.release_year,
            poster_url: self.poster_url,
            imdb_id:    imdb.clone(),
            ..Default::default()
        };
        if let Some(id) = imdb {
            entry.external_ids.insert(id_sources::IMDB.to_string(), id);
        }
        entry
    }
}

/// `/movies/{id}` response. Field set mirrors the keys list returned
/// by the user's curl probe. Box-office fields are deserialized but
/// not yet projected into PluginEntry (no canonical entry field for
/// them today). Add later if the catalog grows a money column.
#[derive(Debug, Deserialize)]
struct MovieDetail {
    #[serde(default)] id:                String,
    #[serde(default)] title:             String,
    /// Acknowledged but not yet projected onto PluginEntry — the entry
    /// shape has no `original_title` field today.
    #[serde(default, rename = "original_title")] _original_title: Option<String>,
    #[serde(default)] release_year:      Option<u32>,
    /// Acknowledged; PluginEntry only carries `year` today, not full date.
    #[serde(default, rename = "release_date")]   _release_date:   Option<String>,
    #[serde(default)] runtime_minutes:   Option<u32>,
    /// IMDb-shaped 0–10. Maps to entry.ratings["imdb"] and entry.rating.
    #[serde(default)] rating:            Option<f32>,
    /// Acknowledged; the rating aggregator could use this for Bayesian
    /// shrinkage but the wire shape doesn't carry per-source vote counts
    /// today. Surface alongside `rating_votes["imdb"]` once added.
    #[serde(default, rename = "vote_count")]     _vote_count:     Option<u32>,
    /// Metacritic 0–100. Maps to entry.ratings["metacritic"].
    #[serde(default)] metascore:         Option<f32>,
    #[serde(default)] genres:            Vec<String>,
    #[serde(default)] plot:              Option<String>,
    #[serde(default)] poster_url:        Option<String>,
    #[serde(default)] imdb_url:          Option<String>,
    #[serde(default)] title_type:        Option<String>,
    #[serde(default)] full_cast_and_crew: Vec<CreditEntry>,
    // Acknowledged but not consumed today:
    #[serde(default)] _budget:           Option<u64>,
    #[serde(default)] _worldwide_gross:  Option<u64>,
    #[serde(default)] _certificate:      Option<String>,
}

impl MovieDetail {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let imdb = self.imdb_url.as_deref().and_then(imdb_id_from_url);
        // Project rating + metascore into the per-source ratings map so
        // the catalog aggregator's weighted_median picks them up under
        // the same `imdb` / `metacritic` keys omdb-provider uses.
        let mut ratings = std::collections::HashMap::new();
        if let Some(r) = self.rating {
            ratings.insert("imdb".to_string(), r);
        }
        if let Some(m) = self.metascore {
            ratings.insert("metacritic".to_string(), m);
        }
        let id_for_entry = imdb.clone().unwrap_or_else(|| {
            if !self.id.is_empty() { self.id.clone() } else { self.title.clone() }
        });
        let mut entry = PluginEntry {
            id:          id_for_entry,
            kind,
            source:      "xmdb".to_string(),
            title:       self.title,
            year:        self.release_year,
            poster_url:  self.poster_url,
            imdb_id:     imdb.clone(),
            description: self.plot,
            genre:       Some(self.genres.join(", ")).filter(|s| !s.is_empty()),
            rating:      self.rating,
            duration:    self.runtime_minutes,
            ratings,
            ..Default::default()
        };
        if let Some(id) = imdb {
            entry.external_ids.insert(id_sources::IMDB.to_string(), id);
        }
        // Surface the xmdb id alongside imdb for callers that want to
        // look up via xmdb directly later.
        if !self.id.is_empty() {
            entry.external_ids.insert("xmdb".to_string(), self.id);
        }
        let _ = kind; // kind is recorded via the field above
        entry
    }
}

/// Credits row. xmdb may use `role` for cast members (e.g. "actor")
/// and crew members alike, with an additional `character` field for
/// cast and `department` for crew. The exact field names aren't
/// documented; defaulting `role` to "crew" is the safe fallback.
#[derive(Debug, Deserialize)]
struct CreditEntry {
    #[serde(default)] name:       String,
    #[serde(default)] role:       Option<String>,
    #[serde(default)] department: Option<String>,
    #[serde(default)] character:  Option<String>,
}

impl CreditEntry {
    fn is_cast(&self) -> bool {
        // Treat anyone with a `character` populated, or a role string
        // matching "cast"/"actor"/"actress", as cast. Everyone else is
        // crew.
        if self.character.as_deref().map(|c| !c.is_empty()).unwrap_or(false) {
            return true;
        }
        let r = self.role.as_deref().unwrap_or("").to_ascii_lowercase();
        r == "cast" || r == "actor" || r == "actress"
    }
}

#[derive(Debug, Deserialize)]
struct SeasonResponse {
    #[serde(default)] episodes: Vec<RawEpisode>,
}

#[derive(Debug, Deserialize)]
struct RawEpisode {
    #[serde(default)] title:           Option<String>,
    #[serde(default)] release_date:    Option<String>,
    #[serde(default)] episode_number:  Option<u32>,
    #[serde(default)] runtime_minutes: Option<u32>,
    #[serde(default)] imdb_url:        Option<String>,
    #[serde(default)] imdb_id:         Option<String>,
}

// ── WASM exports ──────────────────────────────────────────────────────────────

impl stui_plugin_sdk::StreamProvider for XmdbPlugin {}

stui_export_catalog_plugin!(XmdbPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn assert_plugin<T: Plugin>() {}
        fn assert_catalog<T: CatalogPlugin>() {}
        assert_plugin::<XmdbPlugin>();
        assert_catalog::<XmdbPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = XmdbPlugin::new();
        assert_eq!(p.manifest().plugin.name, "xmdb");
    }

    #[test]
    fn imdb_id_extracted_from_url() {
        assert_eq!(
            imdb_id_from_url("https://www.imdb.com/title/tt0111161/"),
            Some("tt0111161".to_string()),
        );
        assert_eq!(
            imdb_id_from_url("https://www.imdb.com/title/tt0111161"),
            Some("tt0111161".to_string()),
        );
        assert_eq!(imdb_id_from_url("not a url"), None);
        assert_eq!(imdb_id_from_url("https://example.com/foo/bar"), None);
    }

    #[test]
    fn opt_non_empty_strips_blank() {
        assert_eq!(opt_non_empty(""), None);
        assert_eq!(opt_non_empty("   "), None);
        assert_eq!(opt_non_empty("hello"), Some("hello".to_string()));
    }

    #[test]
    fn movie_detail_projects_ratings() {
        let detail = MovieDetail {
            id:                 "xmdb-123".into(),
            title:              "The Shawshank Redemption".into(),
            _original_title:    None,
            release_year:       Some(1994),
            _release_date:      Some("1994-09-23".into()),
            runtime_minutes:    Some(142),
            rating:             Some(9.3),
            _vote_count:        Some(2_700_000),
            metascore:          Some(82.0),
            genres:             vec!["Drama".into()],
            plot:               Some("...".into()),
            poster_url:         Some("https://example.com/p.jpg".into()),
            imdb_url:           Some("https://www.imdb.com/title/tt0111161/".into()),
            title_type:         Some("movie".into()),
            full_cast_and_crew: vec![],
            _budget:            None,
            _worldwide_gross:   None,
            _certificate:       None,
        };
        let e = detail.into_entry(EntryKind::Movie);
        assert_eq!(e.kind, EntryKind::Movie);
        assert_eq!(e.source, "xmdb");
        assert_eq!(e.year, Some(1994));
        assert_eq!(e.imdb_id.as_deref(), Some("tt0111161"));
        assert_eq!(e.external_ids.get(id_sources::IMDB).map(String::as_str), Some("tt0111161"));
        assert_eq!(e.external_ids.get("xmdb").map(String::as_str), Some("xmdb-123"));
        assert_eq!(e.rating, Some(9.3));
        assert_eq!(e.ratings.get("imdb").copied(), Some(9.3));
        assert_eq!(e.ratings.get("metacritic").copied(), Some(82.0));
        assert_eq!(e.duration, Some(142));
        assert_eq!(e.genre.as_deref(), Some("Drama"));
    }

    #[test]
    fn movie_detail_handles_missing_metascore() {
        let detail = MovieDetail {
            id: "xmdb-123".into(),
            title: "Untracked".into(),
            _original_title: None,
            release_year: None,
            _release_date: None,
            runtime_minutes: None,
            rating: Some(7.0),
            _vote_count: None,
            metascore: None,
            genres: vec![],
            plot: None,
            poster_url: None,
            imdb_url: None,
            title_type: None,
            full_cast_and_crew: vec![],
            _budget: None,
            _worldwide_gross: None,
            _certificate: None,
        };
        let e = detail.into_entry(EntryKind::Movie);
        assert_eq!(e.ratings.get("imdb").copied(), Some(7.0));
        assert!(e.ratings.get("metacritic").is_none());
    }

    #[test]
    fn credit_entry_is_cast_via_character() {
        let c = CreditEntry {
            name: "Tim Robbins".into(),
            role: None,
            department: None,
            character: Some("Andy Dufresne".into()),
        };
        assert!(c.is_cast());
    }

    #[test]
    fn credit_entry_is_cast_via_role() {
        let c = CreditEntry {
            name: "Morgan Freeman".into(),
            role: Some("actor".into()),
            department: None,
            character: None,
        };
        assert!(c.is_cast());
    }

    #[test]
    fn credit_entry_default_to_crew() {
        let c = CreditEntry {
            name: "Frank Darabont".into(),
            role: Some("director".into()),
            department: Some("Directing".into()),
            character: None,
        };
        assert!(!c.is_cast());
    }

    #[test]
    fn build_episodes_uses_imdb_id_when_present() {
        let raw = vec![
            RawEpisode {
                title:           Some("Pilot".into()),
                release_date:    Some("2008-01-20".into()),
                episode_number:  Some(1),
                runtime_minutes: Some(58),
                imdb_url:        Some("https://www.imdb.com/title/tt0959621/".into()),
                imdb_id:         None,
            },
        ];
        let eps = build_episodes("tt0903747", 1, raw);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].entry_id, "xmdb-tt0959621");
        assert_eq!(eps[0].provider, "xmdb");
        assert_eq!(eps[0].runtime_mins, Some(58));
    }

    #[test]
    fn build_episodes_synthesises_when_imdb_missing() {
        let raw = vec![RawEpisode {
            title:           None,
            release_date:    None,
            episode_number:  Some(2),
            runtime_minutes: None,
            imdb_url:        None,
            imdb_id:         None,
        }];
        let eps = build_episodes("tt0903747", 1, raw);
        assert_eq!(eps[0].entry_id, "xmdb-tt0903747:s1e2");
        assert_eq!(eps[0].title, "Episode 2");
    }

    #[test]
    fn lookup_rejects_non_imdb_id_source() {
        let p = XmdbPlugin::new_for_test("fake");
        let req = LookupRequest {
            id: "12345".into(),
            id_source: id_sources::TMDB.to_string(),
            kind: EntryKind::Movie,
            locale: None,
        };
        match p.lookup(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            PluginResult::Ok(_)  => panic!("expected UNKNOWN_ID"),
        }
    }

    #[test]
    fn enrich_requires_imdb_id() {
        let p = XmdbPlugin::new_for_test("fake");
        let req = EnrichRequest {
            partial: PluginEntry {
                title: "No-id Movie".into(),
                kind: EntryKind::Movie,
                ..Default::default()
            },
            prefer_id_source: None,
        };
        match p.enrich(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            PluginResult::Ok(_)  => panic!("expected UNKNOWN_ID"),
        }
    }

    #[test]
    fn new_for_test_caches_api_key() {
        let p = XmdbPlugin::new_for_test("fake");
        assert_eq!(p.api_key().unwrap(), "fake");
    }

    /// End-to-end through MockHost. Stubs /movies/{imdb_id} with a
    /// canned response and checks the parsed PluginEntry.
    #[test]
    fn lookup_roundtrips_through_mock_host() {
        use stui_plugin_sdk::testing::MockHost;

        MockHost::reset();
        let fixture = r#"{
            "id": "xmdb-1",
            "title": "The Shawshank Redemption",
            "release_year": 1994,
            "release_date": "1994-09-23",
            "runtime_minutes": 142,
            "rating": 9.3,
            "vote_count": 2700000,
            "metascore": 82,
            "genres": ["Drama"],
            "plot": "Two imprisoned men...",
            "poster_url": "https://p/1.jpg",
            "imdb_url": "https://www.imdb.com/title/tt0111161/",
            "title_type": "movie",
            "full_cast_and_crew": []
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0111161?apiKey=fake",
            fixture,
        );

        let plugin = XmdbPlugin::new_for_test("fake");
        let req = LookupRequest {
            id: "tt0111161".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            locale: None,
        };
        let resp = match plugin.lookup(req) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("lookup Err {}: {}", e.code, e.message),
        };
        assert_eq!(resp.entry.title, "The Shawshank Redemption");
        assert_eq!(resp.entry.year, Some(1994));
        assert_eq!(resp.entry.imdb_id.as_deref(), Some("tt0111161"));
        assert_eq!(resp.entry.ratings.get("imdb").copied(), Some(9.3));
        assert_eq!(resp.entry.ratings.get("metacritic").copied(), Some(82.0));
    }
}
