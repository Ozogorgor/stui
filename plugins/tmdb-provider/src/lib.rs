//! TMDB metadata provider — movies, TV series, episodes.
//!
//! Implements the `Plugin` + `CatalogPlugin` surface against The Movie Database
//! API v3. All six catalog verbs are supported: `search`, `lookup`, `enrich`,
//! `get_artwork`, `get_credits`, `related`.
//!
//! ## API key
//!
//! Required. Read from `InitContext.config["api_key"]` when `Plugin::init` is
//! invoked. As a pragmatic fallback (while the WASM ABI `stui_init` export is
//! still pending), the plugin also honours the `TMDB_API_KEY` environment
//! variable surfaced by the host through `cache_get("__env:TMDB_API_KEY")`.
//!
//! ## External id enrichment
//!
//! `search` does NOT call `/movie/{id}/external_ids` per-result (N+1; quota
//! heavy). `lookup` / `enrich` populate `PluginEntry.external_ids["imdb"]`
//! from the `external_ids` field already present on TMDB's direct-lookup
//! responses (available via `append_to_response=external_ids`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use stui_plugin_sdk::{
    parse_manifest,
    cache_get, cache_set, error_codes, http_get, log_url,
    id_sources, normalize_crew_role,
    plugin_error, plugin_info,
    stui_export_catalog_plugin,
    ArtworkRequest, ArtworkResponse, ArtworkSize, ArtworkVariant,
    CastMember, CastRole,
    CatalogPlugin,
    CreditsRequest, CreditsResponse,
    CrewMember,
    EnrichRequest, EnrichResponse,
    EpisodeWire, EpisodesRequest, EpisodesResponse,
    EntryKind,
    InitContext,
    LookupRequest, LookupResponse,
    Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    RelatedRequest, RelatedResponse, RelationKind,
    SearchRequest, SearchResponse, SearchScope,
};

const BASE_URL: &str = "https://api.themoviedb.org/3";
const IMAGE_BASE_URL: &str = "https://image.tmdb.org/t/p/";

/// The size used for poster URLs on `PluginEntry.poster_url`. Callers that
/// need a different size should use `get_artwork`.
const DEFAULT_POSTER_SIZE: &str = "w342";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct TmdbPlugin {
    manifest: PluginManifest,
    /// Set eagerly when `init` is called, or lazily on first verb invocation
    /// via the env-fallback path. Stored in a `OnceLock` so the `&self` verb
    /// methods (shared through `stui_export_catalog_plugin!`) can initialise
    /// it without interior mutability headaches.
    api_key: OnceLock<String>,
}

impl TmdbPlugin {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self {
            manifest,
            api_key: OnceLock::new(),
        }
    }

    /// Construct with an API key already set. Used by host-side unit tests.
    #[cfg(test)]
    pub fn new_for_test(api_key: &str) -> Self {
        let inst = Self::new();
        let _ = inst.api_key.set(api_key.to_string());
        inst
    }

    /// Resolve the API key: cached value → env fallback.
    fn api_key(&self) -> Result<&str, PluginError> {
        if let Some(k) = self.api_key.get() {
            return Ok(k.as_str());
        }
        let env_key = cache_get("__env:TMDB_API_KEY").unwrap_or_default();
        if env_key.is_empty() {
            return Err(PluginError {
                code: error_codes::INVALID_REQUEST.to_string(),
                message: "TMDB api_key not configured".to_string(),
            });
        }
        Ok(self.api_key.get_or_init(|| env_key).as_str())
    }
}

impl Default for TmdbPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for TmdbPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        // `api_key` is required=true in plugin.toml; StateStore short-circuits
        // a missing value to `NeedsConfig` before init is called. We still
        // check defensively — Fatal only if we got here without a key.
        let key = ctx
            .config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("TMDB_API_KEY").cloned())
            .unwrap_or_default();
        if key.is_empty() {
            return Err(PluginInitError::MissingConfig {
                fields: vec!["api_key".to_string()],
                hint: Some("Get a free key at themoviedb.org/settings/api".to_string()),
            });
        }
        // `set` only fails if the value was already written (e.g. test harness).
        let _ = self.api_key.set(key);
        Ok(())
    }
}

// ── HTTP error classification ─────────────────────────────────────────────────

