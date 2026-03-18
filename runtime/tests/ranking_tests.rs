//! Integration tests for the stream quality ranking module.
//!
//! These tests verify that the scoring and ranking logic produces the
//! expected ordering for realistic stream names.

use stui_runtime::quality::{rank, RankingPolicy};
use stui_runtime::providers::{Stream, StreamQuality};
use stui_runtime::config::types::StreamPreferences;

fn stream(name: &str, quality: StreamQuality) -> Stream {
    Stream {
        id:       name.to_string(),
        name:     name.to_string(),
        url:      format!("magnet:?xt=urn:btih:{}", name.len()),
        mime:     None,
        quality,
        provider: "test".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_4k_beats_1080p() {
    let streams = vec![
        stream("1080p BluRay HEVC", StreamQuality::Hd1080),
        stream("2160p 4K BluRay", StreamQuality::Uhd4k),
    ];
    let ranked = rank(streams, &RankingPolicy::default());
    assert_eq!(ranked[0].stream.quality, StreamQuality::Uhd4k, "4K should rank first");
}

#[test]
fn test_1080p_beats_720p() {
    let streams = vec![
        stream("720p HDTV x264", StreamQuality::Hd720),
        stream("1080p WEB-DL x265", StreamQuality::Hd1080),
    ];
    let ranked = rank(streams, &RankingPolicy::default());
    assert_eq!(ranked[0].stream.quality, StreamQuality::Hd1080);
}

#[test]
fn test_bluray_beats_hdtv_same_resolution() {
    let streams = vec![
        stream("1080p HDTV x264", StreamQuality::Hd1080),
        stream("1080p BluRay x264", StreamQuality::Hd1080),
    ];
    let ranked = rank(streams, &RankingPolicy::default());
    assert!(
        ranked[0].stream.name.contains("BluRay"),
        "BluRay should rank above HDTV at same resolution"
    );
}

#[test]
fn test_hevc_beats_h264_same_resolution() {
    let streams = vec![
        stream("1080p WEB-DL x264", StreamQuality::Hd1080),
        stream("1080p WEB-DL HEVC", StreamQuality::Hd1080),
    ];
    let ranked = rank(streams, &RankingPolicy::default());
    assert!(
        ranked[0].stream.name.contains("HEVC"),
        "HEVC should rank above H264 at same resolution"
    );
}

#[test]
fn test_cam_penalised_in_source_score() {
    // CAM streams receive the lowest source score (5 pts vs 80 for WEB-DL).
    // However a CAM stream at a higher resolution may still outrank a
    // lower-resolution WEB-DL stream because resolution weight dominates.
    // This test verifies that a same-resolution CAM stream ranks below WEB-DL.
    let streams = vec![
        stream("1080p CAM", StreamQuality::Hd1080),
        stream("1080p WEB-DL", StreamQuality::Hd1080),
    ];
    let ranked = rank(streams, &RankingPolicy::default());
    assert!(
        ranked[0].stream.name.contains("WEB-DL"),
        "WEB-DL should rank above CAM at the same resolution"
    );
    assert!(
        ranked.last().unwrap().stream.name.contains("CAM"),
        "CAM should rank below WEB-DL at same resolution"
    );
}

#[test]
fn test_hdr_bonus_applied() {
    let streams = vec![
        stream("1080p BluRay x265", StreamQuality::Hd1080),
        stream("1080p BluRay x265 HDR10", StreamQuality::Hd1080),
    ];
    let ranked = rank(streams, &RankingPolicy::default());
    assert!(
        ranked[0].stream.name.contains("HDR"),
        "HDR should give a score bonus"
    );
}

#[test]
fn test_bandwidth_saver_prefers_720p() {
    let streams = vec![
        stream("2160p 4K BluRay", StreamQuality::Uhd4k),
        stream("720p WEB-DL", StreamQuality::Hd720),
    ];
    let ranked = rank(streams, &RankingPolicy::bandwidth_saver());
    assert_eq!(
        ranked[0].stream.quality,
        StreamQuality::Hd720,
        "bandwidth_saver should prefer 720p over 4K"
    );
}

#[test]
fn test_empty_input_returns_empty() {
    let ranked = rank(vec![], &RankingPolicy::default());
    assert!(ranked.is_empty());
}

#[test]
fn test_single_stream_returned_unchanged() {
    let streams = vec![stream("720p WEB-DL", StreamQuality::Hd720)];
    let ranked = rank(streams, &RankingPolicy::default());
    assert_eq!(ranked.len(), 1);
}

#[test]
fn test_badge_contains_resolution() {
    let s = stream("1080p BluRay HEVC", StreamQuality::Hd1080);
    let ranked = rank(vec![s], &RankingPolicy::default());
    let badge = ranked[0].badge();
    assert!(badge.contains("1080p"), "badge should include resolution label");
    assert!(badge.contains('★'), "badge should include score star");
}

#[test]
fn test_stream_prefs_default_equals_ranking_policy_default() {
    let prefs = StreamPreferences::default();
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    let default_policy = stui_runtime::quality::RankingPolicy::default();
    assert_eq!(policy.resolution_weights, default_policy.resolution_weights);
    assert_eq!(policy.seeder_weight, default_policy.seeder_weight);
    assert_eq!(policy.exclude_cam, default_policy.exclude_cam);
    assert_eq!(policy.min_seeders, default_policy.min_seeders);
}

#[test]
fn test_max_resolution_1080p_zeroes_4k_weight() {
    let prefs = StreamPreferences {
        max_resolution: Some("1080p".to_string()),
        ..Default::default()
    };
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    assert_eq!(policy.resolution_weights[3], 0, "4K weight should be 0 when max is 1080p");
    assert_eq!(policy.resolution_weights[2], 300, "1080p weight should be 300");
}

#[test]
fn test_max_resolution_720p_zeroes_1080p_and_4k() {
    let prefs = StreamPreferences {
        max_resolution: Some("720p".to_string()),
        ..Default::default()
    };
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    assert_eq!(policy.resolution_weights[3], 0);
    assert_eq!(policy.resolution_weights[2], 0);
    assert_eq!(policy.resolution_weights[1], 200);
}

#[test]
fn test_seeder_weight_and_min_seeders_forwarded() {
    let prefs = StreamPreferences {
        seeder_weight: 2.5,
        min_seeders: 10,
        ..Default::default()
    };
    let policy = stui_runtime::quality::RankingPolicy::from(&prefs);
    assert_eq!(policy.seeder_weight, 2.5);
    assert_eq!(policy.min_seeders, 10);
}
