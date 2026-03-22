//! Circuit breaker for provider requests.
//!
//! A circuit breaker prevents cascading failures by temporarily disabling a
//! provider after too many consecutive failures.  This gives the provider
//! time to recover and prevents wasted resources on likely-to-fail requests.
//!
//! # State Machine
//!
//! ```text
//!     ┌─────────────────────────────────────────────┐
//!     │                                             │
//!     │   ┌──────┐    failures >= threshold   ┌─────┴────┐
//!     │   │Closed│ ──────────────────────▶  │   Open    │
//!     │   └──┬───┘                          └───────────┘
//!     │      │ success                           │
//!     │      │                                   │ timeout
//!     │      │                                   ▼
//!     │      │                              ┌─────────┐
//!     │      └──────────────────────────────▶│Half-Open│
//!     │           success                    └───┬─────┘
//!     │                                           │ failure
//!     └───────────────────────────────────────────┘
//! ```
//!
//! # Configuration
//!
//! | Parameter           | Default | Description                           |
//! |--------------------|---------|---------------------------------------|
//! | `failure_threshold` | 5       | Failures before opening circuit      |
//! | `recovery_timeout`  | 60s     | Time before trying half-open        |
//! | `half_open_max`   | 1       | Test requests allowed in half-open  |
//!
//! # Usage
//!
//! See module tests for usage examples.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{info, warn};

// ── Circuit state ────────────────────────────────────────────────────────────

/// Current state of a provider's circuit breaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed — requests go through normally.
    Closed,
    /// Circuit is open — requests are blocked.
    Open,
    /// Circuit is half-open — one test request is allowed.
    HalfOpen,
}

/// Statistics for one provider's circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub provider: String,
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub total_failures: u64,
    pub total_successes: u64,
    pub total_trips: u64,
    pub opened_at: Option<Instant>,
    pub recovery_at: Option<Instant>,
}

impl CircuitBreakerStats {
    fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            state: CircuitState::Closed,
            consecutive_failures: 0,
            total_failures: 0,
            total_successes: 0,
            total_trips: 0,
            opened_at: None,
            recovery_at: None,
        }
    }
}

// ── Per-provider breaker state ───────────────────────────────────────────────

#[derive(Debug)]
struct BreakerState {
    /// Current circuit state.
    state: CircuitState,
    /// Consecutive failures since last success.
    consecutive_failures: u32,
    /// Total failures (for metrics).
    total_failures: u64,
    /// Total successes (for metrics).
    total_successes: u64,
    /// Total circuit trips (for metrics).
    total_trips: u64,
    /// When the circuit was opened.
    opened_at: Option<Instant>,
    /// When the circuit should attempt recovery.
    recovery_at: Option<Instant>,
    /// Number of test requests allowed in half-open state.
    half_open_requests: u32,
}

impl BreakerState {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            total_failures: 0,
            total_successes: 0,
            total_trips: 0,
            opened_at: None,
            recovery_at: None,
            half_open_requests: 1,
        }
    }
}

// ── Configuration ───────────────────────────────────────────────────────────

/// Configuration for a circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Time to wait before attempting recovery (half-open state).
    pub recovery_timeout: Duration,
    /// Maximum test requests allowed in half-open state.
    pub half_open_max: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(60),
            half_open_max: 1,
        }
    }
}

// ── Circuit breaker ──────────────────────────────────────────────────────────

/// Thread-safe circuit breaker for provider requests.
///
/// Cheap to clone — all clones share the underlying `Arc`.
#[derive(Clone)]
pub struct CircuitBreaker {
    inner: Arc<RwLock<HashMap<String, BreakerState>>>,
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default configuration.
    pub fn new() -> Self {
        Self::with_config(CircuitBreakerConfig::default())
    }

