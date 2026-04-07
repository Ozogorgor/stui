package screens

// music_queue.go — Queue sub-tab: the live MPD playback queue.
//
// Layout (wide, >100 cols):
//
//	┌─ Queue (12 tracks · 48:32) ──────────────────────┬──────────────────┐
//	│  ▶  1  Bohemian Rhapsody       5:55  Queen        │  Bohemian        │
//	│     2  Don't Stop Me Now       3:29  Queen        │  Rhapsody        │
//	│     3  We Will Rock You        2:01  Queen        │                  │
//	│     …                                             │  Queen           │
//	│                                                   │  A Night at the  │
//	│                                                   │  Opera · 1975    │
//	└───────────────────────────────────────────────────┴──────────────────┘
//	enter play · d remove · c clear · g top · G bottom
//
// Layout (narrow, ≤100 cols): only the left track list is shown.

import (
	"fmt"
	"math"
	"strings"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

// MusicQueueScreen displays and controls the live MPD playback queue.
type MusicQueueScreen struct {
	Dims
	client     *ipc.Client
	tracks     []ipc.MpdTrack
	cursor     int
	loading    bool
	nowTitle   string // from MpdStatusMsg — used to highlight current track
	nowArtist  string
	nowSongID  int32 // from MpdStatusMsg.SongID; 0 if unknown
	nowSongPos int32 // from MpdStatusMsg.SongPos; -1 if unknown
	spinner    components.Spinner

	// Now-playing state from MpdStatusMsg
	nowElapsed  float64
	nowDuration float64
	nowVolume   uint32
	prevVolume  uint32 // saved before local mute toggle
	nowMuted    bool

	// Visualizer reference — set by MusicScreen.SetVisualizer
	visualizer *components.Visualizer
}

// NewMusicQueueScreen creates a new queue screen and triggers the initial fetch.
func NewMusicQueueScreen(client *ipc.Client) MusicQueueScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	s := MusicQueueScreen{
		client:     client,
		loading:    true,
		nowSongPos: -1,
		spinner:    *components.NewSpinner("loading queue…", dimStyle),
	}
	if client != nil {
		client.MpdGetQueue()
	}
	return s
}

// fmtMusicDuration formats seconds as "m:ss" or "h:mm:ss" for >= 3600 seconds.
func fmtMusicDuration(secs float64) string {
	total := int(math.Round(secs))
	if total < 0 {
		total = 0
	}
	h := total / 3600
	m := (total % 3600) / 60
	s := total % 60
	if h > 0 {
		return fmt.Sprintf("%d:%02d:%02d", h, m, s)
	}
	return fmt.Sprintf("%d:%02d", m, s)
}

// totalDuration returns the sum of all track durations in the queue.
func (s MusicQueueScreen) totalDuration() float64 {
	var total float64
	for _, t := range s.tracks {
		total += t.Duration
	}
	return total
}

// isCurrentTrack returns true if the given track is the currently playing one.
func (s MusicQueueScreen) isCurrentTrack(t ipc.MpdTrack) bool {
	if s.nowSongID != 0 {
		return int32(t.ID) == s.nowSongID
	}
	if s.nowSongPos >= 0 {
		return t.Pos == uint32(s.nowSongPos)
	}
	return t.Title == s.nowTitle && t.Artist == s.nowArtist
}

