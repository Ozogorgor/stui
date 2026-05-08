//! Kitsu metadata provider — anime movies and series via the Kitsu JSON:API.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup, episodes}`. API
//! key is optional — Kitsu's public endpoints allow unauthenticated
//! requests with generous rate limits.

use serde::Deserialize;

use stui_plugin_sdk::{
    cache_get, error_codes, http_get, id_sources, parse_manifest, plugin_error, plugin_info,
    stui_export_catalog_plugin, CatalogPlugin, EntryKind, EpisodeWire, EpisodesRequest,
    EpisodesResponse, InitContext, LookupRequest, LookupResponse, Plugin, PluginEntry, PluginError,
    PluginInitError, PluginManifest, PluginResult, SearchRequest, SearchResponse, SearchScope,
};

const API_BASE: &str = "https://kitsu.io/api/edge";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct KitsuPlugin {
    manifest: PluginManifest,
}

impl KitsuPlugin {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self { manifest }
    }
}

impl Default for KitsuPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for KitsuPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

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
                    404 => error_codes::UNKNOWN_ID,
                    429 => error_codes::RATE_LIMITED,
                    500..=599 => error_codes::TRANSIENT,
                    _ => error_codes::REMOTE_ERROR,
                };
                return PluginError {
                    code: code.to_string(),
                    message: format!("Kitsu HTTP {status}: {body}"),
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
        plugin_error!("kitsu: parse error: {}", e);
        PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("Kitsu JSON parse failure: {e}"),
        }
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
    }

    let packed = unsafe { stui_http_post(payload.as_ptr(), payload.len() as i32) };
    if packed == 0 {
        return Err("http request failed".into());
    }
    let ptr = ((packed >> 32) & 0xFFFFFFFF) as *const u8;
    let len = (packed & 0xFFFFFFFF) as usize;
    // Host writes into plugin-owned linear memory; the SDK's sibling HTTP
    // helpers don't manually free either — the allocation is reclaimed on
    // the next `stui_alloc` cycle or when the module unloads.
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    let json = std::str::from_utf8(slice)
        .map(String::from)
        .map_err(|e| e.to_string())?;

    #[derive(Deserialize)]
    struct R {
        status: u16,
        body: String,
    }

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
        None => http_get(url),
    };
    raw.map_err(|e| classify_http_err(&e))
}

