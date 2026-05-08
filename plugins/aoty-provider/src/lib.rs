//! Album of the Year metadata provider — critic + user scores via the
//! self-hosted AlbumOfTheYearAPI wrapper.
//!
//! Implements `Plugin` + `CatalogPlugin::{enrich, bulk_enrich}`. Search
//! is stubbed (NOT_IMPLEMENTED) — the underlying API has no search.
//!
//! Endpoints used:
//! - `GET /artist/mb/{mb_id}` for `EntryKind::Artist`
//! - `GET /album/mb/{mb_id}`  for `EntryKind::Album`
//!
//! Both populate `ratings["aoty_critic"]` (0–100) and
//! `ratings["aoty_user"]` (0–100) plus matching `rating_votes` so the
//! aggregator's Bayesian shrinkage applies automatically.
//!
//! ## API key
//! Required. `InitContext.config["api_key"]` or `AOTY_API_KEY` env var.
//!
//! ## Base URL
//! Optional. Defaults to `https://album-of-the-year-api.vercel.app`.

use std::sync::OnceLock;

use serde::Deserialize;
use stui_plugin_sdk::{
    err_not_implemented, error_codes, http_request, parse_manifest, plugin_error, plugin_info,
    stui_export_catalog_plugin, BulkEnrichEntry, BulkEnrichRequest, BulkEnrichResponse,
    CatalogPlugin, EnrichRequest, EnrichResponse, EntryKind, HttpRequest, InitContext, Plugin,
    PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult, SearchRequest,
    SearchResponse, StreamProvider,
};

const DEFAULT_BASE_URL: &str = "https://album-of-the-year-api.vercel.app";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct AotyProvider {
    manifest: PluginManifest,
    api_key: OnceLock<String>,
    base_url: OnceLock<String>,
}

impl AotyProvider {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self {
            manifest,
            api_key: OnceLock::new(),
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

    fn api_key(&self) -> Result<&str, PluginError> {
        if let Some(k) = self.api_key.get() {
            return Ok(k.as_str());
        }
        let env_key = stui_plugin_sdk::cache_get("__env:AOTY_API_KEY").unwrap_or_default();
        if env_key.is_empty() {
            return Err(PluginError {
                code: error_codes::INVALID_REQUEST.to_string(),
                message: "AOTY api_key not configured".to_string(),
            });
        }
        Ok(self.api_key.get_or_init(|| env_key).as_str())
    }

    fn base_url(&self) -> &str {
        if let Some(u) = self.base_url.get() {
            return u.as_str();
        }
        let env_url = stui_plugin_sdk::cache_get("__env:AOTY_API_BASE_URL").unwrap_or_default();
        let resolved = if env_url.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            env_url
        };
        self.base_url.get_or_init(|| resolved).as_str()
    }

    /// `GET /{path}/mb/{mb_id}` → JSON body. Caller picks the path
    /// (`artist` or `album`) and the response shape.
    fn fetch_mb(&self, path: &str, mb_id: &str) -> Result<String, PluginError> {
        let api_key = self.api_key()?.to_string();
        let url = format!(
            "{}/{}/mb/{}",
            self.base_url(),
            path,
            urlencoding::encode(mb_id),
        );
        let req = HttpRequest {
            method: "GET".to_string(),
            url,
            headers: vec![("X-API-Key".to_string(), api_key)],
            body: None,
        };
        plugin_info!("aoty-provider: GET /{}/mb/{}", path, mb_id);
        let resp = http_request(req).map_err(|e| PluginError {
            code: error_codes::TRANSIENT.to_string(),
            message: format!("aoty-api: {e}"),
        })?;
        classify_http(resp.status)?;
        Ok(resp.body)
    }
}

impl Default for AotyProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ── Plugin impl ───────────────────────────────────────────────────────────────

