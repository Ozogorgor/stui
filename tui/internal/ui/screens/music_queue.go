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

// View renders the queue screen within the given width/height constraints.
func (s MusicQueueScreen) View(w, h int) string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	cursorStyle := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Bold(true)

	footerLine := hintBar("enter play", "d remove", "c clear", "g top", "G bottom")

	// Reserve 2 rows: 1 header + 1 footer
	listHeight := h - 2
	if listHeight < 1 {
		listHeight = 1
	}

	wide := w > 100
	rightPanelW := 0
	listW := w
	if wide {
		rightPanelW = 20
		listW = w - rightPanelW - 1 // 1 for separator
	}

	// Virtualized list rendering (for scroll calculation)
	vl := components.NewVirtualizedList(
		len(s.tracks),
		s.cursor,
		listHeight,
		components.WithScrollMode(components.ScrollModeCenter),
	)

	// Header
	headerText := fmt.Sprintf("Queue (%d tracks · %s)", len(s.tracks), fmtMusicDuration(s.totalDuration()))
	header := accentStyle.Render(headerText)

	var sb strings.Builder
	sb.WriteString(header + "\n")

	// Add scroll indicator if there are more items above
	start, _ := vl.VisibleRange()
	scrollbar := vl.VerticalScrollbar(1, dimStyle)
	if start > 0 {
		sb.WriteString(dimStyle.Render("↑ more\n"))
	}

	// Loading / empty states
	if s.loading && len(s.tracks) == 0 {
		sb.WriteString("  " + s.spinner.View() + "\n")
		sb.WriteString(footerLine + "\n")
		return sb.String()
	}

	if !s.loading && len(s.tracks) == 0 {
		// Center the "Queue is empty" message
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
		sb.WriteString(footerLine + "\n")
		return sb.String()
	}

	// Column widths for the list pane
	// Prefix: 3, Pos: 3, space: 1, Duration: 5, space: 1 → fixed = 13
	// Remaining split: ~40% title, rest artist
	available := listW - 13
	if available < 10 {
		available = 10
	}
	titleW := available * 40 / 100
	if titleW < 8 {
		titleW = 8
	}
	artistW := available - titleW - 2 // 2 for spacing
	if artistW < 4 {
		artistW = 4
	}

	// Build list lines
	_, end := vl.VisibleRange()
	var listLines []string
	for i := start; i < end; i++ {
		t := s.tracks[i]
		isCurrent := s.isCurrentTrack(t)
		isCursor := i == s.cursor

		prefix := "   "
		if isCurrent {
			prefix = "▶  "
		}

		posStr := fmt.Sprintf("%3d", t.Pos+1)
		titleStr := truncate(t.Title, titleW)
		titleStr = fmt.Sprintf("%-*s", titleW, titleStr)
		durStr := fmt.Sprintf("%5s", fmtMusicDuration(t.Duration))
		artistStr := truncate(t.Artist, artistW)

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

	// Pad list to listHeight
	for len(listLines) < listHeight {
		listLines = append(listLines, "")
	}

	// Add scrollbar to right side
	if scrollbar != "" {
		for i := range listLines {
			if i < len(listLines) {
				listLines[i] = listLines[i] + " " + scrollbar
			}
		}
	}

	if !wide {
		sb.WriteString(strings.Join(listLines, "\n"))
		sb.WriteString("\n")
	} else {
		// Build right panel (album/artist info for current track)
		var rightLines []string
		var currentTitle, currentArtist, currentAlbum string
		for _, t := range s.tracks {
			if s.isCurrentTrack(t) {
				currentTitle = t.Title
				currentArtist = t.Artist
				currentAlbum = t.Album
				break
			}
		}

		rpW := rightPanelW - 1
		if currentTitle != "" {
			titleWrapped := wrapText(currentTitle, rpW)
			rightLines = append(rightLines, "")
			rightLines = append(rightLines, titleWrapped...)
			rightLines = append(rightLines, "")
			artistWrapped := wrapText(currentArtist, rpW)
			rightLines = append(rightLines, artistWrapped...)
			rightLines = append(rightLines, "")
			albumWrapped := wrapText(currentAlbum, rpW)
			rightLines = append(rightLines, albumWrapped...)
		}

		// Pad right panel to listHeight
		for len(rightLines) < listHeight {
			rightLines = append(rightLines, "")
		}
		if len(rightLines) > listHeight {
			rightLines = rightLines[:listHeight]
		}

		sep := dimStyle.Render("│")
		for i, ll := range listLines {
			rr := ""
			if i < len(rightLines) {
				rr = accentStyle.Render(rightLines[i])
			}
			sb.WriteString(ll + sep + rr + "\n")
		}
	}

	sb.WriteString(footerLine + "\n")
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
// Fixed overhead = 13ch (prefix 3 + # 3 + space 1 + duration 6).
func queueColWidths(L int) (titleW, artistW, albumW int) {
	R := L - 13
	if R < 1 {
		R = 1
	}
	if L >= 120 {
		titleW  = R * 40 / 100
		artistW = R * 35 / 100
		albumW  = R * 25 / 100
		// remainder goes to title
		titleW += R - titleW - artistW - albumW
	} else {
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
