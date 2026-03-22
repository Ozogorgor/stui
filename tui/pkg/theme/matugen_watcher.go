package theme

// matugen_watcher.go — watches matugen's colors.json and applies theme updates.
//
// This file provides a file watcher that monitors matugen's colors.json
// and applies theme changes directly in Go, eliminating the need for
// Rust to watch the file and push updates via IPC.

import (
	"encoding/json"
	"os"
	"path/filepath"
	"time"

	"github.com/fsnotify/fsnotify"
)

// MatugenColorsPath returns the path to matugen's colors.json.
// Priority: STUI_MATUGEN_COLORS env var → ~/.config/matugen/colors.json
func MatugenColorsPath() string {
	if path := os.Getenv("STUI_MATUGEN_COLORS"); path != "" {
		return path
	}
	home, _ := os.UserHomeDir()
	if home == "" {
		home = "/root"
	}
	return filepath.Join(home, ".config", "matugen", "colors.json")
}

// MatugenWatcher watches matugen's colors.json and calls onApply when it changes.
type MatugenWatcher struct {
	watcher *fsnotify.Watcher
	path    string
	onApply func(Palette)
	stop    chan struct{}
}

// NewMatugenWatcher creates a watcher that monitors matugen's colors.json.
// Call Start() to begin watching, and Stop() to close.
func NewMatugenWatcher(onApply func(Palette)) (*MatugenWatcher, error) {
	path := MatugenColorsPath()

	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	dir := filepath.Dir(path)
	if err := watcher.Add(dir); err != nil {
		watcher.Close()
		return nil, err
	}

	return &MatugenWatcher{
		watcher: watcher,
		path:    path,
		onApply: onApply,
		stop:    make(chan struct{}),
	}, nil
}

// loadAndApply reads the colors file and applies the palette.
func (w *MatugenWatcher) loadAndApply(mode string) {
	data, err := os.ReadFile(w.path)
	if err != nil {
		return
	}

	var root map[string]interface{}
	if err := json.Unmarshal(data, &root); err != nil {
		return
	}

	if colors, ok := root["colors"].(map[string]interface{}); ok {
		if dark, ok := colors[mode].(map[string]interface{}); ok {
			m := make(map[string]string)
			for k, v := range dark {
				if s, ok := v.(string); ok {
					m[k] = s
				}
			}
			w.onApply(FromMatugen(m))
			return
		}
	}

	m := make(map[string]string)
	for k, v := range root {
		if s, ok := v.(string); ok {
			m[k] = s
		}
	}
	if len(m) > 0 {
		w.onApply(FromMatugen(m))
	}
}

// Start begins watching for file changes. Runs in background.
func (w *MatugenWatcher) Start(mode string) {
	w.loadAndApply(mode)
	go func() {
		debounce := time.NewTimer(150 * time.Millisecond)
		pending := false

		for {
			select {
			case <-w.stop:
				return
			case event, ok := <-w.watcher.Events:
				if !ok {
					return
				}
				if event.Name == w.path {
					pending = true
					debounce.Reset(150 * time.Millisecond)
				}
			case <-debounce.C:
				if pending {
					pending = false
					w.loadAndApply(mode)
				}
			case _, ok := <-w.watcher.Errors:
				if !ok {
					return
				}
			}
		}
	}()
}

// Stop closes the watcher.
func (w *MatugenWatcher) Stop() error {
	close(w.stop)
	return w.watcher.Close()
}