impl Plugin for AotyProvider {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        let key = ctx
            .config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("AOTY_API_KEY").cloned())
            .unwrap_or_default();
        if key.is_empty() {
            return Err(PluginInitError::MissingConfig {
                fields: vec!["api_key".to_string()],
                hint: Some(
                    "Get a key from your self-hosted album-of-the-year-api admin (X-API-Key header)"
                        .to_string(),
                ),
            });
        }
        let _ = self.api_key.set(key);

        let url = ctx
            .config
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("AOTY_API_BASE_URL").cloned())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let _ = self.base_url.set(url);

        Ok(())
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for AotyProvider {
    fn search(&self, _req: SearchRequest) -> PluginResult<SearchResponse> {
        err_not_implemented()
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        let kind = req.partial.kind;
        let Some(mb_id) = mb_id_from(&req.partial) else {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                "aoty-provider enrich: musicbrainz id is required",
            );
        };
        match enrich_one(self, kind, &mb_id) {
            Ok(entry) => PluginResult::ok(EnrichResponse {
                entry,
                confidence: 1.0,
            }),
            Err(e) => PluginResult::Err(e),
        }
    }

    fn bulk_enrich(&self, req: BulkEnrichRequest) -> PluginResult<BulkEnrichResponse> {
        // AOTY API has no MB-keyed batch endpoint; loop single-id calls.
        // The API caches per-MB-id internally so repeats are cheap.
        let entries: Vec<BulkEnrichEntry> = req
            .partials
            .into_iter()
            .map(|partial| {
                let id = partial.id.clone();
                let Some(mb_id) = mb_id_from(&partial) else {
                    return BulkEnrichEntry {
                        id,
                        result: PluginResult::err(
                            error_codes::UNKNOWN_ID,
                            "aoty-provider: musicbrainz id is required",
                        ),
                    };
                };
                let result = match enrich_one(self, partial.kind, &mb_id) {
                    Ok(entry) => PluginResult::ok(EnrichResponse {
                        entry,
                        confidence: 1.0,
                    }),
                    Err(e) => PluginResult::Err(e),
                };
                BulkEnrichEntry { id, result }
            })
            .collect();
        PluginResult::ok(BulkEnrichResponse { entries })
    }
}

impl StreamProvider for AotyProvider {}

stui_export_catalog_plugin!(AotyProvider);

// ── API response types ────────────────────────────────────────────────────────

/// `GET /artist/mb/{id}` response. Critic + user nested.
#[derive(Debug, Clone, Deserialize)]
struct AotyArtistResponse {
    #[serde(default)]
    artist_id: String,
    #[serde(default)]
    critic: Option<NestedCritic>,
    #[serde(default)]
    user: Option<NestedUser>,
}

