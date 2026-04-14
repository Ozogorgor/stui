// Package actions defines the typed action model for the stui TUI.
//
// Instead of mixing key-handling logic directly inside BubbleTea Update()
// methods, all meaningful user intents are expressed as AppAction values.
// This separation makes it easy to:
//
//   - Test intent-handling without simulating key events
//   - Support remappable keybindings (map key → AppAction)
//   - Handle the same action from multiple sources (keyboard, mouse, IPC)
//   - Log/record user actions for analytics or replay
//
// # Architecture
//
//	KeyMsg ──► keyToAction() ──► AppAction ──► handleAction()
//	                                              │
//	                          ┌───────────────────┼────────────────────┐
//	                          ▼                   ▼                    ▼
//	                    model update          IPC send             screen change
//
// # Usage
//
//	func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
//	    switch msg := msg.(type) {
//	    case tea.KeyPressMsg:
//	        if action, ok := actions.FromKey(msg.String()); ok {
//	            return m.handleAction(action)
//	        }
//	    }
//	    ...
//	}
package actions

import "sync"

// AppAction is a typed user intent.
// All user actions that have side effects are represented here.
type AppAction int

const (
	// ── Navigation ────────────────────────────────────────────────────────
	ActionNone AppAction = iota
	ActionNavigateUp
	ActionNavigateDown
	ActionNavigateLeft
	ActionNavigateRight
	ActionSelect
	ActionBack
	ActionQuit

	// ── Tabs ──────────────────────────────────────────────────────────────
	ActionTab1 // Movies
	ActionTab2 // Series
	ActionTab3 // Music
	ActionTab4 // Library
	ActionTab5 // Collections
	ActionNextTab
	ActionPrevTab

	// ── Search & navigation ───────────────────────────────────────────────
	ActionOpenSearch
	ActionOpenSettings
	ActionOpenHelp

	// ── Player — transport ────────────────────────────────────────────────
	ActionPlayerPause       // toggle pause
	ActionPlayerSeekFwd     // +10s
	ActionPlayerSeekBack    // -10s
	ActionPlayerSeekFwdLong // +60s
	ActionPlayerSeekBackLong// -60s
	ActionPlayerStop
	ActionPlayerFullscreen
	ActionPlayerScreenshot

	// ── Player — volume ───────────────────────────────────────────────────
	ActionVolumeUp
	ActionVolumeDown
	ActionVolumeMute

	// ── Player — subtitles ────────────────────────────────────────────────
	ActionSubtitleCycle     // cycle to next track
	ActionSubtitleOff       // disable subtitles
	ActionSubDelayPlus      // +0.1s
	ActionSubDelayMinus     // -0.1s
	ActionSubDelayReset

	// ── Player — audio ────────────────────────────────────────────────────
	ActionAudioPicker       // open audio track picker screen
	ActionAudioCycle        // cycle to next audio track
	ActionAudioDelayPlus    // +0.1s
	ActionAudioDelayMinus   // -0.1s
	ActionAudioDelayReset

	// ── Player — stream switching ─────────────────────────────────────────
	ActionStreamSwitch      // open stream picker
	ActionStreamNext        // auto-switch to next candidate

	// ── Skip detection ────────────────────────────────────────────────────
	ActionSkipIntro         // skip detected intro or credits segment

	// Sentinel for range checks
	actionMax
)

