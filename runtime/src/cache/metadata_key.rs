//! Cache key shape shared between metadata verb caches and the
//! request-deduplication table in the orchestrator.

use serde::{Deserialize, Serialize};

/// Which metadata verb the cached payload came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataVerb {
    Enrich,
    Credits,
    Artwork,
    Related,
    RatingsAggregator,
}

/// Which external catalog namespace the `id` field belongs to.
/// Keeping this local to the cache module so we aren't coupled to a
/// specific ABI re-export; extend via `Other(String)` if needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdSource {
    Imdb,
    Tmdb,
    Tvdb,
    Anilist,
    Kitsu,
    Musicbrainz,
    Discogs,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataCacheKey {
    pub verb: MetadataVerb,
    pub id_source: IdSource,
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_serializes_round_trip() {
        let k = MetadataCacheKey {
            verb: MetadataVerb::Credits,
            id_source: IdSource::Imdb,
            id: "tt0133093".into(),
        };
        let s = serde_json::to_string(&k).unwrap();
        let back: MetadataCacheKey = serde_json::from_str(&s).unwrap();
        assert_eq!(k, back);
    }

    #[test]
    fn verb_has_four_variants() {
        let _v: [MetadataVerb; 4] = [
            MetadataVerb::Enrich,
            MetadataVerb::Credits,
            MetadataVerb::Artwork,
            MetadataVerb::Related,
        ];
    }
}
