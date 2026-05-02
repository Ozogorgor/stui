//! OMDb metadata provider — movies and series via the Open Movie Database.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup, enrich,
//! get_credits, episodes}`. OMDb has no related / artwork-catalog
//! endpoints, so those verbs default to `NOT_IMPLEMENTED` from the trait.
//!
//! ## API key
//!
//! Required. Read from `InitContext.config["api_key"]` at `Plugin::init`.
//! Fallback: `OMDB_API_KEY` env var surfaced by the host through
//! `cache_get("__env:OMDB_API_KEY")`.

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

const BASE_URL: &str = "https://www.omdbapi.com/";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct OmdbPlugin {
    manifest: PluginManifest,
    api_key: OnceLock<String>,
}

impl OmdbPlugin {
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
        let env_key = cache_get("__env:OMDB_API_KEY").unwrap_or_default();
        if env_key.is_empty() {
            return Err(PluginError {
                code: error_codes::INVALID_REQUEST.to_string(),
                message: "OMDb api_key not configured".to_string(),
            });
        }
        Ok(self.api_key.get_or_init(|| env_key).as_str())
    }
}

impl Default for OmdbPlugin {
    fn default() -> Self { Self::new() }
}

impl Plugin for OmdbPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        let key = ctx
            .config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("OMDB_API_KEY").cloned())
            .unwrap_or_default();
        if key.is_empty() {
            return Err(PluginInitError::MissingConfig {
                fields: vec!["api_key".to_string()],
                hint: Some("Get a free key at www.omdbapi.com/apikey.aspx".to_string()),
            });
        }
        let _ = self.api_key.set(key);
        Ok(())
    }
}

// ── Error classification ──────────────────────────────────────────────────────

