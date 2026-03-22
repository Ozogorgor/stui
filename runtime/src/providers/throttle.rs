//! Per-provider rate-limit throttle.
//!
//! Scrapers and APIs frequently impose rate limits.  Without a throttle,
//! aggressive fan-out searches can trigger HTTP 429 responses and temporary
//! IP bans.  This module implements a simple token-bucket limiter that:
//!
//! - Caps the maximum requests per second per provider.
//! - Enforces an exponential back-off when a 429 is detected.
//! - Provides a cooldown check so the engine can skip a cooling provider
//!   rather than waiting synchronously.
//!
//! # Usage
//!
//! See module tests for usage examples.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::warn;

// ── Per-provider limiter state ────────────────────────────────────────────────

#[derive(Debug)]
struct LimiterState {
    /// Maximum tokens in the bucket (= max requests per second).
    capacity: u32,

    /// Currently available tokens.
    tokens: f64,

    /// Timestamp of the last token refill.
    last_refill: Instant,

    /// If set, no requests are allowed until this instant.
    cooldown_until: Option<Instant>,

    /// Consecutive 429 count — used to compute exponential back-off.
    consecutive_429s: u32,
}

impl LimiterState {
    fn new(capacity: u32) -> Self {
        LimiterState {
            capacity,
            tokens: capacity as f64,
            last_refill: Instant::now(),
            cooldown_until: None,
            consecutive_429s: 0,
        }
    }

    /// Refill tokens proportionally to elapsed time since last refill.
    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_secs_f64();
        let refill_amount = elapsed * self.capacity as f64;
        self.tokens = (self.tokens + refill_amount).min(self.capacity as f64);
        self.last_refill = Instant::now();
    }

    /// True if this provider is in a forced cooldown period.
    fn is_cooling_down(&self) -> bool {
        self.cooldown_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Time remaining in the current cooldown, if any.
    fn cooldown_remaining(&self) -> Option<Duration> {
        self.cooldown_until.and_then(|t| {
            let now = Instant::now();
            if now < t { Some(t - now) } else { None }
        })
    }

    /// Consume one token.  Returns the time to wait before the token is
    /// available, or `None` if a token was available immediately.
    fn try_acquire(&mut self) -> Option<Duration> {
        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None // no wait
        } else {
            // Calculate wait time until next token
            let wait_secs = (1.0 - self.tokens) / self.capacity as f64;
            Some(Duration::from_secs_f64(wait_secs))
        }
    }

    /// Enter cooldown after a 429 response.  Uses exponential back-off.
    fn enter_cooldown(&mut self, hint_secs: Option<u64>) {
        self.consecutive_429s += 1;
        let backoff = hint_secs.unwrap_or_else(|| {
            // Exponential: 5s, 10s, 20s, 40s, 80s, … capped at 10 minutes
            let base = 5u64;
            let exp = self.consecutive_429s.saturating_sub(1);
            (base << exp).min(600)
        });
        self.cooldown_until = Some(Instant::now() + Duration::from_secs(backoff));
        // Drain all tokens during cooldown
        self.tokens = 0.0;
    }

    /// Called after a successful request — resets the 429 counter.
    fn record_success(&mut self) {
        self.consecutive_429s = 0;
    }
}

// ── ProviderThrottle ──────────────────────────────────────────────────────────

/// Thread-safe, per-provider rate limiter.
///
/// Cheap to clone — all clones share the underlying `Arc`.
#[derive(Clone)]
pub struct ProviderThrottle {
    inner: Arc<Mutex<HashMap<String, LimiterState>>>,
    /// Default capacity for new providers (requests per second).
    default_capacity: u32,
}

impl ProviderThrottle {
    /// Create a new throttle with `default_capacity` req/s for unknown providers.
    pub fn new() -> Self {
        ProviderThrottle {
            inner:            Arc::new(Mutex::new(HashMap::new())),
            default_capacity: 4,
        }
    }

    /// Override the default rate for all unknown providers.
    pub fn with_default_capacity(mut self, rps: u32) -> Self {
        self.default_capacity = rps;
        self
    }

    /// Set the rate limit for a specific provider (requests per second).
    pub async fn set_limit(&self, provider: &str, requests_per_second: u32) {
        let mut map = self.inner.lock().await;
        let entry = map.entry(provider.to_string())
            .or_insert_with(|| LimiterState::new(requests_per_second));
        entry.capacity = requests_per_second;
    }

