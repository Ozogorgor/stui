// Package catalogbrowser provides the reusable N-column catalog browser
// component extracted from MusicLibraryScreen (Task 5.2).
//
// The component owns:
//   - Per-column cursor + scroll state
//   - j/k/up/down navigation within a column
//   - h/l/left/right focus transitions between columns
//   - Column-line rendering (buildPaneLines) and the border-wrapped layout
//   - Header row
//
// The component does NOT own:
//   - Dialog / action menus — those are caller-specific
//   - MPD IPC calls on cursor movement — the DataSource or caller handles that
//   - Directory mode — MPD-specific, stays in MusicLibraryScreen
//   - Tag-normalization exception marking — MPD-specific
//   - Mouse hit-testing — callers own mouse events, then call SetCursor
package catalogbrowser

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

// ColumnDef describes a single column in the browser.
type ColumnDef struct {
	Kind  ipc.EntryKind
	Label string // header label, e.g. "Artists", "Albums", "Tracks"
}

// ExtraColumn describes an optional extra column shown alongside the main columns.
type ExtraColumn struct {
	Label string   // header label, e.g. "Track Info"
	Lines []string // rendered lines for display
}

// NavMsg is posted by the browser when the user navigates so the caller
// can react (e.g. pre-fetch the next column's data from MPD).
type NavMsg struct {
	Column int // 0-based column index that moved
	Row    int // new cursor row within that column
}

// Model is the reusable N-column catalog browser.
//
// Construction: call New with a DataSource and a slice of ColumnDef.
// The number of columns matches len(cols); typical usage is 3 for
// Artists | Albums | Tracks.
//
// Callers intercept library-specific key events first, then pass the
// remainder to Model.Update. Callers own mouse handling and call SetCursor
// to synchronise position after a click.
type Model struct {
	src          DataSource
	cols         []ColumnDef
	cursors      []int // per-column cursor row
	activeColumn int   // index of the focused column
	width        int
	height       int
}

// New constructs a browser model with the given data source and columns.
// cursors start at row 0 for every column.
func New(src DataSource, cols []ColumnDef) Model {
	return Model{
		src:     src,
		cols:    cols,
		cursors: make([]int, len(cols)),
	}
}

// Source returns the underlying DataSource.
func (m Model) Source() DataSource { return m.src }

// ActiveColumn returns the index of the currently focused column.
func (m Model) ActiveColumn() int {
	return m.activeColumn
}

// ColumnCursor returns the cursor row for column col.
func (m Model) ColumnCursor(col int) int {
	if col < 0 || col >= len(m.cursors) {
		return 0
	}
	return m.cursors[col]
}

// SetSize stores the available terminal dimensions. Call from the parent's
// WindowSizeMsg handler.
func (m *Model) SetSize(w, h int) {
	m.width = w
	m.height = h
}

// SetCursor directly sets the cursor row for a column (used by mouse
// handlers in the parent screen). No bounds checking — callers must
// validate before calling.
func (m *Model) SetCursor(col, row int) {
	if col >= 0 && col < len(m.cursors) {
		m.cursors[col] = row
	}
}

// SetActiveColumn moves keyboard focus to the given column index.
func (m *Model) SetActiveColumn(col int) {
	if col < 0 || col >= len(m.cols) {
		return
	}
	m.activeColumn = col
}

// Update processes Bubbletea messages. Only generic navigation keys (j/k/up/down,
// h/l/left/right) are handled here. Callers must consume library-specific
// keys BEFORE delegating to Update.
//
// Returns (Model, NavMsg-emitting tea.Cmd | nil). If the user moved within a
// column the returned Cmd posts a NavMsg so the parent can pre-fetch the next
// column's data.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height

	case tea.KeyPressMsg:
		return m.HandleKey(msg.String())
	}
	return m, nil
}