/// The SDK's `http_get` surfaces non-2xx responses as `Err("HTTP {code}: {body}")`.
/// Re-classify the code into one of our canonical error codes.
fn classify_http_err(err: &str) -> PluginError {
    if let Some(rest) = err.strip_prefix("HTTP ") {
        if let Some((code_str, body)) = rest.split_once(": ") {
            if let Ok(status) = code_str.parse::<u16>() {
                let code = match status {
                    401 | 402 => error_codes::INVALID_REQUEST,   // bad/exhausted key
                    404         => error_codes::UNKNOWN_ID,
                    429         => error_codes::RATE_LIMITED,
                    500..=599   => error_codes::TRANSIENT,
                    _           => error_codes::REMOTE_ERROR,
                };
                return PluginError {
                    code: code.to_string(),
                    message: format!("OMDb HTTP {status}: {body}"),
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
        plugin_error!("omdb: parse error: {}", e);
        PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("OMDb JSON parse failure: {e}"),
        }
    })
}

/// Parse the leading 4-digit year from a value like `"2020"` or `"2020–2023"`.
fn parse_year(raw: &str) -> Option<u32> {
    raw.split('–')
        .next()
        .and_then(|y| y.trim().parse::<u32>().ok())
}

fn opt_non_na(s: &str) -> Option<String> {
    if s.is_empty() || s == "N/A" { None } else { Some(s.to_string()) }
}

fn type_param(kind: EntryKind) -> Option<&'static str> {
    match kind {
        EntryKind::Movie  => Some("movie"),
        EntryKind::Series => Some("series"),
        _                 => None,
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for OmdbPlugin {
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
                    "omdb only supports movie and series scopes",
                );
            }
        };

        let query = req.query.trim();
        // OMDb has no trending/browse endpoint; an empty query yields zero results.
        if query.is_empty() {
            return PluginResult::ok(SearchResponse { items: vec![], total: 0 });
        }

        let tp = type_param(entry_kind).expect("checked by scope match above");
        let url = format!(
            "{BASE_URL}?s={}&type={}&apikey={}",
            urlencoding::encode(query),
            tp,
            api_key,
        );
        plugin_info!("omdb: search {} (type={tp})", query);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        let raw: SearchResponseRaw = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };

        // OMDb returns `Response: "False"` + `Error: "..."` when no matches or
        // on non-HTTP errors (e.g. invalid key that they surface with 200).
        if raw.response.eq_ignore_ascii_case("false") {
            return PluginResult::ok(SearchResponse { items: vec![], total: 0 });
        }

        let limit = if req.limit == 0 { usize::MAX } else { req.limit as usize };
        let items: Vec<PluginEntry> = raw
            .search
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .map(|s| s.into_entry(entry_kind))
            .collect();
        let total = raw.total_results.unwrap_or(items.len() as u32);
        plugin_info!("omdb: {} entries", items.len());
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("omdb lookup only supports imdb ids, got: {}", req.id_source),
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };

        let url = format!(
            "{BASE_URL}?i={}&plot=full&apikey={}",
            urlencoding::encode(&req.id),
            api_key,
        );
        plugin_info!("omdb: lookup {} (imdb)", req.id);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let detail: DetailResponse = match parse_json(&body) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };

        if detail.response.eq_ignore_ascii_case("false") {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                detail.error.unwrap_or_else(|| format!("no OMDb entry for imdb {}", req.id)),
            );
        }

        // Upstream `Type` can be `movie`, `series`, `episode`, `game`. We only
        // map the first two; anything else gets squashed to the request kind.
        let kind = match detail.media_type.as_deref() {
            Some("movie")   => EntryKind::Movie,
            Some("series")  => EntryKind::Series,
            _               => req.kind,
        };

        PluginResult::ok(LookupResponse { entry: detail.into_entry(kind) })
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        // Fast path: partial already carries an IMDB id (the only id_source
        // OMDb natively understands). Reuse lookup verbatim.
        let imdb = req
            .partial
            .external_ids
            .get(id_sources::IMDB)
            .cloned()
            .or_else(|| req.partial.imdb_id.clone());
        if let Some(imdb_id) = imdb {
            let lookup_req = LookupRequest {
                id: imdb_id,
                id_source: id_sources::IMDB.to_string(),
                kind: req.partial.kind,
                locale: None,
                force_refresh: false,
            };
            return match self.lookup(lookup_req) {
                PluginResult::Ok(r) => PluginResult::ok(EnrichResponse {
                    entry: r.entry,
                    confidence: 1.0,
                }),
                PluginResult::Err(e) => PluginResult::Err(e),
            };
        }

        // Fallback: title + optional year search via OMDb's `?t=` endpoint
        // (single best-match by title). `?s=` returns a list, but `?t=` is
        // both cheaper and reflects OMDb's own match-quality preference.
        let title = req.partial.title.trim();
        if title.is_empty() {
            return PluginResult::err(
                error_codes::INVALID_REQUEST,
                "omdb enrich: empty title and no imdb id",
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let mut url = format!(
            "{BASE_URL}?t={}&plot=full&apikey={}",
            urlencoding::encode(title),
            api_key,
        );
        if let Some(y) = req.partial.year {
            url.push_str(&format!("&y={y}"));
        }
        if let Some(t) = type_param(req.partial.kind) {
            url.push_str(&format!("&type={t}"));
        }
        plugin_info!("omdb: enrich title-search {}", title);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let detail: DetailResponse = match parse_json(&body) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        if detail.response.eq_ignore_ascii_case("false") {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                detail.error.unwrap_or_else(|| format!("omdb: no match for '{title}'")),
            );
        }
        let kind = match detail.media_type.as_deref() {
            Some("movie")  => EntryKind::Movie,
            Some("series") => EntryKind::Series,
            _              => req.partial.kind,
        };
        // Title-search match is less precise than imdb-id lookup; reflect
        // that in the confidence score.
        PluginResult::ok(EnrichResponse {
            entry: detail.into_entry(kind),
            confidence: 0.7,
        })
    }

    fn get_credits(&self, req: CreditsRequest) -> PluginResult<CreditsResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("omdb credits only supports imdb ids, got: {}", req.id_source),
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let url = format!(
            "{BASE_URL}?i={}&plot=short&apikey={}",
            urlencoding::encode(&req.id),
            api_key,
        );
        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let detail: DetailResponse = match parse_json(&body) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        if detail.response.eq_ignore_ascii_case("false") {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                detail.error.unwrap_or_else(|| format!("omdb: no entry for {}", req.id)),
            );
        }

        // OMDb returns Director / Writer / Actors as comma-separated name
        // strings (no character names, no billing order). Best-effort split
        // and tag with the appropriate role.
        let mut crew: Vec<CrewMember> = Vec::new();
        for name in split_names(&detail.director) {
            crew.push(CrewMember {
                name,
                role: normalize_crew_role("director"),
                department: Some("Directing".to_string()),
                external_ids: Default::default(),
            });
        }
        for name in split_names(&detail.writer) {
            crew.push(CrewMember {
                name,
                role: normalize_crew_role("writer"),
                department: Some("Writing".to_string()),
                external_ids: Default::default(),
            });
        }
        let cast: Vec<CastMember> = split_names(&detail.actors)
            .into_iter()
            .enumerate()
            .map(|(i, name)| CastMember {
                name,
                role: CastRole::Actor,
                character: None, // OMDb doesn't expose character names
                instrument: None,
                billing_order: Some(i as u32 + 1),
                external_ids: Default::default(),
            })
            .collect();

        PluginResult::ok(CreditsResponse { cast, crew })
    }

    fn episodes(&self, req: EpisodesRequest) -> PluginResult<EpisodesResponse> {
        // OMDb's plugin id is "omdb" but its canonical id source is "imdb"
        // (every OMDb id IS an imdb tt-id). The runtime's episodes
        // dispatcher routes by plugin id, so `req.id_source` arrives as
        // `"omdb"`; older callers that key by canonical source send
        // `"imdb"`. Accept both — anything else is genuinely wrong.
        if req.id_source != id_sources::IMDB && req.id_source != "omdb" {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("omdb episodes only supports imdb/omdb id_source, got: {}", req.id_source),
            );
        }
        if req.season < 1 {
            return PluginResult::err(
                error_codes::INVALID_REQUEST,
                "omdb episodes: season must be >= 1",
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };

        let url = format!(
            "{BASE_URL}?i={}&Season={}&apikey={}",
            urlencoding::encode(&req.series_id),
            req.season,
            api_key,
        );
        plugin_info!("omdb: episodes id={} season={}", req.series_id, req.season);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let resp: SeasonResponse = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        if resp.response.eq_ignore_ascii_case("false") {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                resp.error.unwrap_or_else(|| format!(
                    "omdb: no season {} for {}", req.season, req.series_id,
                )),
            );
        }

        let episodes = build_episodes(&req.series_id, req.season, resp.episodes);
        PluginResult::ok(EpisodesResponse { episodes })
    }
}