// Update handles incoming messages and key events.
func (s MusicQueueScreen) Update(msg tea.Msg) (MusicQueueScreen, tea.Cmd) {
	switch m := msg.(type) {

	case spinner.TickMsg:
		_, cmd := s.spinner.Update(m)
		return s, cmd

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case ipc.MpdQueueResultMsg:
		if m.Err == nil {
			s.tracks = m.Tracks
		}
		s.loading = false
		s.spinner.Stop()

	case ipc.MpdQueueChangedMsg:
		s.loading = true
		s.spinner.Start()
		return s, func() tea.Msg {
			s.client.MpdGetQueue()
			return nil
		}

	case ipc.MpdStatusMsg:
		s.nowTitle = m.SongTitle
		s.nowArtist = m.SongArtist
		s.nowSongID = m.SongID
		s.nowSongPos = m.SongPos
		s.nowElapsed = m.Elapsed
		s.nowDuration = m.Duration
		s.nowVolume = m.Volume
		// External volume change clears local mute state
		if s.nowMuted && m.Volume > 0 {
			s.nowMuted = false
		}

	case tea.KeyPressMsg:
		switch m.String() {
		case "j", "down":
			if s.cursor < len(s.tracks)-1 {
				s.cursor++
			}
		case "k", "up":
			if s.cursor > 0 {
				s.cursor--
			}
		case "g":
			s.cursor = 0
		case "G":
			if len(s.tracks) > 0 {
				s.cursor = len(s.tracks) - 1
			}
		case "0":
			if s.nowMuted {
				// unmute: restore saved volume
				if s.client != nil {
					s.client.MpdCmd("mpd_set_volume", map[string]any{"volume": int(s.prevVolume)})
				}
				s.nowMuted = false
			} else {
				// mute: save current volume (even if 0)
				s.prevVolume = s.nowVolume
				if s.client != nil {
					s.client.MpdCmd("mpd_set_volume", map[string]any{"volume": 0})
				}
				s.nowMuted = true
			}
		case "+", "=":
			vol := int(s.nowVolume) + 5
			if vol > 100 {
				vol = 100
			}
			if s.client != nil {
				s.client.MpdCmd("mpd_set_volume", map[string]any{"volume": vol})
			}
			s.nowVolume = uint32(vol) // optimistic update so display refreshes immediately
			s.nowMuted = false

		case "-":
			vol := int(s.nowVolume) - 5
			if vol < 0 {
				vol = 0
			}
			if s.client != nil {
				s.client.MpdCmd("mpd_set_volume", map[string]any{"volume": vol})
			}
			s.nowVolume = uint32(vol) // optimistic update

		case "<":
			if s.nowDuration > 0 && s.client != nil {
				seekTime := s.nowElapsed - 5
				if seekTime < 0 {
					seekTime = 0
				}
				s.client.MpdCmd("mpd_seek", map[string]any{"id": s.nowSongID, "time": seekTime})
			}
		case ">":
			if s.nowDuration > 0 && s.client != nil {
				seekTime := s.nowElapsed + 5
				if seekTime > s.nowDuration {
					seekTime = s.nowDuration
				}
				s.client.MpdCmd("mpd_seek", map[string]any{"id": s.nowSongID, "time": seekTime})
			}
		case "d", "delete":
			if len(s.tracks) > 0 && s.cursor < len(s.tracks) {
				trackID := s.tracks[s.cursor].ID
				s.client.MpdCmd("mpd_remove", map[string]any{"id": trackID})
			}
		case "c":
			s.client.MpdCmd("mpd_clear", nil)
		case "enter":
			if len(s.tracks) > 0 && s.cursor < len(s.tracks) {
				trackID := s.tracks[s.cursor].ID
				s.client.MpdCmd("mpd_play_id", map[string]any{"id": trackID})
			}
		}
	}

	return s, nil
}

// queueArtPlaceholder returns a fixed 9-row art placeholder box (20ch wide).
func queueArtPlaceholder() string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	boxStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.TextDim()).
		Width(18).
		Height(9).
		Align(lipgloss.Center, lipgloss.Center)
	return boxStyle.Render(dim.Render("♪")) + "\n"
}

