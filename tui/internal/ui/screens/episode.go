package screens

// episode.go — EpisodeScreen: season/episode browser.
//
// Two display modes toggled with 'v':
//
//   List view (default)  — seasons left, episode rows right
//   Grid view            — seasons left, episode cells right
//                          e.g. [01] [02] [03] [04]
//                               [05] [06] [07] [08]
//
// 'b' toggles binge mode — BingeContextMsg is fired on play so Model can
// auto-queue the next episode when playback ends.

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// EpisodeScreen is the season/episode tree browser.
// To open: screen.TransitionCmd(NewEpisodeScreen(client, seriesTitle, seriesID), true)
type EpisodeScreen struct {
	client       *ipc.Client
	title        string
	seriesID     string
	seasons      []int         // available season numbers
	seasonCursor int
	episodes     []episodeItem // episodes for the selected season
	epCursor     int
	inEpisodes   bool // false = navigating seasons, true = navigating episodes
	loading      bool
	width        int
	gridView     bool // true = grid cell layout; false = list layout
	bingeEnabled bool // true = auto-play next episode on end-of-file
}

// episodeItem is aliased from ipc.EpisodeEntry
type episodeItem = ipc.EpisodeEntry

func NewEpisodeScreen(client *ipc.Client, title, seriesID string, autoplayDefault bool) EpisodeScreen {
	return EpisodeScreen{
		client:       client,
		title:        title,
		seriesID:     seriesID,
		loading:      true,
		seasons:      []int{1, 2, 3, 4, 5}, // populated from metadata
		bingeEnabled: autoplayDefault,
	}
}

// gridCols returns how many cells fit across the episode panel.
func (s EpisodeScreen) gridCols() int {
	const seasonW = 16
	const cellW   = 6 // "[E01] " — 6 chars per cell
	avail := s.width - seasonW - 4
	if avail < cellW {
		return 1
	}
	cols := avail / cellW
	if cols < 1 {
		return 1
	}
	return cols
}

func (s EpisodeScreen) Init() tea.Cmd {
	if s.client != nil && s.seriesID != "" && len(s.seasons) > 0 {
		s.client.LoadEpisodes(s.seriesID, s.seasons[0])
	}
	return nil
}

func (s EpisodeScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {

	case tea.WindowSizeMsg:
		s.width = m.Width
		s.loading = false

	case ipc.EpisodesLoadedMsg:
		if m.SeriesID == s.seriesID {
			s.episodes = m.Episodes
			s.epCursor = 0
			s.loading = false
		}

	case tea.KeyMsg:
		key := m.String()

		// ── Mode toggles (checked first so they always fire) ──────────────
		switch key {
		case "v":
			s.gridView = !s.gridView
			return s, nil
		case "b":
			s.bingeEnabled = !s.bingeEnabled
			return s, nil
		}

		if action, ok := actions.FromKey(key); ok {
			switch action {

			// ── Season navigation (same in both modes) ────────────────────
			case actions.ActionNavigateDown:
				if !s.inEpisodes {
					s.loadSeason(s.seasonCursor + 1)
				} else if s.gridView {
					cols := s.gridCols()
					next := s.epCursor + cols
					if next < len(s.episodes) {
						s.epCursor = next
					}
				} else {
					if s.epCursor < len(s.episodes)-1 {
						s.epCursor++
					}
				}

			case actions.ActionNavigateUp:
				if !s.inEpisodes {
					s.loadSeason(s.seasonCursor - 1)
				} else if s.gridView {
					cols := s.gridCols()
					if s.epCursor >= cols {
						s.epCursor -= cols
					}
				} else {
					if s.epCursor > 0 {
						s.epCursor--
					}
				}

			case actions.ActionNavigateRight:
				if !s.inEpisodes {
					s.inEpisodes = true
				} else if s.gridView {
					if s.epCursor < len(s.episodes)-1 {
						s.epCursor++
					}
				}
				// In list mode right does nothing extra (enter plays)

			case actions.ActionNavigateLeft:
				if s.inEpisodes {
					if s.gridView && s.epCursor%s.gridCols() > 0 {
						// Not at left edge of grid row — move left
						s.epCursor--
					} else {
						// At left edge or list mode — exit to seasons pane
						s.inEpisodes = false
					}
				}

			case actions.ActionBack:
				if s.inEpisodes {
					s.inEpisodes = false
				} else {
					return s, func() tea.Msg { return screen.PopMsg{} }
				}

			case actions.ActionSelect:
				return s, s.playSelected()
			}
		}

		// Enter also plays in both modes
		if key == "enter" && s.inEpisodes {
			return s, s.playSelected()
		}
	}
	return s, nil
}

// loadSeason switches to season at index idx (bounds-checked).
func (s *EpisodeScreen) loadSeason(idx int) {
	if idx < 0 || idx >= len(s.seasons) {
		return
	}
	s.seasonCursor = idx
	s.epCursor = 0
	s.loading = true
	s.episodes = nil
	if s.client != nil {
		s.client.LoadEpisodes(s.seriesID, s.seasons[s.seasonCursor])
	}
}

