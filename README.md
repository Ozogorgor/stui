# stui

**stui** is a plugin-driven terminal streaming platform for Linux.

A keyboard-native TUI for discovering and playing movies, series, music, radio,
and podcasts — powered by a Rust async runtime and an extensible plugin system.

```
Search → Providers → Stream Candidates → Ranking → MPV
```

---

## Features

- **Netflix-style poster grid** with detail overlays, cast, similar titles
- **Universal Provider Protocol** — one interface for movies, music, radio, anime, podcasts
- **Smart stream ranking** — blends quality score × provider reliability × latency
- **Live stream switching** — switch quality mid-playback without restarting mpv
- **Full mpv integration** — subtitle delay, audio track, volume, all from the TUI
- **Plugin system** — RPC plugins in any language (Python, Go, Node, Rust)
- **Torrentio RPC plugin** — working Python plugin included, stdlib only
- **Provider health tracking** — unreliable providers are auto-penalised
- **Per-provider rate limiting** — token-bucket throttle prevents 429 errors
- **Live config updates** — change settings without restarting (`SetConfig` IPC)
- **Settings screen** — in-TUI settings panel with Playback/Streaming/Subtitles/Providers
- **Episode browser** — season/episode tree for series
- **Help screen** — full keybinding reference, always in sync
- **Daemon mode** — `stui-runtime daemon` for persistent cache and fast reconnect

---

## Quickstart

```bash
# Build everything
./scripts/build.sh

# Start aria2c (required for torrent streaming)
./scripts/aria2c-start.sh
export ARIA2_SECRET=<printed secret>

# Optional: set API keys
export TMDB_API_KEY=<key>
export OS_API_KEY=<opensubtitles key>

# Run
./dist/stui

# Or daemon mode (persistent cache, fast reconnect)
stui-runtime daemon &
stui
```

---

## Keybindings

| Key | Action |
|-----|--------|
| `/` | Search (full-screen) |
| `?` | Help / keybinding reference |
| `,` | Settings |
| `1–4` | Switch tabs (Movies/Series/Music/Library) |
| `↑↓` / `jk` | Navigate |
| `enter` | Select |
| `esc` | Back |
| **Playback** | |
| `space` | Pause/resume |
| `←/→` | Seek ±10s |
| `⇧←/⇧→` | Seek ±60s |
| `]/[` | Volume ±5 |
| `m` | Mute |
| `v` / `V` | Cycle subtitles / off |
| `z` / `Z` | Subtitle delay ±0.1s |
| `X` | Reset subtitle delay |
| `a` | Cycle audio track |
| `s` | Stream picker (switch quality) |
| `n` | Next stream candidate |
| `Q` | Stop playback |

---

## Plugin System

stui supports two plugin types:

**RPC plugins** (recommended) — any language, stdio JSON-RPC protocol:

```bash
mkdir -p ~/.stui/plugins/my-plugin
cp my-plugin.py plugin.json ~/.stui/plugins/my-plugin/
```

**WASM plugins** — compiled to WebAssembly, sandboxed execution.

A working Torrentio RPC plugin is included at `plugins/torrentio-rpc/`.

See [`docs/upp.md`](docs/upp.md) for the Universal Provider Protocol spec,
and [`docs/plugins.md`](docs/plugins.md) for the plugin API reference.

---

## Architecture

```
TUI (Go / BubbleTea)
  tui/internal/ui/
    root.go          ← Screen stack (SearchScreen, StreamPickerScreen, EpisodeScreen, HelpScreen)
    ui.go            ← Main model, IPC message handling, actions dispatch
    screens/         ← detail.go, grid.go, settings.go
    components/      ← player.go (full HUD), card.go, toast.go
    actions/         ← Typed AppAction enum, key→action map
        │
        │ NDJSON (stdin/stdout or Unix socket)
        ▼
Runtime (Rust / Tokio)
  engine/
    pipeline.rs      ← Orchestration: search → resolve → rank → play
    mod.rs           ← Engine: plugin dispatch, provider fan-out
  providers/
    mod.rs           ← Provider trait + ProviderCapabilities
    health.rs        ← HealthRegistry: reliability scoring, blend_score()
    capabilities.rs  ← ProviderCapabilities: catalog/streams/subtitles/metadata
    throttle.rs      ← ProviderThrottle: token-bucket rate limiting
  player/
    state.rs         ← PlaybackState: authoritative mpv state model
    commands.rs      ← PlayerCommand: typed control API
    mpv.rs           ← MpvPlayer: IPC socket, 12 observed properties
    manager.rs       ← PlayerManager: handle_command(), stream fallback
  config/
    manager.rs       ← ConfigManager: live updates via EventBus
    types.rs         ← RuntimeConfig + PlaybackConfig/StreamingConfig/...
  events/
    event.rs         ← RuntimeEvent enum (21 variants)
    bus.rs           ← EventBus: broadcast channel, emit/subscribe
  quality/
    mod.rs           ← rank() / rank_with_health(): quality × reliability blend
    score.rs         ← QualityScore: resolution/codec/seeders/bitrate/source/HDR
  ipc/v1/mod.rs      ← Typed IPC protocol (versioned)
  error.rs           ← StuidError: is_recoverable(), user_message()
```

---

## Development

```bash
# Run all tests
cargo test --workspace

# Watch mode
./scripts/dev.sh

# Build plugins
./scripts/build-plugins.sh

# Test torrentio plugin directly
python3 plugins/torrentio-rpc/plugin.py
```

---

## Docs

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — runtime architecture deep-dive
- [`docs/upp.md`](docs/upp.md) — Universal Provider Protocol specification
- [`docs/plugins.md`](docs/plugins.md) — Plugin API reference
- [`docs/runtime-ipc.md`](docs/runtime-ipc.md) — IPC wire protocol reference
