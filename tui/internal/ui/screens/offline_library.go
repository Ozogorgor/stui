package screens

// offline_library.go — Browse locally cached catalog entries when offline.
//
// Layout:
//
//   ┌─────────────────────────────────────────────────────────────────────┐
//   │ 📦 Offline Library  ·  142 titles cached                           │
//   ├────────────┬────────────────────────────────────────────────────────┤
//   │  Movies    │  Title                      Year   Genre        Rating │
//   │  Series    │  ─────────────────────────────────────────────────     │
//   │            │▶ Dune: Part Two             2024   Sci-Fi        8.4   │
//   │            │  Blade Runner 2049          2017   Sci-Fi        8.0   │
//   │            │  ...                                                   │
//   └────────────┴────────────────────────────────────────────────────────┘
//
//  Keys: ↑↓/j/k  navigate · enter open detail · Tab/L/R switch tab · q/esc close

import (
	"fmt"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/mediacache"
	"github.com/stui/stui/pkg/theme"
)

// OpenOfflineLibraryMsg triggers the offline library screen from the root model.
type OpenOfflineLibraryMsg struct{}

// ClearMediaCacheMsg asks the root model to wipe the local media cache.
type ClearMediaCacheMsg struct{}

// OfflineOpenDetailMsg is sent when the user presses Enter on a cached entry.
// The root model handles it via its normal openDetail path.
type OfflineOpenDetailMsg struct {
	Entry ipc.CatalogEntry
}

// ── Tab ordering ──────────────────────────────────────────────────────────────

var offlineTabs = []string{"movies", "series", "library"}

// ── Screen ────────────────────────────────────────────────────────────────────

// OfflineLibraryScreen shows the locally cached catalog.
type OfflineLibraryScreen struct {
	cache      *mediacache.Store
	tabs       []string            // tabs that have cached data
	activeTab  int                 // index into tabs
	entries    []ipc.CatalogEntry  // entries for the current tab
	cursor     int
	width      int
	height     int
}

// NewOfflineLibraryScreen creates the screen, pre-selecting the first tab
// that has cached data.
func NewOfflineLibraryScreen(cache *mediacache.Store) OfflineLibraryScreen {
	m := OfflineLibraryScreen{cache: cache}
	m.buildTabs()
	return m
}

// buildTabs populates m.tabs and refreshes m.entries for the active tab.
func (m *OfflineLibraryScreen) buildTabs() {
	m.tabs = nil
	for _, t := range offlineTabs {
		if len(m.cache.EntriesForTab(t)) > 0 {
			m.tabs = append(m.tabs, t)
		}
	}
	m.refreshEntries()
}

func (m *OfflineLibraryScreen) refreshEntries() {
	if len(m.tabs) == 0 {
		m.entries = nil
		return
	}
	tab := m.tabs[m.activeTab]
	m.entries = m.cache.EntriesForTab(tab)
	if m.cursor >= len(m.entries) {
		m.cursor = max(0, len(m.entries)-1)
	}
}

func (m *OfflineLibraryScreen) switchTab(delta int) {
	if len(m.tabs) == 0 {
		return
	}
	m.activeTab = (m.activeTab + delta + len(m.tabs)) % len(m.tabs)
	m.cursor = 0
	m.refreshEntries()
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m OfflineLibraryScreen) Init() tea.Cmd { return nil }

func (m OfflineLibraryScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height

	case tea.KeyMsg:
		switch msg.String() {
		case "up", "k":
			if m.cursor > 0 {
				m.cursor--
			}
		case "down", "j":
			if m.cursor < len(m.entries)-1 {
				m.cursor++
			}
		case "tab", "right", "l":
			m.switchTab(+1)
		case "shift+tab", "left", "h":
			m.switchTab(-1)
		case "enter":
			if m.cursor < len(m.entries) {
				entry := m.entries[m.cursor]
				return m, func() tea.Msg { return OfflineOpenDetailMsg{Entry: entry} }
			}
		case "q", "esc":
			return m, func() tea.Msg { return screen.PopMsg{} }
		}

	case tea.MouseMsg:
		switch {
		case msg.Button == tea.MouseButtonWheelUp:
			if m.cursor > 0 {
				m.cursor--
			}
		case msg.Button == tea.MouseButtonWheelDown:
			if m.cursor < len(m.entries)-1 {
				m.cursor++
			}
		}
	}
	return m, nil
}

