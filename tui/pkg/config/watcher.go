package config

import (
	"path/filepath"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
)

const (
	watcherDebounce   = 150 * time.Millisecond
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
	activeTheme  string
	writeGuardAt time.Time
}

// NewWatcher creates a Watcher for cfgPath and the themes/ directory.
// onReload is called on the background goroutine whenever an external change
// is detected. Returns an error if fsnotify cannot be initialised.
func NewWatcher(cfgPath string, onReload func(Config)) (*Watcher, error) {
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}

	if err := fw.Add(filepath.Dir(cfgPath)); err != nil {
		fw.Close()
		return nil, err
	}

	_ = fw.Add(ThemesDir())

	return &Watcher{
		watcher:  fw,
		cfgPath:  cfgPath,
		onReload: onReload,
		stop:     make(chan struct{}),
	}, nil
}

// Start begins watching in a background goroutine.
func (w *Watcher) Start() {
	go w.loop()
}

// SetActiveTheme tells the watcher which theme name is currently active.
func (w *Watcher) SetActiveTheme(name string) {
	w.mu.Lock()
	w.activeTheme = name
	w.mu.Unlock()
}

// NotifyWrite suppresses watcher events for 200ms after stui writes config.toml.
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
	w.mu.Lock()
	active := w.activeTheme
	w.mu.Unlock()
	if builtinSet[active] || active == "" {
		return false
	}
	themeFilePath := filepath.Join(ThemesDir(), active+".toml")
	themeAbs, _ := filepath.Abs(themeFilePath)
	return abs == themeAbs
}
