// mouse.go — mouse-event router for the ui controller plus the
// hit-test helpers that translate cursor coordinates into widget
// targets (top tab bar, search box, gear icon, etc.).

package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/theme"
)

// lastClick tracks the last click position and time for double-click detection.
var lastClick struct {
	x, y    int
	time    time.Time
}

const doubleClickThreshold = 300 * time.Millisecond

func (m Model) handleMouse(msg tea.MouseMsg) (tea.Model, tea.Cmd) {
	mouse := msg.Mouse()
	switch {
	case mouse.Button == tea.MouseWheelUp:
		return m.handleKey(tea.KeyPressMsg{Code: 'k', Text: "k"})
	case mouse.Button == tea.MouseWheelDown:
		return m.handleKey(tea.KeyPressMsg{Code: 'j', Text: "j"})
	case mouse.Button == tea.MouseRight:
		// Right-click is currently only meaningful in the Music tab so it
		// can open the per-track context dialog (Add to queue / Replace
		// queue / Add to Playlist / Create Playlist). Other tabs ignore.
		if m.state.ActiveTab != state.TabMusic {
			return m, nil
		}
		topBarY := m.overlayRowCount()
		const topBarTotalRowsR = 5
		relY := mouse.Y - topBarY - topBarTotalRowsR - 1
		cardX := mouse.X - 3
		var cmd tea.Cmd
		m.musicScreen, cmd = m.musicScreen.HandleRightMouse(cardX, relY)
		return m, cmd
	case mouse.Button == tea.MouseLeft:
		topBarY := m.overlayRowCount()
		y := mouse.Y
		x := mouse.X
		// TopBarStyle: MarginTop(1) + border-top(1) + content(1) + border-bottom(1) = 4 rows.
		// Content row (tabs/search/gear) is at topBarY+2.
		// After topbar + 1 blank gap row, main content starts at topBarY+5.
		const topBarContentOffset = 2 // MarginTop + border-top
		const topBarTotalRows = 5     // 4 topbar rows + 1 gap blank line
		if y == topBarY+topBarContentOffset {
			// Click on top tab bar — hit-test tabs, search, and gear.
			if tab, ok := m.hitTestTopTabBar(x); ok {
				m.switchTab(tab)
				return m, nil
			}
			if next, cmd, hit := m.hitTestTopBarWidgets(x); hit {
				return next, cmd
			}
			return m, nil
		}
		if m.state.ActiveTab == state.TabMusic {
			// Music content starts after: overlay rows (topBarY) + topbar (4) + gap (1)
			// The sub-tab bar is at row 0 of the MusicScreen area.
			relY := y - topBarY - topBarTotalRows
			prev := m.musicScreen.ActiveSubTab()
			var cmd tea.Cmd
			m.musicScreen, cmd = m.musicScreen.HandleMouse(x, relY)
			if m.musicScreen.ActiveSubTab() != prev {
				return m, tea.Batch(cmd, m.sessionSaveCmd())
			}
			return m, cmd
		}
		if m.state.ActiveTab == state.TabCollections {
			var cmd tea.Cmd
			m.collectionsScreen, cmd = m.collectionsScreen.Update(msg)
			return m, cmd
		}
		// Detail screen tab-bar click routing. The tab bar sits at a
		// known offset below the header (poster + info column) inside
		// the MainCardStyle frame. The header rows are
		// `detailPosterHeight + 4` tall; the tab bar's clickable label
		// row is the second of the three-row tab bar. Account for:
		//   - overlay rows (toast, mpd hud, etc.) → topBarY
		//   - top tab bar (5 rows) — topBarTotalRows
		//   - MainCardStyle border-top (1) + padding (0)
		//   - header rows (detailPosterHeight + 4 = 18)
		//   - tab bar border-top (1)
		//   - label row at offset +1 inside tab bar
		// Detail screen also uses `MainCardStyle` margin(1)+border(1) =
		// 2 cols of left chrome, so x is shifted right by 2.
		if m.screen == screenDetail && m.detail != nil {
			const detailHeaderH = 18 // detailPosterHeight (14) + 4
			const cardChromeRows = 1 // MainCardStyle border-top
			tabLabelRow := topBarY + topBarTotalRows + cardChromeRows + detailHeaderH + 1
			if y == tabLabelRow {
				cardX := x - 2 // MainCardStyle margin(1) + border(1)
				if cardX < 0 {
					return m, nil
				}
				if tab, ok := m.detail.HitTestDetailTabBar(cardX); ok {
					m.detail.ActiveTab = tab
					m.detail.Focus = defaultFocusForTab(tab)
					return m, m.maybeLoadEpisodesForTab(m.detail)
				}
			}
			return m, nil
		}
		// Movies/Series mouse handling: calculate relative coordinates
		// and update grid cursor based on click position.
		// Only handle grid mouse clicks when NOT in detail view (screenDetail)
		// When viewing a series/movie detail (Episode screen), mouse should be handled there.
		if m.screen != screenDetail && (m.state.ActiveTab == state.TabMovies || m.state.ActiveTab == state.TabSeries) {
			// Calculate relative Y to the grid area.
			// Main content starts after topbar (topBarY + topBarTotalRows + 1 for border)
			contentStartY := topBarY + topBarTotalRows + 1
			relY := y - contentStartY
			cardX := x - 2 // MainCardStyle: margin(1) + border(1), cards start here
			
			// If click is within valid grid area, update cursor.
			if relY >= 0 && cardX >= 0 {
				// Get actual card dimensions from components (same as grid rendering)
				rowH := components.CardTotalRows
				cols := components.CardColumns
				cardW := components.CardWidth(m.innerWidth())
				
				// Calculate grid position from click coordinates.
				row := relY / rowH
				col := cardX / cardW
				
				// Calculate cursor index from row/col.
				idx := row*cols + col
				entries := m.currentGridEntries()
				if idx >= 0 && idx < len(entries) {
					// Check for double-click (same position within threshold)
					isDoubleClick := (x == lastClick.x && y == lastClick.y) && 
						time.Since(lastClick.time) < doubleClickThreshold
					lastClick.x = x
					lastClick.y = y
					lastClick.time = time.Now()
					
					m.gridCursor = screens.GridCursor{Row: row, Col: col}
					
					// Double-click opens detail (same as Enter)
					if isDoubleClick {
						return m, m.openDetail(entries[idx])
					}
				}
			}
			return m, nil
		}
		if m.screen == screenList {
			// Click on a result row in the list view.
			topBarRows := topBarY + topBarTotalRows // overlay rows + topbar rows + gap
			colHeaderRow := topBarRows              // column header row
			bodyStartY := colHeaderRow + 1          // result rows start here
			bodyRow := y - bodyStartY
			if bodyRow >= 0 {
				availH := max(1, m.state.Height-9)
				start := 0
				if m.state.Cursor >= availH {
					start = m.state.Cursor - availH + 1
				}
				idx := start + bodyRow
				if idx >= 0 && idx < len(m.state.Results) {
					m.state.Cursor = idx
					m.state.Focus = state.FocusResults
				}
			}
			return m, nil
		}
	}
	return m, nil
}

