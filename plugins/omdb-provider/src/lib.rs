//! OMDb metadata provider — movies and series via the Open Movie Database.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup}`. OMDb has no
//! related / credits / artwork-catalog / enrich endpoints, so those verbs
//! default to `NOT_IMPLEMENTED` from the trait.
//!
//! ## API key
//!
//! Required. Read from `InitContext.config["api_key"]` at `Plugin::init`.
//! Fallback: `OMDB_API_KEY` env var surfaced by the host through
//! `cache_get("__env:OMDB_API_KEY")`.

use std::sync::OnceLock;

use serde::Deserialize;

use stui_plugin_sdk::{
    cache_get, error_codes, http_get,
    id_sources,
    plugin_error, plugin_info,
    stui_export_catalog_plugin,
    CatalogPlugin,
    EntryKind,
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
        let manifest: PluginManifest = toml::from_str(include_str!("../plugin.toml"))
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
}

impl DetailResponse {
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
            description: opt_non_na(&self.plot),
            genre:       opt_non_na(&self.genre),
            rating:      opt_non_na(&self.rating).and_then(|s| s.parse::<f32>().ok()),
            duration:    parse_runtime_minutes(&self.runtime),
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
}
