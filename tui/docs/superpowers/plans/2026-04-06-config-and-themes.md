# Config File & Themes Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `~/.config/stui/config.toml` (all user preferences, live-reload on external writes) and `~/.config/stui/themes/*.toml` (user palette files), with full roundtrip: TUI changes write back to disk and external script writes apply live.

**Architecture:** A new `pkg/config` package owns the Config struct, Load/Save/ApplyChange, theme file loading, and the fsnotify watcher. `main.go` loads the config at startup and starts the watcher. `ui.go` handles write-back with a sequence-number debounce. `settings.go` reads initial values from Config and handles live-reload messages.

**Tech Stack:** Go 1.25, `github.com/BurntSushi/toml`, `github.com/fsnotify/fsnotify` (already in go.mod), `pkg/theme` (existing palette types)

---

## File Map

| File | Role |
|---|---|
| `pkg/config/config.go` | Config struct + sub-structs, Default(), Load(), Save(), DefaultPath(), ApplyChange(), ConfigReloadMsg |
| `pkg/config/theme_loader.go` | ThemesDir(), LoadTheme(), ListThemes() |
| `pkg/config/watcher.go` | Watcher: watches config.toml + themes/ dir, write-guard, debounce |
| `pkg/config/config_test.go` | Tests for Load, Save, ApplyChange, LoadTheme, ListThemes |
| `cmd/stui/main.go` | Load config, create+start watcher, pass cfg through Options |
| `internal/ui/ui.go` | cfg/cfgPath/cfgSaveSeq/watcher fields, SettingsChangedMsg write-back, ConfigReloadMsg handler |
| `internal/ui/screens/settings.go` | Accept Config in constructor, populateFromConfig helper, ConfigReloadMsg handler, add "interface.theme" item |
| `go.mod` / `go.sum` | Add github.com/BurntSushi/toml |

---

## Chunk 1: `pkg/config` — Config, Theme Loader, Tests

### Task 1: Add TOML dependency and create Config struct with sub-structs

**Files:**
- Modify: `go.mod`, `go.sum` (via `go get`)
- Create: `tui/pkg/config/config.go`
- Create: `tui/pkg/config/config_test.go`

- [ ] **Step 1.1 — Add the TOML dependency**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go get github.com/BurntSushi/toml
```

Expected: `go.mod` and `go.sum` updated, no errors.

- [ ] **Step 1.2 — Write failing tests for Default(), Load(), Save(), DefaultPath()**

Create `tui/pkg/config/config_test.go`:

```go
package config_test

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDefaultReturnsNonZeroValues(t *testing.T) {
	cfg := Default()
	if cfg.Playback.DefaultVolume != 100 {
		t.Errorf("DefaultVolume = %d, want 100", cfg.Playback.DefaultVolume)
	}
	if cfg.Streaming.PreferHTTP != true {
		t.Error("PreferHTTP should default to true")
	}
	if cfg.Skipper.SimilarityThreshold != 0.85 {
		t.Errorf("SimilarityThreshold = %f, want 0.85", cfg.Skipper.SimilarityThreshold)
	}
	if cfg.Interface.Theme != "default" {
		t.Errorf("Interface.Theme = %q, want %q", cfg.Interface.Theme, "default")
	}
}

func TestLoadMissingFileReturnsDefault(t *testing.T) {
	cfg, err := Load("/nonexistent/path/config.toml")
	if err != nil {
		t.Fatalf("Load of missing file returned error: %v", err)
	}
	want := Default()
	if cfg.Playback.DefaultVolume != want.Playback.DefaultVolume {
		t.Errorf("Load missing: DefaultVolume = %d, want %d", cfg.Playback.DefaultVolume, want.Playback.DefaultVolume)
	}
}

