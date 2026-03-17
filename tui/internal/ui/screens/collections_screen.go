package screens

// collections_screen.go — the Collections tab component.
//
// Layout:
//
//  ┌─────────────────────────────────────────────────────────────────┐
//  │  Collections                 [n] new  [d] del  [r] rename       │  ← header
//  ├──────────────────────┬──────────────────────────────────────────┤
//  │  ▸ Watchlist    (4)  │  Dune: Part Two               movies     │
//  │    Favorites    (2)  │  Attack on Titan S01          series     │
//  │    Anime Backlog(8)  │  Blade Runner 2049            movies     │
//  │                      │                                          │
//  ├──────────────────────┴──────────────────────────────────────────┤
//  │  tab → cycle panes  ←/→ switch  enter open  x remove  n new    │  ← footer
//  └─────────────────────────────────────────────────────────────────┘
//
// Pane navigation:
//   Left pane  — j/k move collection cursor; n new; d delete; r rename; → switch to right
//   Right pane — j/k move entry cursor; enter open detail; x remove; ← switch to left
//
// Note: the global 'tab' key is consumed by ActionNextTab before reaching this
// component, so pane switching uses ←/→ (h/l) instead.

import (
	"fmt"
	"math"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/pkg/collections"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

// CollectionOpenDetailMsg is fired when the user presses Enter on an entry.
// Handled by Model to open the standard detail overlay.
type CollectionOpenDetailMsg struct {
	Entry ipc.CatalogEntry
}

// collectionsPane tracks which pane has keyboard focus.
type collectionsPane int

const (
	collectionsPaneLeft  collectionsPane = iota // collection list
	collectionsPaneRight                        // entry list
)

const collLeftWidth = 26 // chars for the left pane (inc. separator)

// continueWatchingName is the reserved name for the auto-managed history section.
const continueWatchingName = "Continue Watching"

// CollectionsScreen is the component embedded in Model for the Collections tab.
// It follows the same value-receiver / copy-on-update pattern as MusicScreen.
type CollectionsScreen struct {
	store        *collections.Store
	historyStore *watchhistory.Store
	width        int
	height       int

	activePane  collectionsPane
	leftCursor  int // index into store.Collections
	rightCursor int
	rightScroll int

	// Inline text-input modes (new collection / rename)
	inputMode  bool   // true = creating new collection
	inputBuf   string
	renameMode bool   // true = renaming leftCursor collection
	renameBuf  string
}

// NewCollectionsScreen creates a ready-to-use CollectionsScreen.
func NewCollectionsScreen(store *collections.Store, history *watchhistory.Store) CollectionsScreen {
	return CollectionsScreen{store: store, historyStore: history}
}

// SetSize updates the terminal dimensions.
func (s CollectionsScreen) SetSize(w, h int) CollectionsScreen {
	s.width = w
	s.height = h
	return s
}

// ── Update ───────────────────────────────────────────────────────────────────

// Update handles BubbleTea messages forwarded by Model.
func (s CollectionsScreen) Update(msg tea.Msg) (CollectionsScreen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		s.width = msg.Width
		s.height = msg.Height
	case tea.KeyMsg:
		return s.handleKey(msg.String())
	case tea.MouseMsg:
		return s.handleMouse(msg)
	}
	return s, nil
}