#[derive(Debug, Clone, Deserialize)]
struct NestedCritic {
    #[serde(default)]
    critic_score: Option<f32>,
    #[serde(default)]
    review_count: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct NestedUser {
    #[serde(default)]
    user_score: Option<f32>,
    #[serde(default)]
    rating_count: Option<u32>,
}

/// `GET /album/mb/{id}` response. Flat.
#[derive(Debug, Clone, Deserialize)]
struct AotyAlbumResponse {
    #[serde(default)]
    album_slug: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    /// Primary genre per AOTY's curated taxonomy (e.g. "Indie Pop",
    /// "Hip Hop"). Single string, not a tag cloud — preferred over
    /// Last.fm/MB tag-derived genres which leak labels like "album" /
    /// "favorite" / personal-taxonomy noise. None when the album page
    /// has no `<a href="/genre/...">` row.
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    critic_score: Option<f32>,
    #[serde(default)]
    review_count: Option<u32>,
    #[serde(default)]
    user_score: Option<f32>,
    #[serde(default)]
    rating_count: Option<u32>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the MusicBrainz ID from a partial entry. Looks in
/// `external_ids["musicbrainz"]`. Empty/missing → None.
fn mb_id_from(partial: &PluginEntry) -> Option<String> {
    partial
        .external_ids
        .get("musicbrainz")
        .map(String::clone)
        .filter(|s| !s.is_empty())
}

/// Run a single enrich for the given kind, returning a fully populated
/// PluginEntry on success.
fn enrich_one(
    plugin: &AotyProvider,
    kind: EntryKind,
    mb_id: &str,
) -> Result<PluginEntry, PluginError> {
    match kind {
        EntryKind::Artist => {
            let body = plugin.fetch_mb("artist", mb_id)?;
            let resp: AotyArtistResponse = serde_json::from_str(&body).map_err(|e| {
                plugin_error!("aoty-provider: artist parse error: {}", e);
                PluginError {
                    code: error_codes::PARSE_ERROR.to_string(),
                    message: format!("aoty-api: artist JSON parse failure: {e}"),
                }
            })?;
            Ok(project_artist(resp, mb_id))
        }
        EntryKind::Album => {
            let body = plugin.fetch_mb("album", mb_id)?;
            let resp: AotyAlbumResponse = serde_json::from_str(&body).map_err(|e| {
                plugin_error!("aoty-provider: album parse error: {}", e);
                PluginError {
                    code: error_codes::PARSE_ERROR.to_string(),
                    message: format!("aoty-api: album JSON parse failure: {e}"),
                }
            })?;
            Ok(project_album(resp, mb_id))
        }
        // AOTY has no Track-level data; fan-out should not call us with Track.
        _ => Err(PluginError {
            code: error_codes::INVALID_REQUEST.to_string(),
            message: format!("aoty-provider: unsupported kind {kind:?}"),
        }),
    }
}

/// Build a PluginEntry for an artist response. Drops zero/null scores.
fn project_artist(resp: AotyArtistResponse, mb_id: &str) -> PluginEntry {
    let mut ratings = std::collections::HashMap::new();
    let mut votes = std::collections::HashMap::new();
    if let Some(c) = resp.critic {
        if let Some(s) = c.critic_score.filter(|v| *v > 0.0) {
            ratings.insert("aoty_critic".to_string(), s);
        }
        if let Some(n) = c.review_count.filter(|v| *v > 0) {
            votes.insert("aoty_critic".to_string(), n);
        }
    }
    if let Some(u) = resp.user {
        if let Some(s) = u.user_score.filter(|v| *v > 0.0) {
            ratings.insert("aoty_user".to_string(), s);
        }
        if let Some(n) = u.rating_count.filter(|v| *v > 0) {
            votes.insert("aoty_user".to_string(), n);
        }
    }
    // The API normally returns the AOTY slug as artist_id (e.g.
    // `183-kanye-west`). Fall back to the MusicBrainz id when the
    // upstream field is empty so we never emit `id = ""` — empty ids
    // collide in the orchestrator's dedup map and silently lose
    // entries during merge.
    let id = if resp.artist_id.is_empty() {
        mb_id.to_string()
    } else {
        resp.artist_id
    };
    let mut entry = PluginEntry {
        id,
        kind: EntryKind::Artist,
        title: String::new(), // AOTY artist endpoint doesn't surface display name
        source: "albumoftheyear".to_string(),
        ratings,
        rating_votes: votes,
        ..Default::default()
    };
    entry
        .external_ids
        .insert("musicbrainz".to_string(), mb_id.to_string());
    entry
}

/// Build a PluginEntry for an album response. Drops zero/null scores.
fn project_album(resp: AotyAlbumResponse, mb_id: &str) -> PluginEntry {
    let mut ratings = std::collections::HashMap::new();
    let mut votes = std::collections::HashMap::new();
    if let Some(s) = resp.critic_score.filter(|v| *v > 0.0) {
        ratings.insert("aoty_critic".to_string(), s);
    }
    if let Some(n) = resp.review_count.filter(|v| *v > 0) {
        votes.insert("aoty_critic".to_string(), n);
    }
    if let Some(s) = resp.user_score.filter(|v| *v > 0.0) {
        ratings.insert("aoty_user".to_string(), s);
    }
    if let Some(n) = resp.rating_count.filter(|v| *v > 0) {
        votes.insert("aoty_user".to_string(), n);
    }
    // Same id-fallback story as project_artist: empty ids collide on
    // dedup and silently drop entries during merge. AOTY's album_slug
    // is the canonical identifier when present; otherwise pin to
    // mb_id so every entry has a stable, non-empty key.
    let id = resp
        .album_slug
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| mb_id.to_string());
    let genre = resp.genre.filter(|s| !s.is_empty());
    let mut entry = PluginEntry {
        id,
        kind: EntryKind::Album,
        title: resp.title.unwrap_or_default(),
        source: "albumoftheyear".to_string(),
        genre,
        ratings,
        rating_votes: votes,
        ..Default::default()
    };
    entry
        .external_ids
        .insert("musicbrainz".to_string(), mb_id.to_string());
    if let Some(artist) = resp.artist.filter(|s| !s.is_empty()) {
        entry
            .external_ids
            .insert("aoty_artist_name".to_string(), artist);
    }
    entry
}

/// Map an HTTP response status to a PluginError. 404 → UNKNOWN_ID
/// (no MB→AOTY mapping yet), 401 → INVALID_REQUEST (bad key), etc.
fn classify_http(status: u16) -> Result<(), PluginError> {
    match status {
        200..=299 => Ok(()),
        401 => Err(PluginError {
            code: error_codes::INVALID_REQUEST.to_string(),
            message: "aoty-api: unauthorized (check AOTY_API_KEY)".to_string(),
        }),
        404 => Err(PluginError {
            code: error_codes::UNKNOWN_ID.to_string(),
            message: "aoty-api: no AOTY mapping for this MusicBrainz ID".to_string(),
        }),
        429 => Err(PluginError {
            code: error_codes::RATE_LIMITED.to_string(),
            message: "aoty-api: rate limit exceeded".to_string(),
        }),
        500..=599 => Err(PluginError {
            code: error_codes::TRANSIENT.to_string(),
            message: format!("aoty-api: HTTP {status}"),
        }),
        _ => Err(PluginError {
            code: error_codes::REMOTE_ERROR.to_string(),
            message: format!("aoty-api: unexpected HTTP {status}"),
        }),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = AotyProvider::new();
        let m = p.manifest();
        assert_eq!(m.plugin.name, "albumoftheyear");
        assert_eq!(m.plugin._abi_version, Some(2));
    }

    #[test]
    fn search_returns_not_implemented() {
        use stui_plugin_sdk::SearchScope;
        let p = AotyProvider::new();
        let result = p.search(SearchRequest {
            query: "Kanye West".to_string(),
            scope: SearchScope::Artist,
            page: 1,
            limit: 10,
            per_scope_limit: None,
            locale: None,
        });
        match result {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::NOT_IMPLEMENTED),
            PluginResult::Ok(_) => panic!("expected NOT_IMPLEMENTED"),
        }
    }

