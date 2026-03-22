//! Property-based tests for the quality scoring system.
//!
//! These tests verify invariants about quality scores using random input generation.

use crate::providers::{HdrFormat, Stream, StreamQuality};
use crate::quality::{QualityScore, RankingPolicy};
use proptest::prelude::*;

fn make_stream(
    name: String,
    quality: StreamQuality,
    seeders: Option<u32>,
    hdr: HdrFormat,
) -> Stream {
    Stream {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        url: "https://example.com/stream".to_string(),
        mime: Some("video/x-matroska".to_string()),
        quality,
        provider: "test".to_string(),
        seeders,
        bitrate_kbps: None,
        codec: None,
        resolution: None,
        hdr,
        size_bytes: None,
        latency_ms: None,
        speed_mbps: None,
        audio_channels: None,
        language: None,
        protocol: None,
    }
}

fn arb_stream() -> impl Strategy<Value = Stream> {
    ("\\PC+", 0u8..4, 0u32..1000, 0u8..4).prop_map(|(name, quality_idx, seeders, hdr_idx)| {
        let quality = match quality_idx {
            0 => StreamQuality::Sd,
            1 => StreamQuality::Hd720,
            2 => StreamQuality::Hd1080,
            _ => StreamQuality::Uhd4k,
        };
        let hdr = match hdr_idx {
            0 => HdrFormat::None,
            1 => HdrFormat::Hdr10,
            2 => HdrFormat::Hdr10Plus,
            _ => HdrFormat::DolbyVision,
        };
        make_stream(name.to_string(), quality, Some(seeders), hdr)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_total_score_never_exceeds_max(stream in arb_stream()) {
        let policy = RankingPolicy::default();
        let score = QualityScore::from_stream(&stream, &policy);
        prop_assert!(score.total() <= 1000);
    }

    #[test]
    fn prop_total_equals_sum_of_parts(stream in arb_stream()) {
        let policy = RankingPolicy::default();
        let score = QualityScore::from_stream(&stream, &policy);
        let expected = score.resolution + score.codec + score.seeders + score.bitrate + score.source + score.hdr_bonus;
        prop_assert_eq!(score.total(), expected);
    }

    #[test]
    fn prop_individual_scores_within_bounds(stream in arb_stream()) {
        let policy = RankingPolicy::default();
        let score = QualityScore::from_stream(&stream, &policy);
        prop_assert!(score.resolution <= 400);
        prop_assert!(score.codec <= 150);
        prop_assert!(score.seeders <= 150);
        prop_assert!(score.bitrate <= 150);
        prop_assert!(score.source <= 100);
        prop_assert!(score.hdr_bonus <= 50);
    }

    #[test]
    fn prop_resolution_scores_match_policy_weights(stream in arb_stream()) {
        let policy = RankingPolicy::default();
        let score = QualityScore::from_stream(&stream, &policy);
        match stream.quality {
            StreamQuality::Uhd4k => prop_assert_eq!(score.resolution, policy.resolution_weights[3]),
            StreamQuality::Hd1080 => prop_assert_eq!(score.resolution, policy.resolution_weights[2]),
            StreamQuality::Hd720 => prop_assert_eq!(score.resolution, policy.resolution_weights[1]),
            StreamQuality::Sd => prop_assert_eq!(score.resolution, policy.resolution_weights[0]),
            StreamQuality::Unknown => prop_assert_eq!(score.resolution, 0),
        }
    }

    #[test]
    fn prop_hdr_scores_match_format(stream in arb_stream()) {
        let policy = RankingPolicy::default();
        let score = QualityScore::from_stream(&stream, &policy);
        let expected = match stream.hdr {
            HdrFormat::DolbyVision => 50,
            HdrFormat::Hdr10Plus => 45,
            HdrFormat::Hdr10 => 40,
            HdrFormat::None => 0,
        };
        prop_assert_eq!(score.hdr_bonus, expected);
    }

    #[test]
    fn prop_higher_seeders_produce_higher_scores(seeders_high: u32, seeders_low: u32) {
        prop_assume!(seeders_high != seeders_low);
        // Prevent overflow when computing max(...) + 1
        prop_assume!(seeders_high.max(seeders_low) < u32::MAX);
        let policy = RankingPolicy::default();

        let high_seeder = make_stream("Test 1080p".to_string(), StreamQuality::Hd1080, Some(seeders_high.max(seeders_low) + 1), HdrFormat::None);
        let low_seeder = make_stream("Test 1080p".to_string(), StreamQuality::Hd1080, Some(seeders_low.min(seeders_high)), HdrFormat::None);

        let score_high = QualityScore::from_stream(&high_seeder, &policy);
        let score_low = QualityScore::from_stream(&low_seeder, &policy);
        prop_assert!(score_high.seeders >= score_low.seeders);
    }

    #[test]
    fn prop_higher_resolution_produces_higher_scores_default_policy(_seed: usize) {
        let sd = make_stream("Test SD".to_string(), StreamQuality::Sd, None, HdrFormat::None);
        let hd720 = make_stream("Test 720p".to_string(), StreamQuality::Hd720, None, HdrFormat::None);
        let hd1080 = make_stream("Test 1080p".to_string(), StreamQuality::Hd1080, None, HdrFormat::None);
        let uhd4k = make_stream("Test 4K".to_string(), StreamQuality::Uhd4k, None, HdrFormat::None);

        let policy = RankingPolicy::default();

        let score_sd = QualityScore::from_stream(&sd, &policy);
        let score_720 = QualityScore::from_stream(&hd720, &policy);
        let score_1080 = QualityScore::from_stream(&hd1080, &policy);
        let score_4k = QualityScore::from_stream(&uhd4k, &policy);

        if !policy.prefer_lower_resolution {
            prop_assert!(score_720.resolution >= score_sd.resolution);
            prop_assert!(score_1080.resolution >= score_720.resolution);
            prop_assert!(score_4k.resolution >= score_1080.resolution);
        }
    }

    #[test]
    fn prop_rank_preserves_stream_count(streams in prop::collection::vec(arb_stream(), 0..50)) {
        use crate::quality::rank;
        let policy = RankingPolicy::default();
        let input_count = streams.len();
        let ranked = rank(streams, &policy);
        prop_assert_eq!(ranked.len(), input_count);
    }

    #[test]
    fn prop_rank_produces_sorted_results(streams in prop::collection::vec(arb_stream(), 2..20)) {
        use crate::quality::rank;
        let policy = RankingPolicy::default();
        let ranked = rank(streams, &policy);
        for window in ranked.windows(2) {
            prop_assert!(window[0].score.total() >= window[1].score.total());
        }
    }
}

#[test]
fn test_seeders_log_scale_capped_at_100() {
    let policy = RankingPolicy::default();

    let at_100 = make_stream(
        "Test".to_string(),
        StreamQuality::Hd1080,
        Some(100),
        HdrFormat::None,
    );
    let at_1000 = make_stream(
        "Test".to_string(),
        StreamQuality::Hd1080,
        Some(1000),
        HdrFormat::None,
    );

    let score_100 = QualityScore::from_stream(&at_100, &policy);
    let score_1000 = QualityScore::from_stream(&at_1000, &policy);

    assert_eq!(
        score_100.seeders, score_1000.seeders,
        "100 and 1000 seeders should score the same"
    );
    assert!(
        score_100.seeders <= 150,
        "seeder score should be capped at 150"
    );
}

#[test]
fn test_bitrate_capped_at_40mbps() {
    let policy = RankingPolicy::default();

    let low_bitrate = Stream {
        name: "Test 5mbps".to_string(),
        bitrate_kbps: Some(5000),
        ..Default::default()
    };
    let high_bitrate = Stream {
        name: "Test 50mbps".to_string(),
        bitrate_kbps: Some(50000),
        ..Default::default()
    };

    let score_low = QualityScore::from_stream(&low_bitrate, &policy);
    let score_high = QualityScore::from_stream(&high_bitrate, &policy);

    assert!(
        score_high.bitrate <= 150,
        "bitrate score should be capped at 150"
    );
    assert!(score_high.bitrate >= score_low.bitrate);
}

#[test]
fn test_bandwidth_saver_prefers_720p() {
    let policy = RankingPolicy::bandwidth_saver();

    let high_res = make_stream(
        "Movie 4K".to_string(),
        StreamQuality::Uhd4k,
        None,
        HdrFormat::None,
    );
    let low_res = make_stream(
        "Movie 720p".to_string(),
        StreamQuality::Hd720,
        None,
        HdrFormat::None,
    );

    let score_high = QualityScore::from_stream(&high_res, &policy);
    let score_low = QualityScore::from_stream(&low_res, &policy);

    assert!(
        score_low.resolution > score_high.resolution,
        "bandwidth saver should prefer 720p over 4K"
    );
}

#[test]
fn test_fastest_start_weights_seeders() {
    let policy = RankingPolicy::fastest_start();

    let low_seeder = make_stream(
        "Test".to_string(),
        StreamQuality::Hd1080,
        Some(10),
        HdrFormat::None,
    );
    let high_seeder = make_stream(
        "Test".to_string(),
        StreamQuality::Hd1080,
        Some(100),
        HdrFormat::None,
    );

    let score_low = QualityScore::from_stream(&low_seeder, &policy);
    let score_high = QualityScore::from_stream(&high_seeder, &policy);

    assert!(score_high.seeders > score_low.seeders);
    assert!(policy.seeder_weight > 1.0);
}

#[test]
fn test_source_scores_respect_hierarchy() {
    let policy = RankingPolicy::default();

    let bluray = make_stream(
        "Movie Bluray 1080p".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::None,
    );
    let webdl = make_stream(
        "Movie WEB-DL 1080p".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::None,
    );
    let hdtv = make_stream(
        "Movie HDTV 1080p".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::None,
    );
    let cam = make_stream(
        "Movie CAM 1080p".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::None,
    );

    let score_bluray = QualityScore::from_stream(&bluray, &policy).source;
    let score_webdl = QualityScore::from_stream(&webdl, &policy).source;
    let score_hdtv = QualityScore::from_stream(&hdtv, &policy).source;
    let score_cam = QualityScore::from_stream(&cam, &policy).source;

    assert!(score_bluray > score_webdl);
    assert!(score_webdl > score_hdtv);
    assert!(score_hdtv > score_cam);
}

#[test]
fn test_codec_scores_respect_hierarchy() {
    let policy = RankingPolicy::default();

    let av1 = Stream {
        name: "Video AV1".to_string(),
        quality: StreamQuality::Hd1080,
        codec: Some("AV1".to_string()),
        ..Default::default()
    };
    let hevc = Stream {
        name: "Video HEVC".to_string(),
        quality: StreamQuality::Hd1080,
        codec: Some("HEVC".to_string()),
        ..Default::default()
    };
    let h264 = Stream {
        name: "Video H264".to_string(),
        quality: StreamQuality::Hd1080,
        codec: Some("H264".to_string()),
        ..Default::default()
    };

    let score_av1 = QualityScore::from_stream(&av1, &policy).codec;
    let score_hevc = QualityScore::from_stream(&hevc, &policy).codec;
    let score_h264 = QualityScore::from_stream(&h264, &policy).codec;

    assert!(score_av1 > score_hevc);
    assert!(score_hevc > score_h264);
}

#[test]
fn test_hdr_dolby_vision_highest_score() {
    let policy = RankingPolicy::default();

    let dv = make_stream(
        "Video DolbyVision".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::DolbyVision,
    );
    let hdr10plus = make_stream(
        "Video HDR10+".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::Hdr10Plus,
    );
    let hdr10 = make_stream(
        "Video HDR10".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::Hdr10,
    );
    let none = make_stream(
        "Video".to_string(),
        StreamQuality::Hd1080,
        None,
        HdrFormat::None,
    );

    let score_dv = QualityScore::from_stream(&dv, &policy).hdr_bonus;
    let score_hdr10plus = QualityScore::from_stream(&hdr10plus, &policy).hdr_bonus;
    let score_hdr10 = QualityScore::from_stream(&hdr10, &policy).hdr_bonus;
    let score_none = QualityScore::from_stream(&none, &policy).hdr_bonus;

    assert!(score_dv > score_hdr10plus);
    assert!(score_hdr10plus > score_hdr10);
    assert!(score_hdr10 > score_none);
}