/// Upstream Kitsu `showType` values for each scope. Movie → `["movie"]`;
/// Series → the TV/OVA/ONA/special set. Used both to filter requests
/// server-side and — for endpoints that don't accept the filter (trending) —
/// to post-filter the response.
fn show_types_for_scope(scope: SearchScope) -> Result<&'static [&'static str], PluginError> {
    match scope {
        SearchScope::Movie => Ok(&["movie"]),
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
        None => true, // Kitsu sometimes omits show_type; keep rather than drop.
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for KitsuPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let entry_kind = match req.scope {
            SearchScope::Movie => EntryKind::Movie,
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
        let page = req.page.max(1);
        let per_page = if req.limit == 0 {
            20
        } else {
            req.limit.min(20).max(1) as u32
        };
        let offset = (page - 1).saturating_mul(per_page);

        let url = if query.is_empty() && page == 1 {
            // `/trending/anime` has no filter; we post-filter by show_type after.
            format!("{API_BASE}/trending/anime?limit={per_page}&include=mappings")
        } else if query.is_empty() {
            // Page>1: fall back to sort=-userCount with the format filter applied.
            format!(
                "{API_BASE}/anime?page[limit]={per_page}&page[offset]={offset}&sort=-userCount&filter[subtype]={}&include=mappings",
                wanted_types.join(","),
            )
        } else {
            format!(
                "{API_BASE}/anime?filter[text]={}&page[limit]={per_page}&page[offset]={offset}&filter[subtype]={}&include=mappings",
                urlencoding::encode(query),
                wanted_types.join(","),
            )
        };
        plugin_info!(
            "kitsu: search '{}' (scope={:?}, page={})",
            query,
            req.scope,
            page
        );

        let body = match fetch(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };
        let resp: AnimeResponse = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };

        // Build mapping_id → MAL externalId for the rows in `included[]`.
        // Kitsu wraps every cross-site link as a Mapping row keyed by site;
        // we filter to MyAnimeList anime entries and drop the rest.
        let mal_by_mapping_id: std::collections::HashMap<&str, &str> = resp
            .included
            .iter()
            .filter(|m| m.kind == "mappings")
            .filter(|m| m.attributes.external_site == "myanimelist/anime")
            .filter(|m| !m.id.is_empty() && !m.attributes.external_id.is_empty())
            .map(|m| (m.id.as_str(), m.attributes.external_id.as_str()))
            .collect();

        // Walk each anime's `relationships.mappings.data[]` list to find
        // the first mapping that resolves to a MAL row. Build a parallel
        // anime_id → mal_id map keyed by the ids actually present in `data[]`.
        let mal_by_anime: std::collections::HashMap<String, String> = resp
            .data
            .iter()
            .filter_map(|a| {
                let refs = a.relationships.as_ref()?.mappings.as_ref()?;
                let mal = refs
                    .data
                    .iter()
                    .find_map(|r| mal_by_mapping_id.get(r.id.as_str()).copied())?;
                Some((a.id.clone(), mal.to_string()))
            })
            .collect();

        // Trending doesn't support the filter param — drop entries whose
        // showType doesn't match the requested scope. For the filtered paths
        // this loop is effectively a no-op (every entry already matches).
        let items: Vec<PluginEntry> = resp
            .data
            .into_iter()
            .filter(|a| show_type_matches(a, wanted_types))
            .map(|a| {
                let mal = mal_by_anime.get(&a.id).cloned();
                a.into_entry(entry_kind, mal)
            })
            .collect();
        let total = resp
            .meta
            .and_then(|m| m.count)
            .unwrap_or(items.len() as u32);
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        let entry_kind = match req.kind {
            EntryKind::Movie => EntryKind::Movie,
            _ => EntryKind::Series,
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
            other => {
                return PluginResult::err(
                    error_codes::UNKNOWN_ID,
                    format!("unsupported id_source: {other}"),
                )
            }
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
                return PluginResult::err(
                    error_codes::REMOTE_ERROR,
                    "kitsu: unexpected list shape for direct anime lookup",
                );
            }
            (id_sources::MYANIMELIST, SingleOrManyResponse::Many { included, .. }) => {
                match included.into_iter().find(|a| a.kind == "anime") {
                    Some(a) => a,
                    None => {
                        return PluginResult::err(
                            error_codes::UNKNOWN_ID,
                            format!("kitsu: no anime for myanimelist id {}", req.id),
                        )
                    }
                }
            }
            _ => {
                return PluginResult::err(
                    error_codes::UNKNOWN_ID,
                    "kitsu: lookup returned no results",
                )
            }
        };
        // Preserve the input MAL id when lookup was invoked with id_source=myanimelist.
        // The /mappings response carries the externalId we already used to query, so
        // round-trip it into external_ids so downstream dedup sees the cross-link.
        let mal_for_entry = if req.id_source == id_sources::MYANIMELIST {
            Some(req.id.clone())
        } else {
            None
        };
        PluginResult::ok(LookupResponse {
            entry: anime.into_entry(entry_kind, mal_for_entry),
        })
    }

    fn episodes(&self, req: EpisodesRequest) -> PluginResult<EpisodesResponse> {
        if req.id_source != id_sources::KITSU {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!(
                    "kitsu episodes only supports kitsu id_source, got: {}",
                    req.id_source
                ),
            );
        }
        if req.season < 1 {
            return PluginResult::err(
                error_codes::INVALID_REQUEST,
                "kitsu episodes: season must be >= 1",
            );
        }
        plugin_info!("kitsu: episodes id={} season={}", req.series_id, req.season);

        // Kitsu paginates with a 20-item hard cap per page (`page[limit]`),
        // so we walk pages until either the response is short (= last page)
        // or we hit the hard cap. The cap protects against runaway shows
        // (One Piece-class) silently ballooning into 50+ round trips and
        // tying up the supervisor's per-call timeout.
        let mut all: Vec<KitsuEpisode> = Vec::new();
        for page in 0..MAX_EPISODE_PAGES {
            let offset = page * EPISODES_PER_PAGE;
            let url = format!(
                "{API_BASE}/anime/{}/episodes?page[limit]={}&page[offset]={}&sort=number",
                urlencoding::encode(&req.series_id),
                EPISODES_PER_PAGE,
                offset,
            );
            let body = match fetch(&url) {
                Ok(b) => b,
                Err(e) => return PluginResult::Err(e),
            };
            let resp: KitsuEpisodesResponse = match parse_json(&body) {
                Ok(r) => r,
                Err(e) => return PluginResult::Err(e),
            };
            let n = resp.data.len() as u32;
            all.extend(resp.data);
            if n < EPISODES_PER_PAGE {
                break;
            }
        }

        let episodes = build_episodes(req.season, all);
        PluginResult::ok(EpisodesResponse { episodes })
    }
}

