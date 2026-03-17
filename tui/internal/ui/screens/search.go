package screens

// search.go — SearchScreen: Vim-style incremental search.
//
// Usage:
//   Press '/' from anywhere to open.  Type immediately — results stream in
//   as you type.  Use ↑/↓ to navigate, Enter to open a result's detail,
//   Esc to close.
//
// Press 'a' to toggle between searching the current tab only and all tabs.
//
// Implementation notes:
//   - The screen searches the active tab by default.
//   - On Enter, a screen.PopMsg is batched with the selection message so
//     LegacyScreen (Model) receives SearchResultSelectedMsg and can open the
//     detail overlay.  Without the Pop, the message would be consumed by this
//     screen (since RootModel only forwards to the active screen).

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

// SearchScreen is a self-contained screen for global incremental search.
type SearchScreen struct {
	client    *ipc.Client
	activeTab ipc.MediaTab // tab that was active when search was opened
	query     string
	results   []ipc.MediaEntry
	cursor    int
	loading   bool
	err       string
	width     int
	height    int
	searchAll bool // true = search all tabs; false = active tab only
	reqSeq    int  // monotonic counter to discard stale results
}

func NewSearchScreen(client *ipc.Client, activeTab ipc.MediaTab) SearchScreen {
	return SearchScreen{
		client:    client,
		activeTab: activeTab,
	}
}

func (s SearchScreen) Init() tea.Cmd { return nil }

func (s SearchScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {

	case tea.WindowSizeMsg:
		s.width = m.Width
		s.height = m.Height

	case tea.KeyMsg:
		key := m.String()

		// ── Navigation and selection ──────────────────────────────────────
		if action, ok := actions.FromKey(key); ok {
			switch action {
			case actions.ActionNavigateDown:
				if s.cursor < len(s.results)-1 {
					s.cursor++
				}
			case actions.ActionNavigateUp:
				if s.cursor > 0 {
					s.cursor--
				}
			case actions.ActionSelect:
				return s, s.selectCurrent()
			case actions.ActionBack:
				return s, func() tea.Msg { return screen.PopMsg{} }
			}
		}

		// ── Search-specific keys ──────────────────────────────────────────
		switch key {
		case "esc":
			return s, func() tea.Msg { return screen.PopMsg{} }

		case "enter":
			return s, s.selectCurrent()

		case "a":
			// Toggle search all tabs
			s.searchAll = !s.searchAll
			if s.query != "" {
				return s, s.dispatchSearch()
			}

		case "backspace":
			if len(s.query) > 0 {
				// Trim one Unicode rune from the end
				runes := []rune(s.query)
				s.query = string(runes[:len(runes)-1])
				if s.query == "" {
					s.results = nil
					s.loading = false
					s.err = ""
				} else {
					return s, s.dispatchSearch()
				}
			}

		default:
			// Any printable character appends to query and triggers search
			if m.Type == tea.KeyRunes {
				s.query += key
				return s, s.dispatchSearch()
			}
		}

	case ipc.SearchResultMsg:
		if m.Err != nil {
			s.err = m.Err.Error()
			s.loading = false
		} else {
			// Deduplicate by ID across multi-tab responses: merge into results,
			// keeping the first occurrence (earlier tabs take priority).
			seen := make(map[string]bool, len(s.results))
			for _, r := range s.results {
				seen[r.ID] = true
			}
			for _, item := range m.Result.Items {
				if !seen[item.ID] {
					s.results = append(s.results, item)
					seen[item.ID] = true
				}
			}
			s.loading = false
			s.err = ""
		}
	}

	return s, nil
}

// dispatchSearch fires search request(s) and resets the result set.
func (s *SearchScreen) dispatchSearch() tea.Cmd {
	s.results = nil
	s.cursor = 0
	s.loading = true
	s.err = ""
	s.reqSeq++
	q := s.query

	if s.client == nil {
		s.loading = false
		return nil
	}

	if s.searchAll {
		// Fire a request per searchable tab simultaneously
		tabs := []ipc.MediaTab{ipc.TabMovies, ipc.TabSeries, ipc.TabLibrary}
		return func() tea.Msg {
			for _, t := range tabs {
				s.client.Search(fmt.Sprintf("qs-%s-%s", t, q), q, t, 30, 0)
			}
			return nil
		}
	}

	tab := s.activeTab
	if tab == "" {
		tab = ipc.TabMovies
	}
	return func() tea.Msg {
		s.client.Search(fmt.Sprintf("qs-%s-%s", tab, q), q, tab, 50, 0)
		return nil
	}
}

