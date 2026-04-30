//! Discogs metadata provider — artists and albums via the Discogs REST API.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup}`. Track scope is
//! dropped per spec §5 (Discogs has no addressable per-track resource).
//!
//! ## API key
//!
//! Required. Read from `InitContext.config["api_key"]` at `Plugin::init`.
//! Fallback: `DISCOGS_API_KEY` env var via `cache_get("__env:...")`.

use std::sync::OnceLock;

use serde::Deserialize;

use stui_plugin_sdk::{
    parse_manifest,
    cache_get, error_codes, http_get,
    id_sources,
    plugin_error, plugin_info,
    stui_export_catalog_plugin,
    CatalogPlugin,
    EnrichRequest, EnrichResponse,
    EntryKind,
    InitContext,
    LookupRequest, LookupResponse,
    Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    SearchRequest, SearchResponse, SearchScope,
};

const API_BASE: &str = "https://api.discogs.com";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct DiscogsPlugin {
    manifest: PluginManifest,
    api_key: OnceLock<String>,
}

impl DiscogsPlugin {
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

    /// Resolve the Discogs personal-access token. Returns `None` when
    /// unset — Discogs API v2.0 allows unauthenticated requests (25 req/min);
    /// a token is only needed to unlock the 60 req/min tier plus user-scoped
    /// endpoints.
    fn api_key(&self) -> Option<&str> {
        if let Some(k) = self.api_key.get() {
            return Some(k.as_str());
        }
        let env_key = cache_get("__env:DISCOGS_API_KEY").unwrap_or_default();
        if env_key.is_empty() {
            return None;
        }
        Some(self.api_key.get_or_init(|| env_key).as_str())
    }
}

impl Default for DiscogsPlugin {
    fn default() -> Self { Self::new() }
}

impl Plugin for DiscogsPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        // api_key is optional; absent → unauthenticated path is fine.
        let key = ctx.config.get("api_key").and_then(|v| v.as_str()).map(str::to_string)
            .or_else(|| ctx.env.get("DISCOGS_API_KEY").cloned())
            .unwrap_or_default();
        if !key.is_empty() {
            let _ = self.api_key.set(key);
        }
        Ok(())
    }
}

// ── Error handling ────────────────────────────────────────────────────────────

