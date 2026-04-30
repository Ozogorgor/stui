//! AniList metadata provider — anime movies and series via the public
//! AniList GraphQL API. No API key required.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup, enrich,
//! get_artwork, get_credits, related, episodes}`.

use serde::{Deserialize, Serialize};

use stui_plugin_sdk::{
    parse_manifest,
    error_codes, http_post_json,
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
    EpisodeWire, EpisodesRequest, EpisodesResponse,
    InitContext,
    LookupRequest, LookupResponse,
    Plugin, PluginEntry, PluginError, PluginInitError, PluginManifest, PluginResult,
    RelatedRequest, RelatedResponse, RelationKind,
    SearchRequest, SearchResponse, SearchScope,
};

const GRAPHQL_URL: &str = "https://graphql.anilist.co";

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct AnilistPlugin {
    manifest: PluginManifest,
}

impl AnilistPlugin {
    pub fn new() -> Self {
        let manifest: PluginManifest = parse_manifest(include_str!("../plugin.toml"))
            .expect("plugin.toml failed to parse at compile time");
        Self { manifest }
    }
}

impl Default for AnilistPlugin {
    fn default() -> Self { Self::new() }
}

impl Plugin for AnilistPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }

    fn init(&mut self, _ctx: &InitContext) -> Result<(), PluginInitError> {
        // AniList's public GraphQL endpoint needs no key; init is a no-op.
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
                    message: format!("AniList HTTP {status}: {body}"),
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
        plugin_error!("anilist: parse error: {}", e);
        PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("AniList JSON parse failure: {e}"),
        }
    })
}

fn gql(query: &'static str, variables: serde_json::Value) -> Result<String, PluginError> {
    let body = serde_json::to_string(&GraphQLRequest { query, variables })
        .map_err(|e| PluginError {
            code: error_codes::PARSE_ERROR.to_string(),
            message: format!("gql request encode: {e}"),
        })?;
    http_post_json(GRAPHQL_URL, &body).map_err(|e| classify_http_err(&e))
}

/// AniList's `format` → our `SearchScope` filter. Movie scope queries
/// `format: MOVIE`; Series scope queries `format_in: [TV, TV_SHORT, ONA, OVA]`.
fn scope_to_format_filter(scope: SearchScope) -> Result<(&'static str, bool), PluginError> {
    match scope {
        SearchScope::Movie  => Ok(("MOVIE", false)),
        SearchScope::Series => Ok(("[TV, TV_SHORT, ONA, OVA]", true)),
        _ => Err(PluginError {
            code: error_codes::UNSUPPORTED_SCOPE.to_string(),
            message: "anilist only supports movie and series scopes".to_string(),
        }),
    }
}

// ── CatalogPlugin impl ────────────────────────────────────────────────────────

impl CatalogPlugin for AnilistPlugin {
    fn search(&self, req: SearchRequest) -> PluginResult<SearchResponse> {
        let entry_kind = match req.scope {
            SearchScope::Movie  => EntryKind::Movie,
            SearchScope::Series => EntryKind::Series,
            _ => {
                return PluginResult::err(
                    error_codes::UNSUPPORTED_SCOPE,
                    "anilist only supports movie and series scopes",
                );
            }
        };
        let (format, is_list) = match scope_to_format_filter(req.scope) {
            Ok(p) => p,
            Err(e) => return PluginResult::Err(e),
        };

        let query_str = req.query.trim();
        let page    = req.page.max(1) as i32;
        let per_page = if req.limit == 0 { 20 } else { req.limit.min(50) as i32 };

        // Both trending and search share the Page wrapper; the only difference
        // is the media(...) argument set. We build the arg fragment here so the
        // `Page { pageInfo { ... } media(...) { ... } }` envelope stays stable.
        let (query_template, variables) = if query_str.is_empty() {
            // Trending: page-aware, filtered to the requested format.
            let filter = if is_list {
                format!("type: ANIME, format_in: {format}, sort: TRENDING_DESC")
            } else {
                format!("type: ANIME, format: {format}, sort: TRENDING_DESC")
            };
            (
                build_page_query(&filter),
                serde_json::json!({ "page": page, "perPage": per_page }),
            )
        } else {
            let filter = if is_list {
                format!("search: $search, type: ANIME, format_in: {format}")
            } else {
                format!("search: $search, type: ANIME, format: {format}")
            };
            (
                build_search_query(&filter),
                serde_json::json!({ "search": query_str, "page": page, "perPage": per_page }),
            )
        };

        plugin_info!("anilist: search '{}' (scope={:?}, page={})", query_str, req.scope, page);

        let raw = match gql(Box::leak(query_template.into_boxed_str()), variables) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };
        let gql_resp: GqlEnvelope<PagedMedia> = match parse_json(&raw) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        if let Some(errors) = gql_resp.errors {
            let msg = errors.first().map(|e| e.message.clone()).unwrap_or_default();
            return PluginResult::err(error_codes::REMOTE_ERROR, &format!("anilist: {msg}"));
        }
        let Some(data) = gql_resp.data else {
            return PluginResult::err(error_codes::REMOTE_ERROR, "anilist: empty data payload");
        };