func (m OfflineLibraryScreen) View() string {
	neon := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	bold := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	activeTabStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimTabStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	total := m.cache.TotalCount()
	lastUp := m.cache.LastUpdated()
	freshness := ""
	if lastUp > 0 {
		age := time.Now().Unix() - lastUp
		switch {
		case age < 3600:
			freshness = fmt.Sprintf(" · %dm ago", age/60)
		case age < 86400:
			freshness = fmt.Sprintf(" · %dh ago", age/3600)
		default:
			freshness = fmt.Sprintf(" · %dd ago", age/86400)
		}
	}

	header := neon.Render("📦 Offline Library") +
		dim.Render(fmt.Sprintf("  ·  %d title(s) cached%s", total, freshness))

	if total == 0 {
		empty := dim.Render("No cached titles yet — browse while online to populate the library.")
		footer := "\n\n" + hintBar("q close")
		return "  " + header + "\n\n  " + empty + footer
	}

	if len(m.tabs) == 0 {
		return "  " + header + "\n\n" + hintBar("q close")
	}

	// ── Left panel: tab list ──────────────────────────────────────────────
	leftW := 14
	var tabLines []string
	for i, t := range m.tabs {
		label := "  " + strings.Title(t)
		if i == m.activeTab {
			tabLines = append(tabLines, activeTabStyle.Render("▶ "+strings.Title(t)))
		} else {
			tabLines = append(tabLines, dimTabStyle.Render(label))
		}
	}
	leftPanel := lipgloss.NewStyle().
		Width(leftW).
		PaddingLeft(1).
		Render(strings.Join(tabLines, "\n"))

	// ── Right panel: entry table ──────────────────────────────────────────
	rightW := m.width - leftW - 4
	if rightW < 30 {
		rightW = 30
	}

	titleW := rightW - 28
	if titleW < 20 {
		titleW = 20
	}

	// Header row
	colHeader := bold.Render(fmt.Sprintf("%-*s  %-4s  %-14s  %s",
		titleW, "Title", "Year", "Genre", "Rating"))
	divider := dim.Render(strings.Repeat("─", rightW-2))

	// Visible window
	listHeight := m.height - 8
	if listHeight < 4 {
		listHeight = 4
	}
	start := 0
	if m.cursor >= listHeight {
		start = m.cursor - listHeight + 1
	}
	end := start + listHeight
	if end > len(m.entries) {
		end = len(m.entries)
	}

	var rows []string
	rows = append(rows, colHeader)
	rows = append(rows, divider)
	for i := start; i < end; i++ {
		e := m.entries[i]
		year := ""
		if e.Year != nil {
			year = *e.Year
		}
		genre := ""
		if e.Genre != nil {
			genre = *e.Genre
		}
		rating := "—"
		if e.Rating != nil && *e.Rating != "" {
			rating = *e.Rating
		}
		titleTrunc := olTruncate(e.Title, titleW)
		genreTrunc := olTruncate(genre, 14)
		line := fmt.Sprintf("%-*s  %-4s  %-14s  %s",
			titleW, titleTrunc, year, genreTrunc, rating)
		if i == m.cursor {
			rows = append(rows, lipgloss.NewStyle().
				Foreground(theme.T.Accent()).Bold(true).
				Render("▶ "+line))
		} else {
			rows = append(rows, lipgloss.NewStyle().
				Foreground(theme.T.Text()).
				Render("  "+line))
		}
	}

	// Scroll indicator
	if len(m.entries) > listHeight {
		scrollInfo := dim.Render(fmt.Sprintf("  %d–%d of %d", start+1, end, len(m.entries)))
		rows = append(rows, scrollInfo)
	}

	rightPanel := lipgloss.NewStyle().
		Width(rightW).
		PaddingLeft(1).
		Render(strings.Join(rows, "\n"))

	body := lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, rightPanel)

	footer := hintBar("↑↓ navigate", "enter open", "tab/←→ switch tab", "q close")

	return "  " + header + "\n\n" + body + "\n\n" + footer
}

// olTruncate truncates s to n runes, adding "…" if needed.
func olTruncate(s string, n int) string {
	runes := []rune(s)
	if len(runes) <= n {
		return s
	}
	if n <= 1 {
		return "…"
	}
	return string(runes[:n-1]) + "…"
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}
