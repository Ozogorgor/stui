package screens

// search.go — SearchScreen: incremental search with a full textinput widget.
//
// Usage:
//   Press '/' from anywhere to open.  Type immediately — results stream in
//   as you type.  Use ↑/↓ (or ctrl+p/n) to navigate, Enter to open, Esc to close.
//
// Press 'a' to toggle between searching the current tab only and all tabs.
//
// The query input is a charm.land/bubbles/v2/textinput.Model — supports full
// cursor movement (←/→, ctrl+a/e, alt+←/→ word-jump, ctrl+w delete-word, etc.).

import (
	"fmt"
	"strings"
	"time"

	"charm.land/bubbles/v2/spinner"
	"charm.land/bubbles/v2/textinput"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

const maxQueryLength = 256

// SearchScreen is a self-contained screen for global incremental search.
type SearchScreen struct {
	Dims
	client    *ipc.Client
	activeTab ipc.MediaTab
	input     textinput.Model
	results   []ipc.MediaEntry
	cursor    int
	loading   bool
	err       string
	searchAll bool
	reqSeq    int
	spinner   components.Spinner
	debouncer *components.Debouncer
}

func NewSearchScreen(client *ipc.Client, activeTab ipc.MediaTab) SearchScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	ti := textinput.New()
	ti.Placeholder = "type to search…"
	ti.CharLimit = maxQueryLength
	ti.SetStyles(textinput.Styles{
		Focused: textinput.StyleState{
			Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
			Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
			Prompt:      lipgloss.NewStyle().Foreground(theme.T.Accent()),
		},
		Blurred: textinput.StyleState{
			Text:        lipgloss.NewStyle().Foreground(theme.T.TextDim()),
			Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
		},
	})
	// Focus it right away so it is ready to accept input immediately.
	ti.Focus()

	return SearchScreen{
		client:    client,
		activeTab: activeTab,
		input:     ti,
		spinner:   *components.NewSpinner("searching…", dimStyle),
		debouncer: components.NewDebouncer(150 * time.Millisecond),
	}
}

func (s SearchScreen) Init() tea.Cmd {
	// Return the blink command so the cursor starts animating.
	return tea.Batch(s.spinner.Init(), s.input.Focus())
}

func (s SearchScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {

	case spinner.TickMsg:
		_, cmd := s.spinner.Update(m)
		return s, cmd

	case tea.WindowSizeMsg:
		s.setWindowSize(m)
		s.input.SetWidth(max(20, m.Width/3))
		return s, nil

	case tea.KeyPressMsg:
		key := m.String()

		// Screen-level keys: handle before forwarding to textinput.
		switch key {
		case "esc":
			return s, func() tea.Msg { return screen.PopMsg{} }
		case "enter":
			return s, s.selectCurrent()
		case "up", "ctrl+p":
			if s.cursor > 0 {
				s.cursor--
			}
			return s, nil
		case "down", "ctrl+n":
			if s.cursor < len(s.results)-1 {
				s.cursor++
			}
			return s, nil
		case "a":
			s.searchAll = !s.searchAll
			if s.input.Value() != "" {
				return s, s.debouncedSearch()
			}
			return s, nil
		}

		// All other keys go to the textinput (cursor movement, backspace, typing).
		prevVal := s.input.Value()
		var tiCmd tea.Cmd
		s.input, tiCmd = s.input.Update(m)
		newVal := s.input.Value()

		if newVal != prevVal {
			if newVal == "" {
				s.results = nil
				s.loading = false
				s.err = ""
				s.spinner.Stop()
				s.debouncer.Cancel()
			} else {
				return s, tea.Batch(tiCmd, s.debouncedSearch())
			}
		}
		return s, tiCmd

	case ipc.SearchResultMsg:
		if m.Err != nil {
			s.err = m.Err.Error()
			s.loading = false
			s.spinner.Stop()
		} else {
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

	// Forward all other messages to textinput (e.g. cursor blink tick).
	var tiCmd tea.Cmd
	s.input, tiCmd = s.input.Update(msg)
	return s, tiCmd
}

func (s *SearchScreen) debouncedSearch() tea.Cmd {
	s.results = nil
	s.cursor = 0
	s.loading = true
	s.err = ""
	s.reqSeq++
	s.spinner.Start()

	q := s.input.Value()
	s.debouncer.Trigger(s.reqSeq, func() {
		if s.client == nil {
			return
		}
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

	promptStr := acc.Render("/") + " " + s.input.View() + scopeStr
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
	if s.input.Value() == "" {
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
		sb.WriteString(dim.Render("  No results for \u201c"+s.input.Value()+"\u201d") + "\n")
		return tea.NewView(sb.String())
	}

	// ── Results list ──────────────────────────────────────────────────────────
	w := s.width
	if w == 0 {
		w = 80
	}

	vl := components.NewVirtualizedList(
		len(s.results),
		s.cursor,
		s.height,
		components.WithHeaderHeight(6),
		components.WithFooterHeight(1),
	)

	start, end := vl.VisibleRange()
	scrollbar := vl.VerticalScrollbar(1, dim)

	var contentLines []string
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

		badge := searchTabBadge(string(r.Tab))
		badgeStr := neon.Render(badge)

		yearStr := ""
		if r.Year != nil && *r.Year != "" {
			yearStr = dim.Render("  " + *r.Year)
		}

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
		contentLines = append(contentLines, line)
	}

	for i, line := range contentLines {
		if i == 0 && scrollbar != "" {
			sb.WriteString(line + " " + scrollbar + "\n")
		} else {
			sb.WriteString(line + "\n")
		}
	}

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