        let items: Vec<PluginEntry> = data.page.media.into_iter()
            .map(|m| m.into_entry(entry_kind))
            .collect();
        let total = data.page.page_info.total.unwrap_or(items.len() as u32);
        PluginResult::ok(SearchResponse { items, total })
    }

    fn lookup(&self, req: LookupRequest) -> PluginResult<LookupResponse> {
        let entry_kind = match req.kind {
            EntryKind::Movie => EntryKind::Movie,
            _                => EntryKind::Series,
        };
        let (variables, query) = match req.id_source.as_str() {
            id_sources::ANILIST => {
                let id: i64 = match req.id.parse() {
                    Ok(n) => n,
                    Err(_) => return PluginResult::err(
                        error_codes::UNKNOWN_ID,
                        format!("anilist id must be numeric, got: {}", req.id),
                    ),
                };
                (serde_json::json!({ "id": id }), LOOKUP_BY_ID_QUERY)
            }
            id_sources::MYANIMELIST => {
                let id: i64 = match req.id.parse() {
                    Ok(n) => n,
                    Err(_) => return PluginResult::err(
                        error_codes::UNKNOWN_ID,
                        format!("myanimelist id must be numeric, got: {}", req.id),
                    ),
                };
                (serde_json::json!({ "idMal": id }), LOOKUP_BY_MAL_QUERY)
            }
            other => return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("unsupported id_source: {other}"),
            ),
        };

        let raw = match gql(query, variables) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };
        let gql_resp: GqlEnvelope<SingleMedia> = match parse_json(&raw) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        if gql_resp.errors.as_ref().is_some_and(|e| !e.is_empty()) {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist: no match for {}={}", req.id_source, req.id),
            );
        }
        let Some(data) = gql_resp.data else {
            return PluginResult::err(error_codes::REMOTE_ERROR, "anilist: empty data payload");
        };
        let Some(media) = data.media else {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist: no match for {}={}", req.id_source, req.id),
            );
        };
        // Walk the prequel→sequel chain so the EpisodeScreen can list
        // every cour as a separate season. Skip for movies — they have
        // no episodic concept and the relations chain often points at
        // unrelated franchise spinoffs which would mislead the UI.
        let chain = if matches!(entry_kind, EntryKind::Movie) {
            Vec::new()
        } else {
            walk_chain(media.id, media.relations.as_ref())
        };
        let mut entry = media.into_entry(entry_kind);
        if chain.len() > 1 {
            entry.season_count = Some(chain.len() as u32);
            entry.season_ids = chain
                .into_iter()
                .map(|id| format!("anilist-{id}"))
                .collect();
        } else {
            // Single cour or movie: explicit 1 so the UI doesn't fall
            // back to the unknown-count placeholder.
            entry.season_count = Some(1);
        }
        PluginResult::ok(LookupResponse { entry })
    }

    fn enrich(&self, req: EnrichRequest) -> PluginResult<EnrichResponse> {
        // Fast path: partial already carries an AniList id.
        if let Some(id) = req.partial.external_ids.get(id_sources::ANILIST) {
            let lookup_req = LookupRequest {
                id: id.clone(),
                id_source: id_sources::ANILIST.to_string(),
                kind: req.partial.kind,
                locale: None,
            };
            return match self.lookup(lookup_req) {
                PluginResult::Ok(r) => PluginResult::ok(EnrichResponse { entry: r.entry, confidence: 1.0 }),
                PluginResult::Err(e) => PluginResult::Err(e),
            };
        }

        let title = req.partial.title.trim();
        if title.is_empty() {
            return PluginResult::err(error_codes::INVALID_REQUEST, "enrich: partial.title is empty");
        }
        let search_req = SearchRequest {
            query: title.to_string(),
            scope: match req.partial.kind {
                EntryKind::Movie => SearchScope::Movie,
                _                => SearchScope::Series,
            },
            page: 1,
            limit: 5,
            per_scope_limit: None,
            locale: None,
        };
        let results = match self.search(search_req) {
            PluginResult::Ok(r) => r.items,
            PluginResult::Err(e) => return PluginResult::Err(e),
        };

        // Best-match: prefer equal-year hits over pure title-similarity.
        let best = results.into_iter()
            .max_by(|a, b| match_score(&req.partial, a).partial_cmp(&match_score(&req.partial, b)).unwrap_or(std::cmp::Ordering::Equal));
        match best {
            Some(entry) => {
                let confidence = match_score(&req.partial, &entry);
                PluginResult::ok(EnrichResponse { entry, confidence })
            }
            None => PluginResult::err(error_codes::UNKNOWN_ID, "anilist: no enrich match found"),
        }
    }

    fn get_artwork(&self, req: ArtworkRequest) -> PluginResult<ArtworkResponse> {
        if req.id_source != id_sources::ANILIST && req.id_source != id_sources::MYANIMELIST {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist artwork: unsupported id_source: {}", req.id_source),
            );
        }
        let lookup_req = LookupRequest {
            id: req.id.clone(),
            id_source: req.id_source.clone(),
            kind: req.kind,
            locale: None,
        };
        let media = match self.lookup(lookup_req) {
            PluginResult::Ok(r) => r.entry,
            PluginResult::Err(e) => return PluginResult::Err(e),
        };

        // AniList doesn't expose image dimensions; we approximate with documented
        // cover image sizes (medium ~230×323, large ~460×645, extraLarge ~920×1290).
        let mut variants = Vec::new();
        if let Some(url) = media.external_ids.get("anilist_cover_medium") {
            variants.push(ArtworkVariant { size: ArtworkSize::Thumbnail, url: url.clone(), mime: guess_mime(url), width: Some(230), height: Some(323) });
        }
        if let Some(url) = media.external_ids.get("anilist_cover_large").or(media.poster_url.as_ref()) {
            variants.push(ArtworkVariant { size: ArtworkSize::Standard, url: url.clone(), mime: guess_mime(url), width: Some(460), height: Some(645) });
        }
        if let Some(url) = media.external_ids.get("anilist_cover_extra_large") {
            variants.push(ArtworkVariant { size: ArtworkSize::HiRes, url: url.clone(), mime: guess_mime(url), width: Some(920), height: Some(1290) });
        }
        if let Some(url) = media.external_ids.get("anilist_banner") {
            variants.push(ArtworkVariant { size: ArtworkSize::HiRes, url: url.clone(), mime: guess_mime(url), width: None, height: None });
        }

        // If requester asked for a specific size, surface matching variants first.
        if !matches!(req.size, ArtworkSize::Any) {
            variants.sort_by_key(|v| if v.size == req.size { 0 } else { 1 });
        }
        PluginResult::ok(ArtworkResponse { variants })
    }

    fn get_credits(&self, req: CreditsRequest) -> PluginResult<CreditsResponse> {
        if req.id_source != id_sources::ANILIST {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist credits only supports anilist id_source, got: {}", req.id_source),
            );
        }
        let id: i64 = match req.id.parse() {
            Ok(n) => n,
            Err(_) => return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist id must be numeric, got: {}", req.id),
            ),
        };
        let raw = match gql(CREDITS_QUERY, serde_json::json!({ "id": id })) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };
        let gql_resp: GqlEnvelope<CreditsMedia> = match parse_json(&raw) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        let Some(data) = gql_resp.data.and_then(|d| d.media) else {
            return PluginResult::err(error_codes::UNKNOWN_ID, "anilist credits: media not found");
        };

        // Cast = character edges; each character has 0+ voiceActors (we pick the
        // first — AniList surfaces multiple language variants, JP is typical).
        let cast: Vec<CastMember> = data
            .characters
            .map(|c| c.edges)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|edge| {
                let character_name = edge.node.name.and_then(|n| n.full);
                let voice_actor = edge.voice_actors.into_iter().find_map(|va| va.name.and_then(|n| n.full));
                voice_actor.map(|name| CastMember {
                    name,
                    role: CastRole::Actor,
                    character: character_name,
                    instrument: None,
                    billing_order: edge.role.as_deref().and_then(|r| match r {
                        "MAIN"       => Some(1),
                        "SUPPORTING" => Some(2),
                        "BACKGROUND" => Some(3),
                        _            => None,
                    }),
                    external_ids: Default::default(),
                })
            })
            .collect();

        let crew: Vec<CrewMember> = data
            .staff
            .map(|s| s.edges)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|edge| {
                let name = edge.node.name.and_then(|n| n.full)?;
                let role_str = edge.role.unwrap_or_default();
                Some(CrewMember {
                    name,
                    role: normalize_crew_role(&role_str),
                    department: if role_str.is_empty() { None } else { Some(role_str) },
                    external_ids: Default::default(),
                })
            })
            .collect();

        PluginResult::ok(CreditsResponse { cast, crew })
    }

    fn related(&self, req: RelatedRequest) -> PluginResult<RelatedResponse> {
        if req.id_source != id_sources::ANILIST {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist related only supports anilist id_source, got: {}", req.id_source),
            );
        }
        // Map RelationKind → AniList recs/relations:
        //  - Similar / Any   → `recommendations`
        //  - Sequel          → `relations` filtered to SEQUEL / PREQUEL types
        //  - SameStudio etc  → unsupported on AniList
        let (use_recs, wanted_relation_types): (bool, &[&str]) = match req.relation {
            RelationKind::Similar | RelationKind::Any => (true, &[]),
            RelationKind::Sequel                      => (false, &["SEQUEL"]),
            RelationKind::Compilation                 => (false, &["SIDE_STORY", "PARENT"]),
            _ => return PluginResult::err(
                error_codes::UNSUPPORTED_SCOPE,
                format!("anilist does not surface {:?} relations", req.relation),
            ),
        };

        let id: i64 = match req.id.parse() {
            Ok(n) => n,
            Err(_) => return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist id must be numeric, got: {}", req.id),
            ),
        };
        let raw = match gql(RELATED_QUERY, serde_json::json!({ "id": id })) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };
        let gql_resp: GqlEnvelope<RelatedMedia> = match parse_json(&raw) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        let Some(media) = gql_resp.data.and_then(|d| d.media) else {
            return PluginResult::err(error_codes::UNKNOWN_ID, "anilist related: media not found");
        };

        let limit = if req.limit == 0 { 20 } else { req.limit as usize };
        let items: Vec<PluginEntry> = if use_recs {
            media.recommendations
                .map(|r| r.edges)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| e.node.media_recommendation)
                .take(limit)
                .map(|m| m.into_entry(EntryKind::Series))
                .collect()
        } else {
            media.relations
                .map(|r| r.edges)
                .unwrap_or_default()
                .into_iter()
                .filter(|edge| wanted_relation_types.contains(&edge.relation_type.as_deref().unwrap_or("")))
                .filter_map(|e| e.node)
                .take(limit)
                .map(|m| m.into_entry(EntryKind::Series))
                .collect()
        };
        PluginResult::ok(RelatedResponse { items })
    }

    fn episodes(&self, req: EpisodesRequest) -> PluginResult<EpisodesResponse> {
        if req.id_source != id_sources::ANILIST {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist episodes only supports anilist id_source, got: {}", req.id_source),
            );
        }
        // Each cour is its own AniList Media id (the chain-walk in `lookup`
        // emits one season_id per cour), so `series_id` is the cour's
        // numeric AniList id and `season` is conventionally 1. We don't
        // reject `season != 1` — the value is forwarded to the wire so the
        // TUI's grouping stays correct if the caller chose otherwise.
        let id: i64 = match req.series_id.parse() {
            Ok(n) => n,
            Err(_) => return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist id must be numeric, got: {}", req.series_id),
            ),
        };
        plugin_info!("anilist: episodes id={} season={}", id, req.season);

        let raw = match gql(EPISODES_QUERY, serde_json::json!({ "id": id })) {
            Ok(b) => b,
            Err(e) => return PluginResult::Err(e),
        };
        let env: GqlEnvelope<SingleEpisodesMedia> = match parse_json(&raw) {
            Ok(r) => r,
            Err(e) => return PluginResult::Err(e),
        };
        if env.errors.as_ref().is_some_and(|e| !e.is_empty()) {
            return PluginResult::err(
                error_codes::UNKNOWN_ID,
                format!("anilist episodes: no media for id={id}"),
            );
        }
        let Some(payload) = env.data.and_then(|d| d.media) else {
            return PluginResult::err(error_codes::UNKNOWN_ID, "anilist episodes: media not found");
        };

        let episodes = build_episodes(&req.series_id, req.season, payload);
        PluginResult::ok(EpisodesResponse { episodes })
    }
}