    #[test]
    fn project_artist_populates_ratings_and_votes() {
        let resp = AotyArtistResponse {
            artist_id: "183-kanye-west".into(),
            critic: Some(NestedCritic {
                critic_score: Some(73.0),
                review_count: Some(376),
            }),
            user: Some(NestedUser {
                user_score: Some(80.0),
                rating_count: Some(349666),
            }),
        };
        let e = project_artist(resp, "164f0d73-1234-4e2c-8743-d77bf2191051");
        assert_eq!(e.kind, EntryKind::Artist);
        assert_eq!(e.source, "albumoftheyear");
        assert_eq!(e.ratings.get("aoty_critic").copied(), Some(73.0));
        assert_eq!(e.ratings.get("aoty_user").copied(), Some(80.0));
        assert_eq!(e.rating_votes.get("aoty_critic").copied(), Some(376));
        assert_eq!(e.rating_votes.get("aoty_user").copied(), Some(349666));
        assert_eq!(
            e.external_ids.get("musicbrainz").map(String::as_str),
            Some("164f0d73-1234-4e2c-8743-d77bf2191051"),
        );
    }

    #[test]
    fn project_artist_drops_zero_and_missing() {
        let resp = AotyArtistResponse {
            artist_id: "x".into(),
            critic: Some(NestedCritic {
                critic_score: Some(0.0),
                review_count: None,
            }),
            user: None,
        };
        let e = project_artist(resp, "mb-id");
        assert!(
            e.ratings.is_empty(),
            "0 score + missing user should drop both"
        );
        assert!(e.rating_votes.is_empty());
    }