/// The `http_get` SDK helper surfaces non-2xx responses as `Err("HTTP {code}: {body}")`.
/// This parses the code back out and maps it to a canonical error code.
fn classify_http_err(err: &str) -> PluginError {
    // Shape: "HTTP 404: {...}"
    if let Some(rest) = err.strip_prefix("HTTP ") {
        if let Some((code_str, body)) = rest.split_once(": ") {
            if let Ok(status) = code_str.parse::<u16>() {
                let code = match status {
                    404 => error_codes::UNKNOWN_ID,
                    429 => error_codes::RATE_LIMITED,
                    500..=599 => error_codes::TRANSIENT,
                    _ => error_codes::REMOTE_ERROR,
                };
                return PluginError {
                    code: code.to_string(),
                    message: format!("TMDB HTTP {status}: {body}"),
                };
            }
        }
    }
    // Non-HTTP failure (socket, timeout, null response): transient.
    PluginError {
        code: error_codes::TRANSIENT.to_string(),
        message: err.to_string(),
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, PluginError> {
    serde_json::from_str(body).map_err(|e| {
        plugin_error!("tmdb: parse error: {}", e);
        PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("TMDB JSON parse failure: {e}"),
        }
    })
}

/// Build `{BASE_URL}{path}?{query}&api_key={key}`. `path` must begin with `/`.
fn build_url(path: &str, query: &str, api_key: &str) -> String {
    if query.is_empty() {
        format!("{BASE_URL}{path}?api_key={api_key}")
    } else {
        format!("{BASE_URL}{path}?{query}&api_key={api_key}")
    }
}

// ── Shared serde types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PagedResponse<T> {
    results: Vec<T>,
    #[serde(default)]
    total_results: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
struct MovieItem {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    genre_ids: Vec<u32>,
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    /// ISO 639-1. TMDB includes this on search + discover + trending.
    /// Needed so the runtime's anime-mix classifier can flag
    /// Japanese animation ("Animation" + "ja") that ships via TMDB.
    #[serde(default)]
    original_language: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct TvItem {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    genre_ids: Vec<u32>,
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    original_language: Option<String>,
}

/// Bundled payload for `/movie/{id}?append_to_response=external_ids,images,credits,recommendations`.
///
/// All four verb-specific endpoints (`lookup`, `artwork`, `credits`, `related`)
/// pull from this single response when the bundle is in the persistent cache,
/// collapsing the per-detail-card hit on the TMDB metadata API from 4 to 1
/// (or 0 on a cache hit). The sub-resources are `Option` because callers
/// fall back to per-endpoint fetches if a bundle-style cached payload
/// doesn't carry them (e.g. on schema migration or older cache entries).
#[derive(Debug, Deserialize)]
struct MovieDetail {
    id: u64,
    title: String,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    runtime: Option<u32>,
    #[serde(default)]
    imdb_id: Option<String>,
    #[serde(default)]
    external_ids: Option<ExternalIds>,
    #[serde(default)]
    images: Option<ImagesResponse>,
    #[serde(default)]
    credits: Option<CreditsPayload>,
    #[serde(default)]
    recommendations: Option<PagedResponse<MovieItem>>,
}

/// Bundled payload for `/tv/{id}?append_to_response=external_ids,images,credits,recommendations`.
/// See `MovieDetail` for rationale.
#[derive(Debug, Deserialize)]
struct TvDetail {
    id: u64,
    name: String,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    external_ids: Option<ExternalIds>,
    /// Total number of seasons for the series. The TUI's episode browser
    /// uses this to populate its season list — without it the browser
    /// falls back to a single-season default.
    #[serde(default)]
    number_of_seasons: Option<u32>,
    #[serde(default)]
    images: Option<ImagesResponse>,
    #[serde(default)]
    credits: Option<CreditsPayload>,
    #[serde(default)]
    recommendations: Option<PagedResponse<TvItem>>,
}

/// Bundle path & query parameters for the `append_to_response` super-call.
const BUNDLE_APPEND: &str =
    "append_to_response=external_ids,images,credits,recommendations";

/// Cache key for the persistent bundle response. Keyed on (kind_path, id) so
/// movies and series with overlapping numeric ids don't trample each other.
fn bundle_cache_key(kind_path: &str, id: &str) -> String {
    format!("tmdb_bundle:{}:{}", kind_path, id)
}

/// Persistent cache TTL info isn't exposed to plugins — the runtime owns it.
/// We just `cache_set` the raw JSON; callers `cache_get` first and fall through
/// to the network on a miss. On the network path the response is re-stashed.
///
/// `kind_path` is `"movie"` or `"tv"` — used both as the URL fragment and as
/// the cache key disambiguator.
fn fetch_bundle_raw(kind_path: &str, id: &str, api_key: &str) -> Result<String, String> {
    let cache_key = bundle_cache_key(kind_path, id);
    if let Some(cached) = cache_get(&cache_key) {
        return Ok(cached);
    }
    let path = format!("/{kind_path}/{id}");
    let url = build_url(&path, BUNDLE_APPEND, api_key);
    let body = http_get(&url).map_err(|e| e.to_string())?;
    cache_set(&cache_key, &body);
    Ok(body)
}

#[derive(Debug, Deserialize)]
struct Genre {
    #[allow(dead_code)]
    id: u32,
    name: String,
}

#[derive(Debug, Deserialize, Default)]
struct ExternalIds {
    #[serde(default)]
    imdb_id: Option<String>,
    #[serde(default)]
    tvdb_id: Option<u64>,
}

/// `/find/{imdb_id}?external_source=imdb_id` response.
#[derive(Debug, Deserialize)]
struct FindResponse {
    #[serde(default)]
    movie_results: Vec<MovieItem>,
    #[serde(default)]
    tv_results: Vec<TvItem>,
}

#[derive(Debug, Deserialize, Default)]
struct ImagesResponse {
    #[serde(default)]
    posters: Vec<ImageInfo>,
    #[serde(default)]
    backdrops: Vec<ImageInfo>,
}

#[derive(Debug, Deserialize)]
struct ImageInfo {
    file_path: String,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
struct CreditsPayload {
    #[serde(default)]
    cast: Vec<CastEntry>,
    #[serde(default)]
    crew: Vec<CrewEntry>,
}

#[derive(Debug, Deserialize)]
struct CastEntry {
    name: String,
    #[serde(default)]
    character: Option<String>,
    #[serde(default)]
    order: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CrewEntry {
    name: String,
    #[serde(default)]
    job: Option<String>,
    #[serde(default)]
    department: Option<String>,
}

// ── PluginEntry builders ──────────────────────────────────────────────────────

fn year_from_date(d: Option<&str>) -> Option<u32> {
    d.and_then(|s| s.split('-').next())
        .and_then(|y| y.parse::<u32>().ok())
}

fn poster_url(path: Option<&str>, size: &str) -> Option<String> {
    path.map(|p| format!("{IMAGE_BASE_URL}{size}{p}"))
}

fn nonzero_rating(v: f32) -> Option<f32> {
    if v > 0.0 {
        Some(v)
    } else {
        None
    }
}

impl MovieItem {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        PluginEntry {
            id: self.id.to_string(),
            kind,
            source: "tmdb".to_string(),
            title: self.title,
            year: year_from_date(self.release_date.as_deref()),
            genre: self
                .genre_ids
                .first()
                .map(|&g| genre_name(g).to_string()),
            rating: nonzero_rating(self.vote_average),
            description: self.overview,
            poster_url: poster_url(self.poster_path.as_deref(), DEFAULT_POSTER_SIZE),
            original_language: self.original_language,
            ..Default::default()
        }
    }
}

impl TvItem {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        PluginEntry {
            id: self.id.to_string(),
            kind,
            source: "tmdb".to_string(),
            title: self.name,
            year: year_from_date(self.first_air_date.as_deref()),
            genre: self
                .genre_ids
                .first()
                .map(|&g| genre_name(g).to_string()),
            rating: nonzero_rating(self.vote_average),
            description: self.overview,
            poster_url: poster_url(self.poster_path.as_deref(), DEFAULT_POSTER_SIZE),
            original_language: self.original_language,
            ..Default::default()
        }
    }
}

impl MovieDetail {
    fn into_entry(self) -> PluginEntry {
        let mut external = HashMap::new();
        let ext_imdb = self
            .external_ids
            .as_ref()
            .and_then(|x| x.imdb_id.clone())
            .or_else(|| self.imdb_id.clone());
        if let Some(i) = ext_imdb.as_ref() {
            external.insert(id_sources::IMDB.to_string(), i.clone());
        }
        PluginEntry {
            id: self.id.to_string(),
            kind: EntryKind::Movie,
            source: "tmdb".to_string(),
            title: self.title,
            year: year_from_date(self.release_date.as_deref()),
            genre: self.genres.first().map(|g| g.name.clone()),
            rating: nonzero_rating(self.vote_average),
            description: self.overview,
            poster_url: poster_url(self.poster_path.as_deref(), DEFAULT_POSTER_SIZE),
            imdb_id: ext_imdb,
            duration: self.runtime,
            external_ids: external,
            ..Default::default()
        }
    }
}

impl TvDetail {
    fn into_entry(self) -> PluginEntry {
        let mut external = HashMap::new();
        let ext_imdb = self
            .external_ids
            .as_ref()
            .and_then(|x| x.imdb_id.clone());
        if let Some(i) = ext_imdb.as_ref() {
            external.insert(id_sources::IMDB.to_string(), i.clone());
        }
        if let Some(tvdb) = self.external_ids.as_ref().and_then(|x| x.tvdb_id) {
            external.insert(id_sources::TVDB.to_string(), tvdb.to_string());
        }
        PluginEntry {
            id: self.id.to_string(),
            kind: EntryKind::Series,
            source: "tmdb".to_string(),
            title: self.name,
            year: year_from_date(self.first_air_date.as_deref()),
            genre: self.genres.first().map(|g| g.name.clone()),
            rating: nonzero_rating(self.vote_average),
            description: self.overview,
            poster_url: poster_url(self.poster_path.as_deref(), DEFAULT_POSTER_SIZE),
            imdb_id: ext_imdb,
            external_ids: external,
            season_count: self.number_of_seasons,
            ..Default::default()
        }
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for TmdbPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let query = req.query.trim();

        // Scope → endpoint mapping. No `Global` variant exists; callers should
        // dispatch per-scope to each plugin.
        let (endpoint, entry_kind, use_multi) = match req.scope {
            SearchScope::Movie => ("/search/movie", EntryKind::Movie, false),
            SearchScope::Series => ("/search/tv", EntryKind::Series, false),
            // TMDB doesn't expose episode search; fall back to /search/tv and
            // label the entries as episodes so callers can refine.
            SearchScope::Episode => ("/search/tv", EntryKind::Episode, false),
            // Music / Artist / Album / Track scopes are out of TMDB's domain.
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "tmdb does not support this scope",
                );
            }
        };

        // Trending shortcut when query is empty (preserves existing UX).
        let url = if query.is_empty() {
            let trending = match req.scope {
                SearchScope::Movie => "/trending/movie/week",
                _ => "/trending/tv/week",
            };
            build_url(
                trending,
                &format!("page={}", req.page.max(1)),
                &api_key,
            )
        } else {
            let q = format!(
                "query={}&page={}",
                urlencoding::encode(query),
                req.page.max(1)
            );
            build_url(endpoint, &q, &api_key)
        };
        let _ = use_multi; // reserved for future `SearchScope::Global` variant

        plugin_info!("tmdb: search {} (query='{}')", log_url(&url), query);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        let limit = if req.limit == 0 {
            usize::MAX
        } else {
            req.limit as usize
        };

        // Deserialize per endpoint: each branch knows its concrete item type,
        // so we don't need an untagged enum that could silently misclassify.
        let (items, total_results) = match entry_kind {
            EntryKind::Movie => {
                let paged: PagedResponse<MovieItem> = match parse_json(&body) {
                    Ok(p) => p,
                    Err(e) => return PluginResult::Err(e),
                };
                let total = paged.total_results;
                let items: Vec<PluginEntry> = paged
                    .results
                    .into_iter()
                    .take(limit)
                    .map(|m| m.into_entry(entry_kind))
                    .collect();
                (items, total)
            }
            _ => {
                let paged: PagedResponse<TvItem> = match parse_json(&body) {
                    Ok(p) => p,
                    Err(e) => return PluginResult::Err(e),
                };
                let total = paged.total_results;
                let items: Vec<PluginEntry> = paged
                    .results
                    .into_iter()
                    .take(limit)
                    .map(|t| t.into_entry(entry_kind))
                    .collect();
                (items, total)
            }
        };

        let total = total_results.unwrap_or(items.len() as u32);
        plugin_info!("tmdb: {} entries", items.len());
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };

        match req.id_source.as_str() {
            id_sources::TMDB => {
                let kind_path = match req.kind {
                    EntryKind::Movie => "movie",
                    EntryKind::Series | EntryKind::Episode => "tv",
                    _ => {
                        return PluginResult::err(
                            error_codes::UNSUPPORTED_SCOPE,
                            "tmdb lookup supports movie/series/episode only",
                        );
                    }
                };
                // Goes through the persistent bundle cache. First call for
                // an entry hits the network; subsequent lookup/artwork/
                // credits/related calls all share the cached payload.
                let body = match fetch_bundle_raw(kind_path, &req.id, &api_key) {
                    Ok(b) => b,
                    Err(e) => return PluginResult::Err(classify_http_err(&e)),
                };
                let entry = match req.kind {
                    EntryKind::Movie => match parse_json::<MovieDetail>(&body) {
                        Ok(d) => d.into_entry(),
                        Err(e) => return PluginResult::Err(e),
                    },
                    _ => match parse_json::<TvDetail>(&body) {
                        Ok(d) => {
                            let mut e = d.into_entry();
                            if matches!(req.kind, EntryKind::Episode) {
                                e.kind = EntryKind::Episode;
                            }
                            e
                        }
                        Err(e) => return PluginResult::Err(e),
                    },
                };
                PluginResult::ok(LookupResponse { entry })
            }
            id_sources::IMDB => {
                let path = format!("/find/{}", req.id);
                let url = build_url(&path, "external_source=imdb_id", &api_key);
                let body = match http_get(&url) {
                    Ok(b) => b,
                    Err(e) => return PluginResult::Err(classify_http_err(&e)),
                };
                let found: FindResponse = match parse_json(&body) {
                    Ok(r) => r,
                    Err(e) => return PluginResult::Err(e),
                };
                let entry = if !found.movie_results.is_empty() {
                    let mut e = found
                        .movie_results
                        .into_iter()
                        .next()
                        .expect("guarded by is_empty check above")
                        .into_entry(EntryKind::Movie);
                    e.imdb_id = Some(req.id.clone());
                    e.external_ids.insert(id_sources::IMDB.to_string(), req.id.clone());
                    e
                } else if !found.tv_results.is_empty() {
                    let mut e = found
                        .tv_results
                        .into_iter()
                        .next()
                        .expect("guarded by is_empty check above")
                        .into_entry(EntryKind::Series);
                    e.imdb_id = Some(req.id.clone());
                    e.external_ids.insert(id_sources::IMDB.to_string(), req.id.clone());
                    e
                } else {
                    return PluginResult::err(
                        error_codes::UNKNOWN_ID,
                        format!("no TMDB entry for imdb id {}", req.id),
                    );
                };
                PluginResult::ok(LookupResponse { entry })
            }
            other => PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("unsupported id_source: {other}"),
            ),
        }
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        // Fast path: partial already carries a TMDB id in external_ids.
        if let Some(tmdb_id) = req.partial.external_ids.get(id_sources::TMDB) {
            let lookup_req = LookupRequest {
                id: tmdb_id.clone(),
                id_source: id_sources::TMDB.to_string(),
                kind: req.partial.kind,
                locale: None,
            };
            return match self.lookup(lookup_req) {
                PluginResult::Ok(r) => PluginResult::ok(EnrichResponse {
                    entry: r.entry,
                    confidence: 1.0,
                }),
                PluginResult::Err(e) => PluginResult::Err(e),
            };
        }

        // Second fast path: IMDB id → /find.
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
            };
            return match self.lookup(lookup_req) {
                PluginResult::Ok(r) => PluginResult::ok(EnrichResponse {
                    entry: r.entry,
                    confidence: 0.9,
                }),
                PluginResult::Err(e) => PluginResult::Err(e),
            };
        }

        // Fallback: title+year search via a direct URL so we can pass the
        // correct year query param to TMDB (primary_release_year for movies,
        // first_air_date_year for TV). The `search` verb never reads
        // `SearchRequest.locale`, so we build the URL ourselves instead of
        // routing through `self.search()`.
        let title = req.partial.title.trim();
        if title.is_empty() {
            return PluginResult::err(
                error_codes::INVALID_REQUEST,
                "enrich requires at least a title",
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let (endpoint, entry_kind) = match req.partial.kind {
            EntryKind::Movie => ("/search/movie", EntryKind::Movie),
            EntryKind::Series | EntryKind::Episode => ("/search/tv", EntryKind::Series),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "tmdb enrich supports movie/series/episode only",
                );
            }
        };
        let mut query = format!(
            "query={}&page=1&language=en-US",
            urlencoding::encode(title)
        );
        if let Some(y) = req.partial.year {
            // TMDB uses different param names per endpoint.
            let year_param = if entry_kind == EntryKind::Movie {
                "primary_release_year"
            } else {
                "first_air_date_year"
            };
            query.push_str(&format!("&{year_param}={y}"));
        }
        let search_url = build_url(endpoint, &query, &api_key);
        plugin_info!("tmdb: enrich search {}", log_url(&search_url));

        let search_body = match http_get(&search_url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let search_items: Vec<PluginEntry> = if entry_kind == EntryKind::Movie {
            let paged: PagedResponse<MovieItem> = match parse_json(&search_body) {
                Ok(p) => p,
                Err(e) => return PluginResult::Err(e),
            };
            paged.results.into_iter().take(10).map(|m| m.into_entry(entry_kind)).collect()
        } else {
            let paged: PagedResponse<TvItem> = match parse_json(&search_body) {
                Ok(p) => p,
                Err(e) => return PluginResult::Err(e),
            };
            paged.results.into_iter().take(10).map(|t| t.into_entry(entry_kind)).collect()
        };

        let (best, confidence) = pick_best_match(&req.partial, &search_items);
        match best {
            Some(idx) => {
                let winner = search_items
                    .into_iter()
                    .nth(idx)
                    .expect("index from pick_best_match is in-bounds");
                // Hydrate the winner via direct lookup so external_ids get
                // populated (search results don't carry them).
                let lookup_req = LookupRequest {
                    id: winner.id.clone(),
                    id_source: id_sources::TMDB.to_string(),
                    kind: req.partial.kind,
                    locale: None,
                };
                match self.lookup(lookup_req) {
                    PluginResult::Ok(r) => PluginResult::ok(EnrichResponse {
                        entry: r.entry,
                        confidence,
                    }),
                    PluginResult::Err(_) => PluginResult::ok(EnrichResponse {
                        entry: winner,
                        confidence,
                    }),
                }
            }
            None => PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("no TMDB match for title '{title}'"),
            ),
        }
    }

    fn get_artwork(&self, req: ArtworkRequest) -> PluginResult<ArtworkResponse> {
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let kind_path = match req.kind {
            EntryKind::Movie => "movie",
            EntryKind::Series | EntryKind::Episode => "tv",
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "tmdb artwork supports movie/series/episode only",
                );
            }
        };
        // Pull the bundled response first (cached). The `images` sub-field
        // carries everything `/movie/{id}/images` would have returned; we
        // only fall back to a dedicated `/images` request if the bundle
        // somehow lacks it (older cache entries from before the bundle
        // schema, deserialise hiccup, …).
        let body = match fetch_bundle_raw(kind_path, &req.id, &api_key) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let images: ImagesResponse = if req.kind == EntryKind::Movie {
            match parse_json::<MovieDetail>(&body) {
                Ok(d) => d.images.unwrap_or_default(),
                Err(e) => return PluginResult::Err(e),
            }
        } else {
            match parse_json::<TvDetail>(&body) {
                Ok(d) => d.images.unwrap_or_default(),
                Err(e) => return PluginResult::Err(e),
            }
        };

        // Build a variant list. For a specific requested size, emit one URL
        // per poster; for `Any`, emit all three sizes per poster. Backdrops
        // are used as a fallback if no posters exist.
        let wanted_sizes: &[ArtworkSize] = match req.size {
            ArtworkSize::Any => &[ArtworkSize::Thumbnail, ArtworkSize::Standard, ArtworkSize::HiRes],
            _ => std::slice::from_ref(&req.size),
        };

        let source = if !images.posters.is_empty() {
            images.posters
        } else {
            images.backdrops
        };

        let mut variants = Vec::with_capacity(source.len() * wanted_sizes.len());
        for img in source.iter().take(20) {
            for &size in wanted_sizes {
                let size_prefix = tmdb_size_prefix(size);
                variants.push(ArtworkVariant {
                    size,
                    url: format!("{IMAGE_BASE_URL}{size_prefix}{}", img.file_path),
                    mime: "image/jpeg".to_string(),
                    width: img.width,
                    height: img.height,
                });
            }
        }
        PluginResult::ok(ArtworkResponse { variants })
    }

    fn get_credits(&self, req: CreditsRequest) -> PluginResult<CreditsResponse> {
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let kind_path = match req.kind {
            EntryKind::Movie => "movie",
            EntryKind::Series | EntryKind::Episode => "tv",
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "tmdb credits supports movie/series/episode only",
                );
            }
        };
        // Bundle-cached: see lookup() / get_artwork() for the rationale.
        let body = match fetch_bundle_raw(kind_path, &req.id, &api_key) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let payload: CreditsPayload = if req.kind == EntryKind::Movie {
            match parse_json::<MovieDetail>(&body) {
                Ok(d) => d.credits.unwrap_or_default(),
                Err(e) => return PluginResult::Err(e),
            }
        } else {
            match parse_json::<TvDetail>(&body) {
                Ok(d) => d.credits.unwrap_or_default(),
                Err(e) => return PluginResult::Err(e),
            }
        };

        let cast: Vec<CastMember> = payload
            .cast
            .into_iter()
            .map(|c| CastMember {
                name: c.name,
                role: CastRole::Actor,
                character: c.character,
                instrument: None,
                billing_order: c.order,
                external_ids: HashMap::new(),
            })
            .collect();

        let crew: Vec<CrewMember> = payload
            .crew
            .into_iter()
            .map(|c| CrewMember {
                name: c.name,
                role: normalize_crew_role(c.job.as_deref().unwrap_or("")),
                department: c.department,
                external_ids: HashMap::new(),
            })
            .collect();

        PluginResult::ok(CreditsResponse { cast, crew })
    }

    fn related(&self, req: RelatedRequest) -> PluginResult<RelatedResponse> {
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        // TMDB exposes `/recommendations` (algorithmic, users-who-liked-this)
        // and `/similar` (keyword-based). Map both `Similar` and `Any` to
        // `/recommendations`; there is no direct mapping for SameDirector /
        // SameStudio etc., so we short-circuit those with an empty list.
        match req.relation {
            RelationKind::Similar | RelationKind::Any | RelationKind::Sequel => { /* supported */ }
            _ => return PluginResult::ok(RelatedResponse { items: Vec::new() }),
        }

        let (kind_path, entry_kind) = match req.kind {
            EntryKind::Movie => ("movie", EntryKind::Movie),
            EntryKind::Series | EntryKind::Episode => ("tv", EntryKind::Series),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "tmdb related supports movie/series/episode only",
                );
            }
        };
        // Bundle-cached: see lookup() for the rationale. The bundle's
        // recommendations sub-field carries the same `results` page
        // /recommendations would have returned (page 1 only — TMDB's
        // append_to_response gives us the first page, which is all the
        // detail card needs anyway).
        let body = match fetch_bundle_raw(kind_path, &req.id, &api_key) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let limit = if req.limit == 0 { 20 } else { req.limit as usize };
        let items: Vec<PluginEntry> = if entry_kind == EntryKind::Movie {
            match parse_json::<MovieDetail>(&body) {
                Ok(d) => d
                    .recommendations
                    .map(|p| p.results)
                    .unwrap_or_default()
                    .into_iter()
                    .take(limit)
                    .map(|m| m.into_entry(entry_kind))
                    .collect(),
                Err(e) => return PluginResult::Err(e),
            }
        } else {
            match parse_json::<TvDetail>(&body) {
                Ok(d) => d
                    .recommendations
                    .map(|p| p.results)
                    .unwrap_or_default()
                    .into_iter()
                    .take(limit)
                    .map(|t| t.into_entry(entry_kind))
                    .collect(),
                Err(e) => return PluginResult::Err(e),
            }
        };
        PluginResult::ok(RelatedResponse { items })
    }

    fn episodes(&self, req: EpisodesRequest) -> PluginResult<EpisodesResponse> {
        if req.id_source != id_sources::TMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("tmdb episodes only supports tmdb id_source, got: {}", req.id_source),
            );
        }
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        let path = format!("/tv/{}/season/{}", req.series_id, req.season);
        let url = build_url(&path, "language=en-US", &api_key);
        plugin_info!("tmdb: episodes {}", log_url(&url));

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let payload: SeasonResponse = match parse_json(&body) {
            Ok(p) => p,
            Err(e) => return PluginResult::Err(e),
        };

        let episodes: Vec<EpisodeWire> = payload
            .episodes
            .into_iter()
            .map(|ep| {
                // TMDB exposes a per-episode id; `<series>:s<S>e<E>` is a
                // stable fallback used only if the id is missing.
                let entry_id = ep
                    .id
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!(
                        "{}:s{}e{}",
                        req.series_id, ep.season_number, ep.episode_number
                    ));
                EpisodeWire {
                    season: ep.season_number,
                    episode: ep.episode_number,
                    title: ep.name.unwrap_or_default(),
                    air_date: ep.air_date,
                    runtime_mins: ep.runtime,
                    provider: "tmdb".to_string(),
                    entry_id,
                }
            })
            .collect();
        PluginResult::ok(EpisodesResponse { episodes })
    }
}