// ── Episodes builder ──────────────────────────────────────────────────────────

/// Build the per-episode wire list from an AniList `Media` payload.
///
/// AniList exposes two adjacent fields that together describe the cour:
///   - `episodes` (Int): canonical episode count for the cour.
///   - `streamingEpisodes` ([{ title, thumbnail }]): per-episode display
///     metadata for sites that have it. Returned in airing order, but
///     NOT episode-numbered — we derive `episode = i + 1` from list
///     position. May be shorter than the count, longer (rare, e.g.
///     specials folded in), or absent entirely.
///
/// We emit `max(streamingEpisodes.len(), episodes)` rows so the TUI
/// always sees a populated grid. Slots without streaming data fall back
/// to `"Episode N"` so the screen stays usable while still calling out
/// missing metadata visually.
///
/// `air_date` and `runtime_mins` stay `None` — AniList's public API
/// doesn't expose either at the per-episode level (`Media.duration` is
/// the cour-wide average and would mislead).
fn build_episodes(series_id: &str, season: u32, payload: EpisodesPayload) -> Vec<EpisodeWire> {
    let count = payload.episodes.unwrap_or(0) as usize;
    let stream = payload.streaming_episodes.unwrap_or_default();
    let total = stream.len().max(count);
    (0..total)
        .map(|i| {
            let n = (i + 1) as u32;
            let title = stream
                .get(i)
                .and_then(|s| s.title.clone())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| format!("Episode {n}"));
            EpisodeWire {
                season,
                episode: n,
                title,
                air_date: None,
                runtime_mins: None,
                provider: "anilist".to_string(),
                entry_id: format!("anilist-{series_id}:e{n}"),
            }
        })
        .collect()
}

