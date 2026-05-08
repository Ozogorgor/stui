//! Last.fm / Libre.fm metadata provider — music discovery and charts.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, enrich}`. Lookup is
//! intentionally skipped (see plugin.toml for the rationale).
//!
//! ## API key
//!
//! Required. Read from `InitContext.config["api_key"]` at `Plugin::init`.
//! Fallback: `LASTFM_API_KEY` env var via `cache_get("__env:...")`.

use std::sync::OnceLock;

use serde::Deserialize;

use stui_plugin_sdk::{
    cache_get, error_codes, http_get, id_sources, parse_manifest, plugin_error, plugin_info,
    stui_export_catalog_plugin, CatalogPlugin, EnrichRequest, EnrichResponse, EntryKind,
    InitContext, Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    SearchRequest, SearchResponse, SearchScope,
};

const API_BASE: &str = "https://ws.audioscrobbler.com/2.0";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct LastfmPlugin {
    manifest: PluginManifest,
    api_key: OnceLock<String>,
}

impl LastfmPlugin {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self {
            manifest,
            api_key: OnceLock::new(),
        }
    }

    #[cfg(test)]
    pub fn new_for_test(api_key: &str) -> Self {
        let inst = Self::new();
        let _ = inst.api_key.set(api_key.to_string());
        inst
    }

    fn api_key(&self) -> Result<&str, PluginError> {
        if let Some(k) = self.api_key.get() {
            return Ok(k.as_str());
        }
        let env_key = cache_get("__env:LASTFM_API_KEY").unwrap_or_default();
        if env_key.is_empty() {
            return Err(PluginError {
                code: error_codes::INVALID_REQUEST.to_string(),
                message: "Last.fm api_key not configured".to_string(),
            });
        }
        Ok(self.api_key.get_or_init(|| env_key).as_str())
    }
}

impl Default for LastfmPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for LastfmPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn init(&mut self, ctx: &InitContext) -> Result<(), PluginInitError> {
        let key = ctx
            .config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.env.get("LASTFM_API_KEY").cloned())
            .unwrap_or_default();
        if key.is_empty() {
            return Err(PluginInitError::MissingConfig {
                fields: vec!["api_key".to_string()],
                hint: Some("Free at last.fm/api/account/create".to_string()),
            });
        }
        let _ = self.api_key.set(key);
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
                    404 => error_codes::UNKNOWN_ID,
                    429 => error_codes::RATE_LIMITED,
                    500..=599 => error_codes::TRANSIENT,
                    _ => error_codes::REMOTE_ERROR,
                };
                return PluginError {
                    code: code.to_string(),
                    message: format!("lastfm HTTP {status}: {body}"),
                };
            }
        }
    }
    PluginError {
        code: error_codes::TRANSIENT.to_string(),
        message: err.to_string(),
    }
}

/// Last.fm error-envelope shape: `{"error": N, "message": "..."}`. When
/// the API rejects a request (invalid key, rate limit, method mismatch)
/// it returns this instead of the expected result envelope, and a naive
/// `serde_json::from_str::<TrackSearchResponse>()` fails with "missing
/// field `tracks`" — useless for triage. Detecting the envelope first
/// gives us "lastfm API error 10: Invalid API key" which is actionable.
#[derive(Deserialize)]
struct LastfmErrorEnvelope {
    error: i32,
    #[serde(default)]
    message: Option<String>,
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, PluginError> {
    if let Ok(env) = serde_json::from_str::<LastfmErrorEnvelope>(body) {
        let msg = env.message.as_deref().unwrap_or("(no message)");
        plugin_error!("lastfm: API error {}: {}", env.error, msg);
        return Err(PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("lastfm API error {}: {}", env.error, msg),
        });
    }
    serde_json::from_str(body).map_err(|e| {
        plugin_error!("lastfm: parse error: {}", e);
        PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("lastfm JSON parse failure: {e}"),
        }
    })
}

fn pick_image(images: Vec<Image>) -> Option<String> {
    // Prefer extralarge → large → mega → anything non-empty.
    for want in ["extralarge", "large", "mega"] {
        if let Some(u) = images
            .iter()
            .find(|i| i.size == want)
            .map(|i| i.text.clone())
            .filter(|s| !s.is_empty())
        {
            return Some(u);
        }
    }
    images.into_iter().map(|i| i.text).find(|s| !s.is_empty())
}