// ── Episodes builder ──────────────────────────────────────────────────────────

/// Kitsu paginates `/anime/{id}/episodes` 20 items at a time (the API
/// hard-caps `page[limit]`). 25 pages = 500 episodes — enough for every
/// anime except long-running shounen, where the sub-card UX would be
/// useless anyway and a saner per-cour split is the right answer.
const EPISODES_PER_PAGE: u32 = 20;
const MAX_EPISODE_PAGES: u32 = 25;

/// Convert raw Kitsu episode rows into the wire shape the runtime emits.
///
/// Kitsu treats one anime entry as encompassing every cour, distinguishing
/// them via `seasonNumber`. We filter to the requested season — for the
/// vast majority of anime this is a no-op (Kitsu omits `seasonNumber` or
/// sets it to 1 for the whole series). Multi-season titles (rare on
/// Kitsu) honour the filter.
///
/// Episode numbering uses `relativeNumber` first (per-season index) and
/// falls back to the series-wide `number` when relative is absent — the
/// fallback matches what the canonical TV episode browsers expect when
/// `seasonNumber` is single.
fn build_episodes(requested_season: u32, raw: Vec<KitsuEpisode>) -> Vec<EpisodeWire> {
    raw.into_iter()
        .filter(|e| e.attributes.season_number.unwrap_or(1) == requested_season)
        .map(|e| {
            let n = e
                .attributes
                .relative_number
                .or(e.attributes.number)
                .unwrap_or(0);
            let title = e
                .attributes
                .canonical_title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .unwrap_or_else(|| format!("Episode {n}"));
            EpisodeWire {
                season: requested_season,
                episode: n,
                title,
                air_date: e.attributes.airdate,
                runtime_mins: e.attributes.length,
                provider: "kitsu".to_string(),
                entry_id: format!("kitsu-{}", e.id),
            }
        })
        .collect()
}

// ── API types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AnimeResponse {
    data: Vec<Anime>,
    meta: Option<AnimeMeta>,
    /// Kitsu's `?include=mappings` folds related Mapping rows here.
    /// Empty for endpoints we don't ask `include` of (lookup); empty
    /// when no anime in the page has cross-mappings (rare).
    ///
    /// Note (verified against live Kitsu API 2026-04-26): the back-pointer
    /// from a mapping row to its parent anime does NOT live on
    /// `included[].relationships.item.data.id`. Kitsu's JSON:API only
    /// emits `relationships.item.links` on the included mapping rows.
    /// The actual association lives on the parent side:
    /// `data[].relationships.mappings.data[]` lists the mapping ids
    /// owned by each anime. We build the index from that direction.
    #[serde(default)]
    included: Vec<Mapping>,
}

// Accept both `{ data: {...} }` (single-lookup) and
// `{ data: [...], included: [...] }` (mappings lookup) shapes. `#[serde(default)]`
// on these Vec fields would force a `U: Default` bound at derive time, so we
// leave them out and rely on serde-json's native absence→empty handling.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SingleOrManyResponse<T, U> {
    One { data: T },
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
    /// Present on search responses (with `?include=mappings`); absent on
    /// /anime/{id} lookups. The forward pointer to mapping ids in `included[]`.
    #[serde(default)]
    relationships: Option<AnimeRelationships>,
}

/// Captures only the `mappings` relationship — the rest of an anime's
/// relationships (genres, characters, etc.) are unused.
#[derive(Debug, Deserialize)]
struct AnimeRelationships {
    #[serde(default)]
    mappings: Option<RelationshipList>,
}

#[derive(Debug, Deserialize)]
struct RelationshipList {
    #[serde(default)]
    data: Vec<RelationshipRef>,
}

#[derive(Debug, Deserialize)]
struct RelationshipRef {
    #[serde(default)]
    id: String,
}