// ── Scoring helper ────────────────────────────────────────────────────────────

/// Crude match-confidence score [0.0, 1.0]: case-insensitive exact title match
/// counts for 0.7; matching year adds 0.3; only-prefix adds 0.4.
fn match_score(partial: &PluginEntry, candidate: &PluginEntry) -> f32 {
    let p_title = partial.title.to_lowercase();
    let c_title = candidate.title.to_lowercase();
    let title_score = if p_title == c_title {
        0.7
    } else if !p_title.is_empty() && c_title.starts_with(&p_title) {
        0.4
    } else {
        0.0
    };
    let year_score = match (partial.year, candidate.year) {
        (Some(a), Some(b)) if a == b => 0.3,
        _ => 0.0,
    };
    title_score + year_score
}

fn guess_mime(url: &str) -> String {
    let lower = url.to_lowercase();
    if lower.ends_with(".png") { "image/png".into() }
    else if lower.ends_with(".webp") { "image/webp".into() }
    else { "image/jpeg".into() }
}

// ── GraphQL query strings ─────────────────────────────────────────────────────

const MEDIA_FIELDS: &str = r#"
id
idMal
title { romaji english native }
seasonYear
averageScore
episodes
duration
format
description(asHtml: false)
coverImage { medium large extraLarge }
bannerImage
genres
"#;

fn build_page_query(filter: &str) -> String {
    format!(
        r#"
query ($page: Int, $perPage: Int) {{
    Page(page: $page, perPage: $perPage) {{
        pageInfo {{ total currentPage lastPage hasNextPage }}
        media({filter}) {{ {MEDIA_FIELDS} }}
    }}
}}
"#
    )
}

fn build_search_query(filter: &str) -> String {
    format!(
        r#"
query ($search: String, $page: Int, $perPage: Int) {{
    Page(page: $page, perPage: $perPage) {{
        pageInfo {{ total currentPage lastPage hasNextPage }}
        media({filter}) {{ {MEDIA_FIELDS} }}
    }}
}}
"#
    )
}

const LOOKUP_BY_ID_QUERY: &str = r#"
query ($id: Int) {
    Media(id: $id, type: ANIME) {
        id idMal
        title { romaji english native }
        seasonYear averageScore episodes duration format
        description(asHtml: false)
        coverImage { medium large extraLarge }
        bannerImage
        genres
        relations { edges { relationType node { id } } }
    }
}
"#;

const LOOKUP_BY_MAL_QUERY: &str = r#"
query ($idMal: Int) {
    Media(idMal: $idMal, type: ANIME) {
        id idMal
        title { romaji english native }
        seasonYear averageScore episodes duration format
        description(asHtml: false)
        coverImage { medium large extraLarge }
        bannerImage
        genres
        relations { edges { relationType node { id } } }
    }
}
"#;

/// Lightweight query used while walking the prequel/sequel chain.
/// Skips media-detail fields — we only need the next hop, so fetching
/// title/poster/etc. per chain step is wasted bandwidth.
const CHAIN_HOP_QUERY: &str = r#"
query ($id: Int) {
    Media(id: $id, type: ANIME) {
        id
        relations { edges { relationType node { id } } }
    }
}
"#;