/// Format listeners/playcount counts into a human-friendly description
/// fragment. Splits numeric strings into the usual 1,234 thousands form.
fn format_stats(listeners: Option<&str>, playcount: Option<&str>) -> Option<String> {
    let mut parts = Vec::<String>::new();
    if let Some(v) = listeners.filter(|s| !s.is_empty()) {
        parts.push(format!("{} listeners", thousands(v)));
    }
    if let Some(v) = playcount.filter(|s| !s.is_empty()) {
        parts.push(format!("{} plays", thousands(v)));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn thousands(raw: &str) -> String {
    match raw.trim().parse::<u64>() {
        Ok(n) => {
            let s = n.to_string();
            let bytes = s.as_bytes();
            let mut out = String::with_capacity(s.len() + s.len() / 3);
            for (i, b) in bytes.iter().enumerate() {
                if i > 0 && (bytes.len() - i) % 3 == 0 {
                    out.push(',');
                }
                out.push(*b as char);
            }
            out
        }
        Err(_) => raw.to_string(),
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for LastfmPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let entry_kind = match req.scope {
            SearchScope::Artist => EntryKind::Artist,
            SearchScope::Album => EntryKind::Album,
            SearchScope::Track => EntryKind::Track,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "lastfm only supports artist, album, and track scopes",
                );
            }
        };
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };

        let query = req.query.trim();
        let limit = if req.limit == 0 {
            20
        } else {
            req.limit.min(50) as usize
        };

        // Default-state Album browse (no query) needs special handling:
        // Last.fm has no chart.gettopalbums endpoint, so we aggregate
        // tag.gettopalbums across a small list of popular seed tags. This
        // shape stays as the "All / no filter" code path even after the
        // future genre-filter dropdown lands — that just routes a single
        // user-selected tag through the same per-tag fetch.
        if matches!(req.scope, SearchScope::Album) && query.is_empty() {
            let items =
                aggregate_top_albums_by_tags(&api_key, AGGREGATE_SEED_TAGS, limit, entry_kind);
            let total = items.len() as u32;
            return PluginResult::ok(SearchResponse { items, total });
        }

        let url = if query.is_empty() {
            // Artist + Track charts still use the chart.* endpoints.
            let method = match req.scope {
                SearchScope::Artist => "chart.gettopartists",
                _ => "chart.gettoptracks",
            };
            format!("{API_BASE}?method={method}&api_key={api_key}&format=json&limit={limit}")
        } else {
            let method = match req.scope {
                SearchScope::Artist => "artist.search",
                SearchScope::Album => "album.search",
                _ => "track.search",
            };
            let param = match req.scope {
                SearchScope::Artist => "artist",
                SearchScope::Album => "album",
                _ => "track",
            };
            format!(
                "{API_BASE}?method={method}&{param}={}&api_key={api_key}&format=json&limit={limit}",
                urlencoding::encode(query),
            )
        };
        plugin_info!(
            "lastfm: search '{}' (scope={:?}, limit={limit})",
            query,
            req.scope
        );

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };

        // SearchScope::Album + empty query is handled by the early return above.
        let parsed: Result<Vec<PluginEntry>, PluginError> = match (req.scope, query.is_empty()) {
            (SearchScope::Artist, true) => parse_top_artists(&body),
            (SearchScope::Artist, false) => parse_artist_search(&body, entry_kind),
            (SearchScope::Album, false) => parse_album_search(&body, entry_kind),
            (SearchScope::Track, true) => parse_top_tracks(&body, entry_kind),
            (SearchScope::Track, false) => parse_track_search(&body, entry_kind),
            _ => parse_top_tracks(&body, entry_kind),
        };
        let items = match parsed {
            Ok(v) => v,
            Err(e) => return PluginResult::Err(e),
        };
        let total = items.len() as u32;
        PluginResult::ok(SearchResponse { items, total })
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        let title = req.partial.title.trim();
        if title.is_empty() {
            return PluginResult::err(
                error_codes::INVALID_REQUEST,
                "enrich: partial.title is empty",
            );
        }

        // Album fast path: when we have both album title and artist
        // we can hit album.getInfo directly and pull listeners +
        // playcount + tags + wiki summary in a single round-trip.
        // Listeners gets normalized to a synthetic 0-5 rating since
        // last.fm has no actual rating field — see
        // synth_rating_from_listeners for the formula.
        if matches!(req.partial.kind, EntryKind::Album) {
            if let Some(artist) = req
                .partial
                .artist_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return self.enrich_album_via_get_info(title, artist);
            }
        }

        // Fallback for artist/track or albums without an artist hint:
        // fan out a search and pick the highest-confidence match.
        let scope = match req.partial.kind {
            EntryKind::Artist => SearchScope::Artist,
            EntryKind::Album => SearchScope::Album,
            _ => SearchScope::Track,
        };
        let search_req = SearchRequest {
            query: title.to_string(),
            scope,
            page: 1,
            limit: 10,
            per_scope_limit: None,
            locale: None,
        };
        let candidates = match self.search(search_req) {
            PluginResult::Ok(r) => r.items,
            PluginResult::Err(e) => return PluginResult::Err(e),
        };

        let best = candidates.into_iter().max_by(|a, b| {
            enrich_score(&req.partial, a)
                .partial_cmp(&enrich_score(&req.partial, b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        match best {
            Some(entry) => {
                let confidence = enrich_score(&req.partial, &entry);
                PluginResult::ok(EnrichResponse { entry, confidence })
            }
            None => PluginResult::err(error_codes::UNKNOWN_ID, "lastfm: no enrich match found"),
        }
    }
}

impl LastfmPlugin {
    /// Album-specific enrich via `album.getInfo`. Returns a PluginEntry
    /// with synthetic rating (from listeners), genre (top tag), and
    /// description (wiki.summary) populated. The runtime's
    /// music_enrich layer prefers Discogs's real community rating
    /// when present and falls back to this synthetic one when Discogs
    /// has no data — coverage from this path is much higher (every
    /// album with any listeners) than from Discogs (only releases
    /// with community votes).
    fn enrich_album_via_get_info(&self, album: &str, artist: &str) -> PluginResult<EnrichResponse> {
        let api_key = match self.api_key() {
            Ok(k) => k.to_string(),
            Err(e) => return PluginResult::Err(e),
        };
        // `autocorrect=1` lets last.fm fix common artist/album spelling
        // variants ("the beatles" → "The Beatles"), reducing
        // unknown-id errors on the cold path.
        let url = format!(
            "{API_BASE}?method=album.getInfo&api_key={api_key}&artist={a}&album={al}&format=json&autocorrect=1",
            a  = urlencoding::encode(artist),
            al = urlencoding::encode(album),
        );
        plugin_info!(
            "lastfm: album.getInfo artist='{}' album='{}'",
            artist,
            album
        );

        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(classify_http_err(&e)),
        };
        let resp: AlbumInfoEnvelope = match parse_json(&body) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        let info = match resp.album {
            Some(a) => a,
            None => {
                return PluginResult::err(
                    error_codes::UNKNOWN_ID,
                    format!("lastfm: album.getInfo returned no album for '{album}' / '{artist}'"),
                )
            }
        };

        let listeners = info
            .listeners
            .as_deref()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let rating = if listeners > 0 {
            Some(synth_rating_from_listeners(listeners))
        } else {
            None
        };
        // Last.fm's top tags are user-contributed and routinely include
        // non-genre noise: "5 stars", "favorite", "owned", "2021",
        // "seen-live", "vinyl", etc. Walk the list in count order and
        // pick the first tag that passes pick_genre_tag's heuristic
        // filter. If none qualify we leave genre empty rather than
        // surface a star-rating string as the card's genre badge.
        let genre = info.tags.as_ref().and_then(|t| pick_genre_tag(&t.tag));
        let description = info
            .wiki
            .as_ref()
            .and_then(|w| w.summary.clone())
            .filter(|s| !s.is_empty())
            // last.fm's wiki.summary trails with a "Read more on Last.fm"
            // link tag — strip it for cleaner card text.
            .map(|s| s.split("<a href=").next().unwrap_or(&s).trim().to_string());
        let poster_url = info.image.as_ref().and_then(|imgs| {
            imgs.iter().rev().find_map(|i| {
                if i.text.is_empty() {
                    None
                } else {
                    Some(i.text.clone())
                }
            })
        });

        let mut entry = PluginEntry {
            id: format!("lastfm-album-{}-{}", artist, album),
            kind: EntryKind::Album,
            source: "lastfm".to_string(),
            title: info.name.unwrap_or_else(|| album.to_string()),
            year: None,
            artist_name: Some(info.artist.unwrap_or_else(|| artist.to_string())),
            album_name: Some(album.to_string()),
            genre,
            description,
            poster_url,
            rating,
            ..Default::default()
        };
        if let Some(mbid) = info.mbid.filter(|s| !s.is_empty()) {
            entry
                .external_ids
                .insert(id_sources::MUSICBRAINZ.to_string(), mbid);
        }

        // Confidence is high when we have a populated rating signal
        // (album was matched and has scrobbles); lower if last.fm
        // returned the album shell with no listeners.
        let confidence = if rating.is_some() { 0.9 } else { 0.5 };
        PluginResult::ok(EnrichResponse { entry, confidence })
    }
}

/// Normalize last.fm `listeners` count to a 0-5 star rating using a
/// log10 scale. Justified by the long-tail distribution of listeners:
/// most albums sit in the 1k-100k range, popular ones reach millions,
/// only a handful exceed 10M. Linear scaling would crush all but the
/// mega-hits to ~0; log scaling gives meaningful spread across the
/// realistic range.
///
/// Formula: `clamp(log10(listeners + 1) - 2, 0, 5)`
///
/// | listeners | rating |
/// |-----------|--------|
/// | 100       | 0.0    |
/// | 1,000     | 1.0    |
/// | 10,000    | 2.0    |
/// | 100,000   | 3.0    |
/// | 1,000,000 | 4.0    |
/// | 10,000,000| 5.0    |
///
/// Anchor at `listeners=100,000 → 3.0` matches the intuitive "mid-tier
/// popular" feeling — albums most active listeners would recognize.
fn synth_rating_from_listeners(listeners: u64) -> f32 {
    let log = ((listeners + 1) as f64).log10();
    let raw = log - 2.0;
    raw.clamp(0.0, 5.0) as f32
}

/// Walk lastfm's top-tags list (already ordered by community vote
/// count) and return the first tag that looks like an actual genre.
/// Last.fm tags are user-submitted and the list routinely contains
/// non-genre noise: rating-style ("5 stars", "five stars"), year
/// stamps ("2021", "1970s"), decade markers ("80s", "90s"),
/// personal-state ("favorite", "owned", "wishlist"), and
/// concert-tracking ("seen-live"). When the top tag is one of those,
/// we want to fall through to whatever genre-shaped tag came next.
fn pick_genre_tag(tags: &[AlbumTag]) -> Option<String> {
    tags.iter()
        .map(|t| t.name.trim())
        .find(|name| !name.is_empty() && is_genre_like(name))
        .map(|s| s.to_string())
}

/// Heuristic: does this tag look like a genre rather than a personal
/// marker, rating, year, or other lastfm-tagging artifact?
fn is_genre_like(tag: &str) -> bool {
    let lower = tag.to_lowercase();
    let lower = lower.trim();
    if lower.is_empty() {
        return false;
    }

    // Star ratings: "5 stars", "four stars", "5/5", "10/10".
    if lower.contains("star") {
        let rest = lower.replace("star", "").replace("s", "");
        if rest.trim().is_empty()
            || rest
                .trim()
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_whitespace())
        {
            return false;
        }
    }
    if lower.contains('/')
        && lower
            .chars()
            .all(|c| c.is_ascii_digit() || c == '/' || c.is_whitespace())
    {
        return false;
    }

    // 4-digit year stamp.
    if lower.len() == 4 && lower.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    // Decade markers: "80s", "1990s", "70's", etc. We strip optional
    // century prefix and trailing 's' / "'s" then check the residue
    // is just two digits — if so, it's a decade tag.
    let decade_check = lower
        .trim_end_matches("'s")
        .trim_end_matches('s')
        .trim_start_matches("19")
        .trim_start_matches("20");
    if decade_check.len() == 2 && decade_check.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    // Personal-state / opinion / format / event markers. Each is
    // matched as the entire trimmed tag (lowercased) since these
    // appear standalone, not embedded in compound genre names.
    const PERSONAL_TAGS: &[&str] = &[
        "favorite",
        "favorites",
        "favourite",
        "favourites",
        "love",
        "loved",
        "love it",
        "loves",
        "best",
        "best of",
        "best ever",
        "best albums",
        "great",
        "good",
        "cool",
        "amazing",
        "awesome",
        "perfect",
        "epic",
        "masterpiece",
        "classic",
        "underrated",
        "overrated",
        "must hear",
        "owned",
        "own",
        "have",
        "i own this",
        "wishlist",
        "want",
        "to listen",
        "to listen to",
        "to buy",
        "checked",
        "heard",
        "listened",
        "my albums",
        "my favorites",
        "my favourites",
        "my collection",
        "albums i own",
        "albums i love",
        "seen live",
        "seen-live",
        "concert",
        "live",
        "vinyl",
        "cd",
        "cassette",
        "tape",
        "english",
        "spanish",
        "french",
        "german",
        "italian",
        "japanese",
        "male vocalists",
        "female vocalists",
        "male vocalist",
        "female vocalist",
        "albums",
    ];
    if PERSONAL_TAGS.iter().any(|p| *p == lower) {
        return false;
    }

    true
}