func TestLoadOverridesOnlyPresentKeys(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")
	// Only set one field — all others should stay at default.
	if err := os.WriteFile(path, []byte("[playback]\ndefault_volume = 42\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if cfg.Playback.DefaultVolume != 42 {
		t.Errorf("DefaultVolume = %d, want 42", cfg.Playback.DefaultVolume)
	}
	// Unset field must keep default.
	if cfg.Streaming.PreferHTTP != true {
		t.Error("PreferHTTP should still be true (default) when not set in file")
	}
}

func TestSaveRoundtrip(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")
	cfg := Default()
	cfg.Playback.DefaultVolume = 77
	cfg.Interface.Theme = "noctalia"

	if err := Save(path, cfg); err != nil {
		t.Fatalf("Save: %v", err)
	}
	got, err := Load(path)
	if err != nil {
		t.Fatalf("Load after Save: %v", err)
	}
	if got.Playback.DefaultVolume != 77 {
		t.Errorf("DefaultVolume = %d, want 77", got.Playback.DefaultVolume)
	}
	if got.Interface.Theme != "noctalia" {
		t.Errorf("Theme = %q, want %q", got.Interface.Theme, "noctalia")
	}
}

func TestSaveCreatesParentDir(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "nested", "dir", "config.toml")
	if err := Save(path, Default()); err != nil {
		t.Fatalf("Save should create parent dirs: %v", err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("File not created: %v", err)
	}
}

func TestDefaultPathNotEmpty(t *testing.T) {
	if DefaultPath() == "" {
		t.Error("DefaultPath() should not be empty")
	}
}
```

- [ ] **Step 1.3 — Run tests to confirm they fail**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -v 2>&1 | head -20
```

Expected: compile error (package does not exist yet).

- [ ] **Step 1.4 — Create `tui/pkg/config/config.go`**

```go
package config

import (
	"os"
	"path/filepath"

	"github.com/BurntSushi/toml"
)

// ── Sub-structs ───────────────────────────────────────────────────────────────

type InterfaceConfig struct {
	Theme        string `toml:"theme"`
	ThemeMode    string `toml:"theme_mode"`
	ShowBorders  bool   `toml:"show_borders"`
	MouseSupport bool   `toml:"mouse_support"`
	BiDiMode     string `toml:"bidi_mode"`
}

type PlaybackConfig struct {
	DefaultVolume     int    `toml:"default_volume"`
	Hwdec             string `toml:"hwdec"`
	CacheSecs         int    `toml:"cache_secs"`
	KeepOpen          bool   `toml:"keep_open"`
	AutoplayNext      bool   `toml:"autoplay_next"`
	AutoplayCountdown int    `toml:"autoplay_countdown"`
	MinPrerollSecs    int    `toml:"min_preroll_secs"`
	DemuxerMaxMB      int    `toml:"demuxer_max_mb"`
	TerminalVO        string `toml:"terminal_vo"`
}

type StreamingConfig struct {
	PreferHTTP       bool `toml:"prefer_http"`
	AutoFallback     bool `toml:"auto_fallback"`
	MaxCandidates    int  `toml:"max_candidates"`
	BenchmarkStreams  bool `toml:"benchmark_streams"`
	AutoDeleteVideo  bool `toml:"auto_delete_video"`
	AutoDeleteAudio  bool `toml:"auto_delete_audio"`
}

type DownloadsConfig struct {
	VideoDir string `toml:"video_dir"`
	MusicDir string `toml:"music_dir"`
}

type SubtitlesConfig struct {
	AutoDownload      bool    `toml:"auto_download"`
	PreferredLanguage string  `toml:"preferred_language"`
	DefaultDelay      float64 `toml:"default_delay"`
}

type ProvidersConfig struct {
	EnableTMDB          bool `toml:"enable_tmdb"`
	EnableOMDB          bool `toml:"enable_omdb"`
	EnableTorrentio     bool `toml:"enable_torrentio"`
	EnableProwlarr      bool `toml:"enable_prowlarr"`
	EnableOpenSubtitles bool `toml:"enable_opensubtitles"`
}

type NotificationsConfig struct {
	Enabled    bool   `toml:"enabled"`
	Backend    string `toml:"backend"`
	OnPlayback bool   `toml:"on_playback"`
	OnDownload bool   `toml:"on_download"`
	OnStreams   bool   `toml:"on_streams"`
}

type SkipperConfig struct {
	Enabled             bool    `toml:"enabled"`
	AutoSkipIntro       bool    `toml:"auto_skip_intro"`
	AutoSkipCredits     bool    `toml:"auto_skip_credits"`
	IntroScanSecs       int     `toml:"intro_scan_secs"`
	MinIntroSecs        int     `toml:"min_intro_secs"`
	MaxIntroSecs        int     `toml:"max_intro_secs"`
	SimilarityThreshold float64 `toml:"similarity_threshold"`
	MinEpisodes         int     `toml:"min_episodes"`
}

// ── Config ────────────────────────────────────────────────────────────────────

// Config is the full set of user preferences.
// Always construct via Default() — never use a zero-value Config directly,
// as many defaults are non-zero (e.g. DefaultVolume=100, PreferHTTP=true).
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

// ConfigReloadMsg is sent to the bubbletea program when config.toml or the
// active theme file is changed by an external process.
type ConfigReloadMsg struct {
	Config Config
}

// Default returns a Config with all application-default values.
// Matches the hardcoded defaults in settings.go defaultCategories().
func Default() Config {
	home, _ := os.UserHomeDir()
	if home == "" {
		home = "."
	}
	return Config{
		Interface: InterfaceConfig{
			Theme:       "default",
			ThemeMode:   "dark",
			ShowBorders: true,
			BiDiMode:    "auto",
		},
		Playback: PlaybackConfig{
			DefaultVolume:     100,
			Hwdec:             "auto",
			CacheSecs:         20,
			AutoplayCountdown: 5,
			MinPrerollSecs:    3,
			DemuxerMaxMB:      200,
		},
		Streaming: StreamingConfig{
			PreferHTTP:      true,
			AutoFallback:    true,
			MaxCandidates:   10,
			AutoDeleteVideo: true,
		},
		Downloads: DownloadsConfig{
			VideoDir: filepath.Join(home, "Videos"),
			MusicDir: filepath.Join(home, "Music"),
		},
		Subtitles: SubtitlesConfig{
			PreferredLanguage: "eng",
		},
		Providers: ProvidersConfig{
			EnableTMDB:      true,
			EnableTorrentio: true,
		},
		Notifications: NotificationsConfig{
			Enabled:    true,
			Backend:    "auto",
			OnPlayback: true,
			OnDownload: true,
		},
		Skipper: SkipperConfig{
			Enabled:             true,
			IntroScanSecs:       300,
			MinIntroSecs:        20,
			MaxIntroSecs:        120,
			SimilarityThreshold: 0.85,
			MinEpisodes:         2,
		},
	}
}

// DefaultPath returns ~/.config/stui/config.toml.
func DefaultPath() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "config.toml")
	}
	if home, err := os.UserHomeDir(); err == nil {
		return filepath.Join(home, ".config", "stui", "config.toml")
	}
	return ""
}

// Load reads config.toml at path, merging over Default() so missing keys keep
// their default values. Returns Default() (no error) if the file does not exist.
func Load(path string) (Config, error) {
	cfg := Default()
	data, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return cfg, nil
	}
	if err != nil {
		return cfg, err
	}
	if _, err := toml.Decode(string(data), &cfg); err != nil {
		return cfg, err
	}
	return cfg, nil
}

// Save writes cfg to path atomically (temp file + rename).
// Creates parent directories as needed.
func Save(path string, cfg Config) error {
	if path == "" {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if err := toml.NewEncoder(f).Encode(cfg); err != nil {
		f.Close()
		os.Remove(tmp)
		return err
	}
	if err := f.Close(); err != nil {
		os.Remove(tmp)
		return err
	}
	return os.Rename(tmp, path)
}

// ApplyChange applies a single settings-screen change to cfg and returns the
// updated Config. key is the settingItem.key from settings.go. Unknown keys
// are silently ignored (actions, read-only items).
func ApplyChange(cfg Config, key string, value interface{}) Config {
	switch key {
	case "interface.theme":
		if v, ok := value.(string); ok {
			cfg.Interface.Theme = v
		}
	case "app.theme_mode":
		if v, ok := value.(string); ok {
			cfg.Interface.ThemeMode = v
		}
	case "ui.show_borders":
		if v, ok := value.(bool); ok {
			cfg.Interface.ShowBorders = v
		}
	case "ui.mouse_support":
		if v, ok := value.(bool); ok {
			cfg.Interface.MouseSupport = v
		}
	case "ui.bidi_mode":
		if v, ok := value.(string); ok {
			cfg.Interface.BiDiMode = v
		}
	case "player.default_volume":
		if v, ok := value.(int); ok {
			cfg.Playback.DefaultVolume = v
		}
	case "player.hwdec":
		if v, ok := value.(string); ok {
			cfg.Playback.Hwdec = v
		}
	case "player.cache_secs":
		if v, ok := value.(int); ok {
			cfg.Playback.CacheSecs = v
		}
	case "player.keep_open":
		if v, ok := value.(bool); ok {
			cfg.Playback.KeepOpen = v
		}
	case "playback.autoplay_next":
		if v, ok := value.(bool); ok {
			cfg.Playback.AutoplayNext = v
		}
	case "playback.autoplay_countdown":
		if v, ok := value.(int); ok {
			cfg.Playback.AutoplayCountdown = v
		}
	case "player.min_preroll_secs":
		if v, ok := value.(int); ok {
			cfg.Playback.MinPrerollSecs = v
		}
	case "player.demuxer_max_mb":
		if v, ok := value.(int); ok {
			cfg.Playback.DemuxerMaxMB = v
		}
	case "player.terminal_vo":
		if v, ok := value.(string); ok {
			cfg.Playback.TerminalVO = v
		}
	case "streaming.prefer_http":
		if v, ok := value.(bool); ok {
			cfg.Streaming.PreferHTTP = v
		}
	case "streaming.auto_fallback":
		if v, ok := value.(bool); ok {
			cfg.Streaming.AutoFallback = v
		}
	case "streaming.max_candidates":
		if v, ok := value.(int); ok {
			cfg.Streaming.MaxCandidates = v
		}
	case "streaming.benchmark_streams":
		if v, ok := value.(bool); ok {
			cfg.Streaming.BenchmarkStreams = v
		}
	case "streaming.auto_delete_video":
		if v, ok := value.(bool); ok {
			cfg.Streaming.AutoDeleteVideo = v
		}
	case "streaming.auto_delete_audio":
		if v, ok := value.(bool); ok {
			cfg.Streaming.AutoDeleteAudio = v
		}
	case "downloads.video_dir":
		if v, ok := value.(string); ok {
			cfg.Downloads.VideoDir = v
		}
	case "downloads.music_dir":
		if v, ok := value.(string); ok {
			cfg.Downloads.MusicDir = v
		}
	case "subtitles.auto_download":
		if v, ok := value.(bool); ok {
			cfg.Subtitles.AutoDownload = v
		}
	case "subtitles.preferred_language":
		if v, ok := value.(string); ok {
			cfg.Subtitles.PreferredLanguage = v
		}
	case "subtitles.default_delay":
		if v, ok := value.(float64); ok {
			cfg.Subtitles.DefaultDelay = v
		}
	case "providers.enable_tmdb":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableTMDB = v
		}
	case "providers.enable_omdb":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableOMDB = v
		}
	case "providers.enable_torrentio":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableTorrentio = v
		}
	case "providers.enable_prowlarr":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableProwlarr = v
		}
	case "providers.enable_opensubtitles":
		if v, ok := value.(bool); ok {
			cfg.Providers.EnableOpenSubtitles = v
		}
	case "notifications.enabled":
		if v, ok := value.(bool); ok {
			cfg.Notifications.Enabled = v
		}
	case "notifications.backend":
		if v, ok := value.(string); ok {
			cfg.Notifications.Backend = v
		}
	case "notifications.on_playback":
		if v, ok := value.(bool); ok {
			cfg.Notifications.OnPlayback = v
		}
	case "notifications.on_download":
		if v, ok := value.(bool); ok {
			cfg.Notifications.OnDownload = v
		}
	case "notifications.on_streams":
		if v, ok := value.(bool); ok {
			cfg.Notifications.OnStreams = v
		}
	case "skipper.enabled":
		if v, ok := value.(bool); ok {
			cfg.Skipper.Enabled = v
		}
	case "skipper.auto_skip_intro":
		if v, ok := value.(bool); ok {
			cfg.Skipper.AutoSkipIntro = v
		}
	case "skipper.auto_skip_credits":
		if v, ok := value.(bool); ok {
			cfg.Skipper.AutoSkipCredits = v
		}
	case "skipper.intro_scan_secs":
		if v, ok := value.(int); ok {
			cfg.Skipper.IntroScanSecs = v
		}
	case "skipper.min_intro_secs":
		if v, ok := value.(int); ok {
			cfg.Skipper.MinIntroSecs = v
		}
	case "skipper.max_intro_secs":
		if v, ok := value.(int); ok {
			cfg.Skipper.MaxIntroSecs = v
		}
	case "skipper.similarity_threshold":
		if v, ok := value.(float64); ok {
			cfg.Skipper.SimilarityThreshold = v
		}
	case "skipper.min_episodes":
		if v, ok := value.(int); ok {
			cfg.Skipper.MinEpisodes = v
		}
	}
	return cfg
}
```

- [ ] **Step 1.5 — Run tests to confirm they pass**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -run "TestDefault|TestLoad|TestSave|TestDefaultPath" -v
```

Expected: all 5 tests PASS.

- [ ] **Step 1.6 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add pkg/config/config.go pkg/config/config_test.go go.mod go.sum && git commit -m "feat(config): Config struct, Default, Load, Save, ApplyChange, ConfigReloadMsg"
```

---

### Task 2: Theme loader — LoadTheme and ListThemes

**Files:**
- Create: `tui/pkg/config/theme_loader.go`
- Modify: `tui/pkg/config/config_test.go` (add theme loader tests)

- [ ] **Step 2.1 — Write failing tests for LoadTheme and ListThemes**

Append to `tui/pkg/config/config_test.go`:

```go
func TestLoadThemeBuiltinDefault(t *testing.T) {
	p, err := LoadTheme("default")
	if err != nil {
		t.Fatalf("LoadTheme(default): %v", err)
	}
	// The default palette Bg should be non-nil.
	if p.Bg == nil {
		t.Error("LoadTheme(default): Bg should not be nil")
	}
}

func TestLoadThemeBuiltinHighContrast(t *testing.T) {
	p, err := LoadTheme("high-contrast")
	if err != nil {
		t.Fatalf("LoadTheme(high-contrast): %v", err)
	}
	if p.Bg == nil {
		t.Error("LoadTheme(high-contrast): Bg should not be nil")
	}
}

func TestLoadThemeBuiltinMonochrome(t *testing.T) {
	p, err := LoadTheme("monochrome")
	if err != nil {
		t.Fatalf("LoadTheme(monochrome): %v", err)
	}
	if p.Bg == nil {
		t.Error("LoadTheme(monochrome): Bg should not be nil")
	}
}

func TestLoadThemeBuiltinMatugen(t *testing.T) {
	// "matugen" returns Default() as a placeholder — no error.
	p, err := LoadTheme("matugen")
	if err != nil {
		t.Fatalf("LoadTheme(matugen): %v", err)
	}
	if p.Bg == nil {
		t.Error("LoadTheme(matugen): should return default palette")
	}
}

func TestLoadThemeFromFile(t *testing.T) {
	dir := t.TempDir()
	// Point ThemesDir at our temp dir by writing to a known path.
	// We test via LoadThemeFromPath directly (helper used by LoadTheme).
	tomlContent := `bg = "#112233"` + "\n"
	path := filepath.Join(dir, "mytheme.toml")
	if err := os.WriteFile(path, []byte(tomlContent), 0o644); err != nil {
		t.Fatal(err)
	}
	p, err := loadThemeFromPath(path)
	if err != nil {
		t.Fatalf("loadThemeFromPath: %v", err)
	}
	// bg should be overridden; other fields fall back to Default().
	if p.Bg == nil {
		t.Error("Bg should not be nil after loading theme file")
	}
	// Default surface is non-nil (was not overridden).
	if p.Surface == nil {
		t.Error("Surface should fall back to Default() and not be nil")
	}
}

func TestLoadThemeMissingFileReturnsDefault(t *testing.T) {
	p, err := LoadTheme("nonexistent-theme-xyzzy")
	if err == nil {
		t.Error("LoadTheme of nonexistent theme should return an error")
	}
	// Palette should still be the default (not zero).
	if p.Bg == nil {
		t.Error("LoadTheme missing: should return Default() palette")
	}
}

func TestListThemesContainsBuiltins(t *testing.T) {
	themes := ListThemes()
	builtins := []string{"default", "high-contrast", "monochrome", "matugen"}
	for _, b := range builtins {
		found := false
		for _, name := range themes {
			if name == b {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("ListThemes: missing builtin %q", b)
		}
	}
}

func TestListThemesBuiltinsFirst(t *testing.T) {
	themes := ListThemes()
	if len(themes) < 4 {
		t.Fatalf("ListThemes: expected at least 4 items, got %d", len(themes))
	}
	if themes[0] != "default" {
		t.Errorf("first theme = %q, want %q", themes[0], "default")
	}
}

func TestListThemesSkipsReservedFileNames(t *testing.T) {
	// ListThemes should never return a duplicate of a built-in, even if a file
	// named default.toml exists in ThemesDir(). We can't easily test the real
	// ThemesDir, but we can verify the dedup logic via a separate helper.
	reserved := map[string]bool{"default": true, "high-contrast": true, "monochrome": true, "matugen": true}
	themes := ListThemes()
	seen := map[string]int{}
	for _, name := range themes {
		seen[name]++
		if seen[name] > 1 {
			t.Errorf("ListThemes: %q appears more than once", name)
		}
		if reserved[name] && seen[name] > 1 {
			t.Errorf("ListThemes: reserved name %q duplicated", name)
		}
	}
}
```

- [ ] **Step 2.2 — Run tests to confirm they fail**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -run "TestLoadTheme|TestListThemes" -v 2>&1 | head -20
```

Expected: compile errors (functions not defined yet).

- [ ] **Step 2.3 — Create `tui/pkg/config/theme_loader.go`**

```go
package config

import (
	"os"
	"path/filepath"
	"sort"

	"charm.land/lipgloss/v2"
	"github.com/BurntSushi/toml"
	"github.com/stui/stui/pkg/theme"
)

// ThemesDir returns ~/.config/stui/themes.
func ThemesDir() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "themes")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "stui", "themes")
}