const CREDITS_QUERY: &str = r#"
query ($id: Int) {
    Media(id: $id, type: ANIME) {
        characters(sort: [ROLE, RELEVANCE], perPage: 25) {
            edges {
                role
                node { name { full } }
                voiceActors(language: JAPANESE) { name { full } }
            }
        }
        staff(perPage: 25) {
            edges {
                role
                node { name { full } }
            }
        }
    }
}
"#;

/// Per-cour episode list. `episodes` is the canonical count; the
/// `streamingEpisodes` array carries per-episode display titles when
/// available. We deliberately don't query `thumbnail` — the TUI's
/// episode grid is text-only today and dragging thumbnails through the
/// wire would inflate the response without payoff.
const EPISODES_QUERY: &str = r#"
query ($id: Int) {
    Media(id: $id, type: ANIME) {
        episodes
        streamingEpisodes { title }
    }
}
"#;

const RELATED_QUERY: &str = r#"
query ($id: Int) {
    Media(id: $id, type: ANIME) {
        recommendations(perPage: 20, sort: RATING_DESC) {
            edges {
                node {
                    mediaRecommendation {
                        id idMal title { romaji english }
                        seasonYear coverImage { large extraLarge }
                    }
                }
            }
        }
        relations {
            edges {
                relationType
                node {
                    id idMal title { romaji english }
                    seasonYear coverImage { large extraLarge }
                }
            }
        }
    }
}
"#;

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GraphQLRequest {
    query: &'static str,
    variables: serde_json::Value,
}