// String returns a human-readable name for the action (useful for logging).
func (a AppAction) String() string {
	switch a {
	case ActionNone:             return "none"
	case ActionNavigateUp:       return "navigate_up"
	case ActionNavigateDown:     return "navigate_down"
	case ActionNavigateLeft:     return "navigate_left"
	case ActionNavigateRight:    return "navigate_right"
	case ActionSelect:           return "select"
	case ActionBack:             return "back"
	case ActionQuit:             return "quit"
	case ActionTab1:             return "tab_movies"
	case ActionTab2:             return "tab_series"
	case ActionTab3:             return "tab_music"
	case ActionTab4:             return "tab_library"
	case ActionTab5:             return "tab_collections"
	case ActionNextTab:          return "next_tab"
	case ActionPrevTab:          return "prev_tab"
	case ActionOpenSearch:       return "open_search"
	case ActionOpenSettings:     return "open_settings"
	case ActionOpenHelp:         return "open_help"
	case ActionPlayerPause:      return "player_pause"
	case ActionPlayerSeekFwd:    return "player_seek_fwd"
	case ActionPlayerSeekBack:   return "player_seek_back"
	case ActionPlayerSeekFwdLong:  return "player_seek_fwd_long"
	case ActionPlayerSeekBackLong: return "player_seek_back_long"
	case ActionPlayerStop:       return "player_stop"
	case ActionPlayerFullscreen: return "player_fullscreen"
	case ActionPlayerScreenshot: return "player_screenshot"
	case ActionVolumeUp:         return "volume_up"
	case ActionVolumeDown:       return "volume_down"
	case ActionVolumeMute:       return "volume_mute"
	case ActionSubtitleCycle:    return "subtitle_cycle"
	case ActionSubtitleOff:      return "subtitle_off"
	case ActionSubDelayPlus:     return "sub_delay_plus"
	case ActionSubDelayMinus:    return "sub_delay_minus"
	case ActionSubDelayReset:    return "sub_delay_reset"
	case ActionAudioPicker:      return "audio_picker"
	case ActionAudioCycle:       return "audio_cycle"
	case ActionAudioDelayPlus:   return "audio_delay_plus"
	case ActionAudioDelayMinus:  return "audio_delay_minus"
	case ActionAudioDelayReset:  return "audio_delay_reset"
	case ActionStreamSwitch:     return "stream_switch"
	case ActionStreamNext:       return "stream_next"
	case ActionSkipIntro:        return "skip_intro"
	default:                     return "unknown"
	}
}

// IsPlayerAction returns true for actions that require active playback.
func (a AppAction) IsPlayerAction() bool {
	return a >= ActionPlayerPause && a < actionMax
}

// ── Default key map ───────────────────────────────────────────────────────────