// ── album.getInfo response ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AlbumInfoEnvelope {
    #[serde(default)]
    album: Option<AlbumInfo>,
}

#[derive(Debug, Deserialize)]
struct AlbumInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    mbid: Option<String>,
    #[serde(default)]
    listeners: Option<String>,
    #[serde(default)]
    image: Option<Vec<AlbumImage>>,
    #[serde(default)]
    tags: Option<AlbumTags>,
    #[serde(default)]
    wiki: Option<AlbumWiki>,
}

#[derive(Debug, Deserialize)]
struct AlbumImage {
    #[serde(rename = "#text", default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct AlbumTags {
    #[serde(default)]
    tag: Vec<AlbumTag>,
}

#[derive(Debug, Deserialize)]
struct AlbumTag {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct AlbumWiki {
    #[serde(default)]
    summary: Option<String>,
}

/// Enrich-confidence heuristic [0.0, 1.0]:
/// - +0.6 on case-insensitive exact title match (else +0.3 if candidate starts with it)
/// - +0.4 if both sides carry an artist_name and they match case-insensitively
fn enrich_score(partial: &PluginEntry, candidate: &PluginEntry) -> f32 {
    let pt = partial.title.to_lowercase();
    let ct = candidate.title.to_lowercase();
    let title = if pt == ct {
        0.6
    } else if !pt.is_empty() && ct.starts_with(&pt) {
        0.3
    } else {
        0.0
    };
    let artist = match (&partial.artist_name, &candidate.artist_name) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => 0.4,
        _ => 0.0,
    };
    title + artist
}

