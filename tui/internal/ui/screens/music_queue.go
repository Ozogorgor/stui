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

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/pkg/theme"
)

// MusicQueueScreen displays and controls the live MPD playback queue.
type MusicQueueScreen struct {
	client     *ipc.Client
	tracks     []ipc.MpdTrack
	cursor     int
	width      int
	height     int
	loading    bool
	nowTitle   string // from MpdStatusMsg — used to highlight current track
	nowArtist  string
	nowSongID  int32 // from MpdStatusMsg.SongID; 0 if unknown
	nowSongPos int32 // from MpdStatusMsg.SongPos; -1 if unknown
}

// NewMusicQueueScreen creates a new queue screen and triggers the initial fetch.
func NewMusicQueueScreen(client *ipc.Client) MusicQueueScreen {
	s := MusicQueueScreen{
		client:     client,
		loading:    true,
		nowSongPos: -1,
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

	case tea.WindowSizeMsg:
		s.width = m.Width
		s.height = m.Height

	case ipc.MpdQueueResultMsg:
		if m.Err == nil {
			s.tracks = m.Tracks
		}
		s.loading = false

	case ipc.MpdQueueChangedMsg:
		s.loading = true
		return s, func() tea.Msg {
			s.client.MpdGetQueue()
			return nil
		}

	case ipc.MpdStatusMsg:
		s.nowTitle = m.SongTitle
		s.nowArtist = m.SongArtist
		s.nowSongID = m.SongID
		s.nowSongPos = m.SongPos

	case tea.KeyMsg:
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

// View renders the queue screen within the given width/height constraints.
func (s MusicQueueScreen) View(w, h int) string {
	accentStyle  := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle     := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle    := lipgloss.NewStyle().Foreground(theme.T.Text())
	cursorStyle  := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Bold(true)

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

	// Header
	headerText := fmt.Sprintf("Queue (%d tracks · %s)", len(s.tracks), fmtMusicDuration(s.totalDuration()))
	header := accentStyle.Render(headerText)

	var sb strings.Builder
	sb.WriteString(header + "\n")

	// Loading / empty states
	if s.loading && len(s.tracks) == 0 {
		sb.WriteString(dimStyle.Render("  Loading queue…") + "\n")
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

	// Scrolling: keep cursor visible
	scroll := 0
	if len(s.tracks) > listHeight {
		// Center cursor when possible
		scroll = s.cursor - listHeight/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(s.tracks)-listHeight {
			scroll = len(s.tracks) - listHeight
		}
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
	var listLines []string
	end := scroll + listHeight
	if end > len(s.tracks) {
		end = len(s.tracks)
	}
	for i := scroll; i < end; i++ {
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
