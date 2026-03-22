//! `MediaItem` — the universal atom returned by every provider and plugin.
//!
//! The UI doesn't care whether data came from TMDB, a torrent index, or a
//! local file scanner — it always receives a `MediaItem`.  Type-specific
//! extras live in `EpisodeInfo` / `TrackInfo` attached fields.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{EpisodeInfo, MediaId, MediaType, TrackInfo};

/// A single piece of media content, provider-agnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    // ── Identity ──────────────────────────────────────────────────────────
    /// Namespaced identifier: `"tmdb:movie:tt0816692"`, `"local:/path/to/file"`.
    pub id: MediaId,

    /// Human-readable title.
    pub title: String,

    /// Coarse media classification.
    pub media_type: MediaType,

    // ── Core metadata ─────────────────────────────────────────────────────
    /// Release year (or first air year for series).
    pub year: Option<u32>,

    /// Short description or synopsis.
    pub description: Option<String>,

    /// Comma-separated genre labels (e.g. "Action, Sci-Fi").
    pub genres: Option<String>,

    /// Weighted composite rating string (e.g. "8.3") — computed by the aggregator.
    pub rating: Option<String>,

    /// Per-source raw scores.  Keys match the provider name or sub-score id
    /// (e.g. "tomatometer", "audience_score", "imdb", "tmdb", "anilist").
    /// Values are in their native scale (RT: 0–100, IMDB/TMDB: 0–10, AniList: 0–100).
    #[serde(default)]
    pub ratings: HashMap<String, f64>,

    /// Poster image URL (fetched and rendered separately).
    pub poster_url: Option<String>,

    /// Pre-rendered ANSI art (populated after first poster fetch, cached).
    pub poster_art: Option<String>,

    // ── Cross-database IDs ────────────────────────────────────────────────
    /// IMDB `tt` identifier, used for cross-provider dedup and subtitle lookup.
    pub imdb_id: Option<String>,

    /// TMDB numeric identifier.
    pub tmdb_id: Option<u64>,

    // ── Type-specific extensions ──────────────────────────────────────────
    /// Set when `media_type == Episode`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode: Option<EpisodeInfo>,

    /// Set when `media_type == Track`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackInfo>,

    // ── Origin ────────────────────────────────────────────────────────────
    /// Which provider produced this item.
    pub provider: String,

    /// The tab this item belongs to ("movies", "series", "music", "library").
    pub tab: String,
}

impl MediaItem {
    #[allow(dead_code)]
    /// Deduplication key: prefer IMDB id, fall back to normalised title+year.
    pub fn dedup_key(&self) -> String {
        if let Some(ref id) = self.imdb_id {
            return id.clone();
        }
        format!(
            "{}:{}",
            self.title.to_lowercase().trim().replace(' ', "-"),
            self.year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "?".into()),
        )
    }

    #[allow(dead_code)]
    /// True if this item has enough data to display in the grid.
    pub fn is_displayable(&self) -> bool {
        !self.title.is_empty()
    }
}

// ── Conversion from catalog::CatalogEntry ────────────────────────────────────

use crate::catalog::CatalogEntry;

impl From<CatalogEntry> for MediaItem {
    fn from(e: CatalogEntry) -> Self {
        let year = e.year.as_deref().and_then(|y| y.parse().ok());
        MediaItem {
            id: MediaId::new(&e.provider, &e.id),
            title: e.title,
            media_type: e.media_type,
            year,
            description: e.description,
            genres: e.genre,
            rating: e.rating,
            ratings: e.ratings,
            poster_url: e.poster_url,
            poster_art: e.poster_art,
            imdb_id: e.imdb_id,
            tmdb_id: e.tmdb_id,
            episode: None,
            track: None,
            provider: e.provider,
            tab: e.tab,
        }
    }
}

impl From<MediaItem> for CatalogEntry {
    fn from(m: MediaItem) -> Self {
        CatalogEntry {
            id: m.id.key.clone(),
            title: m.title,
            year: m.year.map(|y| y.to_string()),
            genre: m.genres,
            rating: m.rating,
            ratings: m.ratings,
            description: m.description,
            poster_url: m.poster_url,
            poster_art: m.poster_art,
            provider: m.provider,
            tab: m.tab,
            imdb_id: m.imdb_id,
            tmdb_id: m.tmdb_id,
            media_type: m.media_type,
        }
    }
}