// ── Episodes builder ──────────────────────────────────────────────────────────

/// Convert OMDb's per-season `Episodes[]` rows into wire shape.
///
/// OMDb's season endpoint returns title + release date + per-episode
/// `imdbID` but no per-episode runtime — fetching runtime would require
/// one extra request per episode (`?i={imdbID}`), which is a poor trade
/// for a list view. `runtime_mins` stays `None`.
///
/// `entry_id` is `"omdb-{imdbID}"` when present so the runtime's
/// future stream-resolve dispatcher can route on the prefix; falls back
/// to a synthetic `"omdb-{series}:s{S}e{E}"` when OMDb omits the imdb id
/// (rare but possible for unreleased episodes).
fn build_episodes(series_id: &str, season: u32, raw: Vec<RawEpisode>) -> Vec<EpisodeWire> {
    raw.into_iter()
        .map(|ep| {
            let n = ep.episode.trim().parse::<u32>().unwrap_or(0);
            let title = match opt_non_na(&ep.title) {
                Some(t) => t,
                None    => format!("Episode {n}"),
            };
            let imdb = opt_non_na(&ep.imdb_id);
            let entry_id = match imdb {
                Some(id) => format!("omdb-{id}"),
                None     => format!("omdb-{series_id}:s{season}e{n}"),
            };
            EpisodeWire {
                season,
                episode: n,
                title,
                air_date: opt_non_na(&ep.released),
                runtime_mins: None,
                provider: "omdb".to_string(),
                entry_id,
            }
        })
        .collect()
}

/// Split OMDb's comma-separated name lists into trimmed individual names.
/// Filters out `"N/A"` (OMDb's literal placeholder) and empty entries.
fn split_names(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() || raw == "N/A" {
        return Vec::new();
    }
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "N/A")
        .collect()
}

