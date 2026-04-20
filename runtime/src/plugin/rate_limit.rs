//! Minimal async token-bucket rate limiter.
//!
//! Tokens refill at `rps` per second up to `burst`. `acquire()` returns as
//! soon as a token is available; callers that need a non-blocking variant
//! can race this future against a timeout.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Shared state inside a `TokenBucket`. Split out so `TokenBucket` can be
/// `Clone` without re-allocating tokens.
#[derive(Debug)]
struct Inner {
    /// Fractional token count — refills at `rps` tokens/sec up to `burst`.
    tokens: f64,
    /// Bucket size (maximum burst).
    burst: f64,
    /// Refill rate in tokens/second.
    rps: f64,
    /// Last observed Instant used to compute refill since.
    last: Instant,
}

#[derive(Debug, Clone)]
pub struct TokenBucket {
    inner: Arc<Mutex<Inner>>,
}

impl TokenBucket {
    /// Build a new bucket with `rps` refill and `burst` capacity.
    ///
    /// `burst` is clamped to at least 1 (a bucket of 0 would never emit).
    /// `rps = 0` means no refill — the bucket drains once and stays empty,
    /// effectively disabling calls. Callers should treat `rps = 0` as "disabled".
    pub fn new(rps: u32, burst: u32) -> Self {
        let burst = burst.max(1) as f64;
        Self {
            inner: Arc::new(Mutex::new(Inner {
                tokens: burst,
                burst,
                rps: rps as f64,
                last: Instant::now(),
            })),
        }
    }

    /// Await a token. Refills the bucket based on elapsed time, then either
    /// consumes a token (returning immediately) or sleeps until one is due.
    pub async fn acquire(&self) {
        loop {
            let sleep_for = {
                let mut g = self.inner.lock().await;
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(g.last).as_secs_f64();
                g.last = now;
                // Refill.
                g.tokens = (g.tokens + elapsed * g.rps).min(g.burst);

                if g.tokens >= 1.0 {
                    g.tokens -= 1.0;
                    return;
                }

                // Compute time until the next full token.
                // tokens needed = 1.0 - g.tokens
                // at rate g.rps, secs needed = (1.0 - g.tokens) / g.rps
                if g.rps <= 0.0 {
                    // Disabled bucket — sleep a long time and re-check.
                    Duration::from_secs(3600)
                } else {
                    let needed = (1.0 - g.tokens) / g.rps;
                    Duration::from_secs_f64(needed.max(0.0))
                }
            };
            tokio::time::sleep(sleep_for).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 rps bucket with burst=1: 5 consecutive acquires should complete in
    /// roughly (5 - burst) / rps = 0.4s of simulated time, i.e. ~400ms.
    #[tokio::test(start_paused = true)]
    async fn ten_rps_burst_one_paces_five_acquires_over_400ms() {
        let bucket = TokenBucket::new(10, 1);
        let start = Instant::now();

        for _ in 0..5 {
            bucket.acquire().await;
        }

        let elapsed = start.elapsed();
        // First token is immediate (bucket starts full with burst=1);
        // remaining 4 each wait ~100ms for a new token.
        // Tolerate small scheduling slack.
        assert!(
            elapsed >= Duration::from_millis(395),
            "expected >= 395ms, got {:?}",
            elapsed
        );
        assert!(
            elapsed <= Duration::from_millis(450),
            "expected <= 450ms, got {:?}",
            elapsed
        );
    }

    /// Bucket with burst=3 should allow 3 acquires instantly, then throttle.
    #[tokio::test(start_paused = true)]
    async fn burst_limits_instant_acquires() {
        let bucket = TokenBucket::new(1, 3);
        let start = Instant::now();

        // Three instant acquires (bucket starts full).
        for _ in 0..3 {
            bucket.acquire().await;
        }
        let after_burst = start.elapsed();
        assert!(
            after_burst < Duration::from_millis(50),
            "burst should be ~instant, got {:?}",
            after_burst
        );

        // Fourth acquire must wait roughly 1 second for a new token at rps=1.
        bucket.acquire().await;
        let total = start.elapsed();
        assert!(
            total >= Duration::from_millis(995),
            "fourth should wait ~1s, total = {:?}",
            total
        );
    }

    #[tokio::test(start_paused = true)]
    async fn zero_burst_is_clamped_to_one() {
        let bucket = TokenBucket::new(100, 0);
        // First acquire should be instant despite burst=0 (clamped to 1).
        bucket.acquire().await;
    }
}
