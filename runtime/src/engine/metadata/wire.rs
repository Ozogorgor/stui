//! ABI → wire conversion for merged per-verb responses.
//!
//! The orchestrator's `run_verb` produces merged `abi::types::*` values
//! (`EnrichResponse`, `CreditsResponse`, etc.); the IPC layer expects
//! `ipc::v1::MetadataPayload` with wire-friendly struct fields (e.g.
//! `Vec<CastWire>` not `Vec<CastMember>` — the wire form has no
//! `external_ids` map and flattens role enums to snake_case strings).
//!
//! Each `*_to_payload` helper does exactly that conversion and nothing
//! else.  Kept in its own file so the orchestrator stays focused on
//! fan-out / timeout / merge plumbing.

use crate::abi::types::{
    ArtworkResponse, ArtworkSize, ArtworkVariant, CastMember, CastRole, CreditsResponse,
    CrewMember, CrewRole, EnrichResponse, PluginEntry,
};
use crate::ipc::v1::{
    ArtworkData, ArtworkVariantWire, CastWire, CreditsData, CrewWire, EnrichData, MetadataPayload,
    RelatedData, RelatedItemWire,
};

// ── Enrich ───────────────────────────────────────────────────────────────────

/// Convert a merged `EnrichResponse` into a wire `MetadataPayload::Enrich`.
///
/// The ABI's `EnrichResponse` carries the whole `PluginEntry`; the wire
/// form only surfaces the handful of fields the media-card detail panel
/// needs (studio / networks / external_ids).  Those aren't yet present
/// on `PluginEntry` as typed fields, so for now we emit `EnrichData` with
/// just the IMDB id picked up into `external_ids` if present.  The rich
/// studio/networks plumbing lands alongside the plugin-side enrich
/// additions (tracked in the Chunk-7 TUI task).
pub(super) fn enrich_to_payload(resp: EnrichResponse) -> MetadataPayload {
    let mut data = EnrichData::default();
    // Forward every cross-provider id the plugin returned (anilist, mal,
    // tmdb, tvdb, kitsu, …).  This is the bridge that lets a kitsu-only
    // entry's enrich result feed an AniList native id back into the
    // orchestrator so credits/artwork/related can dispatch correctly.
    for (k, v) in &resp.entry.external_ids {
        data.external_ids.insert(k.clone(), v.clone());
    }
    // Some plugins populate the dedicated `imdb_id` field but not
    // `external_ids["imdb"]`.  Mirror it across so consumers don't need
    // to special-case both.
    if let Some(imdb) = resp.entry.imdb_id.clone() {
        data.external_ids.entry("imdb".into()).or_insert(imdb);
    }
    data.season_count = resp.entry.season_count;
    data.season_ids = resp.entry.season_ids.clone();
    data.has_specials = resp.entry.has_specials;
    tracing::info!(
        provider = %resp.entry.source,
        title = %resp.entry.title,
        season_count = ?resp.entry.season_count,
        season_ids = ?resp.entry.season_ids,
        has_specials = resp.entry.has_specials,
        "enrich_to_payload: emitting season_count"
    );
    MetadataPayload::Enrich(data)
}

// ── Credits ──────────────────────────────────────────────────────────────────

pub(super) fn credits_to_payload(resp: CreditsResponse) -> MetadataPayload {
    let cast = resp.cast.into_iter().map(cast_to_wire).collect();
    let crew = resp.crew.into_iter().map(crew_to_wire).collect();
    MetadataPayload::Credits(CreditsData { cast, crew })
}

fn cast_to_wire(c: CastMember) -> CastWire {
    CastWire {
        name: c.name,
        role: cast_role_to_str(&c.role),
        character: c.character,
        billing_order: c.billing_order,
    }
}

fn crew_to_wire(c: CrewMember) -> CrewWire {
    CrewWire {
        name: c.name,
        role: crew_role_to_str(&c.role),
        department: c.department,
    }
}

fn cast_role_to_str(r: &CastRole) -> String {
    match r {
        CastRole::Actor => "actor".into(),
        CastRole::Vocalist => "vocalist".into(),
        CastRole::FeaturedArtist => "featured_artist".into(),
        CastRole::GuestAppearance => "guest_appearance".into(),
        CastRole::Other(s) => s.clone(),
    }
}

fn crew_role_to_str(r: &CrewRole) -> String {
    match r {
        CrewRole::Director => "director".into(),
        CrewRole::Writer => "writer".into(),
        CrewRole::Producer => "producer".into(),
        CrewRole::ExecutiveProducer => "executive_producer".into(),
        CrewRole::Cinematographer => "cinematographer".into(),
        CrewRole::Editor => "editor".into(),
        CrewRole::Composer => "composer".into(),
        CrewRole::Songwriter => "songwriter".into(),
        CrewRole::Lyricist => "lyricist".into(),
        CrewRole::Arranger => "arranger".into(),
        CrewRole::Instrumentalist => "instrumentalist".into(),
        CrewRole::ProductionDesigner => "production_designer".into(),
        CrewRole::ArtDirector => "art_director".into(),
        CrewRole::CostumeDesigner => "costume_designer".into(),
        CrewRole::SoundDesigner => "sound_designer".into(),
        CrewRole::VfxSupervisor => "vfx_supervisor".into(),
        CrewRole::AnimationDirector => "animation_director".into(),
        CrewRole::LeadAnimator => "lead_animator".into(),
        CrewRole::Other(s) => s.clone(),
    }
}

