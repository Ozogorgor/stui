//! Adapter layer between `TvdbClient`'s cached `/extended` responses and
//! the runtime's plugin-shaped verb responses.
//!
//! Three async functions (`enrich`, `credits`, `artwork`) are the entry
//! points called from `engine::metadata::dispatch::EngineMetadataDispatch`.
//! Each resolves a tvdb_id from the request, fetches the cached extended
//! payload, and dispatches to a pure extractor function.
//!
//! The extractors are split out so they're unit-testable against fixture
//! JSON without going through the cache or HTTP layers.

use std::collections::HashMap;
use std::sync::Arc;

use stui_plugin_sdk::EntryKind;

use crate::abi::types::{
    ArtworkRequest, ArtworkResponse, ArtworkSize, ArtworkVariant, CastMember, CastRole,
    CreditsRequest, CreditsResponse, CrewMember, CrewRole, EnrichRequest, EnrichResponse,
    PluginEntry,
};
use crate::tvdb::client::TvdbClient;
use crate::tvdb::types::{Artwork, Character, ExtendedMovie, ExtendedSeries, Genre, RemoteId};

/// TVDB's "Aired Order" season-type id. Other ids (DVD, absolute,
/// alternate, regional) are filtered out of `season_count` so alternate
/// orderings don't double the count.
const TVDB_DEFAULT_SEASON_TYPE: u32 = 1;

/// Build an `EnrichResponse` from a cached `ExtendedSeries`. Pure — fed
/// fixture JSON in tests.
///
/// `kind` is forwarded onto the `PluginEntry.kind` field so the
/// orchestrator's downstream verb router routes correctly.
pub fn extract_enrich_series(extended: &ExtendedSeries, kind: EntryKind) -> EnrichResponse {
    let (imdb_id, tmdb_id) = extract_external_ids(&extended.remote_ids);
    let mut external_ids = HashMap::new();
    external_ids.insert("tvdb".to_string(), extended.id.to_string());
    if let Some(ref imdb) = imdb_id {
        external_ids.insert("imdb".to_string(), imdb.clone());
    }
    if let Some(ref tmdb) = tmdb_id {
        external_ids.insert("tmdb".to_string(), tmdb.clone());
    }

    let season_count = extended
        .seasons
        .iter()
        .filter(|s| s.number >= 1 && s.season_type.id == TVDB_DEFAULT_SEASON_TYPE)
        .map(|s| s.number)
        .max();
    // Specials = season 0 in the default ordering. TVDB's Season struct
    // doesn't carry an episode count, so we treat the existence of the
    // row as the signal — if season 0 is in the canonical order, the
    // show has specials, full stop.
    let has_specials = extended
        .seasons
        .iter()
        .any(|s| s.number == 0 && s.season_type.id == TVDB_DEFAULT_SEASON_TYPE);

    let entry = PluginEntry {
        id: format!("tvdb-{}", extended.id),
        kind,
        title: extended.name.clone(),
        source: "tvdb".to_string(),
        year: extended.year.as_deref().and_then(|s| s.parse::<u32>().ok()),
        rating: extended.score.map(|s| s as f32),
        description: extended.overview.clone(),
        genre: if extended.genres.is_empty() {
            None
        } else {
            Some(genres_to_string(&extended.genres))
        },
        poster_url: extended.image.clone(),
        imdb_id,
        external_ids,
        season_count,
        season_ids: Vec::new(), // TMDB-style routing — TUI calls per-season
        has_specials,
        original_language: extended.original_language.clone(),
        ..Default::default()
    };

    EnrichResponse {
        entry,
        confidence: 1.0,
    }
}

pub fn extract_enrich_movie(extended: &ExtendedMovie, kind: EntryKind) -> EnrichResponse {
    let (imdb_id, tmdb_id) = extract_external_ids(&extended.remote_ids);
    let mut external_ids = HashMap::new();
    external_ids.insert("tvdb".to_string(), extended.id.to_string());
    if let Some(ref imdb) = imdb_id {
        external_ids.insert("imdb".to_string(), imdb.clone());
    }
    if let Some(ref tmdb) = tmdb_id {
        external_ids.insert("tmdb".to_string(), tmdb.clone());
    }

    let entry = PluginEntry {
        id: format!("tvdb-{}", extended.id),
        kind,
        title: extended.name.clone(),
        source: "tvdb".to_string(),
        year: extended.year.as_deref().and_then(|s| s.parse::<u32>().ok()),
        rating: extended.score.map(|s| s as f32),
        description: extended.overview.clone(),
        genre: if extended.genres.is_empty() {
            None
        } else {
            Some(genres_to_string(&extended.genres))
        },
        poster_url: extended.image.clone(),
        imdb_id,
        external_ids,
        original_language: extended.original_language.clone(),
        ..Default::default()
    };

    EnrichResponse {
        entry,
        confidence: 1.0,
    }
}

