//! MusicBrainz metadata provider — artists, release-groups/albums, recordings/tracks.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup, enrich,
//! get_artwork, get_credits}`. `related` is a declared stub per plugin.toml.
//!
//! ## User-Agent
//!
//! MB's public API requires a meaningful `User-Agent` header on every
//! request (RFC-compliant + project-identifying). The current SDK
//! `http_get` helper does NOT forward custom headers, so we currently
//! send plain GETs and rely on the runtime's default UA. Adding a
//! headers-capable HTTP helper to the SDK is tracked as BACKLOG — when
//! that lands we wire `USER_AGENT` through.

use serde::Deserialize;

use stui_plugin_sdk::{
    parse_manifest,
    error_codes, http_get,
    id_sources, normalize_crew_role,
    plugin_error, plugin_info,
    stui_export_catalog_plugin,
    ArtworkRequest, ArtworkResponse, ArtworkSize, ArtworkVariant,
    CastMember, CastRole,
    CatalogPlugin,
    CreditsRequest, CreditsResponse,
    CrewMember,
    EnrichRequest, EnrichResponse,
    EntryKind,
    InitContext,
    LookupRequest, LookupResponse,
    Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    SearchRequest, SearchResponse, SearchScope,
};

const WS_BASE:       &str = "https://musicbrainz.org/ws/2";
const COVER_ART_BASE: &str = "https://coverartarchive.org";

/// Project User-Agent per MB's terms of use. Not yet threaded through —
/// SDK gap, see module docstring.
#[allow(dead_code)]
const USER_AGENT: &str = concat!(
    "stui-musicbrainz-provider/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/Ozogorgor/stui )",
);

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct MusicbrainzPlugin {
    manifest: PluginManifest,
}

impl MusicbrainzPlugin {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self { manifest }
    }
}

impl Default for MusicbrainzPlugin {
    fn default() -> Self { Self::new() }
}

impl Plugin for MusicbrainzPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, _ctx: &InitContext) -> Result<(), PluginInitError> {
        // MusicBrainz's public API needs no key; init is a no-op.
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
                    503       => error_codes::RATE_LIMITED,  // MB serves 503 on over-limit
                    500..=599 => error_codes::TRANSIENT,
                    _         => error_codes::REMOTE_ERROR,
                };
                return PluginError { code: code.to_string(), message: format!("MB HTTP {status}: {body}") };
            }
        }
    }
    PluginError { code: error_codes::TRANSIENT.to_string(), message: err.to_string() }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, PluginError> {
    serde_json::from_str(body).map_err(|e| {
        plugin_error!("musicbrainz: parse error: {}", e);
        PluginError { code: error_codes::PARSE_ERROR.to_string(), message: format!("MB JSON parse failure: {e}") }
    })
}

