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
    cache_get, cache_set, error_codes, http_get,
    id_sources, normalize_crew_role,
    plugin_error, plugin_info,
    stui_export_catalog_plugin,
    AlternativeTitle, AlternativeTitlesRequest, AlternativeTitlesResponse,
    ArtworkRequest, ArtworkResponse, ArtworkSize, ArtworkVariant,
    BoxOfficeRequest, BoxOfficeResponse, MoneyAmount,
    CastMember, CastRole, CatalogPlugin, CreditsRequest, CreditsResponse, CrewMember,
    EnrichRequest, EnrichResponse,
    EntryKind,
    EpisodeWire, EpisodesRequest, EpisodesResponse,
    InitContext,
    Keyword, KeywordsRequest, KeywordsResponse,
    LookupRequest, LookupResponse,
    Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    RelatedRequest, RelatedResponse, RelationKind,
    ReleaseEntry, ReleaseInfoRequest, ReleaseInfoResponse,
    SearchRequest, SearchResponse, SearchScope,
    Trailer, TrailerKind, TrailersRequest, TrailersResponse,
};

const BASE_URL: &str = "https://xmdbapi.com/api/v1";
const CACHE_TTL_SECS: i64 = 24 * 3600;

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

// ── Cache helper ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedPayload {
    body: String,
    expires_at: i64,
}

impl CachedPayload {
    fn wrap(body: &str, ttl_secs: i64) -> Self {
        Self {
            body: body.to_string(),
            expires_at: stui_plugin_sdk::now_unix() + ttl_secs,
        }
    }
    fn expired(&self) -> bool {
        stui_plugin_sdk::now_unix() >= self.expires_at
    }
}

impl XmdbPlugin {
    /// Fetch the `/movies/{imdb_id}` payload, hitting cache when fresh.
    /// `force_refresh: true` bypasses the cache, refetches, and writes
    /// the fresh payload back. 24h TTL, key `xmdb:movies:{imdb_id}`.
    fn fetch_or_cache_movie_payload(
        &self,
        imdb_id: &str,
        force_refresh: bool,
    ) -> Result<MovieDetail, PluginError> {
        let api_key = self.api_key()?.to_string();
        let cache_key = format!("xmdb:movies:{imdb_id}");

        if !force_refresh {
            if let Some(blob) = cache_get(&cache_key) {
                if let Ok(cached) = serde_json::from_str::<CachedPayload>(&blob) {
                    if !cached.expired() {
                        return parse_json::<MovieDetail>(&cached.body);
                    }
                }
            }
        }

        let url = format!(
            "{BASE_URL}/movies/{}?apiKey={}",
            urlencoding::encode(imdb_id),
            api_key,
        );
        plugin_info!("xmdb: fetch {} (force_refresh={})", imdb_id, force_refresh);

        let body = http_get(&url).map_err(|e| classify_http_err(&e))?;
        let serialized = serde_json::to_string(&CachedPayload::wrap(&body, CACHE_TTL_SECS))
            .unwrap_or_default();
        cache_set(&cache_key, &serialized);
        parse_json::<MovieDetail>(&body)
    }
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
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
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
            force_refresh: req.force_refresh,
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
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        let (cast, crew) = split_credits(&detail.full_cast_and_crew);
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