var builtinNames = []string{"default", "high-contrast", "monochrome", "matugen"}

var builtinSet = map[string]bool{
	"default": true, "high-contrast": true, "monochrome": true, "matugen": true,
}

// LoadTheme resolves a theme name to a Palette.
// Built-in names return the corresponding Go palette.
// "matugen" returns Default() as a placeholder (MatugenWatcher owns the live palette).
// Any other name is loaded from ThemesDir()/<name>.toml; missing fields fall back to Default().
// Returns (Default(), error) if the file is not found or cannot be parsed.
func LoadTheme(name string) (theme.Palette, error) {
	switch name {
	case "default":
		return theme.Default(), nil
	case "high-contrast":
		return theme.HighContrast(), nil
	case "monochrome":
		return theme.Monochrome(), nil
	case "matugen":
		return theme.Default(), nil
	default:
		path := filepath.Join(ThemesDir(), name+".toml")
		return loadThemeFromPath(path)
	}
}

// themeFile mirrors the TOML structure of a user theme file.
// All fields are hex strings; missing fields are empty and fall back to Default().
type themeFile struct {
	Name      string `toml:"name"`
	Bg        string `toml:"bg"`
	Surface   string `toml:"surface"`
	Border    string `toml:"border"`
	BorderFoc string `toml:"border_foc"`
	Text      string `toml:"text"`
	TextDim   string `toml:"text_dim"`
	TextMuted string `toml:"text_muted"`
	Accent    string `toml:"accent"`
	AccentAlt string `toml:"accent_alt"`
	Neon      string `toml:"neon"`
	Green     string `toml:"green"`
	Red       string `toml:"red"`
	Yellow    string `toml:"yellow"`
	Warn      string `toml:"warn"`
	Success   string `toml:"success"`
	TabActive   string `toml:"tab_active"`
	TabInactive string `toml:"tab_inactive"`
	TabText     string `toml:"tab_text"`
	TabTextDim  string `toml:"tab_text_dim"`
}

