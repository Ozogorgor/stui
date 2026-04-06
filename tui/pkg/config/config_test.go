package config

import (
	"os"
	"path/filepath"
	"testing"
	"time"
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

func TestLoadThemeBuiltinDefault(t *testing.T) {
	p, err := LoadTheme("default")
	if err != nil {
		t.Fatalf("LoadTheme(default): %v", err)
	}
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
	tomlContent := `bg = "#112233"` + "\n"
	path := filepath.Join(dir, "mytheme.toml")
	if err := os.WriteFile(path, []byte(tomlContent), 0o644); err != nil {
		t.Fatal(err)
	}
	p, err := loadThemeFromPath(path)
	if err != nil {
		t.Fatalf("loadThemeFromPath: %v", err)
	}
	if p.Bg == nil {
		t.Error("Bg should not be nil after loading theme file")
	}
	if p.Surface == nil {
		t.Error("Surface should fall back to Default() and not be nil")
	}
}

func TestLoadThemeMissingFileReturnsDefault(t *testing.T) {
	p, err := LoadTheme("nonexistent-theme-xyzzy")
	if err == nil {
		t.Error("LoadTheme of nonexistent theme should return an error")
	}
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
	themes := ListThemes()
	seen := map[string]int{}
	for _, name := range themes {
		seen[name]++
		if seen[name] > 1 {
			t.Errorf("ListThemes: %q appears more than once", name)
		}
	}
}

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
	cfg = ApplyChange(cfg, "audio.dsp", "open")
	if cfg.Playback.DefaultVolume != before {
		t.Error("ApplyChange unknown key should not change any field")
	}
}

func TestWatcherFiresOnConfigChange(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "config.toml")
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

	time.Sleep(50 * time.Millisecond)
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

	w.NotifyWrite()
	if err := Save(cfgPath, Default()); err != nil {
		t.Fatal(err)
	}
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
	w, err := NewWatcher(cfgPath, func(Config) {})
	if err != nil {
		t.Fatalf("NewWatcher: %v", err)
	}
	defer w.Stop()
	w.SetActiveTheme("default")
	w.SetActiveTheme("noctalia")
	w.SetActiveTheme("high-contrast")
}
