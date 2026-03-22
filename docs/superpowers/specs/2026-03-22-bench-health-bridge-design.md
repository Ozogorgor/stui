# Bench Health Bridge — Design Spec

## Goal

Benchmark results currently feed into stream ranking (via speed bonus) but never update provider health scores. A provider whose streams consistently time out or deliver sub-1-Mbps throughput receives no long-term penalty. This spec describes a `BenchHealthBridge` that closes that gap by translating repeated bad benchmark results into health registry failures.

## Background

The runtime already has two relevant subsystems:

- **`providers/benchmark.rs`** — probes HTTP streams for throughput and latency. Results are used for stream selection but not fed anywhere else.
- **`providers/health.rs`** — `HealthRegistry` tracks per-provider `reliability_score` (0–1.0), updated via `record_success/record_failure()`. This score is blended into final ranking by `quality::rank_with_health_and_speed()`.

The missing link: benchmark results never call `HealthRegistry::record_success/failure()`.

## Design

### New file: `runtime/src/providers/bench_health_bridge.rs`

A single `BenchHealthBridge` struct. Responsibilities:

1. Receive one benchmark result at a time (`record(provider, result)`)
2. Classify it as good or bad
3. Track consecutive-bad streaks per provider
4. When the streak threshold is reached, call `HealthRegistry::record_failure()`
5. On a good result, reset the streak and call `HealthRegistry::record_success()`

```rust
pub struct BenchHealthBridge {
    health: Arc<HealthRegistry>,
    streaks: HashMap<String, u32>,   // provider → consecutive bad count
    streak_threshold: u32,           // default: 3
    slow_mbps_threshold: f64,        // default: 1.0
}
```

### Bad result definition

A benchmark result is classified as **bad** if either condition holds:
- `result.err.is_some()` — probe errored or timed out
- `result.speed_mbps > 0.0 && result.speed_mbps < slow_mbps_threshold` — measured speed below threshold

A result with `speed_mbps == 0.0` and no error (e.g. torrent seeder estimate) is treated as **neutral** — neither good nor bad — and does not affect the streak.

### Pattern and consequence

```
bad result  → streak++
             if streak >= threshold → record_failure(provider), streak = 0
good result → streak = 0, record_success(provider)
neutral     → no change
```

One health failure is recorded per `streak_threshold` consecutive bad results. Recovery is natural: good benchmarks reset the streak and record successes.

### Configuration

Both thresholds are constructor parameters with defaults:

| Field | Default | Meaning |
|---|---|---|
| `streak_threshold` | 3 | Consecutive bad results before one health failure |
| `slow_mbps_threshold` | 1.0 | Speed (Mbps) below which a result is "bad" |

Not wired into `stui.toml` for now — internal tuning knobs only.

### Wiring

`Engine` already owns both `StreamBenchmarker` and `HealthRegistry`. `BenchHealthBridge` is constructed in `Engine::new()` (or equivalent) and stored as a field. Wherever benchmark results are currently consumed (IPC handler or pipeline playback stage), a `bridge.record(provider, &result)` call is added.

### Effect on ranking

No changes to `quality::rank_with_health_and_speed()` are needed. It already reads `reliability_score` from `HealthRegistry`. Once the bridge updates scores, ranking picks them up automatically on the next query.

## Data Flow

```
StreamBenchmarker
      │  BenchmarkResult { provider, speed_mbps, err, ... }
      ▼
BenchHealthBridge::record()
      │  streak >= threshold?
      ├─ yes → HealthRegistry::record_failure(provider)
      └─ no  → (accumulate or reset streak)
                                    │
                                    ▼
                         HealthRegistry::record_success(provider)
                                    │
                                    ▼
                    rank_with_health_and_speed() reads reliability_score
```

## Testing

- **Unit: bad result accumulation** — N-1 bad results do not trigger failure; Nth does
- **Unit: good result resets streak** — streak resets and success is recorded
- **Unit: neutral result is ignored** — speed=0, no error → no streak change
- **Unit: slow-but-not-failing stream** — speed below threshold counts as bad
- **Integration: health score falls after repeated bad bench** — mock `HealthRegistry`, verify `record_failure` call count

## Files Touched

| File | Change |
|---|---|
| `runtime/src/providers/bench_health_bridge.rs` | New — `BenchHealthBridge` struct and logic |
| `runtime/src/providers/mod.rs` | Export `bench_health_bridge` module |
| `runtime/src/engine.rs` (or equivalent) | Construct bridge, store as field |
| Bench result consumer (IPC handler / pipeline) | Add `bridge.record(provider, &result)` call |
