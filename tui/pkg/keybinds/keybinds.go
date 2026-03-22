package keybinds

import (
	"encoding/json"
	"os"
	"path/filepath"

	"charm.land/bubbles/v2/key"
)

// ── Persistence ───────────────────────────────────────────────────────────────

// UserBindings maps action name strings to their replacement key strings.
// Only actions the user has explicitly overridden are present.
// Stored as JSON at DefaultPath().
//
// Example file contents:
//
//	{
//	  "navigate_up":   ["w", "up"],
//	  "player_pause":  ["p"]
//	}
type UserBindings map[string][]string

// DefaultPath returns the canonical path for the keybinds config file.
// (~/.config/stui/keybinds.json)
func DefaultPath() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "keybinds.json")
	}
	if home, err := os.UserHomeDir(); err == nil {
		return filepath.Join(home, ".config", "stui", "keybinds.json")
	}
	return ""
}

// Load reads user keybindings from path.
// Returns an empty map (no error) if the file does not exist.
func Load(path string) (UserBindings, error) {
	data, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return UserBindings{}, nil
	}
	if err != nil {
		return nil, err
	}
	var b UserBindings
	if err := json.Unmarshal(data, &b); err != nil {
		return nil, err
	}
	return b, nil
}

// Save writes user keybindings to path, creating parent directories as needed.
// An empty map writes a minimal valid JSON file.
func Save(path string, b UserBindings) error {
	if path == "" {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(b, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0o644)
}

// KeyMap holds all keybindings for stui.
// Grouped by context: Navigation, Tabs, Search, Player, Subtitles, Audio.
type KeyMap struct {
	// ── Navigation ────────────────────────────────────────────────────────
	Up    key.Binding
	Down  key.Binding
	Left  key.Binding
	Right key.Binding
	Enter key.Binding
	Back  key.Binding

	// ── Tabs ──────────────────────────────────────────────────────────────
	Tab1    key.Binding // Movies
	Tab2    key.Binding // Series
	Tab3    key.Binding // Music
	Tab4    key.Binding // Library
	NextTab key.Binding
	PrevTab key.Binding

	// ── App actions ───────────────────────────────────────────────────────
	Search   key.Binding
	Settings key.Binding
	Quit     key.Binding
	Help     key.Binding

	// ── Player — transport ────────────────────────────────────────────────
	PlayerPause        key.Binding // space
	PlayerSeekFwd      key.Binding // →  (+10s)
	PlayerSeekBack     key.Binding // ←  (-10s)
	PlayerSeekFwdLong  key.Binding // shift+→ (+60s)
	PlayerSeekBackLong key.Binding // shift+← (-60s)
	PlayerStop         key.Binding // Q (shift+q)
	PlayerFullscreen   key.Binding // f
	PlayerScreenshot   key.Binding // S (shift+s)

	// ── Player — volume ───────────────────────────────────────────────────
	VolumeUp   key.Binding // ] or 0
	VolumeDown key.Binding // [ or 9
	VolumeMute key.Binding // m

	// ── Player — stream switching ─────────────────────────────────────────
	SwitchStream    key.Binding // s → opens stream picker
	NextCandidate   key.Binding // n → auto-switch to next candidate

	// ── Player — subtitle control ─────────────────────────────────────────
	SubtitleTrack    key.Binding // v → cycle subtitle tracks
	SubtitleDisable  key.Binding // V (shift+v) → disable subs
	SubDelayPlus     key.Binding // z → subtitle +0.1s
	SubDelayMinus    key.Binding // Z (shift+z) → subtitle -0.1s
	SubDelayReset    key.Binding // X (shift+x) → reset sub delay

	// ── Player — audio control ────────────────────────────────────────────
	AudioTrack       key.Binding // a → cycle audio tracks
	AudioDelayPlus   key.Binding // ctrl+] → audio +0.1s
	AudioDelayMinus  key.Binding // ctrl+[ → audio -0.1s
	AudioDelayReset  key.Binding // ctrl+\ → reset audio delay
}

// Default returns the default keybindings.
func Default() KeyMap {
	return KeyMap{
		// Navigation
		Up:    key.NewBinding(key.WithKeys("up", "k"),    key.WithHelp("↑/k", "up")),
		Down:  key.NewBinding(key.WithKeys("down", "j"),  key.WithHelp("↓/j", "down")),
		Left:  key.NewBinding(key.WithKeys("left", "h"),  key.WithHelp("←/h", "left")),
		Right: key.NewBinding(key.WithKeys("right", "l"), key.WithHelp("→/l", "right")),
		Enter: key.NewBinding(key.WithKeys("enter"),      key.WithHelp("enter", "select")),
		Back:  key.NewBinding(key.WithKeys("esc"),        key.WithHelp("esc", "back")),

		// Tabs
		Tab1:    key.NewBinding(key.WithKeys("1"),         key.WithHelp("1", "movies")),
		Tab2:    key.NewBinding(key.WithKeys("2"),         key.WithHelp("2", "series")),
		Tab3:    key.NewBinding(key.WithKeys("3"),         key.WithHelp("3", "music")),
		Tab4:    key.NewBinding(key.WithKeys("4"),         key.WithHelp("4", "library")),
		NextTab: key.NewBinding(key.WithKeys("tab"),       key.WithHelp("tab", "next tab")),
		PrevTab: key.NewBinding(key.WithKeys("shift+tab"), key.WithHelp("shift+tab", "prev tab")),

		// App
		Search:   key.NewBinding(key.WithKeys("/"),   key.WithHelp("/", "search")),
		Settings: key.NewBinding(key.WithKeys(","),   key.WithHelp(",", "settings")),
		Quit:     key.NewBinding(key.WithKeys("q", "ctrl+c"), key.WithHelp("q", "quit")),
		Help:     key.NewBinding(key.WithKeys("?"),   key.WithHelp("?", "help")),

		// Player — transport
		PlayerPause:        key.NewBinding(key.WithKeys(" "),          key.WithHelp("space", "pause")),
		PlayerSeekFwd:      key.NewBinding(key.WithKeys("right"),      key.WithHelp("→", "+10s")),
		PlayerSeekBack:     key.NewBinding(key.WithKeys("left"),       key.WithHelp("←", "-10s")),
		PlayerSeekFwdLong:  key.NewBinding(key.WithKeys("shift+right"),key.WithHelp("⇧→", "+60s")),
		PlayerSeekBackLong: key.NewBinding(key.WithKeys("shift+left"), key.WithHelp("⇧←", "-60s")),
		PlayerStop:         key.NewBinding(key.WithKeys("Q"),          key.WithHelp("Q", "stop")),
		PlayerFullscreen:   key.NewBinding(key.WithKeys("f"),          key.WithHelp("f", "fullscreen")),
		PlayerScreenshot:   key.NewBinding(key.WithKeys("S"),          key.WithHelp("S", "screenshot")),

		// Player — volume
		VolumeUp:   key.NewBinding(key.WithKeys("]", "0"), key.WithHelp("]/0", "vol +")),
		VolumeDown: key.NewBinding(key.WithKeys("[", "9"), key.WithHelp("[/9", "vol -")),
		VolumeMute: key.NewBinding(key.WithKeys("m"),      key.WithHelp("m", "mute")),

		// Player — stream switching
		SwitchStream:  key.NewBinding(key.WithKeys("s"), key.WithHelp("s", "streams")),
		NextCandidate: key.NewBinding(key.WithKeys("n"), key.WithHelp("n", "next stream")),

		// Player — subtitle control
		SubtitleTrack:   key.NewBinding(key.WithKeys("v"),   key.WithHelp("v", "cycle subs")),
		SubtitleDisable: key.NewBinding(key.WithKeys("V"),   key.WithHelp("V", "no subs")),
		SubDelayPlus:    key.NewBinding(key.WithKeys("z"),   key.WithHelp("z", "sub +0.1s")),
		SubDelayMinus:   key.NewBinding(key.WithKeys("Z"),   key.WithHelp("Z", "sub -0.1s")),
		SubDelayReset:   key.NewBinding(key.WithKeys("X"),   key.WithHelp("X", "sub reset")),

		// Player — audio control
		AudioTrack:      key.NewBinding(key.WithKeys("a"),       key.WithHelp("a", "cycle audio")),
		AudioDelayPlus:  key.NewBinding(key.WithKeys("ctrl+]"),  key.WithHelp("⌃]", "audio +0.1s")),
		AudioDelayMinus: key.NewBinding(key.WithKeys("ctrl+["),  key.WithHelp("⌃[", "audio -0.1s")),
		AudioDelayReset: key.NewBinding(key.WithKeys("ctrl+\\"), key.WithHelp("⌃\\", "audio reset")),
	}
}

// ShortHelp returns a compact keybinding list for the status bar.
func (k KeyMap) ShortHelp() []key.Binding {
	return []key.Binding{k.Search, k.Up, k.Down, k.Enter, k.Settings, k.Quit}
}

// PlayerHelp returns keybindings shown in the player HUD footer.
func (k KeyMap) PlayerHelp() []key.Binding {
	return []key.Binding{
		k.PlayerPause,
		k.PlayerSeekBack,
		k.PlayerSeekFwd,
		k.PlayerStop,
		k.SwitchStream,
		k.SubtitleTrack,
		k.SubDelayPlus,
		k.SubDelayMinus,
		k.VolumeUp,
		k.VolumeDown,
	}
}
