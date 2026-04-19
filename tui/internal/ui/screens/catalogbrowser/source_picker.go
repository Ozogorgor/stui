package catalogbrowser

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/pkg/theme"
)

// SourcePicker is a modal for selecting one of N source-equivalent entries.
// Shown when a user Enters on a row in CatalogBrowser whose grouped entries
// span multiple sources (e.g., "Creep" returned by both Spotify and
// SoundCloud plugins).
//
// Single-column list with up/down navigation, Enter selects, Esc cancels.
type SourcePicker struct {
	title      string  // e.g., "Creep — Radiohead"
	candidates []Entry // each Entry's Source distinguishes it
	cursor     int
}

// NewSourcePicker constructs a picker over the given title and candidates.
// Candidates should already be deduped to one per source by the caller.
func NewSourcePicker(title string, candidates []Entry) SourcePicker {
	return SourcePicker{title: title, candidates: candidates}
}

// SourceSelectedMsg is posted when the user confirms a selection.
type SourceSelectedMsg struct {
	Entry Entry
}

// SourcePickerCancelledMsg is posted when the user cancels (Esc).
type SourcePickerCancelledMsg struct{}

// Update handles key input. Returns the updated picker plus any cmd.
func (p SourcePicker) Update(msg tea.Msg) (SourcePicker, tea.Cmd) {
	keyMsg, ok := msg.(tea.KeyPressMsg)
	if !ok {
		return p, nil
	}
	switch keyMsg.String() {
	case "up", "k":
		if p.cursor > 0 {
			p.cursor--
		}
	case "down", "j":
		if p.cursor < len(p.candidates)-1 {
			p.cursor++
		}
	case "enter":
		if p.cursor < len(p.candidates) {
			sel := p.candidates[p.cursor]
			return p, func() tea.Msg { return SourceSelectedMsg{Entry: sel} }
		}
	case "esc":
		return p, func() tea.Msg { return SourcePickerCancelledMsg{} }
	}
	return p, nil
}

// View renders the modal as a bordered box.
func (p SourcePicker) View() string {
	if len(p.candidates) == 0 {
		return ""
	}

	// Title row (bold)
	titleStyle := lipgloss.NewStyle().Bold(true)
	rows := []string{titleStyle.Render(p.title)}
	rows = append(rows, "")

	// Source selection rows
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	accentStyle := lipgloss.NewStyle().Bold(true).Foreground(theme.T.Accent())

	for i, c := range p.candidates {
		cursor := "  "
		if i == p.cursor {
			cursor = "> "
		}
		line := fmt.Sprintf("%s%s", cursor, c.Source)
		if i == p.cursor {
			line = accentStyle.Render(line)
		} else {
			line = textStyle.Render(line)
		}
		rows = append(rows, line)
	}

	// Footer hint (faint)
	footerStyle := lipgloss.NewStyle().Faint(true)
	rows = append(rows, "")
	rows = append(rows, footerStyle.Render("↑↓ select  enter pick  esc cancel"))

	// Container with border
	borderStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 2)

	return borderStyle.Render(strings.Join(rows, "\n"))
}

// SelectedIndex returns the current cursor for tests.
func (p SourcePicker) SelectedIndex() int { return p.cursor }
