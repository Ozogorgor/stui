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
//! Rating aggregation is handled by `catalog_engine::aggregator` — see
//! `weighted_median` function and the public `WEIGHTS_MOVIE` constant there.

use serde::{Deserialize, Serialize};

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
