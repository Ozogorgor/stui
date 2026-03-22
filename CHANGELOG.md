# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Circuit breakers for providers** - Prevents cascading failures by temporarily disabling providers after consecutive failures
  - New `CircuitBreaker` module in `runtime/src/providers/circuit_breaker.rs`
  - State machine: Closed → Open → Half-Open → Closed
  - Configurable failure threshold (default: 5), recovery timeout (default: 60s)
  - Wired into stream resolution pipeline
- **Plugin resource limits** - CPU and memory limits for plugin processes
  - `max_cpu_percent` via nice/renice on Linux
  - `request_timeout_ms` for individual RPC calls
  - `memory_mb` tracking in watchdog
  - `timeout_count` statistics
- **Watch history migration (Go → Rust)** - Centralized playback position tracking
  - New `watchhistory` module in Rust runtime
  - IPC-backed Go client with local cache
  - Auto-complete threshold (90%) preserved
- **Media cache migration (Go → Rust)** - Centralized catalog caching
  - New `mediacache` module in Rust runtime
  - Runtime caches live grid updates directly
  - IPC methods for Go TUI access
- **Stream policy scoring migration (Go → Rust)** - Policy-based stream ranking
  - `StreamPreferences` and `UserPolicy` in Rust quality module
  - IPC handler for rank operations
  - Replaces duplicated Go scoring logic

### Changed
- **Stricter clippy linting** - Enhanced code quality enforcement
  - Added `runtime/.clippy.toml` with complexity thresholds
  - CI uses `-D warnings` to deny all warnings
  - `cognitive-complexity-threshold = 30`
  - `type-complexity-threshold = 100`

### Security
- **Secret management** - API keys loaded from `.env` files
- **HTTP request timeouts** - Added to all external HTTP calls (Jikan, AniList, MusicBrainz, WASM host)

### Documentation
- **Developer setup guide** - `docs/DEVELOPER_SETUP.md`
- **Contributing guidelines** - `CONTRIBUTING.md`

### Infrastructure
- **CI/CD improvements**
  - Added `cargo-audit` for security vulnerabilities
  - Added `golangci-lint` for Go code quality
  - Matrix testing (Rust: stable/beta/nightly, Go: 1.21/1.22/1.23)

## [0.8.0] - 2026-03-19

### Added
- Go IPC integration tests (26 tests)
- Structured logging with slog (`tui/pkg/log/`)
- Large file refactor (IPC split into 5 modules)

### Changed
- Split `ipc.go` into `messages.go`, `requests.go`, `events.go`, `client.go`, `types.go`

---

## Version History

| Version | Date | Status |
|---------|------|--------|
| Unreleased | 2026-03-19 | Current |
| 0.8.0 | 2026-03-19 | Previous |