fn classify_http_err(err: &str) -> PluginError {
    if let Some(rest) = err.strip_prefix("HTTP ") {
        if let Some((code_str, body)) = rest.split_once(": ") {
            if let Ok(status) = code_str.parse::<u16>() {
                let code = match status {
                    401 | 403 => error_codes::INVALID_REQUEST,
                    404       => error_codes::UNKNOWN_ID,
                    429       => error_codes::RATE_LIMITED,
                    500..=599 => error_codes::TRANSIENT,
                    _         => error_codes::REMOTE_ERROR,
                };
                return PluginError { code: code.to_string(), message: format!("Discogs HTTP {status}: {body}") };
            }
        }
    }
    PluginError { code: error_codes::TRANSIENT.to_string(), message: err.to_string() }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, PluginError> {
    serde_json::from_str(body).map_err(|e| {
        plugin_error!("discogs: parse error: {}", e);
        PluginError { code: error_codes::PARSE_ERROR.to_string(), message: format!("Discogs JSON parse failure: {e}") }
    })
}

/// Build the `&key=<token>` URL suffix when a personal-access token is
/// configured; empty string otherwise (unauthenticated path).
fn auth_suffix_for(key: Option<&str>) -> String {
    match key.filter(|k| !k.is_empty()) {
        Some(k) => format!("&key={k}"),
        None    => String::new(),
    }
}

/// Return `genre` and `description` strings with a clean split:
/// - `genre`  = `genres + styles` (music-genre information)
/// - `desc`   = format / country / label (physical-release metadata)
fn partition_genre_and_description(
    genres: &[String],
    styles: &[String],
    format: &[String],
    country: Option<&str>,
    label: &[String],
) -> (Option<String>, Option<String>) {
    let mut combined_genres: Vec<&str> = genres.iter().map(|s| s.as_str()).collect();
    combined_genres.extend(styles.iter().map(|s| s.as_str()));
    let genre = if combined_genres.is_empty() {
        None
    } else {
        Some(combined_genres.into_iter().take(5).collect::<Vec<_>>().join(", "))
    };

    let mut desc_parts: Vec<String> = Vec::new();
    if !format.is_empty() {
        desc_parts.push(format.join(", "));
    }
    if let Some(c) = country {
        if !c.is_empty() {
            desc_parts.push(c.to_string());
        }
    }
    if !label.is_empty() {
        desc_parts.push(format!("Label: {}", label.join(", ")));
    }
    let description = if desc_parts.is_empty() { None } else { Some(desc_parts.join(" | ")) };
    (genre, description)
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for DiscogsPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let (search_type, entry_kind) = match req.scope {
            SearchScope::Artist => ("artist",  EntryKind::Artist),
            SearchScope::Album  => ("release", EntryKind::Album),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "discogs only supports artist and album scopes",
                );
            }
        };

        let auth_suffix = auth_suffix_for(self.api_key());

        let page     = req.page.max(1);
        let per_page = if req.limit == 0 { 20 } else { req.limit.min(50) } as usize;
        let query    = req.query.trim();

        // Discogs has no meaningful "trending" surface — the closest API
        // (sort=date_added,desc) returns recently-cataloged releases, which
        // are dominated by indie pressings with missing cover art and
        // "Artist - Album" formatted titles. Returning those for empty
        // queries pollutes the runtime's catalog trending grid (which on
        // the music tab is lastfm-driven via tag.gettopalbums). Short-
        // circuit to an empty result so discogs only contributes to
        // explicit user searches; the enrich path stays unaffected.
        if query.is_empty() {
            return PluginResult::ok(SearchResponse { items: vec![], total: 0 });
        }

        let url = format!(
            "{API_BASE}/database/search?q={}&type={search_type}&page={page}&per_page={per_page}{auth_suffix}",
            urlencoding::encode(query),
        );
        plugin_info!("discogs: search '{}' type={search_type} (page {page})", query);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let resp: SearchEnvelope = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };

        let items: Vec<PluginEntry> = resp.results.into_iter()
            .filter(|r| r.id > 0)
            .take(per_page)
            .map(|r| r.into_entry(entry_kind))
            .collect();
        let total = resp.pagination.as_ref()
            .and_then(|p| p.items)
            .unwrap_or(items.len() as i32)
            .max(0) as u32;
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        if req.id_source != id_sources::DISCOGS {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("discogs lookup: unsupported id_source: {}", req.id_source),
            );
        }
        let auth_suffix = auth_suffix_for(self.api_key());

        let (path, entry_kind) = match req.kind {
            EntryKind::Artist => (format!("/artists/{}", urlencoding::encode(&req.id)), EntryKind::Artist),
            EntryKind::Album  => (format!("/releases/{}", urlencoding::encode(&req.id)), EntryKind::Album),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "discogs lookup supports artist and album kinds only",
                );
            }
        };
        // `?` starts the query string even when there's no token; Discogs ignores
        // the trailing empty query just fine.
        let url = format!("{API_BASE}{path}?_=1{auth_suffix}");
        plugin_info!("discogs: lookup {} ({:?})", req.id, req.kind);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let entry = match entry_kind {
            EntryKind::Artist => match parse_json::<ArtistDetail>(&body) {
                Ok(a) => a.into_entry(),
                Err(e) => return PluginResult::Err(e),
            },
            _ => match parse_json::<ReleaseDetail>(&body) {
                Ok(r) => r.into_entry(),
                Err(e) => return PluginResult::Err(e),
            },
        };
        PluginResult::ok(LookupResponse { entry })
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        // Fast path: caller already carries a discogs id (e.g. from a
        // prior lookup). Reuse lookup verbatim so the rating gets
        // populated from the same /releases/{id} response.
        if let Some(discogs_id) = req.partial.external_ids.get(id_sources::DISCOGS).cloned() {
            let lookup_req = LookupRequest {
                id:        discogs_id,
                id_source: id_sources::DISCOGS.to_string(),
                kind:      req.partial.kind,
                locale:    None,
            };
            return match self.lookup(lookup_req) {
                PluginResult::Ok(r)  => PluginResult::ok(EnrichResponse { entry: r.entry, confidence: 1.0 }),
                PluginResult::Err(e) => PluginResult::Err(e),
            };
        }

        // Fallback: title + artist search via /database/search?type=release.
        // We only support album kind here — rating data is meaningless on
        // an artist record, so artist enrich short-circuits to a no-op
        // error (the caller treats EnrichResponse as best-effort).
        if !matches!(req.partial.kind, EntryKind::Album) {
            return PluginResult::err(
                error_codes::UNSUPPORTED_SCOPE,
                "discogs enrich: only album kind is supported (rating data)",
            );
        }
        let title = req.partial.title.trim();
        if title.is_empty() {
            return PluginResult::err(
                error_codes::INVALID_REQUEST,
                "discogs enrich: empty title and no discogs id",
            );
        }
        // Compose "title artist" so Discogs's search ranking gives us a
        // tightly-matched release. Releases without an artist still get
        // a useful query (just the album title).
        let query = match req.partial.artist_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(artist) => format!("{title} {artist}"),
            None         => title.to_string(),
        };
        let auth_suffix = auth_suffix_for(self.api_key());
        let search_url = format!(
            "{API_BASE}/database/search?q={}&type=release&per_page=5{auth_suffix}",
            urlencoding::encode(&query),
        );
        plugin_info!("discogs: enrich search '{}'", query);

        let body = match http_get(&search_url) {
            Ok(b)  => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let resp: SearchEnvelope = match parse_json(&body) {
            Ok(r)  => r,
            Err(e) => return PluginResult::Err(e),
        };
        let hit = match resp.results.into_iter().find(|h| h.id > 0) {
            Some(h) => h,
            None    => return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("discogs: no release matched '{query}'"),
            ),
        };
        // Hop into the per-release endpoint to pick up community.rating —
        // search hits don't include it. This is the second of the two
        // round-trips we accept on the cold path; subsequent enrich calls
        // for the same album reuse the result via the runtime's HTTP
        // response cache.
        let lookup_req = LookupRequest {
            id:        hit.id.to_string(),
            id_source: id_sources::DISCOGS.to_string(),
            kind:      EntryKind::Album,
            locale:    None,
        };
        match self.lookup(lookup_req) {
            // Title-search is less precise than direct id lookup, so
            // surface that in the confidence score.
            PluginResult::Ok(r)  => PluginResult::ok(EnrichResponse { entry: r.entry, confidence: 0.7 }),
            PluginResult::Err(e) => PluginResult::Err(e),
        }
    }
}