// queueSeekBar returns (barRow, timeRow) for the progress display.
// barRow is 20 chars of ━/╸/─. timeRow shows elapsed and total, padded to 20ch.
func queueSeekBar(elapsed, duration float64) (barRow, timeRow string) {
	const w = 20
	// When duration == 0, return all dashes (no cursor tip)
	if duration <= 0 {
		barRow  = strings.Repeat("─", w)
		timeRow = "0:00" + strings.Repeat(" ", w-8) + "0:00"
		return
	}
	filled := int(elapsed / duration * w)
	if filled > w-1 {
		filled = w - 1
	}
	var b strings.Builder
	for i := 0; i < w; i++ {
		switch {
		case i < filled:
			b.WriteRune('━')
		case i == filled:
			b.WriteRune('╸')
		default:
			b.WriteRune('─')
		}
	}
	barRow = b.String()

	elStr := fmtMusicDuration(elapsed)
	totStr := fmtMusicDuration(duration)
	pad := w - len(elStr) - len(totStr)
	if pad < 1 {
		pad = 1
	}
	timeRow = elStr + strings.Repeat(" ", pad) + totStr
	return
}

// queueVolumeBar returns (barRow, hintRow) for the volume display.
func queueVolumeBar(volume uint32, muted bool) (barRow, hintRow string) {
	filled := int(volume / 10)
	empty  := 10 - filled
	bar := strings.Repeat("▮", filled) + strings.Repeat("▯", empty)
	barRow = fmt.Sprintf("%s  %d%%", bar, volume)
	if muted {
		hintRow = "+ vol  - vol  0 unmute"
	} else {
		hintRow = "+ vol  - vol  0 mute"
	}
	return
}

// padRightANSI pads s with spaces on the right to reach visual width w.
// Uses lipgloss.Width to correctly handle ANSI escape codes.
func padRightANSI(s string, w int) string {
	vis := lipgloss.Width(s)
	if vis >= w {
		return s
	}
	return s + strings.Repeat(" ", w-vis)
}