// Note: `Option<T>` fields are already optional in serde-json, so we don't
// need `#[serde(default)]`. Adding it would force a `T: Default` bound here.
#[derive(Debug, Deserialize)]
struct GqlEnvelope<T> {
    data: Option<T>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Debug, Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct PagedMedia {
    #[serde(rename = "Page")]
    page: PageBody,
}

#[derive(Debug, Deserialize)]
struct PageBody {
    #[serde(default)]
    page_info: PageInfo,
    #[serde(default)]
    media: Vec<Media>,
}

#[derive(Debug, Deserialize, Default)]
struct PageInfo {
    #[serde(default)]
    total: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct SingleMedia {
    #[serde(rename = "Media", default)]
    media: Option<Media>,
}

#[derive(Debug, Deserialize)]
struct CreditsMedia {
    #[serde(rename = "Media", default)]
    media: Option<CreditsPayload>,
}

#[derive(Debug, Deserialize, Default)]
struct CreditsPayload {
    #[serde(default)]
    characters: Option<CharacterConnection>,
    #[serde(default)]
    staff: Option<StaffConnection>,
}

#[derive(Debug, Deserialize)]
struct CharacterConnection {
    #[serde(default)]
    edges: Vec<CharacterEdge>,
}

#[derive(Debug, Deserialize)]
struct CharacterEdge {
    #[serde(default)]
    role: Option<String>,
    node: CharacterNode,
    #[serde(default, rename = "voiceActors")]
    voice_actors: Vec<VoiceActor>,
}

#[derive(Debug, Deserialize)]
struct CharacterNode {
    #[serde(default)]
    name: Option<NameFull>,
}

#[derive(Debug, Deserialize)]
struct VoiceActor {
    #[serde(default)]
    name: Option<NameFull>,
}

#[derive(Debug, Deserialize)]
struct NameFull {
    #[serde(default)]
    full: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StaffConnection {
    #[serde(default)]
    edges: Vec<StaffEdge>,
}

#[derive(Debug, Deserialize)]
struct StaffEdge {
    #[serde(default)]
    role: Option<String>,
    node: StaffNode,
}

#[derive(Debug, Deserialize)]
struct StaffNode {
    #[serde(default)]
    name: Option<NameFull>,
}

#[derive(Debug, Deserialize)]
struct SingleEpisodesMedia {
    #[serde(rename = "Media", default)]
    media: Option<EpisodesPayload>,
}

#[derive(Debug, Deserialize, Default)]
struct EpisodesPayload {
    #[serde(default)]
    episodes: Option<u32>,
    #[serde(rename = "streamingEpisodes", default)]
    streaming_episodes: Option<Vec<StreamingEpisode>>,
}

#[derive(Debug, Deserialize)]
struct StreamingEpisode {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelatedMedia {
    #[serde(rename = "Media", default)]
    media: Option<RelatedPayload>,
}

#[derive(Debug, Deserialize, Default)]
struct RelatedPayload {
    #[serde(default)]
    recommendations: Option<RecConnection>,
    #[serde(default)]
    relations: Option<RelationConnection>,
}

#[derive(Debug, Deserialize)]
struct RecConnection {
    #[serde(default)]
    edges: Vec<RecEdge>,
}

#[derive(Debug, Deserialize)]
struct RecEdge {
    node: RecNode,
}

#[derive(Debug, Deserialize)]
struct RecNode {
    #[serde(default, rename = "mediaRecommendation")]
    media_recommendation: Option<MediaStub>,
}

#[derive(Debug, Deserialize)]
struct RelationConnection {
    #[serde(default)]
    edges: Vec<RelationEdge>,
}

#[derive(Debug, Deserialize)]
struct RelationEdge {
    #[serde(default, rename = "relationType")]
    relation_type: Option<String>,
    #[serde(default)]
    node: Option<MediaStub>,
}

#[derive(Debug, Deserialize)]
struct Media {
    id: u64,
    #[serde(rename = "idMal", default)]
    id_mal: Option<u64>,
    title: AnimeTitle,
    #[serde(rename = "seasonYear", default)]
    season_year: Option<u32>,
    #[serde(rename = "averageScore", default)]
    average_score: Option<f32>,
    #[serde(default)]
    episodes: Option<u32>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "coverImage", default)]
    cover_image: Option<CoverImage>,
    #[serde(rename = "bannerImage", default)]
    banner_image: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    /// Adjacency edges to other Media (sequel/prequel/sidestory/etc.).
    /// Lookup uses these to walk the prequel→sequel chain so the
    /// EpisodeScreen can list every cour as a separate "season".
    #[serde(default)]
    relations: Option<RelationConnection>,
}

#[derive(Debug, Deserialize)]
struct ChainNode {
    #[allow(dead_code)]
    id: u64,
    #[serde(default)]
    relations: Option<RelationConnection>,
}

#[derive(Debug, Deserialize)]
struct MediaStub {
    id: u64,
    #[serde(rename = "idMal", default)]
    id_mal: Option<u64>,
    /// Optional so the chain-walk query (which only fetches `id`) still
    /// deserializes — the chain only needs the node id, not display data.
    #[serde(default)]
    title: AnimeTitle,
    #[serde(rename = "seasonYear", default)]
    season_year: Option<u32>,
    #[serde(rename = "coverImage", default)]
    cover_image: Option<CoverImage>,
}

#[derive(Debug, Deserialize, Default)]
struct AnimeTitle {
    #[serde(default)] romaji:  Option<String>,
    #[serde(default)] english: Option<String>,
    #[serde(default)] native:  Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CoverImage {
    #[serde(default)] medium:      Option<String>,
    #[serde(default)] large:       Option<String>,
    #[serde(rename = "extraLarge", default)] extra_large: Option<String>,
}

fn pick_title(t: AnimeTitle) -> String {
    t.english.or(t.romaji).or(t.native).unwrap_or_default()
}

/// Maximum chain depth — anti-cycle bound. Anime franchises rarely
/// exceed this in either direction; capping protects against malformed
/// `relations` graphs (cycles, self-edges) that would otherwise loop
/// the chain walker forever.
const CHAIN_MAX_DEPTH: usize = 20;

/// Pull the first PREQUEL or SEQUEL id from a relations connection.
/// Returns None when no such edge exists, so callers can stop walking.
fn first_relation_id(rel: Option<&RelationConnection>, want: &str) -> Option<u64> {
    rel?.edges.iter().find_map(|e| {
        let kind = e.relation_type.as_deref()?;
        if kind.eq_ignore_ascii_case(want) {
            e.node.as_ref().map(|n| n.id)
        } else {
            None
        }
    })
}

/// Fetch only the chain-relevant fields of a Media. Used to traverse
/// prequel/sequel hops without paying for the full Media payload at
/// every step.
fn chain_hop(id: u64) -> Result<Option<ChainNode>, PluginError> {
    let raw = gql(CHAIN_HOP_QUERY, serde_json::json!({ "id": id }))?;
    let env: GqlEnvelope<SingleChainNode> = parse_json(&raw)?;
    if env.errors.as_ref().is_some_and(|e| !e.is_empty()) {
        return Ok(None);
    }
    Ok(env.data.and_then(|d| d.media))
}

/// Walk the prequel→sequel chain from `anchor_id`. Result is the
/// ordered list of media ids from earliest cour to latest, INCLUDING
/// the anchor at its true position in the chain.
///
/// Returns `vec![anchor_id]` when no relations exist or the chain is
/// degenerate — callers should treat a length-1 result as "single
/// season" rather than a special case.
fn walk_chain(anchor_id: u64, anchor_relations: Option<&RelationConnection>) -> Vec<u64> {
    // ── Step 1: walk PREQUEL backward to chain root. ──────────────────
    let mut prefix: Vec<u64> = Vec::new();
    let mut current_relations: Option<RelationConnection> =
        anchor_relations.map(|r| RelationConnection {
            edges: r.edges.iter().map(|e| RelationEdge {
                relation_type: e.relation_type.clone(),
                node: e.node.as_ref().map(|n| MediaStub {
                    id: n.id,
                    id_mal: n.id_mal,
                    title: AnimeTitle::default(),
                    season_year: n.season_year,
                    cover_image: None,
                }),
            }).collect(),
        });
    let mut hops = 0;
    while let Some(prev_id) = first_relation_id(current_relations.as_ref(), "PREQUEL") {
        if hops >= CHAIN_MAX_DEPTH || prefix.contains(&prev_id) || prev_id == anchor_id {
            break;
        }
        prefix.push(prev_id);
        match chain_hop(prev_id) {
            Ok(Some(node)) => current_relations = node.relations,
            _ => break,
        }
        hops += 1;
    }
    prefix.reverse(); // now [oldest .. just-before-anchor]

    // ── Step 2: walk SEQUEL forward from anchor. ──────────────────────
    let mut suffix: Vec<u64> = Vec::new();
    // Restart from anchor's relations (we may have mutated current_relations above).
    current_relations = anchor_relations.map(|r| RelationConnection {
        edges: r.edges.iter().map(|e| RelationEdge {
            relation_type: e.relation_type.clone(),
            node: e.node.as_ref().map(|n| MediaStub {
                id: n.id,
                id_mal: n.id_mal,
                title: AnimeTitle::default(),
                season_year: n.season_year,
                cover_image: None,
            }),
        }).collect(),
    });
    hops = 0;
    while let Some(next_id) = first_relation_id(current_relations.as_ref(), "SEQUEL") {
        if hops >= CHAIN_MAX_DEPTH
            || suffix.contains(&next_id)
            || prefix.contains(&next_id)
            || next_id == anchor_id
        {
            break;
        }
        suffix.push(next_id);
        match chain_hop(next_id) {
            Ok(Some(node)) => current_relations = node.relations,
            _ => break,
        }
        hops += 1;
    }

    // [oldest .. anchor .. latest]
    let mut chain = prefix;
    chain.push(anchor_id);
    chain.extend(suffix);
    chain
}

#[derive(Debug, Deserialize)]
struct SingleChainNode {
    #[serde(rename = "Media")]
    media: Option<ChainNode>,
}

impl Media {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let mut entry = PluginEntry {
            id: format!("anilist-{}", self.id),
            kind,
            source: "anilist".to_string(),
            title: pick_title(self.title),
            year: self.season_year,
            // averageScore is 0–100; scale to 0.0–10.0 so it matches other providers.
            rating: self.average_score.map(|s| s / 10.0),
            // AniList descriptions arrive with `<br>` line breaks plus a
            // trailing "(Source: …)" attribution even when `asHtml: false`
            // is requested. Strip them through the SDK helper so the TUI
            // gets readable text.
            description: self.description.as_deref().map(stui_plugin_sdk::clean_description),
            duration: self.duration,
            genre: if self.genres.is_empty() { None } else { Some(self.genres.join(", ")) },
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::ANILIST.to_string(), self.id.to_string());
        if let Some(mal) = self.id_mal {
            entry.external_ids.insert(id_sources::MYANIMELIST.to_string(), mal.to_string());
        }
        if let Some(ci) = self.cover_image {
            entry.poster_url = ci.large.clone().or(ci.extra_large.clone()).or(ci.medium.clone());
            if let Some(m) = ci.medium       { entry.external_ids.insert("anilist_cover_medium".into(), m); }
            if let Some(l) = ci.large        { entry.external_ids.insert("anilist_cover_large".into(), l); }
            if let Some(xl) = ci.extra_large { entry.external_ids.insert("anilist_cover_extra_large".into(), xl); }
        }
        if let Some(b) = self.banner_image {
            entry.external_ids.insert("anilist_banner".into(), b);
        }
        entry
    }
}

impl MediaStub {
    fn into_entry(self, kind: EntryKind) -> PluginEntry {
        let mut entry = PluginEntry {
            id: format!("anilist-{}", self.id),
            kind,
            source: "anilist".to_string(),
            title: pick_title(self.title),
            year: self.season_year,
            ..Default::default()
        };
        entry.external_ids.insert(id_sources::ANILIST.to_string(), self.id.to_string());
        if let Some(mal) = self.id_mal {
            entry.external_ids.insert(id_sources::MYANIMELIST.to_string(), mal.to_string());
        }
        if let Some(ci) = self.cover_image {
            entry.poster_url = ci.large.or(ci.extra_large);
        }
        entry
    }
}

// ── WASM exports ──────────────────────────────────────────────────────────────

impl stui_plugin_sdk::StreamProvider for AnilistPlugin {}

stui_export_catalog_plugin!(AnilistPlugin);

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_trait_satisfied() {
        fn _p<T: Plugin>() {}
        fn _c<T: CatalogPlugin>() {}
        _p::<AnilistPlugin>();
        _c::<AnilistPlugin>();
    }