// playSelected returns the Cmd to play the episode under epCursor.
func (s EpisodeScreen) playSelected() tea.Cmd {
	if len(s.episodes) == 0 || s.client == nil {
		return nil
	}
	ep := s.episodes[s.epCursor]
	s.client.Play(ep.EntryID, ep.Provider, "", ipc.TabSeries)
	ctx := ipc.BingeContextMsg{
		SeriesTitle:  s.title,
		SeriesID:     s.seriesID,
		Tab:          ipc.TabSeries,
		Episodes:     append([]ipc.EpisodeEntry(nil), s.episodes...),
		CurrentIdx:   s.epCursor,
		BingeEnabled: s.bingeEnabled,
	}
	return tea.Batch(
		func() tea.Msg { return screen.PopMsg{} },
		func() tea.Msg { return ctx },
	)
}

// ── View ──────────────────────────────────────────────────────────────────────

func (s EpisodeScreen) View() string {
	acc  := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim  := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())

	var sb strings.Builder
	sb.WriteString("\n  " + acc.Render("\U0001f4fa "+s.title) + "\n\n")

	if s.loading {
		sb.WriteString(dim.Render("  Loading episodes\u2026") + "\n")
		return sb.String()
	}

	const seasonW = 16
	leftPanel := s.renderSeasonPanel(acc, dim, seasonW)

	var rightPanel string
	if s.gridView {
		rightPanel = s.renderGridPanel(acc, dim, neon)
	} else {
		rightPanel = s.renderListPanel(acc, dim, seasonW)
	}

	body := lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, "  ", rightPanel)
	sb.WriteString(body)

	// Footer
	var modeHint, bingeHint string
	if s.gridView {
		modeHint = neon.Render("v  grid")
	} else {
		modeHint = dim.Render("v  list")
	}
	if s.bingeEnabled {
		bingeHint = neon.Render("b  binge ON")
	} else {
		bingeHint = dim.Render("b  binge off")
	}
	navHint := hintBar("←→↑↓ navigate", "enter play", "esc back")
	sb.WriteString("\n\n" + navHint + "   " + modeHint + "   " + bingeHint + "\n")

	return sb.String()
}

func (s EpisodeScreen) renderSeasonPanel(acc, dim lipgloss.Style, w int) string {
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())
	var lines []string
	for i, sn := range s.seasons {
		cursor := "  "
		var style lipgloss.Style
		switch {
		case i == s.seasonCursor && !s.inEpisodes:
			cursor = "▶ "
			style = acc
		case i == s.seasonCursor:
			cursor = "▶ "
			style = normal
		default:
			style = dim
		}
		lines = append(lines, style.Render(fmt.Sprintf("%sSeason %d", cursor, sn)))
	}
	return lipgloss.NewStyle().Width(w).Render(strings.Join(lines, "\n"))
}

func (s EpisodeScreen) renderListPanel(acc, dim lipgloss.Style, seasonW int) string {
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())
	epW := s.width - seasonW - 8
	if epW < 20 {
		epW = 20
	}
	var lines []string
	for i, ep := range s.episodes {
		cursor := "  "
		var style lipgloss.Style
		if i == s.epCursor && s.inEpisodes {
			cursor = "▶ "
			style = acc
		} else {
			style = normal
		}
		epNum := fmt.Sprintf("E%02d", ep.Episode)
		title := ep.Title
		maxT := epW - 10
		if maxT > 0 && len(title) > maxT {
			title = title[:maxT-1] + "\u2026"
		}
		line := cursor + dim.Render(epNum) + "  " + style.Render(title)
		if ep.AirDate != "" {
			line += "  " + dim.Render(ep.AirDate[:min(len(ep.AirDate), 10)])
		}
		lines = append(lines, line)
	}
	return strings.Join(lines, "\n")
}

func (s EpisodeScreen) renderGridPanel(acc, dim, neon lipgloss.Style) string {
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())
	cols := s.gridCols()

	var rows []string
	for i := 0; i < len(s.episodes); i += cols {
		var cells []string
		for c := 0; c < cols; c++ {
			idx := i + c
			if idx >= len(s.episodes) {
				cells = append(cells, "      ") // pad last row
				continue
			}
			ep := s.episodes[idx]
			num := fmt.Sprintf("%02d", ep.Episode)
			var cell string
			if idx == s.epCursor && s.inEpisodes {
				cell = acc.Render("[") + acc.Render("E"+num) + acc.Render("]")
			} else if ep.AirDate == "" {
				// future / unaired
				cell = dim.Render("[E" + num + "]")
			} else {
				cell = normal.Render("[E" + num + "]")
			}
			cells = append(cells, cell+" ")
		}
		rows = append(rows, strings.Join(cells, ""))
	}

	// Info line: show selected episode title below the grid
	infoLine := ""
	if s.inEpisodes && s.epCursor >= 0 && s.epCursor < len(s.episodes) {
		ep := s.episodes[s.epCursor]
		info := fmt.Sprintf("E%02d", ep.Episode)
		if ep.Title != "" {
			info += "  " + ep.Title
		}
		if ep.AirDate != "" {
			info += "  " + dim.Render(ep.AirDate[:min(len(ep.AirDate), 10)])
		}
		if ep.Runtime > 0 {
			info += "  " + dim.Render(fmt.Sprintf("%dm", ep.Runtime))
		}
		infoLine = "\n\n  " + acc.Render(info)
		_ = neon // used for binge hint in View; suppress lint
	}

	return strings.Join(rows, "\n") + infoLine
}