    #[test]
    fn project_album_populates_ratings_and_votes() {
        let resp = AotyAlbumResponse {
            album_slug: Some("2546-dom-family-of-love".into()),
            title: Some("Family of Love".into()),
            artist: Some("DOM".into()),
            genre: Some("Indie Pop".into()),
            critic_score: Some(73.0),
            review_count: Some(7),
            user_score: Some(67.0),
            rating_count: Some(9),
        };
        let e = project_album(resp, "mb-album-id");
        assert_eq!(e.kind, EntryKind::Album);
        assert_eq!(e.title, "Family of Love");
        assert_eq!(e.genre.as_deref(), Some("Indie Pop"));
        assert_eq!(e.ratings.get("aoty_critic").copied(), Some(73.0));
        assert_eq!(e.ratings.get("aoty_user").copied(), Some(67.0));
        assert_eq!(e.rating_votes.get("aoty_critic").copied(), Some(7));
        assert_eq!(e.rating_votes.get("aoty_user").copied(), Some(9));
        assert_eq!(
            e.external_ids.get("aoty_artist_name").map(String::as_str),
            Some("DOM"),
        );
    }

    #[test]
    fn project_artist_falls_back_to_mb_id_when_artist_id_empty() {
        // Defensive: if the API ever returns an empty artist_id field
        // (e.g. partial response on slug-resolution failure), we must
        // not emit `id = ""` because empty keys collide in the
        // orchestrator's dedup map.
        let resp = AotyArtistResponse {
            artist_id: String::new(),
            critic: Some(NestedCritic {
                critic_score: Some(73.0),
                review_count: Some(376),
            }),
            user: None,
        };
        let mb = "164f0d73-1234-4e2c-8743-d77bf2191051";
        let e = project_artist(resp, mb);
        assert_eq!(e.id, mb);
    }

    #[test]
    fn project_album_falls_back_to_mb_id_when_slug_missing_or_empty() {
        // Same dedup-key concern as project_artist. Cover both the
        // None and empty-string cases serde could produce.
        let mb = "5d6e21e1-deb5-428e-bb42-c2a567f3619b";

        let resp_none = AotyAlbumResponse {
            album_slug: None,
            title: Some("X".into()),
            artist: None,
            genre: None,
            critic_score: Some(50.0),
            review_count: Some(1),
            user_score: None,
            rating_count: None,
        };
        assert_eq!(project_album(resp_none, mb).id, mb);

        let resp_empty = AotyAlbumResponse {
            album_slug: Some(String::new()),
            title: Some("X".into()),
            artist: None,
            genre: None,
            critic_score: Some(50.0),
            review_count: Some(1),
            user_score: None,
            rating_count: None,
        };
        assert_eq!(project_album(resp_empty, mb).id, mb);
    }

    #[test]
    fn project_album_omits_empty_genre() {
        let resp = AotyAlbumResponse {
            album_slug: Some("x".into()),
            title: Some("X".into()),
            artist: None,
            genre: Some("".into()),
            critic_score: Some(50.0),
            review_count: Some(1),
            user_score: None,
            rating_count: None,
        };
        let e = project_album(resp, "mb");
        assert!(e.genre.is_none(), "empty-string genre should be dropped");
    }