    /// Wait until a token is available for `provider`, then consume it.
    ///
    /// If the provider is in a rate-limit cooldown, this waits for the
    /// full cooldown to expire before returning.  For large cooldowns
    /// (>10s) the caller may prefer to check `is_cooling_down` and skip
    /// the provider entirely.
    pub async fn acquire(&self, provider: &str) {
        loop {
            let wait = {
                let mut map = self.inner.lock().await;
                let cap = self.default_capacity;
                let state = map.entry(provider.to_string())
                    .or_insert_with(|| LimiterState::new(cap));

                if let Some(remaining) = state.cooldown_remaining() {
                    Some(remaining)
                } else {
                    state.try_acquire()
                }
            };

            match wait {
                None => return, // token acquired
                Some(d) => {
                    if d > Duration::from_secs(30) {
                        warn!(provider, secs = d.as_secs(), "long throttle wait — consider skipping");
                    }
                    tokio::time::sleep(d).await;
                }
            }
        }
    }

    /// Check whether a provider is in cooldown without consuming a token.
    pub async fn is_cooling_down(&self, provider: &str) -> bool {
        self.inner.lock().await
            .get(provider)
            .map(|s| s.is_cooling_down())
            .unwrap_or(false)
    }

    /// Time remaining in the cooldown for `provider`, or `None`.
    pub async fn cooldown_remaining(&self, provider: &str) -> Option<Duration> {
        self.inner.lock().await
            .get(provider)
            .and_then(|s| s.cooldown_remaining())
    }

    /// Call this when a provider returns HTTP 429 or signals rate-limiting.
    ///
    /// `retry_after_secs` is the value from the `Retry-After` header, if present.
    pub async fn record_rate_limited(&self, provider: &str, retry_after_secs: Option<u64>) {
        let mut map = self.inner.lock().await;
        let cap = self.default_capacity;
        let state = map.entry(provider.to_string())
            .or_insert_with(|| LimiterState::new(cap));
        state.enter_cooldown(retry_after_secs);
        warn!(
            provider,
            backoff_secs = state.cooldown_remaining().map(|d| d.as_secs()).unwrap_or(0),
            "provider rate limited — entering cooldown"
        );
    }

    /// Call this after a successful request to reset the back-off counter.
    pub async fn record_success(&self, provider: &str) {
        let mut map = self.inner.lock().await;
        if let Some(state) = map.get_mut(provider) {
            state.record_success();
        }
    }

    /// All providers currently in cooldown, with remaining seconds.
    pub async fn cooling_down_providers(&self) -> Vec<(String, u64)> {
        self.inner.lock().await
            .iter()
            .filter_map(|(name, state)| {
                state.cooldown_remaining()
                    .map(|d| (name.clone(), d.as_secs()))
            })
            .collect()
    }
}

impl Default for ProviderThrottle {
    fn default() -> Self { Self::new() }
}

// ── Default rate limits ───────────────────────────────────────────────────────

/// Apply sensible default rate limits for known providers.
pub fn apply_default_limits(throttle: &mut ProviderThrottle) {
    tokio::spawn({
        let t = throttle.clone();
        async move {
            t.set_limit("tmdb",          4).await;  // TMDB: 4 req/s (their public limit)
            t.set_limit("omdb",          1).await;  // OMDB: ~1 req/s free tier
            t.set_limit("torrentio",     2).await;  // Torrentio: be polite
            t.set_limit("prowlarr",      5).await;  // Local — can be higher
            t.set_limit("opensubtitles", 2).await;  // OS has strict limits
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_provider_not_cooling_down() {
        let t = ProviderThrottle::new();
        assert!(!t.is_cooling_down("unknown").await);
    }

    #[tokio::test]
    async fn cooldown_set_after_429() {
        let t = ProviderThrottle::new();
        t.record_rate_limited("tmdb", Some(60)).await;
        assert!(t.is_cooling_down("tmdb").await);
        let remaining = t.cooldown_remaining("tmdb").await;
        assert!(remaining.is_some());
        assert!(remaining.unwrap().as_secs() <= 60);
    }

    #[tokio::test]
    async fn success_clears_429_counter() {
        let t = ProviderThrottle::new();
        t.record_rate_limited("tmdb", Some(0)).await; // 0s cooldown = expired immediately
        t.record_success("tmdb").await;
        // After success the backoff counter resets (next 429 will be 5s again)
        // We can't directly test the counter but we can verify no lingering cooldown
        // when retry_after was 0
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!t.is_cooling_down("tmdb").await);
    }
}