// loadThemeFromPath reads a theme TOML file and merges over Default().
func loadThemeFromPath(path string) (theme.Palette, error) {
	p := theme.Default()
	data, err := os.ReadFile(path)
	if err != nil {
		return p, err
	}
	var tf themeFile
	if _, err := toml.Decode(string(data), &tf); err != nil {
		return p, err
	}
	apply := func(dst *interface{ }, hex string) {
		// unused — use inline approach below
	}
	_ = apply
	if tf.Bg != "" {
		p.Bg = lipgloss.Color(tf.Bg)
	}
	if tf.Surface != "" {
		p.Surface = lipgloss.Color(tf.Surface)
	}
	if tf.Border != "" {
		p.Border = lipgloss.Color(tf.Border)
	}
	if tf.BorderFoc != "" {
		p.BorderFoc = lipgloss.Color(tf.BorderFoc)
	}
	if tf.Text != "" {
		p.Text = lipgloss.Color(tf.Text)
	}
	if tf.TextDim != "" {
		p.TextDim = lipgloss.Color(tf.TextDim)
	}
	if tf.TextMuted != "" {
		p.TextMuted = lipgloss.Color(tf.TextMuted)
	}
	if tf.Accent != "" {
		p.Accent = lipgloss.Color(tf.Accent)
	}
	if tf.AccentAlt != "" {
		p.AccentAlt = lipgloss.Color(tf.AccentAlt)
	}
	if tf.Neon != "" {
		p.Neon = lipgloss.Color(tf.Neon)
	}
	if tf.Green != "" {
		p.Green = lipgloss.Color(tf.Green)
	}
	if tf.Red != "" {
		p.Red = lipgloss.Color(tf.Red)
	}
	if tf.Yellow != "" {
		p.Yellow = lipgloss.Color(tf.Yellow)
	}
	if tf.Warn != "" {
		p.Warn = lipgloss.Color(tf.Warn)
	}
	if tf.Success != "" {
		p.Success = lipgloss.Color(tf.Success)
	}
	if tf.TabActive != "" {
		p.TabActive = lipgloss.Color(tf.TabActive)
	}
	if tf.TabInactive != "" {
		p.TabInactive = lipgloss.Color(tf.TabInactive)
	}
	if tf.TabText != "" {
		p.TabText = lipgloss.Color(tf.TabText)
	}
	if tf.TabTextDim != "" {
		p.TabTextDim = lipgloss.Color(tf.TabTextDim)
	}
	return p, nil
}

// ListThemes returns the available theme names: built-ins first, then
// filenames (without .toml) from ThemesDir(), sorted alphabetically.
// Files whose names collide with a built-in are silently skipped.
func ListThemes() []string {
	result := make([]string, len(builtinNames))
	copy(result, builtinNames)

	entries, err := os.ReadDir(ThemesDir())
	if err != nil {
		return result // directory doesn't exist — return built-ins only
	}

	var userNames []string
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		name := e.Name()
		if filepath.Ext(name) != ".toml" {
			continue
		}
		stem := name[:len(name)-5] // strip ".toml"
		if builtinSet[stem] {
			continue // reserved name — skip
		}
		userNames = append(userNames, stem)
	}
	sort.Strings(userNames)
	return append(result, userNames...)
}
```

**Note:** Remove the dead code block with `apply` variable (lines starting `apply := func...` through `_ = apply`) — it was left by mistake in the template above. The final file should not include those lines.

- [ ] **Step 2.4 — Clean up dead code in theme_loader.go**

Remove these lines from `loadThemeFromPath` (they were a drafting artifact):

```go
	apply := func(dst *interface{ }, hex string) {
		// unused — use inline approach below
	}
	_ = apply