// ── API types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponseRaw {
    #[serde(rename = "Search", default)]
    search: Option<Vec<SearchResult>>,
    #[serde(rename = "totalResults", default, deserialize_with = "de_opt_u32_string")]
    total_results: Option<u32>,
    #[serde(rename = "Response", default)]
    response: String,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(rename = "Title",  default)] title:     String,
    #[serde(rename = "Year",   default)] year:      String,
    #[serde(rename = "imdbID", default)] imdb_id:   String,
    #[serde(rename = "Poster", default)] poster:    String,
}

impl SearchResult {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let imdb = opt_non_na(&self.imdb_id);
        let mut entry = PluginEntry {
            id:          imdb.clone().unwrap_or_else(|| self.title.clone()),
            kind,
            source:      "omdb".to_string(),
            title:       self.title,
            year:        parse_year(&self.year),
            poster_url:  opt_non_na(&self.poster),
            imdb_id:     imdb.clone(),
            ..Default::default()
        };
        if let Some(id) = imdb {
            entry.external_ids.insert(id_sources::IMDB.to_string(), id);
        }
        entry
    }
}

#[derive(Debug, Deserialize)]
struct DetailResponse {
    #[serde(rename = "Response", default)]  response:  String,
    #[serde(rename = "Error",    default)]  error:     Option<String>,
    #[serde(rename = "Type",     default)]  media_type: Option<String>,
    #[serde(rename = "Title",    default)]  title:     String,
    #[serde(rename = "Year",     default)]  year:      String,
    #[serde(rename = "imdbID",   default)]  imdb_id:   String,
    #[serde(rename = "Poster",   default)]  poster:    String,
    #[serde(rename = "Plot",     default)]  plot:      String,
    #[serde(rename = "Genre",    default)]  genre:     String,
    #[serde(rename = "Runtime",  default)]  runtime:   String,
    #[serde(rename = "imdbRating", default)] rating:   String,
    #[serde(rename = "Director", default)]  director:  String,
    #[serde(rename = "Writer",   default)]  writer:    String,
    #[serde(rename = "Actors",   default)]  actors:    String,
    /// OMDb's `Ratings` array carries the multi-source breakdown:
    /// IMDb (X.Y/10), Rotten Tomatoes (X%), Metacritic (X/100). We
    /// project these into PluginEntry.ratings so the aggregator can
    /// compose a weighted composite using the catalog_engine's
    /// existing `imdb`/`tomatometer`/`metacritic` weight keys.
    #[serde(rename = "Ratings", default)]   ratings: Vec<RatingSource>,
}

#[derive(Debug, Deserialize)]
struct RatingSource {
    #[serde(rename = "Source", default)] source: String,
    #[serde(rename = "Value",  default)] value:  String,
}

/// Parse a single OMDb rating value into (aggregator_key, score)
/// where `score` is on the upstream's native scale. Returns None for
/// unrecognised sources or values that don't parse — those simply
/// don't contribute to the composite.
///
/// Format examples seen in OMDb responses:
///   - `Internet Movie Database` → `"8.4/10"`        → ("imdb", 8.4)
///   - `Rotten Tomatoes`         → `"92%"`           → ("tomatometer", 92.0)
///   - `Metacritic`              → `"78/100"`        → ("metacritic", 78.0)
fn parse_omdb_rating_source(r: &RatingSource) -> Option<(&'static str, f32)> {
    let key = match r.source.as_str() {
        "Internet Movie Database" => "imdb",
        "Rotten Tomatoes"         => "tomatometer",
        "Metacritic"              => "metacritic",
        _                          => return None,
    };
    let v = r.value.trim();
    // Strip the suffix to extract just the numeric portion. OMDb is
    // consistent enough that splitting on '/' and '%' handles every
    // form we've seen, but `parse` errors fall through to None.
    let num_str = v.split_once('/').map(|(n, _)| n).unwrap_or_else(|| v.trim_end_matches('%'));
    num_str.trim().parse::<f32>().ok().map(|n| (key, n))
}