/// One row inside `included[]` when search is called with `?include=mappings`.
/// We extract only the `myanimelist/anime` site; other sites are silently
/// ignored. The parent anime back-link is established via
/// `data[].relationships.mappings.data[]` rather than this row's
/// relationships (Kitsu only emits `links` for `relationships.item` here).
#[derive(Debug, Deserialize)]
struct Mapping {
    #[serde(default)]
    id: String,
    #[serde(default, rename = "type")]
    kind: String,
    attributes: MappingAttributes,
}

#[derive(Debug, Deserialize)]
struct MappingAttributes {
    #[serde(default, rename = "externalSite")]
    external_site: String,
    #[serde(default, rename = "externalId")]
    external_id: String,
}

#[derive(Debug, Deserialize)]
struct AnimeAttributes {
    #[serde(rename = "canonicalTitle", default)]
    title: String,
    #[serde(rename = "synopsis", default)]
    synopsis: Option<String>,
    #[serde(rename = "averageRating", default)]
    rating: Option<String>,
    #[serde(rename = "startDate", default)]
    start_date: Option<String>,
    #[serde(rename = "episodeLength", default)]
    episode_length: Option<u32>,
    #[serde(rename = "posterImage", default)]
    poster: Option<Image>,
    #[serde(rename = "showType", default)]
    show_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Image {
    small: Option<String>,
    large: Option<String>,
    original: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KitsuEpisodesResponse {
    #[serde(default)]
    data: Vec<KitsuEpisode>,
}

#[derive(Debug, Deserialize)]
struct KitsuEpisode {
    id: String,
    attributes: KitsuEpisodeAttributes,
}

#[derive(Debug, Deserialize, Default)]
struct KitsuEpisodeAttributes {
    #[serde(rename = "canonicalTitle", default)]
    canonical_title: Option<String>,
    #[serde(rename = "seasonNumber", default)]
    season_number: Option<u32>,
    #[serde(default)]
    number: Option<u32>,
    #[serde(rename = "relativeNumber", default)]
    relative_number: Option<u32>,
    #[serde(default)]
    airdate: Option<String>,
    #[serde(default)]
    length: Option<u32>,
}

impl Anime {
    fn into_entry(self, kind: EntryKind, mal_id: Option<String>) -> PluginEntry {
        let attrs = self.attributes;
        let year = attrs
            .start_date
            .as_deref()
            .and_then(|d| d.split('-').next())
            .and_then(|y| y.parse::<u32>().ok());
        let rating = attrs
            .rating
            .as_deref()
            .and_then(|r| r.parse::<f32>().ok())
            .map(|r| r / 10.0);
        let poster_url = attrs
            .poster
            .as_ref()
            .and_then(|p| p.large.clone().or(p.original.clone()).or(p.small.clone()));

        let mut entry = PluginEntry {
            id: format!("kitsu-{}", self.id),
            kind,
            source: "kitsu".to_string(),
            title: attrs.title,
            year,
            rating,
            // Kitsu synopses are mostly plain text, but a subset still
            // ship with `(Source: …)` attribution and stray HTML — apply
            // the same cleanup AniList uses so descriptions stay
            // consistent across providers.
            description: attrs
                .synopsis
                .as_deref()
                .map(stui_plugin_sdk::clean_description),
            poster_url,
            duration: attrs.episode_length,
            ..Default::default()
        };
        entry
            .external_ids
            .insert(id_sources::KITSU.to_string(), self.id);
        if let Some(mal) = mal_id.filter(|s| !s.is_empty()) {
            entry
                .external_ids
                .insert(id_sources::MYANIMELIST.to_string(), mal);
        }
        entry
    }
}

// ── WASM exports ──────────────────────────────────────────────────────────────

impl stui_plugin_sdk::StreamProvider for KitsuPlugin {}

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
        let e = a.into_entry(EntryKind::Series, None);
        assert_eq!(e.source, "kitsu");
        assert_eq!(e.title, "Cowboy Bebop");
        assert_eq!(e.year, Some(1998));
        assert_eq!(e.rating, Some(8.6)); // 86 / 10
        assert_eq!(e.duration, Some(24));
        assert_eq!(e.poster_url.as_deref(), Some("large.jpg"));
        assert_eq!(
            e.external_ids.get(id_sources::KITSU).map(String::as_str),
            Some("1")
        );
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
            relationships: None,
        }
    }

    #[test]
    fn build_episodes_filters_to_requested_season() {
        // Build twice: KitsuEpisode is intentionally non-Clone (matches the
        // wire types' non-Clone shape in the rest of the file), so we build
        // a fresh Vec for each call instead of cloning.
        let raw1 = vec![
            ep(
                "1",
                "S1E1",
                Some(1),
                Some(1),
                Some(1),
                Some("2013-04-07".into()),
                Some(24),
            ),
            ep("2", "S1E2", Some(1), Some(2), Some(2), None, Some(24)),
            ep("3", "S2E1", Some(2), Some(26), Some(1), None, Some(24)),
        ];
        let raw2 = vec![
            ep(
                "1",
                "S1E1",
                Some(1),
                Some(1),
                Some(1),
                Some("2013-04-07".into()),
                Some(24),
            ),
            ep("2", "S1E2", Some(1), Some(2), Some(2), None, Some(24)),
            ep("3", "S2E1", Some(2), Some(26), Some(1), None, Some(24)),
        ];
        let s1 = build_episodes(1, raw1);
        assert_eq!(s1.len(), 2);
        assert!(s1.iter().all(|e| e.season == 1 && e.provider == "kitsu"));
        let s2 = build_episodes(2, raw2);
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0].episode, 1); // relativeNumber, not series-wide
    }

    #[test]
    fn build_episodes_treats_missing_season_as_one() {
        let raw1 = vec![ep("1", "Pilot", None, Some(1), Some(1), None, None)];
        let raw2 = vec![ep("1", "Pilot", None, Some(1), Some(1), None, None)];
        assert_eq!(build_episodes(1, raw1).len(), 1);
        assert!(build_episodes(2, raw2).is_empty());
    }

    #[test]
    fn build_episodes_falls_back_to_number_when_relative_missing() {
        let raw = vec![ep("1", "Solo", Some(1), Some(7), None, None, None)];
        let eps = build_episodes(1, raw);
        assert_eq!(eps[0].episode, 7);
    }

    #[test]
    fn build_episodes_falls_back_to_episode_n_for_missing_titles() {
        let raw = vec![
            ep("1", "  ", Some(1), Some(1), Some(1), None, None),
            ep_no_title("2", Some(1), Some(2), Some(2)),
        ];
        let eps = build_episodes(1, raw);
        assert_eq!(eps[0].title, "Episode 1");
        assert_eq!(eps[1].title, "Episode 2");
    }

    #[test]
    fn build_episodes_uses_kitsu_episode_id_for_entry_id() {
        let raw = vec![ep("987", "Title", Some(1), Some(1), Some(1), None, None)];
        let eps = build_episodes(1, raw);
        assert_eq!(eps[0].entry_id, "kitsu-987");
    }

    #[test]
    fn build_episodes_propagates_airdate_and_runtime() {
        let raw = vec![ep(
            "1",
            "T",
            Some(1),
            Some(1),
            Some(1),
            Some("2024-01-01".into()),
            Some(23),
        )];
        let eps = build_episodes(1, raw);
        assert_eq!(eps[0].air_date.as_deref(), Some("2024-01-01"));
        assert_eq!(eps[0].runtime_mins, Some(23));
    }

    #[test]
    fn episodes_rejects_non_kitsu_id_source() {
        let p = KitsuPlugin::new();
        let req = EpisodesRequest {
            series_id: "1".into(),
            id_source: "anilist".into(),
            season: 1,
        };
        match p.episodes(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            PluginResult::Ok(_) => panic!("expected UNKNOWN_ID rejection"),
        }
    }

    #[test]
    fn episodes_rejects_zero_season() {
        let p = KitsuPlugin::new();
        let req = EpisodesRequest {
            series_id: "1".into(),
            id_source: id_sources::KITSU.to_string(),
            season: 0,
        };
        match p.episodes(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::INVALID_REQUEST),
            PluginResult::Ok(_) => panic!("expected INVALID_REQUEST rejection"),
        }
    }

    fn ep(
        id: &str,
        title: &str,
        season_number: Option<u32>,
        number: Option<u32>,
        relative_number: Option<u32>,
        airdate: Option<String>,
        length: Option<u32>,
    ) -> KitsuEpisode {
        KitsuEpisode {
            id: id.into(),
            attributes: KitsuEpisodeAttributes {
                canonical_title: Some(title.into()),
                season_number,
                number,
                relative_number,
                airdate,
                length,
            },
        }
    }

    fn ep_no_title(
        id: &str,
        season_number: Option<u32>,
        number: Option<u32>,
        relative_number: Option<u32>,
    ) -> KitsuEpisode {
        KitsuEpisode {
            id: id.into(),
            attributes: KitsuEpisodeAttributes {
                canonical_title: None,
                season_number,
                number,
                relative_number,
                airdate: None,
                length: None,
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
            relationships: None,
        }
    }

    /// Trimmed real Kitsu `?include=mappings` response (captured 2026-04-26
    /// from `/anime?filter[text]=cowboy+bebop&page[limit]=2&include=mappings`).
    /// The relationship back-pointer lives on the parent side
    /// (`data[].relationships.mappings.data[]`) — included mapping rows
    /// only carry `relationships.item.links`, so we walk the parent list
    /// to attribute mappings to anime ids.
    #[test]
    fn kitsu_search_response_parses_included_mappings() {
        let raw = r#"{
            "data": [
                {
                    "id": "1",
                    "type": "anime",
                    "attributes": {
                        "canonicalTitle": "Cowboy Bebop",
                        "showType": "TV",
                        "synopsis": null,
                        "averageRating": null,
                        "startDate": null,
                        "episodeLength": null,
                        "posterImage": null
                    },
                    "relationships": {
                        "mappings": {
                            "data": [
                                { "type": "mappings", "id": "100" },
                                { "type": "mappings", "id": "101" }
                            ]
                        }
                    }
                },
                {
                    "id": "2",
                    "type": "anime",
                    "attributes": {
                        "canonicalTitle": "Trigun",
                        "showType": "TV",
                        "synopsis": null,
                        "averageRating": null,
                        "startDate": null,
                        "episodeLength": null,
                        "posterImage": null
                    },
                    "relationships": {
                        "mappings": {
                            "data": [
                                { "type": "mappings", "id": "102" },
                                { "type": "mappings", "id": "103" }
                            ]
                        }
                    }
                }
            ],
            "included": [
                {
                    "id": "100",
                    "type": "mappings",
                    "attributes": { "externalSite": "myanimelist/anime", "externalId": "1" },
                    "relationships": { "item": { "links": { "self": "https://kitsu.io/api/edge/mappings/100/relationships/item", "related": "https://kitsu.io/api/edge/mappings/100/item" } } }
                },
                {
                    "id": "101",
                    "type": "mappings",
                    "attributes": { "externalSite": "anidb", "externalId": "23" },
                    "relationships": { "item": { "links": { "self": "https://kitsu.io/api/edge/mappings/101/relationships/item", "related": "https://kitsu.io/api/edge/mappings/101/item" } } }
                },
                {
                    "id": "102",
                    "type": "mappings",
                    "attributes": { "externalSite": "myanimelist/anime", "externalId": "6" },
                    "relationships": { "item": { "links": { "self": "https://kitsu.io/api/edge/mappings/102/relationships/item", "related": "https://kitsu.io/api/edge/mappings/102/item" } } }
                },
                {
                    "id": "103",
                    "type": "mappings",
                    "attributes": { "externalSite": "anidb", "externalId": "45" },
                    "relationships": { "item": { "links": { "self": "https://kitsu.io/api/edge/mappings/103/relationships/item", "related": "https://kitsu.io/api/edge/mappings/103/item" } } }
                }
            ]
        }"#;
        let resp: AnimeResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.included.len(), 4);

        let mal_by_mapping_id: std::collections::HashMap<&str, &str> = resp
            .included
            .iter()
            .filter(|m| m.kind == "mappings")
            .filter(|m| m.attributes.external_site == "myanimelist/anime")
            .map(|m| (m.id.as_str(), m.attributes.external_id.as_str()))
            .collect();

        let mal_by_anime: std::collections::HashMap<String, String> = resp
            .data
            .iter()
            .filter_map(|a| {
                let refs = a.relationships.as_ref()?.mappings.as_ref()?;
                let mal = refs
                    .data
                    .iter()
                    .find_map(|r| mal_by_mapping_id.get(r.id.as_str()).copied())?;
                Some((a.id.clone(), mal.to_string()))
            })
            .collect();

        assert_eq!(mal_by_anime.len(), 2);
        assert_eq!(mal_by_anime.get("1").map(String::as_str), Some("1"));
        assert_eq!(mal_by_anime.get("2").map(String::as_str), Some("6"));
    }

    /// Mapping rows for anime ids that aren't in `data[]` (Kitsu sometimes
    /// folds extras when paginating sparse responses) must not appear in
    /// the index — but the per-anime walk side-steps the issue entirely
    /// because it's keyed off `data[]` ids, not the mapping rows. This
    /// test pins that invariant.
    #[test]
    fn kitsu_search_response_drops_orphan_mappings() {
        let raw = r#"{
            "data": [
                {
                    "id": "1",
                    "type": "anime",
                    "attributes": {
                        "canonicalTitle": "Real Anime",
                        "showType": "TV",
                        "synopsis": null,
                        "averageRating": null,
                        "startDate": null,
                        "episodeLength": null,
                        "posterImage": null
                    },
                    "relationships": {
                        "mappings": {
                            "data": [
                                { "type": "mappings", "id": "100" }
                            ]
                        }
                    }
                }
            ],
            "included": [
                {
                    "id": "100",
                    "type": "mappings",
                    "attributes": { "externalSite": "myanimelist/anime", "externalId": "5" },
                    "relationships": { "item": { "links": { "self": "x", "related": "y" } } }
                },
                {
                    "id": "101",
                    "type": "mappings",
                    "attributes": { "externalSite": "myanimelist/anime", "externalId": "999" },
                    "relationships": { "item": { "links": { "self": "x", "related": "y" } } }
                }
            ]
        }"#;
        let resp: AnimeResponse = serde_json::from_str(raw).unwrap();

        let mal_by_mapping_id: std::collections::HashMap<&str, &str> = resp
            .included
            .iter()
            .filter(|m| m.kind == "mappings")
            .filter(|m| m.attributes.external_site == "myanimelist/anime")
            .map(|m| (m.id.as_str(), m.attributes.external_id.as_str()))
            .collect();

        let mal_by_anime: std::collections::HashMap<String, String> = resp
            .data
            .iter()
            .filter_map(|a| {
                let refs = a.relationships.as_ref()?.mappings.as_ref()?;
                let mal = refs
                    .data
                    .iter()
                    .find_map(|r| mal_by_mapping_id.get(r.id.as_str()).copied())?;
                Some((a.id.clone(), mal.to_string()))
            })
            .collect();

        // Only anime "1" appears in data[]; the orphan mapping (101 → "999")
        // is unreferenced because no anime claims mapping id 101 in its
        // relationships.mappings.data list.
        assert_eq!(mal_by_anime.len(), 1);
        assert_eq!(mal_by_anime.get("1").map(String::as_str), Some("5"));
        let real_anime_ids: Vec<&str> = resp.data.iter().map(|a| a.id.as_str()).collect();
        assert!(!real_anime_ids.contains(&"999"));
    }

    #[test]
    fn kitsu_into_entry_populates_external_ids_with_mal() {
        let a = Anime {
            id: "1".into(),
            kind: "anime".into(),
            attributes: AnimeAttributes {
                title: "X".into(),
                synopsis: None,
                rating: None,
                start_date: None,
                episode_length: None,
                poster: None,
                show_type: None,
            },
            relationships: None,
        };
        let e = a.into_entry(EntryKind::Series, Some("12345".into()));
        assert_eq!(
            e.external_ids
                .get(id_sources::MYANIMELIST)
                .map(String::as_str),
            Some("12345"),
        );
        assert_eq!(
            e.external_ids.get(id_sources::KITSU).map(String::as_str),
            Some("1"),
        );
    }

    #[test]
    fn kitsu_into_entry_omits_external_ids_mal_when_none() {
        let a = Anime {
            id: "1".into(),
            kind: "anime".into(),
            attributes: AnimeAttributes {
                title: "X".into(),
                synopsis: None,
                rating: None,
                start_date: None,
                episode_length: None,
                poster: None,
                show_type: None,
            },
            relationships: None,
        };
        let e = a.into_entry(EntryKind::Series, None);
        assert!(e.external_ids.get(id_sources::MYANIMELIST).is_none());
        assert_eq!(
            e.external_ids.get(id_sources::KITSU).map(String::as_str),
            Some("1"),
        );
    }
}