// defaultKeyMap is the built-in key → action mapping.
// In the future this could be read from the config file.
var defaultKeyMap = map[string]AppAction{
	// Navigation
	"up":    ActionNavigateUp,
	"k":     ActionNavigateUp,
	"down":  ActionNavigateDown,
	"j":     ActionNavigateDown,
	"left":  ActionNavigateLeft,
	"h":     ActionNavigateLeft,
	"right": ActionNavigateRight,
	"l":     ActionNavigateRight,
	"enter": ActionSelect,
	"esc":   ActionBack,
	"q":     ActionQuit,
	"ctrl+c": ActionQuit,

	// Tabs
	"1":         ActionTab1,
	"2":         ActionTab2,
	"3":         ActionTab3,
	"4":         ActionTab4,
	"5":         ActionTab5,
	"tab":       ActionNextTab,
	"shift+tab": ActionPrevTab,

	// App
	"/": ActionOpenSearch,
	"`": ActionOpenSettings,
	"~": ActionOpenSettings,
	"?": ActionOpenHelp,

	// Player transport
	" ":           ActionPlayerPause,
	"shift+right": ActionPlayerSeekFwdLong,
	"shift+left":  ActionPlayerSeekBackLong,
	"Q":           ActionPlayerStop,
	"f":           ActionPlayerFullscreen,
	"S":           ActionPlayerScreenshot,

	// Volume
	"]": ActionVolumeUp,
	"0": ActionVolumeUp,
	"[": ActionVolumeDown,
	"9": ActionVolumeDown,
	"m": ActionVolumeMute,

	// Subtitles
	"v": ActionSubtitleCycle,
	"V": ActionSubtitleOff,
	"z": ActionSubDelayPlus,
	"Z": ActionSubDelayMinus,
	"X": ActionSubDelayReset,

	// Audio
	"A":      ActionAudioPicker,
	"a":      ActionAudioCycle,
	"ctrl+]": ActionAudioDelayPlus,
	"ctrl+[": ActionAudioDelayMinus,
	`ctrl+\`: ActionAudioDelayReset,

	// Streams
	"s": ActionStreamSwitch,
	"n": ActionStreamNext,

	// Skip detection
	"i": ActionSkipIntro,
}

// ── Live key map (mutable, user-configurable) ─────────────────────────────────

var (
	kmMu       sync.RWMutex
	activeMap  map[string]AppAction   // key string → action  (dispatch)
	reverseMap map[AppAction][]string // action → key strings (display)
	// userOverrides records which actions the user has explicitly rebound.
	// Keys are action name strings; values are the replacement key strings.
	userOverrides = map[string][]string{}
)

func init() {
	activeMap, reverseMap = compileMaps(defaultKeyMap)
}

// compileMaps builds both the forward and reverse maps from a key→action map.
func compileMaps(km map[string]AppAction) (map[string]AppAction, map[AppAction][]string) {
	fwd := make(map[string]AppAction, len(km))
	rev := make(map[AppAction][]string, len(km))
	for k, a := range km {
		fwd[k] = a
		rev[a] = append(rev[a], k)
	}
	return fwd, rev
}

// applyOverrides rebuilds the active maps from defaults + current userOverrides.
// Must be called with kmMu held for writing.
func applyOverrides() {
	km := make(map[string]AppAction, len(defaultKeyMap))
	for k, a := range defaultKeyMap {
		km[k] = a
	}
	for name, keys := range userOverrides {
		action := ActionFromString(name)
		if action == ActionNone {
			continue
		}
		// Remove all default keys for this action
		for k, a := range km {
			if a == action {
				delete(km, k)
			}
		}
		// Add user keys (skip empty strings)
		for _, key := range keys {
			if key != "" {
				km[key] = action
			}
		}
	}
	activeMap, reverseMap = compileMaps(km)
}

// FromKey looks up the AppAction for a given key string.
// Returns (ActionNone, false) if no mapping exists.
func FromKey(key string) (AppAction, bool) {
	kmMu.RLock()
	a, ok := activeMap[key]
	kmMu.RUnlock()
	return a, ok
}

// ActionKeys returns the key strings currently bound to action a.
func ActionKeys(a AppAction) []string {
	kmMu.RLock()
	keys := reverseMap[a]
	kmMu.RUnlock()
	return keys
}

// IsOverridden reports whether action a has a user-set keybind.
func IsOverridden(a AppAction) bool {
	kmMu.RLock()
	_, ok := userOverrides[a.String()]
	kmMu.RUnlock()
	return ok
}

// ActionFromString returns the AppAction whose String() equals s.
// Returns ActionNone for unknown names.
func ActionFromString(s string) AppAction {
	for a := ActionNone + 1; a < actionMax; a++ {
		if a.String() == s {
			return a
		}
	}
	return ActionNone
}

// SetUserBindings applies a complete set of user overrides (loaded from disk).
// Actions absent from bindings retain their defaults.
func SetUserBindings(bindings map[string][]string) {
	kmMu.Lock()
	userOverrides = make(map[string][]string, len(bindings))
	for k, v := range bindings {
		userOverrides[k] = v
	}
	applyOverrides()
	kmMu.Unlock()
}

// BindAction rebinds a single action to the given keys.
// Pass nil or an empty slice to revert the action to its defaults.
// Returns the updated user-overrides map suitable for persisting to disk.
func BindAction(action AppAction, keys []string) map[string][]string {
	kmMu.Lock()
	defer kmMu.Unlock()

	if len(keys) == 0 {
		delete(userOverrides, action.String())
	} else {
		userOverrides[action.String()] = keys
	}
	applyOverrides()

	// Return a copy for the caller to persist
	out := make(map[string][]string, len(userOverrides))
	for k, v := range userOverrides {
		out[k] = v
	}
	return out
}

// UserOverrides returns a copy of the current user-override map for persistence.
func UserOverrides() map[string][]string {
	kmMu.RLock()
	out := make(map[string][]string, len(userOverrides))
	for k, v := range userOverrides {
		out[k] = v
	}
	kmMu.RUnlock()
	return out
}

// AllMappings returns a copy of the active key → action map (for help screen).
func AllMappings() map[string]AppAction {
	kmMu.RLock()
	out := make(map[string]AppAction, len(activeMap))
	for k, v := range activeMap {
		out[k] = v
	}
	kmMu.RUnlock()
	return out
}

// GroupedHelp returns actions grouped by category for the help screen.
func GroupedHelp() []ActionGroup {
	return []ActionGroup{
		{
			Title: "Navigation",
			Rows: []ActionRow{
				{"↑/k / ↓/j", "Move"},
				{"enter", "Select"},
				{"esc", "Back"},
				{"1–5", "Switch tab"},
				{"tab / shift+tab", "Cycle tabs"},
			},
		},
		{
			Title: "Search & App",
			Rows: []ActionRow{
				{"/", "Search"},
				{"`", "Settings"},
				{"?", "Help"},
				{"q", "Quit"},
			},
		},
		{
			Title: "Player",
			Rows: []ActionRow{
				{"space", "Pause / resume"},
				{"← / →", "Seek ±10s"},
				{"⇧← / ⇧→", "Seek ±60s"},
				{"]/[ or 0/9", "Volume ±5"},
				{"m", "Mute toggle"},
				{"Q", "Stop"},
				{"f", "Fullscreen"},
			},
		},
		{
			Title: "Subtitles & Audio",
			Rows: []ActionRow{
				{"v / V", "Cycle subs / off"},
				{"z / Z", "Sub delay ±0.1s"},
				{"X", "Reset sub delay"},
				{"A", "Pick audio track"},
			{"a", "Cycle audio track"},
				{"⌃] / ⌃[", "Audio delay ±0.1s"},
			},
		},
		{
			Title: "Streams",
			Rows: []ActionRow{
				{"s", "Switch stream"},
				{"n", "Next candidate"},
			},
		},
	}
}