// ── API types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    pagination: Option<Pagination>,
    #[serde(default)]
    results: Vec<SearchHit>,
}

#[derive(Debug, Deserialize, Default)]
struct Pagination {
    items: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    #[serde(default)] id: i64,
    #[serde(default)] title: String,
    /// Discogs's `/database/search` endpoint returns `year` as a JSON
    /// string (e.g. `"2024"`); the `/releases/{id}` detail endpoint
    /// returns it as a number. Accept either via `flexible_year` so
    /// both code paths share this struct.
    #[serde(default, deserialize_with = "flexible_year")]
    year: Option<i32>,
    #[serde(default)] country: Option<String>,
    #[serde(default)] format: Vec<String>,
    #[serde(default)] label: Vec<String>,
    #[serde(rename = "cover_image", default)] cover_image: Option<String>,
    #[serde(rename = "thumb", default)]        thumb: Option<String>,
    #[serde(default)] genre: Vec<String>,
    #[serde(default)] style: Vec<String>,
}

/// Deserialize a year that Discogs may emit as either a JSON number
/// or a JSON string. Search-result hits use the string form; lookup
/// detail uses the number form. Empty / non-numeric strings → None.
fn flexible_year<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Int(i32),
        Str(String),
        Null,
    }
    match Option::<Either>::deserialize(deserializer)? {
        None | Some(Either::Null) => Ok(None),
        Some(Either::Int(i)) => Ok(Some(i)),
        Some(Either::Str(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            trimmed.parse::<i32>().map(Some).map_err(D::Error::custom)
        }
    }
}

impl SearchHit {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let year = self.year.and_then(|y| if y > 0 { Some(y as u32) } else { None });
        let (genre, description) = partition_genre_and_description(
            &self.genre,
            &self.style,
            &self.format,
            self.country.as_deref(),
            &self.label,
        );
        let poster_url = self.cover_image.or(self.thumb);