// overlayRowCount returns the number of rows prepended above the main content
// by applyToast (NowPlaying bar for non-MPD playback, DSP status).
// MPD HUD is no longer prepended — it lives in the footer slot.
func (m Model) overlayRowCount() int {
	n := 0
	if m.nowPlaying != nil {
		s := components.RenderNowPlaying(m.nowPlaying, m.state.Width)
		if s != "" {
			n += strings.Count(s, "\n")
		}
	}
	if m.dspState != nil && m.dspState.Enabled {
		dspHud := components.RenderDspStatus(m.dspState, m.state.Width)
		if dspHud != "" {
			n += strings.Count(dspHud, "\n")
		}
	}
	return n
}

// hitTestTopTabBar returns the Tab the user clicked based on the X coordinate.
func (m Model) hitTestTopTabBar(x int) (state.Tab, bool) {
	pos := 3 // MarginLeft(1) + BorderLeft(1) + PaddingLeft(1)
	for _, t := range state.Tabs() {
		label := fmt.Sprintf(" %s ", t.String())
		var rendered string
		if t == m.state.ActiveTab {
			rendered = theme.T.TabActiveStyle().Render(label)
		} else {
			rendered = theme.T.TabStyle().Render(label)
		}
		w := lipgloss.Width(rendered)
		if x >= pos && x < pos+w {
			return t, true
		}
		pos += w
	}
	return 0, false
}

// hitTestTopBarWidgets returns the (possibly updated) Model, a command,
// and whether the click landed on a top-bar widget. The Model must be
// returned because focusing the search bar mutates state that needs to
// propagate back to the Bubble Tea program.
func (m Model) hitTestTopBarWidgets(x int) (Model, tea.Cmd, bool) {
	w := m.state.Width
	// Compute tab strip width (same logic as viewTopBar / hitTestTopTabBar).
	tabsW := 0
	for _, t := range state.Tabs() {
		label := fmt.Sprintf(" %s ", t.String())
		var rendered string
		if t == m.state.ActiveTab {
			rendered = theme.T.TabActiveStyle().Render(label)
		} else {
			rendered = theme.T.TabStyle().Render(label)
		}
		tabsW += lipgloss.Width(rendered)
	}

	// Compute search box and gear widths (same logic as viewTopBar).
	prefix := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Render("⌕ ")
	var searchBox string
	switch {
	case m.state.Focus == state.FocusSearch:
		searchBox = theme.T.SearchFocusedStyle().Render(prefix + m.search.View())
	case m.search.Value() != "":
		searchBox = theme.T.SearchStyle().Render(prefix + lipgloss.NewStyle().Foreground(theme.T.Text()).Render(m.search.Value()))
	default:
		searchBox = theme.T.SearchStyle().Render(prefix + lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("Search…  /"))
	}
	var gear string
	switch m.state.RuntimeStatus {
	case state.RuntimeError:
		gear = theme.T.GearStyle().Foreground(theme.T.Red()).Render("⚙")
	case state.RuntimeReady:
		gear = theme.T.GearFocusedStyle().Render("⚙")
	default:
		gear = theme.T.GearStyle().Render("⚙")
	}
	searchW := lipgloss.Width(searchBox)
	gearW := lipgloss.Width(gear)

	contentW := w - 6
	spacerLeft := max(0, (contentW/2)-tabsW-(searchW/2))
	// TopBarStyle has MarginLeft(1) + BorderLeft(1) + PaddingLeft(1) = 3.
	const topBarPaddingLeft = 3
	searchStart := topBarPaddingLeft + tabsW + spacerLeft
	searchEnd := searchStart + searchW
	gearStart := searchEnd + max(0, contentW-tabsW-searchW-gearW-spacerLeft)
	gearEnd := gearStart + gearW

	switch {
	case x >= searchStart && x < searchEnd:
		m.state.Focus = state.FocusSearch
		m.state.SearchActive = true
		return m, m.search.Focus(), true
	case x >= gearStart && x < gearEnd:
		return m, screen.OpenOverlayCmd(screens.NewSettingsModel(m.client, m.cfg)), true
	}
	return m, nil, false
}