// View renders the queue screen within the given width/height constraints.
func (s MusicQueueScreen) View(w, h int) string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle    := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle   := lipgloss.NewStyle().Foreground(theme.T.Text())
	cursorStyle := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Bold(true)

	// ── Narrow layout (≤80 cols): existing single-column behaviour ────────
	if w <= 80 {
		return s.viewNarrow(w, h, accentStyle, dimStyle, textStyle, cursorStyle)
	}

	// Wide layout: two bordered boxes side by side
	const rightBoxW = 22  // outer width of right box (border + 20 inner + border)
	leftBoxW := w - rightBoxW  // outer width of left box
	innerL   := leftBoxW - 2   // inner content width of left box
	const innerR = 20           // inner content width of right box

	vizHeight := 0
	if s.visualizer != nil && s.visualizer.IsRunning() {
		vizHeight = s.visualizer.Config().Height
	}

	// Box outer height: all rows minus visualizer
	boxH := h - vizHeight
	if boxH < 3 {
		boxH = 3
	}
	innerBoxH := boxH - 2  // inner content rows (border top+bottom = 2)

	// Track rows = innerBoxH - 1 (first inner row is column header)
	TH := innerBoxH - 1
	if TH < 1 {
		TH = 1
	}

	// ── Column widths ─────────────────────────────────────────────────────
	titleW, artistW, albumW := queueColWidths(innerL)

	// ── Column headers row ────────────────────────────────────────────────
	var colHeaderRaw string
	if albumW > 0 {
		colHeaderRaw = fmt.Sprintf("   %-3s %-*s %-*s %-*s %6s",
			"#", titleW, "Title", artistW, "Artist", albumW, "Album", "Dur")
	} else {
		colHeaderRaw = fmt.Sprintf("   %-3s %-*s %-*s %6s",
			"#", titleW, "Title", artistW, "Artist", "Dur")
	}
	colHeaderStyled := dimStyle.Render(colHeaderRaw)

	// ── Track list ────────────────────────────────────────────────────────
	vl := components.NewVirtualizedList(
		len(s.tracks), s.cursor, TH,
		components.WithScrollMode(components.ScrollModeCenter),
	)
	start, end := vl.VisibleRange()
	scrollbar := vl.VerticalScrollbar(1, dimStyle)

	var trackLines []string
	for i := start; i < end; i++ {
		tr := s.tracks[i]
		isCurrent := s.isCurrentTrack(tr)
		isCursor  := i == s.cursor

		prefix := "   "
		if isCurrent {
			prefix = "▶  "
		}
		posStr   := fmt.Sprintf("%3d", tr.Pos+1)
		durStr   := fmt.Sprintf("%6s", fmtMusicDuration(tr.Duration))
		titleStr  := truncate(tr.Title,  titleW)
		artistStr := truncate(tr.Artist, artistW)

		var line string
		if albumW > 0 {
			albumStr := truncate(tr.Album, albumW)
			line = fmt.Sprintf("%s%s %-*s %-*s %-*s %s",
				prefix, posStr, titleW, titleStr, artistW, artistStr, albumW, albumStr, durStr)
		} else {
			line = fmt.Sprintf("%s%s %-*s %-*s %s",
				prefix, posStr, titleW, titleStr, artistW, artistStr, durStr)
		}

		var st lipgloss.Style
		switch {
		case isCurrent: st = accentStyle
		case isCursor:  st = cursorStyle
		default:        st = textStyle
		}
		trackLines = append(trackLines, st.Render(line))
	}
	for len(trackLines) < TH {
		trackLines = append(trackLines, "")
	}

	if scrollbar != "" && len(trackLines) > 0 {
		trackLines[0] = trackLines[0] + " " + scrollbar
	}

	// ── Build left bordered box ───────────────────────────────────────────
	queueTitle := fmt.Sprintf("Queue (%d tracks · %s)", len(s.tracks), fmtMusicDuration(s.totalDuration()))
	titleRunes := len([]rune(queueTitle))
	// Top border: ╭─ {title} {dashes}╮  total visible = leftBoxW
	// ╭(1) + ─(1) + space(1) + title + space(1) + dashes + ╮(1) = leftBoxW
	dashCt := innerL - 3 - titleRunes
	if dashCt < 0 {
		dashCt = 0
	}
	topLeft := dimStyle.Render("╭─ ") + accentStyle.Render(queueTitle) +
		dimStyle.Render(" "+strings.Repeat("─", dashCt)+"╮")
	botLeft    := dimStyle.Render("╰" + strings.Repeat("─", innerL) + "╯")
	borderVert := dimStyle.Render("│")

	var leftLines []string
	leftLines = append(leftLines, topLeft)
	leftLines = append(leftLines, borderVert+padRightANSI(colHeaderStyled, innerL)+borderVert)
	for _, tl := range trackLines {
		leftLines = append(leftLines, borderVert+padRightANSI(tl, innerL)+borderVert)
	}
	leftLines = append(leftLines, botLeft)

	// ── Build right bordered box ──────────────────────────────────────────
	rightContent := s.buildRightPanel(innerBoxH, albumW > 0)
	for len(rightContent) < innerBoxH {
		rightContent = append(rightContent, "")
	}
	rightContent = rightContent[:innerBoxH]

	topRight := dimStyle.Render("╭" + strings.Repeat("─", innerR) + "╮")
	botRight  := dimStyle.Render("╰" + strings.Repeat("─", innerR) + "╯")

	var rightLines []string
	rightLines = append(rightLines, topRight)
	for _, rl := range rightContent {
		rightLines = append(rightLines, dimStyle.Render("│")+padRightANSI(rl, innerR)+dimStyle.Render("│"))
	}
	rightLines = append(rightLines, botRight)

	// ── Combine ───────────────────────────────────────────────────────────
	var sb strings.Builder
	for i, ll := range leftLines {
		rl := ""
		if i < len(rightLines) {
			rl = rightLines[i]
		}
		sb.WriteString(ll + rl + "\n")
	}

	if s.visualizer != nil && s.visualizer.IsRunning() {
		sb.WriteString(s.visualizer.RenderBars(w))
	}

	return sb.String()
}

