package screens

// settings.go — Settings screen: a two-level menu (category → items).
//
// Layout:
//
//   ┌─────────────────────────────────────────────────┐
//   │  ⚙  Settings                                    │
//   ├─────────────────────────────────────────────────┤
//   │                                                 │
//   │    ▶ Playback                                   │
//   │      Providers                                  │
//   │      Subtitles                                  │
//   │      Interface                                  │
//   │      Plugins                                    │
//   │                                                 │
//   ├─────────────────────────────────────────────────┤
//   │  ↑↓ navigate   enter select   esc back   q quit │
//   └─────────────────────────────────────────────────┘
//
// When a category is selected, the right panel shows its items:
//
//   ┌──────────────┬──────────────────────────────────┐
//   │ Categories   │ Playback                         │
//   │              │                                  │
//   │ ▶ Playback   │  Volume          100             │
//   │   Providers  │  Hardware decode auto            │
//   │   Subtitles  │  Cache (secs)    20              │
//   │   Interface  │  Auto fallback   on              │
//   │   Plugins    │  Benchmark       off             │
//   │              │                                  │
//   └──────────────┴──────────────────────────────────┘
//
// Changes are sent to the runtime via IPC SetConfig messages.

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"charm.land/bubbles/v2/textinput"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/config"
	"github.com/stui/stui/pkg/theme"
)

// settingsHomeDir is resolved once at program start and used by displayValue()
// and defaultCategories() to avoid calling os.UserHomeDir() on every render.
// Tests can set this variable directly to control the home path.
var settingsHomeDir string

func init() {
	h, err := os.UserHomeDir()
	if err != nil || h == "" {
		settingsHomeDir = "."
	} else {
		settingsHomeDir = h
	}
}

// detectMpdConfPaths reads mpd.conf and extracts music_directory and
// playlist_directory. Returns empty strings if not found.
func detectMpdConfPaths() (musicDir, playlistDir string) {
	candidates := []string{
		filepath.Join(settingsHomeDir, ".config", "mpd", "mpd.conf"),
		filepath.Join(settingsHomeDir, ".mpd", "mpd.conf"),
		"/etc/mpd.conf",
	}
	for _, path := range candidates {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		for _, line := range strings.Split(string(data), "\n") {
			line = strings.TrimSpace(line)
			if strings.HasPrefix(line, "#") || line == "" {
				continue
			}
			if val, ok := extractMpdDirective(line, "music_directory"); ok && musicDir == "" {
				musicDir = expandTilde(val)
			}
			if val, ok := extractMpdDirective(line, "playlist_directory"); ok && playlistDir == "" {
				playlistDir = expandTilde(val)
			}
		}
		if musicDir != "" || playlistDir != "" {
			return
		}
	}
	return
}

func extractMpdDirective(line, directive string) (string, bool) {
	if !strings.HasPrefix(line, directive) {
		return "", false
	}
	rest := strings.TrimSpace(line[len(directive):])
	rest = strings.Trim(rest, "\"")
	if rest == "" {
		return "", false
	}
	return rest, true
}

func expandTilde(path string) string {
	if strings.HasPrefix(path, "~") {
		return settingsHomeDir + path[1:]
	}
	return path
}

func isValidPath(path string) bool {
	abs, err := filepath.Abs(path)
	if err != nil {
		return false
	}
	abs = filepath.Clean(abs)
	if strings.HasPrefix(abs, "..") {
		return false
	}
	return true
}

// ── Setting item types ────────────────────────────────────────────────────────

type settingKind int

const (
	settingBool   settingKind = iota // on/off toggle
	settingInt                       // integer with +/- adjustment
	settingFloat                     // float with +/- adjustment
	settingChoice                    // cycle through a fixed list
	settingInfo                      // read-only informational row
	settingAction                    // press Enter → emits a message (no value change)
	settingPath                      // editable filesystem path; Enter opens inline textinput
	settingString                    // freeform text string
)

// settingItem represents one configurable value in a category.
type settingItem struct {
	label         string
	key           string // dot-separated config key e.g. "player.default_volume"
	kind          settingKind
	boolVal       bool
	intVal        int
	floatVal      float64
	choiceVals    []string
	choiceIdx     int
	strVal        string   // current path value for settingPath items
	description   string   // shown in the footer when focused
	minVal        int      // lower bound for settingInt; 0 = no lower bound
	maxVal        int      // upper bound for settingInt; 0 = no upper bound
	choiceDisplay []string // optional display names for choice values (if nil, use choiceVals)
	hidden        bool    // if true, item is not rendered and skipped during navigation
}

// ── Visualizer visibility helpers ────────────────────────────────────────────

// isClassicVizMode returns true for the cava/chroma bar-style modes.
func isClassicVizMode(mode string) bool {
	switch mode {
	case "bars", "mirror", "filled", "led":
		return true
	}
	return false
}

// updateVizVisibility hides or shows visualizer items in cat based on
// the currently selected backend and mode. Call whenever either changes.
func updateVizVisibility(cat *settingCategory) {
	var backend, mode string
	for _, item := range cat.items {
		switch item.key {
		case "visualizer.backend":
			if item.choiceIdx < len(item.choiceVals) {
				backend = item.choiceVals[item.choiceIdx]
			}
		case "visualizer.mode":
			if item.choiceIdx < len(item.choiceVals) {
				mode = item.choiceVals[item.choiceIdx]
			}
		}
	}
	isOff := backend == "off"
	isChroma := backend == "chroma"
	isCliamp := !isClassicVizMode(mode)
	for _, item := range cat.items {
		switch item.key {
		case "visualizer.backend":
			item.hidden = false
		case "visualizer.bars":
			item.hidden = isOff || isCliamp
		case "visualizer.height":
			item.hidden = isOff
		case "visualizer.framerate":
			item.hidden = isOff
		case "visualizer.mode":
			item.hidden = isOff
		case "visualizer.peak_hold":
			item.hidden = isOff || isCliamp
		case "visualizer.gradient":
			item.hidden = isOff
		case "visualizer.input_method":
			item.hidden = isOff || isChroma
		}
	}
}

// firstVisibleIdx returns the index of the first non-hidden item.
func firstVisibleIdx(items []*settingItem) int {
	for i, item := range items {
		if !item.hidden {
			return i
		}
	}
	return 0
}