func (s CollectionsScreen) handleKey(key string) (CollectionsScreen, tea.Cmd) {
	// ── Input mode (new collection name) ─────────────────────────────────
	if s.inputMode {
		switch key {
		case "esc":
			s.inputMode = false
			s.inputBuf = ""
		case "enter":
			name := strings.TrimSpace(s.inputBuf)
			if name != "" {
				s.store.NewCollection(name)
				go func() { _ = s.store.Save() }()
				s.leftCursor = len(s.store.Collections) - 1
				s.rightCursor = 0
				s.rightScroll = 0
			}
			s.inputMode = false
			s.inputBuf = ""
		case "backspace":
			if len(s.inputBuf) > 0 {
				s.inputBuf = s.inputBuf[:len(s.inputBuf)-1]
			}
		default:
			if len(key) == 1 {
				s.inputBuf += key
			}
		}
		return s, nil
	}

	// ── Rename mode ───────────────────────────────────────────────────────
	if s.renameMode {
		colls := s.store.Collections
		switch key {
		case "esc":
			s.renameMode = false
			s.renameBuf = ""
		case "enter":
			name := strings.TrimSpace(s.renameBuf)
			if name != "" && s.leftCursor < len(colls) {
				s.store.RenameCollection(colls[s.leftCursor].Name, name)
				go func() { _ = s.store.Save() }()
			}
			s.renameMode = false
			s.renameBuf = ""
		case "backspace":
			if len(s.renameBuf) > 0 {
				s.renameBuf = s.renameBuf[:len(s.renameBuf)-1]
			}
		default:
			if len(key) == 1 {
				s.renameBuf += key
			}
		}
		return s, nil
	}

	// ── Normal navigation ─────────────────────────────────────────────────

	switch key {
	case "j", "down":
		if s.activePane == collectionsPaneLeft {
			if s.leftCursor < s.totalLeftItems()-1 {
				s.leftCursor++
				s.rightCursor = 0
				s.rightScroll = 0
			}
		} else {
			entries := s.currentEntries()
			if s.rightCursor < len(entries)-1 {
				s.rightCursor++
				listH := s.listHeight()
				if s.rightCursor >= s.rightScroll+listH {
					s.rightScroll++
				}
			}
		}

	case "k", "up":
		if s.activePane == collectionsPaneLeft {
			if s.leftCursor > 0 {
				s.leftCursor--
				s.rightCursor = 0
				s.rightScroll = 0
			}
		} else {
			if s.rightCursor > 0 {
				s.rightCursor--
				if s.rightCursor < s.rightScroll {
					s.rightScroll = s.rightCursor
				}
			}
		}

	case "l", "right":
		s.activePane = collectionsPaneRight

	case "h", "left":
		s.activePane = collectionsPaneLeft

	case "enter":
		if s.activePane == collectionsPaneLeft {
			// Enter on a collection switches focus to its entries
			s.activePane = collectionsPaneRight
		} else {
			// Enter on an entry → open detail overlay
			return s, s.openDetailCmd()
		}

	case "n":
		if s.activePane == collectionsPaneLeft {
			s.inputMode = true
			s.inputBuf = ""
		}

	case "r":
		// Renaming the Continue Watching section is not allowed
		if s.activePane == collectionsPaneLeft && !s.isContinueWatching() {
			idx := s.selectedCollectionIdx()
			if idx >= 0 && idx < len(s.store.Collections) {
				s.renameMode = true
				s.renameBuf = s.store.Collections[idx].Name
			}
		}

	case "d", "delete":
		// The Continue Watching section cannot be deleted
		if s.activePane == collectionsPaneLeft && !s.isContinueWatching() {
			idx := s.selectedCollectionIdx()
			if idx >= 0 && idx < len(s.store.Collections) {
				s.store.DeleteCollection(s.store.Collections[idx].Name)
				go func() { _ = s.store.Save() }()
				if s.leftCursor >= s.totalLeftItems() && s.leftCursor > 0 {
					s.leftCursor--
				}
				s.rightCursor = 0
				s.rightScroll = 0
			}
		}

	case "x":
		if s.activePane == collectionsPaneRight {
			entries := s.currentEntries()
			if s.rightCursor < len(entries) {
				if s.isContinueWatching() && s.historyStore != nil {
					// Remove from watch history
					s.historyStore.Remove(entries[s.rightCursor].ID)
					go func() { _ = s.historyStore.Save() }()
				} else {
					idx := s.selectedCollectionIdx()
					if idx >= 0 && idx < len(s.store.Collections) {
						s.store.RemoveFrom(s.store.Collections[idx].Name, entries[s.rightCursor].ID)
						go func() { _ = s.store.Save() }()
					}
				}
				remaining := s.currentEntries()
				if s.rightCursor >= len(remaining) && s.rightCursor > 0 {
					s.rightCursor--
				}
			}
		}
	}

	return s, nil
}