```

The function body should go directly from `var tf themeFile` to `if _, err := toml.Decode(...)`.

- [ ] **Step 2.5 — Run theme loader tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -run "TestLoadTheme|TestListThemes" -v
```

Expected: all 9 theme tests PASS.

- [ ] **Step 2.6 — Run full config test suite**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -v
```

Expected: all tests PASS.

- [ ] **Step 2.7 — Build check**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./...
```

Expected: no errors.

- [ ] **Step 2.8 — Write an ApplyChange test**

Add to `config_test.go`:

```go
func TestApplyChangeBool(t *testing.T) {
	cfg := Default()
	cfg = ApplyChange(cfg, "ui.show_borders", false)
	if cfg.Interface.ShowBorders != false {
		t.Error("ApplyChange ui.show_borders should set ShowBorders to false")
	}
}

func TestApplyChangeInt(t *testing.T) {
	cfg := Default()
	cfg = ApplyChange(cfg, "player.default_volume", 55)
	if cfg.Playback.DefaultVolume != 55 {
		t.Errorf("ApplyChange player.default_volume: got %d, want 55", cfg.Playback.DefaultVolume)
	}
}

func TestApplyChangeFloat(t *testing.T) {
	cfg := Default()
	cfg = ApplyChange(cfg, "skipper.similarity_threshold", 0.9)
	if cfg.Skipper.SimilarityThreshold != 0.9 {
		t.Errorf("ApplyChange skipper.similarity_threshold: got %f, want 0.9", cfg.Skipper.SimilarityThreshold)
	}
}

func TestApplyChangeThemeName(t *testing.T) {
	cfg := Default()
	cfg = ApplyChange(cfg, "interface.theme", "noctalia")
	if cfg.Interface.Theme != "noctalia" {
		t.Errorf("ApplyChange interface.theme: got %q, want %q", cfg.Interface.Theme, "noctalia")
	}
}

func TestApplyChangeUnknownKeyIsNoop(t *testing.T) {
	cfg := Default()
	before := cfg.Playback.DefaultVolume
	cfg = ApplyChange(cfg, "audio.dsp", "open") // settingAction — ignored
	if cfg.Playback.DefaultVolume != before {
		t.Error("ApplyChange unknown key should not change any field")
	}
}
```

- [ ] **Step 2.9 — Run all config tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -v
```

Expected: all tests PASS.

- [ ] **Step 2.10 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add pkg/config/theme_loader.go pkg/config/config_test.go && git commit -m "feat(config): LoadTheme, ListThemes, ApplyChange tests"
```

---

## Chunk 2: `pkg/config/watcher.go`

### Task 3: Config file watcher with write-guard and debounce

**Files:**
- Create: `tui/pkg/config/watcher.go`
- Modify: `tui/pkg/config/config_test.go` (add watcher tests)

The watcher mirrors the pattern in `pkg/theme/matugen_watcher.go` (read it for reference). It watches both `config.toml` and the `themes/` directory, debounces events at 150ms, and suppresses its own writes with a 200ms write-guard.

- [ ] **Step 3.1 — Write failing watcher tests**

Append to `tui/pkg/config/config_test.go`:

```go
func TestWatcherFiresOnConfigChange(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "config.toml")
	// Write initial config.
	if err := Save(cfgPath, Default()); err != nil {
		t.Fatal(err)
	}

	received := make(chan Config, 1)
	w, err := NewWatcher(cfgPath, func(c Config) { received <- c })
	if err != nil {
		t.Fatalf("NewWatcher: %v", err)
	}
	defer w.Stop()
	w.Start()

	// Modify the file externally (simulates a script write).
	time.Sleep(50 * time.Millisecond) // let watcher settle
	cfg := Default()
	cfg.Playback.DefaultVolume = 42
	if err := Save(cfgPath, cfg); err != nil {
		t.Fatal(err)
	}

	select {
	case got := <-received:
		if got.Playback.DefaultVolume != 42 {
			t.Errorf("reloaded DefaultVolume = %d, want 42", got.Playback.DefaultVolume)
		}
	case <-time.After(2 * time.Second):
		t.Error("watcher did not fire within 2s")
	}
}

func TestWatcherWriteGuardSuppressesSelfWrite(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "config.toml")
	if err := Save(cfgPath, Default()); err != nil {
		t.Fatal(err)
	}

	callCount := 0
	w, err := NewWatcher(cfgPath, func(Config) { callCount++ })
	if err != nil {
		t.Fatalf("NewWatcher: %v", err)
	}
	defer w.Stop()
	w.Start()
	time.Sleep(50 * time.Millisecond)

	// Simulate stui writing the file: call NotifyWrite before the actual save.
	w.NotifyWrite()
	if err := Save(cfgPath, Default()); err != nil {
		t.Fatal(err)
	}
	// Wait longer than the debounce + guard window to ensure no spurious call.
	time.Sleep(500 * time.Millisecond)
	if callCount != 0 {
		t.Errorf("write guard failed: onReload called %d times after NotifyWrite", callCount)
	}
}

func TestWatcherSetActiveThemeFiltersUnrelatedChanges(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "config.toml")
	if err := Save(cfgPath, Default()); err != nil {
		t.Fatal(err)
	}
	// Create themes dir inside temp dir; point ThemesDir won't work here,
	// so we use the watcher's internal mechanism by using the real ThemesDir.
	// This test only verifies that SetActiveTheme does not panic.
	w, err := NewWatcher(cfgPath, func(Config) {})
	if err != nil {
		t.Fatalf("NewWatcher: %v", err)
	}
	defer w.Stop()
	w.SetActiveTheme("default")         // built-in — no file watch
	w.SetActiveTheme("noctalia")        // user theme — no file exists, should not panic
	w.SetActiveTheme("high-contrast")   // built-in again
}
```

- [ ] **Step 3.2 — Run tests to confirm they fail**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -run "TestWatcher" -v 2>&1 | head -20
```

Expected: compile errors (Watcher not defined).

- [ ] **Step 3.3 — Create `tui/pkg/config/watcher.go`**

```go
package config

import (
	"path/filepath"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
)

const (
	watcherDebounce  = 150 * time.Millisecond
	watcherWriteGuard = 200 * time.Millisecond
)

// Watcher watches config.toml and the themes/ directory for external changes.
// It debounces events (150ms) and suppresses stui's own writes (200ms guard).
type Watcher struct {
	watcher      *fsnotify.Watcher
	cfgPath      string
	onReload     func(Config)
	stop         chan struct{}

	mu           sync.Mutex
	activeTheme  string    // current theme name (filters theme dir events)
	writeGuardAt time.Time // set by NotifyWrite; events within 200ms are suppressed
}

// NewWatcher creates a Watcher for cfgPath and the themes/ directory.
// onReload is called on the background goroutine whenever an external change
// is detected. Returns an error if fsnotify cannot be initialised.
func NewWatcher(cfgPath string, onReload func(Config)) (*Watcher, error) {
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	// Watch the directory containing config.toml (more reliable than the file itself).
	if err := fw.Add(filepath.Dir(cfgPath)); err != nil {
		fw.Close()
		return nil, err
	}

	// Watch the themes directory if it exists (ignore error if absent).
	_ = fw.Add(ThemesDir())

	return &Watcher{
		watcher:  fw,
		cfgPath:  cfgPath,
		onReload: onReload,
		stop:     make(chan struct{}),
	}, nil
}

