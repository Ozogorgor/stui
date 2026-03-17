# stui

**stui** is a plugin-driven terminal streaming platform for Linux.

A fast, keyboard-first TUI for discovering and playing movies, series, music, radio, and podcasts — powered by a Rust async runtime, intelligent stream selection, and a fully extensible plugin system.

```
Search → Providers → Streams → Rank → Play
```

---

## Status

stui is currently in **active development**.

* Core streaming, playback, and plugin system are implemented
* Most high-level features are functional
* Focus is shifting toward stability, performance, and reliability

⚠️ Expect rough edges, incomplete providers, and occasional breakage.

---

## Why stui?

stui is not just a TUI frontend — it is a **universal media runtime**.

* Decouples discovery, resolution, and playback via plugins
* Automatically ranks and selects the best stream across providers
* Tracks provider reliability and adapts over time
* Fully keyboard-driven — no mouse required
* Designed for power users who live in the terminal

Think:

> mpv + plugin ecosystem + streaming intelligence

---

## Features

### Core

* **Netflix-style poster grid** with detail overlays, cast, and similar titles
* **Episode browser** — season/episode tree for series
* **Collections & history** — resume playback and track progress
* **Universal Provider Protocol (UPP)** — one interface for all media types

### Playback

* **Full mpv integration**

  * subtitle delay
  * audio track switching
  * volume control
  * playback control from TUI
* **Live stream switching** — change quality without restarting playback
* **Autoplay / binge mode**
* **Smart stream ranking**

  * quality × latency × provider reliability

### Plugins

* **RPC plugins (any language)** — Python, Go, Node, Rust
* **WASM plugins** — sandboxed execution
* **Provider health tracking**
* **Per-provider rate limiting (token bucket)**

### System

* **Live config updates** (no restart required)
* **Settings screen** (Playback / Streaming / Subtitles / Providers)
* **Daemon mode** for persistent cache and fast startup
* **Typed IPC protocol (v1)**
* **Event-driven runtime (Tokio + EventBus)**

---

## Requirements

* Linux (Wayland or X11)
* `mpv` (required)
* `mpd` (required)
* `aria2c` (required for torrent streaming)
* `python3` (for some plugins)

Optional:

* TMDB API key (metadata)
* OpenSubtitles API key

---

## Quickstart

```bash
# Build everything
./scripts/build.sh

# Start aria2c (required for torrent streaming)
./scripts/aria2c-start.sh
export ARIA2_SECRET=<printed secret>

# Optional: API keys
export TMDB_API_KEY=<key>
export OS_API_KEY=<opensubtitles key>

# Run
./dist/stui

# Or daemon mode (faster startup, persistent cache)
stui-runtime daemon &
stui
```

### First Run

On first launch:

* plugins are loaded
* cache is initialized
* first search may be slower than usual

---

## Keybindings

| Key         | Action      |
| ----------- | ----------- |
| `/`         | Search      |
| `?`         | Help        |
| `,`         | Settings    |
| `1–4`       | Switch tabs |
| `↑↓` / `jk` | Navigate    |
| `enter`     | Select      |
| `esc`       | Back        |

### Playback

| Key       | Action                |
| --------- | --------------------- |
| `space`   | Pause / resume        |
| `←/→`     | Seek ±10s             |
| `⇧←/⇧→`   | Seek ±60s             |
| `]/[`     | Volume ±5             |
| `m`       | Mute                  |
| `v` / `V` | Cycle subtitles / off |
| `z` / `Z` | Subtitle delay ±0.1s  |
| `X`       | Reset subtitle delay  |
| `a`       | Cycle audio track     |
| `s`       | Stream picker         |
| `n`       | Next stream candidate |
| `Q`       | Stop playback         |

---

## Plugins

Plugins power everything in stui.

They are responsible for:

* searching content
* providing streams
* fetching subtitles
* enriching metadata

stui itself does **not** fetch media — plugins do.

### Types

**RPC plugins (recommended)**
Any language using JSON-RPC over stdio.

```bash
mkdir -p ~/.stui/plugins/my-plugin
cp my-plugin.py plugin.json ~/.stui/plugins/my-plugin/
```

**WASM plugins**
Compiled to WebAssembly for sandboxed execution.

---

## Architecture

```
TUI (Go / BubbleTea)
  ↓
IPC (NDJSON / Unix socket)
  ↓
Runtime (Rust / Tokio)
  ├── Engine (pipeline orchestration)
  ├── Providers (plugin interface + health + throttling)
  ├── Player (mpv integration)
  ├── Config (live updates)
  ├── Events (EventBus)
  ├── Quality (stream ranking)
```

---

## Configuration

Configuration is managed via the Settings screen.

* Stored at: `~/.config/stui/config.toml`
* Updated live via IPC (`SetConfig`)
* No restart required

---

## Debugging

Run with debug logs:

```bash
RUST_LOG=debug stui
```

Common issues:

* No streams → provider issue
* Playback fails → mpv / network / resolver issue
* Missing metadata → API keys not set

---

## Development

```bash
# Run tests
cargo test --workspace

# Dev mode
./scripts/dev.sh

# Build plugins
./scripts/build-plugins.sh

# Test plugin directly
python3 plugins/torrentio-rpc/plugin.py
```

---

## Roadmap

* Improved provider ecosystem
* Better stream reliability heuristics
* Subtitle auto-sync
* Remote control / second-screen support
* Plugin registry / discovery system

---

## Disclaimer

stui does not host, store, or distribute any media.

All content is provided by third-party plugins.
Users are responsible for complying with local laws and regulations.

The core project only provides:

* a runtime
* a plugin system
* a playback interface

---

## Logo

Terminal-first design (Tyrian purple 👀) — coming soon.
