// view.go — Bubbletea View renderer + per-region helpers for the
// ui controller. All methods are read-only on Model; mutating logic
// stays in update.go / handlers_*.go.

package ui

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/theme"
)

// ── View ──────────────────────────────────────────────────────────────────────

func (m Model) View() tea.View {
	if m.state.Width == 0 {
		return tea.NewView("Loading…")
	}
	var content string
	if m.screen == screenDetail && m.detail != nil {
		// Wrap the detail overlay in MainCardStyle (same chrome the
		// grid/list screens use) so border color, side margins, and
		// rounded corners match across screens. Reserve rows for the
		// statusbar (4) + a blank row + MainCardStyle's border+padding
		// (2) so the overlay content fits cleanly inside the frame.
		const statusBarRows = 4
		const blankRow = 1
		const cardChromeRows = 2 // top + bottom border
		overlayH := max(1, m.state.Height-statusBarRows-blankRow-cardChromeRows)
		overlay := screens.RenderDetailOverlay(
			m.detail,
			m.state.Width-4, // MainCardStyle adds margin(2) + border(2) of horizontal chrome
			overlayH,
			m.state.ActiveTab,
			m.state.RuntimeStatus.String(),
		)
		framed := theme.T.MainCardStyle(true).Width(m.state.Width - 2).Render(overlay)
		composed := lipgloss.JoinVertical(lipgloss.Left,
			framed,
			"",
			m.viewStatusBar(),
		)
		content = m.applyToast(composed)
	} else {
		// Hide the footer (statusbar + preceding blank line) only on the
		// Queue sub-tab, which uses every row for tracklist + visualizer.
		// Library/Browse/Playlists keep the footer for status messages.
		hidingFooter := m.state.ActiveTab == state.TabMusic &&
			m.musicScreen.ActiveSubTab() == screens.MusicQueue
		var parts []string
		parts = append(parts, m.viewTopBar(m.state.Focus == state.FocusSearch), "", m.viewMainCard(hidingFooter))
		if !hidingFooter {
			parts = append(parts, "", m.viewFooter())
		}
		base := lipgloss.JoinVertical(lipgloss.Left, parts...)
		content = m.applyToast(base)
	}
	v := tea.NewView(content)
	v.AltScreen = true
	v.MouseMode = tea.MouseModeCellMotion
	return v
}

func (m Model) applyToast(base string) string {
	// Prepend NowPlaying bar if playing outside the detail panel
	if m.nowPlaying != nil {
		np := components.RenderNowPlaying(m.nowPlaying, m.state.Width)
		if np != "" {
			base = np + base
		}
	}
	// MPD HUD is now rendered inline in the footer slot (viewFooter),
	// not prepended here. This keeps layout stable regardless of playback.
	// Prepend DSP status panel when DSP is enabled
	if m.dspState != nil && m.dspState.Enabled {
		dspHud := components.RenderDspStatus(m.dspState, m.state.Width)
		if dspHud != "" {
			base = dspHud + base
		}
	}
	// Subtitle / audio sync overlay
	if m.syncOverlay != nil {
		if s := components.RenderSyncOverlay(m.syncOverlay.isAudio, m.syncOverlay.delay, m.state.Width); s != "" {
			base = s + "\n" + base
		}
	}
	// Skip intro overlay
	if m.skipIntro != nil && m.nowPlaying != nil {
		pos := m.nowPlaying.Position
		if pos >= m.skipIntro.Start && pos <= m.skipIntro.End+15 {
			skipStr := components.RenderSkipPrompt("Intro", m.skipIntro.End, m.state.Width)
			if skipStr != "" {
				base = skipStr + base
			}
		}
	}
	// Skip credits overlay
	if m.skipCredits != nil && m.nowPlaying != nil {
		dur := m.nowPlaying.Duration
		pos := m.nowPlaying.Position
		if dur > 0 {
			fromEnd := dur - pos
			if fromEnd <= m.skipCredits.Start+15 && fromEnd >= m.skipCredits.End-5 {
				seekTo := dur - m.skipCredits.End + 2
				skipStr := components.RenderSkipPrompt("Credits", seekTo, m.state.Width)
				if skipStr != "" {
					base = skipStr + base
				}
			}
		}
	}
	// Binge countdown banner — appended below the main content
	if overlay := m.viewBingeOverlay(); overlay != "" {
		base = base + overlay
	}
	// Buffering overlay — shown while waiting for pre-roll or stall-guard
	if overlay := m.viewBufferingOverlay(); overlay != "" {
		base = base + overlay
	}
	if m.activeToast == nil {
		return base
	}
	toastStr := components.RenderToast(m.activeToast, m.state.Width, m.state.Height)
	if toastStr == "" {
		return base
	}
	return lipgloss.Place(
		m.state.Width, m.state.Height,
		lipgloss.Right, lipgloss.Bottom,
		toastStr,
		lipgloss.WithWhitespaceStyle(lipgloss.NewStyle()),
	)
}