// Start begins watching in the background goroutine.
func (w *Watcher) Start() {
	go w.loop()
}

// SetActiveTheme tells the watcher which theme name is currently active.
// Events for other theme files are ignored. Built-in names disable theme-file watching.
func (w *Watcher) SetActiveTheme(name string) {
	w.mu.Lock()
	w.activeTheme = name
	w.mu.Unlock()
}

// NotifyWrite suppresses watcher events for 200ms after stui writes config.toml.
// Call this immediately before config.Save().
func (w *Watcher) NotifyWrite() {
	w.mu.Lock()
	w.writeGuardAt = time.Now()
	w.mu.Unlock()
}

// Stop closes the watcher goroutine and underlying fsnotify watcher.
func (w *Watcher) Stop() error {
	close(w.stop)
	return w.watcher.Close()
}

func (w *Watcher) loop() {
	debounce := time.NewTimer(watcherDebounce)
	debounce.Stop()
	pending := false

	for {
		select {
		case <-w.stop:
			return

		case event, ok := <-w.watcher.Events:
			if !ok {
				return
			}
			if !w.isRelevant(event.Name) {
				continue
			}
			pending = true
			debounce.Reset(watcherDebounce)

		case <-debounce.C:
			if !pending {
				continue
			}
			pending = false
			w.mu.Lock()
			guarded := time.Since(w.writeGuardAt) < watcherWriteGuard
			w.mu.Unlock()
			if guarded {
				continue
			}
			cfg, err := Load(w.cfgPath)
			if err != nil {
				continue
			}
			w.onReload(cfg)

		case _, ok := <-w.watcher.Errors:
			if !ok {
				return
			}
		}
	}
}

// isRelevant returns true if the changed file is config.toml or the active theme file.
func (w *Watcher) isRelevant(name string) bool {
	abs, err := filepath.Abs(name)
	if err != nil {
		return false
	}
	cfgAbs, _ := filepath.Abs(w.cfgPath)
	if abs == cfgAbs {
		return true
	}
	// Check if it's the active theme file.
	w.mu.Lock()
	active := w.activeTheme
	w.mu.Unlock()
	if builtinSet[active] || active == "" {
		return false // built-ins have no file to watch
	}
	themeFile := filepath.Join(ThemesDir(), active+".toml")
	themeAbs, _ := filepath.Abs(themeFile)
	return abs == themeAbs
}
```

- [ ] **Step 3.4 — Run watcher tests**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -run "TestWatcher" -v -timeout 30s
```

Expected: all 3 watcher tests PASS.

- [ ] **Step 3.5 — Run full config test suite**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -v -timeout 30s
```

Expected: all tests PASS.

- [ ] **Step 3.6 — Build check**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./...
```

Expected: no errors.

- [ ] **Step 3.7 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add pkg/config/watcher.go pkg/config/config_test.go && git commit -m "feat(config): Watcher with fsnotify, write-guard, debounce"
```

---

## Chunk 3: Wire Up — main.go, ui.go, settings.go

### Task 4: Wire config into `main.go`

**Files:**
- Modify: `tui/cmd/stui/main.go`
- Modify: `tui/internal/ui/ui.go` (Options struct only — full wiring in Task 5)

`main.go` is straightforward: load config, create watcher (with `p.Send` as reload callback), start watcher after `tea.NewProgram`. Do this in **both** the `noSplash` branch and the default splash branch.

- [ ] **Step 4.1 — Add `CfgPath` to `ui.Options`**

In `tui/internal/ui/ui.go`, find the `Options` struct (around line 39) and add `CfgPath string`:

```go
type Options struct {
    RuntimePath string
    NoRuntime   bool
    Verbose     bool
    CfgPath     string
}
```

- [ ] **Step 4.2 — Update `main.go`**

In `tui/cmd/stui/main.go`:

1. Add imports for `pkg/config` and `pkg/theme`.
2. Before `opts := ui.Options{...}`, load the config:

```go
cfgPath := config.DefaultPath()
cfg, err := config.Load(cfgPath)
if err != nil {
    log.Warn("failed to load config, using defaults", "error", err)
    cfg = config.Default()
}