func (s CollectionsScreen) handleMouse(msg tea.MouseMsg) (CollectionsScreen, tea.Cmd) {
	switch msg.Button {
	case tea.MouseButtonWheelUp:
		return s.handleKey("k")
	case tea.MouseButtonWheelDown:
		return s.handleKey("j")
	}

	if msg.Action != tea.MouseActionPress || msg.Button != tea.MouseButtonLeft {
		return s, nil
	}

	headerH := 1 // header row
	bodyY := msg.Y - headerH
	if bodyY < 0 {
		return s, nil
	}

	rightPaneX := collLeftWidth + 1 // start of right pane

	if msg.X < collLeftWidth {
		// Click in left pane
		s.activePane = collectionsPaneLeft
		idx := bodyY
		if idx >= 0 && idx < s.totalLeftItems() {
			s.leftCursor = idx
			s.rightCursor = 0
			s.rightScroll = 0
		}
	} else if msg.X >= rightPaneX {
		// Click in right pane
		s.activePane = collectionsPaneRight
		entries := s.currentEntries()
		targetIdx := s.rightScroll + bodyY
		if targetIdx >= 0 && targetIdx < len(entries) {
			s.rightCursor = targetIdx
		}
	}

	return s, nil
}

// openDetailCmd builds a tea.Cmd that fires CollectionOpenDetailMsg for the
// currently selected entry.
func (s CollectionsScreen) openDetailCmd() tea.Cmd {
	entries := s.currentEntries()
	if s.rightCursor < 0 || s.rightCursor >= len(entries) {
		return nil
	}
	e := entries[s.rightCursor]
	year := e.Year
	imdbID := e.ImdbID
	catalogEntry := ipc.CatalogEntry{
		ID:       e.ID,
		Title:    e.Title,
		Year:     &year,
		Tab:      e.Tab,
		Provider: e.Provider,
		ImdbID:   &imdbID,
	}
	return func() tea.Msg {
		return CollectionOpenDetailMsg{Entry: catalogEntry}
	}
}

// inProgressProgressStr returns a progress string for the right-pane entry at
// index i (used when rendering Continue Watching rows).
func (s CollectionsScreen) progressForEntry(id string) string {
	if s.historyStore == nil {
		return ""
	}
	e := s.historyStore.Get(id)
	if e == nil || e.Position <= 0 {
		return ""
	}
	pos := formatCollDuration(e.Position)
	if e.Duration > 0 {
		pct := int(math.Min(e.Position/e.Duration, 1.0) * 100)
		return fmt.Sprintf("%s  %d%%", pos, pct)
	}
	return pos
}

func formatCollDuration(secs float64) string {
	total := int(secs)
	h := total / 3600
	m := (total % 3600) / 60
	s := total % 60
	if h > 0 {
		return fmt.Sprintf("%d:%02d:%02d", h, m, s)
	}
	return fmt.Sprintf("%d:%02d", m, s)
}

// ── View ─────────────────────────────────────────────────────────────────────

// View renders the full Collections tab.
func (s CollectionsScreen) View() string {
	if s.width == 0 {
		return ""
	}
	header := s.renderHeader()
	body := s.renderBody()
	footer := s.renderFooter()
	return lipgloss.JoinVertical(lipgloss.Left, header, body, footer)
}

func (s CollectionsScreen) renderHeader() string {
	title := lipgloss.NewStyle().
		Foreground(theme.T.Text()).
		Bold(true).
		Render("Collections")

	hint := lipgloss.NewStyle().
		Foreground(theme.T.TextDim()).
		Render("[n] new   [d] delete   [r] rename   [x] remove entry")

	gapW := max(0, s.width-lipgloss.Width(title)-lipgloss.Width(hint)-4)
	row := title + strings.Repeat(" ", gapW) + hint
	return theme.T.TopBarStyle().Width(s.width - 2).Render(row)
}

func (s CollectionsScreen) renderBody() string {
	bodyH := s.height - 3 // minus header + footer + border
	if bodyH < 1 {
		bodyH = 1
	}

	leftW := collLeftWidth
	rightW := s.width - leftW - 1 // 1 for separator column

	left := s.renderLeft(leftW, bodyH)
	sep := s.renderSeparator(bodyH)
	right := s.renderRight(rightW, bodyH)

	return lipgloss.JoinHorizontal(lipgloss.Top, left, sep, right)
}

