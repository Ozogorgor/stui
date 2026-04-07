package screens

// downloads.go — DownloadsScreen: live view of all aria2 torrent downloads.
//
// Open with:  screen.TransitionCmd(screens.NewDownloadsScreen(client, entries), true)
//
// Keys:
//   ↑↓    navigate
//   enter  play completed download in mpv
//   x      cancel / remove download
//   esc/q  close

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// DownloadsScreen lists active and completed aria2 downloads.
type DownloadsScreen struct {
	Dims
	client  *ipc.Client
	entries []*ipc.DownloadEntry // kept in arrival order
	cursor  int
}

func NewDownloadsScreen(client *ipc.Client, entries []*ipc.DownloadEntry) DownloadsScreen {
	return DownloadsScreen{
		client:  client,
		entries: entries,
	}
}

func (s DownloadsScreen) Init() tea.Cmd { return nil }

func (s DownloadsScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	// Keep entries in sync with live IPC events even while the screen is open.
	case ipc.DownloadStartedMsg:
		for _, e := range s.entries {
			if e.GID == m.GID {
				return s, nil
			}
		}
		title := m.Title
		if title == "" {
			title = m.URI
		}
		s.entries = append(s.entries, &ipc.DownloadEntry{
			GID:    m.GID,
			Title:  title,
			Status: "active",
		})

	case ipc.DownloadProgressMsg:
		for _, e := range s.entries {
			if e.GID == m.GID {
				e.Progress = m.Progress
				e.Speed = m.Speed
				e.ETA = m.ETA
				e.Seeders = m.Seeders
				break
			}
		}

	case ipc.DownloadCompleteMsg:
		for _, e := range s.entries {
			if e.GID == m.GID {
				e.Status = "complete"
				e.Progress = 1.0
				e.Files = m.Files
				e.Speed = ""
				e.ETA = ""
				break
			}
		}

	case ipc.DownloadErrorMsg:
		for _, e := range s.entries {
			if e.GID == m.GID {
				e.Status = "error"
				e.Error = m.Message
				break
			}
		}

	case tea.KeyPressMsg:
		key := m.String()
		switch key {
		case "up", "k":
			if s.cursor > 0 {
				s.cursor--
			}
		case "down", "j":
			if s.cursor < len(s.entries)-1 {
				s.cursor++
			}
		case "enter":
			if len(s.entries) > 0 && s.client != nil {
				e := s.entries[s.cursor]
				if e.Status == "complete" && len(e.Files) > 0 {
					s.client.PlayFile(e.Files[0], e.Title)
					return s, func() tea.Msg { return screen.PopMsg{} }
				}
			}
		case "x":
			if len(s.entries) > 0 && s.client != nil {
				e := s.entries[s.cursor]
				s.client.CancelDownload(e.GID)
				// Mark as error locally (runtime will emit download_error shortly)
				e.Status = "error"
				e.Error = "cancelled"
			}
		case "esc", "q":
			return s, func() tea.Msg { return screen.PopMsg{} }
		}
	}
	return s, nil
}

// ── View ──────────────────────────────────────────────────────────────────────

func (s DownloadsScreen) View() tea.View {
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())
	green := lipgloss.NewStyle().Foreground(theme.T.Success())
	red := lipgloss.NewStyle().Foreground(lipgloss.Color("#e06c75"))
	bold := lipgloss.NewStyle().Bold(true)

	var sb strings.Builder
	sb.WriteString("\n  " + acc.Render("⬇  Torrent Downloads") + "\n\n")

	if len(s.entries) == 0 {
		sb.WriteString("  " + dim.Render("No downloads yet.") + "\n")
		sb.WriteString("  " + dim.Render("Press ") + acc.Render("D") + dim.Render(" in the stream picker to pre-download a torrent.") + "\n")
		sb.WriteString("\n  " + dim.Render("[Esc] close") + "\n")
		return tea.NewView(sb.String())
	}

	w := s.width - 4
	if w < 60 {
		w = 60
	}

	// ── Header row ───────────────────────────────────────────────────────
	titleW := w - 42
	if titleW < 20 {
		titleW = 20
	}
	hdr := dim.Render(
		padRight("Title", titleW) + "  " +
			padRight("Progress", 20) + "  " +
			padRight("Speed", 10) + "  ETA",
	)
	sb.WriteString("  " + hdr + "\n")
	sb.WriteString("  " + dim.Render(strings.Repeat("─", w)) + "\n")

	// Calculate scrollbar
	maxRows := s.height - 8
	if maxRows < 1 {
		maxRows = 10
	}
	start := 0
	if s.cursor >= maxRows {
		start = s.cursor - maxRows + 1
	}
	end := start + maxRows
	if end > len(s.entries) {
		end = len(s.entries)
	}

	var scrollbar string
	if len(s.entries) > maxRows {
		vl := components.NewVirtualizedList(len(s.entries), s.cursor, maxRows)
		scrollbar = vl.VerticalScrollbar(1, dim)
	}

	for i := start; i < end; i++ {
		e := s.entries[i]
		sel := i == s.cursor

		// Title
		title := truncateStr(e.Title, titleW)
		titlePad := padRight(title, titleW)

		// Progress bar (18 chars) + percentage
		const barW = 18
		filled := int(float64(barW) * e.Progress)
		if filled > barW {
			filled = barW
		}
		bar := strings.Repeat("█", filled) + strings.Repeat("░", barW-filled)
		pct := fmt.Sprintf("%3.0f%%", e.Progress*100)
		progressCell := padRight(bar+" "+pct, 22)

		// Speed / status
		speedCell := padRight(e.Speed, 10)
		etaCell := e.ETA

		// Status badge
		var statusBadge string
		switch e.Status {
		case "active":
			if e.Seeders > 0 {
				statusBadge = dim.Render(fmt.Sprintf("  %d seeders", e.Seeders))
			}
		case "complete":
			statusBadge = green.Render("  ✓ complete")
			progressCell = padRight(strings.Repeat("█", barW)+" 100%", 22)
			speedCell = ""
			etaCell = ""
		case "error":
			statusBadge = red.Render("  ✗ " + e.Error)
			speedCell = ""
			etaCell = ""
		}

		line := titlePad + "  " + progressCell + speedCell + "  " + etaCell + statusBadge

		if sel {
			line = bold.Foreground(theme.T.Accent()).Render("> ") + neon.Render(line)
		} else {
			line = "  " + line
		}

		// Add scrollbar for first item if scrolling is active
		if i == start && scrollbar != "" {
			line = line + " " + scrollbar
		}
		sb.WriteString(line + "\n")
	}

	// ── Footer ───────────────────────────────────────────────────────────
	sb.WriteString("\n")
	footer := dim.Render("[↑↓] navigate  ") +
		acc.Render("[Enter]") + dim.Render(" play complete  ") +
		acc.Render("[x]") + dim.Render(" cancel  ") +
		acc.Render("[Esc]") + dim.Render(" close")
	sb.WriteString("  " + footer + "\n")

	return tea.NewView(sb.String())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

func padRight(s string, n int) string {
	r := []rune(s)
	if len(r) >= n {
		return string(r[:n])
	}
	return s + strings.Repeat(" ", n-len(r))
}

func truncateStr(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	if n <= 1 {
		return "…"
	}
	return string(r[:n-1]) + "…"
}