impl DetailResponse {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let imdb = opt_non_na(&self.imdb_id);
        // Project the multi-source Ratings[] block into PluginEntry's
        // ratings map. Unrecognised sources are ignored.
        let mut ratings = std::collections::HashMap::new();
        for r in &self.ratings {
            if let Some((key, score)) = parse_omdb_rating_source(r) {
                ratings.insert(key.to_string(), score);
            }
        }
        let mut entry = PluginEntry {
            id:          imdb.clone().unwrap_or_else(|| self.title.clone()),
            kind,
            source:      "omdb".to_string(),
            title:       self.title,
            year:        parse_year(&self.year),
            poster_url:  opt_non_na(&self.poster),
            imdb_id:     imdb.clone(),
            description: opt_non_na(&self.plot),
            genre:       opt_non_na(&self.genre),
            rating:      opt_non_na(&self.rating).and_then(|s| s.parse::<f32>().ok()),
            duration:    parse_runtime_minutes(&self.runtime),
            ratings,
            ..Default::default()
        };
        if let Some(id) = imdb {
            entry.external_ids.insert(id_sources::IMDB.to_string(), id);
        }
        entry
    }
}

/// `"142 min"` → `Some(142)`. Anything else → `None`.
fn parse_runtime_minutes(raw: &str) -> Option<u32> {
    raw.split_whitespace().next().and_then(|n| n.parse::<u32>().ok())
}

#[derive(Debug, Deserialize)]
struct SeasonResponse {
    #[serde(rename = "Response", default)] response: String,
    #[serde(rename = "Error",    default)] error:    Option<String>,
    #[serde(rename = "Episodes", default)] episodes: Vec<RawEpisode>,
}

#[derive(Debug, Deserialize)]
struct RawEpisode {
    #[serde(rename = "Title",    default)] title:    String,
    #[serde(rename = "Released", default)] released: String,
    #[serde(rename = "Episode",  default)] episode:  String,
    #[serde(rename = "imdbID",   default)] imdb_id:  String,
}

/// OMDb stringifies `totalResults`; accept both string and number shapes.
fn de_opt_u32_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    use serde::de::{self, Visitor};
    use std::fmt;

    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<u32>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "u32 as string or number, or null")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.trim().parse::<u32>().ok())
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> { self.visit_str(&v) }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> { Ok(Some(v as u32)) }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> { Ok(if v < 0 { None } else { Some(v as u32) }) }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> { Ok(None) }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> { Ok(None) }
        fn visit_some<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_any(V)
        }
    }
    d.deserialize_any(V)
}

// ── WASM exports ──────────────────────────────────────────────────────────────

impl stui_plugin_sdk::StreamProvider for OmdbPlugin {}