// HandleKey processes a navigation key string and returns the updated model
// plus an optional NavMsg command. Exported so callers can pass a pre-parsed
// key string directly (e.g. from their own KeyPressMsg handler) without
// needing to reconstruct a tea.KeyPressMsg.
func (m Model) HandleKey(key string) (Model, tea.Cmd) {
	active := m.ActiveColumn()
	nCols := len(m.cols)

	switch key {
	case "j", "down":
		if active < 0 || active >= nCols {
			break
		}
		items := m.src.Items(m.cols[active].Kind)
		if m.cursors[active] < len(items)-1 {
			m.cursors[active]++
			col := active
			row := m.cursors[active]
			return m, func() tea.Msg { return NavMsg{Column: col, Row: row} }
		}

	case "k", "up":
		if active < 0 || active >= nCols {
			break
		}
		if m.cursors[active] > 0 {
			m.cursors[active]--
			col := active
			row := m.cursors[active]
			return m, func() tea.Msg { return NavMsg{Column: col, Row: row} }
		}

	case "l", "right":
		if active < nCols-1 {
			m.activeColumn = active + 1
			// No NavMsg on focus-only move — parent already has data for
			// the next column (it pre-fetched on the previous down/up).
		}

	case "h", "left":
		if active > 0 {
			m.activeColumn = active - 1
		}
	}

	return m, nil
}

// View renders the N-column tag browser within the configured width/height.
// accentStyle, dimStyle, textStyle are passed in so the caller's theme
// preferences apply without coupling the component to a global theme call.
//
// The optional extraCol parameter lets the caller inject an extra rendered
// column to the right (e.g. MusicLibrary's "Track Info" sidebar). Pass nil
// to render exactly the defined columns.
func (m Model) View(
	accentStyle, dimStyle, textStyle lipgloss.Style,
	loadingByCol []bool,
	loadingTextByCol []string,
	emptyTextByCol []string,
	extraCol *ExtraColumn, // optional extra column with label and lines, or nil
) string {
	w := m.width
	h := m.height

	nCols := len(m.cols)
	if nCols == 0 {
		return ""
	}

	// Total height budget = h. Subtract: 1 header row + 2 border rows.
	listH := h - 3
	if listH < 1 {
		listH = 1
	}

	totalCols := nCols
	if extraCol != nil {
		totalCols++
	}

	// Inner content width: outer = w-2, minus border(2)+padding(2) = content w-6.
	// Then subtract (totalCols-1) separator chars.
	contentW := w - 6 - (totalCols - 1)
	paneW := contentW / totalCols
	if paneW < 10 {
		paneW = 10
	}

	borderStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 1)

	var sb strings.Builder

	// Header
	active := m.ActiveColumn()
	sep := dimStyle.Render("│")
	headerParts := make([]string, 0, totalCols)
	for i, col := range m.cols {
		headerParts = append(headerParts, renderHeader(col.Label, i == active, paneW, accentStyle, dimStyle))
	}
	if extraCol != nil {
		headerParts = append(headerParts, renderHeader(extraCol.Label, false, paneW, accentStyle, dimStyle))
	}
	sb.WriteString(strings.Join(headerParts, sep) + "\n")

	// Build each column's lines
	colLines := make([][]string, nCols)
	for i, col := range m.cols {
		items := m.src.Items(col.Kind)
		names := make([]string, len(items))
		for j, e := range items {
			names[j] = e.Title
		}
		loading := i < len(loadingByCol) && loadingByCol[i]
		loadingText := "Loading…"
		if i < len(loadingTextByCol) && loadingTextByCol[i] != "" {
			loadingText = loadingTextByCol[i]
		}
		emptyText := "No items"
		if i < len(emptyTextByCol) && emptyTextByCol[i] != "" {
			emptyText = emptyTextByCol[i]
		}
		cursor := 0
		if i < len(m.cursors) {
			cursor = m.cursors[i]
		}
		colLines[i] = buildPaneLines(
			names, cursor, listH,
			i == active, loading,
			loadingText, emptyText,
			paneW, accentStyle, dimStyle, textStyle,
		)
	}

	// Assemble rows
	var paneContent strings.Builder
	for row := 0; row < listH; row++ {
		parts := make([]string, 0, totalCols)
		for i := 0; i < nCols; i++ {
			l := ""
			if row < len(colLines[i]) {
				l = colLines[i][row]
			}
			parts = append(parts, l)
		}
		if extraCol != nil {
			l := ""
			if row < len(extraCol.Lines) {
				l = extraCol.Lines[row]
			}
			parts = append(parts, l)
		}
		paneContent.WriteString(strings.Join(parts, sep) + "\n")
	}

	body := strings.TrimRight(paneContent.String(), "\n")
	borderedContent := borderStyle.Width(w - 2).Render(body)
	sb.WriteString(borderedContent)

	return sb.String()
}

