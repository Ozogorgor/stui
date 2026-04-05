# STUI Config File & Themes

**Date:** 2026-04-06
**Status:** Approved

---

## Overview

Add a human-editable `~/.config/stui/config.toml` that persists all user preferences, and a `~/.config/stui/themes/` directory for custom palette files. Changes made in the TUI settings screen write back to `config.toml` immediately (debounced). External writes to the config or theme files (e.g. a Noctalia template script) are detected via `fsnotify` and applied live without restart.

---

## File Layout

```
~/.config/stui/
  config.toml          ← all user preferences (created on first save)
  themes/
    <name>.toml        ← user-defined palette files
  keybinds.json        ← unchanged
  session.json         ← unchanged
```

---

## config.toml Structure

Missing keys fall back to defaults. A minimal file with only `[interface]\ntheme = "noctalia"` is valid.

```toml
[interface]
theme      = "default"   # "default" | "high-contrast" | "monochrome" | "matugen" | custom filename
theme_mode = "dark"      # "dark" | "light"  (only used when theme = "matugen")
show_borders  = true
mouse_support = false
bidi_mode     = "auto"   # "auto" | "force" | "off"

[playback]
default_volume     = 100
hwdec              = "auto"   # "auto" | "vaapi" | "nvdec" | "videotoolbox" | "no"
cache_secs         = 20
keep_open          = false
autoplay_next      = false
autoplay_countdown = 5
min_preroll_secs   = 3
demuxer_max_mb     = 200
terminal_vo        = ""       # "" | "kitty" | "sixel" | "tct" | "chafa"

[streaming]
prefer_http         = true
auto_fallback       = true
max_candidates      = 10
benchmark_streams   = false
auto_delete_video   = true
auto_delete_audio   = false

[downloads]
video_dir = "~/Videos"
music_dir = "~/Music"

[subtitles]
auto_download      = false
preferred_language = "eng"
default_delay      = 0.0

[providers]
enable_tmdb          = true
enable_omdb          = false
enable_torrentio     = true
enable_prowlarr      = false
enable_opensubtitles = false

[notifications]
enabled     = true
backend     = "auto"   # "auto" | "notify-send" | "dunstctl" | "off"
on_playback = true
on_download = true
on_streams  = false

[skipper]
enabled              = true
auto_skip_intro      = false
auto_skip_credits    = false
intro_scan_secs      = 300
min_intro_secs       = 20
max_intro_secs       = 120
similarity_threshold = 0.85
min_episodes         = 2
```

---

## Theme Files (`~/.config/stui/themes/<name>.toml`)

All fields are hex color strings. Missing fields fall back to the built-in `default` palette values. The filename (without `.toml`) is the theme name used in `config.toml`.

```toml
# Example: ~/.config/stui/themes/noctalia.toml
name = "Noctalia"   # optional display name

bg         = "#0a0a0f"
surface    = "#0f0f1a"
border     = "#1e1e2e"
border_foc = "#7c3aed"

text       = "#e2e8f0"
text_dim   = "#4a5568"
text_muted = "#718096"

accent     = "#7c3aed"
accent_alt = "#06b6d4"
neon       = "#a855f7"
green      = "#10b981"
red        = "#ef4444"
yellow     = "#f59e0b"
warn       = "#e5c07b"
success    = "#98c379"

tab_active   = "#a855f7"
tab_inactive = "#1a1a2e"
tab_text     = "#e2e8f0"
tab_text_dim = "#4a5568"
```

**Built-in theme names** (reserved, no file lookup): `"default"`, `"high-contrast"`, `"monochrome"`, `"matugen"`. Any other value is treated as a filename in `~/.config/stui/themes/`. If a user places a file named `default.toml` or another reserved name in `themes/`, it is silently skipped by `ListThemes` (reserved names always resolve to the built-in).

`"matugen"` continues to use the existing `MatugenWatcher` with `theme_mode` controlling dark/light. When `theme = "matugen"`, the `MatugenWatcher` is started alongside the config watcher; the config palette is not applied on startup (matugen owns the palette).

---

## New Dependency

`github.com/BurntSushi/toml` — TOML parsing and serialisation.

---

## New Package: `pkg/config`

### `pkg/config/config.go`

```go
// Config is the full set of user preferences.
// Always construct via Default() — never use a zero-value Config directly,
// as many defaults are non-zero (e.g. DefaultVolume = 100, PreferHTTP = true).
type Config struct {
    Interface     InterfaceConfig     `toml:"interface"`
    Playback      PlaybackConfig      `toml:"playback"`
    Streaming     StreamingConfig     `toml:"streaming"`
    Downloads     DownloadsConfig     `toml:"downloads"`
    Subtitles     SubtitlesConfig     `toml:"subtitles"`
    Providers     ProvidersConfig     `toml:"providers"`
    Notifications NotificationsConfig `toml:"notifications"`
    Skipper       SkipperConfig       `toml:"skipper"`
}
```