    #[test]
    fn manifest_parses_at_compile_time() {
        let p = AnilistPlugin::new();
        assert_eq!(p.manifest().plugin.name, "anilist");
    }

    #[test]
    fn scope_mapping_movie_uses_singular_format() {
        let (f, is_list) = scope_to_format_filter(SearchScope::Movie).unwrap();
        assert_eq!(f, "MOVIE");
        assert!(!is_list);
    }

    #[test]
    fn scope_mapping_series_uses_format_in_list() {
        let (f, is_list) = scope_to_format_filter(SearchScope::Series).unwrap();
        assert_eq!(f, "[TV, TV_SHORT, ONA, OVA]");
        assert!(is_list);
    }

    #[test]
    fn scope_mapping_other_errors() {
        assert!(scope_to_format_filter(SearchScope::Track).is_err());
    }

    #[test]
    fn page_query_wraps_media_in_page_node_with_pagination_vars() {
        let q = build_page_query("type: ANIME, format: MOVIE");
        assert!(q.contains("Page(page: $page, perPage: $perPage)"), "missing Page wrapper: {q}");
        assert!(q.contains("media(type: ANIME, format: MOVIE)"));
        assert!(q.contains("pageInfo"));
    }

    #[test]
    fn search_query_keeps_search_var_and_format_filter() {
        let q = build_search_query("search: $search, type: ANIME, format_in: [TV, ONA]");
        assert!(q.contains("$search: String"));
        assert!(q.contains("format_in: [TV, ONA]"));
    }

    #[test]
    fn pick_title_prefers_english_over_romaji() {
        let t = AnimeTitle { english: Some("Attack on Titan".into()), romaji: Some("Shingeki no Kyojin".into()), native: None };
        assert_eq!(pick_title(t), "Attack on Titan");
    }

    #[test]
    fn pick_title_falls_back_to_romaji_then_native() {
        let t = AnimeTitle { english: None, romaji: Some("Romaji".into()), native: Some("Native".into()) };
        assert_eq!(pick_title(t), "Romaji");
    }