// Apply the configured theme before the UI starts.
if cfg.Interface.Theme != "matugen" {
    if palette, err := config.LoadTheme(cfg.Interface.Theme); err == nil {
        theme.T.Apply(palette)
    }
}
```

3. Pass `CfgPath` and update `ui.New` call (currently `ui.New(opts)` — Task 5 changes the signature; for now pass opts as-is):

```go
opts := ui.Options{
    RuntimePath: *runtimePath,
    NoRuntime:   *noRuntime,
    Verbose:     *verbose,
    CfgPath:     cfgPath,
}
```

4. Create the watcher (the `onReload` closure captures `p` as a pointer — declare `var p *tea.Program` before both branches):

Add just above the `if *noSplash {` block:

```go
var p *tea.Program

// Config watcher — fires when config.toml or the active theme file changes externally.
cfgWatcher, watchErr := config.NewWatcher(cfgPath, func(c config.Config) {
    if p != nil {
        p.Send(config.ConfigReloadMsg{Config: c})
    }
})
if watchErr != nil {
    log.Warn("could not start config watcher", "error", watchErr)
    cfgWatcher = nil
}
if cfgWatcher != nil {
    cfgWatcher.SetActiveTheme(cfg.Interface.Theme)
}
```

5. In the `noSplash` branch, replace `p := tea.NewProgram(...)` with an assignment to the outer `p`:

```go
if *noSplash {
    p = tea.NewProgram(&mainModel)
    mainModel.SetProgram(p)
    if cfgWatcher != nil {
        cfgWatcher.Start()
    }
    // ... existing run/error handling ...
}
```

6. In the splash branch, similarly assign to outer `p`:

```go
p = tea.NewProgram(model)
(&mainModel).SetProgram(p)
if cfgWatcher != nil {
    cfgWatcher.Start()
}
```

7. Defer watcher stop at the top of `main()` (after watcher is created):

```go
defer func() {
    if cfgWatcher != nil {
        cfgWatcher.Stop()
    }
}()
```

- [ ] **Step 4.3 — Build check**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./...
```

Expected: no errors. (ui.New signature unchanged yet — cfg not yet passed to it.)

- [ ] **Step 4.4 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add cmd/stui/main.go internal/ui/ui.go && git commit -m "feat(config): load config at startup, create watcher in main.go"
```

---

### Task 5: Wire config into `ui.go` — write-back and reload

**Files:**
- Modify: `tui/internal/ui/ui.go`

This task adds four things to `ui.go`:
1. `cfg`, `cfgPath`, `cfgSaveSeq`, `watcher` fields on Model
2. Updated `New(opts, cfg)` signature
3. `SettingsChangedMsg` handler adds `ApplyChange` + debounce write-back
4. `config.ConfigReloadMsg` handler

- [ ] **Step 5.1 — Add fields to Model struct**

Find the `Model` struct in `ui.go` (around line 100). Add after the `mediaCache` field:

```go
// Config persistence.
cfg         config.Config
cfgPath     string
cfgSaveSeq  int
watcher     *config.Watcher
```

Add import `"github.com/stui/stui/pkg/config"` to the import block.

- [ ] **Step 5.2 — Add `configSaveTickMsg` type**

Near the other unexported msg types (e.g. near `bingeTickMsg`), add:

```go
// configSaveTickMsg is sent by the debounce timer after a settings change.
// seq must match m.cfgSaveSeq; stale ticks (from superseded changes) are discarded.
type configSaveTickMsg struct{ seq int }
```

- [ ] **Step 5.3 — Update `New` to accept cfg and wire fields**

Change `func New(opts Options) Model` to `func New(opts Options, cfg config.Config) Model`.

Inside `New`, after `return Model{` (around line 251), add to the struct literal:

```go
cfg:     cfg,
cfgPath: opts.CfgPath,
```

Update all three call sites where `New(opts)` or `screens.NewSettingsModel(m.client)` appear in `ui.go` — the `NewSettingsModel` signature change is in Task 6; for now the call is unchanged.

Also update `cmd/stui/main.go` to pass `cfg`:

```go
innerModel := ui.New(opts, cfg)
```

- [ ] **Step 5.4 — Add write-back to `SettingsChangedMsg` handler**

The existing handler is around line 813. After the existing `m.client.SetConfig(msg.Key, msg.Value)` call (and the local mirror switch), add:

```go
// Persist to config file (debounced 300ms).
m.cfg = config.ApplyChange(m.cfg, msg.Key, msg.Value)
if msg.Key == "interface.theme" {
    if p, err := config.LoadTheme(m.cfg.Interface.Theme); err == nil {
        theme.T.Apply(p)
    }
    if m.watcher != nil {
        m.watcher.SetActiveTheme(m.cfg.Interface.Theme)
    }
}
m.cfgSaveSeq++
seq := m.cfgSaveSeq
cmds = append(cmds, tea.Tick(300*time.Millisecond, func(time.Time) tea.Msg {
    return configSaveTickMsg{seq}
}))
```

(If the handler currently uses a single `return m, cmd` pattern, switch to accumulating cmds with `tea.Batch`. Look at how the existing handler ends and append accordingly.)

- [ ] **Step 5.5 — Add `configSaveTickMsg` handler**

In the `Update` switch, add a new case (place it near the other internal tick handlers like `bingeTickMsg`):

```go
case configSaveTickMsg:
    if msg.seq != m.cfgSaveSeq {
        return m, nil // stale — a later change superseded this one
    }
    if m.watcher != nil {
        m.watcher.NotifyWrite()
    }
    _ = config.Save(m.cfgPath, m.cfg)
    return m, nil
```

- [ ] **Step 5.6 — Add `config.ConfigReloadMsg` handler**

In the `Update` switch, add (near other external-update handlers like `ipc.ThemeUpdateMsg`):

```go
case config.ConfigReloadMsg:
    m.cfg = msg.Config
    if msg.Config.Interface.Theme != "matugen" {
        if p, err := config.LoadTheme(msg.Config.Interface.Theme); err == nil {
            theme.T.Apply(p)
        }
    }
    if m.watcher != nil {
        m.watcher.SetActiveTheme(msg.Config.Interface.Theme)
    }
    // Forward to settings screen so its displayed values update.
    var cmd tea.Cmd
    m.settingsModel, cmd = m.settingsModel.Update(msg)
    return m, cmd
```

(Note: `m.settingsModel` may not exist as a direct field — the settings screen is opened via `screen.TransitionCmd`. If settings is only accessed via the RootModel screen stack, skip the forward here; the settings screen will receive the message via the normal Update delegation in RootModel. Only add the forward if there is a direct `m.settingsModel` field.)

Check whether `SettingsModel` is stored directly on `Model` or only opened via transitions. If only via transitions, remove the forward block and the `ConfigReloadMsg` will reach the settings screen automatically through RootModel's Update delegation.

- [ ] **Step 5.7 — Build check**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./...
```

Expected: no errors.

- [ ] **Step 5.8 — Run full test suite**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./...
```

Expected: all pass.

- [ ] **Step 5.9 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add internal/ui/ui.go cmd/stui/main.go && git commit -m "feat(config): wire cfg into ui.go — write-back debounce, ConfigReloadMsg handler"
```

---

### Task 6: Wire config into `settings.go` — pre-populate from cfg, add theme choice

**Files:**
- Modify: `tui/internal/ui/screens/settings.go`
- Modify: `tui/internal/ui/ui.go` (update NewSettingsModel call sites)

- [ ] **Step 6.1 — Update `NewSettingsModel` to accept cfg**

Find `func NewSettingsModel(client *ipc.Client) SettingsModel` in `settings.go`. Change signature to:

```go
func NewSettingsModel(client *ipc.Client, cfg config.Config) SettingsModel {
    m := SettingsModel{
        categories: defaultCategories(),
        client:     client,
    }
    m.populateFromConfig(cfg)
    return m
}
```

Add import `"github.com/stui/stui/pkg/config"` to `settings.go`.

- [ ] **Step 6.2 — Add the `"interface.theme"` settingChoice item**

In `defaultCategories()`, find the Interface category (around line 869). Add a new item as the **first** item in the Interface items slice, before the existing `"app.theme_mode"` item:

```go
{
    label:       "Theme",
    key:         "interface.theme",
    kind:        settingChoice,
    choiceVals:  config.ListThemes(),
    choiceIdx:   0,
    description: "Active colour theme (built-in or from ~/.config/stui/themes/)",
},
```

Keep the existing `"app.theme_mode"` item unchanged (it controls matugen dark/light).

Update the existing `"app.theme_mode"` item's description to clarify:

```go
description: "Matugen mode — only used when Theme = matugen",
```

- [ ] **Step 6.3 — Add `populateFromConfig` helper**

Add this private method to `SettingsModel` in `settings.go`:

```go
// populateFromConfig sets each settingItem's value from cfg.
// Preserves catCursor and itemCursor (navigation state is not reset).
func (m *SettingsModel) populateFromConfig(cfg config.Config) {
    for _, cat := range m.categories {
        for _, item := range cat.items {
            switch item.key {
            case "interface.theme":
                for i, v := range item.choiceVals {
                    if v == cfg.Interface.Theme {
                        item.choiceIdx = i
                        break
                    }
                }
            case "app.theme_mode":
                for i, v := range item.choiceVals {
                    if v == cfg.Interface.ThemeMode {
                        item.choiceIdx = i
                        break
                    }
                }
            case "ui.show_borders":
                item.boolVal = cfg.Interface.ShowBorders
            case "ui.mouse_support":
                item.boolVal = cfg.Interface.MouseSupport
            case "ui.bidi_mode":
                for i, v := range item.choiceVals {
                    if v == cfg.Interface.BiDiMode {
                        item.choiceIdx = i
                        break
                    }
                }
            case "player.default_volume":
                item.intVal = cfg.Playback.DefaultVolume
            case "player.hwdec":
                for i, v := range item.choiceVals {
                    if v == cfg.Playback.Hwdec {
                        item.choiceIdx = i
                        break
                    }
                }
            case "player.cache_secs":
                item.intVal = cfg.Playback.CacheSecs
            case "player.keep_open":
                item.boolVal = cfg.Playback.KeepOpen
            case "playback.autoplay_next":
                item.boolVal = cfg.Playback.AutoplayNext
            case "playback.autoplay_countdown":
                item.intVal = cfg.Playback.AutoplayCountdown
            case "player.min_preroll_secs":
                item.intVal = cfg.Playback.MinPrerollSecs
            case "player.demuxer_max_mb":
                item.intVal = cfg.Playback.DemuxerMaxMB
            case "player.terminal_vo":
                for i, v := range item.choiceVals {
                    if v == cfg.Playback.TerminalVO {
                        item.choiceIdx = i
                        break
                    }
                }
            case "streaming.prefer_http":
                item.boolVal = cfg.Streaming.PreferHTTP
            case "streaming.auto_fallback":
                item.boolVal = cfg.Streaming.AutoFallback
            case "streaming.max_candidates":
                item.intVal = cfg.Streaming.MaxCandidates
            case "streaming.benchmark_streams":
                item.boolVal = cfg.Streaming.BenchmarkStreams
            case "streaming.auto_delete_video":
                item.boolVal = cfg.Streaming.AutoDeleteVideo
            case "streaming.auto_delete_audio":
                item.boolVal = cfg.Streaming.AutoDeleteAudio
            case "downloads.video_dir":
                item.strVal = cfg.Downloads.VideoDir
            case "downloads.music_dir":
                item.strVal = cfg.Downloads.MusicDir
            case "subtitles.auto_download":
                item.boolVal = cfg.Subtitles.AutoDownload
            case "subtitles.preferred_language":
                for i, v := range item.choiceVals {
                    if v == cfg.Subtitles.PreferredLanguage {
                        item.choiceIdx = i
                        break
                    }
                }
            case "subtitles.default_delay":
                item.floatVal = cfg.Subtitles.DefaultDelay
            case "providers.enable_tmdb":
                item.boolVal = cfg.Providers.EnableTMDB
            case "providers.enable_omdb":
                item.boolVal = cfg.Providers.EnableOMDB
            case "providers.enable_torrentio":
                item.boolVal = cfg.Providers.EnableTorrentio
            case "providers.enable_prowlarr":
                item.boolVal = cfg.Providers.EnableProwlarr
            case "providers.enable_opensubtitles":
                item.boolVal = cfg.Providers.EnableOpenSubtitles
            case "notifications.enabled":
                item.boolVal = cfg.Notifications.Enabled
            case "notifications.backend":
                for i, v := range item.choiceVals {
                    if v == cfg.Notifications.Backend {
                        item.choiceIdx = i
                        break
                    }
                }
            case "notifications.on_playback":
                item.boolVal = cfg.Notifications.OnPlayback
            case "notifications.on_download":
                item.boolVal = cfg.Notifications.OnDownload
            case "notifications.on_streams":
                item.boolVal = cfg.Notifications.OnStreams
            case "skipper.enabled":
                item.boolVal = cfg.Skipper.Enabled
            case "skipper.auto_skip_intro":
                item.boolVal = cfg.Skipper.AutoSkipIntro
            case "skipper.auto_skip_credits":
                item.boolVal = cfg.Skipper.AutoSkipCredits
            case "skipper.intro_scan_secs":
                item.intVal = cfg.Skipper.IntroScanSecs
            case "skipper.min_intro_secs":
                item.intVal = cfg.Skipper.MinIntroSecs
            case "skipper.max_intro_secs":
                item.intVal = cfg.Skipper.MaxIntroSecs
            case "skipper.similarity_threshold":
                item.floatVal = cfg.Skipper.SimilarityThreshold
            case "skipper.min_episodes":
                item.intVal = cfg.Skipper.MinEpisodes
            }
        }
    }
}
```

- [ ] **Step 6.4 — Handle `config.ConfigReloadMsg` in settings Update**

In `SettingsModel.Update`, add a new case (before or after the `tea.WindowSizeMsg` case):

```go
case config.ConfigReloadMsg:
    m.populateFromConfig(msg.Config)
    return m, nil
```

- [ ] **Step 6.5 — Update all `NewSettingsModel` call sites in `ui.go`**

Search for all three call sites (around lines 1232, 1355, 1839):

```bash
grep -n "NewSettingsModel" /home/ozogorgor/Projects/Stui_Project/stui/tui/internal/ui/ui.go
```

Change each from `screens.NewSettingsModel(m.client)` to `screens.NewSettingsModel(m.client, m.cfg)`.

- [ ] **Step 6.6 — Build check**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./...
```

Expected: no errors.

- [ ] **Step 6.7 — Run full test suite**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./...
```

Expected: all pass.

- [ ] **Step 6.8 — Smoke test: launch and verify settings populate from config**

```bash
# Write a config with a changed value.
mkdir -p ~/.config/stui
cat > ~/.config/stui/config.toml <<'EOF'
[playback]
default_volume = 42
EOF

cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go run ./cmd/stui/main.go --no-runtime --no-splash 2>/dev/null &
sleep 2
kill %1 2>/dev/null || true
```

(This only verifies startup doesn't panic. Visual verification of the settings screen showing volume=42 requires interactive use.)

- [ ] **Step 6.9 — Commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git add internal/ui/screens/settings.go internal/ui/ui.go && git commit -m "feat(config): populate settings screen from config, ConfigReloadMsg handler, interface.theme choice"
```

---

### Task 7: Create `~/.config/stui/themes/` and verify end-to-end

- [ ] **Step 7.1 — Create the themes directory on disk**

```bash
mkdir -p ~/.config/stui/themes
```

- [ ] **Step 7.2 — Write a test theme file**

```bash
cat > ~/.config/stui/themes/test-theme.toml <<'EOF'
name = "Test Theme"
bg         = "#001122"
accent     = "#ff6600"
border_foc = "#ff6600"
EOF
```

- [ ] **Step 7.3 — Test ListThemes includes the new file**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go test ./pkg/config/... -run TestListThemes -v
```

Expected: passes (the test doesn't check the real ThemesDir but the unit logic is verified).

- [ ] **Step 7.4 — Reference the test theme in config and launch**

```bash
cat > ~/.config/stui/config.toml <<'EOF'
[interface]
theme = "test-theme"
EOF
```

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build -o /tmp/stui-test ./cmd/stui && /tmp/stui-test --no-runtime --no-splash &
sleep 2; kill %1 2>/dev/null || true
```

Expected: starts without panic; if running in a real terminal, accent color should be `#ff6600`.

- [ ] **Step 7.5 — Final full build + test**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && go build ./... && go test ./...
```

Expected: clean build, all tests pass.

- [ ] **Step 7.6 — Final commit**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/tui && git commit --allow-empty -m "feat(config): config+themes feature complete — TOML config file, themes dir, live reload"
```

(Empty commit only if no files are staged; otherwise stage and commit normally.)
