//! Integration tests for `HealthRegistry` and `ProviderThrottle`.

use stui_runtime::providers::{HealthRegistry, ProviderThrottle};
use stui_runtime::providers::health::FailureKind;

// ── HealthRegistry ────────────────────────────────────────────────────────────

#[test]
fn new_provider_gets_full_reliability() {
    let h = HealthRegistry::new();
    assert_eq!(h.reliability_score("unseen"), 1.0,
        "unseen provider gets 1.0 benefit of the doubt");
}

#[test]
fn perfect_record_keeps_full_score() {
    let h = HealthRegistry::new();
    h.record_success("tmdb", 120);
    h.record_success("tmdb", 95);
    h.record_success("tmdb", 150);
    let score = h.reliability_score("tmdb");
    assert!(score > 0.9, "three successes should give near-perfect score, got {score}");
}

#[test]
fn failures_lower_score() {
    let h = HealthRegistry::new();
    h.record_success("bad", 200);
    h.record_failure("bad", FailureKind::Error);
    h.record_failure("bad", FailureKind::Error);
    let score = h.reliability_score("bad");
    assert!(score < 0.6, "2 failures out of 3 requests should drop score below 0.6, got {score}");
}

#[test]
fn timeout_counts_as_failure() {
    let h = HealthRegistry::new();
    h.record_success("slow", 5000);
    h.record_failure("slow", FailureKind::Timeout);
    let stats = h.stats("slow");
    assert_eq!(stats.timeout_count, 1);
    assert_eq!(stats.failure_count, 1);
}

#[test]
fn empty_response_tracked_separately() {
    let h = HealthRegistry::new();
    h.record_success("empty-prov", 100);
    h.record_failure("empty-prov", FailureKind::Empty);
    let stats = h.stats("empty-prov");
    assert_eq!(stats.empty_count, 1);
    // Empty counts as a failure in request_count but NOT in failure_count
    assert_eq!(stats.failure_count, 0, "empty should not be a hard failure");
    assert_eq!(stats.request_count, 2);
}

#[test]
fn avg_latency_computed_correctly() {
    let h = HealthRegistry::new();
    h.record_success("p", 100);
    h.record_success("p", 200);
    h.record_success("p", 300);
    let stats = h.stats("p");
    let avg = stats.avg_latency_ms();
    assert!((avg - 200.0).abs() < 1.0, "average of 100+200+300 should be 200ms, got {avg}");
}

#[test]
fn all_stats_sorted_by_reliability() {
    let h = HealthRegistry::new();
    h.record_success("good", 100);
    h.record_success("good", 120);
    h.record_failure("bad", FailureKind::Error);
    h.record_failure("bad", FailureKind::Error);
    h.record_failure("bad", FailureKind::Error);

    let all = h.all_stats();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].name, "good", "good provider should rank first");
}

#[test]
fn degraded_providers_filter() {
    let h = HealthRegistry::new();
    h.record_success("fine", 100);
    h.record_success("fine", 100);
    h.record_success("fine", 100);

    h.record_failure("broken", FailureKind::Error);
    h.record_failure("broken", FailureKind::Error);
    h.record_failure("broken", FailureKind::Error);

    let degraded = h.degraded_providers(0.5);
    assert!(degraded.contains(&"broken".to_string()));
    assert!(!degraded.contains(&"fine".to_string()));
}

// ── ProviderThrottle ──────────────────────────────────────────────────────────

#[tokio::test]
async fn new_provider_not_cooling_down() {
    let t = ProviderThrottle::new();
    assert!(!t.is_cooling_down("any-provider").await);
}

#[tokio::test]
async fn cooldown_set_after_rate_limit() {
    let t = ProviderThrottle::new();
    t.record_rate_limited("tmdb", Some(60)).await;
    assert!(t.is_cooling_down("tmdb").await);

    let remaining = t.cooldown_remaining("tmdb").await;
    assert!(remaining.is_some());
    assert!(remaining.unwrap().as_secs() <= 60,
        "remaining cooldown should be ≤ the hint value");
}

#[tokio::test]
async fn zero_second_cooldown_not_blocking() {
    let t = ProviderThrottle::new();
    t.record_rate_limited("tmdb", Some(0)).await;
    // Wait a tiny bit for time to pass
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(!t.is_cooling_down("tmdb").await,
        "0-second cooldown should expire immediately");
}

#[tokio::test]
async fn cooling_down_providers_list() {
    let t = ProviderThrottle::new();
    t.record_rate_limited("tmdb",      Some(60)).await;
    t.record_rate_limited("torrentio", Some(30)).await;

    let cooling = t.cooling_down_providers().await;
    assert_eq!(cooling.len(), 2);
    let names: Vec<&str> = cooling.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"tmdb"));
    assert!(names.contains(&"torrentio"));
}

#[tokio::test]
async fn success_does_not_remove_cooldown() {
    // record_success resets the backoff counter but doesn't clear an
    // active cooldown that hasn't expired yet
    let t = ProviderThrottle::new();
    t.record_rate_limited("tmdb", Some(60)).await;
    t.record_success("tmdb").await;
    // cooldown should still be active (time hasn't passed)
    assert!(t.is_cooling_down("tmdb").await,
        "active cooldown should persist even after success call");
}

#[tokio::test]
async fn set_limit_respects_capacity() {
    let t = ProviderThrottle::new();
    t.set_limit("fast-provider", 100).await; // 100 req/s
    // At 100 tokens/s the bucket should have 100 tokens immediately,
    // so acquire() should return without any wait
    let start = std::time::Instant::now();
    t.acquire("fast-provider").await;
    assert!(start.elapsed().as_millis() < 50,
        "acquire with full bucket should be instant");
}
