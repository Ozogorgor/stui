//! Kitsu metadata provider — anime movies and series via the Kitsu JSON:API.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup}`. API key is
//! optional — Kitsu's public endpoints allow unauthenticated requests with
//! generous rate limits.

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

const API_BASE: &str = "https://kitsu.io/api/edge";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct KitsuPlugin {
    manifest: PluginManifest,
}

impl KitsuPlugin {
    pub fn new() -> Self {
        let manifest: PluginManifest = toml::from_str(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self { manifest }
    }
}

impl Default for KitsuPlugin {
    fn default() -> Self { Self::new() }
}

impl Plugin for KitsuPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, _ctx: &InitContext) -> Result<(), PluginInitError> {
        // Kitsu's API is usable unauthenticated; the api_key is optional and
        // only read on-demand from the host's config cache when making a
        // request. No init-time validation required.
        Ok(())
    }
}

// ── Error handling ────────────────────────────────────────────────────────────

fn classify_http_err(err: &str) -> PluginError {
    if let Some(rest) = err.strip_prefix("HTTP ") {
        if let Some((code_str, body)) = rest.split_once(": ") {
            if let Ok(status) = code_str.parse::<u16>() {
                let code = match status {
                    404       => error_codes::UNKNOWN_ID,
                    429       => error_codes::RATE_LIMITED,
                    500..=599 => error_codes::TRANSIENT,
                    _         => error_codes::REMOTE_ERROR,
                };
                return PluginError {
                    code: code.to_string(),
                    message: format!("Kitsu HTTP {status}: {body}"),
                };
            }
        }
    }
    PluginError { code: error_codes::TRANSIENT.to_string(), message: err.to_string() }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, PluginError> {
    serde_json::from_str(body).map_err(|e| {
        plugin_error!("kitsu: parse error: {}", e);
        PluginError { code: "parse_error".to_string(), message: format!("Kitsu JSON parse failure: {e}") }
    })
}

/// Bearer-token HTTP helper.
///
/// Justification for keeping this inline rather than promoting to an SDK
/// helper: Kitsu is the only bundled plugin that wants bearer auth today,
/// and the call path is exercised only when the user configures an optional
/// API key. Promoting to `sdk::host::BearerAuth` would widen the plugin ABI
/// without another consumer to shape it; when a second caller arrives we
/// can lift this verbatim.
#[cfg(target_arch = "wasm32")]
fn http_get_with_bearer(url: &str, token: &str) -> Result<String, String> {
    let payload = serde_json::json!({
        "url": url,
        "body": "",
        "__stui_headers": { "Authorization": format!("Bearer {token}") },
    })
    .to_string();

    #[link(wasm_import_module = "stui")]
    extern "C" {
        fn stui_http_post(ptr: *const u8, len: i32) -> i64;
        fn stui_free(ptr: i32, len: i32);
    }

    let packed = unsafe { stui_http_post(payload.as_ptr(), payload.len() as i32) };
    if packed == 0 {
        return Err("http request failed".into());
    }
    let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
    let len = (packed & 0xFFFFFFFF) as usize;
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    let json = std::str::from_utf8(slice).map(String::from);
    unsafe { stui_free(ptr as i32, len as i32) };
    let json = json.map_err(|e| e.to_string())?;

    #[derive(Deserialize)]
    struct R { status: u16, body: String }

    let resp: R = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    if (200..300).contains(&resp.status) {
        Ok(resp.body)
    } else {
        Err(format!("HTTP {}: {}", resp.status, resp.body))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn http_get_with_bearer(_url: &str, _token: &str) -> Result<String, String> {
    Err("http_get_with_bearer only available in WASM context".into())
}

/// Fetch a URL; use bearer auth when an api_key is configured, plain GET otherwise.
fn fetch(url: &str) -> Result<String, PluginError> {
    let api_key = cache_get("__config:api_key")
        .filter(|k| !k.is_empty())
        .or_else(|| cache_get("__env:KITSU_API_KEY").filter(|k| !k.is_empty()));

    let raw = match api_key {
        Some(key) => http_get_with_bearer(url, &key),
        None      => http_get(url),
    };
    raw.map_err(|e| classify_http_err(&e))
}

/// Upstream Kitsu `showType` values for each scope. Movie → `["movie"]`;
/// Series → the TV/OVA/ONA/special set. Used both to filter requests
/// server-side and — for endpoints that don't accept the filter (trending) —
/// to post-filter the response.
fn show_types_for_scope(scope: SearchScope) -> Result<&'static [&'static str], PluginError> {
    match scope {
        SearchScope::Movie  => Ok(&["movie"]),
        SearchScope::Series => Ok(&["TV", "OVA", "ONA", "special"]),
        _ => Err(PluginError {
            code: error_codes::UNSUPPORTED_SCOPE.to_string(),
            message: "kitsu only supports movie and series scopes".to_string(),
        }),
    }
}

fn show_type_matches(anime: &Anime, wanted: &[&str]) -> bool {
    match anime.attributes.show_type.as_deref() {
        Some(t) => wanted.iter().any(|w| w.eq_ignore_ascii_case(t)),
        None    => true, // Kitsu sometimes omits show_type; keep rather than drop.
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for KitsuPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let entry_kind = match req.scope {
            SearchScope::Movie  => EntryKind::Movie,
            SearchScope::Series => EntryKind::Series,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "kitsu only supports movie and series scopes",
                );
            }
        };
        let wanted_types = match show_types_for_scope(req.scope) {
            Ok(t) => t,
            Err(e) => return PluginResult::Err(e),
        };

        let query = req.query.trim();
        let page     = req.page.max(1);
        let per_page = if req.limit == 0 { 20 } else { req.limit.min(20).max(1) as u32 };
        let offset   = (page - 1).saturating_mul(per_page);

        let url = if query.is_empty() && page == 1 {
            // `/trending/anime` has no filter; we post-filter by show_type after.
            format!("{API_BASE}/trending/anime?limit={per_page}")
        } else if query.is_empty() {
            // Page>1: fall back to sort=-userCount with the format filter applied.
            format!(
                "{API_BASE}/anime?page[limit]={per_page}&page[offset]={offset}&sort=-userCount&filter[subtype]={}",
                wanted_types.join(","),
            )
        } else {
            format!(
                "{API_BASE}/anime?filter[text]={}&page[limit]={per_page}&page[offset]={offset}&filter[subtype]={}",
                urlencoding::encode(query),
                wanted_types.join(","),
            )
        };
        plugin_info!("kitsu: search '{}' (scope={:?}, page={})", query, req.scope, page);

        let body = match fetch(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };
        let resp: AnimeResponse = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };

        // Trending doesn't support the filter param — drop entries whose
        // showType doesn't match the requested scope. For the filtered paths
        // this loop is effectively a no-op (every entry already matches).
        let items: Vec<PluginEntry> = resp
            .data
            .into_iter()
            .filter(|a| show_type_matches(a, wanted_types))
            .map(|a| a.into_entry(entry_kind))
            .collect();
        let total = resp.meta.and_then(|m| m.count).unwrap_or(items.len() as u32);
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        let entry_kind = match req.kind {
            EntryKind::Movie => EntryKind::Movie,
            _                => EntryKind::Series,
        };

        let url = match req.id_source.as_str() {
            id_sources::KITSU => {
                format!("{API_BASE}/anime/{}", urlencoding::encode(&req.id))
            }
            id_sources::MYANIMELIST => {
                // Kitsu stores cross-mappings under /mappings; filter by
                // externalSite + externalId then use `include=item` to fold
                // the target anime directly into `included[]`.
                format!(
                    "{API_BASE}/mappings?filter[externalSite]=myanimelist/anime&filter[externalId]={}&include=item",
                    urlencoding::encode(&req.id),
                )
            }
            other => return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("unsupported id_source: {other}"),
            ),
        };
        plugin_info!("kitsu: lookup id_source={} id={}", req.id_source, req.id);

        let body = match fetch(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };

        // Kitsu returns either a single-anime envelope (`data: {...}`) or a
        // list envelope (`data: [...]`). SingleOrManyResponse accepts both.
        let shape: SingleOrManyResponse<Anime, Anime> = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        let anime = match (req.id_source.as_str(), shape) {
            (id_sources::KITSU, SingleOrManyResponse::One { data }) => data,
            (id_sources::KITSU, SingleOrManyResponse::Many { .. }) => {
                return PluginResult::err(error_codes::REMOTE_ERROR, "kitsu: unexpected list shape for direct anime lookup");
            }
            (id_sources::MYANIMELIST, SingleOrManyResponse::Many { included, .. }) => {
                match included.into_iter().find(|a| a.kind == "anime") {
                    Some(a) => a,
                    None => return PluginResult::err(
                        error_codes::UNKNOWN_ID,
                        format!("kitsu: no anime for myanimelist id {}", req.id),
                    ),
                }
            }
            _ => return PluginResult::err(error_codes::UNKNOWN_ID, "kitsu: lookup returned no results"),
        };
        PluginResult::ok(LookupResponse { entry: anime.into_entry(entry_kind) })
    }
}