// ── Aggregated album browse (seed tags) ──────────────────────────────────────

/// Default seed tags for the album-browse "All" / no-filter state. When
/// the future genre-filter dropdown lands, a user-selected tag bypasses
/// this list and queries `tag.gettopalbums` for that one tag. The list
/// is intentionally short to keep the worst-case HTTP fan-out small.
const AGGREGATE_SEED_TAGS: &[&str] = &["rock", "pop", "indie", "electronic", "hip-hop"];

/// Fan out `tag.gettopalbums` across `tags`, merge results, dedup by
/// `(artist, title)` (preserves the input order so seed-tag priority is
/// respected). Failures from individual tags are swallowed so one bad
/// tag doesn't kill the whole browse.
fn aggregate_top_albums_by_tags(
    api_key: &str,
    tags: &[&str],
    per_tag_limit: usize,
    kind: EntryKind,
) -> Vec<PluginEntry> {
    let mut out: Vec<PluginEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for tag in tags {
        let url = format!(
            "{API_BASE}?method=tag.gettopalbums&tag={}&api_key={api_key}&format=json&limit={per_tag_limit}",
            urlencoding::encode(tag),
        );
        plugin_info!("lastfm: tag.gettopalbums tag='{tag}' limit={per_tag_limit}");
        let body = match http_get(&url) {
            Ok(b) => b,
            Err(e) => {
                plugin_info!("lastfm: tag '{tag}' failed: {e}");
                continue;
            }
        };
        let entries = match parse_tag_top_albums(&body, kind) {
            Ok(v) => v,
            Err(e) => {
                plugin_info!("lastfm: tag '{tag}' parse failed: {}", e.message);
                continue;
            }
        };
        for e in entries {
            let key = match (&e.artist_name, &e.album_name) {
                (Some(a), Some(t)) => format!("{}|{}", a.to_lowercase(), t.to_lowercase()),
                _ => e.title.to_lowercase(),
            };
            if seen.insert(key) {
                out.push(e);
            }
        }
    }
    out
}