// nextVisibleIdx returns the nearest non-hidden index from from+delta direction.
func nextVisibleIdx(items []*settingItem, from, delta int) int {
	n := len(items)
	for idx := from + delta; idx >= 0 && idx < n; idx += delta {
		if !items[idx].hidden {
			return idx
		}
	}
	return from
}

// nearestVisibleIdx returns from if visible, otherwise the closest non-hidden neighbour.
func nearestVisibleIdx(items []*settingItem, from int) int {
	if from < len(items) && !items[from].hidden {
		return from
	}
	for delta := 1; delta < len(items); delta++ {
		if i := from + delta; i < len(items) && !items[i].hidden {
			return i
		}
		if i := from - delta; i >= 0 && !items[i].hidden {
			return i
		}
	}
	return firstVisibleIdx(items)
}

// visibleItemAt maps a rendered row (skipping hidden items) to the slice index.
func visibleItemAt(items []*settingItem, row int) (int, bool) {
	count := 0
	for i, item := range items {
		if item.hidden {
			continue
		}
		if count == row {
			return i, true
		}
		count++
	}
	return 0, false
}

func (s *settingItem) displayValue() string {
	switch s.kind {
	case settingBool:
		if s.boolVal {
			return "on"
		}
		return "off"
	case settingInt:
		return fmt.Sprintf("%d", s.intVal)
	case settingFloat:
		return fmt.Sprintf("%.1f", s.floatVal)
	case settingChoice:
		if s.choiceIdx < len(s.choiceVals) {
			// Use display name if available, otherwise use value
			if s.choiceDisplay != nil && s.choiceIdx < len(s.choiceDisplay) {
				return s.choiceDisplay[s.choiceIdx]
			}
			return s.choiceVals[s.choiceIdx]
		}
		return "—"
	case settingInfo:
		return s.description
	case settingAction:
		return "→"
	case settingPath:
		if settingsHomeDir == "." {
			return s.strVal
		}
		rel, err := filepath.Rel(settingsHomeDir, s.strVal)
		if err == nil && rel != "." && !strings.HasPrefix(rel, "..") {
			return "~/" + rel
		}
		if err == nil && rel == "." {
			return "~"
		}
		return s.strVal
	}
	return "—"
}

func (s *settingItem) toggle() {
	switch s.kind {
	case settingBool:
		s.boolVal = !s.boolVal
	case settingChoice:
		s.choiceIdx = (s.choiceIdx + 1) % len(s.choiceVals)
	}
}

func (s *settingItem) adjust(delta int) {
	switch s.kind {
	case settingInt:
		s.intVal += delta
		if s.maxVal > 0 && s.intVal > s.maxVal {
			s.intVal = s.maxVal
		}
		if s.minVal > 0 && s.intVal < s.minVal {
			s.intVal = s.minVal
		}
	case settingFloat:
		s.floatVal += float64(delta) * 0.5
	case settingChoice:
		n := len(s.choiceVals)
		s.choiceIdx = (s.choiceIdx + delta + n) % n
	}
}

// ── Setting category ──────────────────────────────────────────────────────────

type settingCategory struct {
	name  string
	icon  string
	items []*settingItem
}

// ── Settings message ──────────────────────────────────────────────────────────

// SettingsChangedMsg is sent to the root model when a value changes.
type SettingsChangedMsg struct {
	Key   string
	Value interface{}
}

// ── SettingsModel ─────────────────────────────────────────────────────────────

// SettingsModel is the standalone settings screen.
// It implements the screen.Screen interface.
type SettingsModel struct {
	Dims
	categories []settingCategory
	catCursor  int  // which category is selected
	itemCursor int  // which item within the category is focused
	inCategory bool // true = navigating items; false = navigating categories
	// Path editing state — active when the user is editing a settingPath item.
	editing   bool
	editInput textinput.Model
	client    *ipc.Client
}

func NewSettingsModel(client *ipc.Client, cfg config.Config) SettingsModel {
	m := SettingsModel{
		categories: defaultCategories(),
		client:     client,
	}
	m.populateFromConfig(cfg)
	return m
}

// populateFromConfig sets each settingItem's value from cfg.
func (m *SettingsModel) populateFromConfig(cfg config.Config) {
	for ci := range m.categories {
		cat := &m.categories[ci]
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
			case "storage.movies":
				item.strVal = cfg.Storage.Movies
			case "storage.series":
				item.strVal = cfg.Storage.Series
			case "storage.anime":
				item.strVal = cfg.Storage.Anime
			case "storage.music":
				item.strVal = cfg.Storage.Music
			case "storage.podcasts":
				item.strVal = cfg.Storage.Podcasts
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
			case "visualizer.backend":
				for i, v := range item.choiceVals {
					if v == cfg.Visualizer.Backend {
						item.choiceIdx = i
						break
					}
				}
			case "visualizer.bars":
				item.intVal = cfg.Visualizer.Bars
			case "visualizer.height":
				item.intVal = cfg.Visualizer.Height
			case "visualizer.framerate":
				item.intVal = cfg.Visualizer.Framerate
			case "visualizer.mode":
				for i, v := range item.choiceVals {
					if v == cfg.Visualizer.Mode {
						item.choiceIdx = i
						break
					}
				}
			case "visualizer.peak_hold":
				item.boolVal = cfg.Visualizer.PeakHold
			case "visualizer.gradient":
				item.boolVal = cfg.Visualizer.Gradient
			case "visualizer.input_method":
				for i, v := range item.choiceVals {
					if v == cfg.Visualizer.InputMethod {
						item.choiceIdx = i
						break
					}
				}
			}
		}
		// After values are set, update visibility for visualizer items.
		for _, item := range cat.items {
			if item.key == "visualizer.backend" {
				updateVizVisibility(cat)
				break
			}
		}
	}
}

// ── Screen interface ──────────────────────────────────────────────────────────

func (m SettingsModel) Init() tea.Cmd { return nil }

