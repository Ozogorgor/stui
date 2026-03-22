//! Quality ranking system for stream candidates.
//!
//! When multiple providers return streams for the same item, the engine
//! needs to rank them so the best option surfaces first.  This module
//! provides a composable scoring system.
//!
//! # Scoring model
//!
//! Each `StreamCandidate` receives a `QualityScore` (0–1000).  Higher is
//! better.  Scores are built by summing weighted sub-scores:
//!
//! | Factor         | Max points | Notes |
//! |----------------|-----------|-------|
//! | Resolution     | 400       | 4K=400, 1080p=300, 720p=200, SD=100 |
//! | Codec          | 150       | AV1>HEVC>H264>other |
//! | Seeders        | 150       | log-scaled, capped at 100 seeders |
//! | Bitrate        | 150       | higher is better, capped |
//! | Source         | 100       | BluRay > WEB-DL > HDTV > CAM |
//! | HDR            | 50        | bonus for HDR10 / Dolby Vision |
//!
//! The UI can re-rank at any time (e.g. the user prefers 720p over 4K
//! for a slow connection) by passing a custom `RankingPolicy`.

pub mod candidate;
pub mod policy;
pub mod score;
pub mod user_policy;

#[cfg(test)]
mod tests;

pub use candidate::StreamCandidate;
pub use policy::{RankingPolicy, StreamPreferences};
pub use score::QualityScore;
#[allow(unused_imports)]
pub use user_policy::{rank_streams, score_stream_policy, ScoredStream};

use crate::providers::Stream;

/// Rank a list of streams according to the given policy.
/// Returns them sorted best-first.
pub fn rank(streams: Vec<Stream>, policy: &RankingPolicy) -> Vec<StreamCandidate> {
    rank_with_health(streams, policy, None)
}

/// Rank streams, blending quality score with provider reliability.
///
/// `health` maps provider name → reliability score (0.0–1.0).
/// When provided, the final sort key is `blend_score(quality, reliability)`.
pub fn rank_with_health(
    streams: Vec<Stream>,
    policy: &RankingPolicy,
    health: Option<&std::collections::HashMap<String, f64>>,
) -> Vec<StreamCandidate> {
    rank_with_health_and_speed(streams, policy, health, None)
}

/// Rank streams with quality, provider reliability, and measured speed.
///
/// `health` maps provider name → reliability score (0.0–1.0).
/// `speed` maps stream URL → measured speed in Mbps.
/// Speed is blended into the score: streams with measured speed get a bonus.
pub fn rank_with_health_and_speed(
    streams: Vec<Stream>,
    policy: &RankingPolicy,
    health: Option<&std::collections::HashMap<String, f64>>,
    speed_map: Option<&std::collections::HashMap<String, f64>>,
) -> Vec<StreamCandidate> {
    use crate::providers::health::blend_score;

    let mut candidates: Vec<StreamCandidate> = streams
        .into_iter()
        .map(|s| {
            let score = QualityScore::from_stream(&s, policy);
            StreamCandidate { stream: s, score }
        })
        .collect();

    // Calculate max quality for normalization
    let max_quality = candidates
        .iter()
        .map(|c| c.score.total())
        .max()
        .unwrap_or(1)
        .max(1) as f64;

    candidates.sort_by(|a, b| {
        let qa = a.score.total() as f64 / max_quality;
        let qb = b.score.total() as f64 / max_quality;

        // Provider reliability component
        let ra = health
            .and_then(|h| h.get(a.stream.provider.as_str()).copied())
            .unwrap_or(1.0);
        let rb = health
            .and_then(|h| h.get(b.stream.provider.as_str()).copied())
            .unwrap_or(1.0);

        // Speed component (if measured)
        let speed_bonus = |url: &str| -> f64 {
            speed_map
                .and_then(|m| m.get(url).copied())
                .map(|speed| {
                    // Normalize speed: 50 Mbps = full bonus, linear scale below
                    // This gives faster streams a ranking boost
                    (speed / 50.0).min(1.0) * 0.1 // 10% max bonus
                })
                .unwrap_or(0.0)
        };

        let sa = blend_score(qa, ra) + speed_bonus(&a.stream.url);
        let sb = blend_score(qb, rb) + speed_bonus(&b.stream.url);

        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });

    candidates
}
