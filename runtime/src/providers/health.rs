//! Provider health tracking — reliability metrics used by the ranking engine.
//!
//! Each provider accumulates statistics over its lifetime.  The ranking engine
//! uses these to penalise unreliable providers in stream selection, so users
//! automatically get streams from providers that actually work.
//!
//! # Metrics
//!
//! | Metric           | Description                                      |
//! |------------------|--------------------------------------------------|
//! | `success_rate`   | Fraction of requests that returned ≥1 result     |
//! | `avg_latency_ms` | Rolling average response time                    |
//! | `failure_count`  | Cumulative hard failures (errors, not empty)     |
//! | `timeout_count`  | Cumulative timeout/network failures              |
//! | `empty_count`    | Successful responses that returned no items      |
//!
//! # Usage
//!
//! See module tests for usage examples.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ── Failure kind ──────────────────────────────────────────────────────────────

/// Classification of a provider failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureKind {
    /// Hard error: HTTP 4xx/5xx, parse failure, etc.
    Error,
    /// Request timed out.
    Timeout,
    /// Request succeeded but returned no results.
    Empty,
}

// ── Per-provider stats ────────────────────────────────────────────────────────

/// Accumulated statistics for one provider.
#[derive(Debug, Clone)]
pub struct ProviderStats {
    pub name: String,
    pub request_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub timeout_count: u64,
    pub empty_count: u64,
    /// Sum of all response latencies in milliseconds.
    latency_sum_ms: u64,
    /// Timestamp of first request (for rate-based metrics).
    first_seen: Option<Instant>,
    /// Timestamp of last successful request.
    pub last_success: Option<Instant>,
}

impl ProviderStats {
    fn new(name: &str) -> Self {
        ProviderStats {
            name: name.to_string(),
            request_count: 0,
            success_count: 0,
            failure_count: 0,
            timeout_count: 0,
            empty_count: 0,
            latency_sum_ms: 0,
            first_seen: None,
            last_success: None,
        }
    }

    /// Fraction of non-empty successful responses (0.0–1.0).
    pub fn success_rate(&self) -> f64 {
        if self.request_count == 0 {
            return 1.0;
        } // benefit of the doubt
        let good = self.success_count.saturating_sub(self.empty_count);
        good as f64 / self.request_count as f64
    }

    /// Raw sum of all recorded latencies in milliseconds.
    pub fn latency_sum_ms(&self) -> u64 {
        self.latency_sum_ms
    }

    /// Average response latency in milliseconds.
    pub fn avg_latency_ms(&self) -> f64 {
        if self.success_count == 0 {
            return 0.0;
        }
        self.latency_sum_ms as f64 / self.success_count as f64
    }

    /// Composite reliability score used by the ranking engine.
    ///
    /// Range: 0.0 (completely broken) → 1.0 (perfect).
    ///
    /// Formula:
    ///   reliability = success_rate × latency_factor
    ///
    /// where `latency_factor` = 1.0 for ≤200ms, decays toward 0.5 at 2000ms.
    pub fn reliability_score(&self) -> f64 {
        let sr = self.success_rate();
        let lat = self.avg_latency_ms();
        // Latency penalty: 1.0 at 0ms, 0.5 at 2000ms, capped at 0.5
        let latency_factor = if lat <= 0.0 {
            1.0
        } else {
            (1.0 - (lat / 4000.0)).clamp(0.5, 1.0)
        };
        sr * latency_factor
    }
}

// ── Health registry ───────────────────────────────────────────────────────────

/// Shared registry of per-provider health statistics.
///
/// Cheap to clone — wraps an `Arc<Mutex<…>>`.
#[derive(Clone)]
pub struct HealthRegistry {
    inner: Arc<Mutex<HashMap<String, ProviderStats>>>,
}

impl HealthRegistry {
    pub fn new() -> Self {
        HealthRegistry {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ── Recording ─────────────────────────────────────────────────────────

    /// Record a successful provider response.
    ///
    /// `latency_ms` is the wall-clock time from request start to first result.
    pub fn record_success(&self, provider: &str, latency_ms: u64) {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let s = map
            .entry(provider.to_string())
            .or_insert_with(|| ProviderStats::new(provider));
        s.request_count += 1;
        s.success_count += 1;
        s.latency_sum_ms += latency_ms;
        s.first_seen.get_or_insert(Instant::now());
        s.last_success = Some(Instant::now());
    }

    /// Record a provider failure.
    pub fn record_failure(&self, provider: &str, kind: FailureKind) {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let s = map
            .entry(provider.to_string())
            .or_insert_with(|| ProviderStats::new(provider));
        s.request_count += 1;
        s.failure_count += 1;
        s.first_seen.get_or_insert(Instant::now());
        match kind {
            FailureKind::Timeout => s.timeout_count += 1,
            FailureKind::Empty => {
                s.empty_count += 1;
                s.failure_count -= 1;
            }
            FailureKind::Error => {}
        }
    }

    // ── Query ─────────────────────────────────────────────────────────────

    /// Get a clone of stats for one provider (returns defaults if unknown).
    pub fn stats(&self, provider: &str) -> ProviderStats {
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.get(provider)
            .cloned()
            .unwrap_or_else(|| ProviderStats::new(provider))
    }

    /// Reliability score for `provider` (0.0–1.0, higher = more reliable).
    pub fn reliability_score(&self, provider: &str) -> f64 {
        self.stats(provider).reliability_score()
    }

    /// All provider stats, sorted by reliability descending.
    pub fn all_stats(&self) -> Vec<ProviderStats> {
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<ProviderStats> = map.values().cloned().collect();
        v.sort_by(|a, b| {
            b.reliability_score()
                .partial_cmp(&a.reliability_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    /// Providers with a reliability score below `threshold` (for warnings).
    pub fn degraded_providers(&self, threshold: f64) -> Vec<String> {
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.values()
            .filter(|s| s.request_count >= 3 && s.reliability_score() < threshold)
            .map(|s| s.name.clone())
            .collect()
    }

    /// Get reliability scores for all providers as a HashMap.
    /// Used by `rank_with_health` to blend quality with reliability.
    pub fn all_reliability_scores(&self) -> HashMap<String, f64> {
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.iter()
            .map(|(name, stats)| (name.clone(), stats.reliability_score()))
            .collect()
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Score blend helper ────────────────────────────────────────────────────────

/// Blend a stream quality score with provider reliability.
///
/// `quality_score` comes from `quality::score()` (0.0–1.0).
/// `reliability` comes from `HealthRegistry::reliability_score()` (0.0–1.0).
///
/// Weight: 75% quality, 25% reliability — quality wins, but bad providers
/// get penalised enough to be overtaken by good ones.
pub fn blend_score(quality_score: f64, reliability: f64) -> f64 {
    0.75 * quality_score + 0.25 * reliability
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_provider_gets_benefit_of_doubt() {
        let r = HealthRegistry::new();
        assert_eq!(r.reliability_score("new-provider"), 1.0);
    }

    #[test]
    fn success_rate_tracks() {
        let r = HealthRegistry::new();
        r.record_success("p", 100);
        r.record_success("p", 200);
        r.record_failure("p", FailureKind::Error);
        let s = r.stats("p");
        assert!((s.success_rate() - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn blend_weights_quality_over_reliability() {
        let score = blend_score(1.0, 0.0);
        assert!((score - 0.75).abs() < 1e-6);
    }
}
