//! Rotten Tomatoes metadata provider — critic + audience scores via self-hosted rt-api.
//!
//! Implements `Plugin` + `CatalogPlugin::search` (stub, returns NOT_IMPLEMENTED).
//! Primary verb is `bulk_enrich` (batch endpoint, ≤50 IMDb IDs per call).
//! `enrich` provides a single-entry fallback.
//!
//! ## API key
//!
//! Required. Read from `InitContext.config["api_key"]` at `Plugin::init`.
//! Fallback: `RT_API_KEY` env var surfaced by the host through
//! `cache_get("__env:RT_API_KEY")`.
//!
//! ## Base URL
//!
//! Optional. Defaults to `https://rotten-tomatoes-api-delta.vercel.app`.
//! Override via `InitContext.config["base_url"]` or `RT_API_BASE_URL` env var.

use std::sync::OnceLock;

use stui_plugin_sdk::{
    err_not_implemented,
    error_codes,
    http_request, HttpRequest,
    parse_manifest,
    plugin_error, plugin_info,
    stui_export_catalog_plugin,
    BulkEnrichEntry, BulkEnrichRequest, BulkEnrichResponse,
    CatalogPlugin, EnrichRequest, EnrichResponse,
    EntryKind,
    InitContext,
    Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    SearchRequest, SearchResponse,
    StreamProvider,
};
use serde::{Deserialize, Serialize};

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct RtProvider {
    manifest: PluginManifest,
    api_key:  OnceLock<String>,
    base_url: OnceLock<String>,
}

const DEFAULT_BASE_URL: &str = "https://rotten-tomatoes-api-delta.vercel.app";

impl RtProvider {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self {
            manifest,
            api_key:  OnceLock::new(),
            base_url: OnceLock::new(),
        }
    }

    #[cfg(test)]
    pub fn new_for_test(api_key: &str, base_url: &str) -> Self {
        let inst = Self::new();
        let _ = inst.api_key.set(api_key.to_string());
        let _ = inst.base_url.set(base_url.to_string());
        inst
    }

    /// Resolve the API key: cached → env fallback. Returns `INVALID_REQUEST`
    /// when unset so the caller can surface it to the user.
    fn api_key(&self) -> Result<&str, PluginError> {
        if let Some(k) = self.api_key.get() {
            return Ok(k.as_str());
        }
        let env_key = stui_plugin_sdk::cache_get("__env:RT_API_KEY").unwrap_or_default();
        if env_key.is_empty() {
            return Err(PluginError {
                code: error_codes::INVALID_REQUEST.to_string(),
                message: "RT api_key not configured".to_string(),
            });
        }
        Ok(self.api_key.get_or_init(|| env_key).as_str())
    }

    /// Resolve the base URL: cached → env fallback → default.
    fn base_url(&self) -> &str {
        if let Some(u) = self.base_url.get() {
            return u.as_str();
        }
        let env_url = stui_plugin_sdk::cache_get("__env:RT_API_BASE_URL").unwrap_or_default();
        let resolved = if env_url.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            env_url
        };
        self.base_url.get_or_init(|| resolved).as_str()
    }

    /// Fetch a single movie's RT data via `GET /api/v1/movie/{imdb_id}`.
    fn fetch_single(&self, imdb_id: &str) -> Result<RtMovieDetail, PluginError> {
        let api_key = self.api_key()?.to_string();
        let url = format!(
            "{}/api/v1/movie/{}",
            self.base_url(),
            urlencoding::encode(imdb_id),
        );
        let req = HttpRequest {
            method: "GET".to_string(),
            url,
            headers: vec![("X-API-Key".to_string(), api_key)],
            body: None,
        };
        plugin_info!("rt-provider: fetch_single {}", imdb_id);
        let resp = http_request(req).map_err(|e| PluginError {
            code: error_codes::TRANSIENT.to_string(),
            message: format!("rt-api: {e}"),
        })?;
        classify_http(resp.status)?;
        serde_json::from_str::<RtMovieDetail>(&resp.body).map_err(|e| {
            plugin_error!("rt-provider: parse error: {}", e);
            PluginError {
                code: error_codes::PARSE_ERROR.to_string(),
                message: format!("rt-api: JSON parse failure: {e}"),
            }
        })
    }

    /// Fetch a single chunk (≤50 ids) via `POST /api/v1/movies/batch`.
    /// Returns the parsed SSE event stream.
    fn fetch_batch(&self, imdb_ids: &[String]) -> Result<Vec<RtBatchEvent>, PluginError> {
        let api_key = self.api_key()?.to_string();
        let url = format!("{}/api/v1/movies/batch", self.base_url());
        let body_payload = serde_json::to_string(&BatchRequestBody { imdb_ids })
            .map_err(|e| PluginError {
                code: error_codes::PARSE_ERROR.to_string(),
                message: format!("rt-api: serialize batch body: {e}"),
            })?;
        let req = HttpRequest {
            method: "POST".to_string(),
            url,
            headers: vec![
                ("X-API-Key".to_string(), api_key),
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            body: Some(body_payload),
        };
        plugin_info!("rt-provider: fetch_batch {} ids", imdb_ids.len());
        let resp = http_request(req).map_err(|e| PluginError {
            code: error_codes::TRANSIENT.to_string(),
            message: format!("rt-api: {e}"),
        })?;
        classify_http(resp.status)?;
        Ok(parse_sse(&resp.body))
    }
}

