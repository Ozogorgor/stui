//! Unit tests for `RankingPolicy` construction and field defaults.

use stui_runtime::quality::RankingPolicy;

#[test]
fn ranking_policy_default() {
    let policy = RankingPolicy::default();
    assert!(!policy.prefer_lower_resolution);
    assert!(policy.exclude_cam);
}

#[test]
fn ranking_policy_bandwidth_saver() {
    let policy = RankingPolicy::bandwidth_saver();
    assert!(policy.prefer_lower_resolution);
    assert!(policy.exclude_cam);
    assert!(policy.preferences.min_seeders > 0);
}

#[test]
fn ranking_policy_fastest_start() {
    let policy = RankingPolicy::fastest_start();
    assert!(policy.seeder_weight > 1.0);
    assert!(policy.preferences.min_seeders > 0);
}