Sub-structs mirror the TOML sections above. Field types match the setting kinds in `settings.go` (bool, int, float64, string).

Key functions:
- `Default() Config` — returns Config with all application-default values (matches hardcoded defaults in `defaultCategories()`). **This is the only valid way to construct a Config.**
- `DefaultPath() string` — `~/.config/stui/config.toml` via `os.UserConfigDir()`
- `Load(path string) (Config, error)` — starts from `Default()`, decodes TOML over it so missing keys keep defaults; returns `Default()` (no error) if the file does not exist
- `Save(path string, cfg Config) error` — atomic write (temp file + rename), creates parent dir
- `ApplyChange(cfg Config, key string, value interface{}) Config` — maps a `SettingsChangedMsg.Key` string to the correct Config field (see key mapping table below)

**`ConfigReloadMsg`** is defined in this package:
```go
// ConfigReloadMsg is sent to the bubbletea program when the config file or
// active theme file is changed by an external process.
type ConfigReloadMsg struct {
    Config Config
}
```

### `ApplyChange` key mapping table

`SettingsChangedMsg.Key` values (from `settings.go`) → Config struct fields:

| Key string | Config field |
|---|---|
| `"interface.theme"` | `Interface.Theme` (full theme name, e.g. `"noctalia"`, `"default"`) |
| `"app.theme_mode"` | `Interface.ThemeMode` (matugen dark/light string: `"dark"` or `"light"`) |
| `"ui.show_borders"` | `Interface.ShowBorders` |
| `"ui.mouse_support"` | `Interface.MouseSupport` |
| `"ui.bidi_mode"` | `Interface.BiDiMode` |
| `"player.default_volume"` | `Playback.DefaultVolume` |
| `"player.hwdec"` | `Playback.Hwdec` |
| `"player.cache_secs"` | `Playback.CacheSecs` |
| `"player.keep_open"` | `Playback.KeepOpen` |
| `"playback.autoplay_next"` | `Playback.AutoplayNext` |
| `"playback.autoplay_countdown"` | `Playback.AutoplayCountdown` |
| `"player.min_preroll_secs"` | `Playback.MinPrerollSecs` |
| `"player.demuxer_max_mb"` | `Playback.DemuxerMaxMB` |
| `"player.terminal_vo"` | `Playback.TerminalVO` |
| `"streaming.prefer_http"` | `Streaming.PreferHTTP` |
| `"streaming.auto_fallback"` | `Streaming.AutoFallback` |
| `"streaming.max_candidates"` | `Streaming.MaxCandidates` |
| `"streaming.benchmark_streams"` | `Streaming.BenchmarkStreams` |
| `"streaming.auto_delete_video"` | `Streaming.AutoDeleteVideo` |
| `"streaming.auto_delete_audio"` | `Streaming.AutoDeleteAudio` |
| `"downloads.video_dir"` | `Downloads.VideoDir` |
| `"downloads.music_dir"` | `Downloads.MusicDir` |
| `"subtitles.auto_download"` | `Subtitles.AutoDownload` |
| `"subtitles.preferred_language"` | `Subtitles.PreferredLanguage` |
| `"subtitles.default_delay"` | `Subtitles.DefaultDelay` |
| `"providers.enable_tmdb"` | `Providers.EnableTMDB` |
| `"providers.enable_omdb"` | `Providers.EnableOMDB` |
| `"providers.enable_torrentio"` | `Providers.EnableTorrentio` |
| `"providers.enable_prowlarr"` | `Providers.EnableProwlarr` |
| `"providers.enable_opensubtitles"` | `Providers.EnableOpenSubtitles` |
| `"notifications.enabled"` | `Notifications.Enabled` |
| `"notifications.backend"` | `Notifications.Backend` |
| `"notifications.on_playback"` | `Notifications.OnPlayback` |
| `"notifications.on_download"` | `Notifications.OnDownload` |
| `"notifications.on_streams"` | `Notifications.OnStreams` |
| `"skipper.enabled"` | `Skipper.Enabled` |
| `"skipper.auto_skip_intro"` | `Skipper.AutoSkipIntro` |
| `"skipper.auto_skip_credits"` | `Skipper.AutoSkipCredits` |
| `"skipper.intro_scan_secs"` | `Skipper.IntroScanSecs` |
| `"skipper.min_intro_secs"` | `Skipper.MinIntroSecs` |
| `"skipper.max_intro_secs"` | `Skipper.MaxIntroSecs` |
| `"skipper.similarity_threshold"` | `Skipper.SimilarityThreshold` |
| `"skipper.min_episodes"` | `Skipper.MinEpisodes` |