// selectCurrent returns a Cmd that pops this screen and fires
// SearchResultSelectedMsg so the LegacyScreen (Model) can open the detail overlay.
func (s SearchScreen) selectCurrent() tea.Cmd {
	if s.cursor < 0 || s.cursor >= len(s.results) {
		return nil
	}
	entry := s.results[s.cursor]
	return tea.Batch(
		func() tea.Msg { return screen.PopMsg{} },
		func() tea.Msg { return ipc.SearchResultSelectedMsg{Entry: entry} },
	)
}

// ── View ─────────────────────────────────────────────────────────────────────

func (s SearchScreen) View() string {
	acc   := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim   := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon  := lipgloss.NewStyle().Foreground(theme.T.Neon())
	muted := lipgloss.NewStyle().Foreground(theme.T.TextMuted())

	var sb strings.Builder

	// ── Search prompt ─────────────────────────────────────────────────────────
	scope := string(s.activeTab)
	if s.searchAll {
		scope = "all tabs"
	}
	scopeStr := muted.Render(" [" + scope + "]")

	cursor := acc.Render("▌")
	promptStr := acc.Render("/") + " " + s.query + cursor + scopeStr
	if s.loading {
		promptStr += dim.Render("  searching…")
	}

	sb.WriteString("\n  " + promptStr + "\n")

	// Toggle hint
	if s.searchAll {
		sb.WriteString("  " + dim.Render("a  ← switch to tab-only search") + "\n")
	} else {
		sb.WriteString("  " + dim.Render("a  → search all tabs") + "\n")
	}
	sb.WriteString("\n")

	// ── Empty state ────────────────────────────────────────────────────────────
	if s.query == "" {
		sb.WriteString("  " + dim.Render("Start typing to search…") + "\n")
		return sb.String()
	}

	// ── Error state ────────────────────────────────────────────────────────────
	if s.err != "" {
		sb.WriteString("  " + lipgloss.NewStyle().Foreground(theme.T.Red()).Render("Error: "+s.err) + "\n")
		return sb.String()
	}

	// ── No results ────────────────────────────────────────────────────────────
	if len(s.results) == 0 && !s.loading {
		sb.WriteString(dim.Render("  No results for \u201c"+s.query+"\u201d") + "\n")
		return sb.String()
	}

	// ── Results list ──────────────────────────────────────────────────────────
	w := s.width
	if w == 0 {
		w = 80
	}

	// Visible window
	listH := s.height - 7 // header(3) + toggle(1) + footer(3)
	if listH < 4 {
		listH = 4
	}
	start := 0
	if s.cursor >= listH {
		start = s.cursor - listH + 1
	}
	end := min(start+listH, len(s.results))

	if start > 0 {
		sb.WriteString("  " + dim.Render("↑ more") + "\n")
	}

	for i := start; i < end; i++ {
		r := s.results[i]

		var prefix string
		var titleStyle lipgloss.Style
		if i == s.cursor {
			prefix = "▶ "
			titleStyle = acc
		} else {
			prefix = "  "
			titleStyle = lipgloss.NewStyle().Foreground(theme.T.Text())
		}

		// Tab badge
		badge := searchTabBadge(string(r.Tab))
		badgeStr := neon.Render(badge)

		// Year
		yearStr := ""
		if r.Year != nil && *r.Year != "" {
			yearStr = dim.Render("  " + *r.Year)
		}

		// Provider
		provStr := ""
		if r.Provider != "" {
			provStr = dim.Render("  " + r.Provider)
		}

		titleW := w - len(prefix) - len(badge) - 22
		if titleW < 10 {
			titleW = 10
		}
		titleTrunc := truncate(r.Title, titleW)

		line := "  " + prefix + titleStyle.Render(titleTrunc) + yearStr + provStr + "  " + badgeStr
		sb.WriteString(line + "\n")
	}

	if end < len(s.results) {
		sb.WriteString("  " + dim.Render("↓ more") + "\n")
	}

	// ── Footer hint ────────────────────────────────────────────────────────────
	sb.WriteString("\n" + hintBar("↑↓ navigate", "enter open", "a toggle scope", "esc close") + "\n")

	return sb.String()
}

func searchTabBadge(tab string) string {
	switch tab {
	case "movies":
		return "[movie]"
	case "series":
		return "[series]"
	case "library":
		return "[lib]"
	case "music":
		return "[music]"
	default:
		if tab == "" {
			return ""
		}
		return "[" + tab + "]"
	}
}