/// Scope → (MB search endpoint, EntryKind) mapping.
fn scope_endpoint(scope: SearchScope) -> Result<(&'static str, EntryKind), PluginError> {
    match scope {
        SearchScope::Artist => Ok(("artist",       EntryKind::Artist)),
        // MB models "albums" as release-groups; that aggregates reissues /
        // regional pressings into one logical record.
        SearchScope::Album  => Ok(("release-group", EntryKind::Album)),
        SearchScope::Track  => Ok(("recording",    EntryKind::Track)),
        _ => Err(PluginError {
            code: error_codes::UNSUPPORTED_SCOPE.to_string(),
            message: "musicbrainz only supports artist, album, and track scopes".to_string(),
        }),
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for MusicbrainzPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let (endpoint, entry_kind) = match scope_endpoint(req.scope) {
            Ok(p) => p,
            Err(e) => return PluginResult::Err(e),
        };
        let query = req.query.trim();
        if query.is_empty() {
            // MB has no "trending" — empty query yields zero results.
            return PluginResult::ok(SearchResponse { items: vec![], total: 0 });
        }

        let limit  = if req.limit == 0 { 20 } else { req.limit.min(100) };
        let offset = req.page.saturating_sub(1).saturating_mul(limit);

        let url = format!(
            "{WS_BASE}/{endpoint}?query={}&fmt=json&limit={limit}&offset={offset}",
            urlencoding::encode(query),
        );
        plugin_info!("musicbrainz: search {endpoint} q='{}' limit={limit}", query);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        let items: Vec<PluginEntry> = match endpoint {
            "artist" => parse_artist_search(&body).into_iter().map(|a| a.into_entry(entry_kind)).collect(),
            "release-group" => parse_release_group_search(&body).into_iter().map(|g| g.into_entry(entry_kind)).collect(),
            _ /* recording */ => parse_recording_search(&body).into_iter().map(|r| r.into_entry(entry_kind)).collect(),
        };
        let total = items.len() as u32;
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        if req.id_source != id_sources::MUSICBRAINZ {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("mb lookup only supports musicbrainz id_source, got: {}", req.id_source),
            );
        }
        let (path, entry_kind, inc) = match req.kind {
            EntryKind::Artist => (format!("/artist/{}", urlencoding::encode(&req.id)),          EntryKind::Artist, "aliases+tags"),
            EntryKind::Album  => (format!("/release-group/{}", urlencoding::encode(&req.id)),   EntryKind::Album,  "artists+releases+tags"),
            EntryKind::Track  => (format!("/recording/{}", urlencoding::encode(&req.id)),       EntryKind::Track,  "artists+releases+tags"),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "mb lookup supports artist/album/track only",
                );
            }
        };
        let url = format!("{WS_BASE}{path}?inc={inc}&fmt=json");
        plugin_info!("musicbrainz: lookup {} ({:?})", req.id, req.kind);

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        let entry = match entry_kind {
            EntryKind::Artist => match parse_json::<ArtistDetail>(&body) {
                Ok(a) => a.into_entry(),
                Err(e) => return PluginResult::Err(e),
            },
            EntryKind::Album  => match parse_json::<ReleaseGroupDetail>(&body) {
                Ok(g) => g.into_entry(),
                Err(e) => return PluginResult::Err(e),
            },
            _ /* Track */     => match parse_json::<RecordingDetail>(&body) {
                Ok(r) => r.into_entry(),
                Err(e) => return PluginResult::Err(e),
            },
        };
        PluginResult::ok(LookupResponse { entry })
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        // Fast path: partial already carries an MB id.
        if let Some(id) = req.partial.external_ids.get(id_sources::MUSICBRAINZ) {
            let lookup_req = LookupRequest {
                id: id.clone(),
                id_source: id_sources::MUSICBRAINZ.to_string(),
                kind: req.partial.kind,
                locale: None,
            };
            return match self.lookup(lookup_req) {
                PluginResult::Ok(r)  => PluginResult::ok(EnrichResponse { entry: r.entry, confidence: 1.0 }),
                PluginResult::Err(e) => PluginResult::Err(e),
            };
        }

        let title = req.partial.title.trim();
        if title.is_empty() {
            return PluginResult::err(error_codes::INVALID_REQUEST, "enrich: partial.title is empty");
        }

        // Build a Lucene query combining title + artist hint when present. MB's
        // search endpoint scores these natively, so we don't need a second pass.
        let mut query_parts = vec![format!("\"{title}\"")];
        if let Some(artist) = req.partial.artist_name.as_deref().filter(|s| !s.is_empty()) {
            query_parts.push(format!("AND artist:\"{artist}\""));
        }
        if let Some(album) = req.partial.album_name.as_deref().filter(|s| !s.is_empty()) {
            if req.partial.kind == EntryKind::Track {
                query_parts.push(format!("AND release:\"{album}\""));
            }
        }
        let scope = match req.partial.kind {
            EntryKind::Artist => SearchScope::Artist,
            EntryKind::Album  => SearchScope::Album,
            _                 => SearchScope::Track,
        };
        let search_req = SearchRequest {
            query: query_parts.join(" "),
            scope,
            page: 1,
            limit: 5,
            per_scope_limit: None,
            locale: None,
        };
        let results = match self.search(search_req) {
            PluginResult::Ok(r)  => r.items,
            PluginResult::Err(e) => return PluginResult::Err(e),
        };

        let best = results.into_iter()
            .max_by(|a, b| enrich_score(&req.partial, a).partial_cmp(&enrich_score(&req.partial, b)).unwrap_or(std::cmp::Ordering::Equal));
        match best {
            Some(entry) => {
                let confidence = enrich_score(&req.partial, &entry);
                PluginResult::ok(EnrichResponse { entry, confidence })
            }
            None => PluginResult::err(error_codes::UNKNOWN_ID, "mb enrich: no match found"),
        }
    }

    fn get_artwork(&self, req: ArtworkRequest) -> PluginResult<ArtworkResponse> {
        if req.id_source != id_sources::MUSICBRAINZ {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("mb artwork: unsupported id_source: {}", req.id_source),
            );
        }
        // Cover Art Archive keys on release MBIDs, not release-group MBIDs —
        // but `/release-group/{mbid}` redirects to the "canonical" release if
        // one exists, which is the common case. Tracks use the parent release.
        let mbid = urlencoding::encode(&req.id);
        let path = match req.kind {
            EntryKind::Album => format!("/release-group/{mbid}"),
            EntryKind::Track => format!("/release/{mbid}"),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "mb artwork supports album (release-group) or track (release) only",
                );
            }
        };
        let url = format!("{COVER_ART_BASE}{path}");
        plugin_info!("musicbrainz: artwork {url}");

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        let caa: CoverArtArchive = match parse_json(&body) {
            Ok(c) => c,
            Err(e) => return PluginResult::Err(e),
        };

        let mut variants = Vec::new();
        for img in caa.images.into_iter() {
            if let Some(thumbs) = img.thumbnails {
                if let Some(url) = thumbs.small  { variants.push(ArtworkVariant { size: ArtworkSize::Thumbnail, url, mime: "image/jpeg".into(), width: Some(250), height: None }); }
                if let Some(url) = thumbs.large  { variants.push(ArtworkVariant { size: ArtworkSize::Standard,  url, mime: "image/jpeg".into(), width: Some(500), height: None }); }
            }
            if let Some(url) = img.image {
                variants.push(ArtworkVariant { size: ArtworkSize::HiRes, url, mime: "image/jpeg".into(), width: None, height: None });
            }
        }
        if !matches!(req.size, ArtworkSize::Any) {
            variants.sort_by_key(|v| if v.size == req.size { 0 } else { 1 });
        }
        PluginResult::ok(ArtworkResponse { variants })
    }

    fn get_credits(&self, req: CreditsRequest) -> PluginResult<CreditsResponse> {
        if req.id_source != id_sources::MUSICBRAINZ {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("mb credits: unsupported id_source: {}", req.id_source),
            );
        }
        // Credits live on the recording (per-track) or release (per-album) level.
        // For album scope we walk all recordings in the release-group; for track,
        // we just query the recording and its artist-relationships.
        let (path, inc) = match req.kind {
            EntryKind::Track => (format!("/recording/{}", urlencoding::encode(&req.id)),        "artist-credits+work-rels+artist-rels"),
            EntryKind::Album => (format!("/release-group/{}", urlencoding::encode(&req.id)),    "artist-credits+artist-rels"),
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "mb credits supports track or album kinds only",
                );
            }
        };
        let url = format!("{WS_BASE}{path}?inc={inc}&fmt=json");
        plugin_info!("musicbrainz: credits {url}");

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        // Shape is slightly different per endpoint; we reuse CreditsPayload
        // which tolerates both by treating the optional fields as absent.
        let payload: CreditsPayload = match parse_json(&body) {
            Ok(p) => p,
            Err(e) => return PluginResult::Err(e),
        };

        let cast: Vec<CastMember> = payload.artist_credit.unwrap_or_default().into_iter()
            .filter_map(|ac| {
                let name = ac.name.or_else(|| ac.artist.as_ref().map(|a| a.name.clone()))?;
                if name.is_empty() { None } else {
                    let mut m = CastMember {
                        name,
                        role: CastRole::FeaturedArtist,
                        character: None,
                        instrument: None,
                        billing_order: None,
                        external_ids: Default::default(),
                    };
                    if let Some(a) = ac.artist {
                        m.external_ids.insert(id_sources::MUSICBRAINZ.to_string(), a.id);
                    }
                    Some(m)
                }
            })
            .collect();

        let crew: Vec<CrewMember> = payload.relations.unwrap_or_default().into_iter()
            .filter_map(|rel| {
                let role_str = rel.rel_type?;
                let name = rel.artist.as_ref().map(|a| a.name.clone())?;
                if name.is_empty() { return None; }
                let mut m = CrewMember {
                    name,
                    role: normalize_crew_role(&role_str),
                    department: Some(role_str),
                    external_ids: Default::default(),
                };
                if let Some(a) = rel.artist {
                    m.external_ids.insert(id_sources::MUSICBRAINZ.to_string(), a.id);
                }
                Some(m)
            })
            .collect();

        PluginResult::ok(CreditsResponse { cast, crew })
    }

    // `related` falls through to the trait default → NOT_IMPLEMENTED.
    // See plugin.toml `related = { stub = true, reason = ... }`.
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Confidence score in [0.0, 1.0]: exact title adds 0.5 (prefix 0.2), matching
/// artist adds 0.3, matching album/release adds 0.2.
fn enrich_score(partial: &PluginEntry, candidate: &PluginEntry) -> f32 {
    let p_title = partial.title.to_lowercase();
    let c_title = candidate.title.to_lowercase();
    let title = if p_title == c_title {
        0.5
    } else if !p_title.is_empty() && c_title.starts_with(&p_title) {
        0.2
    } else {
        0.0
    };
    let artist = match (&partial.artist_name, &candidate.artist_name) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => 0.3,
        _ => 0.0,
    };
    let album = match (&partial.album_name, &candidate.album_name) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => 0.2,
        _ => 0.0,
    };
    title + artist + album
}