func (s CollectionsScreen) renderSeparator(h int) string {
	lines := make([]string, h)
	for i := range lines {
		lines[i] = "│"
	}
	return lipgloss.NewStyle().
		Foreground(theme.T.Border()).
		Render(strings.Join(lines, "\n"))
}

func (s CollectionsScreen) renderLeft(w, h int) string {
	var lines []string
	nameW := w - 7

	renderItem := func(idx int, name string, count int, isSpecial bool) string {
		selected := idx == s.leftCursor
		focused := selected && s.activePane == collectionsPaneLeft

		if s.renameMode && selected && !isSpecial {
			return theme.T.SearchFocusedStyle().
				Width(w - 1).
				Render("  " + s.renameBuf + "█")
		}

		truncName := truncate(name, nameW)
		countStr := lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			Width(5).
			Render(fmt.Sprintf("(%d)", count))

		prefix := "  "
		if selected {
			prefix = "▸ "
		}
		var nameStyle lipgloss.Style
		if isSpecial {
			nameStyle = lipgloss.NewStyle().Foreground(theme.T.Neon())
		} else if focused {
			nameStyle = theme.T.TabActiveStyle()
		} else if selected {
			nameStyle = lipgloss.NewStyle().Foreground(theme.T.AccentAlt())
		} else {
			nameStyle = lipgloss.NewStyle().Foreground(theme.T.TextDim())
		}
		nameStr := nameStyle.Width(nameW + 1).Render(prefix + truncName)

		if focused && !isSpecial {
			// Override the whole row with active style for non-special focused
			row := nameStyle.Width(nameW + 1).Render(prefix + truncName)
			return lipgloss.JoinHorizontal(lipgloss.Top, row, countStr)
		}
		return lipgloss.JoinHorizontal(lipgloss.Top, nameStr, countStr)
	}

	// Index 0: "Continue Watching" (auto-managed)
	cwCount := 0
	if s.historyStore != nil {
		cwCount = len(s.historyStore.InProgress())
	}
	lines = append(lines, renderItem(0, continueWatchingName, cwCount, true))

	// Indexes 1+: user collections
	for i, c := range s.store.Collections {
		lines = append(lines, renderItem(i+1, c.Name, len(c.Entries), false))
	}

	// Text input for new collection at bottom of list
	if s.inputMode {
		lines = append(lines, theme.T.SearchFocusedStyle().
			Width(w-1).
			Render("+ "+s.inputBuf+"█"))
	}

	content := strings.Join(lines, "\n")
	return lipgloss.NewStyle().
		Background(theme.T.Surface()).
		Width(w).
		Height(h).
		Render(content)
}

func (s CollectionsScreen) renderRight(w, h int) string {
	entries := s.currentEntries()

	if len(entries) == 0 {
		var hint string
		if s.isContinueWatching() {
			hint = "Nothing in progress.\n\nPlay a movie or series and\nstui will track your position here."
		} else if len(s.store.Collections) == 0 {
			hint = "No collections yet.\nPress n to create one."
		} else {
			hint = "No entries yet.\n\nFrom a movie or series,\nopen its detail and press c\nto add it to this collection."
		}
		empty := lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			Padding(2, 2).
			Render(hint)
		return lipgloss.NewStyle().
			Background(theme.T.Bg()).
			Width(w).Height(h).
			Render(empty)
	}

	listH := s.listHeight()
	scroll := s.rightScroll
	end := min(scroll+listH, len(entries))

	var lines []string

	// Scroll-up indicator
	if scroll > 0 {
		lines = append(lines,
			lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Render("  ↑ more"))
	}

	showProgress := s.isContinueWatching()

	for i := scroll; i < end; i++ {
		e := entries[i]
		selected := i == s.rightCursor && s.activePane == collectionsPaneRight

		year := e.Year
		if year == "" {
			year = "    "
		}
		badge := collTabBadge(e.Tab)

		var row string
		if showProgress {
			// Show title + progress instead of year+badge
			progress := s.progressForEntry(e.ID)
			titleW := w - len(progress) - 4
			if titleW < 8 {
				titleW = 8
			}
			title := truncate(e.Title, titleW)
			titleStr := lipgloss.NewStyle().Width(titleW).Render(title)
			progStr := lipgloss.NewStyle().Foreground(theme.T.Neon()).Render(progress)
			row = lipgloss.JoinHorizontal(lipgloss.Top, titleStr, "  ", progStr)
		} else {
			titleW := w - 14
			title := truncate(e.Title, titleW)
			titleStr := lipgloss.NewStyle().Width(titleW).Render(title)
			yearStr := lipgloss.NewStyle().
				Foreground(theme.T.TextDim()).Width(6).
				Render(year)
			badgeStr := lipgloss.NewStyle().
				Foreground(theme.T.AccentAlt()).Width(8).
				Render(badge)
			row = lipgloss.JoinHorizontal(lipgloss.Top, titleStr, yearStr, badgeStr)
		}

		if selected {
			row = theme.T.TabActiveStyle().Width(w - 2).Render(row)
		}
		lines = append(lines, "  "+row)
	}

	// Scroll-down indicator
	if end < len(entries) {
		lines = append(lines,
			lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Render("  ↓ more"))
	}

	content := strings.Join(lines, "\n")
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(w).Height(h).
		Render(content)
}