    fn get_artwork(&self, req: ArtworkRequest) -> PluginResult<ArtworkResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb artwork only supports imdb ids, got: {}", req.id_source),
            );
        }
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        // TODO(v2): hit /poster/{id} or /posters for multi-resolution
        // variants once the wire shape is verified. First cut serves the
        // single poster_url already in the cached /movies/{id} payload.
        let variants: Vec<ArtworkVariant> = detail.poster_url
            .into_iter()
            .map(|url| ArtworkVariant {
                size: ArtworkSize::Standard,
                url,
                mime: "image/jpeg".to_string(),
                width: None,
                height: None,
            })
            .collect();
        PluginResult::ok(ArtworkResponse { variants })
    }

    fn related(&self, req: RelatedRequest) -> PluginResult<RelatedResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb related only supports imdb ids, got: {}", req.id_source),
            );
        }
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        let limit = if req.limit == 0 { usize::MAX } else { req.limit as usize };
        let items: Vec<PluginEntry> = detail.similar_titles
            .into_iter()
            .take(limit)
            .map(|s| s.into_entry(req.kind))
            .collect();
        PluginResult::ok(RelatedResponse { items })
    }

    fn get_trailers(&self, req: TrailersRequest) -> PluginResult<TrailersResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb trailers only supports imdb ids, got: {}", req.id_source),
            );
        }
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        let trailers = detail.trailer
            .into_iter()
            .filter(|t| !t.url.is_empty())
            .map(|t| Trailer {
                url:           t.url,
                thumbnail_url: t.thumbnail,
                title:         t.name,
                kind:          TrailerKind::Trailer,
                language:      None,
                duration_secs: None,
            })
            .collect();
        PluginResult::ok(TrailersResponse { trailers })
    }

    fn get_release_info(&self, req: ReleaseInfoRequest) -> PluginResult<ReleaseInfoResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb release_info only supports imdb ids, got: {}", req.id_source),
            );
        }
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        let releases = detail.release_dates
            .into_iter()
            .map(|r| ReleaseEntry {
                country:      r.country_code.or(r.country).unwrap_or_default(),
                date:         r.date,
                release_kind: None,   // xmdb's release_type isn't a meaningful enum
                certificate:  None,   // RawReleaseDate has no certificate field
                note:         None,   // RawReleaseDate has no attributes field
            })
            .filter(|r| !r.country.is_empty())
            .collect();
        PluginResult::ok(ReleaseInfoResponse { releases })
    }

    fn get_keywords(&self, req: KeywordsRequest) -> PluginResult<KeywordsResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb keywords only supports imdb ids, got: {}", req.id_source),
            );
        }
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        let keywords = detail.keywords
            .into_iter()
            .filter(|k| !k.trim().is_empty())
            .map(|name| Keyword { name, source_id: None, provider: None })
            .collect();
        PluginResult::ok(KeywordsResponse { keywords })
    }

    fn get_box_office(&self, req: BoxOfficeRequest) -> PluginResult<BoxOfficeResponse> {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb box_office only supports imdb ids, got: {}", req.id_source),
            );
        }
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        fn project(raw: Option<RawMoney>) -> Option<MoneyAmount> {
            raw.and_then(|m| match (m.amount, m.currency) {
                (Some(a), Some(c)) if a > 0 && !c.is_empty() => Some(MoneyAmount {
                    amount: a, currency: c,
                }),
                _ => None,
            })
        }
        PluginResult::ok(BoxOfficeResponse {
            budget:          project(detail.budget),
            opening_weekend: project(detail.opening_weekend_gross),
            gross_domestic:  project(detail.lifetime_gross),
            gross_worldwide: project(detail.worldwide_gross),
        })
    }

    fn get_alternative_titles(&self, req: AlternativeTitlesRequest)
        -> PluginResult<AlternativeTitlesResponse>
    {
        if req.id_source != id_sources::IMDB {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("xmdb alt_titles only supports imdb ids, got: {}", req.id_source),
            );
        }
        let detail = match self.fetch_or_cache_movie_payload(&req.id, req.force_refresh) {
            Ok(d) => d,
            Err(e) => return PluginResult::Err(e),
        };
        let titles = detail.alternative_titles
            .into_iter()
            .filter(|a| !a.title.trim().is_empty())
            .map(|a| AlternativeTitle {
                title:   a.title,
                locale:  a.language_code.or(a.language),
                country: a.country_code.or(a.country),
                kind:    None,   // xmdb has no per-row classification
            })
            .collect();
        PluginResult::ok(AlternativeTitlesResponse { titles })
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

