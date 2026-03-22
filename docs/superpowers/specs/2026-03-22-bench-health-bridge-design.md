# Bench Health Bridge — Design Spec

## Goal

Benchmark results currently feed into stream ranking (via speed bonus) but never update provider health scores. A provider whose streams consistently deliver sub-1-Mbps throughput receives no long-term penalty. This spec describes a `BenchHealthBridge` that translates repeated bad benchmark results into health registry failures.

## Background

The runtime already has two relevant subsystems, both owned by `Pipeline` in `engine/pipeline.rs`:

- **`bench: StreamBenchmarker`** — probes HTTP streams via `probe_all(&[Stream]) -> Vec<Stream>`, populating `speed_mbps` and `latency_ms` on each stream. Non-HTTP URLs (magnet/torrent) and probe errors both return `speed_mbps: None` — the error string is discarded by `probe_all` and not available downstream.
- **`health: HealthRegistry`** — tracks per-provider `reliability_score` (0.0–1.0) via `record_success(provider, latency_ms: u64)` and `record_failure(provider, kind: FailureKind)`. This score is blended into final ranking by `rank_with_health_and_speed()`.

The missing link: `probe_all` results are used for speed-based ranking but never fed into `HealthRegistry`.

## Design

### New file: `runtime/src/providers/bench_health_bridge.rs`

A single `BenchHealthBridge` struct with one public method: `record(stream)`.

```rust
pub struct BenchHealthBridge {
    health: HealthRegistry,
    streaks: Mutex<HashMap<String, u32>>,  // provider → consecutive bad count
    streak_threshold: u32,                  // default: 3
    slow_mbps_threshold: f64,               // default: 1.0
}
```

`HealthRegistry` is already `Clone` and shares inner `Arc<Mutex<…>>` state — no additional `Arc` wrapper is needed. `BenchHealthBridge` holds a plain clone of `Pipeline`'s `health` field.

`streaks` is wrapped in `Mutex` as a defensive measure in case the bridge is called from multiple async contexts in the future.

### Input type

The bridge consumes `&Stream` (output of `probe_all`), not `&ProbeResult`. The relevant fields are:

- `stream.speed_mbps: Option<f64>` — measured throughput
- `stream.latency_ms: Option<u32>` — first-byte latency
- `stream.provider: String` — provider name

### Result classification

| `speed_mbps` value | Classification | Reason |
|---|---|---|
| `Some(x)` where `x >= threshold` | **Good** | Adequate throughput |
| `Some(x)` where `x < threshold` | **Bad** | Below minimum usable speed |
| `None` | **Neutral** | Non-HTTP stream or probe error — skip |

`None` is always neutral. Since `probe_all` discards the error string, there is no way to distinguish a torrent stream (correctly skipped) from an HTTP timeout — treating both as neutral avoids false positives against torrent providers.

### Streak logic

```
bad result  → streak++ for provider
              if streak >= streak_threshold:
                  record_failure(provider, FailureKind::Error)
                  streak = 0   ← reset to zero, not one
good result → streak = 0 for provider
              record_success(provider, latency_ms.unwrap_or(0) as u64)
neutral     → no change
```

One health failure is recorded per `streak_threshold` consecutive bad results. The streak resets to zero after firing (not one), so the next failure cycle requires a full `streak_threshold` bad results again. Good results reset immediately and record a success to aid recovery.

`FailureKind::Error` is used for slow-stream penalties — there is no `Slow` variant in `FailureKind`. `Error` is the closest available classification for "stream failed to meet quality bar". `latency_ms` is passed as `0` when not measured (`None`) — this is an explicit policy, not an accident.

### Wiring

`BenchHealthBridge` is added as a field on `Pipeline` alongside `health` and `bench`. It is constructed in `Pipeline::new()` and called in `resolve_streams()` right after `probe_all` returns, iterating the results before they are passed to `rank_with_health_and_speed`:

```rust
// engine/pipeline.rs, after probe_all
let probed_streams = self.bench.probe_all(&streams).await;

for stream in &probed_streams {
    self.bridge.record(stream);
}

let mut speed_map = ...
```

`BenchHealthBridge` is constructed in `Pipeline::new()` by cloning `health`:

```rust
let bridge = BenchHealthBridge::new(health.clone());
```

Because `HealthRegistry::clone()` shares the underlying `Arc<Mutex<…>>`, both `pipeline.health` and `bridge.health` refer to the same data. No additional `Arc` wrapping is needed.

### Effect on ranking

No changes to `rank_with_health_and_speed()` are needed. It already reads `reliability_score` from `HealthRegistry`. Updated scores are picked up automatically on the next `resolve_streams` call.

## Data Flow

```
StreamBenchmarker::probe_all(&streams)
        │  Vec<Stream> with speed_mbps / latency_ms populated
        ▼
for each Stream:
    BenchHealthBridge::record(provider, stream)
        ├─ neutral (speed_mbps: None) → skip
        ├─ bad (speed < threshold)    → streak++
        │       if streak >= threshold:
        │           HealthRegistry::record_failure(provider, FailureKind::Error)
        │           streak = 0
        └─ good (speed >= threshold)  → streak = 0
                    HealthRegistry::record_success(provider, latency_ms.unwrap_or(0))
        │
        ▼
rank_with_health_and_speed() reads updated reliability_score
```

## Configuration

Both thresholds are constructor parameters with defaults. Not wired into `stui.toml` for now.

| Field | Default | Meaning |
|---|---|---|
| `streak_threshold` | 3 | Consecutive bad results before one health failure |
| `slow_mbps_threshold` | 1.0 | Speed (Mbps) below which a result is "bad" |

## Testing

- **Unit: N-1 bad results do not fire** — send `streak_threshold - 1` slow streams; verify `record_failure` not called
- **Unit: Nth bad result fires** — Nth slow stream calls `record_failure` and resets streak to 0
- **Unit: good result resets streak** — 2 bad, then 1 good, then 2 bad → no failure fires on second run
- **Unit: neutral result is ignored** — `speed_mbps: None` → no streak change, no registry calls
- **Unit: slow-but-not-failing stream** — `speed_mbps: Some(0.5)` below 1.0 threshold → counts as bad
- **Unit: latency passed to record_success** — good result with `latency_ms: Some(45)` passes `45u64`; `latency_ms: None` passes `0u64`
- **Integration: reliability_score falls after repeated bad bench** — use real `HealthRegistry`, verify `reliability_score` decreases after threshold failures

## Files Touched

| File | Change |
|---|---|
| `runtime/src/providers/bench_health_bridge.rs` | **New** — `BenchHealthBridge` struct and logic |
| `runtime/src/providers/mod.rs` | Export `bench_health_bridge` module |
| `runtime/src/engine/pipeline.rs` | Add `bridge` field; construct in `Pipeline::new()`; call `bridge.record()` after `probe_all`; wrap `health` in `Arc` |
