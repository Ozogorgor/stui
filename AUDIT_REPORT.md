# Stui Codebase Audit Report

**Date:** 2026-03-19  
**Auditor:** Claude Code  
**Project:** Stui - Terminal Streaming Platform  
**Stack:** Go (TUI) + Rust (Runtime)

---

## 1. Executive Summary

Stui is a sophisticated terminal-based streaming platform with a well-architected separation between the Go TUI frontend (BubbleTea) and Rust runtime backend (Tokio). The project demonstrates solid engineering practices but has opportunities for improvement in testing, code organization, and security hardening.

**Overall Assessment:** Solid foundation with identified areas for improvement.

---

## 2. Architecture Overview

### 2.1 High-Level Design

```
┌─────────────────────────────────────────────────────────┐
│                    Go TUI (BubbleTea)                   │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌────────────┐  │
│  │ Screens │  │Components│  │ Keybinds│  │  Theme/L10n│  │
│  └────┬────┘  └────┬────┘  └─────────┘  └────────────┘  │
│       │            │                                     │
│  ┌────┴────────────┴────┐                               │
│  │    IPC Client (NDJSON)│                               │
│  └──────────┬───────────┘                               │
└─────────────┼───────────────────────────────────────────┘
              │ stdin/stdout (NDJSON)
              ▼
┌─────────────────────────────────────────────────────────┐
│                   Rust Runtime (Tokio)                   │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌────────────┐  │
│  │ Engine  │  │Pipeline │  │Providers│  │  Player    │  │
│  │(Plugins)│  │         │  │         │  │  (mpv/MPD) │  │
│  └─────────┘  └─────────┘  └─────────┘  └────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### 2.2 Strengths

- Clean IPC boundary between Go and Rust
- Feature-flagged modular builds (torrent, music, anime, radio, wasm-host)
- EventBus for loose coupling
- Provider trait enables extensibility
- Comprehensive documentation in docs/
- NDJSON wire protocol with typed messages

### 2.3 Weaknesses

- Runtime crash terminates TUI (no graceful degradation)
- IPC latency for high-frequency updates (e.g., playback progress)

---

## 3. Go Frontend Analysis

### 3.1 Structure

```
tui/
├── cmd/stui/main.go          # Entry point
├── internal/
│   ├── ipc/                  # NDJSON client (REFACTORED)
│   │   ├── ipc.go            # ~110 lines - Client struct, Start/Stop
│   │   ├── types.go          # Wire types (MediaTab, RawResponse, etc.)
│   │   ├── messages.go       # BubbleTea message types
│   │   ├── requests.go       # Public request methods
│   │   ├── internal.go       # Private methods (ping, readLoop, etc.)
│   │   ├── mpd_music.go      # MPD/music-specific methods
│   │   ├── ipc_test.go       # IPC integration tests
│   │   └── mock_runtime.go    # Mock runtime for testing
│   ├── state/                # App state management
│   └── ui/
│       ├── ui.go             # ~2500 lines - main model
│       ├── screens/          # 20+ screen implementations
│       └── components/       # Reusable UI components
├── pkg/
│   ├── log/                    # Structured logging (slog-based)
│   ├── theme/                  # Lipgloss styling
│   ├── keybinds/               # Keybinding system
│   ├── session/                # Session persistence
│   ├── mediacache/             # Offline cache
│   ├── collections/            # Collections
│   └── watchhistory/            # Watch history
└── go.mod
```

### 3.2 Issues Identified

#### Critical
- **None identified**

#### High
| Issue | Location | Description |
|-------|----------|-------------|
| ~~Large files~~ | `ipc.go` | **REFACTORED** - split into focused modules |
| Global theme state | `pkg/theme/` | Theme uses global `T` variable; makes testing difficult |
| ~~Untested IPC~~ | `internal/ipc/` | **FIXED** - comprehensive tests added |

#### Medium
| Issue | Location | Description |
|-------|----------|-------------|
| Missing integration tests | `tui/` | Only unit tests exist; no end-to-end IPC tests |
| Error swallowing | Various | Some `unwrap_or_default()` could silently hide errors |
| Magic numbers | `screens/stream_picker.go` | Quality rank values hardcoded |

#### Low
| Issue | Location | Description |
|-------|----------|-------------|
| ~~No structured logging~~ | `tui/pkg/log/` | **COMPLETED** - Added slog-based structured logging |
| No context propagation | IPC client | Missing `context.Context` for cancellation |

---

## 4. Rust Backend Analysis

### 4.1 Structure

```
runtime/src/
├── main.rs                    # Entry point + IPC loop
├── lib.rs                     # Library exports
├── engine/                    # Plugin management
├── pipeline/                  # Search, resolve, playback pipelines
├── providers/                 # Built-in providers
│   ├── metadata/              # TMDB, IMDB, OMDB, AniList, Last.fm
│   └── streams/               # Torrent, HTTP VOD, direct
├── player/                     # mpv IPC, MPD integration
├── config/                     # TOML config + live updates
├── cache/                      # TTL caching
├── catalog/                    # Content catalog
├── catalog_engine/             # Aggregation, filters, ranking
├── quality/                    # Stream quality scoring
├── skipper/                    # Intro/credits detection
├── aria2_bridge/               # aria2c torrent client
├── mpd_bridge/                 # Music Player Daemon
├── plugin_rpc/                 # Plugin RPC supervisor
├── plugin.rs                   # Plugin trait
├── ipc/                        # Wire protocol types
└── sandbox.rs                  # WASM sandboxing
```

### 4.2 Issues Identified

#### Critical
| Issue | Location | Description |
|-------|----------|-------------|
| **WASM sandbox bypass** | `sandbox.rs` | WASM plugins run in wasmtime but sandbox configuration not audited |

#### High
| Issue | Location | Description |
|-------|----------|-------------|
| Plugin arbitrary execution | `plugin_rpc/` | RPC plugins can execute any process; no security boundary |
| Unbounded concurrency | `pipeline/` | Fan-out to providers could overwhelm system |

#### Medium
| Issue | Location | Description |
|-------|----------|-------------|
| Missing rate limit config | `providers/throttle.rs` | Token bucket exists but limits not exposed in config |
| No request timeouts | HTTP clients | Some reqwest calls lack explicit timeouts |
| Cache invalidation | `cache/` | TTL-only; no manual invalidation |

#### Low
| Issue | Location | Description |
|-------|----------|-------------|
| Panics in plugins | `engine/` | Plugin panics could crash runtime |
| No circuit breakers | `providers/` | No fallback when providers fail repeatedly |

---

## 5. Security Analysis

### 5.1 Secrets Management

| Item | Risk Level | Status |
|------|------------|--------|
| API keys in config | Medium | **FIXED** - Now loaded from `~/.stui/secrets.env` or env vars |
| MPD password | Medium | **FIXED** - Now loaded from `~/.stui/secrets.env` or env vars |
| Torrent URLs | External | Inherent to application purpose |
| WASM plugin sandbox | Low | wasmtime sandbox, but not formally audited |

### 5.2 Implemented Secret Management

**Location:** `runtime/src/config/secrets.rs`

Secrets are now loaded from (priority order):
1. Environment variables (highest priority)
2. `~/.stui/secrets.env` file (`.env` format)
3. Config file values (lowest priority)

**Supported secrets:**
- `TMDB_API_KEY`
- `OMDB_API_KEY`
- `LASTFM_API_KEY`
- `MPD_PASSWORD`
- `PROWLARR_API_KEY`
- `OPENSUBTITLES_API_KEY`
- `TORRENTIO_API_KEY`

**Security features:**
- `SecretString` type for redacted display in logs/config exports
- Secrets file should have restricted permissions: `chmod 600 secrets.env`
- Clear separation between config (public) and secrets (sensitive)

### 5.3 Remaining Recommendations

1. ~~Add secret management~~ **COMPLETED**
   - ✅ Support `.env` loading for sensitive values
   - ⬜ Consider keyring integration (e.g., `secret-service`) - optional future enhancement

2. **Harden plugin execution**
   - Add plugin signing/verification
   - Implement resource limits (CPU, memory) for RPC plugins
   - Add seccomp/sandbox for subprocess execution

3. **Secure IPC**
   - Consider TLS for Unix socket in daemon mode
   - Add message signing for authenticity

---

## 6. Testing Coverage

### 6.1 Go Tests

| Area | Coverage | Status |
|------|----------|--------|
| Theme | Theme colors | Minimal |
| Screens | Settings, Stream Picker, Common | Minimal |
| State | App state | Minimal |
| History | Watch history | Minimal |
| IPC | NDJSON protocol, types, routing, events | **Good** |
| Integration | Screen flows | **None** |

### 6.2 Rust Tests

| Area | Coverage | Status |
|------|----------|--------|
| Ranking | Stream quality (property-based) | **Excellent** |
| Providers | Catalog, filters | Good |
| Config | Live updates | Good |
| Pipeline | Orchestration | Good |
| Health | Provider tracking | Good |
| Integration | Runtime workflows | **None** |

### 6.3 Recommendations

1. ~~Add Go IPC integration tests~~ **COMPLETED** ✅
2. Add end-to-end runtime tests (spawn runtime, send commands)
3. ~~Add property-based tests for ranking logic~~ **COMPLETED** ✅
4. Add fuzzy tests for provider responses

---

## 7. Dependencies

### 7.1 Go

| Dependency | Version | Risk |
|------------|---------|------|
| bubbletea | v0.26.4 | Low - stable |
| bubbles | v0.18.0 | Low - stable |
| lipgloss | v0.11.0 | Low - stable |

### 7.2 Rust

| Dependency | Version | Notes |
|------------|---------|-------|
| tokio | 1.x | Stable |
| reqwest | 0.12 | Stable |
| serde | 1 | Stable |
| wasmtime | 22 | Verify security advisories |

### 7.3 Recommendations

- Pin exact versions for reproducible builds
- Add `cargo-audit` to CI
- Review wasmtime security advisories regularly

---

## 8. Code Quality

### 8.1 Go Code Quality

| Metric | Score | Notes |
|--------|-------|-------|
| Formatting | Good | Uses gofmt |
| Linting | Good | go vet passes |
| Type safety | Good | Strong typing |
| Error handling | Fair | Some unwrapping |

**Recommendations:**
- Enable golangci-lint with stricter rules
- Split `ui.go` into smaller modules
- ~~Add structured logging (e.g., `zap` or `slog`)~~ **COMPLETED** - Uses Go 1.21+ slog package

### 8.2 Rust Code Quality

| Metric | Score | Notes |
|--------|-------|-------|
| Formatting | Good | cargo fmt |
| Linting | Good | clippy mostly passes |
| Type safety | Excellent | Strong typing throughout |
| Error handling | Good | thiserror, anyhow used appropriately |

**Recommendations:**
- Add more documentation comments (rustdoc)
- Enable stricter clippy lints
- Add `#![deny(warnings)]` in release builds

