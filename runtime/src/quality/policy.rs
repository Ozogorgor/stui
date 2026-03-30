//! Ranking policy — controls how quality sub-scores are weighted.
//!
//! The default policy maximises quality (prefer 4K HEVC BluRay).
//! The user can override via the settings panel or a per-session command.
//!
//! # Pre-built policies
//!
//! ```text
//! RankingPolicy::default()         # best quality first
//! RankingPolicy::bandwidth_saver() # prefer 720p to reduce buffering
//! RankingPolicy::fastest_start()  # weight seeders heavily (test-only stub)
//! ```
//!
//! # Rating weights
//!
//! Rating aggregation uses `RatingWeight` to combine scores from multiple
//! sources (TMDB, IMDB, etc.) into a single weighted value.
//! See `catalog_engine::aggregator::weighted_rating` and `weighted_median`.

use serde::{Deserialize, Serialize};

/// A source + weight pair used for aggregating ratings from multiple sources.
/// Used by `catalog_engine::aggregator::weighted_rating` and `weighted_median`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RatingWeight {
    /// Key matching the source name in the ratings HashMap (e.g., "tmdb", "imdb").
    pub key: &'static str,
    /// Weight for this source in the aggregation.
    pub weight: f64,
    /// Normalization factor (e.g., 10.0 for sources on 0-10 scale).
    pub normalize: f64,
}

/// Default rating weights for weighted rating aggregation.
/// TMDB and IMDB ratings are weighted higher as they tend to be most reliable.
#[allow(dead_code)]
pub fn default_rating_weights() -> Vec<RatingWeight> {
    vec![
        RatingWeight {
            key: "tmdb",
            weight: 3.0,
            normalize: 10.0,
        },
        RatingWeight {
            key: "imdb",
            weight: 2.5,
            normalize: 10.0,
        },
        RatingWeight {
            key: "rotten_tomatoes",
            weight: 2.0,
            normalize: 10.0,
        },
        RatingWeight {
            key: "metacritic",
            weight: 1.5,
            normalize: 10.0,
        },
    ]
}

/// Rating weights optimized for series/TV content.
#[allow(dead_code)]
pub fn series_rating_weights() -> Vec<RatingWeight> {
    vec![
        RatingWeight {
            key: "imdb",
            weight: 3.0,
            normalize: 10.0,
        },
        RatingWeight {
            key: "tmdb",
            weight: 2.5,
            normalize: 10.0,
        },
        RatingWeight {
            key: "metacritic",
            weight: 2.0,
            normalize: 100.0,
        },
    ]
}

/// Rating weights optimized for anime content.
#[allow(dead_code)]
pub fn anime_rating_weights() -> Vec<RatingWeight> {
    vec![
        RatingWeight {
            key: "myanimelist",
            weight: 3.0,
            normalize: 10.0,
        },
        RatingWeight {
            key: "anilist",
            weight: 2.5,
            normalize: 10.0,
        },
        RatingWeight {
            key: "imdb",
            weight: 1.5,
            normalize: 10.0,
        },
        RatingWeight {
            key: "tmdb",
            weight: 1.0,
            normalize: 10.0,
        },
    ]
}

/// User-configurable preferences for stream selection.
/// These complement the built-in ranking policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPreferences {
    /// Preferred protocol: "torrent", "http", or "" for no preference.
    #[serde(default)]
    pub prefer_protocol: Option<String>,

    /// Maximum resolution cap: "4k", "1080p", "720p", or None for no cap.
    #[serde(default)]
    pub max_resolution: Option<String>,

    /// Maximum file size in MB (0 = no limit).
    #[serde(default)]
    pub max_size_mb: u64,

    /// Minimum seeder count (0 = no minimum).
    #[serde(default)]
    pub min_seeders: u32,

    /// Labels to avoid (case-insensitive substrings).
    #[serde(default)]
    pub avoid_labels: Vec<String>,

    /// Prefer HDR streams.
    #[serde(default)]
    pub prefer_hdr: bool,

    /// Preferred codecs (case-insensitive).
    #[serde(default)]
    pub prefer_codecs: Vec<String>,
}

impl Default for StreamPreferences {
    fn default() -> Self {
        StreamPreferences {
            prefer_protocol: None,
            max_resolution: None,
            max_size_mb: 0,
            min_seeders: 0,
            avoid_labels: vec![
                "cam".to_string(),
                "telesync".to_string(),
                " ts ".to_string(),
            ],
            prefer_hdr: false,
            prefer_codecs: Vec::new(),
        }
    }
}

/// Weights and preferences used by `QualityScore::from_stream`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingPolicy {
    /// Points for [SD, 720p, 1080p, 4K] respectively.
    pub resolution_weights: [u32; 4],

    /// If true, prefer 720p over 1080p/4K (bandwidth saver mode).
    pub prefer_lower_resolution: bool,

    /// Weight multiplier for seeder count (1.0 = default).
    pub seeder_weight: f64,

    /// If true, filter out CAM/HDCAM sources entirely.
    pub exclude_cam: bool,

    /// User preferences for policy-based scoring.
    #[serde(default)]
    pub preferences: StreamPreferences,
}

impl Default for RankingPolicy {
    fn default() -> Self {
        RankingPolicy {
            resolution_weights: [100, 200, 300, 400],
            prefer_lower_resolution: false,
            seeder_weight: 1.0,
            exclude_cam: true,
            preferences: StreamPreferences::default(),
        }
    }
}

impl RankingPolicy {
    /// Prefer 720p — good for slower connections or smaller screens.
    pub fn bandwidth_saver() -> Self {
        RankingPolicy {
            resolution_weights: [100, 400, 200, 100],
            prefer_lower_resolution: true,
            seeder_weight: 1.2,
            exclude_cam: true,
            preferences: StreamPreferences {
                min_seeders: 5,
                ..StreamPreferences::default()
            },
        }
    }

    /// Maximise seeder count — minimises buffering at the cost of quality.
    /// Note: Currently a stub. To integrate with production pipeline, add a caller
    /// from pipeline.rs similar to how bandwidth_saver() is used.
    #[cfg(test)]
    pub fn fastest_start() -> Self {
        RankingPolicy {
            resolution_weights: [100, 150, 200, 220],
            prefer_lower_resolution: false,
            seeder_weight: 3.0,
            exclude_cam: true,
            preferences: StreamPreferences {
                min_seeders: 10,
                ..StreamPreferences::default()
            },
        }
    }
}
