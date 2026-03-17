# stui Architecture

stui is a plugin-driven terminal streaming platform built from two processes that communicate over NDJSON.

---

## System Overview

```
  ┌─────────────────────────────────────────────────────────────────┐
  │  User                                                           │
  │  keyboard · mouse                                               │
  └───────────────────────────┬─────────────────────────────────────┘
                              │ key events
                              ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │  stui  (Go · BubbleTea)                                         │
  │                                                                 │
  │   Screen stack                    pkg/                          │
  │   ├── Grid (trending/catalog)     ├── actions/   key → intent   │
  │   ├── Detail overlay              ├── keybinds/  load/save      │
  │   ├── Search                      ├── theme/     colors         │
  │   ├── Stream Picker               └── bidi/      RTL text       │
  │   ├── Episodes                                                   │
  │   ├── Settings                    ~/.config/stui/               │
  │   ├── Plugin Repos                └── keybinds.json             │
  │   └── Keybinds Editor                                           │
  └──────────────────────────┬──────────────────────────────────────┘
                             │  NDJSON  (stdin/stdout  or  Unix socket)
                             │  Protocol v1 · versioned handshake
                             │  ~/.local/run/stui.sock  (daemon mode)
  ┌──────────────────────────▼──────────────────────────────────────┐
  │  stui-runtime  (Rust · Tokio)                                   │
  │                                                                 │
  │  ┌──────────┐  ┌──────────────┐  ┌──────────────────────────┐  │
  │  │ Catalog  │  │    Engine    │  │     ConfigManager        │  │
  │  │  (grid)  │  │ (fan-out +   │  │  set(key, val)           │  │
  │  │  cache   │  │  health +    │  │  EventBus broadcasts     │  │
  │  │  TTL 3h  │  │  throttle)   │  │  → player / providers    │  │
  │  └────┬─────┘  └──────┬───────┘  └──────────────────────────┘  │
  │       │               │                                         │
  │       └───────┬────────┘                                        │
  │               │                                                 │
  │      ┌────────▼────────────────────────────┐                    │
  │      │           Providers                 │                    │
  │      │  TMDB · IMDB · OMDB · Last.fm       │                    │
  │      │  Torrentio · Prowlarr · HTTP VOD    │                    │
  │      │  Stremio addon bridge               │                    │
  │      │  RPC plugins (any language)         │                    │
  │      │  WASM plugins (sandboxed)           │                    │
  │      └────────┬────────────────────────────┘                    │
  │               │  stream candidates                              │
  │      ┌────────▼────────────────────────────┐                    │
  │      │        Quality Ranker               │                    │
  │      │  resolution · codec · HDR           │                    │
  │      │  seeders · health score · policy    │                    │
  │      └────────┬────────────────────────────┘                    │
  │               │  ranked streams                                 │
  │      ┌────────▼────────────────────────────┐                    │
  │      │        PlayerBridge                 │                    │
  │      │  video/audio routing decision       │                    │
  │      └─────────┬──────────────┬────────────┘                    │
  │                │              │                                 │
  │  ┌─────────────▼──┐    ┌──────▼───────────┐                    │
  │  │  mpv (video)   │    │  MPD (audio)     │                    │
  │  │  IPC socket    │    │  queue_and_play  │                    │
  │  └────────────────┘    └──────────────────┘                    │
  │                                                                 │
  │  aria2c  ·  Skipper (intro/credits detector)                    │
  │  ~/.stui/cache/   ~/.stui/config/stui.toml                      │
  └─────────────────────────────────────────────────────────────────┘
```

---

## Key Request Flows

```
Search
──────
  User types  →  ActionOpenSearch  →  SearchScreen
  SearchScreen.client.Search()  →  IPC {"type":"search"}
  Engine.search()  →  fan-out to providers (throttled, health-tracked)
  Response::SearchResult  →  SearchResultMsg  →  renders list

Play
────
  User selects item  →  client.Play(entryID, provider, imdbID, tab)
  pipeline::playback::run_play()
    ├── PlayerBridge.play()  →  resolve URL  →  mpv or MPD
    └── Skipper.analyze()    →  FFmpeg fingerprint  →  skip_segment events

Stream switching
────────────────
  User presses s  →  StreamPickerScreen  →  client.Resolve()
  StreamsResolvedMsg  →  sorted list (quality / seeders / size / provider)
  User selects  →  client.SwitchStream(url)  →  mpv loadfile replace

Settings change
───────────────
  SettingsScreen  →  SettingsChangedMsg  →  client.SetConfig(key, value)
  ConfigManager.set()  →  validates  →  RuntimeConfig updated
  EventBus.emit(ConfigChanged)  →  player / providers react instantly
```

---

## Detailed Component Map