**Note on theme keys:** The settings screen gains a new `"interface.theme"` `settingChoice` item (full theme name, e.g. `"high-contrast"`, `"noctalia"`) in addition to the existing `"app.theme_mode"` dark/light toggle (which remains for matugen mode). Both are kept in the Interface category.

`settingAction` and `settingInfo` items (`"audio.dsp"`, `"providers.open_settings"`, `"keybinds.edit"`) have no config mapping and are ignored by `ApplyChange`.

### `pkg/config/theme_loader.go`

```go
// ThemesDir returns ~/.config/stui/themes.
func ThemesDir() string

// LoadTheme resolves a theme name to a Palette.
// Built-in names: "default" → theme.Default(), "high-contrast" → theme.HighContrast(),
// "monochrome" → theme.Monochrome(), "matugen" → theme.Default() (placeholder;
// MatugenWatcher owns the actual palette at runtime).
// Any other name → load ThemesDir()/<name>.toml, decode hex fields over Default().
// Returns (Default(), error) if the file is not found or cannot be parsed.
func LoadTheme(name string) (theme.Palette, error)

// ListThemes returns the sorted list of available theme names:
// built-ins first (["default", "high-contrast", "monochrome", "matugen"]),
// followed by filenames (without .toml) found in ThemesDir(), sorted
// alphabetically. Files whose names collide with a built-in are silently
// skipped. Returns built-ins only if ThemesDir() does not exist.
func ListThemes() []string
```

### `pkg/config/watcher.go`

Watches the entire `themes/` directory (not just one file) so that any theme file update triggers a check. On an event, the watcher checks whether the changed file matches the currently active theme name; if not, the event is dropped.

```go
type Watcher struct { ... }

// NewWatcher creates a watcher for cfgPath and ThemesDir().
// onReload is called with a freshly loaded Config when an external write is
// detected. The watcher ignores events for 200ms after NotifyWrite() is
// called (write-guard to prevent reload loops on stui's own saves).
func NewWatcher(cfgPath string, onReload func(Config)) (*Watcher, error)

// Start begins watching in the background.
func (w *Watcher) Start()

// SetActiveTheme tells the watcher which theme file to watch for changes.
// Called by ui.go whenever the active theme changes (including on startup).
// "default", "high-contrast", "monochrome", and "matugen" have no file to
// watch — calling SetActiveTheme with a built-in name disables theme-file watching.
func (w *Watcher) SetActiveTheme(name string)

// NotifyWrite suppresses watcher events for 200ms after stui writes config.toml.
// The 200ms window is intentionally larger than the 150ms debounce so events
// fired by the write itself always fall within the guard window.
func (w *Watcher) NotifyWrite()

// Stop closes the watcher.
func (w *Watcher) Stop() error
```

Debounce: 150ms (matching `MatugenWatcher`). The write-guard (200ms) covers stui's own writes to `config.toml` only — theme files are never written by stui, so they have no write-guard.

---

## Data Flow

### Startup (`cmd/stui/main.go`)

```
config.Load(config.DefaultPath())         → cfg Config
config.LoadTheme(cfg.Interface.Theme)     → palette theme.Palette
theme.T.Apply(palette)                    // skipped if theme == "matugen"
ui.New(opts, cfg)                         → innerModel (settings screen pre-populated)
mainModel := ui.NewRootModel(...)
watcher := config.NewWatcher(path, func(c Config) { p.Send(ConfigReloadMsg{c}) })
watcher.SetActiveTheme(cfg.Interface.Theme)
watcher.Start()
p := tea.NewProgram(&mainModel / &splashModel)   // p now available; onReload closure captures it
```

The `onReload` callback captures `p` by pointer. Since the `Watcher.Start()` goroutine begins after `tea.NewProgram`, `p` is valid when any reload fires. Both branches (`noSplash` and default splash) must wire the watcher the same way — `watcher.Start()` is called after `tea.NewProgram` in both code paths.

If `cfg.Interface.Theme == "matugen"`, start `MatugenWatcher` as today (coexists with `config.Watcher`).

### TUI Settings Change → Write-back (`internal/ui/ui.go`)

```
SettingsChangedMsg{Key, Value}
  → m.cfg = config.ApplyChange(m.cfg, Key, Value)
  → if Key == "interface.theme": theme.T.Apply(loadTheme(m.cfg)); watcher.SetActiveTheme(...)
  → m.cfgSaveSeq++; seq := m.cfgSaveSeq
  → return tea.Tick(300ms, func(t) { return configSaveTickMsg{seq} })

case configSaveTickMsg:
  → if msg.seq != m.cfgSaveSeq { return }  // stale tick — a later change superseded this one
  → watcher.NotifyWrite()
  → config.Save(m.cfgPath, m.cfg)
```

