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
	"time"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

const maxQueryLength = 256

// SearchScreen is a self-contained screen for global incremental search.
type SearchScreen struct {
	Dims
	client    *ipc.Client
	activeTab ipc.MediaTab // tab that was active when search was opened
	query     string
	results   []ipc.MediaEntry
	cursor    int
	loading   bool
	err       string
	searchAll bool // true = search all tabs; false = active tab only
	reqSeq    int  // monotonic counter to discard stale results
	spinner   components.Spinner
	debouncer *components.Debouncer
}

func NewSearchScreen(client *ipc.Client, activeTab ipc.MediaTab) SearchScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return SearchScreen{
		client:    client,
		activeTab: activeTab,
		spinner:   *components.NewSpinner("searching…", dimStyle),
		debouncer: components.NewDebouncer(150 * time.Millisecond),
	}
}

func (s SearchScreen) Init() tea.Cmd {
	return s.spinner.Init()
}

func (s SearchScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {

	case spinner.TickMsg:
		s.spinner.Update(m)
		return s, nil

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case tea.KeyPressMsg:
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
				return s, s.debouncedSearch()
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
					s.spinner.Stop()
					s.debouncer.Cancel()
				} else {
					return s, s.debouncedSearch()
				}
			}

		default:
			// Any printable character appends to query and triggers search
			if len(m.Text) > 0 && len(s.query) < maxQueryLength {
				s.query += key
				return s, s.debouncedSearch()
			}
		}

	case ipc.SearchResultMsg:
		if m.Err != nil {
			s.err = m.Err.Error()
			s.loading = false
			s.spinner.Stop()
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
			s.spinner.Stop()
		}
	}

	return s, nil
}

// debouncedSearch triggers a search after a short delay to batch rapid keystrokes.
func (s *SearchScreen) debouncedSearch() tea.Cmd {
	s.results = nil
	s.cursor = 0
	s.loading = true
	s.err = ""
	s.reqSeq++
	s.spinner.Start()

	s.debouncer.Trigger(s.reqSeq, func() {
		if s.client == nil {
			return
		}
		q := s.query
		if s.searchAll {
			tabs := []ipc.MediaTab{ipc.TabMovies, ipc.TabSeries, ipc.TabLibrary}
			for _, t := range tabs {
				s.client.Search(fmt.Sprintf("qs-%s-%s", t, q), q, t, 30, 0)
			}
		} else {
			tab := s.activeTab
			if tab == "" {
				tab = ipc.TabMovies
			}
			s.client.Search(fmt.Sprintf("qs-%s-%s", tab, q), q, tab, 50, 0)
		}
	})
	return nil
}

// dispatchSearch fires search request(s) and resets the result set.
func (s *SearchScreen) dispatchSearch() tea.Cmd {
	s.results = nil
	s.cursor = 0
	s.loading = true
	s.err = ""
	s.reqSeq++
	s.spinner.Start()
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

func (s SearchScreen) View() tea.View {
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())
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
		promptStr += "  " + s.spinner.View()
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
		return tea.NewView(sb.String())
	}

	// ── Error state ────────────────────────────────────────────────────────────
	if s.err != "" {
		sb.WriteString("  " + lipgloss.NewStyle().Foreground(theme.T.Red()).Render("Error: "+s.err) + "\n")
		return tea.NewView(sb.String())
	}

	// ── No results ────────────────────────────────────────────────────────────
	if len(s.results) == 0 && !s.loading {
		sb.WriteString(dim.Render("  No results for \u201c"+s.query+"\u201d") + "\n")
		return tea.NewView(sb.String())
	}

	// ── Results list ──────────────────────────────────────────────────────────
	w := s.width
	if w == 0 {
		w = 80
	}

	// Virtualized list rendering
	vl := components.NewVirtualizedList(
		len(s.results),
		s.cursor,
		s.height,
		components.WithHeaderHeight(6), // prompt + toggle + blank lines
		components.WithFooterHeight(1), // hint bar
	)

	start, end := vl.VisibleRange()
	indicator := vl.ScrollIndicator(dim)
	if indicator != "" {
		sb.WriteString("  " + indicator + "\n")
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

	// ── Footer hint ────────────────────────────────────────────────────────────
	sb.WriteString("\n" + hintBar("↑↓ navigate", "enter open", "a toggle scope", "esc close") + "\n")

	return tea.NewView(sb.String())
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