// buildRightPanel builds the right panel lines, truncating from the bottom
// if availH is less than the full 21 rows. showAlbum controls whether the
// album value row is rendered (mirrors whether the album column is visible).
func (s MusicQueueScreen) buildRightPanel(availH int, showAlbum bool) []string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle    := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle   := lipgloss.NewStyle().Foreground(theme.T.Text())

	// Find current track
	var curTrack *ipc.MpdTrack
	for i := range s.tracks {
		if s.isCurrentTrack(s.tracks[i]) {
			curTrack = &s.tracks[i]
			break
		}
	}

	valStr := func(v string) string {
		if v == "" {
			return dimStyle.Render("—")
		}
		return textStyle.Render(truncate(v, 20))
	}

	var lines []string

	// 1. Art placeholder (9 rows)
	artLines := strings.Split(strings.TrimRight(queueArtPlaceholder(), "\n"), "\n")
	lines = append(lines, artLines...)

	// 2. Metadata (label+value rows)
	type metaField struct{ label, value string }
	var fields []metaField
	if curTrack != nil {
		fields = []metaField{
			{"TITLE",    curTrack.Title},
			{"ARTIST",   curTrack.Artist},
			{"DURATION", fmtMusicDuration(curTrack.Duration)},
		}
		if showAlbum {
			fields = append(fields, metaField{"ALBUM", curTrack.Album})
		}
	} else {
		fields = []metaField{{"TITLE", ""}, {"ARTIST", ""}, {"DURATION", ""}}
		if showAlbum {
			fields = append(fields, metaField{"ALBUM", ""})
		}
	}
	for _, f := range fields {
		lines = append(lines, dimStyle.Render(f.label))
		lines = append(lines, valStr(f.value))
	}

	// 3. Seek bar (2 rows)
	barRow, timeRow := queueSeekBar(s.nowElapsed, s.nowDuration)
	lines = append(lines, accentStyle.Render(barRow))
	lines = append(lines, dimStyle.Render(timeRow))

	// 4. Volume bar (2 rows)
	volBar, volHint := queueVolumeBar(s.nowVolume, s.nowMuted)
	lines = append(lines, accentStyle.Render(volBar))
	lines = append(lines, dimStyle.Render(volHint))

	// Truncate to availH from the bottom
	if availH < 0 {
		availH = 0
	}
	if len(lines) > availH {
		lines = lines[:availH]
	}
	return lines
}

