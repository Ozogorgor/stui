//! Ranking policy — controls how quality sub-scores are weighted.
//!
//! The default policy maximises quality (prefer 4K HEVC BluRay).
//! The user can override via the settings panel or a per-session command.
//!
//! # Pre-built policies
//!
//! ```rust
//! RankingPolicy::default()         // best quality first
//! RankingPolicy::bandwidth_saver() // prefer 720p to reduce buffering
//! RankingPolicy::fastest_start()   // weight seeders heavily
//! ```

use serde::{Deserialize, Serialize};

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

    /// Minimum acceptable seeder count for torrent streams (0 = no filter).
    pub min_seeders: u32,
}

impl Default for RankingPolicy {
    fn default() -> Self {
        RankingPolicy {
            resolution_weights:       [100, 200, 300, 400],
            prefer_lower_resolution:  false,
            seeder_weight:            1.0,
            exclude_cam:              true,
            min_seeders:              0,
        }
    }
}

impl RankingPolicy {
    /// Prefer 720p — good for slower connections or smaller screens.
    pub fn bandwidth_saver() -> Self {
        RankingPolicy {
            resolution_weights:       [100, 400, 200, 100], // 720p gets top weight
            prefer_lower_resolution:  true,
            seeder_weight:            1.2,
            exclude_cam:              true,
            min_seeders:              5,
        }
    }

    /// Maximise seeder count — minimises buffering at the cost of quality.
    pub fn fastest_start() -> Self {
        RankingPolicy {
            resolution_weights:       [100, 150, 200, 220],
            prefer_lower_resolution:  false,
            seeder_weight:            3.0,
            exclude_cam:              true,
            min_seeders:              10,
        }
    }
}