// viewBingeOverlay renders the "next episode in Ns" countdown banner.
func (m Model) viewBingeOverlay() string {
	if m.bingeCountdown < 0 || m.bingeCtx == nil {
		return ""
	}
	nextIdx := m.bingeCtx.CurrentIdx + 1
	if nextIdx >= len(m.bingeCtx.Episodes) {
		return ""
	}
	ep := m.bingeCtx.Episodes[nextIdx]

	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())

	epLabel := fmt.Sprintf("S%02dE%02d", ep.Season, ep.Episode)
	if ep.Title != "" {
		epLabel += "  " + ep.Title
	}

	line1 := acc.Render("▶") + "  Next: " + neon.Render(m.bingeCtx.SeriesTitle) +
		"  " + dim.Render(epLabel)
	line2 := dim.Render(fmt.Sprintf("  Playing in %ds", m.bingeCountdown)) +
		"   " + acc.Render("[Enter]") + dim.Render(" play now") +
		"   " + dim.Render("[Esc] cancel")

	w := m.state.Width - 4
	if w < 40 {
		w = 40
	}
	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Padding(0, 2).
		Width(w).
		Render(line1 + "\n" + line2)

	return "\n" + box + "\n"
}

// viewBufferingOverlay renders a pre-roll / stall-guard progress bar.
func (m Model) viewBufferingOverlay() string {
	if m.playerBuffer == nil {
		return ""
	}
	buf := m.playerBuffer

	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())

	label := "Buffering"
	if buf.Reason == "stall_guard" {
		label = "Stall guard — paused"
	}

	// Progress bar: 24 chars wide
	const barW = 24
	filled := int(float64(barW) * buf.FillPercent / 100.0)
	if filled > barW {
		filled = barW
	}
	bar := strings.Repeat("█", filled) + strings.Repeat("░", barW-filled)

	pct := fmt.Sprintf("%.0f%%", buf.FillPercent)
	info := fmt.Sprintf("%s MiB/s", strings.TrimRight(strings.TrimRight(fmt.Sprintf("%.1f", buf.SpeedMbps), "0"), "."))
	if buf.EtaSecs > 0 {
		info += fmt.Sprintf("  ETA %ds", int(buf.EtaSecs))
	}
	if buf.PreRollSecs > 0 {
		info += fmt.Sprintf("  (pre-roll %ds)", int(buf.PreRollSecs))
	}

	line1 := acc.Render("⏳ "+label) + "  " + neon.Render(bar) + "  " + dim.Render(pct)
	line2 := dim.Render("   " + info)

	w := m.state.Width - 4
	if w < 44 {
		w = 44
	}
	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Padding(0, 1).
		Width(w).
		Render(line1 + "\n" + line2)

	return "\n" + box + "\n"
}

func (m Model) viewMainCard(footerHidden bool) string {
	focused := m.state.Focus != state.FocusSearch
	inner := m.viewMain()
	style := theme.T.MainCardStyle(focused).Width(m.state.Width - 2)
	if !footerHidden {
		style = style.MarginBottom(1)
	}
	return style.Render(inner)
}

func (m Model) viewMain() string {
	if m.state.ActiveTab == state.TabMusic {
		return m.musicScreen.View(
			m.state.IsLoading,
			m.state.LoadingStart,
			m.state.RuntimeStatus.String(),
			m.state.Plugins,
			&m.loadingSpinner,
		).Content
	}
	if m.state.ActiveTab == state.TabCollections {
		return m.collectionsScreen.View().Content
	}
	// Continue Watching row (Movies and Series tabs only)
	var cwSection string
	if items := m.cwCurrentItems(); len(items) > 0 {
		cwSection = renderContinueWatchingRow(items, m.cwCursor, m.cwFocused, m.innerWidth())
	}

	if m.screen == screenGrid || !m.state.SearchActive {
		availH := max(0, m.state.Height-12)
		grid := screens.RenderGrid(
			m.currentGridEntries(),
			m.gridCursor,
			m.innerWidth(),
			availH,
			m.state.IsLoading,
			m.state.LoadingStart,
			m.state.RuntimeStatus.String(),
			m.state.Plugins,
			&m.loadingSpinner,
		)
		if cwSection != "" {
			return lipgloss.JoinVertical(lipgloss.Left, cwSection, grid)
		}
		return grid
	}
	return lipgloss.JoinVertical(lipgloss.Left,
		m.viewColumnHeaders(),
		m.viewResults(),
	)
}