stui_export_catalog_plugin!(OmdbPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn assert_plugin<T: Plugin>() {}
        fn assert_catalog<T: CatalogPlugin>() {}
        assert_plugin::<OmdbPlugin>();
        assert_catalog::<OmdbPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = OmdbPlugin::new();
        assert_eq!(p.manifest().plugin.name, "omdb");
    }

    #[test]
    fn parse_year_single() {
        assert_eq!(parse_year("2020"), Some(2020));
    }

    #[test]
    fn parse_year_range_takes_first() {
        assert_eq!(parse_year("2020–2023"), Some(2020));
    }

    #[test]
    fn parse_year_junk_returns_none() {
        assert_eq!(parse_year("N/A"), None);
        assert_eq!(parse_year(""), None);
    }

    #[test]
    fn opt_non_na_strips_sentinel() {
        assert_eq!(opt_non_na("N/A"), None);
        assert_eq!(opt_non_na(""), None);
        assert_eq!(opt_non_na("hello"), Some("hello".to_string()));
    }

    #[test]
    fn parse_runtime_minutes_handles_common_shape() {
        assert_eq!(parse_runtime_minutes("142 min"), Some(142));
        assert_eq!(parse_runtime_minutes("N/A"), None);
        assert_eq!(parse_runtime_minutes(""), None);
    }

    #[test]
    fn search_result_into_entry_populates_external_imdb() {
        let s = SearchResult {
            title: "Inception".into(),
            year: "2010".into(),
            imdb_id: "tt1375666".into(),
            poster: "https://example.com/p.jpg".into(),
        };
        let e = s.into_entry(EntryKind::Movie);
        assert_eq!(e.kind, EntryKind::Movie);
        assert_eq!(e.source, "omdb");
        assert_eq!(e.year, Some(2010));
        assert_eq!(e.imdb_id.as_deref(), Some("tt1375666"));
        assert_eq!(e.external_ids.get(id_sources::IMDB).map(String::as_str), Some("tt1375666"));
        assert_eq!(e.poster_url.as_deref(), Some("https://example.com/p.jpg"));
    }

    #[test]
    fn detail_response_no_not_found_to_unknown_id() {
        let body = r#"{"Response":"False","Error":"Incorrect IMDb ID."}"#;
        let d: DetailResponse = serde_json::from_str(body).unwrap();
        assert!(d.response.eq_ignore_ascii_case("false"));
        assert_eq!(d.error.as_deref(), Some("Incorrect IMDb ID."));
    }

    #[test]
    fn search_response_totalresults_accepts_string_and_number() {
        let a: SearchResponseRaw =
            serde_json::from_str(r#"{"Response":"True","totalResults":"42"}"#).unwrap();
        assert_eq!(a.total_results, Some(42));
        let b: SearchResponseRaw =
            serde_json::from_str(r#"{"Response":"True","totalResults":7}"#).unwrap();
        assert_eq!(b.total_results, Some(7));
    }

    #[test]
    fn detail_rating_parses_float() {
        let body = r#"{"Response":"True","Title":"A","Year":"2020","imdbID":"tt1","Type":"movie","imdbRating":"8.4"}"#;
        let d: DetailResponse = serde_json::from_str(body).unwrap();
        let e = d.into_entry(EntryKind::Movie);
        assert_eq!(e.rating, Some(8.4));
    }

    #[test]
    fn new_for_test_caches_api_key() {
        let p = OmdbPlugin::new_for_test("fake");
        assert_eq!(p.api_key().unwrap(), "fake");
    }

    /// End-to-end demonstration of `sdk::testing::MockHost`: stub OMDb's
    /// `?s=...&type=movie&apikey=...` endpoint with canned JSON and verify
    /// that `search()` parses and routes entries all the way through.
    #[test]
    fn search_roundtrips_through_mock_host() {
        use stui_plugin_sdk::{testing::MockHost, SearchRequest, SearchScope};

        MockHost::reset();
        let fixture = r#"{
            "Search":[
                {"Title":"Inception","Year":"2010","imdbID":"tt1375666","Type":"movie","Poster":"https://p/1.jpg"},
                {"Title":"Inception: The Cobol Job","Year":"2010","imdbID":"tt5295894","Type":"movie","Poster":"N/A"}
            ],
            "totalResults":"2",
            "Response":"True"
        }"#;
        let _h = MockHost::new().with_fixture_response(
            // Must match OMDb's URL shape exactly; includes the api_key we
            // stashed via `new_for_test`.
            "https://www.omdbapi.com/?s=inception&type=movie&apikey=fake",
            fixture,
        );

        let plugin = OmdbPlugin::new_for_test("fake");
        let req = SearchRequest {
            query: "inception".into(),
            scope: SearchScope::Movie,
            page: 1,
            limit: 0,
            per_scope_limit: None,
            locale: None,
        };
        let resp = match plugin.search(req) {
            stui_plugin_sdk::PluginResult::Ok(r) => r,
            stui_plugin_sdk::PluginResult::Err(e) => panic!("search Err {}: {}", e.code, e.message),
        };
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.total, 2);
        assert_eq!(resp.items[0].title, "Inception");
        assert_eq!(resp.items[0].imdb_id.as_deref(), Some("tt1375666"));
        assert_eq!(resp.items[0].poster_url.as_deref(), Some("https://p/1.jpg"));
        // Second entry has Poster=N/A which `opt_non_na` strips out.
        assert_eq!(resp.items[1].poster_url, None);
    }

    // ── Episodes verb ─────────────────────────────────────────────────────────

    #[test]
    fn build_episodes_uses_omdb_imdb_id_as_entry_id() {
        let raw = vec![
            RawEpisode {
                title: "Winter Is Coming".into(),
                released: "2011-04-17".into(),
                episode: "1".into(),
                imdb_id: "tt1480055".into(),
            },
        ];
        let eps = build_episodes("tt0944947", 1, raw);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].entry_id, "omdb-tt1480055");
        assert_eq!(eps[0].provider, "omdb");
        assert_eq!(eps[0].season, 1);
        assert_eq!(eps[0].episode, 1);
        assert_eq!(eps[0].title, "Winter Is Coming");
        assert_eq!(eps[0].air_date.as_deref(), Some("2011-04-17"));
        assert!(eps[0].runtime_mins.is_none());
    }

    #[test]
    fn build_episodes_synthesises_entry_id_when_imdb_missing() {
        let raw = vec![RawEpisode {
            title: "Future".into(),
            released: "N/A".into(),
            episode: "3".into(),
            imdb_id: "N/A".into(),
        }];
        let eps = build_episodes("tt0944947", 2, raw);
        assert_eq!(eps[0].entry_id, "omdb-tt0944947:s2e3");
        assert!(eps[0].air_date.is_none());
    }

    #[test]
    fn build_episodes_falls_back_to_episode_n_for_na_title() {
        let raw = vec![
            RawEpisode { title: "N/A".into(),  released: "".into(), episode: "1".into(), imdb_id: "tt1".into() },
            RawEpisode { title: "".into(),     released: "".into(), episode: "2".into(), imdb_id: "tt2".into() },
            RawEpisode { title: "Real".into(), released: "".into(), episode: "3".into(), imdb_id: "tt3".into() },
        ];
        let eps = build_episodes("tt0", 1, raw);
        assert_eq!(eps[0].title, "Episode 1");
        assert_eq!(eps[1].title, "Episode 2");
        assert_eq!(eps[2].title, "Real");
    }

    #[test]
    fn build_episodes_handles_empty_episodes_list() {
        assert!(build_episodes("tt0", 1, vec![]).is_empty());
    }

    #[test]
    fn episodes_rejects_non_imdb_or_omdb_id_source() {
        let p = OmdbPlugin::new_for_test("fake");
        let req = EpisodesRequest {
            series_id: "tt0944947".into(),
            id_source: id_sources::TMDB.to_string(),
            season: 1,
        };
        match p.episodes(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            PluginResult::Ok(_) => panic!("expected UNKNOWN_ID rejection"),
        }
    }

    #[test]
    fn episodes_rejects_zero_season() {
        let p = OmdbPlugin::new_for_test("fake");
        let req = EpisodesRequest {
            series_id: "tt0944947".into(),
            id_source: "omdb".into(),
            season: 0,
        };
        match p.episodes(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::INVALID_REQUEST),
            PluginResult::Ok(_) => panic!("expected INVALID_REQUEST rejection"),
        }
    }

    /// End-to-end through MockHost: stub the season endpoint and verify the
    /// plugin parses + maps episodes through the full call path. Mirrors the
    /// existing `search_roundtrips_through_mock_host` pattern.
    #[test]
    fn episodes_roundtrips_through_mock_host() {
        use stui_plugin_sdk::testing::MockHost;

        MockHost::reset();
        let fixture = r#"{
            "Title":"Game of Thrones",
            "Season":"1",
            "totalSeasons":"8",
            "Episodes":[
                {"Title":"Winter Is Coming","Released":"2011-04-17","Episode":"1","imdbRating":"9.1","imdbID":"tt1480055"},
                {"Title":"The Kingsroad","Released":"2011-04-24","Episode":"2","imdbRating":"8.8","imdbID":"tt1668746"}
            ],
            "Response":"True"
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://www.omdbapi.com/?i=tt0944947&Season=1&apikey=fake",
            fixture,
        );

        let plugin = OmdbPlugin::new_for_test("fake");
        let req = EpisodesRequest {
            series_id: "tt0944947".into(),
            id_source: "omdb".into(),
            season: 1,
        };
        let resp = match plugin.episodes(req) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("episodes Err {}: {}", e.code, e.message),
        };
        assert_eq!(resp.episodes.len(), 2);
        assert_eq!(resp.episodes[0].title, "Winter Is Coming");
        assert_eq!(resp.episodes[0].entry_id, "omdb-tt1480055");
        assert_eq!(resp.episodes[1].episode, 2);
        assert_eq!(resp.episodes[1].air_date.as_deref(), Some("2011-04-24"));
    }
}