/// Walk a `full_cast_and_crew` map and split into typed cast / crew
/// vectors. Cast rows pick up `characters[0]` as the character name
/// (xmdb's `characters` is an array but the first entry is the
/// canonical role; multi-character actors are rare). Crew rows take
/// the role key verbatim and feed it through `normalize_crew_role`.
/// Billing order is assigned in iteration order — for cast that means
/// XMDb's underlying ranking inside the `Actor` array.
fn split_credits(
    fcc: &std::collections::HashMap<String, Vec<CreditPerson>>,
) -> (Vec<CastMember>, Vec<CrewMember>) {
    let mut cast: Vec<CastMember> = Vec::new();
    let mut crew: Vec<CrewMember> = Vec::new();
    for (role, people) in fcc {
        if is_cast_role(role) {
            for p in people {
                let billing_order = Some(cast.len() as u32 + 1);
                let mut external_ids = std::collections::HashMap::new();
                if let Some(id) = p.id.as_deref().filter(|s| !s.is_empty()) {
                    external_ids.insert(id_sources::IMDB.to_string(), id.to_string());
                }
                cast.push(CastMember {
                    name: p.name.clone(),
                    role: CastRole::Actor,
                    character: p.characters.first().cloned(),
                    instrument: None,
                    billing_order,
                    external_ids,
                });
            }
        } else {
            for p in people {
                let mut external_ids = std::collections::HashMap::new();
                if let Some(id) = p.id.as_deref().filter(|s| !s.is_empty()) {
                    external_ids.insert(id_sources::IMDB.to_string(), id.to_string());
                }
                crew.push(CrewMember {
                    name: p.name.clone(),
                    role: normalize_crew_role(role),
                    department: Some(role.clone()),
                    external_ids,
                });
            }
        }
    }
    (cast, crew)
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

/// `/movies/{id}` response. Serde silently ignores extra JSON fields
/// (no `deny_unknown_fields`), so we only declare what we project.
/// All fields are `#[serde(default)]` so partial responses and optional
/// fields never break parsing.
#[derive(Debug, Deserialize)]
struct MovieDetail {
    #[serde(default)] id:                   String,
    #[serde(default)] title:                String,
    #[serde(default)] original_title:       Option<String>,
    #[serde(default)] release_year:         Option<u32>,
    #[serde(default)] runtime_minutes:      Option<u32>,
    /// IMDb-shaped 0–10. Maps to entry.ratings["imdb"] and entry.rating.
    #[serde(default)] rating:               Option<f32>,
    #[serde(default)] vote_count:           Option<u32>,
    /// Metacritic 0–100. Maps to entry.ratings["metacritic"].
    #[serde(default)] metascore:            Option<f32>,
    #[serde(default)] certificate:          Option<String>,
    #[serde(default)] genres:               Vec<String>,
    #[serde(default)] languages:            Vec<String>,
    #[serde(default)] countries:            Vec<String>,
    #[serde(default)] plot:                 Option<String>,
    #[serde(default)] poster_url:           Option<String>,
    #[serde(default)] imdb_url:             Option<String>,
    #[serde(default)] title_type:           Option<String>,
    /// Role-keyed map: `{"Actor": [{...}, ...], "Director": [{...}, ...], ...}`.
    /// Each role's array carries `CreditPerson` rows. Splitting into
    /// cast vs crew is done in `into_entry` based on the role key —
    /// `Actor` / `Actress` go to cast, everything else to crew.
    #[serde(default)] full_cast_and_crew:   std::collections::HashMap<String, Vec<CreditPerson>>,
    // ── New raw shapes for verbs in Chunk 7 ──
    #[serde(default)] keywords:             Vec<String>,
    #[serde(default)] alternative_titles:   Vec<RawAlternativeTitle>,
    #[serde(default)] release_dates:        Vec<RawReleaseDate>,
    #[serde(default)] similar_titles:       Vec<RawSimilarTitle>,
    #[serde(default)] trailer:              Option<RawTrailer>,
    #[serde(default)] budget:               Option<RawMoney>,
    #[serde(default)] opening_weekend_gross: Option<RawMoney>,
    #[serde(default)] lifetime_gross:       Option<RawMoney>,
    #[serde(default)] worldwide_gross:      Option<RawMoney>,
}

// ── Raw wire shapes for Chunk 7 verbs ────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RawAlternativeTitle {
    #[serde(default)] title:         String,
    #[serde(default)] country:       Option<String>,
    #[serde(default)] country_code:  Option<String>,
    #[serde(default)] language:      Option<String>,
    #[serde(default)] language_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawReleaseDate {
    #[serde(default)] country:       Option<String>,
    #[serde(default)] country_code:  Option<String>,
    #[serde(default)] date:          Option<String>,
    #[serde(default)] day:           Option<u32>,
    #[serde(default)] month:         Option<u32>,
    #[serde(default)] year:          Option<u32>,
    #[serde(default)] release_type:  Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSimilarTitle {
    #[serde(default)] id:          Option<String>,
    #[serde(default)] title:       Option<String>,
    #[serde(default)] year:        Option<u32>,
    #[serde(default)] rating:      Option<f32>,
    #[serde(default)] poster_url:  Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTrailer {
    #[serde(default)] url:           String,
    #[serde(default)] thumbnail:     Option<String>,
    #[serde(default)] name:          Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMoney {
    #[serde(default)] amount:   Option<u64>,
    #[serde(default)] currency: Option<String>,
}