// ── API types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AnimeResponse {
    data: Vec<Anime>,
    meta: Option<AnimeMeta>,
}

// Accept both `{ data: {...} }` (single-lookup) and
// `{ data: [...], included: [...] }` (mappings lookup) shapes. `#[serde(default)]`
// on these Vec fields would force a `U: Default` bound at derive time, so we
// leave them out and rely on serde-json's native absence→empty handling.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SingleOrManyResponse<T, U> {
    One  { data: T },
    Many { data: Vec<U>, included: Vec<Anime> },
}

#[derive(Debug, Deserialize)]
struct AnimeMeta {
    count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Anime {
    id: String,
    #[serde(rename = "type", default)]
    kind: String,
    attributes: AnimeAttributes,
}

#[derive(Debug, Deserialize)]
struct AnimeAttributes {
    #[serde(rename = "canonicalTitle", default)]
    title: String,
    #[serde(rename = "synopsis",  default)] synopsis:   Option<String>,
    #[serde(rename = "averageRating", default)] rating: Option<String>,
    #[serde(rename = "startDate", default)] start_date: Option<String>,
    #[serde(rename = "episodeLength", default)] episode_length: Option<u32>,
    #[serde(rename = "posterImage", default)] poster: Option<Image>,
    #[serde(rename = "showType", default)]  show_type:  Option<String>,
}

#[derive(Debug, Deserialize)]
struct Image {
    small:    Option<String>,
    large:    Option<String>,
    original: Option<String>,
}

impl Anime {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let attrs = self.attributes;
        let year = attrs.start_date.as_deref()
            .and_then(|d| d.split('-').next())
            .and_then(|y| y.parse::<u32>().ok());
        let rating = attrs.rating.as_deref()
            .and_then(|r| r.parse::<f32>().ok())
            .map(|r| r / 10.0);
        let poster_url = attrs.poster.as_ref()
            .and_then(|p| p.large.clone().or(p.original.clone()).or(p.small.clone()));

