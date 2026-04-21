//! AniList metadata provider — anime movies and series via the public
//! AniList GraphQL API. No API key required.
//!
//! Implements `Plugin` + `CatalogPlugin::{search, lookup, enrich,
//! get_artwork, get_credits, related}`.

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
        PluginResult::ok(LookupResponse { entry: media.into_entry(entry_kind) })
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
}

#[derive(Debug, Deserialize)]
struct MediaStub {
    id: u64,
    #[serde(rename = "idMal", default)]
    id_mal: Option<u64>,
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
            description: self.description,
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
}