    #[test]
    fn media_into_entry_populates_external_ids_including_mal() {
        let m = Media {
            id: 16498,
            id_mal: Some(16498),
            title: AnimeTitle { romaji: Some("SnK".into()), english: Some("AoT".into()), native: None },
            season_year: Some(2013),
            average_score: Some(86.0),
            episodes: Some(25),
            duration: Some(24),
            format: Some("TV".into()),
            description: Some("desc".into()),
            cover_image: Some(CoverImage { medium: Some("m.jpg".into()), large: Some("l.jpg".into()), extra_large: Some("xl.jpg".into()) }),
            banner_image: Some("b.jpg".into()),
            genres: vec!["Action".into(), "Drama".into()],
            relations: None,
        };
        let e = m.into_entry(EntryKind::Series);
        assert_eq!(e.source, "anilist");
        assert_eq!(e.title, "AoT");
        assert_eq!(e.year, Some(2013));
        assert_eq!(e.rating, Some(8.6));
        assert_eq!(e.poster_url.as_deref(), Some("l.jpg"));
        assert_eq!(e.external_ids.get(id_sources::ANILIST).map(String::as_str), Some("16498"));
        assert_eq!(e.external_ids.get(id_sources::MYANIMELIST).map(String::as_str), Some("16498"));
        assert_eq!(e.genre.as_deref(), Some("Action, Drama"));
    }

    #[test]
    fn match_score_prefers_exact_title_plus_year() {
        let p = PluginEntry { title: "Inception".into(), year: Some(2010), ..Default::default() };
        let exact = PluginEntry { title: "Inception".into(), year: Some(2010), ..Default::default() };
        let close = PluginEntry { title: "Inception II".into(), year: Some(2010), ..Default::default() };
        assert!(match_score(&p, &exact) > match_score(&p, &close));
    }

    #[test]
    fn guess_mime_reads_extension() {
        assert_eq!(guess_mime("https://x/y.png"), "image/png");
        assert_eq!(guess_mime("https://x/y.webp"), "image/webp");
        assert_eq!(guess_mime("https://x/y.jpg"), "image/jpeg");
        assert_eq!(guess_mime("https://x/y.unknown"), "image/jpeg");
    }

    #[test]
    fn build_episodes_uses_streaming_titles_when_present() {
        let payload = EpisodesPayload {
            episodes: Some(3),
            streaming_episodes: Some(vec![
                StreamingEpisode { title: Some("To You, in 2000 Years".into()) },
                StreamingEpisode { title: Some("That Day".into()) },
                StreamingEpisode { title: Some("A Dim Light Amid Despair".into()) },
            ]),
        };
        let eps = build_episodes("16498", 1, payload);
        assert_eq!(eps.len(), 3);
        assert_eq!(eps[0].episode, 1);
        assert_eq!(eps[0].title, "To You, in 2000 Years");
        assert_eq!(eps[2].title, "A Dim Light Amid Despair");
        assert!(eps.iter().all(|e| e.season == 1 && e.provider == "anilist"));
    }

    #[test]
    fn build_episodes_pads_with_episode_number_when_count_exceeds_streaming() {
        let payload = EpisodesPayload {
            episodes: Some(5),
            streaming_episodes: Some(vec![
                StreamingEpisode { title: Some("Pilot".into()) },
                StreamingEpisode { title: Some("Take Two".into()) },
            ]),
        };
        let eps = build_episodes("99", 1, payload);
        assert_eq!(eps.len(), 5);
        assert_eq!(eps[0].title, "Pilot");
        assert_eq!(eps[1].title, "Take Two");
        assert_eq!(eps[2].title, "Episode 3");
        assert_eq!(eps[4].title, "Episode 5");
    }

    #[test]
    fn build_episodes_handles_blank_streaming_titles() {
        let payload = EpisodesPayload {
            episodes: Some(2),
            streaming_episodes: Some(vec![
                StreamingEpisode { title: Some("   ".into()) },
                StreamingEpisode { title: None },
            ]),
        };
        let eps = build_episodes("1", 1, payload);
        assert_eq!(eps[0].title, "Episode 1");
        assert_eq!(eps[1].title, "Episode 2");
    }

    #[test]
    fn build_episodes_emits_anilist_prefixed_entry_ids() {
        let payload = EpisodesPayload {
            episodes: Some(2),
            streaming_episodes: None,
        };
        let eps = build_episodes("16498", 1, payload);
        assert_eq!(eps[0].entry_id, "anilist-16498:e1");
        assert_eq!(eps[1].entry_id, "anilist-16498:e2");
    }

    #[test]
    fn build_episodes_returns_empty_when_no_signal() {
        let payload = EpisodesPayload { episodes: None, streaming_episodes: None };
        assert!(build_episodes("1", 1, payload).is_empty());
    }

    #[test]
    fn build_episodes_uses_streaming_length_when_count_missing() {
        let payload = EpisodesPayload {
            episodes: None,
            streaming_episodes: Some(vec![
                StreamingEpisode { title: Some("A".into()) },
                StreamingEpisode { title: Some("B".into()) },
            ]),
        };
        let eps = build_episodes("1", 1, payload);
        assert_eq!(eps.len(), 2);
    }

    #[test]
    fn episodes_rejects_non_anilist_id_source() {
        let p = AnilistPlugin::new();
        let req = EpisodesRequest {
            series_id: "16498".into(),
            id_source: "tmdb".into(),
            season: 1,
        };
        match p.episodes(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            PluginResult::Ok(_) => panic!("expected UNKNOWN_ID rejection"),
        }
    }

    #[test]
    fn episodes_rejects_non_numeric_id() {
        let p = AnilistPlugin::new();
        let req = EpisodesRequest {
            series_id: "not-a-number".into(),
            id_source: id_sources::ANILIST.to_string(),
            season: 1,
        };
        match p.episodes(req) {
            PluginResult::Err(e) => assert_eq!(e.code, error_codes::UNKNOWN_ID),
            PluginResult::Ok(_) => panic!("expected UNKNOWN_ID rejection"),
        }
    }
}
