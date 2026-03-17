package screens

// help.go — HelpScreen: full keybinding reference.

import (
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// HelpScreen shows the full keybinding reference, grouped by category.
// Built from actions.GroupedHelp() so it always stays in sync with keybinds.go.
//
// To open: screen.TransitionCmd(NewHelpScreen(), true)
type HelpScreen struct {
	width  int
	height int
}

func NewHelpScreen() HelpScreen { return HelpScreen{} }

func (h HelpScreen) Init() tea.Cmd { return nil }

func (h HelpScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	if ws, ok := msg.(tea.WindowSizeMsg); ok {
		h.width = ws.Width
		h.height = ws.Height
	}
	return h, nil
}

func (h HelpScreen) View() string {
	accent   := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	normal   := lipgloss.NewStyle().Foreground(theme.T.Text())
	keyStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Width(18)

	var sb strings.Builder
	sb.WriteString("\n  " + accent.Render("? Help \u2014 Keybindings") + "\n\n")

	for _, group := range actions.GroupedHelp() {
		sb.WriteString("  " + accent.Render(group.Title) + "\n")
		for _, row := range group.Rows {
			sb.WriteString("    " + keyStyle.Render(row.Key) + normal.Render(row.Desc) + "\n")
		}
		sb.WriteString("\n")
	}

	sb.WriteString(hintBar("esc close") + "\n")
	return sb.String()
}