impl RawSimilarTitle {
    fn into_entry(self, fallback_kind: EntryKind) -> PluginEntry {
        // xmdb's similar_titles use IMDb-shaped ids (e.g., "tt0468569")
        let imdb = self.id.as_deref().and_then(|s| {
            if s.starts_with("tt") && s.len() > 2 && s[2..].chars().all(|c| c.is_ascii_digit()) {
                Some(s.to_string())
            } else { None }
        });
        let id = self.id.clone().or_else(|| imdb.clone()).unwrap_or_default();
        let mut entry = PluginEntry {
            id,
            kind:       fallback_kind,
            source:     "xmdb".to_string(),
            title:      self.title.unwrap_or_default(),
            year:       self.year,
            rating:     self.rating,
            poster_url: self.poster_url,
            imdb_id:    imdb.clone(),
            ..Default::default()
        };
        if let Some(ref id) = imdb {
            entry.external_ids.insert(id_sources::IMDB.to_string(), id.clone());
        }
        entry
    }
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
            id:                  id_for_entry,
            kind,
            source:              "xmdb".to_string(),
            title:               self.title,
            year:                self.release_year,
            poster_url:          self.poster_url,
            imdb_id:             imdb.clone(),
            description:         self.plot,
            genre:               Some(self.genres.join(", ")).filter(|s| !s.is_empty()),
            rating:              self.rating,
            duration:            self.runtime_minutes,
            ratings,
            original_title:      self.original_title,
            certificate:         self.certificate,
            certificate_country: Some("US".to_string()),
            languages:           self.languages,
            countries:           self.countries,
            ..Default::default()
        };
        if let Some(votes) = self.vote_count {
            entry.rating_votes.insert("imdb".to_string(), votes);
        }
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

/// One person inside a `full_cast_and_crew[role]` array. Cast rows
/// carry `characters` (an array — usually one name, sometimes multiple
/// e.g. `["Frodo Baggins", "Bilbo's nephew"]`); crew rows omit it.
/// `id` is the IMDb name id (`nm0000209` etc.); the wire also carries
/// `profile_image` but PluginEntry's CastMember has no avatar field
/// today, so it's ignored at deserialize time.
#[derive(Debug, Deserialize)]
struct CreditPerson {
    #[serde(default)] name:       String,
    #[serde(default)] id:         Option<String>,
    #[serde(default)] characters: Vec<String>,
}

