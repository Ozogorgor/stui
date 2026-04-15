package screens

// music_browse.go — Browse sub-tab: catalog search for music entries.

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/pkg/theme"
)

// MusicBrowseScreen shows the music catalog with search filtering.
type MusicBrowseScreen struct {
	Dims
	client  *ipc.Client
	catalog []ipc.CatalogEntry
	cursor  int
	scroll  int
	query   string
}

// NewMusicBrowseScreen creates a new browse screen.
func NewMusicBrowseScreen(client *ipc.Client) MusicBrowseScreen {
	return MusicBrowseScreen{client: client}
}

// filtered returns catalog entries matching the current query.
func (s MusicBrowseScreen) filtered() []ipc.CatalogEntry {
	if s.query == "" {
		return s.catalog
	}
	q := strings.ToLower(s.query)
	var out []ipc.CatalogEntry
	for _, e := range s.catalog {
		if strings.Contains(strings.ToLower(e.Title), q) {
			out = append(out, e)
		}
	}
	return out
}

// Update handles incoming messages and key events.
func (s MusicBrowseScreen) Update(msg tea.Msg) (MusicBrowseScreen, tea.Cmd) {
	switch m := msg.(type) {

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case ipc.GridUpdateMsg:
		if m.Tab == "music" {
			s.catalog = m.Entries
			s.cursor = 0
			s.scroll = 0
		}

	case tea.KeyPressMsg:
		results := s.filtered()
		switch m.String() {
		case "j", "down":
			if s.cursor < len(results)-1 {
				s.cursor++
			}
		case "k", "up":
			if s.cursor > 0 {
				s.cursor--
			}
		case "enter":
			if len(results) > 0 && s.cursor < len(results) && s.client != nil {
				entry := results[s.cursor]
				s.client.MpdCmd("mpd_add", map[string]any{"uri": entry.ID})
			}
		case "/":
			// Future: focus search input. For now reset cursor.
			s.cursor = 0
		}
	}

	return s, nil
}

// HandleMouse handles a left-click within the browse screen's own coordinate space.
func (s MusicBrowseScreen) HandleMouse(x, localY int) MusicBrowseScreen {
	results := s.filtered()
	// listHeight = View's h - 1, where h = terminal_height - 2 → listHeight = s.height - 3
	listHeight := s.height - 3
	if listHeight < 1 {
		listHeight = 1
	}
	if localY < 0 || localY >= listHeight {
		return s
	}
	// Recompute scroll the same way View does.
	scroll := s.scroll
	if s.cursor < scroll {
		scroll = s.cursor
	}
	if s.cursor >= scroll+listHeight {
		scroll = s.cursor - listHeight + 1
	}
	if scroll < 0 {
		scroll = 0
	}
	idx := scroll + localY
	if idx >= 0 && idx < len(results) {
		s.cursor = idx
	}
	return s
}

// View renders the browse screen within the given width/height constraints.
func (s MusicBrowseScreen) View(w, h int) string {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())

	results := s.filtered()

	if len(results) == 0 {
		emptyMsg := "No music in catalog — providers load on startup"
		listHeight := h
		if listHeight < 1 {
			listHeight = 1
		}
		var sb strings.Builder
		mid := listHeight / 2
		for i := 0; i < listHeight; i++ {
			if i == mid {
				pad := (w - len(emptyMsg)) / 2
				if pad < 0 {
					pad = 0
				}
				sb.WriteString(strings.Repeat(" ", pad) + dimStyle.Render(emptyMsg) + "\n")
			} else {
				sb.WriteString("\n")
			}
		}
		return sb.String()
	}

	// Hint/status text lives in the global footer (see ui.viewStatusBar);
	// the screen uses every available row for the bordered list.
	listHeight := h
	if listHeight < 1 {
		listHeight = 1
	}

	// Scrolling: keep cursor visible.
	scroll := s.scroll
	if s.cursor < scroll {
		scroll = s.cursor
	}
	if s.cursor >= scroll+listHeight {
		scroll = s.cursor - listHeight + 1
	}
	if scroll < 0 {
		scroll = 0
	}

	// Column widths: title takes most, then provider, then year.
	yearW := 6
	providerW := 12
	titleW := w - providerW - yearW - 4 // 4 for spacing
	if titleW < 10 {
		titleW = 10
	}

	borderStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 1)

	var sb strings.Builder
	end := scroll + listHeight
	if end > len(results) {
		end = len(results)
	}
	var content strings.Builder
	for i := scroll; i < end; i++ {
		e := results[i]
		titleStr := fmt.Sprintf("%-*s", titleW, truncate(e.Title, titleW))
		providerStr := fmt.Sprintf("%-*s", providerW, truncate(e.Provider, providerW))
		yearStr := ""
		if e.Year != nil {
			yearStr = *e.Year
		}
		yearStr = fmt.Sprintf("%-*s", yearW, truncate(yearStr, yearW))
		line := "  " + titleStr + "  " + providerStr + "  " + yearStr

		if i == s.cursor {
			content.WriteString(accentStyle.Render(line) + "\n")
		} else {
			content.WriteString(textStyle.Render(line) + "\n")
		}
	}

	// Pad to listHeight
	rendered := end - scroll
	for i := rendered; i < listHeight; i++ {
		content.WriteString("\n")
	}

	// Wrap in border container
	borderedContent := borderStyle.Width(w - 2).Render(content.String())
	sb.WriteString(borderedContent)

	return sb.String()
}

// FooterText is what the global status bar shows while this screen is
// active. Static hint — Browse has no per-action status messages of its
// own; status forwarded by other screens still wins via the StatusMsg
// route on the Model.
func (s MusicBrowseScreen) FooterText() string {
	return "enter add to queue · / search · ↑↓ navigate"
}