impl Default for RtProvider {
    fn default() -> Self { Self::new() }
}

// ── Plugin impl ───────────────────────────────────────────────────────────────

impl Plugin for RtProvider {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        // Resolve API key.
        let key = ctx
            .config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("RT_API_KEY").cloned())
            .unwrap_or_default();
        if key.is_empty() {
            return Err(PluginInitError::MissingConfig {
                fields: vec!["api_key".to_string()],
                hint: Some(
                    "Get a key from your self-hosted rt-api admin endpoint (X-API-Key header)"
                        .to_string(),
                ),
            });
        }
        let _ = self.api_key.set(key);

        // Resolve base URL — optional, falls back to DEFAULT_BASE_URL.
        let url = ctx
            .config
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("RT_API_BASE_URL").cloned())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let _ = self.base_url.set(url);

        Ok(())
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for RtProvider {
    /// rt-api has no search endpoint; return NOT_IMPLEMENTED so the runtime
    /// fan-out skips us during title searches.
    fn search(&self, _req: SearchRequest) -> PluginResult<SearchResponse> {
        err_not_implemented()
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        // Resolve imdb_id from the partial entry.
        let imdb = req.partial.imdb_id.clone()
            .or_else(|| req.partial.external_ids.get("imdb").cloned())
            .filter(|s| !s.is_empty());
        let Some(imdb_id) = imdb else {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                "rt-provider enrich: imdb_id is required",
            );
        };
        match self.fetch_single(&imdb_id) {
            Ok(detail) => PluginResult::ok(project_movie(detail)),
            Err(e) => PluginResult::Err(e),
        }
    }

    fn bulk_enrich(&self, req: BulkEnrichRequest) -> PluginResult<BulkEnrichResponse> {
        // Filter input to partials with an imdb_id; collect the
        // (input_id, imdb_id) mapping so we can reorder results to
        // match input order.
        let mut imdb_ids: Vec<String> = Vec::with_capacity(req.partials.len());
        for partial in &req.partials {
            if let Some(imdb) = partial.imdb_id.clone()
                .or_else(|| partial.external_ids.get("imdb").cloned())
                .filter(|s| !s.is_empty())
            {
                imdb_ids.push(imdb);
            }
        }

        // Truly empty input: return Ok(empty) with no HTTP calls.
        if req.partials.is_empty() {
            return PluginResult::ok(BulkEnrichResponse { entries: vec![] });
        }

        // Aggregate per-id results across chunks.
        let mut by_imdb: std::collections::HashMap<String, BulkEnrichEntry> =
            std::collections::HashMap::new();

        for chunk in imdb_ids.chunks(BULK_CHUNK_SIZE) {
            let chunk_vec: Vec<String> = chunk.to_vec();
            match self.fetch_batch(&chunk_vec) {
                Ok(events) => {
                    for ev in events {
                        match ev {
                            RtBatchEvent::Movie(detail) => {
                                let id = detail.imdb_id.clone();
                                let resp = project_movie(detail);
                                by_imdb.insert(id.clone(), BulkEnrichEntry {
                                    id,
                                    result: PluginResult::ok(resp),
                                });
                            }
                            RtBatchEvent::Error { imdb_id, error, message } => {
                                let code = match error.as_str() {
                                    "not_found"     => error_codes::UNKNOWN_ID,
                                    "scrape_failed" => error_codes::TRANSIENT,
                                    "invalid_id"    => error_codes::INVALID_REQUEST,
                                    _               => error_codes::REMOTE_ERROR,
                                };
                                by_imdb.insert(imdb_id.clone(), BulkEnrichEntry {
                                    id: imdb_id,
                                    result: PluginResult::err(code, message),
                                });
                            }
                            RtBatchEvent::Done { total, cached, fetched, errors } => {
                                plugin_info!(
                                    "rt-provider: batch done total={} cached={} fetched={} errors={}",
                                    total, cached, fetched, errors,
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    // Whole-chunk failure (network, 401, 5xx). Emit
                    // an Err per id in this chunk so the orchestrator
                    // sees the failures at per-entry granularity.
                    for id in &chunk_vec {
                        by_imdb.insert(id.clone(), BulkEnrichEntry {
                            id: id.clone(),
                            result: PluginResult::err(e.code.clone(), e.message.clone()),
                        });
                    }
                }
            }
        }

        // Emit entries in input order. For input partials missing an
        // imdb_id, emit an UNKNOWN_ID error.
        let entries: Vec<BulkEnrichEntry> = req.partials.into_iter()
            .map(|partial| {
                let id = partial.id.clone();
                if let Some(imdb) = partial.imdb_id.as_deref()
                    .or_else(|| partial.external_ids.get("imdb").map(String::as_str))
                    .filter(|s| !s.is_empty())
                {
                    if let Some(entry) = by_imdb.remove(imdb) {
                        // Stamp the input-side id (might differ from imdb).
                        BulkEnrichEntry { id, result: entry.result }
                    } else {
                        // imdb id was sent but no event came back.
                        BulkEnrichEntry {
                            id,
                            result: PluginResult::err(
                                error_codes::TRANSIENT,
                                "rt-api: no response event for this id",
                            ),
                        }
                    }
                } else {
                    BulkEnrichEntry {
                        id,
                        result: PluginResult::err(
                            error_codes::UNKNOWN_ID,
                            "rt-provider: imdb_id is required",
                        ),
                    }
                }
            })
            .collect();

        PluginResult::ok(BulkEnrichResponse { entries })
    }
}

// ── StreamProvider (stub — rt-api has no stream endpoint) ────────────────────

impl StreamProvider for RtProvider {}

// ── WASM export ───────────────────────────────────────────────────────────────

stui_export_catalog_plugin!(RtProvider);

// ── API types (serde) ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct RtMovieDetail {
    #[serde(rename = "imdbId")]   imdb_id: String,
    #[serde(rename = "rtUrl")]    rt_url:  String,
    #[serde(default)]             title:   String,
    #[serde(default)]             year:    Option<u32>,
    #[serde(rename = "criticScore",   default)] critic_score:   Option<u32>,
    #[serde(rename = "audienceScore", default)] audience_score: Option<u32>,
}

#[derive(Debug, Serialize)]
struct BatchRequestBody<'a> {
    #[serde(rename = "imdbIds")]
    imdb_ids: &'a [String],
}

#[derive(Debug)]
enum RtBatchEvent {
    Movie(RtMovieDetail),
    Error { imdb_id: String, error: String, message: String },
    Done { total: u32, cached: u32, fetched: u32, errors: u32 },
}

#[derive(Debug, Deserialize)]
struct BatchErrorBody {
    #[serde(rename = "imdbId")]  imdb_id: String,
    #[serde(default)]            error:   String,
    #[serde(default)]            message: String,
}

#[derive(Debug, Deserialize)]
struct BatchDoneBody {
    #[serde(default)] total:   u32,
    #[serde(default)] cached:  u32,
    #[serde(default)] fetched: u32,
    #[serde(default)] errors:  u32,
}

// ── Chunking constant ─────────────────────────────────────────────────────────

const BULK_CHUNK_SIZE: usize = 50;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the RT URL slug. Returns `Some("m/the_dark_knight")` or
/// `Some("tv/breaking_bad")` for canonical RT URLs; `None` for malformed input.
fn parse_rt_slug(url: &str) -> Option<String> {
    let path_start = url.find("rottentomatoes.com/")?;
    let after = &url[path_start + "rottentomatoes.com/".len()..];
    let trimmed = after.trim_end_matches('/');
    if trimmed.is_empty() { return None; }
    let slug_end = trimmed.find(['?', '#']).unwrap_or(trimmed.len());
    let slug = &trimmed[..slug_end];
    if slug.starts_with("m/") || slug.starts_with("tv/") {
        Some(slug.to_string())
    } else {
        None
    }
}

/// Infer EntryKind from RT URL path prefix.
fn kind_from_rt_url(url: &str) -> Option<EntryKind> {
    if url.contains("/m/") { Some(EntryKind::Movie) }
    else if url.contains("/tv/") { Some(EntryKind::Series) }
    else { None }
}

/// Build the EnrichResponse from an RT detail payload. Empty/null
/// scores are dropped from the ratings map. RT title and consensus
/// are intentionally skipped per spec.
fn project_movie(detail: RtMovieDetail) -> EnrichResponse {
    let kind = kind_from_rt_url(&detail.rt_url).unwrap_or(EntryKind::Movie);
    let mut ratings = std::collections::HashMap::new();
    if let Some(s) = detail.critic_score.filter(|v| *v > 0) {
        ratings.insert("tomatometer".to_string(), s as f32);
    }
    if let Some(s) = detail.audience_score.filter(|v| *v > 0) {
        ratings.insert("audience_score".to_string(), s as f32);
    }
    let mut entry = PluginEntry {
        id: detail.imdb_id.clone(),
        kind,
        title: String::new(),  // RT title unreliable for TV; skip
        source: "rottentomatoes".to_string(),
        imdb_id: Some(detail.imdb_id.clone()),
        ratings,
        ..Default::default()
    };
    entry.external_ids.insert("imdb".to_string(), detail.imdb_id);
    if let Some(slug) = parse_rt_slug(&detail.rt_url) {
        entry.external_ids.insert("rottentomatoes".to_string(), slug);
    }
    EnrichResponse { entry, confidence: 1.0 }
}

/// Map an HTTP response status to a `PluginError`. Success returns Ok.
fn classify_http(status: u16) -> Result<(), PluginError> {
    match status {
        200..=299 => Ok(()),
        401 => Err(PluginError {
            code: error_codes::INVALID_REQUEST.to_string(),
            message: "rt-api: unauthorized (check RT_API_KEY)".to_string(),
        }),
        404 => Err(PluginError {
            code: error_codes::UNKNOWN_ID.to_string(),
            message: "rt-api: not found".to_string(),
        }),
        429 => Err(PluginError {
            code: error_codes::RATE_LIMITED.to_string(),
            message: "rt-api: rate limit exceeded".to_string(),
        }),
        500..=599 => Err(PluginError {
            code: error_codes::TRANSIENT.to_string(),
            message: format!("rt-api: HTTP {status}"),
        }),
        _ => Err(PluginError {
            code: error_codes::REMOTE_ERROR.to_string(),
            message: format!("rt-api: unexpected HTTP {status}"),
        }),
    }
}

/// Parse the rt-api batch endpoint's SSE response body. Splits on
/// `\n\n` block boundary, decodes each block by reading the
/// `event:` and `data:` lines, returns a Vec of typed events.
/// Unknown event types and malformed blocks are skipped silently.
fn parse_sse(body: &str) -> Vec<RtBatchEvent> {
    let mut out = Vec::new();
    for block in body.split("\n\n") {
        let trimmed = block.trim();
        if trimmed.is_empty() { continue; }

        let mut event_name: Option<&str> = None;
        let mut data_payload: Option<&str> = None;
        for line in trimmed.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_name = Some(rest.trim());
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_payload = Some(rest.trim());
            }
        }
        let (Some(event_name), Some(data_payload)) = (event_name, data_payload) else {
            continue;
        };
        match event_name {
            "movie" => {
                if let Ok(detail) = serde_json::from_str::<RtMovieDetail>(data_payload) {
                    out.push(RtBatchEvent::Movie(detail));
                }
            }
            "error" => {
                if let Ok(b) = serde_json::from_str::<BatchErrorBody>(data_payload) {
                    out.push(RtBatchEvent::Error {
                        imdb_id: b.imdb_id,
                        error:   b.error,
                        message: b.message,
                    });
                }
            }
            "done" => {
                if let Ok(b) = serde_json::from_str::<BatchDoneBody>(data_payload) {
                    out.push(RtBatchEvent::Done {
                        total: b.total, cached: b.cached,
                        fetched: b.fetched, errors: b.errors,
                    });
                }
            }
            _ => { /* unknown event; skip */ }
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        // Verifies the struct compiles as a CatalogPlugin and Plugin.
        let p = RtProvider::new();
        let _manifest = p.manifest();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = RtProvider::new();
        let m = p.manifest();
        assert_eq!(m.plugin.name, "rottentomatoes");
        assert_eq!(m.plugin._abi_version, Some(2));
    }

    #[test]
    fn search_returns_not_implemented() {
        use stui_plugin_sdk::SearchScope;
        let p = RtProvider::new();
        let result = p.search(SearchRequest {
            query: "The Shawshank Redemption".to_string(),
            scope: SearchScope::Movie,
            page: 1,
            limit: 10,
            per_scope_limit: None,
            locale: None,
        });
        match result {
            stui_plugin_sdk::PluginResult::Err(e) => {
                assert_eq!(e.code, stui_plugin_sdk::error_codes::NOT_IMPLEMENTED);
            }
            stui_plugin_sdk::PluginResult::Ok(_) => panic!("expected NOT_IMPLEMENTED, got Ok"),
        }
    }

    // ── 2.1.3: helper unit tests ──────────────────────────────────────────────

    #[test]
    fn parse_rt_slug_extracts_movie_path() {
        assert_eq!(
            parse_rt_slug("https://www.rottentomatoes.com/m/the_dark_knight"),
            Some("m/the_dark_knight".to_string()),
        );
    }

    #[test]
    fn parse_rt_slug_extracts_tv_path() {
        assert_eq!(
            parse_rt_slug("https://www.rottentomatoes.com/tv/breaking_bad"),
            Some("tv/breaking_bad".to_string()),
        );
    }

    #[test]
    fn parse_rt_slug_returns_none_on_garbage() {
        assert_eq!(parse_rt_slug("not a url"), None);
        assert_eq!(parse_rt_slug("https://example.com/m/foo"), None);
        assert_eq!(parse_rt_slug("https://www.rottentomatoes.com/"), None);
    }

    #[test]
    fn kind_from_rt_url_handles_movie_and_tv_paths() {
        assert_eq!(kind_from_rt_url("https://www.rottentomatoes.com/m/x"), Some(EntryKind::Movie));
        assert_eq!(kind_from_rt_url("https://www.rottentomatoes.com/tv/x"), Some(EntryKind::Series));
        assert_eq!(kind_from_rt_url("https://www.rottentomatoes.com/foo/x"), None);
    }

    #[test]
    fn project_movie_populates_ratings_and_external_ids() {
        let d = RtMovieDetail {
            imdb_id: "tt0111161".into(),
            rt_url: "https://www.rottentomatoes.com/m/shawshank_redemption".into(),
            title: "The Shawshank Redemption".into(),
            year: Some(1994),
            critic_score: Some(89),
            audience_score: Some(98),
        };
        let resp = project_movie(d);
        assert_eq!(resp.confidence, 1.0);
        let e = resp.entry;
        assert_eq!(e.kind, EntryKind::Movie);
        assert_eq!(e.source, "rottentomatoes");
        assert_eq!(e.imdb_id.as_deref(), Some("tt0111161"));
        assert_eq!(e.ratings.get("tomatometer").copied(), Some(89.0));
        assert_eq!(e.ratings.get("audience_score").copied(), Some(98.0));
        assert_eq!(
            e.external_ids.get("rottentomatoes").map(String::as_str),
            Some("m/shawshank_redemption"),
        );
        assert_eq!(e.external_ids.get("imdb").map(String::as_str), Some("tt0111161"));
    }

    #[test]
    fn project_movie_drops_zero_or_null_scores() {
        let d = RtMovieDetail {
            imdb_id: "tt1".into(),
            rt_url: "https://www.rottentomatoes.com/m/x".into(),
            title: "".into(),
            year: None,
            critic_score: Some(0),
            audience_score: None,
        };
        let resp = project_movie(d);
        assert!(resp.entry.ratings.is_empty(), "0 + null should drop both");
    }

    #[test]
    fn project_movie_handles_tv_empty_title() {
        let d = RtMovieDetail {
            imdb_id: "tt0903747".into(),
            rt_url: "https://www.rottentomatoes.com/tv/breaking_bad".into(),
            title: "".into(),  // RT TV pages return empty
            year: Some(2008),
            critic_score: Some(96),
            audience_score: Some(97),
        };
        let resp = project_movie(d);
        assert_eq!(resp.entry.kind, EntryKind::Series);
        assert_eq!(resp.entry.ratings.get("tomatometer").copied(), Some(96.0));
        assert_eq!(
            resp.entry.external_ids.get("rottentomatoes").map(String::as_str),
            Some("tv/breaking_bad"),
        );
    }

    #[test]
    fn classify_http_maps_2xx_to_ok() {
        assert!(classify_http(200).is_ok());
        assert!(classify_http(204).is_ok());
    }

    #[test]
    fn classify_http_maps_401_to_invalid_request() {
        let e = classify_http(401).unwrap_err();
        assert_eq!(e.code, error_codes::INVALID_REQUEST);
    }

    #[test]
    fn classify_http_maps_404_to_unknown_id() {
        let e = classify_http(404).unwrap_err();
        assert_eq!(e.code, error_codes::UNKNOWN_ID);
    }

    #[test]
    fn classify_http_maps_429_to_rate_limited() {
        let e = classify_http(429).unwrap_err();
        assert_eq!(e.code, error_codes::RATE_LIMITED);
    }

    #[test]
    fn classify_http_maps_502_to_transient() {
        let e = classify_http(502).unwrap_err();
        assert_eq!(e.code, error_codes::TRANSIENT);
    }

    #[test]
    fn parse_sse_decodes_movie_event() {
        let body = "event: movie\ndata: {\"imdbId\":\"tt0111161\",\"rtUrl\":\"https://www.rottentomatoes.com/m/shawshank\",\"criticScore\":89,\"audienceScore\":98}\n\n";
        let events = parse_sse(body);
        assert_eq!(events.len(), 1);
        match &events[0] {
            RtBatchEvent::Movie(d) => assert_eq!(d.imdb_id, "tt0111161"),
            _ => panic!("expected Movie variant"),
        }
    }

    #[test]
    fn parse_sse_decodes_mixed_movie_error_done() {
        let body = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/batch_mixed.sse"
        )).expect("captured fixture missing — run Chunk 1 Task 1.4 first");
        let events = parse_sse(&body);
        // 2 movie + 1 error + 1 done = 4 events
        assert_eq!(events.len(), 4, "events: {:?}", events);
        let movies = events.iter().filter(|e| matches!(e, RtBatchEvent::Movie(_))).count();
        let errors = events.iter().filter(|e| matches!(e, RtBatchEvent::Error { .. })).count();
        let dones  = events.iter().filter(|e| matches!(e, RtBatchEvent::Done { .. })).count();
        assert_eq!(movies, 2);
        assert_eq!(errors, 1);
        assert_eq!(dones, 1);
    }

    #[test]
    fn parse_sse_skips_unknown_event_types() {
        let body = "event: unknown\ndata: {}\n\n";
        let events = parse_sse(body);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_sse_handles_missing_data_line() {
        let body = "event: done\n\n";
        let events = parse_sse(body);
        assert!(events.is_empty());
    }

    // ── 2.2.3: enrich MockHost-driven tests ───────────────────────────────────

    #[test]
    fn enrich_single_id_round_trips_through_mock_host() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let body = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/single_movie_tt0111161.json"
        )).expect("fixture missing — run Chunk 1 Task 1.4");
        let _h = MockHost::new().with_fixture_response(
            "https://test.example.com/api/v1/movie/tt0111161",
            &body,
        );
        let p = RtProvider::new_for_test("fake-key", "https://test.example.com");
        let req = EnrichRequest {
            partial: PluginEntry {
                id: "tt0111161".into(),
                kind: EntryKind::Movie,
                title: "Shawshank".into(),
                source: "test".into(),
                imdb_id: Some("tt0111161".into()),
                ..Default::default()
            },
            prefer_id_source: None,
            force_refresh: false,
        };
        let resp = match p.enrich(req) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("enrich err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.entry.imdb_id.as_deref(), Some("tt0111161"));
        assert!(resp.entry.ratings.contains_key("tomatometer"));
        assert!(resp.entry.ratings.contains_key("audience_score"));
        assert!(resp.entry.external_ids.get("rottentomatoes").is_some());
    }

    #[test]
    fn enrich_without_imdb_id_returns_unknown_id() {
        let p = RtProvider::new_for_test("fake-key", "https://test.example.com");
        let req = EnrichRequest {
            partial: PluginEntry {
                id: "no-imdb".into(),
                kind: EntryKind::Movie,
                title: "x".into(),
                source: "test".into(),
                imdb_id: None,
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

    // ── 2.3.3: bulk_enrich MockHost-driven tests ──────────────────────────────

    #[test]
    fn bulk_enrich_aggregates_per_id_results() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let sse = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/batch_mixed.sse"
        )).expect("fixture missing — run Chunk 1 Task 1.4");
        let _h = MockHost::new().with_fixture_response(
            "https://test.example.com/api/v1/movies/batch",
            &sse,
        );
        let p = RtProvider::new_for_test("fake-key", "https://test.example.com");
        let req = BulkEnrichRequest {
            partials: vec![
                PluginEntry { id: "1".into(), kind: EntryKind::Movie,
                              imdb_id: Some("tt0111161".into()), ..Default::default() },
                PluginEntry { id: "2".into(), kind: EntryKind::Movie,
                              imdb_id: Some("tt0468569".into()), ..Default::default() },
                PluginEntry { id: "3".into(), kind: EntryKind::Movie,
                              imdb_id: Some("tt9999999".into()), ..Default::default() },
            ],
            prefer_id_source: None,
            force_refresh: false,
        };
        let resp = match p.bulk_enrich(req) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("bulk err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.entries.len(), 3);
        // Two Ok (movie events), one Err (error event).
        let oks = resp.entries.iter()
            .filter(|e| matches!(e.result, PluginResult::Ok(_))).count();
        let errs = resp.entries.iter()
            .filter(|e| matches!(e.result, PluginResult::Err(_))).count();
        assert_eq!(oks, 2);
        assert_eq!(errs, 1);
        // Verify the error is UNKNOWN_ID (rt-api `not_found` event).
        let bad = resp.entries.iter()
            .find(|e| e.id == "3").expect("entry 3");
        match &bad.result {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            _ => panic!("entry 3 should be Err"),
        }
    }

    #[test]
    fn bulk_enrich_chunks_to_50_ids_per_call() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        // Build a minimal `done`-only SSE so the response parses but
        // emits no movie/error events. The plugin will still iterate
        // each chunk's (empty) result and emit per-id "no event"
        // errors — that's fine for this test, we only care about
        // the chunking behavior (HTTP call count).
        let sse = "event: done\ndata: {\"total\":0,\"cached\":0,\"fetched\":0,\"errors\":0}\n\n";
        let _h = MockHost::new().with_fixture_response(
            "https://test.example.com/api/v1/movies/batch",
            sse,
        );
        let p = RtProvider::new_for_test("fake-key", "https://test.example.com");
        // 75 ids → 2 chunks (50 + 25).
        let partials: Vec<PluginEntry> = (0..75).map(|i| PluginEntry {
            id: format!("e{i}"),
            kind: EntryKind::Movie,
            imdb_id: Some(format!("tt{i:07}")),
            ..Default::default()
        }).collect();
        let req = BulkEnrichRequest {
            partials,
            prefer_id_source: None,
            force_refresh: false,
        };
        let _ = p.bulk_enrich(req);
        assert_eq!(MockHost::http_call_count(), 2,
                   "75 ids should result in 2 batch calls (50 + 25)");
    }

    #[test]
    fn bulk_enrich_empty_input_returns_ok_empty_zero_calls() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let _h = MockHost::new();
        let p = RtProvider::new_for_test("fake-key", "https://test.example.com");
        let req = BulkEnrichRequest {
            partials: vec![],
            prefer_id_source: None,
            force_refresh: false,
        };
        let resp = match p.bulk_enrich(req) {
            PluginResult::Ok(r) => r,
            _ => panic!("expected Ok"),
        };
        assert!(resp.entries.is_empty());
        assert_eq!(MockHost::http_call_count(), 0,
                   "empty input should make zero HTTP calls");
    }

    #[test]
    fn bulk_enrich_drops_partials_without_imdb_id() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let _h = MockHost::new();
        let p = RtProvider::new_for_test("fake-key", "https://test.example.com");
        let req = BulkEnrichRequest {
            partials: vec![
                PluginEntry { id: "no-imdb".into(), kind: EntryKind::Movie,
                              imdb_id: None, ..Default::default() },
            ],
            prefer_id_source: None,
            force_refresh: false,
        };
        let resp = match p.bulk_enrich(req) {
            PluginResult::Ok(r) => r,
            _ => panic!("expected Ok"),
        };
        // No-imdb partial: still emits an entry (with UNKNOWN_ID Err)
        // so callers see a result for every input. No HTTP call made.
        assert_eq!(resp.entries.len(), 1);
        match &resp.entries[0].result {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            _ => panic!("expected UNKNOWN_ID"),
        }
        assert_eq!(MockHost::http_call_count(), 0);
    }

    // ── 2.4.1: Live API smoke scaffold ────────────────────────────────────────

    #[test]
    #[ignore]
    fn live_smoke_against_user_deployment() {
        let key = std::env::var("RT_API_KEY")
            .expect("source ~/.config/stui/secrets.env first");
        let base = std::env::var("RT_API_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        // Direct reqwest-style call would require host-side reqwest in
        // dev-deps; simpler to use ureq blocking with http(s) feature.
        // For now, just exercise project_movie against a real captured
        // payload — the fixture test in 2.2 already covers the wire.
        // This test is a no-op unless the implementer wires real HTTP
        // here; left as a scaffold for manual verification.
        eprintln!("RT_API_KEY length: {}, base: {}", key.len(), base);
    }
}
