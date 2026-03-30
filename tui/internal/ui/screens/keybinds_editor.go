package screens

// keybinds_editor.go — interactive keybind remapping screen.
//
// Layout:
//
//	┌──────────────────────────────────────────────────────┐
//	│  ⌨  Keybinds                                         │
//	├──────────────────────────────────────────────────────┤
//	│  Navigation                                          │
//	│    Move up              up  k                        │
//	│  ▶ Move down            down  j          [custom]    │
//	│    Move left            left  h                      │
//	│  ...                                                 │
//	├──────────────────────────────────────────────────────┤
//	│  ↑↓ navigate  enter rebind  r reset  R reset all     │
//	│  esc back                                            │
//	└──────────────────────────────────────────────────────┘
//
// When the user presses Enter on a row, the screen enters capture mode.
// The very next key event (except Esc) becomes the new binding for that action.
// Changes are persisted to ~/.config/stui/keybinds.json immediately.

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/keybinds"
	"github.com/stui/stui/pkg/theme"
)

// OpenKeybindsEditorMsg triggers navigation to the keybinds editor screen.
type OpenKeybindsEditorMsg struct{}

// ── flat list entry ───────────────────────────────────────────────────────────

type kbEntry struct {
	action actions.ActionDef
	group  string // "" = item, non-empty = section header (action.Action == ActionNone)
}

// KeybindsEditorScreen lists every action and lets the user rebind them.
type KeybindsEditorScreen struct {
	Dims
	entries  []kbEntry // flat list: header rows + action rows
	cursor   int       // index into entries (only action rows are selectable)
	capture  bool      // waiting for user to press a key
	savePath string
}

func NewKeybindsEditorScreen() KeybindsEditorScreen {
	s := KeybindsEditorScreen{
		savePath: keybinds.DefaultPath(),
	}
	s.entries = buildEntries()
	return s
}

// buildEntries flattens GroupedActions into a list of header + item rows.
func buildEntries() []kbEntry {
	var list []kbEntry
	for _, grp := range actions.GroupedActions() {
		list = append(list, kbEntry{group: grp.Title})
		for _, item := range grp.Items {
			list = append(list, kbEntry{action: item})
		}
	}
	return list
}

// isSelectable reports whether the entry at index i can be focused.
func (s KeybindsEditorScreen) isSelectable(i int) bool {
	return i >= 0 && i < len(s.entries) && s.entries[i].group == ""
}

func (s KeybindsEditorScreen) nextSelectable(from, dir int) int {
	i := from + dir
	for i >= 0 && i < len(s.entries) {
		if s.isSelectable(i) {
			return i
		}
		i += dir
	}
	return from
}

func (s KeybindsEditorScreen) Init() tea.Cmd { return nil }

func (s KeybindsEditorScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {
	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case tea.KeyPressMsg:
		key := m.String()

		// ── Capture mode: next key becomes the binding ────────────────────
		if s.capture {
			if key == "esc" {
				s.capture = false
				return s, nil
			}
			// Apply the binding
			entry := &s.entries[s.cursor]
			overrides := actions.BindAction(entry.action.Action, []string{key})
			_ = keybinds.Save(s.savePath, keybinds.UserBindings(overrides))
			s.capture = false
			return s, nil
		}

		// ── Normal navigation ─────────────────────────────────────────────
		switch key {
		case "up", "k":
			s.cursor = s.nextSelectable(s.cursor, -1)
		case "down", "j":
			s.cursor = s.nextSelectable(s.cursor, +1)
		case "enter", " ":
			if s.isSelectable(s.cursor) {
				s.capture = true
			}
		case "r":
			// Reset selected action to default
			if s.isSelectable(s.cursor) {
				overrides := actions.BindAction(s.entries[s.cursor].action.Action, nil)
				_ = keybinds.Save(s.savePath, keybinds.UserBindings(overrides))
			}
		case "R":
			// Reset ALL to defaults
			actions.SetUserBindings(nil)
			_ = keybinds.Save(s.savePath, keybinds.UserBindings{})
		case "esc":
			return s, func() tea.Msg { return screen.PopMsg{} }
		}
	}
	return s, nil
}

func (s KeybindsEditorScreen) View() tea.View {
	accent  := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim     := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	normal  := lipgloss.NewStyle().Foreground(theme.T.Text())
	header  := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	custom  := lipgloss.NewStyle().Foreground(theme.T.Accent())
	capture := lipgloss.NewStyle().Foreground(theme.T.Yellow()).Bold(true)

	var sb strings.Builder
	sb.WriteString("\n  " + accent.Render("⌨  Keybinds") + "\n\n")

	for i, e := range s.entries {
		// Section header
		if e.group != "" {
			sb.WriteString("  " + header.Render(e.group) + "\n")
			continue
		}

		isSelected := i == s.cursor
		prefix := "  "
		descStyle := normal
		if isSelected {
			prefix = "▶ "
			descStyle = accent
		}

		desc := fmt.Sprintf("%-26s", e.action.Desc)

		var keysStr string
		if isSelected && s.capture {
			keysStr = capture.Render("[ press key… ]")
		} else {
			currentKeys := actions.ActionKeys(e.action.Action)
			keysStr = dim.Render(strings.Join(currentKeys, "  "))
			if actions.IsOverridden(e.action.Action) {
				keysStr += "  " + custom.Render("*")
			}
		}

		sb.WriteString("  " + prefix + descStyle.Render(desc) + "  " + keysStr + "\n")
	}

	sb.WriteString("\n")
	var footer string
	if s.capture {
		footer = hintBar("Press any key to bind", "esc to cancel")
	} else {
		footer = hintBar("↑↓ navigate", "enter rebind", "r reset", "R reset all", "esc back")
	}
	sb.WriteString(footer + "\n")
	return tea.NewView(sb.String())
}
