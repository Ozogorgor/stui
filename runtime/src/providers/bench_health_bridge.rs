//! Bridge between stream benchmark results and provider health scoring.
//!
//! `BenchHealthBridge` translates consecutive below-threshold benchmark
//! results into `HealthRegistry::record_failure` calls, so providers that
//! habitually serve slow streams are ranked lower over time.

use std::collections::HashMap;
use std::sync::Mutex;

use super::{FailureKind, HealthRegistry, Stream};

/// Translates stream benchmark results into provider health score updates.
///
/// A result is **bad** if `speed_mbps` is `Some(x)` and `x < slow_mbps_threshold`.
/// A result is **neutral** if `speed_mbps` is `None` (non-HTTP or probe skipped).
/// A result is **good** if `speed_mbps` is `Some(x)` and `x >= slow_mbps_threshold`.
///
/// After `streak_threshold` consecutive bad results for a provider,
/// `HealthRegistry::record_failure` is called and the streak resets to 0.
/// A good result immediately resets the streak and calls `record_success`.
pub struct BenchHealthBridge {
    health: HealthRegistry,
    streaks: Mutex<HashMap<String, u32>>,
    streak_threshold: u32,
    slow_mbps_threshold: f64,
}

impl BenchHealthBridge {
    pub fn new(health: HealthRegistry) -> Self {
        Self::with_config(health, 3, 1.0)
    }

    pub fn with_config(health: HealthRegistry, streak_threshold: u32, slow_mbps_threshold: f64) -> Self {
        Self {
            health,
            streaks: Mutex::new(HashMap::new()),
            streak_threshold,
            slow_mbps_threshold,
        }
    }

    /// Record one benchmark result for the stream's provider.
    pub fn record(&self, stream: &Stream) {
        let speed = match stream.speed_mbps {
            None => return, // neutral: non-HTTP or probe skipped
            Some(s) => s,
        };

        let provider = &stream.provider;

        if speed >= self.slow_mbps_threshold {
            // Good result: reset streak, record success
            {
                let mut streaks = self.streaks.lock().unwrap_or_else(|p| p.into_inner());
                streaks.insert(provider.to_string(), 0);
            }
            let latency = stream.latency_ms.unwrap_or(0) as u64;
            self.health.record_success(provider, latency);
        } else {
            // Bad result: increment streak, fire if threshold reached
            let mut streaks = self.streaks.lock().unwrap_or_else(|p| p.into_inner());
            let streak = streaks.entry(provider.to_string()).or_insert(0);
            *streak += 1;
            if *streak >= self.streak_threshold {
                *streak = 0;
                drop(streaks); // release lock before calling health (which has its own lock)
                self.health.record_failure(provider, FailureKind::Error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderStats;

    fn make_stream(provider: &str, speed_mbps: Option<f64>, latency_ms: Option<u32>) -> Stream {
        Stream {
            id: "test".to_string(),
            name: "test".to_string(),
            url: "http://example.com/stream".to_string(),
            provider: provider.to_string(),
            speed_mbps,
            latency_ms,
            ..Default::default()
        }
    }

    fn stats(bridge: &BenchHealthBridge, provider: &str) -> ProviderStats {
        bridge.health.stats(provider)
    }

    #[test]
    fn neutral_result_is_ignored() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        let stream = make_stream("prov", None, None);
        bridge.record(&stream);
        bridge.record(&stream);
        bridge.record(&stream);
        let s = stats(&bridge, "prov");
        assert_eq!(s.request_count, 0, "neutral results must not touch health registry");
    }

    #[test]
    fn below_threshold_streak_does_not_fire_early() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        let s = stats(&bridge, "prov");
        assert_eq!(s.failure_count, 0, "failure must not fire before streak_threshold");
        assert_eq!(s.request_count, 0);
    }

    #[test]
    fn nth_bad_result_fires_failure_and_resets_streak() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        let s = stats(&bridge, "prov");
        assert_eq!(s.failure_count, 1, "exactly one failure on Nth bad result");
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        assert_eq!(stats(&bridge, "prov").failure_count, 1, "streak must reset to 0 after firing");
    }

    #[test]
    fn good_result_resets_streak_and_records_success() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(10.0), Some(50)));
        let s = stats(&bridge, "prov");
        assert_eq!(s.failure_count, 0, "good result must prevent failure from firing");
        assert_eq!(s.success_count, 1, "good result must record a success");
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        assert_eq!(stats(&bridge, "prov").failure_count, 0);
    }

    #[test]
    fn good_result_passes_latency_to_registry() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        bridge.record(&make_stream("prov", Some(5.0), Some(45)));
        let s = stats(&bridge, "prov");
        assert_eq!(s.success_count, 1);
        assert_eq!(s.latency_sum_ms(), 45, "latency_ms must be forwarded to health registry");
    }

    #[test]
    fn good_result_with_no_latency_passes_zero() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        bridge.record(&make_stream("prov", Some(5.0), None));
        let s = stats(&bridge, "prov");
        assert_eq!(s.success_count, 1);
        assert_eq!(s.latency_sum_ms(), 0, "None latency must become 0, not panic");
    }

    #[test]
    fn reliability_score_drops_after_repeated_bad_bench() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 2, 1.0);
        let score_before = bridge.health.reliability_score("prov");
        for _ in 0..4 {
            bridge.record(&make_stream("prov", Some(0.1), None));
        }
        let score_after = bridge.health.reliability_score("prov");
        assert!(
            score_after < score_before,
            "reliability_score must fall after repeated bench failures: before={score_before}, after={score_after}"
        );
    }
}