    #[test]
    fn enrich_without_mb_id_returns_unknown_id() {
        let p = AotyProvider::new_for_test("fake-key", "https://test.example.com");
        let req = EnrichRequest {
            partial: PluginEntry {
                id: "no-mb".into(),
                kind: EntryKind::Album,
                source: "test".into(),
                ..Default::default()
            },
            prefer_id_source: None,
            force_refresh: false,
        };
        match p.enrich(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            PluginResult::Ok(_) => panic!("expected UNKNOWN_ID"),
        }
    }

    #[test]
    fn classify_http_404_maps_to_unknown_id() {
        let e = classify_http(404).unwrap_err();
        assert_eq!(e.code, error_codes::UNKNOWN_ID);
    }

    #[test]
    fn classify_http_401_maps_to_invalid_request() {
        let e = classify_http(401).unwrap_err();
        assert_eq!(e.code, error_codes::INVALID_REQUEST);
    }

    #[test]
    fn classify_http_429_maps_to_rate_limited() {
        let e = classify_http(429).unwrap_err();
        assert_eq!(e.code, error_codes::RATE_LIMITED);
    }

    #[test]
    fn enrich_artist_round_trips_through_mock_host() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let body = r#"{
            "artist_id":"183-kanye-west",
            "critic":{"critic_score":73,"review_count":376,"success":true},
            "user":{"user_score":80,"rating_count":349666,"success":true},
            "albums":["MBDTF","808s"],
            "success":true
        }"#;
        let _h = MockHost::new().with_fixture_response(
            "https://test.example.com/artist/mb/164f0d73-1234-4e2c-8743-d77bf2191051",
            body,
        );
        let p = AotyProvider::new_for_test("fake-key", "https://test.example.com");
        let mut partial = PluginEntry {
            id: "kanye".into(),
            kind: EntryKind::Artist,
            source: "test".into(),
            ..Default::default()
        };
        partial.external_ids.insert(
            "musicbrainz".into(),
            "164f0d73-1234-4e2c-8743-d77bf2191051".into(),
        );
        let resp = match p.enrich(EnrichRequest {
            partial,
            prefer_id_source: None,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("enrich err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.entry.ratings.get("aoty_critic").copied(), Some(73.0));
        assert_eq!(resp.entry.ratings.get("aoty_user").copied(), Some(80.0));
        assert_eq!(
            resp.entry.rating_votes.get("aoty_critic").copied(),
            Some(376)
        );
    }

    #[test]
    fn enrich_album_round_trips_through_mock_host() {
        use stui_plugin_sdk::testing::MockHost;
        MockHost::reset();
        let body = r#"{
            "album_slug":"2546-dom-family-of-love",
            "title":"Family of Love",
            "artist":"DOM",
            "critic_score":73,"critic_score_precise":73.0643,"review_count":7,
            "user_score":67,"user_score_precise":67.4,"rating_count":9,
            "success":true
        }"#;
        let _h = MockHost::new()
            .with_fixture_response("https://test.example.com/album/mb/some-mb-id", body);
        let p = AotyProvider::new_for_test("fake-key", "https://test.example.com");
        let mut partial = PluginEntry {
            id: "fam".into(),
            kind: EntryKind::Album,
            source: "test".into(),
            ..Default::default()
        };
        partial
            .external_ids
            .insert("musicbrainz".into(), "some-mb-id".into());
        let resp = match p.enrich(EnrichRequest {
            partial,
            prefer_id_source: None,
            force_refresh: false,
        }) {
            PluginResult::Ok(r) => r,
            PluginResult::Err(e) => panic!("enrich err: {} {}", e.code, e.message),
        };
        assert_eq!(resp.entry.title, "Family of Love");
        assert_eq!(resp.entry.ratings.get("aoty_critic").copied(), Some(73.0));
        assert_eq!(resp.entry.rating_votes.get("aoty_user").copied(), Some(9));
    }

    // Note: 404→UNKNOWN_ID is covered by classify_http_404_maps_to_unknown_id;
    // MockHost fixtures only support 200, so the full enrich-404 round-trip
    // can't be expressed here without extending the SDK.
}