```
┌──────────────────────────────────────────────────────────────┐
│  TUI  (Go / BubbleTea)                                       │
│                                                              │
│  RootModel (screen stack)                                    │
│   ├── LegacyScreen (main grid + detail overlay)              │
│   ├── SearchScreen       ← full-screen search, IPC-wired     │
│   ├── StreamPickerScreen ← sortable quality/provider picker  │
│   ├── EpisodeScreen      ← season/episode browser            │
│   ├── HelpScreen         ← keybind reference                 │
│   ├── SettingsScreen     ← live settings, 7 categories       │
│   ├── KeybindsEditor     ← capture-key rebind UI             │
│   └── PluginReposScreen  ← community repo management         │
│                                                              │
│  actions/actions.go      ← AppAction enum, mutable key map  │
│  ipc/ipc.go              ← NDJSON client, typed message set  │
│  msg/messages.go         ← BubbleTea message type aliases    │
└─────────────────────┬────────────────────────────────────────┘
                      │  NDJSON (stdin/stdout or Unix socket)
                      │  Versioned IPC: ipc/v1/mod.rs
                      │  SetConfig, PlayerCmd, Cmd, Resolve …
┌─────────────────────▼────────────────────────────────────────┐
│  Runtime  (Rust / Tokio)                                     │
│                                                              │
│  engine/                                                     │
│   pipeline.rs   ← Orchestration struct: owns everything      │
│     .health     ← HealthRegistry (reliability scoring)       │
│     .throttle   ← ProviderThrottle (token-bucket 429 guard)  │
│     .config     ← ConfigManager (live updates + EventBus)    │
│     .bus        ← EventBus (21 variants, broadcast channel)  │
│   mod.rs        ← Engine: plugin dispatch, provider fan-out  │
│     ranked_streams()            → quality::rank()            │
│     ranked_streams_with_health()→ quality::rank_with_health()│
│                                                              │
│  providers/                                                  │
│   mod.rs        ← Provider trait + capabilities() default    │
│   health.rs     ← HealthRegistry: success_rate, latency,     │
│                   reliability_score(), blend_score()         │
│   capabilities.rs ← ProviderCapabilities: declarative caps   │
│   throttle.rs   ← ProviderThrottle: exponential backoff      │
│   metadata/     ← TMDB, IMDB, OMDB                          │
│   streams/      ← direct HTTP, torrent, VOD                  │
│                                                              │
│  player/                                                     │
│   state.rs      ← PlaybackState (12 observed properties)     │
│   commands.rs   ← PlayerCommand enum (22 typed variants)     │
│   mpv.rs        ← MpvPlayer: IPC socket, TracksUpdated event │
│   manager.rs    ← handle_command(), try_next_candidate()     │
│   bridge.rs     ← URL → aria2 or mpv routing                │
│                                                              │
│  quality/                                                    │
│   mod.rs        ← rank() / rank_with_health()               │
│   score.rs      ← QualityScore: resolution/codec/seeds/HDR   │
│   policy.rs     ← RankingPolicy: weights, bandwidth_saver    │
│                                                              │
│  config/                                                     │
│   manager.rs    ← ConfigManager: set(key,val) + EventBus     │
│   types.rs      ← RuntimeConfig + 4 sub-configs             │
│   loader.rs     ← TOML + STUI_* env overrides               │
│                                                              │
│  events/                                                     │
│   event.rs      ← RuntimeEvent (21 variants)                 │
│   bus.rs        ← EventBus: emit(), subscribe(), 256-slot    │
│                                                              │
│  media/                                                      │
│   source.rs     ← MediaSource enum (10 types)               │
│   stream.rs     ← StreamCandidate (URL+quality+health fields)│
│   item.rs, episode.rs, track.rs                              │
│                                                              │
│  pipeline/                                                   │
│   search.rs    ← fan-out + catalog fallback                  │
│   resolve.rs   ← rank candidates, map to wire types         │
│   playback.rs  ← run_play (player+skipper tasks), mpd outs  │
│   config.rs    ← SetConfig, provider settings, plugin repos │
│                                                              │
│  ipc/v1/mod.rs  ← Typed wire protocol (versioned)           │
│   Ping { ipc_version } / Pong { ipc_version, runtime_version}│
│   Request:  Search, Resolve, Play, PlayerCommand, Cmd,       │
│             SetConfig, LoadPlugin, GetPluginRepos …          │
│   Response: SearchResult, PlaybackState, ConfigUpdated …     │
│                                                              │
│  error.rs       ← StuidError: is_recoverable(), user_message │
│  logging.rs     ← tracing + STUI_LOG env var                 │
│  cache/         ← 3-tier TTL cache + CachePolicy            │
│  catalog_engine/← aggregator, filters, ranking              │
│  stremio/       ← Stremio addon bridge (UPP-compatible)      │
│  plugin_rpc/    ← Language-agnostic JSON-RPC plugin system   │
└──────────────────────────────────────────────────────────────┘
         │               │              │
      aria2c           mpv          plugins
  (torrent DL)    (playback)   (Python/Go/Node/Rust)
```

---

## Data Flow

### Search

