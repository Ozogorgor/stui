//! Policy-based stream scoring with human-readable explanations.
//!
//! This module extends the quality scoring system with user-configurable
//! policy preferences, providing both a numeric score and human-readable
//! explanations for each scoring decision.

use super::policy::RankingPolicy;
use crate::ipc::StreamInfoWire;

pub struct ScoredStream {
    pub stream: StreamInfoWire,
    pub score: i64,
    pub reasons: Vec<String>,
}

fn quality_rank(quality: &str) -> i32 {
    let lower = quality.to_lowercase();
    if lower.starts_with("4k") || lower.starts_with("2160p") || lower.starts_with("uhd") {
        7
    } else if lower.starts_with("1440p") || lower.starts_with("2k") {
        6
    } else if lower.starts_with("1080p") || lower.starts_with("fhd") {
        5
    } else if lower.starts_with("720p") || lower == "hd" {
        4
    } else if lower.contains("576p") {
        3
    } else if lower.starts_with("480p") || lower.starts_with("sd") {
        2
    } else if lower.starts_with("360p") {
        1
    } else {
        0
    }
}

fn resolution_cap_rank(resolution: &str) -> Option<i32> {
    match resolution.to_lowercase().as_str() {
        "4k" | "2160p" | "uhd" => Some(7),
        "1440p" | "2k" => Some(6),
        "1080p" | "fhd" => Some(5),
        "720p" => Some(4),
        "576p" => Some(3),
        "480p" | "sd" => Some(2),
        "360p" => Some(1),
        _ => None,
    }
}

fn add_reason(reasons: &mut Vec<String>, pts: i64, msg: &str) {
    if pts >= 0 {
        reasons.push(format!("{}  +{}", msg, pts));
    } else {
        reasons.push(format!("{}  \u{2212}{}", msg, pts.abs()));
    }
}

/// Score a single stream according to the user policy.
pub fn score_stream_policy(stream: &StreamInfoWire, policy: &RankingPolicy) -> (i64, Vec<String>) {
    let mut total: i64 = 0;
    let mut reasons = Vec::new();

    let prefs = &policy.preferences;

    // Quality contribution: rank × 15 pts
    let qr = quality_rank(&stream.quality);
    if qr > 0 {
        let pts = qr as i64 * 15;
        add_reason(
            &mut reasons,
            pts,
            &format!("quality {} +{} pts", stream.quality, pts),
        );
        total += pts;
    }

    // Max resolution cap
    if let Some(ref max_res) = prefs.max_resolution {
        if let Some(cap) = resolution_cap_rank(max_res) {
            if qr > cap {
                let pts = -40;
                add_reason(
                    &mut reasons,
                    pts,
                    &format!("exceeds max {} \u{2212}40", max_res),
                );
                total += pts;
            }
        }
    }

    // Protocol preference
    if let Some(ref prefer_proto) = prefs.prefer_protocol {
        if !prefer_proto.is_empty()
            && stream
                .url
                .to_lowercase()
                .contains(&prefer_proto.to_lowercase())
        {
            let pts = 25;
            add_reason(
                &mut reasons,
                pts,
                &format!("preferred protocol {} +25", prefer_proto),
            );
            total += pts;
        }
    }

    // Seeders bonus — capped at +20
    let seeders = stream.seeders.unwrap_or(0) as i64;
    if seeders > 0 {
        let bonus = seeders.min(20);
        add_reason(
            &mut reasons,
            bonus,
            &format!("{} seeders +{}", seeders, bonus),
        );
        total += bonus;

        // Min seeders penalty
        if prefs.min_seeders > 0 && seeders < prefs.min_seeders as i64 {
            let pts = -30;
            add_reason(
                &mut reasons,
                pts,
                &format!("below min seeders ({}) \u{2212}30", prefs.min_seeders),
            );
            total += pts;
        }
    }

    // Size limit
    // Note: StreamInfoWire doesn't have size_bytes, so we skip this check
    // unless we add it to the IPC type

    // Avoided labels
    let haystack = format!(
        "{} {}",
        stream.name.to_lowercase(),
        stream.quality.to_lowercase(),
    );
    for avoid in &prefs.avoid_labels {
        if haystack.contains(&avoid.to_lowercase()) {
            let pts = -100;
            add_reason(
                &mut reasons,
                pts,
                &format!("avoided \"{}\" \u{2212}100", avoid),
            );
            total += pts;
            break;
        }
    }

    // HDR preference
    if prefs.prefer_hdr && stream.hdr {
        let pts = 15;
        add_reason(&mut reasons, pts, "HDR +15");
        total += pts;
    }

    // Codec preference
    if let Some(ref codec) = stream.codec {
        let codec_lower = codec.to_lowercase();
        for prefer in &prefs.prefer_codecs {
            if codec_lower.contains(&prefer.to_lowercase()) {
                let pts = 10;
                add_reason(&mut reasons, pts, &format!("codec {} +10", prefer));
                total += pts;
                break;
            }
        }
    }

    // Runtime provider score (normalised to avoid dominating)
    if stream.score > 0 {
        let pts = (stream.score / 10) as i64;
        if pts > 0 {
            add_reason(&mut reasons, pts, &format!("provider score +{}", pts));
            total += pts;
        }
    }

    (total, reasons)
}