// ColScroll returns the scroll offset for a column given a window height,
// using the center-scroll algorithm from music_library's libScroll.
// Exported so callers (mouse handlers) can compute the same scroll offset
// used during rendering.
func (m Model) ColScroll(col, listH int) int {
	if col < 0 || col >= len(m.cols) || m.src == nil {
		return 0
	}
	items := m.src.Items(m.cols[col].Kind)
	cursor := m.cursors[col]
	return colScroll(len(items), cursor, listH)
}

// colScroll computes the scroll offset, mirroring music_library's libScroll.
func colScroll(n, cursor, maxH int) int {
	vl := components.NewVirtualizedList(n, cursor, maxH, components.WithScrollMode(components.ScrollModeCenter))
	start, _ := vl.VisibleRange()
	return start
}

// renderHeader renders a single column header cell padded to paneW.
func renderHeader(label string, active bool, paneW int, accentStyle, dimStyle lipgloss.Style) string {
	padded := fmt.Sprintf("  %-*s", paneW-2, label)
	if active {
		return accentStyle.Render(padded)
	}
	return dimStyle.Render(padded)
}

// buildPaneLines renders one column. This is a pure function extracted from
// MusicLibraryScreen.buildPaneLines. The caller provides the name strings so
// the component remains decoupled from the concrete MPD types.
func buildPaneLines(
	items []string,
	cursor, maxH int,
	active, loading bool,
	loadingText, emptyText string,
	paneW int,
	accentStyle, dimStyle, textStyle lipgloss.Style,
) []string {
	var lines []string

	if loading {
		lines = append(lines, dimStyle.Render(fmt.Sprintf("  %-*s", paneW-2, loadingText)))
		for len(lines) < maxH {
			lines = append(lines, strings.Repeat(" ", paneW))
		}
		return lines
	}

	if len(items) == 0 {
		lines = append(lines, dimStyle.Render(fmt.Sprintf("  %-*s", paneW-2, emptyText)))
		for len(lines) < maxH {
			lines = append(lines, strings.Repeat(" ", paneW))
		}
		return lines
	}

	// Center-scroll: keep the cursor row vertically centered.
	scroll := 0
	if len(items) > maxH {
		scroll = cursor - maxH/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(items)-maxH {
			scroll = len(items) - maxH
		}
	}

	end := scroll + maxH
	if end > len(items) {
		end = len(items)
	}

	for i := scroll; i < end; i++ {
		isCursor := i == cursor
		prefix := "  "
		var style lipgloss.Style
		if isCursor && active {
			prefix = "▶ "
			style = accentStyle
		} else if isCursor {
			prefix = "▶ "
			style = textStyle
		} else {
			style = textStyle
		}
		label := truncate(items[i], paneW-2)
		line := style.Render(fmt.Sprintf("%s%-*s", prefix, paneW-2, label))
		lines = append(lines, line)
	}

	for len(lines) < maxH {
		lines = append(lines, strings.Repeat(" ", paneW))
	}
	return lines
}

// truncate truncates s to at most max runes, appending "…" if truncated.
func truncate(s string, max int) string {
	runes := []rune(s)
	if len(runes) <= max {
		return s
	}
	if max <= 1 {
		return "…"
	}
	return string(runes[:max-1]) + "…"
}
