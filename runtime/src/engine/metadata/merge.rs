//! Pure merge functions for per-verb metadata responses.
//!
//! Each function takes the primary source's response (or `None` if the
//! primary failed / returned nothing) plus any fallback-source responses,
//! and folds them into a single canonical payload:
//!
//! * [`merge_enrich`] — scalar-fallback on `EnrichResponse.entry`'s `Option`
//!   fields (primary first, then first secondary with `Some`).
//! * [`merge_credits`] — unions cast + crew with dedup by `(name, role,
//!   character)` and `(name, role)` respectively; same person in two
//!   distinct roles stays as two rows.
//! * [`merge_artwork`] — unions variants, dedups by `url`, sorts by
//!   `(size desc, width desc)`.
//! * [`merge_related`] — flattens per-source lists, dedups by
//!   `(id_source, id)`, preserves input ordering.
//!
//! All dedup is "first-wins": the earlier (higher-priority) source
//! stays, later duplicates drop.

use std::collections::HashSet;

use crate::abi::types::{
    ArtworkResponse, ArtworkSize, ArtworkVariant, CastMember, CreditsResponse, CrewMember,
    EnrichResponse, PluginEntry,
};

// ── Enrich ───────────────────────────────────────────────────────────────────

/// Merge enrich responses using scalar-fallback on `PluginEntry` Option fields.
///
/// Walks primary first; for each `None` field, fills from the first
/// secondary that has `Some`. `confidence` is taken from the primary
/// (or the first secondary if no primary).
pub fn merge_enrich(
    primary: Option<EnrichResponse>,
    secondaries: Vec<EnrichResponse>,
) -> EnrichResponse {
    let base = match primary {
        Some(p) => p,
        None => {
            let mut it = secondaries.into_iter();
            return match it.next() {
                Some(first) => fill_from_secondaries(first, it.collect()),
                None => EnrichResponse {
                    entry: PluginEntry::default(),
                    confidence: 0.0,
                },
            };
        }
    };
    fill_from_secondaries(base, secondaries)
}

fn fill_from_secondaries(
    mut base: EnrichResponse,
    secondaries: Vec<EnrichResponse>,
) -> EnrichResponse {
    // Scalar Option fields on PluginEntry: year, genre, rating, description,
    // poster_url, imdb_id, duration, artist_name, album_name, track_number,
    // season, episode, original_language.
    macro_rules! fill {
        ($field:ident) => {
            if base.entry.$field.is_none() {
                for s in &secondaries {
                    if s.entry.$field.is_some() {
                        base.entry.$field = s.entry.$field.clone();
                        break;
                    }
                }
            }
        };
    }
    fill!(year);
    fill!(genre);
    fill!(rating);
    fill!(description);
    fill!(poster_url);
    fill!(imdb_id);
    fill!(duration);
    fill!(artist_name);
    fill!(album_name);
    fill!(track_number);
    fill!(season);
    fill!(episode);
    fill!(original_language);
    // Union cross-provider native ids across all responses. Without this
    // a kitsu primary that found nothing would shadow an anilist
    // secondary that resolved an anilist id (which is how the
    // kitsu-only-entry → AniList bridge has to land).
    for s in &secondaries {
        for (k, v) in &s.entry.external_ids {
            base.entry
                .external_ids
                .entry(k.clone())
                .or_insert_with(|| v.clone());
        }
    }
    base
}

// ── Credits ──────────────────────────────────────────────────────────────────

/// Union cast + crew from primary and secondaries with first-wins dedup.
///
/// Cast dedup key: `(name, role, character)`. Crew dedup key: `(name, role)`.
/// Same person in different roles is preserved (two rows). Cast is sorted by
/// `billing_order.unwrap_or(u32::MAX)` ascending; crew by
/// `(department, role, name)`.
pub fn merge_credits(
    primary: Option<CreditsResponse>,
    secondaries: Vec<CreditsResponse>,
) -> CreditsResponse {
    let mut cast: Vec<CastMember> = Vec::new();
    let mut crew: Vec<CrewMember> = Vec::new();
    let mut cast_seen: HashSet<(String, String, Option<String>)> = HashSet::new();
    let mut crew_seen: HashSet<(String, String)> = HashSet::new();

    let sources = primary.into_iter().chain(secondaries.into_iter());
    for src in sources {
        for c in src.cast {
            let key = (c.name.clone(), format!("{:?}", c.role), c.character.clone());
            if cast_seen.insert(key) {
                cast.push(c);
            }
        }
        for w in src.crew {
            let key = (w.name.clone(), format!("{:?}", w.role));
            if crew_seen.insert(key) {
                crew.push(w);
            }
        }
    }

    cast.sort_by_key(|c| c.billing_order.unwrap_or(u32::MAX));
    crew.sort_by(|a, b| {
        let ad = a.department.as_deref().unwrap_or("");
        let bd = b.department.as_deref().unwrap_or("");
        ad.cmp(bd)
            .then_with(|| format!("{:?}", a.role).cmp(&format!("{:?}", b.role)))
            .then_with(|| a.name.cmp(&b.name))
    });

    CreditsResponse { cast, crew }
}

