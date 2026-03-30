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
	"github.com/stui/stui/internal/ui/screen"
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

func NewSettingsModel(client *ipc.Client) SettingsModel {
	return SettingsModel{
		categories: defaultCategories(),
		client:     client,
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
			// Handle click events
			if clickMsg, ok := msg.(tea.MouseClickMsg); ok && clickMsg.Button == tea.MouseLeft {
				// Layout: header at row 0, blank at row 1, body at row 2+.
				// Left panel is leftW=18 wide with PaddingLeft(1).
				const leftPanelW = 19 // 18 width + 1 padding
				bodyRow := mouse.Y - 2
				if bodyRow < 0 {
					break
				}
				if mouse.X < leftPanelW+2 {
					// Left panel: category click.
					if bodyRow < len(m.categories) {
						m.catCursor = bodyRow
						m.inCategory = false
						m.itemCursor = 0
					}
				} else {
					// Right panel: rows 0=cat header, 1=blank, 2+=items.
					itemRow := bodyRow - 2
					if itemRow >= 0 {
						cat := m.categories[m.catCursor]
						if itemRow < len(cat.items) {
							m.inCategory = true
							m.itemCursor = itemRow
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
					m.itemCursor = 0
				}
			} else {
				if m.itemCursor > 0 {
					m.itemCursor--
				}
			}

		case "down", "j":
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

		case "right", "l":
			if !m.inCategory && len(m.categories[m.catCursor].items) > 0 {
				m.inCategory = true
				m.itemCursor = 0
			}

		case "enter":
			if !m.inCategory && len(m.categories[m.catCursor].items) > 0 {
				m.inCategory = true
				m.itemCursor = 0
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
				cat := m.categories[m.catCursor]
				if m.itemCursor < len(cat.items) {
					cat.items[m.itemCursor].adjust(+1)
					return m, settingChangedCmd(cat.items[m.itemCursor])
				}
			}

		case "-", "_":
			if m.inCategory {
				cat := m.categories[m.catCursor]
				if m.itemCursor < len(cat.items) {
					cat.items[m.itemCursor].adjust(-1)
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

	// Styles
	headerStyle := lipgloss.NewStyle().
		Bold(true).
		Foreground(theme.T.Accent()).
		PaddingLeft(2)

	catActiveStyle := lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Bold(true)

	catNormalStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text())

	catDimStyle := lipgloss.NewStyle().
		Foreground(theme.T.TextDim())

	itemActiveStyle := lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Bold(true)

	itemNormalStyle := lipgloss.NewStyle().
		Foreground(theme.T.Text())

	valStyle := lipgloss.NewStyle().
		Foreground(theme.T.TextDim())

	// ── Header ──────────────────────────────────────────────────────────
	header := headerStyle.Render("⚙  Settings")

	// ── Left: categories ─────────────────────────────────────────────────
	leftW := 18
	var catLines []string
	for i, cat := range m.categories {
		prefix := "  "
		if i == m.catCursor {
			prefix = "▶ "
		}
		label := cat.icon + " " + cat.name
		var style lipgloss.Style
		switch {
		case i == m.catCursor && !m.inCategory:
			style = catActiveStyle
		case i == m.catCursor:
			style = catNormalStyle
		default:
			style = catDimStyle
		}
		catLines = append(catLines, style.Render(prefix+label))
	}
	leftPanel := lipgloss.NewStyle().
		Width(leftW).
		PaddingLeft(1).
		Render(strings.Join(catLines, "\n"))

	// ── Right: items ──────────────────────────────────────────────────────
	rightW := m.width - leftW - 6
	if rightW < 20 {
		rightW = 20
	}

	cat := m.categories[m.catCursor]
	var itemLines []string
	itemLines = append(itemLines, catActiveStyle.Render("  "+cat.icon+" "+cat.name))
	itemLines = append(itemLines, "")

	for i, item := range cat.items {
		prefix := "  "
		if m.inCategory && i == m.itemCursor {
			prefix = "▶ "
		}

		labelW := rightW - 14
		if labelW < 10 {
			labelW = 10
		}
		labelPad := fmt.Sprintf("%-*s", labelW, item.label)

		var style lipgloss.Style
		if m.inCategory && i == m.itemCursor {
			style = itemActiveStyle
		} else {
			style = itemNormalStyle
		}

		var val string
		if m.editing && i == m.itemCursor && item.kind == settingPath {
			// Render the live textinput instead of the plain value.
			val = m.editInput.View()
		} else {
			val = valStyle.Render(item.displayValue())
		}
		line := style.Render(prefix+labelPad) + val
		itemLines = append(itemLines, line)
	}

	// Footer hint for focused item
	if m.inCategory && m.itemCursor < len(cat.items) {
		item := cat.items[m.itemCursor]
		if item.description != "" {
			itemLines = append(itemLines, "")
			itemLines = append(itemLines, valStyle.Render("  "+item.description))
		}
	}

	rightPanel := lipgloss.NewStyle().
		Width(rightW).
		PaddingLeft(2).
		Render(strings.Join(itemLines, "\n"))

	// ── Join panels ───────────────────────────────────────────────────────
	body := lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, rightPanel)

	// ── Footer ────────────────────────────────────────────────────────────
	var footer string
	if m.editing {
		footer = hintBar("enter confirm", "esc cancel")
	} else {
		footer = hintBar("↑↓ navigate", "enter select/toggle", "+/- adjust", "← back", "esc exit")
	}

	return tea.NewView(header + "\n\n" + body + "\n\n" + footer + "\n")
}

// ── Default categories ────────────────────────────────────────────────────────

func defaultCategories() []settingCategory {
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
			name: "Downloads",
			icon: "⬇",
			items: []*settingItem{
				{
					label:       "Video download dir",
					key:         "downloads.video_dir",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos"),
					description: "Directory for movie and series downloads (enter to edit)",
				},
				{
					label:       "Music download dir",
					key:         "downloads.music_dir",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Music"),
					description: "Directory for music and audio downloads (enter to edit)",
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
					key:         "app.theme_mode",
					kind:        settingChoice,
					choiceVals:  []string{"dark", "light"},
					choiceIdx:   0,
					description: "Colour theme (restart may be needed)",
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
			name: "MPD Audio",
			icon: "♪",
			items: []*settingItem{
				{
					label:       "Host",
					key:         "mpd.host",
					kind:        settingString,
					strVal:      "127.0.0.1",
					description: "MPD server hostname or IP address",
				},
				{
					label:       "Port",
					key:         "mpd.port",
					kind:        settingInt,
					intVal:      6600,
					minVal:      1,
					maxVal:      65535,
					description: "MPD TCP port (1–65535)",
				},
				{
					label:       "Password",
					key:         "mpd.password",
					kind:        settingInfo,
					description: "MPD password (edit stui.toml to set — sensitive)",
				},
				{
					label:       "Music directory",
					key:         "mpd.music_dir",
					kind:        settingInfo,
					description: "MPD music root (edit stui.toml to enable library browse)",
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
					label:       "Outputs",
					key:         "mpd.outputs",
					kind:        settingInfo,
					description: "MPD outputs list (view in Now Playing screen)",
				},
				{
					label:       "Status",
					key:         "mpd.status",
					kind:        settingInfo,
					description: "MPD connection status (connected/disconnected)",
				},
				// ── Visualizer ───────────────────────────────────────────────
				{
					label:       "Viz backend",
					key:         "visualizer.backend",
					kind:        settingChoice,
					choiceVals:  []string{"off", "cava", "chroma"},
					choiceIdx:   0,
					description: "Frequency visualizer: off/cava/chroma (install with: cargo install chroma --features audio)",
				},
				{
					label:       "Viz bars",
					key:         "visualizer.bars",
					kind:        settingInt,
					intVal:      20,
					description: "Number of frequency bars to display (10–60)",
				},
				{
					label:       "Viz height",
					key:         "visualizer.height",
					kind:        settingInt,
					intVal:      8,
					description: "Visualizer height in terminal rows (4–20)",
				},
				{
					label:       "Viz framerate",
					key:         "visualizer.framerate",
					kind:        settingInt,
					intVal:      20,
					description: "Target animation framerate in fps (10–60)",
				},
				{
					label:       "Viz mode",
					key:         "visualizer.mode",
					kind:        settingChoice,
					choiceVals:  []string{"bars", "mirror", "filled", "led"},
					choiceIdx:   0,
					description: "Visualization style: bars, mirror, filled, or led",
				},
				{
					label:       "Viz peak hold",
					key:         "visualizer.peak_hold",
					kind:        settingBool,
					boolVal:     true,
					description: "Show peak hold indicators on bars",
				},
				{
					label:       "Viz gradient",
					key:         "visualizer.gradient",
					kind:        settingBool,
					boolVal:     true,
					description: "Shade bars from accent colour (top) to dim (bottom)",
				},
				{
					label:       "Viz input",
					key:         "visualizer.input_method",
					kind:        settingChoice,
					choiceVals:  []string{"pulse", "pipewire", "alsa"},
					choiceIdx:   0,
					description: "Audio input method for cava: pulse, pipewire, or alsa",
				},
			},
		},
		{
			name: "DSP Audio",
			icon: "\U0001f3a7", // 🎧
			items: []*settingItem{
				{
					label:       "Enable DSP",
					key:         "dsp.enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Enable high-quality audio processing (upsampling, DSD→PCM, convolution)",
				},
				{
					label:       "Output sample rate",
					key:         "dsp.output_sample_rate",
					kind:        settingInt,
					intVal:      192000,
					description: "Target output sample rate (44100–384000)",
				},
				{
					label:       "Upsample ratio",
					key:         "dsp.upsample_ratio",
					kind:        settingChoice,
					choiceVals:  []string{"1", "2", "4", "8"},
					choiceIdx:   2,
					description: "Upsampling multiplier: 1× (off), 2×, 4×, 8×",
				},
				{
					label:       "Filter type",
					key:         "dsp.filter_type",
					kind:        settingChoice,
					choiceVals:  []string{"fast", "slow", "synchronous"},
					choiceIdx:   2,
					description: "Resampling filter: fast (low latency), slow (higher quality), synchronous (default)",
				},
				{
					label:       "Output mode",
					key:         "dsp.output_mode",
					kind:        settingChoice,
					choiceVals:  []string{"pcm", "dsd", "dsd_to_pcm"},
					choiceIdx:   0,
					description: "Output format: PCM, DSD (native), or DSD→PCM",
				},
				{
					label:       "Output target",
					key:         "dsp.output_target",
					kind:        settingChoice,
					choiceVals:  []string{"pipewire", "alsa", "roon_raat", "mpd"},
					choiceIdx:   0,
					description: "Audio output: PipeWire (default), ALSA direct hw:, Roon RAAT, or MPD",
				},
				{
					label:       "ALSA device",
					key:         "dsp.alsa_device",
					kind:        settingString,
					description: "ALSA hardware device for bit-perfect output (e.g. hw:0,0). Leave empty to use default.",
				},
				{
					label:       "PipeWire role",
					key:         "dsp.pipewire_role",
					kind:        settingChoice,
					choiceVals:  []string{"Music", "Production"},
					choiceIdx:   0,
					description: "PipeWire stream role. Production bypasses WirePlumber resampling (requires WirePlumber ≥ 0.4).",
				},
				{
					label:       "DSD→PCM",
					key:         "dsp.dsd_to_pcm_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Convert DSD audio to PCM (required for most DACs)",
				},
				{
					label:       "Convolution",
					key:         "dsp.convolution_enabled",
					kind:        settingBool,
					boolVal:     false,
					description: "Apply room correction filter (requires filter file path)",
				},
				{
					label:       "Filter path",
					key:         "dsp.convolution_filter_path",
					kind:        settingPath,
					description: "Path to convolution filter WAV file (room correction)",
				},
				{
					label:       "Conv bypass",
					key:         "dsp.convolution_bypass",
					kind:        settingBool,
					boolVal:     true,
					description: "Bypass convolution filter (keep enabled for quick toggle)",
				},
				{
					label:       "Crossfeed",
					key:         "dsp.crossfeed_enabled",
					kind:        settingAction,
					description: "BS2B headphone crossfeed — blend L/R for natural stereo image",
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
			name: "Storage",
			icon: "💾",
			items: []*settingItem{
				{
					label:       "Movies folder",
					key:         "storage.movies",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos", "Movies"),
					description: "Where organized movie files are stored",
				},
				{
					label:       "Series folder",
					key:         "storage.series",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos", "Series"),
					description: "Where organized TV series files are stored",
				},
				{
					label:       "Anime folder",
					key:         "storage.anime",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Videos", "Anime"),
					description: "Where organized anime files are stored",
				},
				{
					label:       "Music folder",
					key:         "storage.music",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Music"),
					description: "Where organized music files are stored",
				},
				{
					label:       "Podcasts folder",
					key:         "storage.podcasts",
					kind:        settingPath,
					strVal:      filepath.Join(settingsHomeDir, "Music", "Podcasts"),
					description: "Where podcast episodes are stored",
				},
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