// viewNarrow renders the original single-column queue layout for width ≤ 80.
func (s MusicQueueScreen) viewNarrow(w, h int,
	accentStyle, dimStyle, textStyle, cursorStyle lipgloss.Style,
) string {
	// Reserve 1 row: 1 header
	listHeight := h - 1
	if listHeight < 1 {
		listHeight = 1
	}

	listW := w

	vl := components.NewVirtualizedList(
		len(s.tracks),
		s.cursor,
		listHeight,
		components.WithScrollMode(components.ScrollModeCenter),
	)

	headerText := fmt.Sprintf("Queue (%d tracks · %s)", len(s.tracks), fmtMusicDuration(s.totalDuration()))
	header := accentStyle.Render(headerText)

	var sb strings.Builder
	sb.WriteString(header + "\n")

	start, _ := vl.VisibleRange()
	scrollbar := vl.VerticalScrollbar(1, dimStyle)
	if start > 0 {
		sb.WriteString(dimStyle.Render("↑ more\n"))
	}

	if s.loading && len(s.tracks) == 0 {
		sb.WriteString("  " + s.spinner.View() + "\n")
		return sb.String()
	}

	if !s.loading && len(s.tracks) == 0 {
		msg := "Queue is empty"
		pad := (listW - len(msg)) / 2
		if pad < 0 {
			pad = 0
		}
		emptyLine := strings.Repeat(" ", pad) + dimStyle.Render(msg)
		for i := 0; i < listHeight; i++ {
			if i == listHeight/2 {
				sb.WriteString(emptyLine + "\n")
			} else {
				sb.WriteString("\n")
			}
		}
		return sb.String()
	}

	available := listW - 13
	if available < 10 {
		available = 10
	}
	titleW := available * 40 / 100
	if titleW < 8 {
		titleW = 8
	}
	artistW := available - titleW - 2
	if artistW < 4 {
		artistW = 4
	}

	_, end := vl.VisibleRange()
	var listLines []string
	for i := start; i < end; i++ {
		tr := s.tracks[i]
		isCurrent := s.isCurrentTrack(tr)
		isCursor  := i == s.cursor

		prefix := "   "
		if isCurrent {
			prefix = "▶  "
		}

		posStr   := fmt.Sprintf("%3d", tr.Pos+1)
		titleStr  := fmt.Sprintf("%-*s", titleW, truncate(tr.Title, titleW))
		durStr    := fmt.Sprintf("%5s", fmtMusicDuration(tr.Duration))
		artistStr := truncate(tr.Artist, artistW)
		line := prefix + posStr + " " + titleStr + " " + durStr + "  " + artistStr

		var style lipgloss.Style
		switch {
		case isCurrent:
			style = accentStyle
		case isCursor:
			style = cursorStyle
		default:
			style = textStyle
		}
		listLines = append(listLines, style.Render(line))
	}

	for len(listLines) < listHeight {
		listLines = append(listLines, "")
	}

	if scrollbar != "" {
		for i := range listLines {
			listLines[i] = listLines[i] + " " + scrollbar
		}
	}

	sb.WriteString(strings.Join(listLines, "\n"))
	sb.WriteString("\n")
	return sb.String()
}

// HandleMouse handles a left-click within the queue's own coordinate space.
// localY 0 is the header row; localY 1..listHeight are track rows.
func (s MusicQueueScreen) HandleMouse(x, localY int) MusicQueueScreen {
	if localY < 1 {
		return s
	}
	// Queue listHeight = View's h - 2, where h = subH = terminal_height - 2
	// → listHeight = s.height - 4  (s.height == terminal height from WindowSizeMsg)
	listHeight := s.height - 4
	if listHeight < 1 {
		listHeight = 1
	}
	trackRow := localY - 1 // 0-based within the visible list
	if trackRow >= listHeight {
		return s
	}
	// Recompute scroll the same way View does.
	scroll := 0
	if len(s.tracks) > listHeight {
		scroll = s.cursor - listHeight/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(s.tracks)-listHeight {
			scroll = len(s.tracks) - listHeight
		}
	}
	idx := scroll + trackRow
	if idx >= 0 && idx < len(s.tracks) {
		s.cursor = idx
	}
	return s
}

// queueColWidths returns (titleW, artistW, albumW) for the track list columns
// given left-panel width L. albumW == 0 means the Album column is hidden.
// Fixed overhead: 15ch (no album) or 16ch (with album).
func queueColWidths(L int) (titleW, artistW, albumW int) {
	if L >= 120 {
		R := L - 16
		if R < 1 {
			R = 1
		}
		titleW  = R * 40 / 100
		artistW = R * 35 / 100
		albumW  = R * 25 / 100
		titleW += R - titleW - artistW - albumW
	} else {
		R := L - 15
		if R < 1 {
			R = 1
		}
		titleW  = R * 55 / 100
		artistW = R * 45 / 100
		albumW  = 0
		titleW += R - titleW - artistW
	}
	return
}

// wrapText wraps a string to the given width, returning lines.
func wrapText(s string, width int) []string {
	if width <= 0 {
		return []string{s}
	}
	var lines []string
	for len(s) > width {
		lines = append(lines, s[:width])
		s = s[width:]
	}
	if s != "" {
		lines = append(lines, s)
	}
	return lines
}