        let mut entry = PluginEntry {
            id: format!("discogs-{}", self.id),
            kind,
            source: "discogs".to_string(),
            title: self.title,
            year,
            genre,
            description,
            poster_url,
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::DISCOGS.to_string(), self.id.to_string());
        entry
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseDetail {
    #[serde(default)] id: i64,
    #[serde(default)] title: String,
    #[serde(default)] year: Option<i32>,
    #[serde(default)] country: Option<String>,
    #[serde(default)] genres: Vec<String>,
    #[serde(default)] styles: Vec<String>,
    #[serde(default)] formats: Vec<ReleaseFormat>,
    #[serde(default)] labels:  Vec<ReleaseLabel>,
    #[serde(default)] images:  Vec<ReleaseImage>,
    #[serde(default)] notes:   Option<String>,
    #[serde(default)] community: Option<ReleaseCommunity>,
}

#[derive(Debug, Deserialize, Default)]
struct ReleaseCommunity {
    #[serde(default)] rating: Option<ReleaseRating>,
}

#[derive(Debug, Deserialize, Default)]
struct ReleaseRating {
    #[serde(default)] average: f32,
    #[serde(default)] count:   u32,
}

#[derive(Debug, Deserialize, Default)]
struct ReleaseFormat { #[serde(default)] name: String }

#[derive(Debug, Deserialize, Default)]
struct ReleaseLabel  { #[serde(default)] name: String }

#[derive(Debug, Deserialize)]
struct ReleaseImage {
    #[serde(rename = "uri",     default)] uri: Option<String>,
    #[serde(rename = "uri150",  default)] uri150: Option<String>,
}

impl ReleaseDetail {
    fn into_entry(self) -> PluginEntry {
        let year = self.year.and_then(|y| if y > 0 { Some(y as u32) } else { None });
        let formats: Vec<String> = self.formats.into_iter().map(|f| f.name).filter(|s| !s.is_empty()).collect();
        let labels:  Vec<String> = self.labels.into_iter().map(|l| l.name).filter(|s| !s.is_empty()).collect();
        let (genre, mut description) = partition_genre_and_description(
            &self.genres,
            &self.styles,
            &formats,
            self.country.as_deref(),
            &labels,
        );
        if let Some(notes) = self.notes.filter(|n| !n.is_empty()) {
            description = match description {
                Some(d) => Some(format!("{d}\n\n{notes}")),
                None    => Some(notes),
            };
        }
        let poster_url = self.images.into_iter().find_map(|i| i.uri.clone().or(i.uri150.clone()));
        // Discogs surfaces a community-aggregated rating on every release
        // (0-5 scale) plus the voter count. Skip ratings with zero votes
        // since the average is meaningless then; downstream UI treats a
        // None as "no rating" and hides the badge.
        let rating = self.community
            .and_then(|c| c.rating)
            .filter(|r| r.count > 0 && r.average > 0.0)
            .map(|r| r.average);

        let mut entry = PluginEntry {
            id: format!("discogs-{}", self.id),
            kind: EntryKind::Album,
            source: "discogs".to_string(),
            title: self.title,
            year,
            genre,
            rating,
            description,
            poster_url,
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::DISCOGS.to_string(), self.id.to_string());
        entry
    }
}

#[derive(Debug, Deserialize)]
struct ArtistDetail {
    #[serde(default)] id: i64,
    #[serde(default)] name: String,
    #[serde(default)] profile: Option<String>,
    #[serde(default)] images: Vec<ReleaseImage>,
    #[serde(default)] namevariations: Vec<String>,
}

impl ArtistDetail {
    fn into_entry(self) -> PluginEntry {
        let description = self.profile.filter(|p| !p.is_empty()).or_else(|| {
            if self.namevariations.is_empty() {
                None
            } else {
                Some(format!("Also known as: {}", self.namevariations.join(", ")))
            }
        });
        let poster_url = self.images.into_iter().find_map(|i| i.uri.or(i.uri150));
        let mut entry = PluginEntry {
            id: format!("discogs-{}", self.id),
            kind: EntryKind::Artist,
            source: "discogs".to_string(),
            title: self.name,
            description,
            poster_url,
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::DISCOGS.to_string(), self.id.to_string());
        entry
    }
}

// ── WASM exports ──────────────────────────────────────────────────────────────

impl stui_plugin_sdk::StreamProvider for DiscogsPlugin {}

stui_export_catalog_plugin!(DiscogsPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn _p<T: Plugin>() {}
        fn _c<T: CatalogPlugin>() {}
        _p::<DiscogsPlugin>();
        _c::<DiscogsPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = DiscogsPlugin::new();
        assert_eq!(p.manifest().plugin.name, "discogs");
    }