fn extract_external_ids(rids: &[RemoteId]) -> (Option<String>, Option<String>) {
    let mut imdb = None;
    let mut tmdb = None;
    for rid in rids {
        match rid.source_name.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("IMDB") => imdb = Some(rid.id.clone()),
            Some(s) if s.contains("MovieDB") || s.eq_ignore_ascii_case("TMDB") => {
                tmdb = Some(rid.id.clone());
            }
            _ => {}
        }
    }
    (imdb, tmdb)
}

fn genres_to_string(genres: &[Genre]) -> String {
    genres
        .iter()
        .map(|g| g.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Map TVDB's peopleType string to `abi::types::CrewRole`. Variant set
/// matches `runtime/src/abi/types.rs:255-275`. Unknown / un-bucketed
/// labels surface via `Other(String)` so the role text isn't lost.
fn map_crew_role(people_type: &str) -> CrewRole {
    match people_type {
        "Director" => CrewRole::Director,
        "Writer" => CrewRole::Writer,
        "Producer" => CrewRole::Producer,
        "Executive Producer" => CrewRole::ExecutiveProducer,
        "Cinematographer" | "Director of Photography" => CrewRole::Cinematographer,
        // TVDB anime entries often surface these:
        "Animation Director" => CrewRole::AnimationDirector,
        "Lead Animator" => CrewRole::LeadAnimator,
        // Creator / Showrunner / etc. don't have dedicated variants —
        // preserve the label in Other(String) rather than dropping.
        other => CrewRole::Other(other.to_string()),
    }
}

/// Convert TVDB's `characters[]` into runtime cast/crew. Actors land in
/// `cast` with `billing_order` from `sort`; everyone else lands in `crew`
/// with their `peopleType` mapped via `map_crew_role`. Rows with no
/// `personName` or no `peopleType` are dropped — they're unactionable.
pub fn extract_credits(characters: &[Character]) -> CreditsResponse {
    let mut cast = Vec::new();
    let mut crew = Vec::new();
    for c in characters {
        let Some(person) = c.person_name.clone() else {
            continue;
        };
        match c.people_type.as_deref() {
            Some("Actor") => {
                cast.push(CastMember {
                    name: person,
                    role: CastRole::Actor,
                    character: c.name.clone(),
                    instrument: None,
                    billing_order: c.sort,
                    external_ids: Default::default(),
                });
            }
            Some(role) => {
                crew.push(CrewMember {
                    name: person,
                    role: map_crew_role(role),
                    department: Some(role.to_string()),
                    external_ids: Default::default(),
                });
            }
            None => {} // unknown peopleType — drop
        }
    }
    CreditsResponse { cast, crew }
}

/// TVDB image type codes → ArtworkSize. Both banner and background map
/// to `HiRes` because the SDK's `ArtworkSize` enum has no kind
/// discriminator (poster / banner / background / clearart all collapse
/// onto Standard or HiRes). See spec for the SDK-side gap.
fn artwork_size_for(image_type: u32) -> ArtworkSize {
    match image_type {
        2 => ArtworkSize::Standard,  // poster
        1 => ArtworkSize::HiRes,     // banner
        3 => ArtworkSize::HiRes,     // background
        22 => ArtworkSize::Standard, // clearart
        _ => ArtworkSize::Any,
    }
}

fn guess_mime(url: &str) -> String {
    let lower = url.to_lowercase();
    if lower.ends_with(".png") {
        "image/png".into()
    } else if lower.ends_with(".webp") {
        "image/webp".into()
    } else {
        "image/jpeg".into()
    }
}

pub fn extract_artwork(artworks: &[Artwork]) -> ArtworkResponse {
    let variants = artworks
        .iter()
        .filter(|a| !a.image.trim().is_empty()) // skip rows with no image URL
        .map(|a| ArtworkVariant {
            size: artwork_size_for(a.image_type),
            url: a.image.clone(),
            mime: guess_mime(&a.image),
            width: None,
            height: None,
        })
        .collect();
    ArtworkResponse { variants }
}

// ── Async adapter functions ───────────────────────────────────────────────────
//
// These three (`enrich`, `credits`, `artwork`) are the entry points
// `engine::metadata::dispatch::EngineMetadataDispatch` calls. Each
// resolves a tvdb_id from the request, fetches the cached extended
// payload via `TvdbClient`, and delegates to one of the pure
// extractors above. All return `Result<_, String>` to match the
// `Result<_, String>` shape that `dispatch.rs::call_*` expects.

/// Resolve a tvdb_id from an enrich request's foreign ids. Order:
/// explicit `tvdb` external_id → `imdb` → `tmdb`. Returns `None` if no
/// source resolves; caller surfaces an empty contribution.
async fn resolve_tvdb_id(
    client: &TvdbClient,
    external_ids: &HashMap<String, String>,
    imdb_id: &Option<String>,
) -> Option<u64> {
    // Explicit tvdb id first.
    if let Some(id) = external_ids.get("tvdb") {
        let stripped = id.strip_prefix("tvdb-").unwrap_or(id);
        return stripped.parse::<u64>().ok();
    }

    // Try imdb.
    let imdb = external_ids.get("imdb").cloned().or_else(|| imdb_id.clone());
    if let Some(imdb) = imdb {
        if let Ok(Some(tvdb)) = client.resolve_remote_id("imdb", &imdb).await {
            return tvdb.parse::<u64>().ok();
        }
    }

    // Fall back to tmdb.
    if let Some(tmdb) = external_ids.get("tmdb") {
        if let Ok(Some(tvdb)) = client.resolve_remote_id("tmdb", tmdb).await {
            return tvdb.parse::<u64>().ok();
        }
    }

    None
}

/// Pick the right `extended_*` endpoint based on the partial entry's kind.
enum FetchedExtended {
    Series(Arc<ExtendedSeries>),
    Movie(Arc<ExtendedMovie>),
}

async fn fetch_for_kind(
    client: &TvdbClient,
    tvdb_id: u64,
    kind: EntryKind,
) -> Result<FetchedExtended, String> {
    match kind {
        EntryKind::Movie => client
            .extended_movie(tvdb_id)
            .await
            .map(FetchedExtended::Movie)
            .map_err(|e| e.to_string()),
        // Series + Episode + anime fall through to series-extended.
        _ => client
            .extended_series(tvdb_id)
            .await
            .map(FetchedExtended::Series)
            .map_err(|e| e.to_string()),
    }
}

pub async fn enrich(client: &TvdbClient, req: EnrichRequest) -> Result<EnrichResponse, String> {
    let kind = req.partial.kind;
    let tvdb_id = resolve_tvdb_id(client, &req.partial.external_ids, &req.partial.imdb_id)
        .await
        .ok_or_else(|| "tvdb: no resolvable id for entry".to_string())?;

    match fetch_for_kind(client, tvdb_id, kind).await? {
        FetchedExtended::Series(s) => Ok(extract_enrich_series(&s, kind)),
        FetchedExtended::Movie(m) => Ok(extract_enrich_movie(&m, kind)),
    }
}

pub async fn credits(client: &TvdbClient, req: CreditsRequest) -> Result<CreditsResponse, String> {
    let tvdb_id = resolve_tvdb_id_from_credits_req(client, &req).await?;
    let kind = req.kind;
    match fetch_for_kind(client, tvdb_id, kind).await? {
        FetchedExtended::Series(s) => Ok(extract_credits(&s.characters)),
        FetchedExtended::Movie(m) => Ok(extract_credits(&m.characters)),
    }
}

pub async fn artwork(client: &TvdbClient, req: ArtworkRequest) -> Result<ArtworkResponse, String> {
    let tvdb_id = resolve_tvdb_id_from_artwork_req(client, &req).await?;
    let kind = req.kind;
    match fetch_for_kind(client, tvdb_id, kind).await? {
        FetchedExtended::Series(s) => Ok(extract_artwork(&s.artworks)),
        FetchedExtended::Movie(m) => Ok(extract_artwork(&m.artworks)),
    }
}

/// `CreditsRequest` carries `id` + `id_source` rather than a `partial`.
/// Resolve through `id_source` directly when it's `tvdb`, fall through
/// to imdb/tmdb otherwise.
async fn resolve_tvdb_id_from_credits_req(
    client: &TvdbClient,
    req: &CreditsRequest,
) -> Result<u64, String> {
    if req.id_source == "tvdb" {
        let stripped = req.id.strip_prefix("tvdb-").unwrap_or(&req.id);
        return stripped
            .parse::<u64>()
            .map_err(|_| format!("tvdb: malformed id {}", req.id));
    }
    let resolved = client
        .resolve_remote_id(&req.id_source, &req.id)
        .await
        .map_err(|e| e.to_string())?;
    resolved
        .ok_or_else(|| format!("tvdb: no match for {}={}", req.id_source, req.id))?
        .parse::<u64>()
        .map_err(|_| "tvdb: resolved id is not numeric".to_string())
}

async fn resolve_tvdb_id_from_artwork_req(
    client: &TvdbClient,
    req: &ArtworkRequest,
) -> Result<u64, String> {
    if req.id_source == "tvdb" {
        let stripped = req.id.strip_prefix("tvdb-").unwrap_or(&req.id);
        return stripped
            .parse::<u64>()
            .map_err(|_| format!("tvdb: malformed id {}", req.id));
    }
    let resolved = client
        .resolve_remote_id(&req.id_source, &req.id)
        .await
        .map_err(|e| e.to_string())?;
    resolved
        .ok_or_else(|| format!("tvdb: no match for {}={}", req.id_source, req.id))?
        .parse::<u64>()
        .map_err(|_| "tvdb: resolved id is not numeric".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tvdb::types::{Genre, RemoteId, Season, SeasonType};

    fn season(number: u32, type_id: u32) -> Season {
        Season {
            id: 0,
            number,
            name: None,
            image: None,
            season_type: SeasonType {
                id: type_id,
                name: None,
            },
        }
    }

    #[test]
    fn enrich_series_excludes_specials_from_season_count() {
        let s = ExtendedSeries {
            id: 1,
            name: "X".into(),
            seasons: vec![
                season(0, 1), // specials → excluded from season_count
                season(1, 1),
                season(2, 1),
                season(3, 1),
            ],
            ..Default::default()
        };
        let r = extract_enrich_series(&s, EntryKind::Series);
        assert_eq!(r.entry.season_count, Some(3));
        // …but the specials presence is surfaced separately so the
        // TUI can render a "Specials" row after the canonical seasons.
        assert!(r.entry.has_specials, "season 0 in default order → has_specials=true");
    }

    #[test]
    fn enrich_series_no_season_zero_means_no_specials() {
        let s = ExtendedSeries {
            id: 1,
            name: "X".into(),
            seasons: vec![season(1, 1), season(2, 1)],
            ..Default::default()
        };
        let r = extract_enrich_series(&s, EntryKind::Series);
        assert!(!r.entry.has_specials);
    }

    #[test]
    fn enrich_series_alternate_order_specials_dont_count() {
        // Season 0 only present in DVD ordering shouldn't trigger
        // has_specials — we only honor the canonical aired order.
        let s = ExtendedSeries {
            id: 1,
            name: "X".into(),
            seasons: vec![
                season(0, 2), // DVD order specials — ignored
                season(1, 1),
                season(2, 1),
            ],
            ..Default::default()
        };
        let r = extract_enrich_series(&s, EntryKind::Series);
        assert!(!r.entry.has_specials);
    }

    #[test]
    fn enrich_series_filters_alternate_season_orderings() {
        let s = ExtendedSeries {
            id: 1,
            name: "X".into(),
            seasons: vec![
                season(1, 1),
                season(2, 1),
                season(1, 2), // DVD order — excluded
                season(2, 2),
                season(3, 2),
            ],
            ..Default::default()
        };
        let r = extract_enrich_series(&s, EntryKind::Series);
        // Only aired-order seasons count.
        assert_eq!(r.entry.season_count, Some(2));
    }

    #[test]
    fn enrich_series_emits_external_ids_for_imdb_and_tmdb() {
        let s = ExtendedSeries {
            id: 81189,
            name: "Breaking Bad".into(),
            remote_ids: vec![
                RemoteId {
                    id: "tt0903747".into(),
                    source_name: Some("IMDB".into()),
                },
                RemoteId {
                    id: "1396".into(),
                    source_name: Some("TheMovieDB.com".into()),
                },
            ],
            ..Default::default()
        };
        let r = extract_enrich_series(&s, EntryKind::Series);
        assert_eq!(r.entry.imdb_id.as_deref(), Some("tt0903747"));
        assert_eq!(
            r.entry.external_ids.get("tvdb").map(String::as_str),
            Some("81189")
        );
        assert_eq!(
            r.entry.external_ids.get("imdb").map(String::as_str),
            Some("tt0903747")
        );
        assert_eq!(
            r.entry.external_ids.get("tmdb").map(String::as_str),
            Some("1396")
        );
    }

    #[test]
    fn enrich_series_concatenates_genres() {
        let s = ExtendedSeries {
            id: 1,
            name: "X".into(),
            genres: vec![
                Genre {
                    name: "Drama".into(),
                },
                Genre {
                    name: "Crime".into(),
                },
            ],
            ..Default::default()
        };
        let r = extract_enrich_series(&s, EntryKind::Series);
        assert_eq!(r.entry.genre.as_deref(), Some("Drama, Crime"));
    }

    #[test]
    fn enrich_movie_omits_season_fields() {
        let m = ExtendedMovie {
            id: 100,
            name: "Inception".into(),
            year: Some("2010".into()),
            ..Default::default()
        };
        let r = extract_enrich_movie(&m, EntryKind::Movie);
        assert_eq!(r.entry.season_count, None);
        assert!(r.entry.season_ids.is_empty());
        assert_eq!(r.entry.year, Some(2010));
    }

    #[test]
    fn credits_splits_actors_and_crew_by_people_type() {
        let chars = vec![
            Character {
                person_name: Some("Bryan Cranston".into()),
                people_type: Some("Actor".into()),
                name: Some("Walter White".into()),
                image: None,
                sort: Some(1),
            },
            Character {
                person_name: Some("Aaron Paul".into()),
                people_type: Some("Actor".into()),
                name: Some("Jesse Pinkman".into()),
                image: None,
                sort: Some(2),
            },
            Character {
                person_name: Some("Vince Gilligan".into()),
                people_type: Some("Director".into()),
                name: None,
                image: None,
                sort: None,
            },
            Character {
                person_name: Some("Vince Gilligan".into()),
                people_type: Some("Writer".into()),
                name: None,
                image: None,
                sort: None,
            },
        ];
        let r = extract_credits(&chars);
        assert_eq!(r.cast.len(), 2);
        assert_eq!(r.cast[0].name, "Bryan Cranston");
        assert_eq!(r.cast[0].character.as_deref(), Some("Walter White"));
        assert_eq!(r.cast[0].billing_order, Some(1));
        assert_eq!(r.crew.len(), 2);
        assert_eq!(r.crew[0].name, "Vince Gilligan");
        assert_eq!(r.crew[0].department.as_deref(), Some("Director"));
    }

    #[test]
    fn credits_drops_rows_with_unknown_people_type() {
        let chars = vec![Character {
            person_name: Some("Mystery Person".into()),
            people_type: None,
            name: None,
            image: None,
            sort: None,
        }];
        let r = extract_credits(&chars);
        assert!(r.cast.is_empty());
        assert!(r.crew.is_empty());
    }

    #[test]
    fn artwork_maps_image_types_to_size_variants() {
        let arts = vec![
            Artwork {
                image: "p.jpg".into(),
                thumbnail: None,
                image_type: 2,
                language: None,
                score: None,
            },
            Artwork {
                image: "b.jpg".into(),
                thumbnail: None,
                image_type: 1,
                language: None,
                score: None,
            },
            Artwork {
                image: "bg.jpg".into(),
                thumbnail: None,
                image_type: 3,
                language: None,
                score: None,
            },
        ];
        let r = extract_artwork(&arts);
        assert_eq!(r.variants.len(), 3);
        assert_eq!(r.variants[0].size, ArtworkSize::Standard); // poster
        assert_eq!(r.variants[1].size, ArtworkSize::HiRes); // banner
        assert_eq!(r.variants[2].size, ArtworkSize::HiRes); // background
    }

    #[test]
    fn artwork_guesses_mime_from_extension() {
        let arts = vec![
            Artwork {
                image: "x.png".into(),
                thumbnail: None,
                image_type: 2,
                language: None,
                score: None,
            },
            Artwork {
                image: "x.webp".into(),
                thumbnail: None,
                image_type: 2,
                language: None,
                score: None,
            },
            Artwork {
                image: "x.jpg".into(),
                thumbnail: None,
                image_type: 2,
                language: None,
                score: None,
            },
        ];
        let r = extract_artwork(&arts);
        assert_eq!(r.variants[0].mime, "image/png");
        assert_eq!(r.variants[1].mime, "image/webp");
        assert_eq!(r.variants[2].mime, "image/jpeg");
    }

    #[test]
    fn artwork_drops_rows_with_empty_image_url() {
        let arts = vec![
            Artwork {
                image: "".into(),
                thumbnail: None,
                image_type: 2,
                language: None,
                score: None,
            },
            Artwork {
                image: "  ".into(),
                thumbnail: None,
                image_type: 2,
                language: None,
                score: None,
            },
            Artwork {
                image: "real.jpg".into(),
                thumbnail: None,
                image_type: 2,
                language: None,
                score: None,
            },
        ];
        let r = extract_artwork(&arts);
        assert_eq!(r.variants.len(), 1);
        assert_eq!(r.variants[0].url, "real.jpg");
    }
}