func (m Model) viewTopBar(focused bool) string {
	w := m.state.Width
	var tabParts []string
	for _, t := range state.Tabs() {
		s := fmt.Sprintf(" %s ", t.String())
		if t == m.state.ActiveTab {
			tabParts = append(tabParts, theme.T.TabActiveStyle().Render(s))
		} else {
			tabParts = append(tabParts, theme.T.TabStyle().Render(s))
		}
	}
	tabs := lipgloss.JoinHorizontal(lipgloss.Top, tabParts...)

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

	tabsW := lipgloss.Width(tabs)
	searchW := lipgloss.Width(searchBox)
	gearW := lipgloss.Width(gear)
	contentW := w - 6
	spacerLeft := max(0, (contentW/2)-tabsW-(searchW/2))
	spacerRight := max(0, contentW-tabsW-searchW-gearW-spacerLeft)

	row := tabs + strings.Repeat(" ", spacerLeft) + searchBox + strings.Repeat(" ", spacerRight) + gear
	return theme.T.TopBarStyle(focused).Width(w - 2).Render(row)
}

func (m Model) viewColumnHeaders() string {
	w := m.innerWidth()
	col := func(s string, width int) string { return theme.T.ColHeaderStyle().Width(width).Render(s) }
	titleW := w/2 - 2
	yearW, genreW, ratingW := 6, 14, 8
	provW := max(10, w-titleW-yearW-genreW-ratingW-5)
	return lipgloss.JoinHorizontal(lipgloss.Top,
		col("Title", titleW), col("Year", yearW),
		col("Genre", genreW), col("Rating", ratingW),
		col("Provider", provW),
	)
}

func (m Model) viewResults() string {
	w := m.innerWidth()
	availH := max(1, m.state.Height-9)

	if len(m.state.Results) == 0 {
		return screens.CenteredMsg(w, availH, lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("No results"))
	}

	// Virtualized list for scrollbar
	vl := components.NewVirtualizedList(len(m.state.Results), m.state.Cursor, availH)

	titleW := w/2 - 2
	yearW, genreW, ratingW := 6, 14, 8
	provW := max(10, w-titleW-yearW-genreW-ratingW-5)

	start, end := vl.VisibleRange()
	bar := components.Scrollbar(start, end-start, len(m.state.Results))

	var rows []string
	for i := start; i < end; i++ {
		r := m.state.Results[i]
		row := fmt.Sprintf("%-*s  %-*s  %-*s  %-*s  %-*s",
			titleW-2, truncate(r.Title, titleW-2),
			yearW-1, truncate(r.Year, yearW-1),
			genreW-1, truncate(r.Genre, genreW-1),
			ratingW-1, truncate(r.Rating, ratingW-1),
			provW-1, truncate(r.Provider, provW-1),
		)
		var styled string
		switch {
		case i == m.state.Cursor && m.state.Focus == state.FocusResults:
			styled = theme.T.ResultRowSelectedStyle().Width(w - 2).Render(row)
		case i == m.state.Cursor:
			styled = theme.T.ResultRowHoveredStyle().Width(w - 2).Render(row)
		case i%2 == 0:
			styled = theme.T.ResultRowStyle().Width(w - 2).Render(row)
		default:
			styled = theme.T.ResultRowAltStyle().Width(w - 2).Render(row)
		}
		rows = append(rows, styled)
	}

	// Place scrollbar as a separate column adjacent to the rows.
	content := lipgloss.JoinHorizontal(lipgloss.Top, strings.Join(rows, "\n"), " ", bar)

	return theme.T.ResultsPanelStyle().Width(w).Height(availH).Render(content)
}

// viewFooter renders either the compact MPD now-playing bar (when playing)
// or the normal status bar (when stopped/paused). Same size either way.
func (m Model) viewFooter() string {
	if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
		return m.viewMpdFooter()
	}
	return m.viewStatusBar()
}

func fmtDuration(secs float64) string {
	m := int(secs) / 60
	s := int(secs) % 60
	return fmt.Sprintf("%d:%02d", m, s)
}