---

## 9. Performance Observations

### 9.1 Potential Bottlenecks

| Area | Issue | Impact |
|------|-------|--------|
| Provider fan-out | Sequential aggregation | High latency |
| TMDB rate limiting | 40 req/10s | Blocks search |
| Catalog processing | Single thread | Slow with large catalogs |
| IPC throughput | NDJSON parsing | May lag during playback |

### 9.2 Recommendations

1. Parallelize provider aggregation with timeout
2. Add local TMDB cache to reduce API calls
3. Consider parallel catalog processing
4. Batch IPC updates during playback (throttle updates)

---

## 10. Documentation

### 10.1 Strengths

- `docs/ARCHITECTURE.md` - Good overview
- `docs/runtime-ipc.md` - IPC protocol documented
- `docs/plugins.md` - Plugin system documented
- Inline comments in critical paths

### 10.2 Gaps

| Gap | Status |
|-----|--------|
| No API documentation for IPC types | Partially addressed in source |
| ~~No developer setup guide~~ | **COMPLETED** - docs/DEVELOPER_SETUP.md |
| ~~No contributing guidelines~~ | **COMPLETED** - CONTRIBUTING.md |
| No CHANGELOG | Pending |

### 10.3 Documentation Added

- [docs/DEVELOPER_SETUP.md](docs/DEVELOPER_SETUP.md) - Complete setup guide including:
  - Prerequisites (Go, Rust, mpv, aria2)
  - Repository setup
  - Build instructions
  - Development workflow
  - Environment variables
  - IDE setup
  - Common issues troubleshooting

