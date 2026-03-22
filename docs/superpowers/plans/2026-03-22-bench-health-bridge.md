# Bench Health Bridge Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire benchmark results into `HealthRegistry` so providers with consistently slow streams accumulate health failures and get ranked lower.

**Architecture:** A new `BenchHealthBridge` struct in `providers/bench_health_bridge.rs` tracks a consecutive-bad-result streak per provider and calls `HealthRegistry::record_failure/record_success` when the streak threshold is crossed. `Pipeline` constructs the bridge by cloning its existing `health` field (they share the same inner `Arc<Mutex<…>>`) and calls `bridge.record(stream)` after each `probe_all` in `resolve_streams_with_benchmark`.

**Tech Stack:** Rust, `std::sync::Mutex`, `std::collections::HashMap`. No new dependencies.

---

## ## Chunk 1: BenchHealthBridge struct + unit tests

### Task 1: Create `bench_health_bridge.rs` with failing tests

**Files:**
- Create: `runtime/src/providers/bench_health_bridge.rs`

- [ ] **Step 1: Add `latency_sum_ms()` accessor to `ProviderStats` in `health.rs`**

`ProviderStats::latency_sum_ms` is a private field. The tests need to read it. In `runtime/src/providers/health.rs`, inside `impl ProviderStats`, add:

```rust
pub fn latency_sum_ms(&self) -> u64 {
    self.latency_sum_ms
}
```

Verify it compiles:
```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo check 2>&1 | grep "^error"
```
Expected: no errors.

- [ ] **Step 2: Write the failing tests**

Create `runtime/src/providers/bench_health_bridge.rs` with the test module and a stub struct:

```rust
//! Bridge between stream benchmark results and provider health scoring.
//!
//! `BenchHealthBridge` translates consecutive below-threshold benchmark
//! results into `HealthRegistry::record_failure` calls, so providers that
//! habitually serve slow streams are ranked lower over time.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::providers::{FailureKind, HealthRegistry, Stream};

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
    pub fn record(&self, _stream: &Stream) {
        // stub — tests will drive the implementation
        todo!()
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

    // ── Neutral: speed_mbps: None never touches health ────────────────────

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

    // ── Bad: below-threshold results accumulate streak ────────────────────

    #[test]
    fn below_threshold_streak_does_not_fire_early() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        // 2 bad results — threshold is 3, so no failure yet
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
        bridge.record(&make_stream("prov", Some(0.5), None)); // 3rd → fires
        let s = stats(&bridge, "prov");
        assert_eq!(s.failure_count, 1, "exactly one failure on Nth bad result");

        // Streak reset: another 2 bad should not fire again
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        assert_eq!(stats(&bridge, "prov").failure_count, 1, "streak must reset to 0 after firing");
    }

    // ── Good: resets streak, records success ─────────────────────────────

    #[test]
    fn good_result_resets_streak_and_records_success() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        // 2 bad, then 1 good — failure must NOT fire
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(10.0), Some(50))); // good
        let s = stats(&bridge, "prov");
        assert_eq!(s.failure_count, 0, "good result must prevent failure from firing");
        assert_eq!(s.success_count, 1, "good result must record a success");

        // After reset, another 2 bad must not fire
        bridge.record(&make_stream("prov", Some(0.5), None));
        bridge.record(&make_stream("prov", Some(0.5), None));
        assert_eq!(stats(&bridge, "prov").failure_count, 0);
    }

    // ── Good with latency ─────────────────────────────────────────────────

    #[test]
    fn good_result_passes_latency_to_registry() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 3, 1.0);
        bridge.record(&make_stream("prov", Some(5.0), Some(45)));
        let s = stats(&bridge, "prov");
        assert_eq!(s.success_count, 1);
        // latency_sum_ms should equal 45
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

    // ── Integration: reliability_score drops after repeated failures ──────

    #[test]
    fn reliability_score_drops_after_repeated_bad_bench() {
        let bridge = BenchHealthBridge::with_config(HealthRegistry::new(), 2, 1.0);
        let score_before = bridge.health.reliability_score("prov");

        // Fire two failure cycles (4 bad results total, threshold=2)
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
```

- [ ] **Step 3: Run tests to verify they fail with `todo!()`**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo test providers::bench_health_bridge 2>&1 | head -30
```

Expected: panic on `todo!()` across all tests (not a compile error — Step 1 already added the accessor).

- [ ] **Step 4: Implement `record()`**

Replace the `todo!()` stub in `BenchHealthBridge::record` with:

```rust
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
```

- [ ] **Step 5: Run tests — verify they pass**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo test providers::bench_health_bridge -- --nocapture
```

Expected: all 8 tests pass.

- [ ] **Step 6: Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui
git add runtime/src/providers/health.rs runtime/src/providers/bench_health_bridge.rs
git commit -m "feat: add BenchHealthBridge with streak-based health penalty"
```

---

## ## Chunk 2: Export and wire into Pipeline

### Task 2: Export module and add to Pipeline

**Files:**
- Modify: `runtime/src/providers/mod.rs`
- Modify: `runtime/src/engine/pipeline.rs`

- [ ] **Step 1: Export `bench_health_bridge` from `providers/mod.rs`**

In `runtime/src/providers/mod.rs`, add after the `circuit_breaker` module line:

```rust
/// Bench health bridge — translates stream benchmark results into health score updates.
pub mod bench_health_bridge;
```

And add a re-export after the existing `pub use benchmark::StreamBenchmarker;` line:

```rust
#[allow(unused_imports)]
pub use bench_health_bridge::BenchHealthBridge;
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo check 2>&1 | grep -E "error|warning.*unused"
```

Expected: no errors.

- [ ] **Step 3: Add `bridge` field to `Pipeline` struct**

In `runtime/src/engine/pipeline.rs`, add the import at the top with the other providers imports:

```rust
use crate::providers::{HealthRegistry, ProviderThrottle, CircuitBreaker, StreamBenchmarker, BenchHealthBridge};
```

Then add the field to the `Pipeline` struct after `bench`:

```rust
    /// Bench health bridge — feeds probe_all results into provider health scoring.
    pub bridge: BenchHealthBridge,
```

- [ ] **Step 4: Construct bridge in `Pipeline::new()`**

In `Pipeline::new()`, after `let bench = StreamBenchmarker::new();`, add:

```rust
        let bridge = BenchHealthBridge::new(health.clone());
```

Then add `bridge` to the struct literal at the end of `Pipeline::new()`:

```rust
        Pipeline { engine, catalog, cache, policy, player,
                   rpc: Arc::new(PluginRpcManager::new()),
                   bus, health, throttle, circuit_breaker, config, bench, bridge }
```

- [ ] **Step 5: Call `bridge.record()` after `probe_all` in `resolve_streams`**

Find the line `let probed_streams = self.bench.probe_all(&streams).await;` in `resolve_streams` (around line 197). Add the bridge loop immediately after:

```rust
        let probed_streams = self.bench.probe_all(&streams).await;

        for stream in &probed_streams {
            self.bridge.record(stream);
        }
```

- [ ] **Step 6: Verify full compile**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo build 2>&1 | grep -E "^error"
```

Expected: clean build, no errors.

- [ ] **Step 7: Run all runtime tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/runtime
cargo test 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui
git add runtime/src/providers/mod.rs runtime/src/engine/pipeline.rs
git commit -m "feat: wire BenchHealthBridge into Pipeline.resolve_streams"
```