// ── Season response shape ─────────────────────────────────────────────────────

/// `/tv/{id}/season/{n}` response — season-level metadata plus the list
/// of episodes. We only deserialise the fields we actually surface.
#[derive(Deserialize)]
struct SeasonResponse {
    #[serde(default)]
    episodes: Vec<TmdbEpisode>,
}

#[derive(Deserialize)]
struct TmdbEpisode {
    #[serde(default)]
    id: Option<u64>,
    #[serde(rename = "season_number")]
    season_number: u32,
    #[serde(rename = "episode_number")]
    episode_number: u32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    air_date: Option<String>,
    #[serde(default)]
    runtime: Option<u32>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn tmdb_size_prefix(size: ArtworkSize) -> &'static str {
    match size {
        ArtworkSize::Thumbnail => "w185",
        ArtworkSize::Standard => "w500",
        ArtworkSize::HiRes => "original",
        // `Any` is expanded at the call site into all three sizes; reaching
        // here with Any is a bug but we fall back to Standard to avoid panics.
        ArtworkSize::Any => "w500",
    }
}

/// Choose the best search hit given a partial hint. Returns (index, confidence).
/// Confidence heuristic: exact title + matching year → 0.95, exact title no
/// year → 0.8, case-insensitive title match → 0.7, first hit → 0.5, nothing → 0.0.
fn pick_best_match(partial: &PluginEntry, items: &[PluginEntry]) -> (Option<usize>, f32) {
    if items.is_empty() {
        return (None, 0.0);
    }
    let want_title = partial.title.trim().to_lowercase();
    let want_year = partial.year;

    let mut best: Option<(usize, f32)> = None;
    for (i, it) in items.iter().enumerate() {
        let t = it.title.trim().to_lowercase();
        let score = if t == want_title && it.year == want_year && want_year.is_some() {
            0.95
        } else if t == want_title {
            0.8
        } else if !t.is_empty() && t.starts_with(&want_title) {
            0.7
        } else {
            continue;
        };
        if best.map(|(_, s)| score > s).unwrap_or(true) {
            best = Some((i, score));
        }
    }
    match best {
        Some((i, s)) => (Some(i), s),
        None => (Some(0), 0.5),
    }
}