// ── Artwork ──────────────────────────────────────────────────────────────────

/// Split variants into backdrops vs posters by aspect ratio (wider
/// than 1:1 is a backdrop).  When no width/height is known, the
/// variant goes into `posters` — matches Stremio's "when in doubt,
/// treat as poster" convention.
pub(super) fn artwork_to_payload(resp: ArtworkResponse) -> MetadataPayload {
    let mut backdrops = Vec::new();
    let mut posters = Vec::new();
    for v in resp.variants {
        let is_backdrop = match (v.width, v.height) {
            (Some(w), Some(h)) if h > 0 => w > h,
            _ => false,
        };
        let wire = artwork_variant_to_wire(v);
        if is_backdrop {
            backdrops.push(wire);
        } else {
            posters.push(wire);
        }
    }
    MetadataPayload::Artwork(ArtworkData { backdrops, posters })
}

fn artwork_variant_to_wire(v: ArtworkVariant) -> ArtworkVariantWire {
    ArtworkVariantWire {
        url: v.url,
        width: v.width,
        height: v.height,
        size_label: size_label(&v.size),
    }
}

fn size_label(s: &ArtworkSize) -> String {
    match s {
        ArtworkSize::Thumbnail => "thumbnail".into(),
        ArtworkSize::Standard => "standard".into(),
        ArtworkSize::HiRes => "hi_res".into(),
        ArtworkSize::Any => "any".into(),
    }
}

// ── Related ──────────────────────────────────────────────────────────────────

pub(super) fn related_to_payload(items: Vec<PluginEntry>) -> MetadataPayload {
    let wire_items = items.into_iter().map(related_entry_to_wire).collect();
    MetadataPayload::Related(RelatedData { items: wire_items })
}

fn related_entry_to_wire(e: PluginEntry) -> RelatedItemWire {
    RelatedItemWire {
        id: e.id,
        // `PluginEntry.source` is the originating plugin id (e.g. "tmdb"),
        // which doubles as the id-source namespace for this entry.
        id_source: e.source,
        title: e.title,
        year: e.year.and_then(|y| u16::try_from(y).ok()),
        poster_url: e.poster_url,
        kind: format!("{:?}", e.kind).to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::types::{ArtworkVariant, CastMember};
    use std::collections::HashMap;

    #[test]
    fn credits_empty_produces_credits_variant_with_empty_vecs() {
        let p = credits_to_payload(CreditsResponse {
            cast: vec![],
            crew: vec![],
        });
        assert_eq!(p, MetadataPayload::Credits(CreditsData::default()));
    }

    #[test]
    fn cast_member_roundtrips_through_wire() {
        let resp = CreditsResponse {
            cast: vec![CastMember {
                name: "Jane".into(),
                role: CastRole::Actor,
                character: Some("Hero".into()),
                instrument: None,
                billing_order: Some(1),
                external_ids: HashMap::new(),
            }],
            crew: vec![],
        };
        match credits_to_payload(resp) {
            MetadataPayload::Credits(d) => {
                assert_eq!(d.cast.len(), 1);
                assert_eq!(d.cast[0].name, "Jane");
                assert_eq!(d.cast[0].role, "actor");
                assert_eq!(d.cast[0].character.as_deref(), Some("Hero"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn artwork_splits_backdrop_vs_poster_by_aspect() {
        let resp = ArtworkResponse {
            variants: vec![
                ArtworkVariant {
                    size: ArtworkSize::HiRes,
                    url: "bd".into(),
                    mime: "image/jpeg".into(),
                    width: Some(1920),
                    height: Some(1080),
                },
                ArtworkVariant {
                    size: ArtworkSize::Standard,
                    url: "po".into(),
                    mime: "image/jpeg".into(),
                    width: Some(500),
                    height: Some(750),
                },
                ArtworkVariant {
                    size: ArtworkSize::Any,
                    url: "unk".into(),
                    mime: "image/jpeg".into(),
                    width: None,
                    height: None,
                },
            ],
        };
        match artwork_to_payload(resp) {
            MetadataPayload::Artwork(d) => {
                assert_eq!(d.backdrops.len(), 1);
                assert_eq!(d.backdrops[0].url, "bd");
                assert_eq!(d.posters.len(), 2);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn crew_role_snake_cases_enum() {
        assert_eq!(
            crew_role_to_str(&CrewRole::AnimationDirector),
            "animation_director"
        );
        assert_eq!(crew_role_to_str(&CrewRole::Director), "director");
        assert_eq!(crew_role_to_str(&CrewRole::Other("sound".into())), "sound");
    }
}