/// Cast roles are the role keys we project as `CastMember`. Anything
/// else under `full_cast_and_crew` becomes a `CrewMember`. Comparison
/// is case-insensitive — XMDb has been observed to use both `"Actor"`
/// and `"Actress"`; future role keys land in crew until added here.
fn is_cast_role(role: &str) -> bool {
    let r = role.to_ascii_lowercase();
    r == "actor" || r == "actress" || r == "cast"
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
            id:                     "xmdb-123".into(),
            title:                  "The Shawshank Redemption".into(),
            original_title:         None,
            release_year:           Some(1994),
            runtime_minutes:        Some(142),
            rating:                 Some(9.3),
            vote_count:             None,
            metascore:              Some(82.0),
            certificate:            None,
            genres:                 vec!["Drama".into()],
            languages:              vec![],
            countries:              vec![],
            plot:                   Some("...".into()),
            poster_url:             Some("https://example.com/p.jpg".into()),
            imdb_url:               Some("https://www.imdb.com/title/tt0111161/".into()),
            title_type:             Some("movie".into()),
            full_cast_and_crew:     std::collections::HashMap::new(),
            keywords:               vec![],
            alternative_titles:     vec![],
            release_dates:          vec![],
            similar_titles:         vec![],
            trailer:                None,
            budget:                 None,
            opening_weekend_gross:  None,
            lifetime_gross:         None,
            worldwide_gross:        None,
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
            id:                     "xmdb-123".into(),
            title:                  "Untracked".into(),
            original_title:         None,
            release_year:           None,
            runtime_minutes:        None,
            rating:                 Some(7.0),
            vote_count:             None,
            metascore:              None,
            certificate:            None,
            genres:                 vec![],
            languages:              vec![],
            countries:              vec![],
            plot:                   None,
            poster_url:             None,
            imdb_url:               None,
            title_type:             None,
            full_cast_and_crew:     std::collections::HashMap::new(),
            keywords:               vec![],
            alternative_titles:     vec![],
            release_dates:          vec![],
            similar_titles:         vec![],
            trailer:                None,
            budget:                 None,
            opening_weekend_gross:  None,
            lifetime_gross:         None,
            worldwide_gross:        None,
        };
        let e = detail.into_entry(EntryKind::Movie);
        assert_eq!(e.ratings.get("imdb").copied(), Some(7.0));
        assert!(e.ratings.get("metacritic").is_none());
    }

    #[test]
    fn is_cast_role_matches_actor_actress_cast() {
        assert!(is_cast_role("Actor"));
        assert!(is_cast_role("actress"));
        assert!(is_cast_role("CAST"));
        assert!(!is_cast_role("Director"));
        assert!(!is_cast_role("Writers"));
        assert!(!is_cast_role(""));
    }

    #[test]
    fn split_credits_routes_actors_to_cast_and_others_to_crew() {
        let mut fcc = std::collections::HashMap::new();
        fcc.insert(
            "Actor".to_string(),
            vec![
                CreditPerson {
                    name: "Tim Robbins".into(),
                    id: Some("nm0000209".into()),
                    characters: vec!["Andy Dufresne".into()],
                },
                CreditPerson {
                    name: "Morgan Freeman".into(),
                    id: Some("nm0000151".into()),
                    characters: vec!["Ellis Boyd 'Red' Redding".into()],
                },
            ],
        );
        fcc.insert(
            "Director".to_string(),
            vec![CreditPerson {
                name: "Frank Darabont".into(),
                id: Some("nm0001104".into()),
                characters: vec![],
            }],
        );

        let (cast, crew) = split_credits(&fcc);
        assert_eq!(cast.len(), 2);
        assert_eq!(crew.len(), 1);

        // Cast picks up first character + IMDb name id + sequential billing.
        let andy = cast.iter().find(|c| c.name == "Tim Robbins").unwrap();
        assert_eq!(andy.character.as_deref(), Some("Andy Dufresne"));
        assert_eq!(
            andy.external_ids.get(id_sources::IMDB).map(String::as_str),
            Some("nm0000209"),
        );
        assert!(andy.billing_order.is_some());

        // Crew row keeps the role key as the department string.
        assert_eq!(crew[0].name, "Frank Darabont");
        assert_eq!(crew[0].department.as_deref(), Some("Director"));
    }

    #[test]
    fn split_credits_handles_actor_without_characters() {
        // Some entries have an empty `characters` array (uncredited /
        // documentary). Cast row still gets created; character = None.
        let mut fcc = std::collections::HashMap::new();
        fcc.insert(
            "Actor".to_string(),
            vec![CreditPerson {
                name: "Background".into(),
                id: None,
                characters: vec![],
            }],
        );
        let (cast, crew) = split_credits(&fcc);
        assert_eq!(cast.len(), 1);
        assert!(cast[0].character.is_none());
        assert!(crew.is_empty());
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
            force_refresh: false,
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
            force_refresh: false,
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
            "release_date": {
                "date": "1994-09-23",
                "day": 23, "month": 9, "year": 1994,
                "country": "United States", "country_code": "US"
            },
            "runtime_minutes": 142,
            "rating": 9.3,
            "vote_count": 2700000,
            "metascore": 82,
            "genres": ["Drama"],
            "plot": "Two imprisoned men...",
            "poster_url": "https://p/1.jpg",
            "imdb_url": "https://www.imdb.com/title/tt0111161/",
            "title_type": "movie",
            "full_cast_and_crew": {}
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
            force_refresh: false,
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

    // ── Task 6.1: cache helper tests ──────────────────────────────────────────

    #[test]
    fn fetch_or_cache_returns_cached_when_fresh() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let now = stui_plugin_sdk::now_unix();
        let body = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,"runtime_minutes":100,
            "rating":7.0,"genres":[],"poster_url":null,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{}
        }"#;
        let cached = format!(
            r#"{{"body":{},"expires_at":{}}}"#,
            serde_json::to_string(body).unwrap(),
            now + 3600,
        );
        let _h = MockHost::new().with_cache_value("xmdb:movies:tt0000001", &cached);
        let plugin = XmdbPlugin::new_for_test("fake");
        let detail = plugin
            .fetch_or_cache_movie_payload("tt0000001", false)
            .expect("should hit cache");
        assert_eq!(detail.title, "X");
        assert_eq!(MockHost::http_call_count(), 0);
    }

    #[test]
    fn fetch_or_cache_refetches_when_expired() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let now = stui_plugin_sdk::now_unix();
        let body = r#"{
            "id":"xmdb-1","title":"Expired","release_year":2000,"runtime_minutes":90,
            "rating":6.0,"genres":[],"poster_url":null,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{}
        }"#;
        // expires_at in the PAST
        let cached = format!(
            r#"{{"body":{},"expires_at":{}}}"#,
            serde_json::to_string(body).unwrap(),
            now - 100,
        );
        let _h = MockHost::new()
            .with_cache_value("xmdb:movies:tt0000001", &cached)
            .with_fixture_response(
                "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
                body,
            );
        let plugin = XmdbPlugin::new_for_test("fake");
        let _ = plugin
            .fetch_or_cache_movie_payload("tt0000001", false)
            .unwrap();
        assert_eq!(MockHost::http_call_count(), 1);
        // Verify cache was rewritten with fresh expires_at
        let new_blob = stui_plugin_sdk::cache_get("xmdb:movies:tt0000001").unwrap();
        let new_cached: CachedPayload = serde_json::from_str(&new_blob).unwrap();
        assert!(new_cached.expires_at > now);
    }

    #[test]
    fn fetch_or_cache_bypasses_when_force_refresh() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let now = stui_plugin_sdk::now_unix();
        let body = r#"{
            "id":"xmdb-1","title":"Fresh","release_year":2000,"runtime_minutes":90,
            "rating":8.0,"genres":[],"poster_url":null,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{}
        }"#;
        // FRESH cache
        let cached = format!(
            r#"{{"body":{},"expires_at":{}}}"#,
            serde_json::to_string(body).unwrap(),
            now + 3600,
        );
        let _h = MockHost::new()
            .with_cache_value("xmdb:movies:tt0000001", &cached)
            .with_fixture_response(
                "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
                body,
            );
        let plugin = XmdbPlugin::new_for_test("fake");
        let _ = plugin
            .fetch_or_cache_movie_payload("tt0000001", true)
            .unwrap();
        // Even with fresh cache, force_refresh=true triggers an HTTP call.
        assert_eq!(MockHost::http_call_count(), 1);
    }

    // ── Task 6.2: fixture smoke tests ─────────────────────────────────────────

    #[test]
    fn movie_detail_parses_real_shawshank_fixture() {
        let body = std::fs::read_to_string(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/movies_tt0111161.json"
            ),
        )
        .expect("fixture missing — run Chunk 0 first");
        let detail: MovieDetail =
            serde_json::from_str(&body).expect("real fixture should deserialize cleanly");

        // Group X coverage
        assert!(detail.original_title.is_some());
        assert!(detail.certificate.is_some());
        assert!(detail.vote_count.is_some());
        assert!(!detail.languages.is_empty());
        assert!(!detail.countries.is_empty());

        // New verbs coverage
        assert!(!detail.keywords.is_empty(), "keywords expected");
        assert!(!detail.alternative_titles.is_empty(), "alt titles expected");
        assert!(!detail.release_dates.is_empty(), "release dates expected");
        assert!(detail.trailer.is_some(), "trailer expected");
        assert!(detail.budget.is_some(), "budget expected");
        assert!(detail.worldwide_gross.is_some(), "worldwide_gross expected");
        assert!(!detail.similar_titles.is_empty(), "similar titles expected");
    }

    #[test]
    fn movie_detail_parses_real_breaking_bad_fixture() {
        let body = std::fs::read_to_string(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/movies_tt0903747.json"
            ),
        )
        .expect("fixture missing");
        let detail: MovieDetail =
            serde_json::from_str(&body).expect("series fixture should deserialize cleanly");
        // title_type for series is "TV Series"
        assert_eq!(detail.title_type.as_deref(), Some("TV Series"));
    }

    // ── Task 7: New verb tests ────────────────────────────────────────────────

    #[test]
    fn get_artwork_projects_poster_url_to_single_variant() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,"runtime_minutes":100,
            "rating":7.0,"genres":[],"poster_url":"https://example.com/p.jpg",
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{}
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_artwork(ArtworkRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            size: ArtworkSize::Standard,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("artwork err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.variants.len(), 1);
        assert_eq!(resp.variants[0].url, "https://example.com/p.jpg");
        assert_eq!(resp.variants[0].size, ArtworkSize::Standard);
        assert_eq!(resp.variants[0].mime, "image/jpeg");
    }

    #[test]
    fn get_artwork_returns_empty_when_no_poster() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,"runtime_minutes":100,
            "rating":7.0,"genres":[],"poster_url":null,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{}
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_artwork(ArtworkRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            size: ArtworkSize::Standard,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            _ => panic!("expected Ok"),
        };
        assert!(resp.variants.is_empty());
    }

    #[test]
    fn related_projects_similar_titles_with_imdb_id() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{},
            "similar_titles":[
                {"id":"tt0468569","title":"Y","year":2008,"rating":9.1,
                 "poster_url":"https://e.com/y.jpg"},
                {"id":"tt0099685","title":"Z","year":1990,"rating":8.7,
                 "poster_url":null}
            ]
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.related(RelatedRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            relation: RelationKind::Similar,
            limit: 0,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("related err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.items[0].title, "Y");
        assert_eq!(resp.items[0].imdb_id.as_deref(), Some("tt0468569"));
        assert_eq!(resp.items[0].kind, EntryKind::Movie);
    }

    #[test]
    fn get_trailers_extracts_url_thumbnail_and_name() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{},
            "trailer": {"id":"vi1","name":"Official Trailer",
                        "url":"https://yt/abc","thumbnail":"https://yt/abc.jpg"}
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_trailers(TrailersRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            locale: None,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("trailers err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.trailers.len(), 1);
        assert_eq!(resp.trailers[0].url, "https://yt/abc");
        assert_eq!(resp.trailers[0].thumbnail_url.as_deref(), Some("https://yt/abc.jpg"));
        assert_eq!(resp.trailers[0].title.as_deref(), Some("Official Trailer"));
        assert_eq!(resp.trailers[0].kind, TrailerKind::Trailer);
    }

    #[test]
    fn get_trailers_returns_empty_when_no_trailer_in_payload() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{"id":"xmdb-1","title":"X","release_year":2000,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{}}"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_trailers(TrailersRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            locale: None,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            _ => panic!("expected Ok"),
        };
        assert!(resp.trailers.is_empty());
    }

    #[test]
    fn get_release_info_projects_per_country_dates() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{},
            "release_dates":[
                {"date":"1994-09-10","day":10,"month":9,"year":1994,
                 "country":"Canada","country_code":"CA","release_type":"label"},
                {"date":"1994-10-14","country":"United States","country_code":"US"}
            ]
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_release_info(ReleaseInfoRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("release_info err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.releases.len(), 2);
        assert_eq!(resp.releases[0].country, "CA");
        assert_eq!(resp.releases[0].date.as_deref(), Some("1994-09-10"));
        assert!(resp.releases[0].release_kind.is_none());
    }

    #[test]
    fn get_keywords_passes_through_strings() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{},
            "keywords":["indie","drama","prison",""," "]
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_keywords(KeywordsRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("keywords err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.keywords.len(), 3);  // empties filtered
        assert_eq!(resp.keywords[0].name, "indie");
        assert!(resp.keywords[0].source_id.is_none());
        assert!(resp.keywords[0].provider.is_none());
    }

    #[test]
    fn get_box_office_extracts_amounts_and_currencies() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{},
            "budget":{"amount":25000000,"currency":"USD"},
            "worldwide_gross":{"amount":29420884,"currency":"USD"},
            "opening_weekend_gross":{"amount":0,"currency":"USD"}
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_box_office(BoxOfficeRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("box_office err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.budget.unwrap().amount, 25_000_000);
        assert_eq!(resp.gross_worldwide.unwrap().amount, 29_420_884);
        assert!(resp.opening_weekend.is_none(), "zero amount should drop");
        assert!(resp.gross_domestic.is_none(),  "missing should drop");
    }

    #[test]
    fn get_alternative_titles_projects_locale_and_country() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let fixture = r#"{
            "id":"xmdb-1","title":"X","release_year":2000,
            "imdb_url":"https://www.imdb.com/title/tt0000001/","title_type":"movie",
            "full_cast_and_crew":{},
            "alternative_titles":[
                {"title":"Les Évadés","country":"France","country_code":"FR",
                 "language":"French","language_code":"fr"},
                {"title":"","country":"X","country_code":"X"},
                {"title":"الخلاص","country":"UAE","country_code":"AE"}
            ]
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://xmdbapi.com/api/v1/movies/tt0000001?apiKey=fake",
            fixture,
        );
        let plugin = XmdbPlugin::new_for_test("fake");
        let resp = match plugin.get_alternative_titles(AlternativeTitlesRequest {
            id: "tt0000001".into(),
            id_source: id_sources::IMDB.to_string(),
            kind: EntryKind::Movie,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("alt_titles err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.titles.len(), 2);  // empty title filtered
        assert_eq!(resp.titles[0].title, "Les \u{00c9}vad\u{00e9}s");
        assert_eq!(resp.titles[0].locale.as_deref(), Some("fr"));
        assert_eq!(resp.titles[0].country.as_deref(), Some("FR"));
        assert!(resp.titles[0].kind.is_none());
    }

    // ── Task 6.3: Group X projection test ────────────────────────────────────

    #[test]
    fn movie_detail_projects_group_x_fields() {
        let detail = MovieDetail {
            id:                     "xmdb-1".into(),
            title:                  "Shawshank".into(),
            original_title:         Some("The Shawshank Redemption".into()),
            release_year:           Some(1994),
            runtime_minutes:        Some(142),
            rating:                 Some(9.3),
            vote_count:             Some(2_700_000),
            metascore:              Some(82.0),
            certificate:            Some("R".into()),
            genres:                 vec!["Drama".into()],
            languages:              vec!["en".into()],
            countries:              vec!["US".into()],
            plot:                   None,
            poster_url:             None,
            imdb_url:               Some("https://www.imdb.com/title/tt0111161/".into()),
            title_type:             Some("movie".into()),
            full_cast_and_crew:     Default::default(),
            keywords:               vec![],
            alternative_titles:     vec![],
            release_dates:          vec![],
            similar_titles:         vec![],
            trailer:                None,
            budget:                 None,
            opening_weekend_gross:  None,
            lifetime_gross:         None,
            worldwide_gross:        None,
        };
        let e = detail.into_entry(EntryKind::Movie);
        assert_eq!(e.original_title.as_deref(), Some("The Shawshank Redemption"));
        assert_eq!(e.certificate.as_deref(), Some("R"));
        assert_eq!(e.certificate_country.as_deref(), Some("US"));
        assert_eq!(e.languages, vec!["en".to_string()]);
        assert_eq!(e.countries, vec!["US".to_string()]);
        assert_eq!(e.rating_votes.get("imdb").copied(), Some(2_700_000));
    }
}
