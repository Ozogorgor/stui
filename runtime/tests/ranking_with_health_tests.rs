//! Integration tests for health-blended stream ranking.
//!
//! Verifies that `rank_with_health()` correctly combines quality scores
//! with provider reliability so that unreliable providers are penalised
//! even when they offer higher-quality streams.

use std::collections::HashMap;
use stui_runtime::quality::{rank_with_health, RankingPolicy};
use stui_runtime::providers::{Stream, StreamQuality};

fn stream(name: &str, quality: StreamQuality, provider: &str) -> Stream {
    Stream {
        id:       name.to_string(),
        name:     name.to_string(),
        url:      format!("https://example.com/{}", name),
        mime:     None,
        quality,
        provider: provider.to_string(),
        ..Default::default()
    }
}

// ── Quality-only (no health map) ──────────────────────────────────────────────

#[test]
fn no_health_map_falls_back_to_quality_order() {
    let streams = vec![
        stream("720p",  StreamQuality::Hd720,  "prov-a"),
        stream("1080p", StreamQuality::Hd1080, "prov-b"),
        stream("4K",    StreamQuality::Uhd4k,  "prov-c"),
    ];
    let ranked = rank_with_health(streams, &RankingPolicy::default(), None);
    assert_eq!(ranked[0].stream.quality, StreamQuality::Uhd4k,  "4K should lead");
    assert_eq!(ranked[1].stream.quality, StreamQuality::Hd1080, "1080p second");
    assert_eq!(ranked[2].stream.quality, StreamQuality::Hd720,  "720p last");
}

// ── Reliability penalises high-quality streams from bad providers ─────────────

#[test]
fn unreliable_4k_provider_loses_to_reliable_1080p() {
    let streams = vec![
        stream("4K-flaky",    StreamQuality::Uhd4k,  "flaky"),
        stream("1080p-solid", StreamQuality::Hd1080, "solid"),
    ];

    let mut health = HashMap::new();
    health.insert("flaky".to_string(), 0.1);  // 10% reliability
    health.insert("solid".to_string(), 1.0);  // 100% reliability

    let ranked = rank_with_health(streams, &RankingPolicy::default(), Some(&health));
    // A 25% reliability weight should be enough to flip 4K→1080p here
    assert_eq!(
        ranked[0].stream.provider, "solid",
        "reliable 1080p should beat unreliable 4K"
    );
}

#[test]
fn reliable_4k_still_beats_reliable_1080p() {
    let streams = vec![
        stream("4K-reliable",    StreamQuality::Uhd4k,  "prov-4k"),
        stream("1080p-reliable", StreamQuality::Hd1080, "prov-hd"),
    ];

    let mut health = HashMap::new();
    health.insert("prov-4k".to_string(), 0.95);
    health.insert("prov-hd".to_string(), 0.90);

    let ranked = rank_with_health(streams, &RankingPolicy::default(), Some(&health));
    assert_eq!(
        ranked[0].stream.quality, StreamQuality::Uhd4k,
        "4K from a reliable provider should still lead"
    );
}

#[test]
fn unknown_provider_gets_benefit_of_doubt() {
    // Providers not in the health map get reliability = 1.0 (optimistic default)
    let streams = vec![
        stream("1080p-known",   StreamQuality::Hd1080, "known"),
        stream("1080p-unknown", StreamQuality::Hd1080, "unknown"),
    ];

    let mut health = HashMap::new();
    health.insert("known".to_string(), 0.5);
    // "unknown" not in map → should get 1.0 → should rank above "known"

    let ranked = rank_with_health(streams, &RankingPolicy::default(), Some(&health));
    assert_eq!(
        ranked[0].stream.provider, "unknown",
        "unseen provider should get 1.0 reliability and outrank a 0.5 provider"
    );
}

#[test]
fn empty_streams_returns_empty() {
    let ranked = rank_with_health(vec![], &RankingPolicy::default(), None);
    assert!(ranked.is_empty());
}

#[test]
fn single_stream_always_first() {
    let streams = vec![stream("720p", StreamQuality::Hd720, "p")];
    let ranked = rank_with_health(streams, &RankingPolicy::default(), None);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].stream.quality, StreamQuality::Hd720);
}

// ── blend_score unit tests ────────────────────────────────────────────────────

#[test]
fn blend_score_formula() {
    use stui_runtime::providers::health::blend_score;
    // 75% quality + 25% reliability
    let score = blend_score(1.0, 0.0);
    assert!((score - 0.75).abs() < 1e-6, "quality=1 reliability=0 → 0.75");

    let score2 = blend_score(0.0, 1.0);
    assert!((score2 - 0.25).abs() < 1e-6, "quality=0 reliability=1 → 0.25");

    let score3 = blend_score(1.0, 1.0);
    assert!((score3 - 1.0).abs() < 1e-6, "perfect score");
}