```
User types query
  → actions.ActionOpenSearch → TransitionCmd(SearchScreen)
  → SearchScreen: client.Search(query, tab)
  → IPC: {"type":"search","query":"dune",...}
  → Engine.search() → fan-out to providers (throttled, health-tracked)
  → ipc::Response::SearchResult → Go TUI
  → SearchResultMsg → SearchScreen renders results
  → User selects → SearchResultSelectedMsg → LegacyScreen.openDetail()
```

### Stream Resolution

```
User presses enter on item
  → client.Play(entryID, provider, imdbID)
  → Engine.resolve() → ranked_streams_with_health()
    → fan-out to stream providers (capabilities-filtered)
    → rank_with_health(streams, policy, health_map)
       blend = 0.75 × quality_score + 0.25 × reliability
  → best URL → PlayerBridge → mpv or aria2
  → player_started → PlayerStartedMsg → NowPlayingState → HUD
```

### Stream Switching

```
User presses s in detail overlay or during playback
  → TransitionCmd(StreamPickerScreen)
  → StreamPickerScreen.Init() → client.Resolve(entryID)
  → StreamsResolvedMsg arrives → renders quality/protocol/seeders list
  → User selects → client.SwitchStream(url)
  → IPC: {"type":"cmd","cmd":"switch_stream","url":"..."}
  → MpvPlayer.loadfile_replace(url) → seamless switch
```

### Live Config Update

```
User changes setting in SettingsScreen
  → SettingsChangedMsg{Key: "player.default_volume", Value: 80}
  → client.SetConfig("player.default_volume", 80)
  → IPC: {"type":"set_config","key":"...","value":80}
  → ConfigManager.set(key, value) → validates type
  → RuntimeConfig updated
  → EventBus.emit(ConfigChanged{key, value})
  → subscribers react (player adjusts volume, providers toggle)
  → Response: ConfigUpdated{key} → config_updated unsolicited event → StatusMsg toast
```

---

## Plugin System

Two plugin types are supported, both implementing the **Universal Provider Protocol**:

### RPC Plugins (any language)

```
plugin.json manifest → discover on startup / hot-reload
  → PluginRpcManager spawns process (stdio JSON-RPC)
  → Handshake: capabilities declaration
  → Engine routes requests based on capabilities
  → Results merged into main pipeline
```

Working example: `plugins/torrentio-rpc/plugin.py` (Python 3, stdlib only)

### WASM Plugins

```
plugin.wasm → wasmtime sandbox
  → ABI host functions exposed (HTTP, cache, IPC)
  → Compiled providers run in isolation
```

See `docs/plugins.md` for the full plugin API and `docs/upp.md` for the Universal Provider Protocol specification.

---

## Daemon Mode

```
stui-runtime daemon [--socket /path/to/stui.sock]
  → UnixListener accepts clients
  → Each client gets a full IPC session
  → Shared state: Engine, Catalog, HealthRegistry, ConfigManager
  → Benefits: persistent cache, background indexing, fast reconnect
```

---

## Key Files

| File | Purpose |
|------|---------|
| `runtime/src/main.rs` | Entry point, IPC loop, daemon mode |
| `runtime/src/engine/pipeline.rs` | Top-level orchestration struct |
| `runtime/src/engine/mod.rs` | Engine: plugin dispatch, stream fan-out |
| `runtime/src/pipeline/` | Request pipelines: search, resolve, playback, config |
| `runtime/src/providers/health.rs` | HealthRegistry: reliability scoring |
| `runtime/src/providers/throttle.rs` | ProviderThrottle: rate limiting |
| `runtime/src/config/manager.rs` | ConfigManager: live updates + EventBus |
| `runtime/src/skipper/` | Intro/credits fingerprint detection (Chromaprint) |
| `runtime/src/player/mpv.rs` | MpvPlayer: IPC socket, 12 properties |
| `runtime/src/quality/mod.rs` | rank() / rank_with_health() |
| `runtime/src/events/event.rs` | RuntimeEvent enum (21 variants) |
| `runtime/src/error.rs` | StuidError: is_recoverable(), user_message() |
| `runtime/src/ipc/v1/mod.rs` | Typed IPC wire protocol (versioned) |
| `tui/internal/ui/ui.go` | Main model, actions dispatch |
| `tui/internal/ui/actions/actions.go` | AppAction enum, mutable key→action map |
| `tui/internal/ui/screens/` | All screens: settings, keybinds editor, stream picker… |
| `tui/internal/ipc/ipc.go` | Go IPC client, message types, versioned handshake |
| `tui/pkg/keybinds/` | KeyMap struct, Load/Save keybinds.json |
| `tui/pkg/bidi/` | Bidirectional text support (auto/force/off modes) |
| `docs/runtime-ipc.md` | Full IPC protocol reference |
| `docs/upp.md` | Universal Provider Protocol spec |
| `scripts/check.sh` | Lint + format check (cargo fmt/clippy, go vet/fmt) |