    #[test]
    fn genre_comes_from_genre_plus_style_only() {
        let (g, _d) = partition_genre_and_description(
            &["Electronic".to_string()],
            &["Techno".to_string(), "IDM".to_string()],
            &["CD".to_string()],
            Some("UK"),
            &["Warp Records".to_string()],
        );
        assert_eq!(g.as_deref(), Some("Electronic, Techno, IDM"));
    }

    #[test]
    fn description_collects_format_country_label() {
        let (_g, d) = partition_genre_and_description(
            &[],
            &[],
            &["CD".to_string(), "Album".to_string()],
            Some("US"),
            &["Atlantic".to_string()],
        );
        assert_eq!(d.as_deref(), Some("CD, Album | US | Label: Atlantic"));
    }

    #[test]
    fn description_empty_when_no_physical_metadata() {
        let (_g, d) = partition_genre_and_description(
            &["Rock".to_string()],
            &[],
            &[],
            None,
            &[],
        );
        assert_eq!(d, None);
    }

    #[test]
    fn search_hit_into_entry_populates_discogs_id() {
        let hit = SearchHit {
            id: 12345,
            title: "OK Computer".into(),
            year: Some(1997),
            country: Some("UK".into()),
            format: vec!["CD".into()],
            label: vec!["Parlophone".into()],
            cover_image: Some("https://img/1.jpg".into()),
            thumb: None,
            genre: vec!["Rock".into()],
            style: vec!["Alternative".into()],
        };
        let e = hit.into_entry(EntryKind::Album);
        assert_eq!(e.source, "discogs");
        assert_eq!(e.kind, EntryKind::Album);
        assert_eq!(e.year, Some(1997));
        assert_eq!(e.genre.as_deref(), Some("Rock, Alternative"));
        assert_eq!(e.description.as_deref(), Some("CD | UK | Label: Parlophone"));
        assert_eq!(e.external_ids.get(id_sources::DISCOGS).map(String::as_str), Some("12345"));
    }

    #[test]
    fn invalid_years_rejected() {
        let hit = SearchHit {
            id: 1, title: "x".into(), year: Some(0),
            country: None, format: vec![], label: vec![],
            cover_image: None, thumb: None, genre: vec![], style: vec![],
        };
        let e = hit.into_entry(EntryKind::Album);
        assert_eq!(e.year, None);
    }

    #[test]
    fn new_for_test_caches_api_key() {
        let p = DiscogsPlugin::new_for_test("fake");
        assert_eq!(p.api_key(), Some("fake"));
    }

    #[test]
    fn api_key_absent_is_unauthenticated_path() {
        let p = DiscogsPlugin::new();
        assert_eq!(p.api_key(), None);
    }

    #[test]
    fn auth_suffix_is_empty_when_key_absent() {
        assert_eq!(auth_suffix_for(None),     "");
        assert_eq!(auth_suffix_for(Some("")), "");
        assert_eq!(auth_suffix_for(Some("abc")), "&key=abc");
    }

    #[test]
    fn release_detail_parses_community_rating() {
        let body = r#"{
            "id": 1234,
            "title": "Kid A",
            "year": 2000,
            "community": { "rating": { "average": 4.55, "count": 19234 } }
        }"#;
        let detail: ReleaseDetail = serde_json::from_str(body).unwrap();
        let entry = detail.into_entry();
        assert_eq!(entry.rating, Some(4.55));
    }

    #[test]
    fn release_detail_drops_zero_count_rating() {
        // Discogs returns `count: 0` with a stale average for releases
        // nobody has rated yet — those numbers are noise. Keep `rating`
        // None so the UI doesn't render a meaningless badge.
        let body = r#"{
            "id": 1, "title": "x",
            "community": { "rating": { "average": 3.0, "count": 0 } }
        }"#;
        let detail: ReleaseDetail = serde_json::from_str(body).unwrap();
        assert_eq!(detail.into_entry().rating, None);
    }

    #[test]
    fn release_detail_handles_missing_community_block() {
        let body = r#"{ "id": 1, "title": "x" }"#;
        let detail: ReleaseDetail = serde_json::from_str(body).unwrap();
        assert_eq!(detail.into_entry().rating, None);
    }
}
