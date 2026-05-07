//! Wire types for the metadata-enrichment IPC flow.
//!
//! Kept in its own sub-file to avoid bloating the already-crowded
//! `ipc::v1::mod.rs` (110+ variants today).
//!
//! The flow is:
//!   1. TUI sends `Request::GetDetailMetadata(GetDetailMetadataRequest)`.
//!   2. Runtime orchestrator fans out the four verbs in parallel.
//!   3. As each verb's merge finishes, the runtime emits a
//!      `Response::DetailMetadataPartial(DetailMetadataPartial)` carrying
//!      the per-verb `MetadataPayload`.  Partials stream back out-of-order
//!      (whichever verb finishes first wins).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Request to fetch enriched detail metadata for a single entry.
///
/// `id_source` / `kind` are wire-form strings; the runtime maps `id_source`
/// to `crate::cache::metadata_key::IdSource` and passes `kind` straight
/// through to the `SourceResolver`.
///
/// `title`, `year`, and `external_ids` are forwarded to per-plugin verb
/// requests so the runtime's enrich stage can title-search when the entry
/// arrived from one provider but the metadata source list points at
/// another (e.g. a `kitsu-…` entry whose richer credits live in AniList).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetDetailMetadataRequest {
    pub entry_id: String,
    /// `"imdb" | "tmdb" | "tvdb" | "anilist" | "kitsu" | "musicbrainz" | "discogs" | ...`
    pub id_source: String,
    /// `"movies" | "series" | "anime" | "music"`
    pub kind: String,
    /// Display title — used by enrich title-search fallbacks when no
    /// native id exists for a given metadata source.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Release year (4-digit) — same purpose as `title`. Optional because
    /// some music entries don't carry a year.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// Pre-known cross-provider ids (e.g. `{"anilist": "5114", "kitsu": "1376"}`)
    /// from the catalog merge. The orchestrator routes each verb call to
    /// its plugin using these when available, falling back to
    /// `(entry_id, id_source)` otherwise.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_ids: BTreeMap<String, String>,
}

/// One merged per-verb payload streamed back to the TUI as soon as its
/// fan-out + merge finishes.  Multiple partials (one per verb) arrive
/// out-of-order per request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailMetadataPartial {
    pub entry_id: String,
    /// `"enrich" | "credits" | "artwork" | "related"`
    pub verb: String,
    pub payload: MetadataPayload,
}

/// Per-verb payload.  `Empty` means "we tried and there's nothing" or
/// "no sources available"; the TUI renders that as "(none)".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MetadataPayload {
    Empty,
    Enrich(EnrichData),
    Credits(CreditsData),
    Artwork(ArtworkData),
    Related(RelatedData),
    RatingsAggregator(RatingsAggregatorData),
}

/// Pre-formatted, human-readable ratings block sourced from the elfhosted
/// Stremio rating-aggregator addon. The `description` field is the addon's
/// own emoji-formatted multi-line string (IMDb / TMDb / Metacritic /
/// Rotten Tomatoes / parent-safe flag) — the TUI renders it verbatim
/// rather than parsing per-source values, since this view is presentation,
/// not data the composite rating consumes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RatingsAggregatorData {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EnrichData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_ids: BTreeMap<String, String>,
    /// For series entries: total seasons reported by the provider.
    /// Forwarded from `PluginEntry::season_count` so the TUI's episode
    /// browser can populate its season list with the real count rather
    /// than a single-season default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_count: Option<u32>,
    /// Per-season provider-native ids (AniList-style multi-entry shows).
    /// Empty for TMDB-style providers. See `PluginEntry::season_ids`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub season_ids: Vec<String>,
    /// True when the provider has Specials content (TVDB season 0).
    /// Forwarded from `PluginEntry::has_specials`.
    #[serde(default)]
    pub has_specials: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CreditsData {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cast: Vec<CastWire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crew: Vec<CrewWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastWire {
    pub name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_order: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrewWire {
    pub name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ArtworkData {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backdrops: Vec<ArtworkVariantWire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub posters: Vec<ArtworkVariantWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtworkVariantWire {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub size_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RelatedData {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<RelatedItemWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelatedItemWire {
    pub id: String,
    pub id_source: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    pub kind: String,
}

/// Convert a crew role string (wire form, `snake_case`) to a humanized
/// label suitable for rendering in the TUI (e.g. `"animation_director"`
/// -> `"Animation Director"`).  Kept near the wire types so the TUI
/// and any Rust-side formatter share one canonical mapping.
#[allow(dead_code)] // TUI side (Chunk 7) will be the first real caller.
pub fn humanize_role(wire: &str) -> String {
    wire.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|first| first.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_payload_empty_round_trips() {
        let p = MetadataPayload::Empty;
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"type\":\"empty\""));
        let back: MetadataPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back, MetadataPayload::Empty);
    }

    #[test]
    fn metadata_payload_credits_round_trips() {
        let p = MetadataPayload::Credits(CreditsData {
            cast: vec![CastWire {
                name: "Jane".into(),
                role: "actor".into(),
                character: Some("Hero".into()),
                billing_order: Some(1),
            }],
            crew: vec![CrewWire {
                name: "Nolan".into(),
                role: "director".into(),
                department: None,
            }],
        });
        let s = serde_json::to_string(&p).unwrap();
        let back: MetadataPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn humanize_role_capitalizes_underscored() {
        assert_eq!(humanize_role("animation_director"), "Animation Director");
        assert_eq!(humanize_role("director"), "Director");
        assert_eq!(humanize_role(""), "");
    }
}