func (s CollectionsScreen) renderFooter() string {
	var hint string
	if s.inputMode {
		hint = "  Type collection name   enter confirm   esc cancel"
	} else if s.renameMode {
		hint = "  Type new name   enter confirm   esc cancel"
	} else if s.activePane == collectionsPaneLeft {
		hint = "  j/k move   → entries   n new   d delete   r rename"
	} else {
		hint = "  j/k move   ← collections   enter open   x remove"
	}
	return lipgloss.NewStyle().
		Foreground(theme.T.TextDim()).
		Background(theme.T.Surface()).
		Width(s.width).
		Render(hint)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

// totalLeftItems returns the number of items in the left pane:
// "Continue Watching" + user collections.
func (s CollectionsScreen) totalLeftItems() int {
	return 1 + len(s.store.Collections)
}

// isContinueWatching reports whether the left cursor points at the auto-managed section.
func (s CollectionsScreen) isContinueWatching() bool {
	return s.leftCursor == 0
}

// selectedCollectionIdx returns the index into store.Collections for the current
// left cursor, or -1 if "Continue Watching" is selected.
func (s CollectionsScreen) selectedCollectionIdx() int {
	return s.leftCursor - 1
}

func (s CollectionsScreen) currentEntries() []collections.Entry {
	if s.isContinueWatching() {
		if s.historyStore == nil {
			return nil
		}
		inProgress := s.historyStore.InProgress()
		out := make([]collections.Entry, 0, len(inProgress))
		for _, h := range inProgress {
			out = append(out, collections.Entry{
				ID:       h.ID,
				Title:    h.Title,
				Year:     h.Year,
				Tab:      h.Tab,
				Provider: h.Provider,
				ImdbID:   h.ImdbID,
			})
		}
		return out
	}
	idx := s.selectedCollectionIdx()
	if idx >= 0 && idx < len(s.store.Collections) {
		return s.store.Collections[idx].Entries
	}
	return nil
}

// inProgressEntry returns the watchhistory.Entry for the currently selected
// right-pane entry, or nil if not applicable.
func (s CollectionsScreen) inProgressEntry() *watchhistory.Entry {
	if !s.isContinueWatching() || s.historyStore == nil {
		return nil
	}
	inProgress := s.historyStore.InProgress()
	if s.rightCursor >= 0 && s.rightCursor < len(inProgress) {
		e := inProgress[s.rightCursor]
		return &e
	}
	return nil
}

func (s CollectionsScreen) listHeight() int {
	h := s.height - 3 // header + footer + 1 margin
	if h < 1 {
		h = 1
	}
	return h
}

func collTabBadge(tab string) string {
	switch tab {
	case "movies":
		return "movie"
	case "series":
		return "series"
	case "music":
		return "music"
	default:
		if len(tab) > 7 {
			return tab[:7]
		}
		return tab
	}
}