// ── Search-response parsers ───────────────────────────────────────────────────

fn parse_track_search(body: &str, kind: EntryKind) -> Result<Vec<PluginEntry>, PluginError> {
    let resp: TrackSearchResponse = parse_json(body)?;
    Ok(resp
        .results
        .trackmatches
        .track
        .into_iter()
        .filter_map(|t| t.into_entry(kind))
        .collect())
}

fn parse_top_tracks(body: &str, kind: EntryKind) -> Result<Vec<PluginEntry>, PluginError> {
    let resp: ChartResponse = parse_json(body)?;
    Ok(resp
        .tracks
        .track
        .into_iter()
        .filter_map(|t| t.into_entry(kind))
        .collect())
}

fn parse_artist_search(body: &str, kind: EntryKind) -> Result<Vec<PluginEntry>, PluginError> {
    let resp: ArtistSearchResponse = parse_json(body)?;
    Ok(resp
        .results
        .artistmatches
        .artist
        .into_iter()
        .filter_map(|a| a.into_entry(kind))
        .collect())
}

fn parse_album_search(body: &str, kind: EntryKind) -> Result<Vec<PluginEntry>, PluginError> {
    let resp: AlbumSearchResponse = parse_json(body)?;
    Ok(resp
        .results
        .albummatches
        .album
        .into_iter()
        .filter_map(|a| a.into_entry(kind))
        .collect())
}