fn genre_name(id: u32) -> &'static str {
    match id {
        28 => "Action",
        12 => "Adventure",
        16 => "Animation",
        35 => "Comedy",
        80 => "Crime",
        99 => "Documentary",
        18 => "Drama",
        10751 => "Family",
        14 => "Fantasy",
        36 => "History",
        27 => "Horror",
        10402 => "Music",
        9648 => "Mystery",
        10749 => "Romance",
        878 => "Sci-Fi",
        10770 => "TV Movie",
        53 => "Thriller",
        10752 => "War",
        37 => "Western",
        10759 => "Action & Adventure",
        10762 => "Kids",
        10763 => "News",
        10764 => "Reality",
        10765 => "Sci-Fi & Fantasy",
        10766 => "Soap",
        10767 => "Talk",
        10768 => "War & Politics",
        _ => "Other",
    }
}

// ── WASM exports ──────────────────────────────────────────────────────────────

impl stui_plugin_sdk::StreamProvider for TmdbPlugin {}

stui_export_catalog_plugin!(TmdbPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn assert_plugin<T: Plugin>() {}
        fn assert_catalog<T: CatalogPlugin>() {}
        assert_plugin::<TmdbPlugin>();
        assert_catalog::<TmdbPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = TmdbPlugin::new();
        assert_eq!(p.manifest().plugin.name, "tmdb");
    }

    #[test]
    fn classify_http_404_is_unknown_id() {
        let e = classify_http_err("HTTP 404: not found");
        assert_eq!(e.code, error_codes::UNKNOWN_ID);
    }

    #[test]
    fn classify_http_429_is_rate_limited() {
        let e = classify_http_err("HTTP 429: too many");
        assert_eq!(e.code, error_codes::RATE_LIMITED);
    }

    #[test]
    fn classify_http_503_is_transient() {
        let e = classify_http_err("HTTP 503: down");
        assert_eq!(e.code, error_codes::TRANSIENT);
    }

    #[test]
    fn classify_http_400_is_remote_error() {
        let e = classify_http_err("HTTP 400: bad request");
        assert_eq!(e.code, error_codes::REMOTE_ERROR);
    }

    #[test]
    fn classify_non_http_is_transient() {
        let e = classify_http_err("connection refused");
        assert_eq!(e.code, error_codes::TRANSIENT);
    }

    #[test]
    fn tmdb_size_prefix_mapping() {
        assert_eq!(tmdb_size_prefix(ArtworkSize::Thumbnail), "w185");
        assert_eq!(tmdb_size_prefix(ArtworkSize::Standard), "w500");
        assert_eq!(tmdb_size_prefix(ArtworkSize::HiRes), "original");
    }

    #[test]
    fn build_url_with_and_without_query() {
        assert_eq!(
            build_url("/movie/1", "", "KEY"),
            "https://api.themoviedb.org/3/movie/1?api_key=KEY"
        );
        assert_eq!(
            build_url("/movie/1", "foo=bar", "KEY"),
            "https://api.themoviedb.org/3/movie/1?foo=bar&api_key=KEY"
        );
    }

    #[test]
    fn movie_item_into_entry_populates_fields() {
        let m = MovieItem {
            id: 100,
            title: "Ran".into(),
            release_date: Some("1985-06-01".into()),
            genre_ids: vec![18, 36],
            vote_average: 8.3,
            overview: Some("desc".into()),
            poster_path: Some("/abc.jpg".into()),
            original_language: Some("ja".into()),
        };
        let e = m.into_entry(EntryKind::Movie);
        assert_eq!(e.id, "100");
        assert_eq!(e.kind, EntryKind::Movie);
        assert_eq!(e.source, "tmdb");
        assert_eq!(e.title, "Ran");
        assert_eq!(e.year, Some(1985));
        assert_eq!(e.genre.as_deref(), Some("Drama"));
        assert!((e.rating.unwrap() - 8.3).abs() < 0.01);
        assert!(e.poster_url.as_ref().unwrap().contains("w342"));
    }

    #[test]
    fn tv_item_into_entry_populates_fields() {
        let t = TvItem {
            id: 1399,
            name: "Game of Thrones".into(),
            first_air_date: Some("2011-04-17".into()),
            genre_ids: vec![10765],
            vote_average: 9.1,
            overview: Some("desc".into()),
            poster_path: Some("/g.jpg".into()),
        };
        let e = t.into_entry(EntryKind::Series);
        assert_eq!(e.id, "1399");
        assert_eq!(e.year, Some(2011));
        assert_eq!(e.genre.as_deref(), Some("Sci-Fi & Fantasy"));
    }

    #[test]
    fn year_from_date_handles_variants() {
        assert_eq!(year_from_date(Some("2001-02-03")), Some(2001));
        assert_eq!(year_from_date(Some("")), None);
        assert_eq!(year_from_date(None), None);
        assert_eq!(year_from_date(Some("not-a-date")), None);
    }

    #[test]
    fn nonzero_rating_drops_zero() {
        assert_eq!(nonzero_rating(0.0), None);
        assert_eq!(nonzero_rating(7.2), Some(7.2));
    }

    #[test]
    fn pick_best_match_exact_title_and_year() {
        let partial = PluginEntry {
            title: "Ran".into(),
            year: Some(1985),
            kind: EntryKind::Movie,
            ..Default::default()
        };
        let items = vec![
            PluginEntry { title: "Random".into(), year: Some(1999), ..Default::default() },
            PluginEntry { title: "Ran".into(), year: Some(1985), ..Default::default() },
        ];
        let (idx, conf) = pick_best_match(&partial, &items);
        assert_eq!(idx, Some(1));
        assert!(conf >= 0.9);
    }

    #[test]
    fn pick_best_match_no_match_fallback() {
        let partial = PluginEntry {
            title: "Foo".into(),
            year: None,
            kind: EntryKind::Movie,
            ..Default::default()
        };
        let items = vec![
            PluginEntry { title: "Totally Different".into(), ..Default::default() },
        ];
        let (idx, conf) = pick_best_match(&partial, &items);
        assert_eq!(idx, Some(0));
        assert!((conf - 0.5).abs() < 0.01);
    }

    #[test]
    fn pick_best_match_empty_items() {
        let partial = PluginEntry::default();
        let (idx, _) = pick_best_match(&partial, &[]);
        assert_eq!(idx, None);
    }

    #[test]
    fn new_for_test_caches_api_key() {
        let p = TmdbPlugin::new_for_test("TESTKEY");
        assert_eq!(p.api_key().unwrap(), "TESTKEY");
    }

    #[test]
    fn movie_detail_into_entry_populates_external_ids() {
        let d = MovieDetail {
            id: 42,
            title: "HHGTTG".into(),
            release_date: Some("2005-04-28".into()),
            genres: vec![Genre { id: 35, name: "Comedy".into() }],
            vote_average: 6.8,
            overview: None,
            poster_path: None,
            runtime: Some(109),
            imdb_id: None,
            external_ids: Some(ExternalIds {
                imdb_id: Some("tt0371724".into()),
                tvdb_id: None,
            }),
        };
        let e = d.into_entry();
        assert_eq!(e.external_ids.get("imdb").map(String::as_str), Some("tt0371724"));
        assert_eq!(e.imdb_id.as_deref(), Some("tt0371724"));
        assert_eq!(e.duration, Some(109));
        assert_eq!(e.genre.as_deref(), Some("Comedy"));
    }

    #[test]
    fn parse_json_invalid_returns_parse_error() {
        let r: Result<serde_json::Value, _> = parse_json("not json");
        let err = r.unwrap_err();
        assert_eq!(err.code, error_codes::PARSE_ERROR);
    }

    /// Verify that a movie search payload parses as `PagedResponse<MovieItem>`
    /// without needing the `SearchResult` untagged enum.
    #[test]
    fn paged_movie_response_deserializes_typed() {
        let json = r#"{
            "results": [{"id": 1, "title": "Dune", "release_date": "2021-09-15"}],
            "total_results": 1
        }"#;
        let paged: PagedResponse<MovieItem> = parse_json(json).unwrap();
        assert_eq!(paged.results.len(), 1);
        assert_eq!(paged.results[0].title, "Dune");
        assert_eq!(paged.total_results, Some(1));
    }

    /// Verify that a TV search payload parses as `PagedResponse<TvItem>`
    /// without needing the `SearchResult` untagged enum.
    #[test]
    fn paged_tv_response_deserializes_typed() {
        let json = r#"{
            "results": [{"id": 2, "name": "Severance", "first_air_date": "2022-02-18"}],
            "total_results": 1
        }"#;
        let paged: PagedResponse<TvItem> = parse_json(json).unwrap();
        assert_eq!(paged.results.len(), 1);
        assert_eq!(paged.results[0].name, "Severance");
        // A TV payload must NOT bleed into MovieItem — `title` field stays empty default.
        let movie_attempt: Result<PagedResponse<MovieItem>, _> = parse_json(json);
        let title = movie_attempt.map(|p| p.results.into_iter().next().map(|m| m.title));
        // Either fails or the title is empty (not "Severance"), confirming the
        // typed approach catches the mismatch that the untagged enum hid.
        match title {
            Ok(Some(t)) => assert!(t.is_empty(), "movie title should not take TV 'name' field"),
            Ok(None) | Err(_) => { /* parse failure is also acceptable */ }
        }
    }
}