// ── Artwork ──────────────────────────────────────────────────────────────────

/// Collect all artwork variants from every source, dedup by `url`,
/// sort by `(size desc, width desc)`.
pub fn merge_artwork(responses: Vec<ArtworkResponse>) -> ArtworkResponse {
    let mut variants: Vec<ArtworkVariant> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in responses {
        for v in r.variants {
            if seen.insert(v.url.clone()) {
                variants.push(v);
            }
        }
    }
    variants.sort_by(|a, b| {
        size_ord(b.size)
            .cmp(&size_ord(a.size))
            .then_with(|| b.width.unwrap_or(0).cmp(&a.width.unwrap_or(0)))
    });
    ArtworkResponse { variants }
}

fn size_ord(s: ArtworkSize) -> u8 {
    match s {
        ArtworkSize::HiRes => 3,
        ArtworkSize::Standard => 2,
        ArtworkSize::Thumbnail => 1,
        ArtworkSize::Any => 0,
    }
}

// ── Related ──────────────────────────────────────────────────────────────────

/// Flatten per-source related-item lists, dedup by `(source, id)`,
/// preserve input ordering (no sort — ranking is the source's job).
pub fn merge_related(items: Vec<Vec<PluginEntry>>) -> Vec<PluginEntry> {
    let mut out: Vec<PluginEntry> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for source_list in items {
        for entry in source_list {
            let key = (entry.source.clone(), entry.id.clone());
            if seen.insert(key) {
                out.push(entry);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::types::{ArtworkSize, ArtworkVariant, CastMember, CastRole, CrewRole};
    use std::collections::HashMap;
    use stui_plugin_sdk::EntryKind;

    fn cm(name: &str, role: CrewRole) -> CrewMember {
        CrewMember {
            name: name.into(),
            role,
            department: None,
            external_ids: HashMap::new(),
        }
    }

    fn cast(name: &str, character: Option<&str>, order: Option<u32>) -> CastMember {
        CastMember {
            name: name.into(),
            role: CastRole::Actor,
            character: character.map(|s| s.into()),
            instrument: None,
            billing_order: order,
            external_ids: HashMap::new(),
        }
    }

    fn entry(id: &str, source: &str, title: &str) -> PluginEntry {
        PluginEntry {
            id: id.into(),
            kind: EntryKind::Movie,
            title: title.into(),
            source: source.into(),
            ..Default::default()
        }
    }

    fn av(url: &str, size: ArtworkSize, width: Option<u32>) -> ArtworkVariant {
        ArtworkVariant {
            size,
            url: url.into(),
            mime: "image/jpeg".into(),
            width,
            height: None,
        }
    }

    // ── Credits ──────────────────────────────────────────────────────────────

    #[test]
    fn merge_credits_unions_crew_with_dedup_by_name_role() {
        let a = CreditsResponse {
            cast: vec![],
            crew: vec![cm("Nolan", CrewRole::Director)],
        };
        let b = CreditsResponse {
            cast: vec![],
            crew: vec![
                cm("Nolan", CrewRole::Director),
                cm("Pfister", CrewRole::Cinematographer),
            ],
        };
        let merged = merge_credits(Some(a), vec![b]);
        assert_eq!(merged.crew.len(), 2);
        assert!(merged.crew.iter().any(|c| c.name == "Pfister"));
    }

    #[test]
    fn merge_credits_primary_missing_uses_secondaries() {
        let merged = merge_credits(
            None,
            vec![CreditsResponse {
                cast: vec![],
                crew: vec![cm("Kubrick", CrewRole::Director)],
            }],
        );
        assert_eq!(merged.crew.len(), 1);
    }

    #[test]
    fn merge_credits_same_name_different_role_keeps_both_rows() {
        let a = CreditsResponse {
            cast: vec![],
            crew: vec![
                cm("Nolan", CrewRole::Director),
                cm("Nolan", CrewRole::Writer),
            ],
        };
        let merged = merge_credits(Some(a), vec![]);
        assert_eq!(merged.crew.len(), 2);
    }

    #[test]
    fn merge_credits_cast_sorted_by_billing_order() {
        let a = CreditsResponse {
            cast: vec![
                cast("A", Some("alpha"), Some(3)),
                cast("B", Some("beta"), Some(1)),
                cast("C", Some("gamma"), None),
            ],
            crew: vec![],
        };
        let merged = merge_credits(Some(a), vec![]);
        assert_eq!(merged.cast[0].name, "B");
        assert_eq!(merged.cast[1].name, "A");
        assert_eq!(merged.cast[2].name, "C"); // None -> u32::MAX last
    }

    #[test]
    fn merge_credits_cast_same_name_role_different_character_keeps_both() {
        // Dedup key is (name, role, character) — so Hero vs Villain stays.
        let a = CreditsResponse {
            cast: vec![
                cast("Jane", Some("Hero"), Some(1)),
                cast("Jane", Some("Villain"), Some(2)),
            ],
            crew: vec![],
        };
        let merged = merge_credits(Some(a), vec![]);
        assert_eq!(merged.cast.len(), 2);
    }

    // ── Artwork ──────────────────────────────────────────────────────────────

    #[test]
    fn merge_artwork_dedups_by_url() {
        let a = ArtworkResponse {
            variants: vec![av("https://x/poster.jpg", ArtworkSize::Standard, Some(500))],
        };
        let b = ArtworkResponse {
            variants: vec![
                av("https://x/poster.jpg", ArtworkSize::Standard, Some(500)),
                av("https://y/hires.jpg", ArtworkSize::HiRes, Some(2000)),
            ],
        };
        let merged = merge_artwork(vec![a, b]);
        assert_eq!(merged.variants.len(), 2);
        // HiRes sorts before Standard.
        assert_eq!(merged.variants[0].url, "https://y/hires.jpg");
    }

    #[test]
    fn merge_artwork_sorts_by_size_then_width() {
        let resp = ArtworkResponse {
            variants: vec![
                av("a", ArtworkSize::Standard, Some(500)),
                av("b", ArtworkSize::Standard, Some(800)),
                av("c", ArtworkSize::Thumbnail, Some(200)),
                av("d", ArtworkSize::HiRes, None),
            ],
        };
        let merged = merge_artwork(vec![resp]);
        assert_eq!(merged.variants[0].url, "d"); // HiRes first
        assert_eq!(merged.variants[1].url, "b"); // Standard, wider
        assert_eq!(merged.variants[2].url, "a"); // Standard, narrower
        assert_eq!(merged.variants[3].url, "c"); // Thumbnail
    }

    // ── Related ──────────────────────────────────────────────────────────────

    #[test]
    fn merge_related_dedups_by_source_id_pair() {
        let a = vec![entry("tt1", "tmdb", "A"), entry("tt2", "tmdb", "B")];
        let b = vec![entry("tt1", "tmdb", "A-dup"), entry("tt3", "omdb", "C")];
        let merged = merge_related(vec![a, b]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].title, "A"); // first-wins
    }

    #[test]
    fn merge_related_preserves_source_order() {
        let a = vec![entry("tt2", "tmdb", "B"), entry("tt1", "tmdb", "A")];
        let merged = merge_related(vec![a]);
        assert_eq!(merged[0].id, "tt2");
        assert_eq!(merged[1].id, "tt1");
    }

    // ── Enrich ───────────────────────────────────────────────────────────────

    #[test]
    fn merge_enrich_scalar_fallback_from_secondary_when_primary_has_none() {
        let primary = EnrichResponse {
            entry: PluginEntry {
                id: "tt1".into(),
                kind: EntryKind::Movie,
                title: "T".into(),
                source: "tmdb".into(),
                year: None,
                genre: None,
                ..Default::default()
            },
            confidence: 0.9,
        };
        let secondary = EnrichResponse {
            entry: PluginEntry {
                id: "tt1".into(),
                kind: EntryKind::Movie,
                title: "T".into(),
                source: "omdb".into(),
                year: Some(1999),
                genre: Some("Sci-Fi".into()),
                ..Default::default()
            },
            confidence: 0.5,
        };
        let merged = merge_enrich(Some(primary), vec![secondary]);
        assert_eq!(merged.entry.year, Some(1999));
        assert_eq!(merged.entry.genre.as_deref(), Some("Sci-Fi"));
        assert_eq!(merged.confidence, 0.9); // primary's confidence stays
    }

    #[test]
    fn merge_enrich_primary_wins_when_both_have_some() {
        let primary = EnrichResponse {
            entry: PluginEntry {
                id: "tt1".into(),
                kind: EntryKind::Movie,
                title: "T".into(),
                source: "tmdb".into(),
                year: Some(1999),
                ..Default::default()
            },
            confidence: 0.9,
        };
        let secondary = EnrichResponse {
            entry: PluginEntry {
                id: "tt1".into(),
                kind: EntryKind::Movie,
                title: "T".into(),
                source: "omdb".into(),
                year: Some(1998),
                ..Default::default()
            },
            confidence: 0.5,
        };
        let merged = merge_enrich(Some(primary), vec![secondary]);
        assert_eq!(merged.entry.year, Some(1999));
    }

    #[test]
    fn merge_enrich_no_primary_uses_first_secondary_as_base() {
        let s = EnrichResponse {
            entry: PluginEntry {
                id: "tt1".into(),
                kind: EntryKind::Movie,
                title: "T".into(),
                source: "omdb".into(),
                year: Some(1999),
                ..Default::default()
            },
            confidence: 0.4,
        };
        let merged = merge_enrich(None, vec![s]);
        assert_eq!(merged.entry.year, Some(1999));
        assert_eq!(merged.confidence, 0.4);
    }
}