func (m SettingsModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	// ── Editing intercept — settingPath inline text input ─────────────────
	// While editing, all input is consumed here. Navigation is suppressed.
	if m.editing {
		switch msg := msg.(type) {
		case tea.KeyPressMsg:
			switch msg.String() {
			case "enter":
				// Confirm: write the typed value back to the item.
				item := m.categories[m.catCursor].items[m.itemCursor]
				newPath := m.editInput.Value()
				if item.kind == settingPath && !isValidPath(newPath) {
					m.editing = false
					return m, nil
				}
				item.strVal = newPath
				m.editing = false
				return m, settingChangedCmd(item)
			case "esc":
				// Cancel: discard typed text, no change.
				m.editing = false
				return m, nil
			default:
				// Forward to textinput; capture both return values.
				newInput, cmd := m.editInput.Update(msg)
				m.editInput = newInput
				return m, cmd
			}
		case tea.MouseMsg:
			// Suppress mouse events during editing to prevent cursor drift.
			return m, nil
		}
		return m, nil
	}

	switch msg := msg.(type) {

	case tea.WindowSizeMsg:
		m.setWindowSize(msg)

	case config.ConfigReloadMsg:
		m.populateFromConfig(msg.Config)
		return m, nil

	case tea.MouseMsg:
		mouse := msg.Mouse()
		switch {
		case mouse.Button == tea.MouseWheelUp:
			if !m.inCategory {
				if m.catCursor > 0 {
					m.catCursor--
					m.itemCursor = 0
				}
			} else {
				if m.itemCursor > 0 {
					m.itemCursor--
				}
			}
		case mouse.Button == tea.MouseWheelDown:
			if !m.inCategory {
				if m.catCursor < len(m.categories)-1 {
					m.catCursor++
					m.itemCursor = 0
				}
			} else {
				cat := m.categories[m.catCursor]
				if m.itemCursor < len(cat.items)-1 {
					m.itemCursor++
				}
			}
		case mouse.Button == tea.MouseLeft:
			// Layout (matches View()):
			//   row 0   header
			//   row 1   blank
			//   row 2   left/right box top border
			//   row 3+  inner content (boxInnerH rows)
			//
			// Columns:
			//   left box  : outer X=0..leftOuterW-1   (leftInnerW=20, +2 border)
			//   1-col gap : X=leftOuterW
			//   right box : outer X=leftOuterW+1..end (border around)
			if clickMsg, ok := msg.(tea.MouseClickMsg); ok && clickMsg.Button == tea.MouseLeft {
				const leftInnerW = 20
				const leftOuterW = leftInnerW + 2 // +2 for border

				// Inner-row index (skip header, blank, top-border).
				innerRow := mouse.Y - 3
				if innerRow < 0 {
					break
				}

				inLeftContent := mouse.X >= 1 && mouse.X <= leftInnerW
				inRightContent := mouse.X >= leftOuterW+1+1 // +1 gap, +1 right-box border

				if inLeftContent {
					if innerRow < len(m.categories) {
						m.catCursor = innerRow
						m.inCategory = false
						m.itemCursor = 0
					}
				} else if inRightContent {
					// Right inner rows: 0=cat header, 1=blank, 2..2+itemsViewH-1=items.
					itemRow := innerRow - 2
					if itemRow >= 0 {
						cat := m.categories[m.catCursor]
						// Build the same visible list View() uses, then apply
						// the same center-mode scroll to translate the row to
						// an index.
						visible := make([]int, 0, len(cat.items))
						for i := range cat.items {
							if !cat.items[i].hidden {
								visible = append(visible, i)
							}
						}
						boxInnerH := len(m.categories)
						if boxInnerH < 4 {
							boxInnerH = 4
						}
						const rightHeaderRows = 2
						const rightFooterRows = 2
						itemsViewH := boxInnerH - rightHeaderRows - rightFooterRows
						if itemsViewH < 1 {
							itemsViewH = 1
						}
						scroll := 0
						if len(visible) > itemsViewH {
							scroll = m.itemCursor - itemsViewH/2
							if scroll < 0 {
								scroll = 0
							}
							if scroll > len(visible)-itemsViewH {
								scroll = len(visible) - itemsViewH
							}
						}
						if itemRow < itemsViewH && scroll+itemRow < len(visible) {
							m.inCategory = true
							m.itemCursor = visible[scroll+itemRow]
						}
					}
				}
			}
		}
		return m, nil

	case tea.KeyPressMsg:
		switch msg.String() {

		// ── Category navigation (left panel) ───────────────────────────
		case "up", "k":
			if !m.inCategory {
				if m.catCursor > 0 {
					m.catCursor--
					m.itemCursor = firstVisibleIdx(m.categories[m.catCursor].items)
				}
			} else {
				cat := m.categories[m.catCursor]
				m.itemCursor = nextVisibleIdx(cat.items, m.itemCursor, -1)
			}

		case "down", "j":
			if !m.inCategory {
				if m.catCursor < len(m.categories)-1 {
					m.catCursor++
					m.itemCursor = firstVisibleIdx(m.categories[m.catCursor].items)
				}
			} else {
				cat := m.categories[m.catCursor]
				m.itemCursor = nextVisibleIdx(cat.items, m.itemCursor, +1)
			}

		case "right", "l":
			if !m.inCategory && len(m.categories[m.catCursor].items) > 0 {
				m.inCategory = true
				m.itemCursor = firstVisibleIdx(m.categories[m.catCursor].items)
			}

		case "enter":
			if !m.inCategory && len(m.categories[m.catCursor].items) > 0 {
				m.inCategory = true
				m.itemCursor = firstVisibleIdx(m.categories[m.catCursor].items)
			} else if m.inCategory {
				cat := m.categories[m.catCursor]
				if m.itemCursor < len(cat.items) {
					item := cat.items[m.itemCursor]
					// settingPath — start inline editing instead of toggle
					if item.kind == settingPath {
						ti := textinput.New()
						ti.SetValue(item.strVal)
						ti.CursorEnd()
						leftW := 18  // match View() left panel width
						margin := 6  // match View() rightW margin
						labelW := 20 // approximate label column width
						inputW := m.width - leftW - labelW - margin
						if inputW < 20 {
							inputW = 20
						}
						ti.SetWidth(inputW)
						ti.CharLimit = 512
						cmd := ti.Focus() // Focus() returns blink cmd — must not be dropped
						m.editInput = ti
						m.editing = true
						return m, cmd
					}
					// Action items navigate to a sub-screen
					if item.kind == settingAction {
						switch item.key {
						case "audio.dsp":
							// Push Settings onto the overlay history so esc/backspace
							// in DSP returns here instead of closing both.
							return m, screen.TransitionCmd(NewAudioSettingsModel(m.client), true)
						case "plugins.manager":
							return m, func() tea.Msg { return OpenPluginManagerMsg{} }
						case "plugins.manage_repos":
							return m, func() tea.Msg { return OpenPluginReposMsg{} }
						case "keybinds.edit":
							return m, func() tea.Msg { return OpenKeybindsEditorMsg{} }
						case "stats.stream_radar":
							return m, func() tea.Msg { return OpenStreamRadarMsg{} }
						case "stats.rating_weights":
							return m, func() tea.Msg { return OpenRatingWeightsMsg{} }
						case "stats.offline_library":
							return m, func() tea.Msg { return OpenOfflineLibraryMsg{} }
						case "stats.clear_cache":
							return m, func() tea.Msg { return ClearMediaCacheMsg{} }
						case "dsp.crossfeed_enabled":
							dialog := NewCrossfeedDialogModel(func(key string, val interface{}) tea.Cmd {
								return func() tea.Msg { return SettingsChangedMsg{Key: key, Value: val} }
							})
							dialog.SetSize(m.width, m.height)
							return m, screen.TransitionCmd(dialog, true)
						default:
							return m, func() tea.Msg { return OpenPluginSettingsMsg{} }
						}
					}
					item.toggle()
					if strings.HasPrefix(item.key, "visualizer.") {
						cat := &m.categories[m.catCursor]
						updateVizVisibility(cat)
						m.itemCursor = nearestVisibleIdx(cat.items, m.itemCursor)
					}
					return m, settingChangedCmd(item)
				}
			}

		case "left", "h", "esc":
			if m.inCategory {
				m.inCategory = false
			}
			// ESC when in category list is handled by RootModel (pop screen)

		case "+", "=":
			if m.inCategory {
				cat := &m.categories[m.catCursor]
				if m.itemCursor < len(cat.items) {
					cat.items[m.itemCursor].adjust(+1)
					if strings.HasPrefix(cat.items[m.itemCursor].key, "visualizer.") {
						updateVizVisibility(cat)
						m.itemCursor = nearestVisibleIdx(cat.items, m.itemCursor)
					}
					return m, settingChangedCmd(cat.items[m.itemCursor])
				}
			}

		case "-", "_":
			if m.inCategory {
				cat := &m.categories[m.catCursor]
				if m.itemCursor < len(cat.items) {
					cat.items[m.itemCursor].adjust(-1)
					if strings.HasPrefix(cat.items[m.itemCursor].key, "visualizer.") {
						updateVizVisibility(cat)
						m.itemCursor = nearestVisibleIdx(cat.items, m.itemCursor)
					}
					return m, settingChangedCmd(cat.items[m.itemCursor])
				}
			}
		}
	}
	return m, nil
}