/// Rank all streams according to the policy, returning them sorted best-first.
pub fn rank_streams(streams: Vec<StreamInfoWire>, policy: &RankingPolicy) -> Vec<ScoredStream> {
    let mut scored: Vec<ScoredStream> = streams
        .into_iter()
        .map(|stream| {
            let (score, reasons) = score_stream_policy(&stream, policy);
            ScoredStream {
                stream,
                score,
                reasons,
            }
        })
        .collect();

    scored.sort_by(|a, b| b.score.cmp(&a.score));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_stream(quality: &str, seeders: u32, hdr: bool) -> StreamInfoWire {
        StreamInfoWire {
            url: "magnet:test".to_string(),
            name: format!("{} test stream", quality),
            quality: quality.to_string(),
            provider: "test".to_string(),
            score: 100,
            codec: Some("H264".to_string()),
            source: Some("BluRay".to_string()),
            hdr,
            seeders: Some(seeders),
            speed_mbps: None,
            latency_ms: None,
        }
    }

    #[test]
    fn test_quality_ranking() {
        let policy = RankingPolicy::default();
        let streams = vec![
            make_stream("480p", 0, false),
            make_stream("720p", 0, false),
            make_stream("1080p", 0, false),
            make_stream("4K", 0, false),
        ];

        let ranked = rank_streams(streams, &policy);
        assert!(ranked[0].stream.quality.contains("4K"));
        assert!(ranked[3].stream.quality.contains("480p"));
    }

    #[test]
    fn test_avoid_labels() {
        let mut policy = RankingPolicy::default();
        policy.preferences.avoid_labels = vec!["cam".to_string()];

        let streams = vec![make_stream("1080p", 100, false)];
        // Stream with "cam" in name would get penalty
        let mut cam_stream = make_stream("1080p", 100, false);
        cam_stream.name = "CAM rip 1080p".to_string();
        let streams = vec![streams[0].clone(), cam_stream];

        let ranked = rank_streams(streams, &policy);
        assert!(!ranked[0].stream.name.contains("CAM"));
    }

    #[test]
    fn test_seeders_bonus() {
        let policy = RankingPolicy::default();
        let streams = vec![
            make_stream("1080p", 10, false),
            make_stream("1080p", 100, false),
        ];

        let ranked = rank_streams(streams, &policy);
        // Higher seeders should rank higher
        assert!(ranked[0].stream.seeders.unwrap_or(0) >= ranked[1].stream.seeders.unwrap_or(0));
    }
}
