package state

import (
	"os"
	"path/filepath"
)

// app_state.go — structured sub-state types embedded in AppState.
//
// These types group related mutable state that was previously scattered
// across the root Model struct, making it easier to pass context to screens
// and reason about what is currently happening in the app.

// ── CurrentMedia ──────────────────────────────────────────────────────────────

// CurrentMedia holds the catalog entry that is currently focused — either
// open in the detail overlay or actively being played back.
// Cleared when the detail overlay is dismissed and nothing is playing.
type CurrentMedia struct {
	ID       string
	Title    string
	Year     string
	Genre    string
	Rating   string
	Tab      Tab
	Provider string
	ImdbID   string
}

// IsSet reports whether a media entry is currently focused.
func (c CurrentMedia) IsSet() bool { return c.ID != "" }

// ── CurrentStream ─────────────────────────────────────────────────────────────

// CurrentStream holds metadata about the actively playing stream.
// Populated on PlayerStartedMsg, updated on PlayerProgressMsg,
// cleared on PlayerEndedMsg.
type CurrentStream struct {
	// URL is the local path or remote URL being played by mpv.
	URL      string
	// Title is the human-readable title of the playing item.
	Title    string
	// Provider is the source that resolved the stream (e.g. "torrentio").
	Provider string
	// Quality is the resolved quality label, e.g. "1080p", "4K" (may be empty).
	Quality  string
	// Protocol is the transport type: "torrent", "http", "magnet", etc.
	Protocol string

	// Position and Duration are in seconds; updated on every progress tick.
	Position float64
	Duration float64
}

// IsSet reports whether a stream is currently playing.
func (c CurrentStream) IsSet() bool { return c.URL != "" }

// ProgressFraction returns playback progress in [0, 1].
// Returns 0 if Duration is unknown.
func (c CurrentStream) ProgressFraction() float64 {
	if c.Duration <= 0 {
		return 0
	}
	return c.Position / c.Duration
}

// ── Settings ──────────────────────────────────────────────────────────────────

// Settings holds user-configurable options that are mirrored from the runtime
// config. Having them here means any screen can read them without going
// through the root Model.
type Settings struct {
	// Playback
	AutoSkipIntro   bool
	AutoSkipCredits bool

	// Post-playback cleanup
	AutoDeleteVideo bool // default true
	AutoDeleteAudio bool // default false

	// Stream selection
	BenchmarkStreams bool // default false

	// Display
	ViewMode ViewMode

	// Autoplay
	AutoplayNext      bool // default false — initialises bingeEnabled on EpisodeScreen
	AutoplayCountdown int  // seconds; 0 treated as 5 in countdown logic

	// Downloads — directory paths for aria2 downloads.
	VideoDownloadDir string // default ~/Videos
	MusicDownloadDir string // default ~/Music
}

// DefaultSettings returns the settings values that match the runtime defaults.
func DefaultSettings() Settings {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		home = "."
	}
	return Settings{
		AutoDeleteVideo:  true,
		VideoDownloadDir: filepath.Join(home, "Videos"),
		MusicDownloadDir: filepath.Join(home, "Music"),
	}
}