// ActionGroup is a named group of key hints for the help overlay.
type ActionGroup struct {
	Title string
	Rows  []ActionRow
}

// ActionRow is a single key → description pair.
type ActionRow struct {
	Key  string
	Desc string
}

// ── Structured action groups (for the keybind editor) ─────────────────────────

// ActionDef pairs an AppAction with its human-readable description.
type ActionDef struct {
	Action AppAction
	Desc   string
}

// ActionGroupDef is a named group of ActionDef values.
type ActionGroupDef struct {
	Title string
	Items []ActionDef
}

// GroupedActions returns all bindable actions grouped by category.
// Used by the keybind editor to list every action with a description.
func GroupedActions() []ActionGroupDef {
	return []ActionGroupDef{
		{
			Title: "Navigation",
			Items: []ActionDef{
				{ActionNavigateUp, "Move up"},
				{ActionNavigateDown, "Move down"},
				{ActionNavigateLeft, "Move left"},
				{ActionNavigateRight, "Move right"},
				{ActionSelect, "Select"},
				{ActionBack, "Back"},
				{ActionTab5, "Collections tab"},
				{ActionNextTab, "Next tab"},
				{ActionPrevTab, "Previous tab"},
			},
		},
		{
			Title: "App",
			Items: []ActionDef{
				{ActionOpenSearch, "Search"},
				{ActionOpenSettings, "Settings"},
				{ActionOpenHelp, "Help"},
				{ActionQuit, "Quit"},
			},
		},
		{
			Title: "Player",
			Items: []ActionDef{
				{ActionPlayerPause, "Pause / resume"},
				{ActionPlayerSeekFwd, "Seek +10s"},
				{ActionPlayerSeekBack, "Seek -10s"},
				{ActionPlayerSeekFwdLong, "Seek +60s"},
				{ActionPlayerSeekBackLong, "Seek -60s"},
				{ActionPlayerStop, "Stop playback"},
				{ActionPlayerFullscreen, "Toggle fullscreen"},
				{ActionPlayerScreenshot, "Screenshot"},
			},
		},
		{
			Title: "Volume",
			Items: []ActionDef{
				{ActionVolumeUp, "Volume +5"},
				{ActionVolumeDown, "Volume -5"},
				{ActionVolumeMute, "Toggle mute"},
			},
		},
		{
			Title: "Subtitles",
			Items: []ActionDef{
				{ActionSubtitleCycle, "Cycle subtitle tracks"},
				{ActionSubtitleOff, "Disable subtitles"},
				{ActionSubDelayPlus, "Subtitle delay +0.1s"},
				{ActionSubDelayMinus, "Subtitle delay -0.1s"},
				{ActionSubDelayReset, "Reset subtitle delay"},
			},
		},
		{
			Title: "Audio",
			Items: []ActionDef{
				{ActionAudioPicker, "Open audio track picker"},
				{ActionAudioCycle, "Cycle audio tracks"},
				{ActionAudioDelayPlus, "Audio delay +0.1s"},
				{ActionAudioDelayMinus, "Audio delay -0.1s"},
				{ActionAudioDelayReset, "Reset audio delay"},
			},
		},
		{
			Title: "Streams",
			Items: []ActionDef{
				{ActionStreamSwitch, "Open stream picker"},
				{ActionStreamNext, "Next stream candidate"},
				{ActionSkipIntro, "Skip intro / credits"},
			},
		},
	}
}
