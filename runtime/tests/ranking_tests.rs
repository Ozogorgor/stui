//! Integration tests for the stream quality ranking module.
//!
//! These tests verify that the scoring and ranking logic produces the
//! expected ordering for realistic stream names.

use stui_runtime::quality::{rank, RankingPolicy};
use stui_runtime::providers::{Stream, StreamQuality};

fn stream(name: &str, quality: StreamQuality) -> Stream {
    Stream {
        id:       name.to_string(),
        name:     name.to_string(),
        url:      format!("magnet:?xt=urn:btih:{}", name.len()),
        mime:     None,
        quality,
        provider: "test".to_string(),
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
fn test_cam_ranks_last() {
    let streams = vec![
        stream("1080p CAM", StreamQuality::Hd1080),
        stream("720p WEB-DL", StreamQuality::Hd720),
        stream("480p BluRay", StreamQuality::Sd),
    ];
    let ranked = rank(streams, &RankingPolicy::default());
    assert!(
        ranked.last().unwrap().stream.name.contains("CAM"),
        "CAM source should always rank last"
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