func settingChangedCmd(item *settingItem) tea.Cmd {
	return func() tea.Msg {
		var v interface{}
		switch item.kind {
		case settingBool:
			v = item.boolVal
		case settingInt:
			v = item.intVal
		case settingFloat:
			v = item.floatVal
		case settingChoice:
			if item.choiceIdx < len(item.choiceVals) {
				v = item.choiceVals[item.choiceIdx]
			}
		case settingPath:
			v = item.strVal
		}
		return SettingsChangedMsg{Key: item.key, Value: v}
	}
}

// ── View ──────────────────────────────────────────────────────────────────────

func (m SettingsModel) View() tea.View {
	if m.width == 0 {
		return tea.NewView("  ⚙  Settings\n")
	}

	// ── Styles ─────────────────────────────────────────────────────────────
	headerStyle := lipgloss.NewStyle().
		Bold(true).
		Foreground(theme.T.Accent()).
		PaddingLeft(2)

	catActiveStyle := lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Background(theme.T.Surface()).
		Bold(true)

	catNormalStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text()).
		Background(theme.T.Surface())

	catDimStyle := lipgloss.NewStyle().
		Foreground(theme.T.TextDim()).
		Background(theme.T.Surface())

	leftBgStyle := lipgloss.NewStyle().
		Background(theme.T.Surface())

	itemActiveStyle := lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Bold(true)

	itemNormalStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text())

	valStyle := lipgloss.NewStyle().
		Foreground(theme.T.TextDim())

	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	// ── Header ─────────────────────────────────────────────────────────────
	header := headerStyle.Render("⚙  Settings")

	// ── Left panel layout: width + box height driven by allocated height ──
	// The overlay system gives us m.height rows. We consume 6 rows of
	// overhead (header, 2 blanks, footer, border top/bottom) so the panels
	// get the rest. Falls back to category count if height isn't set yet.
	const leftInnerW = 20 // inner content width of the categories box
	boxInnerH := m.height - 6 // 1 header + 2 blank + 1 footer + 2 border
	if boxInnerH < len(m.categories) {
		boxInnerH = len(m.categories)
	}
	if boxInnerH < 4 {
		boxInnerH = 4
	}

	// ── Left panel: categories with dim Surface background ───────────────
	catLines := make([]string, boxInnerH)
	for i := 0; i < boxInnerH; i++ {
		if i < len(m.categories) {
			cat := m.categories[i]
			prefix := "  "
			if i == m.catCursor {
				prefix = "▶ "
			}
			label := cat.icon + " " + cat.name
			raw := prefix + label
			var style lipgloss.Style
			switch {
			case i == m.catCursor && !m.inCategory:
				style = catActiveStyle
			case i == m.catCursor:
				style = catNormalStyle
			default:
				style = catDimStyle
			}
			catLines[i] = style.Width(leftInnerW).Render(raw)
		} else {
			catLines[i] = leftBgStyle.Width(leftInnerW).Render(" ")
		}
	}
	leftContent := strings.Join(catLines, "\n")
	leftPanel := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Render(leftContent)

	// ── Right panel layout ──────────────────────────────────────────────
	// The right panel has a fixed maximum width so long values (paths,
	// URLs, outputs) can't stretch the page horizontally. Anything that
	// doesn't fit is truncated with an ellipsis (see rendering below).
	const rightOuterMax = 72
	leftOuterW := leftInnerW + 2 // +2 for border
	rightOuterW := m.width - leftOuterW - 4
	if rightOuterW < 24 {
		rightOuterW = 24
	}
	if rightOuterW > rightOuterMax {
		rightOuterW = rightOuterMax
	}
	rightInnerW := rightOuterW - 2 // -2 for border
	// Reserve 1 col for scrollbar and 1 col of gap before it.
	rightListW := rightInnerW - 2
	if rightListW < 10 {
		rightListW = 10
	}

	// Header row (category name) + blank spacer consume 2 rows inside the
	// right box; items + optional description footer fill the rest. Always
	// reserve space for the scrollbar column.
	cat := m.categories[m.catCursor]
	visibleItems := make([]*settingItem, 0, len(cat.items))
	for i := range cat.items {
		if !cat.items[i].hidden {
			visibleItems = append(visibleItems, cat.items[i])
		}
	}

	// Reserve 1 row for category title, 1 blank, 2 for description footer
	// (label + its own blank above it). Remaining rows = item viewport.
	const rightHeaderRows = 2 // title + blank
	const rightFooterRows = 2 // blank + description line
	itemsViewH := boxInnerH - rightHeaderRows - rightFooterRows
	if itemsViewH < 1 {
		itemsViewH = 1
	}

	// Scroll so the focused item is visible (center where possible).
	scroll := 0
	if len(visibleItems) > itemsViewH {
		scroll = m.itemCursor - itemsViewH/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(visibleItems)-itemsViewH {
			scroll = len(visibleItems) - itemsViewH
		}
	}
	barChars := components.ScrollbarChars(scroll, itemsViewH, len(visibleItems), dimStyle)

	// Build right column content rows.
	rightLines := make([]string, 0, boxInnerH)
	rightLines = append(rightLines, padOrTruncate(catActiveStyle.Render("  "+cat.icon+" "+cat.name), rightInnerW))
	rightLines = append(rightLines, strings.Repeat(" ", rightInnerW))

	labelW := rightListW - 14
	if labelW < 10 {
		labelW = 10
	}
	for r := 0; r < itemsViewH; r++ {
		idx := scroll + r
		var rowText string
		if idx < len(visibleItems) {
			item := visibleItems[idx]
			// itemCursor indexes into the original (non-filtered) cat.items slice;
			// match on pointer equality.
			selected := m.inCategory && m.itemCursor < len(cat.items) && cat.items[m.itemCursor] == item
			prefix := "  "
			if selected {
				prefix = "▶ "
			}
			// Label is fixed-width, truncated if somehow longer.
			labelTrunc := item.label
			if len(labelTrunc) > labelW {
				labelTrunc = truncate(labelTrunc, labelW)
			}
			labelPad := fmt.Sprintf("%-*s", labelW, labelTrunc)
			var style lipgloss.Style
			if selected {
				style = itemActiveStyle
			} else {
				style = itemNormalStyle
			}
			// Value gets whatever space is left after the prefix (2) + label.
			valW := rightListW - 2 - labelW
			if valW < 3 {
				valW = 3
			}
			var val string
			if m.editing && selected && item.kind == settingPath {
				// Textinput already sizes itself to its configured width.
				val = m.editInput.View()
			} else {
				raw := item.displayValue()
				if len(raw) > valW {
					raw = truncate(raw, valW)
				}
				val = valStyle.Render(raw)
			}
			rowText = style.Render(prefix+labelPad) + val
		}
		rowText = padOrTruncate(rowText, rightListW)
		// Append gap + scrollbar cell.
		if r < len(barChars) {
			rowText = rowText + " " + barChars[r]
		} else {
			rowText = rowText + "  "
		}
		rightLines = append(rightLines, padOrTruncate(rowText, rightInnerW))
	}

	// Description footer (2 rows) aligned to the selected item.
	var descLine string
	if m.inCategory && m.itemCursor < len(cat.items) {
		desc := cat.items[m.itemCursor].description
		if desc != "" {
			// Reserve "  " prefix, leave the rest for the text itself.
			maxDescW := rightInnerW - 2
			if maxDescW > 0 && len(desc) > maxDescW {
				desc = truncate(desc, maxDescW)
			}
			descLine = valStyle.Render("  " + desc)
		}
	}
	rightLines = append(rightLines, strings.Repeat(" ", rightInnerW))
	rightLines = append(rightLines, padOrTruncate(descLine, rightInnerW))

	// Pad or truncate to boxInnerH so both columns align.
	if len(rightLines) > boxInnerH {
		rightLines = rightLines[:boxInnerH]
	}
	for len(rightLines) < boxInnerH {
		rightLines = append(rightLines, strings.Repeat(" ", rightInnerW))
	}

	rightContent := strings.Join(rightLines, "\n")
	rightPanel := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Render(rightContent)

	// ── Join panels ───────────────────────────────────────────────────────
	body := lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, " ", rightPanel)

	// ── Footer ────────────────────────────────────────────────────────────
	var footer string
	if m.editing {
		footer = hintBar("enter confirm", "esc cancel")
	} else {
		footer = hintBar("↑↓ navigate", "enter select/toggle", "+/- adjust", "← back", "esc exit")
	}

	base := header + "\n\n" + body + "\n\n" + footer + "\n"

	return tea.NewView(base)
}