fn parse_top_artists(body: &str) -> Result<Vec<PluginEntry>, PluginError> {
    let resp: TopArtistsResponse = parse_json(body)?;
    Ok(resp
        .artists
        .artist
        .into_iter()
        .filter_map(|a| a.into_entry(EntryKind::Artist))
        .collect())
}

fn parse_tag_top_albums(body: &str, kind: EntryKind) -> Result<Vec<PluginEntry>, PluginError> {
    let resp: TagTopAlbumsResponse = parse_json(body)?;
    Ok(resp
        .albums
        .album
        .into_iter()
        .filter_map(|a| a.into_entry(kind))
        .collect())
}

// ── API types ─────────────────────────────────────────────────────────────────

// Track search: results.trackmatches.track[]
#[derive(Debug, Deserialize)]
struct TrackSearchResponse {
    results: TrackSearchResults,
}
#[derive(Debug, Deserialize)]
struct TrackSearchResults {
    trackmatches: TrackMatches,
}
#[derive(Debug, Deserialize)]
struct TrackMatches {
    #[serde(default)]
    track: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Track {
    #[serde(default)]
    name: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    image: Vec<Image>,
    #[serde(default)]
    listeners: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Image {
    #[serde(rename = "#text", default)]
    text: String,
    #[serde(default)]
    size: String,
}

impl Track {
    fn into_entry(self, kind: EntryKind) -> Option<PluginEntry> {
        if self.name.is_empty() {
            return None;
        }
        let desc = format_description(
            &self.artist,
            self.album.as_deref(),
            format_stats(self.listeners.as_deref(), None).as_deref(),
        );
        Some(PluginEntry {
            id: make_id(&self.artist, &self.name),
            kind,
            source: "lastfm".to_string(),
            title: self.name,
            artist_name: opt_non_empty(self.artist),
            album_name: self.album,
            poster_url: pick_image(self.image),
            description: desc,
            ..Default::default()
        })
    }
}

// Chart (top-tracks): tracks.track[]
#[derive(Debug, Deserialize)]
struct ChartResponse {
    tracks: ChartTracks,
}
#[derive(Debug, Deserialize)]
struct ChartTracks {
    #[serde(default)]
    track: Vec<ChartTrack>,
}

#[derive(Debug, Deserialize)]
struct ChartTrack {
    #[serde(default)]
    name: String,
    #[serde(default)]
    artist: ChartArtistRef,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    image: Vec<Image>,
    #[serde(default)]
    listeners: Option<String>,
    #[serde(default)]
    playcount: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ChartArtistRef {
    #[serde(default)]
    name: String,
}

impl ChartTrack {
    fn into_entry(self, kind: EntryKind) -> Option<PluginEntry> {
        if self.name.is_empty() {
            return None;
        }
        let desc = format_description(
            &self.artist.name,
            self.album.as_deref(),
            format_stats(self.listeners.as_deref(), self.playcount.as_deref()).as_deref(),
        );
        Some(PluginEntry {
            id: make_id(&self.artist.name, &self.name),
            kind,
            source: "lastfm".to_string(),
            title: self.name,
            artist_name: opt_non_empty(self.artist.name),
            album_name: self.album,
            poster_url: pick_image(self.image),
            description: desc,
            ..Default::default()
        })
    }
}

// Artist search: results.artistmatches.artist[]
#[derive(Debug, Deserialize)]
struct ArtistSearchResponse {
    results: ArtistSearchResults,
}
#[derive(Debug, Deserialize)]
struct ArtistSearchResults {
    artistmatches: ArtistMatches,
}
#[derive(Debug, Deserialize)]
struct ArtistMatches {
    #[serde(default)]
    artist: Vec<Artist>,
}

#[derive(Debug, Deserialize)]
struct Artist {
    #[serde(default)]
    name: String,
    #[serde(default)]
    image: Vec<Image>,
    #[serde(default)]
    listeners: Option<String>,
    #[serde(default)]
    playcount: Option<String>,
}

impl Artist {
    fn into_entry(self, kind: EntryKind) -> Option<PluginEntry> {
        if self.name.is_empty() {
            return None;
        }
        let desc = format_stats(self.listeners.as_deref(), self.playcount.as_deref());
        Some(PluginEntry {
            id: format!("lastfm-artist-{}", slugify(&self.name)),
            kind,
            source: "lastfm".to_string(),
            title: self.name.clone(),
            artist_name: Some(self.name),
            poster_url: pick_image(self.image),
            description: desc,
            ..Default::default()
        })
    }
}

// Album search: results.albummatches.album[]
#[derive(Debug, Deserialize)]
struct AlbumSearchResponse {
    results: AlbumSearchResults,
}
#[derive(Debug, Deserialize)]
struct AlbumSearchResults {
    albummatches: AlbumMatches,
}
#[derive(Debug, Deserialize)]
struct AlbumMatches {
    #[serde(default)]
    album: Vec<Album>,
}

#[derive(Debug, Deserialize)]
struct Album {
    #[serde(default)]
    name: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    image: Vec<Image>,
}

impl Album {
    fn into_entry(self, kind: EntryKind) -> Option<PluginEntry> {
        if self.name.is_empty() {
            return None;
        }
        let desc = if self.artist.is_empty() {
            None
        } else {
            Some(format!("by {}", self.artist))
        };
        Some(PluginEntry {
            id: make_id(&self.artist, &self.name),
            kind,
            source: "lastfm".to_string(),
            title: self.name.clone(),
            artist_name: opt_non_empty(self.artist),
            album_name: Some(self.name),
            poster_url: pick_image(self.image),
            description: desc,
            ..Default::default()
        })
    }
}

// Charts: top artists wraps as artists.artist[]
#[derive(Debug, Deserialize)]
struct TopArtistsResponse {
    artists: TopArtistsWrap,
}
#[derive(Debug, Deserialize)]
struct TopArtistsWrap {
    #[serde(default)]
    artist: Vec<Artist>,
}

// tag.gettopalbums: {"albums": {"album": [{"name", "artist": {"name"}, "image": [...], "mbid"}]}}
#[derive(Debug, Deserialize)]
struct TagTopAlbumsResponse {
    albums: TagTopAlbumsWrap,
}
#[derive(Debug, Deserialize)]
struct TagTopAlbumsWrap {
    #[serde(default)]
    album: Vec<TagAlbum>,
}

#[derive(Debug, Deserialize)]
struct TagAlbum {
    #[serde(default)]
    name: String,
    #[serde(default)]
    artist: TagAlbumArtist,
    #[serde(default)]
    image: Vec<Image>,
    #[serde(default)]
    mbid: String,
}

#[derive(Debug, Default, Deserialize)]
struct TagAlbumArtist {
    #[serde(default)]
    name: String,
}

impl TagAlbum {
    fn into_entry(self, kind: EntryKind) -> Option<PluginEntry> {
        if self.name.is_empty() {
            return None;
        }
        let artist = self.artist.name;
        let desc = if artist.is_empty() {
            None
        } else {
            Some(format!("by {artist}"))
        };
        // Lastfm's image URLs are usually populated for albums. When
        // they're not (rare), fall back to Cover Art Archive via the
        // release-group MBID. CAA 404s gracefully if no art exists.
        let mut poster = pick_image(self.image);
        if poster.is_none() && !self.mbid.is_empty() {
            poster = Some(format!(
                "https://coverartarchive.org/release-group/{}/front-250",
                self.mbid,
            ));
        }
        let mut entry = PluginEntry {
            id: make_id(&artist, &self.name),
            kind,
            source: "lastfm".to_string(),
            title: self.name.clone(),
            artist_name: opt_non_empty(artist),
            album_name: Some(self.name),
            poster_url: poster,
            description: desc,
            ..Default::default()
        };
        if !self.mbid.is_empty() {
            entry
                .external_ids
                .insert(id_sources::MUSICBRAINZ.to_string(), self.mbid);
        }
        Some(entry)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn opt_non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn make_id(artist: &str, title: &str) -> String {
    format!("lastfm-{}", slugify(&format!("{artist}-{title}")))
}

fn slugify(s: &str) -> String {
    urlencoding::encode(s.trim()).to_string()
}

/// Build a description string combining artist, album, and stats; any
/// individual part may be absent.
fn format_description(artist: &str, album: Option<&str>, stats: Option<&str>) -> Option<String> {
    let mut parts = Vec::<String>::new();
    if !artist.is_empty() {
        parts.push(format!("by {artist}"));
    }
    if let Some(a) = album.filter(|a| !a.is_empty()) {
        parts.push(format!("from {a}"));
    }
    if let Some(s) = stats {
        parts.push(s.to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

// ── WASM exports ──────────────────────────────────────────────────────────────

impl stui_plugin_sdk::StreamProvider for LastfmPlugin {}

stui_export_catalog_plugin!(LastfmPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn _p<T: Plugin>() {}
        fn _c<T: CatalogPlugin>() {}
        _p::<LastfmPlugin>();
        _c::<LastfmPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = LastfmPlugin::new();
        assert_eq!(p.manifest().plugin.name, "lastfm");
    }

    #[test]
    fn thousands_formats_large_numbers() {
        assert_eq!(thousands("1234567"), "1,234,567");
        assert_eq!(thousands("999"), "999");
        assert_eq!(thousands("1000"), "1,000");
        assert_eq!(thousands("abc"), "abc"); // non-numeric passes through unchanged
    }

    #[test]
    fn format_stats_uses_thousands_separator() {
        let s = format_stats(Some("1234567"), Some("89000"));
        assert_eq!(s.as_deref(), Some("1,234,567 listeners · 89,000 plays"));
    }

    #[test]
    fn format_stats_returns_none_when_all_absent() {
        assert_eq!(format_stats(None, None), None);
        assert_eq!(format_stats(Some(""), None), None);
    }

    #[test]
    fn pick_image_prefers_extralarge() {
        let imgs = vec![
            Image {
                text: "small.jpg".into(),
                size: "small".into(),
            },
            Image {
                text: "large.jpg".into(),
                size: "large".into(),
            },
            Image {
                text: "extralarge.jpg".into(),
                size: "extralarge".into(),
            },
        ];
        assert_eq!(pick_image(imgs).as_deref(), Some("extralarge.jpg"));
    }

    #[test]
    fn track_into_entry_has_no_listeners_in_genre() {
        // Regression: old impl stuffed `{n} listeners` into `genre`, which
        // made genre-based filters garbage. Genre must stay None unless
        // lastfm tells us an actual tag.
        let t = Track {
            name: "Dreams".into(),
            artist: "Fleetwood Mac".into(),
            album: Some("Rumours".into()),
            image: vec![Image {
                text: "cover.jpg".into(),
                size: "large".into(),
            }],
            listeners: Some("1234567".into()),
        };
        let e = t.into_entry(EntryKind::Track).unwrap();
        assert_eq!(
            e.genre, None,
            "listeners must not leak into genre (was the old bug)"
        );
        assert_eq!(e.artist_name.as_deref(), Some("Fleetwood Mac"));
        assert_eq!(e.album_name.as_deref(), Some("Rumours"));
        assert!(e
            .description
            .as_deref()
            .unwrap_or_default()
            .contains("listeners"));
    }

    #[test]
    fn chart_track_populates_album_when_present() {
        // Regression: old ChartTrack `album_name` was never populated (even
        // though the struct had an `album: Option<String>` field).
        let ct = ChartTrack {
            name: "Track".into(),
            artist: ChartArtistRef {
                name: "Artist".into(),
            },
            album: Some("Rumours".into()),
            image: vec![],
            listeners: None,
            playcount: None,
        };
        let e = ct.into_entry(EntryKind::Track).unwrap();
        assert_eq!(e.album_name.as_deref(), Some("Rumours"));
    }

    #[test]
    fn enrich_score_rewards_title_plus_artist_match() {
        let mut p = PluginEntry {
            title: "Dreams".into(),
            ..Default::default()
        };
        p.artist_name = Some("Fleetwood Mac".into());
        let mut exact = PluginEntry {
            title: "Dreams".into(),
            ..Default::default()
        };
        exact.artist_name = Some("Fleetwood Mac".into());
        let mut other = PluginEntry {
            title: "Dreams".into(),
            ..Default::default()
        };
        other.artist_name = Some("The Cranberries".into());
        assert!(enrich_score(&p, &exact) > enrich_score(&p, &other));
    }

    #[test]
    fn new_for_test_caches_api_key() {
        let p = LastfmPlugin::new_for_test("fake");
        assert_eq!(p.api_key().unwrap(), "fake");
    }
}