        let mut entry = PluginEntry {
            id: format!("kitsu-{}", self.id),
            kind,
            source: "kitsu".to_string(),
            title: attrs.title,
            year,
            rating,
            description: attrs.synopsis,
            poster_url,
            duration: attrs.episode_length,
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::KITSU.to_string(), self.id);
        entry
    }
}

// ── WASM exports ──────────────────────────────────────────────────────────────

stui_export_catalog_plugin!(KitsuPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn _p<T: Plugin>() {}
        fn _c<T: CatalogPlugin>() {}
        _p::<KitsuPlugin>();
        _c::<KitsuPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = KitsuPlugin::new();
        assert_eq!(p.manifest().plugin.name, "kitsu");
    }

    #[test]
    fn scope_movie_maps_to_movie_show_type() {
        let types = show_types_for_scope(SearchScope::Movie).unwrap();
        assert_eq!(types, &["movie"]);
    }

    #[test]
    fn scope_series_covers_tv_and_adjacent_formats() {
        let types = show_types_for_scope(SearchScope::Series).unwrap();
        assert!(types.contains(&"TV"));
        assert!(types.contains(&"OVA"));
        assert!(types.contains(&"ONA"));
        assert!(types.contains(&"special"));
    }

    #[test]
    fn scope_track_rejects() {
        assert!(show_types_for_scope(SearchScope::Track).is_err());
    }

    #[test]
    fn show_type_matcher_case_insensitive() {
        let mut a = make_anime("1", "t", Some("tv".into()));
        assert!(show_type_matches(&a, &["TV"]));
        a.attributes.show_type = Some("Movie".into());
        assert!(show_type_matches(&a, &["movie"]));
    }

    #[test]
    fn show_type_matcher_keeps_unknown_entries() {
        // If upstream omits show_type, don't filter aggressively.
        let a = make_anime("1", "t", None);
        assert!(show_type_matches(&a, &["movie"]));
    }

    #[test]
    fn anime_into_entry_scales_rating_and_parses_year() {
        let a = make_anime_full();
        let e = a.into_entry(EntryKind::Series);
        assert_eq!(e.source, "kitsu");
        assert_eq!(e.title, "Cowboy Bebop");
        assert_eq!(e.year, Some(1998));
        assert_eq!(e.rating, Some(8.6));   // 86 / 10
        assert_eq!(e.duration, Some(24));
        assert_eq!(e.poster_url.as_deref(), Some("large.jpg"));
        assert_eq!(e.external_ids.get(id_sources::KITSU).map(String::as_str), Some("1"));
    }

    fn make_anime(id: &str, title: &str, show_type: Option<String>) -> Anime {
        Anime {
            id: id.into(),
            kind: "anime".into(),
            attributes: AnimeAttributes {
                title: title.into(),
                synopsis: None,
                rating: None,
                start_date: None,
                episode_length: None,
                poster: None,
                show_type,
            },
        }
    }

    fn make_anime_full() -> Anime {
        Anime {
            id: "1".into(),
            kind: "anime".into(),
            attributes: AnimeAttributes {
                title: "Cowboy Bebop".into(),
                synopsis: Some("..".into()),
                rating: Some("86.0".into()),
                start_date: Some("1998-04-03".into()),
                episode_length: Some(24),
                poster: Some(Image {
                    small: Some("s.jpg".into()),
                    large: Some("large.jpg".into()),
                    original: None,
                }),
                show_type: Some("TV".into()),
            },
        }
    }
}