**Debounce pattern:** `m.cfgSaveSeq int` is incremented on every change. The `tea.Tick` closure captures the seq at the time of the change. When the tick fires, it checks whether the seq still matches the current counter. If a newer change arrived within the 300ms window, the seq will have advanced and the tick is discarded. This is the standard bubbletea v2 debounce idiom — no stored `tea.Cmd` field.

### External Write → Live Reload

```
Script writes ~/.config/stui/themes/noctalia.toml (or config.toml)
  → fsnotify event (debounced 150ms)
  → watcher checks write-guard window — not suppressed
  → watcher checks active theme name matches changed file (for theme files)
  → config.Load() → new Config
  → onReload(newCfg) → p.Send(ConfigReloadMsg{newCfg})

case config.ConfigReloadMsg in ui.go:
  → m.cfg = msg.Config
  → theme.T.Apply(config.LoadTheme(m.cfg.Interface.Theme))
  → watcher.SetActiveTheme(m.cfg.Interface.Theme)
  → forward ConfigReloadMsg to settings screen
```

---

## Settings Screen Changes (`internal/ui/screens/settings.go`)

- `NewSettingsModel(client *ipc.Client, cfg config.Config)` — accepts Config; populates item values by calling `populateFromConfig(cfg)` (new private helper)
- `populateFromConfig(cfg config.Config)` — iterates all items in `m.categories` and sets `boolVal`, `intVal`, `floatVal`, `choiceIdx`, `strVal` from the corresponding Config field. Preserves `catCursor` and `itemCursor` (does not reset navigation state). Used both in `NewSettingsModel` and in `ConfigReloadMsg` handling.
- Interface > Theme item: new `settingChoice` with key `"interface.theme"`, values from `config.ListThemes()`. The `choiceIdx` is set to the index of `cfg.Interface.Theme` in the list, or 0 if not found.
- `Update` handles `config.ConfigReloadMsg` by calling `m.populateFromConfig(msg.Config)`.

---

## `ui.go` Changes (`internal/ui/ui.go`)

- `Model` gains `cfg config.Config`, `cfgPath string`, `cfgSaveSeq int`, `watcher *config.Watcher` fields
- `ui.New(opts Options, cfg config.Config)` accepts the loaded config; passes `cfg` to `NewSettingsModel`
- `Options` gains `CfgPath string`
- `SettingsChangedMsg` handler: `ApplyChange`, theme apply if theme key, increment seq, return debounce tick
- `configSaveTickMsg{seq int}` (unexported): seq-check, `NotifyWrite`, `Save`
- `config.ConfigReloadMsg` handler: update `m.cfg`, apply theme, `SetActiveTheme`, forward to settings screen

---

## `cmd/stui/main.go` Changes

- Load config before `ui.New`
- Create `config.Watcher` with `onReload` closure referencing `p` (pointer)
- Call `watcher.SetActiveTheme(cfg.Interface.Theme)`
- Call `watcher.Start()` after `tea.NewProgram` in **both** the `noSplash` and splash branches
- Pass `cfg` and `CfgPath` through `ui.Options`
- If `cfg.Interface.Theme == "matugen"`, start `MatugenWatcher` as today

---

## Files Changed

| File | Change |
|---|---|
| `pkg/config/config.go` | New — Config, sub-structs, Default, Load, Save, DefaultPath, ApplyChange, ConfigReloadMsg |
| `pkg/config/theme_loader.go` | New — LoadTheme, ListThemes, ThemesDir |
| `pkg/config/watcher.go` | New — Watcher, SetActiveTheme, NotifyWrite |
| `pkg/config/config_test.go` | New — tests for Load, Save, ApplyChange, LoadTheme, ListThemes |
| `cmd/stui/main.go` | Load config, create and start watcher (both branches), pass cfg via Options |
| `internal/ui/ui.go` | cfg/cfgPath/cfgSaveSeq/watcher fields, SettingsChangedMsg write-back, debounce, ConfigReloadMsg handler |
| `internal/ui/screens/settings.go` | Accept Config in constructor, populateFromConfig helper, ConfigReloadMsg handler, add "interface.theme" choice |
| `go.mod` / `go.sum` | Add github.com/BurntSushi/toml |

---

## Out of Scope

- Migrating `keybinds.json` into `config.toml` (keybinds already have their own file and editor)
- A TUI theme editor (themes are file-edited)
- Config validation UI (invalid values are silently replaced with defaults on load)
- Shipping built-in theme files (built-ins remain Go code; only user-created themes live in `themes/`)
