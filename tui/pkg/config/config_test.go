package config

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