// padOrTruncate ensures s has an exact visible width of w, padding with
// spaces on the right or truncating (ANSI-aware via lipgloss.Width).
func padOrTruncate(s string, w int) string {
	if w <= 0 {
		return ""
	}
	vis := lipgloss.Width(s)
	if vis == w {
		return s
	}
	if vis < w {
		return s + strings.Repeat(" ", w-vis)
	}
	// Overflow — truncate ANSI-aware by walking runes and skipping escape
	// sequences when counting width. A trailing reset closes any open SGR.
	var out strings.Builder
	visible := 0
	inEscape := false
	for _, r := range s {
		if r == '\x1b' {
			inEscape = true
			out.WriteRune(r)
			continue
		}
		if inEscape {
			out.WriteRune(r)
			if r == 'm' {
				inEscape = false
			}
			continue
		}
		if visible >= w-1 {
			out.WriteRune('…')
			visible++
			break
		}
		out.WriteRune(r)
		visible++
	}
	out.WriteString("\x1b[0m")
	return out.String()
}

// ── Default categories ────────────────────────────────────────────────────────

func defaultCategories() []settingCategory {
	mpdMusicDir, mpdPlaylistDir := detectMpdConfPaths()
	if mpdMusicDir == "" {
		mpdMusicDir = "(not found — check mpd.conf)"
	}
	if mpdPlaylistDir == "" {
		mpdPlaylistDir = "(not found — check mpd.conf)"
	}
	return []settingCategory{
		{
			name: "Audio",
			icon: "🎵",
			items: []*settingItem{
				{
					label:       "DSP Settings",
					key:         "audio.dsp",
					kind:        settingAction,
					description: "Configure DSP, EQ, crossfeed, and other audio processing",
				},
				// ── MPD server ─────────────────────────────────────────────
				{
					label:       "MPD host",
					key:         "mpd.host",
					kind:        settingString,
					strVal:      "127.0.0.1",
					description: "MPD server hostname or IP address",
				},
				{
					label:       "MPD port",
					key:         "mpd.port",
					kind:        settingInt,
					intVal:      6600,
					minVal:      1,
					maxVal:      65535,
					description: "MPD TCP port (1–65535)",
				},
				{
					label:       "MPD password",
					key:         "mpd.password",
					kind:        settingInfo,
					description: "Edit stui.toml to set — sensitive",
				},
				{
					label:       "ReplayGain",
					key:         "mpd.replay_gain",
					kind:        settingChoice,
					choiceVals:  []string{"auto", "track", "album", "off"},
					choiceIdx:   0,
					description: "ReplayGain mode: auto (recommended), track, album, or off",
				},
				{
					label:       "Crossfade (secs)",
					key:         "mpd.crossfade_secs",
					kind:        settingInt,
					intVal:      0,
					minVal:      0,
					maxVal:      30,
					description: "Crossfade duration between tracks (0 = gapless/off, max 30)",
				},
				{
					label:       "MixRamp dB",
					key:         "mpd.mixramp_db",
					kind:        settingFloat,
					floatVal:    0.0,
					description: "MixRamp threshold for gapless (0 = disabled; try -6.0)",
				},
				{
					label:       "Consume mode",
					key:         "mpd.consume",
					kind:        settingBool,
					boolVal:     false,
					description: "Remove tracks from queue after playing",
				},
				{
					label:       "Music directory",
					key:         "mpd.music_dir",
					kind:        settingInfo,
					strVal:      mpdMusicDir,
					description: "Auto-detected from mpd.conf (or set in stui.toml)",
				},
				{
					label:       "Playlist directory",
					key:         "mpd.playlist_dir",
					kind:        settingInfo,
					strVal:      mpdPlaylistDir,
					description: "Where MPD stores .m3u playlists (auto-detected from mpd.conf)",
				},
				{
					label:       "MPD outputs",
					key:         "mpd.outputs",
					kind:        settingInfo,
					description: "MPD outputs list (view in Now Playing screen)",
				},
				{
					label:       "MPD status",
					key:         "mpd.status",
					kind:        settingInfo,
					description: "MPD connection status (connected/disconnected)",
				},
			},
		},
		{
			name: "Playback",
			icon: "▶",
			items: []*settingItem{
				{
					label:       "Volume",
					key:         "player.default_volume",
					kind:        settingInt,
					intVal:      100,
					description: "Default volume on startup (0–130)",
				},
				{
					label:       "Hardware decode",
					key:         "player.hwdec",
					kind:        settingChoice,
					choiceVals:  []string{"auto", "vaapi", "nvdec", "videotoolbox", "no"},
					choiceIdx:   0,
					description: "GPU-accelerated video decoding backend",
				},
				{
					label:       "Cache (secs)",
					key:         "player.cache_secs",
					kind:        settingInt,
					intVal:      20,
					description: "Read-ahead network cache in seconds of video",
				},
				{
					label:       "Keep open",
					key:         "player.keep_open",
					kind:        settingBool,
					boolVal:     false,
					description: "Keep mpv open after playback ends",
				},
				{
					label:       "Auto-play next episode",
					key:         "playback.autoplay_next",
					kind:        settingBool,
					boolVal:     false,
					description: "Automatically play the next episode when one finishes",
				},
				{
					label:       "Auto-play countdown",
					key:         "playback.autoplay_countdown",
					kind:        settingInt,
					intVal:      5,
					minVal:      3,
					maxVal:      30,
					description: "Seconds to wait before auto-playing the next episode (3–30)",
				},
				{
					label:       "Pre-roll buffer",
					key:         "player.min_preroll_secs",
					kind:        settingInt,
					intVal:      3,
					minVal:      0,
					maxVal:      10,
					description: "Minimum pre-roll before playback (0–10 secs, 0=auto)",
				},
				{
					label:       "Demux buffer (MB)",
					key:         "player.demuxer_max_mb",
					kind:        settingInt,
					intVal:      200,
					minVal:      50,
					maxVal:      1000,
					description: "Maximum demuxer buffer size (50–1000 MB)",
				},
				{
					label:       "Terminal video",
					key:         "player.terminal_vo",
					kind:        settingChoice,
					choiceVals:  []string{"", "kitty", "sixel", "tct", "chafa"},
					choiceIdx:   0,
					description: "Inline video rendering (empty=default window)",
				},
			},
		},
		{
			name: "Streaming",
			icon: "⚡",
			items: []*settingItem{
				{
					label:       "Prefer HTTP",
					key:         "streaming.prefer_http",
					kind:        settingBool,
					boolVal:     true,
					description: "Prefer direct HTTP streams over torrents",
				},
				{
					label:       "Auto fallback",
					key:         "streaming.auto_fallback",
					kind:        settingBool,
					boolVal:     true,
					description: "Auto-switch to next stream if current fails",
				},
				{
					label:       "Max candidates",
					key:         "streaming.max_candidates",
					kind:        settingInt,
					intVal:      10,
					description: "Max stream candidates resolved per item",
				},
				{
					label:       "Benchmark",
					key:         "streaming.benchmark_streams",
					kind:        settingBool,
					boolVal:     false,
					description: "Probe latency before selecting (adds ~1–2s)",
				},
				{
					label:       "Auto-delete video",
					key:         "streaming.auto_delete_video",
					kind:        settingBool,
					boolVal:     true,
					description: "Delete cached video stream after the movie/episode is fully watched",
				},
				{
					label:       "Auto-delete audio",
					key:         "streaming.auto_delete_audio",
					kind:        settingBool,
					boolVal:     false,
					description: "Delete cached audio stream after it is fully played",
				},
			},
		},
		{
			name: "Subtitles",
			icon: "💬",
			items: []*settingItem{
				{
					label:       "Auto download",
					key:         "subtitles.auto_download",
					kind:        settingBool,
					boolVal:     false,
					description: "Auto-fetch subtitles from OpenSubtitles",
				},
				{
					label:       "Language",
					key:         "subtitles.preferred_language",
					kind:        settingChoice,
					choiceVals:  []string{"eng", "fra", "spa", "deu", "ita", "por", "jpn", "zho"},
					choiceIdx:   0,
					description: "Preferred subtitle language (ISO 639-2)",
				},
				{
					label:       "Default delay",
					key:         "subtitles.default_delay",
					kind:        settingFloat,
					floatVal:    0.0,
					description: "Default sub-delay in seconds (+ = later)",
				},
			},
		},
		{
			name: "Providers",
			icon: "🔌",
			items: []*settingItem{
				{
					label:       "Configure API Keys",
					key:         "providers.open_settings",
					kind:        settingAction,
					description: "Enter API keys for TMDB, OMDB, and other providers",
				},
				{
					label:       "TMDB",
					key:         "providers.enable_tmdb",
					kind:        settingBool,
					boolVal:     true,
					description: "TMDB metadata (requires API key — configure above)",
				},
				{
					label:       "OMDB",
					key:         "providers.enable_omdb",
					kind:        settingBool,
					boolVal:     false,
					description: "OMDB metadata fallback (requires API key — configure above)",
				},
				{
					label:       "Torrentio",
					key:         "providers.enable_torrentio",
					kind:        settingBool,
					boolVal:     true,
					description: "Torrent streams via Torrentio RPC plugin",
				},
				{
					label:       "Prowlarr",
					key:         "providers.enable_prowlarr",
					kind:        settingBool,
					boolVal:     false,
					description: "Prowlarr indexer (requires PROWLARR_URL)",
				},
				{
					label:       "OpenSubtitles",
					key:         "providers.enable_opensubtitles",
					kind:        settingBool,
					boolVal:     false,
					description: "OpenSubtitles (requires OS_API_KEY)",
				},
			},
		},
		{
			name: "Notifications",
			icon: "🔔",
			items: []*settingItem{
				{
					label:       "Enabled",
					key:         "notifications.enabled",
					kind:        settingBool,
					boolVal:     true,
					description: "Send desktop notifications (requires notify-send or dunstctl)",
				},
				{
					label:       "Backend",
					key:         "notifications.backend",
					kind:        settingChoice,
					choiceVals:  []string{"auto", "notify-send", "dunstctl", "off"},
					choiceIdx:   0,
					description: "Notification daemon: auto (detect), notify-send, dunstctl, or off",
				},
				{
					label:       "On playback start",
					key:         "notifications.on_playback",
					kind:        settingBool,
					boolVal:     true,
					description: "Notify when mpv starts playing a title",
				},
				{
					label:       "On download done",
					key:         "notifications.on_download",
					kind:        settingBool,
					boolVal:     true,
					description: "Notify when a torrent download finishes",
				},
				{
					label:       "On streams found",
					key:         "notifications.on_streams",
					kind:        settingBool,
					boolVal:     false,
					description: "Notify when stream candidates are resolved (can be noisy)",
				},
			},
		},
		{
			name: "Interface",
			icon: "🖥",
			items: []*settingItem{
				{
					label:       "Theme",
					key:         "interface.theme",
					kind:        settingChoice,
					choiceVals:  config.ListThemes(),
					choiceIdx:   0,
					description: "Active colour theme (built-in or from ~/.config/stui/themes/)",
				},
				{
					label:       "Theme",
					key:         "app.theme_mode",
					kind:        settingChoice,
					choiceVals:  []string{"dark", "light"},
					choiceIdx:   0,
					description: "Matugen mode — only used when Theme = matugen",
				},
				{
					label:       "Show borders",
					key:         "ui.show_borders",
					kind:        settingBool,
					boolVal:     true,
					description: "Draw borders around panels",
				},
				{
					label:       "Mouse support",
					key:         "ui.mouse_support",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable mouse click and scroll support",
				},
				{
					label:       "BiDi text",
					key:         "ui.bidi_mode",
					kind:        settingChoice,
					choiceVals:  []string{"auto", "force", "off"},
					choiceIdx:   0,
					description: "Bidirectional text: auto=alignment only (terminal handles), force=full in-app reordering, off=disabled",
				},
				{
					label:       "Keybinds",
					key:         "keybinds.edit",
					kind:        settingAction,
					description: "Edit keyboard shortcuts",
				},
			},
		},
		{
			name: "Skip Detection",
			icon: "⏭",
			items: []*settingItem{
				{
					label:       "Enabled",
					key:         "skipper.enabled",
					kind:        settingBool,
					boolVal:     true,
					description: "Detect and skip intros/credits using audio fingerprinting (requires FFmpeg+Chromaprint)",
				},
				{
					label:       "Auto-skip intro",
					key:         "skipper.auto_skip_intro",
					kind:        settingBool,
					boolVal:     false,
					description: "Automatically skip detected intros (Netflix-style)",
				},
				{
					label:       "Auto-skip credits",
					key:         "skipper.auto_skip_credits",
					kind:        settingBool,
					boolVal:     false,
					description: "Automatically skip detected end credits",
				},
				{
					label:       "Intro scan (secs)",
					key:         "skipper.intro_scan_secs",
					kind:        settingInt,
					intVal:      300,
					description: "Seconds of audio to analyze from start of video (for intro detection)",
				},
				{
					label:       "Min intro (secs)",
					key:         "skipper.min_intro_secs",
					kind:        settingInt,
					intVal:      20,
					description: "Minimum intro duration to accept (shorter matches ignored)",
				},
				{
					label:       "Max intro (secs)",
					key:         "skipper.max_intro_secs",
					kind:        settingInt,
					intVal:      120,
					description: "Maximum intro duration (longer matches ignored)",
				},
				{
					label:       "Similarity",
					key:         "skipper.similarity_threshold",
					kind:        settingFloat,
					floatVal:    0.85,
					description: "Fingerprint match threshold 0.0–1.0 (higher = stricter; 0.85 recommended)",
				},
				{
					label:       "Min episodes",
					key:         "skipper.min_episodes",
					kind:        settingInt,
					intVal:      2,
					description: "Episodes needed before comparison runs (more = more accurate)",
				},
			},
		},
		{
			name: "Visualizer",
			icon: "\U0001f308", // 🌈
			items: []*settingItem{
				{
					label:       "Backend",
					key:         "visualizer.backend",
					kind:        settingChoice,
					choiceVals:  []string{"off", "cliamp", "cava", "chroma"},
					choiceIdx:   1,
					description: "Visualizer engine — cliamp is built-in (no deps); cava/chroma need external binaries",
				},
				{
					label:       "Bars",
					key:         "visualizer.bars",
					kind:        settingInt,
					intVal:      20,
					description: "Number of frequency bars to display (10–60)",
				},
				{
					label:       "Height",
					key:         "visualizer.height",
					kind:        settingInt,
					intVal:      8,
					description: "Visualizer height in terminal rows (4–20)",
				},
				{
					label:       "Framerate",
					key:         "visualizer.framerate",
					kind:        settingInt,
					intVal:      20,
					description: "Target animation framerate in fps (10–60)",
				},
				{
					label: "Mode",
					key:   "visualizer.mode",
					kind:  settingChoice,
					choiceVals: []string{
						"wave", "scope", "retro", "matrix", "flame", "pulse",
						"binary", "butterfly", "terrain", "sakura", "firework",
						"glitch", "lightning", "rain", "scatter", "columns", "bricks",
						"bars", "mirror", "filled", "led",
					},
					choiceIdx:   0,
					description: "Visualization style — cliamp modes (wave…bricks) need no extra binary; classic modes (bars…led) use the backend subprocess",
				},
				{
					label:       "Peak hold",
					key:         "visualizer.peak_hold",
					kind:        settingBool,
					boolVal:     true,
					description: "Show peak hold indicators on bars",
				},
				{
					label:       "Gradient",
					key:         "visualizer.gradient",
					kind:        settingBool,
					boolVal:     true,
					description: "Shade bars from accent colour (top) to dim (bottom)",
				},
				{
					label:       "Input method",
					key:         "visualizer.input_method",
					kind:        settingChoice,
					choiceVals:  []string{"pulse", "pipewire", "alsa"},
					choiceIdx:   0,
					description: "Audio input method for cava: pulse, pipewire, or alsa",
				},
			},
		},
		{
			name: "Plugins",
			icon: "🧩",
			items: []*settingItem{
				{
					label:       "Plugin directory",
					key:         "app.plugin_dir",
					kind:        settingInfo,
					description: "~/.stui/plugins  (edit stui.toml to change)",
				},
				{
					label:       "Hot reload",
					key:         "app.plugin_hot_reload",
					kind:        settingBool,
					boolVal:     true,
					description: "Watch plugin dir and load new plugins live",
				},
				{
					label:       "Plugin Manager",
					key:         "plugins.manager",
					kind:        settingAction,
					description: "Install, unload, and update plugins (Installed / Available / Updates)",
				},
				{
					label:       "Manage Repos",
					key:         "plugins.manage_repos",
					kind:        settingAction,
					description: "Add or remove community plugin repository URLs",
				},
			},
		},
		{
			name: "Library",
			icon: "📚",
			items: []*settingItem{
				// ── Library roots (where organised media lives) ─────────────
				{
					label:       "Movies directory",
					key:         "storage.movies",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos", "Movies"),
					description: "Where organised movie files are stored",
				},
				{
					label:       "Series directory",
					key:         "storage.series",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos", "Series"),
					description: "Where organised TV series files are stored",
				},
				{
					label:       "Anime directory",
					key:         "storage.anime",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos", "Anime"),
					description: "Where organised anime files are stored",
				},
				{
					label:       "Music directory",
					key:         "storage.music",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Music"),
					description: "Music root scanned by MPD for the Library tab",
				},
				{
					label:       "Podcasts directory",
					key:         "storage.podcasts",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Music", "Podcasts"),
					description: "Where podcast episodes are stored",
				},
				// Download targets are derived from the library directories
				// above — no separate "downloads.*" keys (they were redundant).
			},
		},
		{
			name: "Stats for Nerds",
			icon: "📊",
			items: []*settingItem{
				{
					label:       "Stream Radar",
					key:         "stats.stream_radar",
					kind:        settingAction,
					description: "Resolution / provider / protocol histogram for all streams resolved this session",
				},
				{
					label:       "Rating Weights",
					key:         "stats.rating_weights",
					kind:        settingAction,
					description: "Source weight ratios used by the weighted-median rating aggregator",
				},
				{
					label:       "Offline Library",
					key:         "stats.offline_library",
					kind:        settingAction,
					description: "Browse locally cached catalog — works without network (cache auto-updates on each live fetch)",
				},
				{
					label:       "Clear Cache",
					key:         "stats.clear_cache",
					kind:        settingAction,
					description: "Delete the local media cache (mediacache.json) — next launch will refetch from providers",
				},
			},
		},
		{
			name: "Accessibility",
			icon: "♿",
			items: []*settingItem{
				{
					label:       "Color scheme",
					key:         "ui.color_scheme",
					kind:        settingChoice,
					choiceVals:  []string{"default", "high-contrast", "monochrome"},
					choiceIdx:   0,
					description: "Color palette: default, high-contrast (for low vision), or monochrome",
				},
				{
					label:       "Reduced motion",
					key:         "ui.reduced_motion",
					kind:        settingBool,
					boolVal:     false,
					description: "Disable animations, spinners, and transitions",
				},
				{
					label:       "Screen reader mode",
					key:         "ui.screen_reader",
					kind:        settingBool,
					boolVal:     false,
					description: "Optimize output for screen readers (plain text, no colors)",
				},
			},
		},
		{
			name: "Developer",
			icon: "🔧",
			items: []*settingItem{
				{
					label:       "Debug mode",
					key:         "app.debug_mode",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable verbose IPC tracing and debug-level runtime logs (takes full effect on restart)",
				},
				{
					label:       "Enable tests",
					key:         "app.tests_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Run built-in self-tests at runtime startup to verify subsystem health",
				},
			},
		},
	}
}
