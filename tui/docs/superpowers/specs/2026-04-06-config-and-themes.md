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

**Built-in theme names** (reserved, no file lookup): `"default"`, `"high-contrast"`, `"monochrome"`, `"matugen"`. Any other value is treated as a filename in `~/.config/stui/themes/`.

`"matugen"` continues to use the existing `MatugenWatcher` with `theme_mode` controlling dark/light. When `theme = "matugen"`, the `MatugenWatcher` is still started alongside the config watcher.

---

## New Dependency

`github.com/BurntSushi/toml` — TOML parsing and serialisation.

---

## New Package: `pkg/config`

### `pkg/config/config.go`

```go
// Config is the full set of user preferences. All fields have zero values
// that match the application defaults, so a missing config file is valid.
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
- `Default() Config` — returns Config with all application-default values
- `DefaultPath() string` — `~/.config/stui/config.toml` via `os.UserConfigDir()`
- `Load(path string) (Config, error)` — reads TOML, merges over `Default()` so missing keys get defaults; returns `Default()` (no error) if the file does not exist
- `Save(path string, cfg Config) error` — atomic write (temp file + rename), creates parent dir

### `pkg/config/theme_loader.go`

```go
// ThemesDir returns ~/.config/stui/themes.
func ThemesDir() string

// LoadTheme resolves a theme name to a Palette.
// Built-in names ("default", "high-contrast", "monochrome") return the
// corresponding Go palette. "matugen" returns Default() as a placeholder
// (the actual palette comes from MatugenWatcher). Any other name is looked
// up as ThemesDir()/<name>.toml; missing fields fall back to Default().
func LoadTheme(name string) (theme.Palette, error)

// ListThemes returns built-in names followed by filenames (without .toml)
// found in ThemesDir(). Returns built-ins only if the directory does not exist.
func ListThemes() []string
```

### `pkg/config/watcher.go`

Wraps `fsnotify` (already a dependency). Watches both `config.toml` and the active theme file for external changes.

```go
type Watcher struct { ... }

// NewWatcher creates a watcher for cfgPath and the themes directory.
// onReload is called with the freshly loaded Config whenever an external
// write is detected. The watcher ignores events for 200ms after Save() is
// called (write-guard to prevent reload loops).
func NewWatcher(cfgPath string, onReload func(Config)) (*Watcher, error)

// Start begins watching in the background.
func (w *Watcher) Start()

// NotifyWrite tells the watcher that stui itself just wrote the file.
// Events arriving within 200ms of this call are suppressed.
func (w *Watcher) NotifyWrite()

// Stop closes the watcher.
func (w *Watcher) Stop() error
```

Debounce: 150ms (matching `MatugenWatcher`). Both `config.toml` and `themes/` directory are watched. When any `.toml` file in `themes/` changes and its name matches the active theme, a reload is triggered.

---

## Data Flow

### Startup (`cmd/stui/main.go`)

```
config.Load(config.DefaultPath())    → cfg Config
config.LoadTheme(cfg.Interface.Theme) → palette theme.Palette
theme.T.Apply(palette)
ui.New(opts, cfg)                    → Model (settings screen pre-populated)
config.NewWatcher(path, onReload)    → watcher
watcher.Start()
```

`onReload` sends a `ConfigReloadMsg{Config}` to the running bubbletea `Program` via `p.Send()`.

If `cfg.Interface.Theme == "matugen"`, start `MatugenWatcher` as today (the two watchers coexist).

### TUI Settings Change → Write-back (`internal/ui/ui.go`)

```
SettingsChangedMsg{Key, Value}
  → m.cfg = config.ApplyChange(m.cfg, key, value)
  → if theme key: theme.T.Apply(config.LoadTheme(m.cfg.Interface.Theme))
  → arm 300ms debounce timer
  → on timer fire: watcher.NotifyWrite(); config.Save(path, m.cfg)
```

`config.ApplyChange(cfg Config, key string, value interface{}) Config` maps dot-notation keys (e.g. `"playback.default_volume"`) to the correct struct field via a switch statement.

### External Write → Live Reload

```
Script writes ~/.config/stui/themes/noctalia.toml (or config.toml)
  → fsnotify event (debounced 150ms)
  → watcher checks write-guard window — not suppressed
  → config.Load() → new Config
  → onReload(newCfg) → p.Send(ConfigReloadMsg{newCfg})
  → ui.go: m.cfg = msg.Config
           theme.T.Apply(config.LoadTheme(m.cfg.Interface.Theme))
           settings screen refreshes displayed values
```

---

## Settings Screen Changes (`internal/ui/screens/settings.go`)

- `NewSettingsModel(client *ipc.Client, cfg config.Config)` — takes Config, populates item values from cfg instead of hardcoded defaults
- Interface > Theme: `settingChoice` with values from `config.ListThemes()` (built-ins + user files)
- `Update` handles `ConfigReloadMsg` by rebuilding the displayed item values from the new config

Settings items that are `settingAction` or `settingInfo` have no config file representation and are unchanged.

---

## `ui.go` Changes (`internal/ui/ui.go`)

- `Model` gains `cfg config.Config` and `cfgPath string` fields
- `ui.New(opts Options, cfg config.Config)` accepts the loaded config
- `Options` gains `CfgPath string`
- `SettingsChangedMsg` handler: apply to `m.cfg`, arm debounce, save, apply theme
- `ConfigReloadMsg` handler: update `m.cfg`, apply theme, forward to settings screen
- Debounce: a `configSaveTimer` `tea.Cmd` field; reset on each change, fire triggers save

---

## `cmd/stui/main.go` Changes

- Load config before creating `ui.New`
- Create and start `config.Watcher`; pass `p.Send` as the reload callback (set after `tea.NewProgram`)
- Pass `cfg` and `cfgPath` through `ui.Options`

---

## Files Changed

| File | Change |
|---|---|
| `pkg/config/config.go` | New — Config struct, Default, Load, Save, DefaultPath, ApplyChange |
| `pkg/config/theme_loader.go` | New — LoadTheme, ListThemes, ThemesDir |
| `pkg/config/watcher.go` | New — Watcher wrapping fsnotify |
| `pkg/config/config_test.go` | New — tests for Load, Save, ApplyChange, LoadTheme |
| `cmd/stui/main.go` | Load config, create watcher, pass to ui.New |
| `internal/ui/ui.go` | cfg field, SettingsChangedMsg write-back, ConfigReloadMsg handler, debounce |
| `internal/ui/screens/settings.go` | Accept Config in constructor, populate from cfg, handle ConfigReloadMsg, add theme choices |
| `go.mod` / `go.sum` | Add github.com/BurntSushi/toml |

---

## Out of Scope

- Migrating `keybinds.json` into `config.toml` (keybinds already have their own file and editor)
- A TUI theme editor (themes are file-edited)
- Config validation UI (invalid values are silently replaced with defaults on load)
- Shipping built-in theme files (built-ins remain Go code; only user-created themes live in `themes/`)
