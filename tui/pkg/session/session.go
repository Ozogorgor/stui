package session

// session.go — Persists a small amount of UI state across stui runs.
//
// Saved to ~/.config/stui/session.json.
// Written atomically (temp-file + rename) on every significant change.

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// State is the full set of values persisted between runs.
type State struct {
	// LastTab is the String() value of the active state.Tab ("Movies", "Music", …).
	LastTab string `json:"last_tab,omitempty"`

	// LastMusicSubTab is the int value of the active MusicSubTab (0=Browse … 3=Playlists).
	LastMusicSubTab int `json:"last_music_sub_tab,omitempty"`

	// QueueURIs is the ordered list of MPD file URIs that were in the playback
	// queue the last time stui exited (or the queue changed).
	QueueURIs []string `json:"queue_uris,omitempty"`
}

// DefaultPath returns the canonical path for the session file.
// (~/.config/stui/session.json)
func DefaultPath() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "session.json")
	}
	if home, err := os.UserHomeDir(); err == nil {
		return filepath.Join(home, ".config", "stui", "session.json")
	}
	return ""
}

// Load reads the session file. Returns a zero-value State on any error
// (missing file, corrupt JSON, etc.).
func Load(path string) State {
	data, err := os.ReadFile(path)
	if err != nil {
		return State{}
	}
	var s State
	if err := json.Unmarshal(data, &s); err != nil {
		return State{}
	}
	return s
}

// Save writes s to path atomically.  A missing parent directory is created
// automatically.  Silently does nothing if path is empty.
func Save(path string, s State) error {
	if path == "" {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