/// Flatten MB's `artist-credit` array into a display name like
/// `"Artist A feat. Artist B"`. MB sets `joinphrase` between credits.
fn join_artist_credit(ac: &[ArtistCredit]) -> Option<String> {
    if ac.is_empty() { return None; }
    let mut out = String::new();
    for c in ac {
        let name = c.name.clone().or_else(|| c.artist.as_ref().map(|a| a.name.clone())).unwrap_or_default();
        out.push_str(&name);
        if let Some(jp) = &c.joinphrase { out.push_str(jp); }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn first_year(date: Option<&str>) -> Option<u32> {
    date?.split('-').next()?.parse::<u32>().ok()
}

// ── Wire types ────────────────────────────────────────────────────────────────

// Search endpoints share a `{artists|release-groups|recordings: [...]}`
// top-level envelope. Each inner object varies.

fn parse_artist_search(body: &str) -> Vec<ArtistHit> {
    let env: ArtistSearchEnvelope = match parse_json(body) { Ok(e) => e, Err(_) => return Vec::new() };
    env.artists
}

fn parse_release_group_search(body: &str) -> Vec<ReleaseGroupHit> {
    let env: ReleaseGroupSearchEnvelope = match parse_json(body) { Ok(e) => e, Err(_) => return Vec::new() };
    env.release_groups
}

fn parse_recording_search(body: &str) -> Vec<RecordingHit> {
    let env: RecordingSearchEnvelope = match parse_json(body) { Ok(e) => e, Err(_) => return Vec::new() };
    env.recordings
}

#[derive(Debug, Deserialize)]
struct ArtistSearchEnvelope {
    #[serde(default)]
    artists: Vec<ArtistHit>,
}

#[derive(Debug, Deserialize)]
struct ReleaseGroupSearchEnvelope {
    #[serde(rename = "release-groups", default)]
    release_groups: Vec<ReleaseGroupHit>,
}

#[derive(Debug, Deserialize)]
struct RecordingSearchEnvelope {
    #[serde(default)]
    recordings: Vec<RecordingHit>,
}

#[derive(Debug, Deserialize, Clone)]
struct ArtistHit {
    id: String,
    #[serde(default)] name: String,
    #[serde(rename = "sort-name", default)] _sort_name: Option<String>,
    #[serde(default)] disambiguation: Option<String>,
    #[serde(default)] country: Option<String>,
    #[serde(default)] score: Option<u32>,
}

impl ArtistHit {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let mut entry = PluginEntry {
            id: format!("mb-{}", self.id),
            kind,
            source: "musicbrainz".to_string(),
            title: self.name.clone(),
            artist_name: Some(self.name),
            description: self.disambiguation.or(self.country),
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::MUSICBRAINZ.to_string(), self.id);
        let _ = self.score;
        entry
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseGroupHit {
    id: String,
    #[serde(default)] title: String,
    #[serde(rename = "first-release-date", default)] first_release_date: Option<String>,
    #[serde(rename = "artist-credit", default)] artist_credit: Vec<ArtistCredit>,
    #[serde(rename = "primary-type", default)] primary_type: Option<String>,
    #[serde(default)] disambiguation: Option<String>,
}

impl ReleaseGroupHit {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let artist = join_artist_credit(&self.artist_credit);
        let year = first_year(self.first_release_date.as_deref());
        // Cover Art Archive serves a 250px-wide thumbnail by MBID at a
        // stable URL. 404s gracefully when no art is available — the TUI
        // card falls back to its placeholder. Doing this here avoids a
        // second round-trip per result during search.
        let poster_url = Some(format!(
            "{COVER_ART_BASE}/release-group/{}/front-250",
            self.id,
        ));
        let mut entry = PluginEntry {
            id: format!("mb-{}", self.id),
            kind,
            source: "musicbrainz".to_string(),
            title: self.title.clone(),
            year,
            artist_name: artist,
            album_name: Some(self.title),
            genre: self.primary_type,
            description: self.disambiguation,
            poster_url,
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::MUSICBRAINZ.to_string(), self.id);
        entry
    }
}

#[derive(Debug, Deserialize)]
struct RecordingHit {
    id: String,
    #[serde(default)] title: String,
    #[serde(rename = "artist-credit", default)] artist_credit: Vec<ArtistCredit>,
    #[serde(default)] length: Option<u32>,     // milliseconds
    #[serde(default)] releases: Vec<ReleaseSummary>,
}

impl RecordingHit {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let artist = join_artist_credit(&self.artist_credit);
        let (album, year) = self.releases.first()
            .map(|r| (Some(r.title.clone()), first_year(r.date.as_deref())))
            .unwrap_or((None, None));
        let duration_sec = self.length.map(|ms| ms / 1000);
        let mut entry = PluginEntry {
            id: format!("mb-{}", self.id),
            kind,
            source: "musicbrainz".to_string(),
            title: self.title,
            year,
            artist_name: artist,
            album_name: album,
            duration: duration_sec,
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::MUSICBRAINZ.to_string(), self.id);
        entry
    }
}

#[derive(Debug, Deserialize, Clone)]
struct ReleaseSummary {
    #[serde(default)] title: String,
    #[serde(default)] date: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ArtistCredit {
    #[serde(default)] name: Option<String>,
    #[serde(default)] joinphrase: Option<String>,
    #[serde(default)] artist: Option<ArtistRef>,
}

#[derive(Debug, Deserialize, Clone)]
struct ArtistRef {
    id: String,
    #[serde(default)] name: String,
}

// Lookup payloads reuse the hit shapes but add a few detail fields.

#[derive(Debug, Deserialize)]
struct ArtistDetail {
    id: String,
    #[serde(default)] name: String,
    #[serde(default)] disambiguation: Option<String>,
    #[serde(default)] country: Option<String>,
    #[serde(rename = "life-span", default)] life_span: Option<LifeSpan>,
}

#[derive(Debug, Deserialize)]
struct LifeSpan {
    #[serde(default)] begin: Option<String>,
}

impl ArtistDetail {
    fn into_entry(self) -> PluginEntry {
        let year = first_year(self.life_span.as_ref().and_then(|l| l.begin.as_deref()));
        let mut entry = PluginEntry {
            id: format!("mb-{}", self.id),
            kind: EntryKind::Artist,
            source: "musicbrainz".to_string(),
            title: self.name.clone(),
            year,
            artist_name: Some(self.name),
            description: self.disambiguation.or(self.country),
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::MUSICBRAINZ.to_string(), self.id);
        entry
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseGroupDetail {
    id: String,
    #[serde(default)] title: String,
    #[serde(rename = "first-release-date", default)] first_release_date: Option<String>,
    #[serde(rename = "artist-credit", default)] artist_credit: Vec<ArtistCredit>,
    #[serde(rename = "primary-type", default)] primary_type: Option<String>,
    #[serde(default)] disambiguation: Option<String>,
    #[serde(default)] tags: Vec<Tag>,
}

impl ReleaseGroupDetail {
    fn into_entry(self) -> PluginEntry {
        let hit = ReleaseGroupHit {
            id: self.id.clone(),
            title: self.title,
            first_release_date: self.first_release_date,
            artist_credit: self.artist_credit,
            primary_type: self.primary_type,
            disambiguation: self.disambiguation,
        };
        let mut entry = hit.into_entry(EntryKind::Album);
        let tag_list: Vec<String> = self.tags.into_iter()
            .filter(|t| t.count.unwrap_or(0) > 0)
            .map(|t| t.name)
            .take(5)
            .collect();
        if !tag_list.is_empty() {
            // Append tag list to existing genre (primary-type).
            entry.genre = Some(match entry.genre {
                Some(g) => format!("{g} ({})", tag_list.join(", ")),
                None    => tag_list.join(", "),
            });
        }
        entry
    }
}

#[derive(Debug, Deserialize)]
struct RecordingDetail {
    id: String,
    #[serde(default)] title: String,
    #[serde(rename = "artist-credit", default)] artist_credit: Vec<ArtistCredit>,
    #[serde(default)] length: Option<u32>,
    #[serde(default)] releases: Vec<ReleaseSummary>,
    #[serde(default)] disambiguation: Option<String>,
}

impl RecordingDetail {
    fn into_entry(self) -> PluginEntry {
        let hit = RecordingHit {
            id: self.id,
            title: self.title,
            artist_credit: self.artist_credit,
            length: self.length,
            releases: self.releases,
        };
        let mut entry = hit.into_entry(EntryKind::Track);
        entry.description = self.disambiguation;
        entry
    }
}

#[derive(Debug, Deserialize)]
struct Tag {
    #[serde(default)] name: String,
    #[serde(default)] count: Option<u32>,
}

// Credits-payload shape covers both `/recording/{id}?inc=...` and
// `/release-group/{id}?inc=...` responses.

#[derive(Debug, Deserialize)]
struct CreditsPayload {
    #[serde(rename = "artist-credit", default)]
    artist_credit: Option<Vec<ArtistCredit>>,
    #[serde(default)]
    relations: Option<Vec<Relation>>,
}

#[derive(Debug, Deserialize)]
struct Relation {
    #[serde(rename = "type", default)] rel_type: Option<String>,
    #[serde(default)] artist: Option<ArtistRef>,
}

// Cover Art Archive envelope.

#[derive(Debug, Deserialize)]
struct CoverArtArchive {
    #[serde(default)]
    images: Vec<CoverArtImage>,
}

#[derive(Debug, Deserialize)]
struct CoverArtImage {
    #[serde(default)] image: Option<String>,
    #[serde(default)] thumbnails: Option<CoverThumbnails>,
}

#[derive(Debug, Deserialize)]
struct CoverThumbnails {
    #[serde(default)] small: Option<String>,
    #[serde(default)] large: Option<String>,
}

// ── WASM exports ──────────────────────────────────────────────────────────────

stui_export_catalog_plugin!(MusicbrainzPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn _p<T: Plugin>() {}
        fn _c<T: CatalogPlugin>() {}
        _p::<MusicbrainzPlugin>();
        _c::<MusicbrainzPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = MusicbrainzPlugin::new();
        assert_eq!(p.manifest().plugin.name, "musicbrainz");
    }

    #[test]
    fn scope_mapping_covers_artist_album_track() {
        assert_eq!(scope_endpoint(SearchScope::Artist).unwrap().0, "artist");
        assert_eq!(scope_endpoint(SearchScope::Album ).unwrap().0, "release-group");
        assert_eq!(scope_endpoint(SearchScope::Track ).unwrap().0, "recording");
    }

    #[test]
    fn scope_mapping_rejects_movie() {
        assert!(scope_endpoint(SearchScope::Movie).is_err());
    }

    #[test]
    fn first_year_handles_mb_date_variants() {
        assert_eq!(first_year(Some("1998")),       Some(1998));
        assert_eq!(first_year(Some("1998-04-03")), Some(1998));
        assert_eq!(first_year(Some("")),           None);
        assert_eq!(first_year(None),               None);
    }

    #[test]
    fn join_artist_credit_uses_joinphrase_between_names() {
        let ac = vec![
            ArtistCredit { name: Some("Queen".into()),     joinphrase: Some(" feat. ".into()), artist: None },
            ArtistCredit { name: Some("David Bowie".into()), joinphrase: None, artist: None },
        ];
        assert_eq!(join_artist_credit(&ac).as_deref(), Some("Queen feat. David Bowie"));
    }

    #[test]
    fn artist_hit_into_entry_falls_back_to_country_for_description() {
        let h = ArtistHit {
            id: "abc".into(),
            name: "Björk".into(),
            _sort_name: None,
            disambiguation: None,
            country: Some("IS".into()),
            score: Some(100),
        };
        let e = h.into_entry(EntryKind::Artist);
        assert_eq!(e.description.as_deref(), Some("IS"));
        assert_eq!(e.external_ids.get(id_sources::MUSICBRAINZ).map(String::as_str), Some("abc"));
    }

    #[test]
    fn release_group_into_entry_formats_year_and_artist() {
        let h = ReleaseGroupHit {
            id: "rg-id".into(),
            title: "OK Computer".into(),
            first_release_date: Some("1997-05-21".into()),
            artist_credit: vec![ArtistCredit { name: Some("Radiohead".into()), joinphrase: None, artist: None }],
            primary_type: Some("Album".into()),
            disambiguation: None,
        };
        let e = h.into_entry(EntryKind::Album);
        assert_eq!(e.year, Some(1997));
        assert_eq!(e.artist_name.as_deref(), Some("Radiohead"));
        assert_eq!(e.album_name.as_deref(), Some("OK Computer"));
        assert_eq!(e.genre.as_deref(), Some("Album"));
    }

    #[test]
    fn recording_into_entry_derives_seconds_from_milliseconds() {
        let r = RecordingHit {
            id: "rec-id".into(),
            title: "Karma Police".into(),
            artist_credit: vec![ArtistCredit { name: Some("Radiohead".into()), joinphrase: None, artist: None }],
            length: Some(261000),
            releases: vec![ReleaseSummary { title: "OK Computer".into(), date: Some("1997-05-21".into()) }],
        };
        let e = r.into_entry(EntryKind::Track);
        assert_eq!(e.duration, Some(261));
        assert_eq!(e.year, Some(1997));
        assert_eq!(e.album_name.as_deref(), Some("OK Computer"));
    }

    #[test]
    fn enrich_score_rewards_full_match() {
        let mut partial = PluginEntry { title: "Karma Police".into(), kind: EntryKind::Track, ..Default::default() };
        partial.artist_name = Some("Radiohead".into());
        partial.album_name  = Some("OK Computer".into());
        let mut full = partial.clone();
        full.id = "candidate-1".into();
        let mut partial_match = partial.clone();
        partial_match.album_name = Some("Pablo Honey".into());
        partial_match.id = "candidate-2".into();
        assert!(enrich_score(&partial, &full) > enrich_score(&partial, &partial_match));
    }

    #[test]
    fn user_agent_identifies_stui_and_repo() {
        assert!(USER_AGENT.contains("stui-musicbrainz-provider"));
        assert!(USER_AGENT.contains("github.com"));
    }
}