- [CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines including:
  - Code of conduct
  - Development workflow
  - PR process
  - Code style (Go + Rust)
  - Testing requirements
  - Commit message format
  - Issue reporting template

## 10.4 Go → Rust Migration

### Stream Policy Scoring Migration

**Completed:** Stream policy scoring logic migrated from Go to Rust.

**What was migrated:**
- `tui/internal/ui/screens/stream_picker.go` - Removed duplicate scoring code (~90 lines)
- Added Rust implementation in `runtime/src/quality/user_policy.rs`
- Extended `RankingPolicy` with user preferences (`StreamPreferences`)

**Rust additions:**
- `runtime/src/quality/policy.rs` - Added `StreamPreferences` struct with:
  - `prefer_protocol`, `max_resolution`, `max_size_mb`, `min_seeders`
  - `avoid_labels`, `prefer_hdr`, `prefer_codecs`
- `runtime/src/quality/user_policy.rs` - Policy-based scoring with explanations
- `runtime/src/pipeline/rank.rs` - IPC handler for `rank_streams`
- IPC types: `RankStreamsRequest`, `RankStreamsResponse`, `RankedStreamWire`

**Go additions:**
- `tui/internal/ipc/messages.go` - `StreamPreferences`, `RankedStream`, `StreamsRankedMsg`
- `tui/internal/ipc/requests.go` - `RankStreams()` method
- `tui/internal/ui/screens/stream_picker.go` - Uses Rust via IPC

**Benefits:**
- Single source of truth for scoring logic
- Consistent behavior across TUI and runtime
- Structured explanations from Rust for better UX
- Removed ~90 lines of duplicate Go code

---

## 10.4 Plugin Resource Limits

**Completed:** Plugin resource limits implemented in `runtime/src/plugin_rpc/supervisor.rs`.

**What was implemented:**

### Configuration (`SupervisorConfig`)
- `max_memory_mb: Option<u64>` - Maximum RSS memory (existing)
- `max_cpu_percent: u32` - CPU limit via nice/renice (0 = unlimited)
- `request_timeout_ms: u64` - Timeout for individual RPC calls (default: 30s)

### Stats (`SupervisorStats`)
- `memory_mb: u64` - Current memory usage (updated by watchdog)
- `timeout_count: u32` - Incremented on RPC timeout

### CPU Limiting
- `apply_cpu_limit()` function using `renice` on Linux
- Maps percentage to nice values (0-100% → nice 0-19)
- Applied at spawn time

### Request Timeouts
- All RPC methods (`catalog_search`, `streams_resolve`, `subtitles_fetch`) wrapped with `tokio::time::timeout`
- On timeout: increments `timeout_count`, returns error with context

### Memory Monitoring
- Watchdog polls `/proc/{pid}/status` every 10 seconds
- Updates `memory_mb` in stats on each poll
- Kills process with SIGKILL if limit exceeded

**Benefits:**
- Prevents runaway plugin processes
- Provides visibility into plugin resource usage
- Enables per-plugin resource quotas

---

## 10.5 Watch History Migration (Go → Rust)

**Completed:** Watch history storage migrated from Go local file to Rust IPC-backed storage.

**What was migrated:**

### Rust additions:
- `runtime/src/watchhistory/mod.rs` - Module exports
- `runtime/src/watchhistory/store.rs` - WatchHistoryStore with:
  - `WatchHistoryEntry` struct matching Go's `Entry`
  - `WatchHistoryStore` with thread-safe Arc<RwLock>
  - CRUD operations: `upsert`, `get`, `remove`, `mark_completed`, `update_position`, `in_progress`
  - Auto-complete threshold (90%) preserved
  - Atomic file save (write to tmp, then rename)
  - Default path: `~/.config/stui/history.json`

### IPC types added:
- `GetWatchHistoryEntryRequest` / `WatchHistoryEntryResponse`
- `GetWatchHistoryInProgressRequest` / `WatchHistoryInProgressResponse`
- `UpsertWatchHistoryEntryRequest` / `WatchHistoryUpsertResponse`
- `UpdateWatchHistoryPositionRequest` / `WatchHistoryPositionUpdateResponse`
- `MarkWatchHistoryCompletedRequest`
- `RemoveWatchHistoryEntryRequest` / `WatchHistoryRemoveResponse`

### Go additions:
- `tui/pkg/watchhistory/ipc.go` - `IPCStore` implementation
- `tui/internal/ipc/requests.go` - IPC client methods for watch history
- `tui/internal/ipc/messages.go` - `WatchHistoryEntry` type

### Architecture changes:
- Created `StoreInterface` for abstraction between local and IPC stores
- `CollectionsScreen` and `continue_watching.go` updated to use interface
- `ui.go` creates IPC store on `runtimeStartedMsg`
- IPC store maintains local cache for UI responsiveness, syncs to Rust on changes

**Benefits:**
- Single source of truth for watch history in Rust
- Consistent persistence behavior across TUI and runtime
- Reduced file I/O on Go side (persistence via IPC)
- Maintains backward-compatible interface

---

## 10.6 Media Cache Migration (Go → Rust)

**Completed:** Media cache storage migrated from Go local file to Rust IPC-backed storage.

**What was migrated:**

### Rust additions:
- `runtime/src/mediacache/mod.rs` - Module exports
- `runtime/src/mediacache/store.rs` - `MediaCacheStore` with:
  - Thread-safe `Arc<RwLock<MediaCache>`
  - Operations: `save_tab`, `entries_for_tab`, `all_entries`, `total_count`, `clear`, `last_updated`
  - Auto-cache on live grid updates (source == "live")
  - Default path: `~/.config/stui/mediacache.json`

### IPC types added:
- `GetMediaCacheTabRequest` / `MediaCacheTabResponse`
- `GetMediaCacheAllRequest` / `MediaCacheAllResponse`
- `GetMediaCacheStatsRequest` / `MediaCacheStatsResponse`
- `ClearMediaCacheRequest` / `MediaCacheClearResponse`

### Go additions:
- `tui/pkg/mediacache/ipc.go` - `IPCStore` implementation
- `tui/internal/ipc/requests.go` - IPC client methods (`GetMediaCacheTab`, `GetMediaCacheAll`, `GetMediaCacheStats`, `ClearMediaCache`)

### Architecture changes:
- Created `StoreInterface` for abstraction
- `OfflineLibraryScreen` updated to use interface
- Runtime now caches live grid updates directly (no need for Go to save)
- Go IPCStore maintains local cache for UI responsiveness
- On runtime start, Go seeds IPC store from runtime's cache

**Benefits:**
- Grid updates cached in Rust immediately, no extra save in Go
- Single source of truth for media cache in Rust
- Consistent behavior across TUI and runtime
- Faster startup when runtime has cache

---

## 11. Recommendations Summary

### Priority 1 (Critical)

1. ~~**Add Go integration tests** for IPC layer~~ **COMPLETED** ✅
2. ~~**Split large Go files** (`ui.go`, `ipc.go`)~~ **COMPLETED** ✅
3. ~~**Add request timeouts** to reqwest calls~~ **COMPLETED** ✅
4. ~~**Add secret management** for API keys~~ **COMPLETED** ✅

### Priority 2 (High)

5. ~~**Add golangci-lint** with stricter rules~~ **COMPLETED** ✅
6. ~~**Add cargo-audit** to CI pipeline~~ **COMPLETED** ✅
7. ~~**Document developer setup** and contributing guidelines~~ **COMPLETED** ✅
8. ~~**Add structured logging** to Go frontend~~ **COMPLETED** ✅
9. ~~**Migrate stream policy scoring to Rust**~~ **COMPLETED** ✅

### Priority 3 (Medium)

10. **Enable stricter clippy** lints - **COMPLETED** ✅
11. **Add plugin resource limits** - **COMPLETED** ✅
12. **Migrate watch history to Rust** - **COMPLETED** ✅
13. **Migrate media cache to Rust** - **COMPLETED** ✅
14. **Add circuit breakers for providers** - **COMPLETED** ✅
15. **Create CHANGELOG for releases** - **COMPLETED** ✅

### Priority 4 (Low)

14. **Add property-based tests** for ranking - **COMPLETED** ✅
15. Add local TMDB cache
16. ~~Parallelize catalog processing~~ - **COMPLETED** ✅
17. ~~Batch/throttle IPC updates~~ - **COMPLETED** ✅

---

## 11.2 Circuit Breaker Implementation

**Completed:** Added circuit breaker module to prevent cascading failures from failing providers.

### Implementation

**New file:** `runtime/src/providers/circuit_breaker.rs`

**State Machine:**
- **Closed**: Normal operation, requests go through
- **Open**: After `failure_threshold` (default: 5) consecutive failures, requests are blocked
- **Half-Open**: After `recovery_timeout` (default: 60s), one test request is allowed

**Configuration:**
| Parameter | Default | Description |
|-----------|---------|-------------|
| `failure_threshold` | 5 | Failures before opening circuit |
| `recovery_timeout` | 60s | Time before trying half-open |
| `half_open_max` | 1 | Test requests allowed in half-open |

**Integration:**
- Added to `Pipeline` struct alongside `HealthRegistry` and `ProviderThrottle`
- Wired into `engine::ranked_streams_with_circuit_breaker()`
- Providers with open circuits are skipped during stream resolution
- Failures automatically open circuits, successes close them

**Benefits:**
- Prevents cascading failures when a provider is down
- Reduces load on failing providers (don't waste requests)
- Automatic recovery after timeout

---

**Completed:** Added `runtime/.clippy.toml` with stricter lint configuration.

**Configuration:**
- `cognitive-complexity-threshold = 30` - Limits function complexity
- `type-complexity-threshold = 100` - Limits type nesting depth
- CI uses `-D warnings` to deny all warnings

**Benefits:**
- Enforces code simplicity through complexity limits
- Consistent linting configuration across CI and local development
- Easy to tune thresholds as codebase evolves

---

## 12. Parallelized Catalog Processing

**Completed:** Refactored catalog processing to use concurrent execution.

### Changes

**File:** `runtime/src/catalog.rs`

**Improvements:**
1. **Concurrent tab initialization** - `Catalog::start()` now spawns all tab refreshes concurrently using `futures::future::join_all()` instead of sequential `for` loop

2. **Concurrent provider fan-out** - `refresh_tab()` uses `futures::future::join_all()` to execute all provider requests in parallel

3. **Concurrency limiting** - Added `Semaphore` with `MAX_CONCURRENT_PROVIDERS = 8` to prevent overwhelming external APIs

4. **JoinSet alternative** - Added `refresh_tab_with_join_set()` method demonstrating `tokio::task::JoinSet` for scenarios requiring result collection in completion order

### Key Code

```rust
// Tab initialization - all tabs in parallel
let handles: Vec<_> = tabs
    .into_iter()
    .map(|tab| {
        let catalog = Arc::clone(&self);
        tokio::spawn(async move {
            catalog.init_tab(tab).await;
        })
    })
    .collect();
future::join_all(handles).await;

// Provider fan-out with concurrency limit
let tasks: Vec<_> = self.providers
    .iter()
    .map(|provider| {
        let _permit = sem.acquire().await;
        provider.fetch_trending(&t, 1).await
    })
    .collect();
let results = future::join_all(tasks).await;
```

**Benefits:**
- Faster startup (all tabs load in parallel)
- Reduced catalog refresh latency
- Prevents API rate limiting with concurrency cap
- Back-pressure via semaphore

---

## 13. Batch/Throttle IPC Updates

**Completed:** Added `IpcBatcher` module to reduce IPC overhead for high-frequency GridUpdate events.

### Changes

**New file:** `runtime/src/ipc_batcher.rs`

**Improvements:**
1. **Update buffering** - Buffers rapid GridUpdate events
2. **Periodic flush** - Flushes every 200ms by default (configurable)
3. **Merge same-tab updates** - Multiple updates for the same tab are merged
4. **Max batch size** - Forces flush after 10 updates to prevent unbounded buffering

### Key Code

```rust
pub struct IpcBatcher {
    rx: broadcast::Receiver<GridUpdate>,
    flush_interval: Duration,
    buffer: HashMap<String, GridUpdate>,
    last_flush: Instant,
}

pub async fn recv(&mut self) -> Option<Vec<GridUpdate>> {
    loop {
        tokio::select! {
            update = self.rx.recv() => {
                // Buffer updates, flush on interval or max size
            }
            _ = sleep(self.flush_interval) => {
                // Time-based flush
            }
        }
    }
}
```

**Configuration:**
- `DEFAULT_FLUSH_INTERVAL_MS = 200` (200ms)
- `MAX_BATCH_SIZE = 10` (force flush after 10 updates)

**Benefits:**
- Reduces IPC message count during rapid refreshes
- Prevents overwhelming the Go TUI with individual updates
- Merges updates for same tab to reduce duplicate processing
- Back-pressure via buffer size limit

**Integration:**
- Integrated into `run_ipc_loop()` in `main.rs`
- Replaces direct `grid_rx.recv()` with `batcher.recv()`
- Cache and media cache operations remain unchanged

---

---

## 12. Conclusion

Stui demonstrates solid software engineering with well-separated concerns, good use of idiomatic patterns for both Go and Rust, and a thoughtful plugin architecture. The codebase is production-quality for a personal/developer tool but would benefit from the improvements outlined above before broader distribution.

**Estimated effort to address Priority 1-2 issues:** 2-3 weeks  
**Estimated effort to address all issues:** 4-6 weeks

---

*End of Audit Report*