// viewMpdFooter renders a compact now-playing bar that fits in the footer slot.
func (m Model) viewMpdFooter() string {
	w := m.state.Width
	np := m.mpdNowPlaying
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextMuted())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())

	// State icon
	icon := accentStyle.Render("▶")

	// Artist - Title (truncated to fit)
	track := np.Title
	if np.Artist != "" {
		track = np.Artist + " — " + track
	}

	// Time
	elapsed := fmtDuration(np.Elapsed)
	total := fmtDuration(np.Duration)
	timeStr := dimStyle.Render(fmt.Sprintf(" %s/%s ", elapsed, total))

	// Volume
	volStr := dimStyle.Render(fmt.Sprintf(" %d%% ", np.Volume))

	// Seekbar — fill remaining space
	contentW := w - 8                                                  // account for StatusBarStyle margins/padding/border
	fixedW := 2 + lipgloss.Width(timeStr) + lipgloss.Width(volStr) + 2 // icon + gaps
	trackMaxW := (contentW - fixedW) / 2
	if trackMaxW < 10 {
		trackMaxW = 10
	}
	if len([]rune(track)) > trackMaxW {
		track = string([]rune(track)[:trackMaxW-1]) + "…"
	}
	trackStr := textStyle.Render(" " + track + " ")

	barW := contentW - 2 - lipgloss.Width(trackStr) - lipgloss.Width(timeStr) - lipgloss.Width(volStr)
	if barW < 5 {
		barW = 5
	}
	var seekBar string
	if np.Duration > 0 {
		filled := int(np.Elapsed / np.Duration * float64(barW))
		if filled > barW {
			filled = barW
		}
		seekBar = accentStyle.Render(strings.Repeat("━", filled)) +
			dimStyle.Render(strings.Repeat("─", barW-filled))
	} else {
		seekBar = dimStyle.Render(strings.Repeat("─", barW))
	}

	bar := icon + trackStr + seekBar + timeStr + volStr
	return theme.T.StatusBarStyle().Width(w - 2).Render(bar)
}

func (m Model) viewStatusBar() string {
	w := m.state.Width

	var pill string
	switch m.state.RuntimeStatus {
	case state.RuntimeReady:
		pill = theme.T.StatusAccentStyle().Render(" stui ")
	case state.RuntimeConnecting:
		pill = theme.T.StatusAccentStyle().Background(theme.T.Yellow()).Render(" stui ")
	case state.RuntimeError:
		pill = theme.T.StatusAccentStyle().Background(theme.T.Red()).Render(" stui ")
	default:
		pill = theme.T.StatusAccentStyle().Background(theme.T.TextDim()).Render(" stui ")
	}

	var screenIndicator string
	switch m.screen {
	case screenGrid:
		screenIndicator = lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Render("  ▦ grid")
	case screenList:
		screenIndicator = lipgloss.NewStyle().Foreground(theme.T.TextMuted()).Render("  ≡ list")
	case screenDetail:
		screenIndicator = lipgloss.NewStyle().Foreground(theme.T.Neon()).Render("  ◈ detail")
	}

	// While the Music tab is active each sub-screen publishes its own
	// hint/status text; that supersedes the global StatusMsg slot so the
	// stale "Added X to queue" line from a previous action doesn't sit
	// in the footer forever (the sub-screens apply their own statusTTL
	// before reverting to a key-hint string). The detail overlay uses
	// the same pattern — its focus-specific hotkey hints live here
	// instead of in a handwritten header bar.
	statusText := m.state.StatusMsg
	if m.state.ActiveTab == state.TabMusic {
		if t := m.musicScreen.FooterText(); t != "" {
			statusText = t
		}
	}
	if m.screen == screenDetail && m.detail != nil {
		if t := m.detail.FooterText(); t != "" {
			statusText = t
		}
	}
	statusMsg := lipgloss.NewStyle().Foreground(theme.T.TextMuted()).Render("  " + statusText)

	count := len(m.currentGridEntries())
	if m.screen == screenList {
		count = len(m.state.Results)
	}
	right := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).
		Render(fmt.Sprintf("%s  %d titles  v toggle  R refresh ", m.state.ActiveTab.String(), count))

	contentW := w - 8
	gap := max(0, contentW-lipgloss.Width(pill)-lipgloss.Width(screenIndicator)-lipgloss.Width(statusMsg)-lipgloss.Width(right))
	bar := pill + screenIndicator + statusMsg + strings.Repeat(" ", gap) + right
	return theme.T.StatusBarStyle().Width(w - 2).Render(bar)
}