    /// Create a circuit breaker with custom configuration.
    pub fn with_config(config: CircuitBreakerConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Check if a provider is available (circuit is closed or half-open with permits).
    ///
    /// Returns `true` if requests can be made, `false` if the circuit is open.
    pub async fn is_available(&self, provider: &str) -> bool {
        let mut map = self.inner.write().await;
        match map.get_mut(provider) {
            Some(state) => {
                match state.state {
                    CircuitState::Closed => true,
                    CircuitState::HalfOpen => {
                        // Atomically consume a permit to prevent TOCTOU races.
                        if state.half_open_requests > 0 {
                            state.half_open_requests -= 1;
                            true
                        } else {
                            false
                        }
                    }
                    CircuitState::Open => {
                        // Check if recovery time has passed and transition to half-open
                        if let Some(recovery_at) = state.recovery_at {
                            if Instant::now() >= recovery_at {
                                state.state = CircuitState::HalfOpen;
                                state.half_open_requests = self.config.half_open_max;
                                // Consume one permit for this caller.
                                state.half_open_requests -= 1;
                                return true;
                            }
                        }
                        false
                    }
                }
            }
            None => true, // Unknown providers start closed
        }
    }

    /// Record a successful request. Closes the circuit if it was half-open.
    pub async fn record_success(&self, provider: &str) {
        let mut map = self.inner.write().await;
        let state = map.entry(provider.to_string()).or_insert_with(BreakerState::new);

        state.consecutive_failures = 0;
        state.total_successes += 1;
        state.half_open_requests = self.config.half_open_max;

        if state.state == CircuitState::HalfOpen {
            info!(provider, "circuit breaker closed (half-open → closed)");
            state.state = CircuitState::Closed;
            state.opened_at = None;
            state.recovery_at = None;
        }
    }

    /// Record a failed request. Opens the circuit if threshold is exceeded.
    pub async fn record_failure(&self, provider: &str) {
        let mut map = self.inner.write().await;
        let state = map.entry(provider.to_string()).or_insert_with(BreakerState::new);

        state.consecutive_failures += 1;
        state.total_failures += 1;

        let should_open = match state.state {
            CircuitState::Closed => state.consecutive_failures >= self.config.failure_threshold,
            CircuitState::HalfOpen => state.consecutive_failures >= 1,
            CircuitState::Open => false,
        };

        if should_open {
            state.state = CircuitState::Open;
            state.opened_at = Some(Instant::now());
            state.recovery_at = Some(Instant::now() + self.config.recovery_timeout);
            state.total_trips += 1;
            state.consecutive_failures = 0;
            state.half_open_requests = self.config.half_open_max;
            warn!(
                provider,
                failures = state.total_failures,
                recovery_timeout_secs = self.config.recovery_timeout.as_secs(),
                "circuit breaker OPENED"
            );
        }
    }

    /// Transition to half-open state. Called when recovery timeout expires.
    pub async fn transition_to_half_open(&self, provider: &str) {
        let mut map = self.inner.write().await;
        let state = map.entry(provider.to_string()).or_insert_with(BreakerState::new);

        if state.state == CircuitState::Open {
            state.state = CircuitState::HalfOpen;
            state.half_open_requests = self.config.half_open_max;
            info!(provider, "circuit breaker HALF-OPEN (open → half-open)");
        }
    }

    /// Get current state for a provider.
    pub async fn state(&self, provider: &str) -> CircuitState {
        let map = self.inner.read().await;
        map.get(provider)
            .map(|s| s.state.clone())
            .unwrap_or(CircuitState::Closed)
    }

    /// Get statistics for a provider.
    pub async fn stats(&self, provider: &str) -> CircuitBreakerStats {
        let map = self.inner.read().await;
        match map.get(provider) {
            Some(s) => CircuitBreakerStats {
                provider: provider.to_string(),
                state: s.state.clone(),
                consecutive_failures: s.consecutive_failures,
                total_failures: s.total_failures,
                total_successes: s.total_successes,
                total_trips: s.total_trips,
                opened_at: s.opened_at,
                recovery_at: s.recovery_at,
            },
            None => CircuitBreakerStats::new(provider),
        }
    }

    /// Get statistics for all providers.
    pub async fn all_stats(&self) -> Vec<CircuitBreakerStats> {
        let map = self.inner.read().await;
        map.iter()
            .map(|(name, s)| CircuitBreakerStats {
                provider: name.clone(),
                state: s.state.clone(),
                consecutive_failures: s.consecutive_failures,
                total_failures: s.total_failures,
                total_successes: s.total_successes,
                total_trips: s.total_trips,
                opened_at: s.opened_at,
                recovery_at: s.recovery_at,
            })
            .collect()
    }

    /// Reset all circuits to closed state (for testing or manual intervention).
    pub async fn reset(&self) {
        let mut map = self.inner.write().await;
        map.clear();
    }

    /// Providers with open circuits.
    pub async fn open_providers(&self) -> Vec<String> {
        let map = self.inner.read().await;
        map.iter()
            .filter(|(_, s)| s.state == CircuitState::Open)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Providers currently in half-open state.
    pub async fn half_open_providers(&self) -> Vec<String> {
        let map = self.inner.read().await;
        map.iter()
            .filter(|(_, s)| s.state == CircuitState::HalfOpen)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Consume a half-open permit if available.
    pub async fn try_acquire_half_open(&self, provider: &str) -> bool {
        let mut map = self.inner.write().await;
        let state = map.entry(provider.to_string()).or_insert_with(BreakerState::new);

        if state.state == CircuitState::HalfOpen && state.half_open_requests > 0 {
            state.half_open_requests -= 1;
            return true;
        }
        false
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_provider_is_available() {
        let cb = CircuitBreaker::new();
        assert!(cb.is_available("new-provider").await);
    }

    #[tokio::test]
    async fn opens_after_threshold_failures() {
        let cb = CircuitBreaker::with_config(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        for i in 0..3 {
            cb.record_failure("test-provider").await;
            assert_eq!(i < 2, cb.is_available("test-provider").await);
        }

        assert!(!cb.is_available("test-provider").await);
    }

    #[tokio::test]
    async fn success_resets_failures() {
        let cb = CircuitBreaker::new();
        cb.record_failure("p").await;
        cb.record_failure("p").await;
        assert_eq!(cb.state("p").await, CircuitState::Closed);

        cb.record_success("p").await;
        assert_eq!(cb.state("p").await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn half_open_on_success_after_failure() {
        let cb = CircuitBreaker::new();

        // Open the circuit
        for _ in 0..5 {
            cb.record_failure("p").await;
        }
        assert_eq!(cb.state("p").await, CircuitState::Open);

        // Manually transition to half-open (simulating timeout)
        cb.transition_to_half_open("p").await;
        assert_eq!(cb.state("p").await, CircuitState::HalfOpen);

        // Success closes the circuit
        cb.record_success("p").await;
        assert_eq!(cb.state("p").await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn failure_in_half_open_reopens() {
        let cb = CircuitBreaker::new();

        // Open the circuit
        for _ in 0..5 {
            cb.record_failure("p").await;
        }

        // Transition to half-open
        cb.transition_to_half_open("p").await;
        assert_eq!(cb.state("p").await, CircuitState::HalfOpen);

        // Failure re-opens
        cb.record_failure("p").await;
        assert_eq!(cb.state("p").await, CircuitState::Open);
    }

    #[tokio::test]
    async fn reset_clears_all() {
        let cb = CircuitBreaker::new();

        for _ in 0..5 {
            cb.record_failure("p").await;
        }
        assert_eq!(cb.state("p").await, CircuitState::Open);

        cb.reset().await;
        assert_eq!(cb.state("p").await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn tracks_trip_count() {
        let cb = CircuitBreaker::new();

        for _ in 0..5 {
            cb.record_failure("p").await;
        }

        let stats = cb.stats("p").await;
        assert_eq!(stats.total_trips, 1);
    }
}
