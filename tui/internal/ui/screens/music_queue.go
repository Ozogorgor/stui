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
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

// seekTickMsg fires every second to update the elapsed time display.
type seekTickMsg struct{}

// VizCycleBackendMsg is emitted when the user presses V in the queue to
// cycle through visualizer backends (off → cliamp → cava → chroma → off).
type VizCycleBackendMsg struct{}

// VizCycleModeMsg is emitted when the user presses v in the queue to
// cycle through styles/modes for the current visualizer backend.
type VizCycleModeMsg struct{}

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
	nowState    string  // "play" | "pause" | "stop"
	nowElapsed  float64
	nowDuration float64
	nowVolume   uint32
	prevVolume  uint32 // saved before local mute toggle
	nowMuted    bool

	// Visualizer reference — set by MusicScreen.SetVisualizer
	visualizer *components.Visualizer

	// IDs we've already asked MPD to delete via auto-dedup but which
	// still appear in the latest queue refresh (MPD hasn't applied yet).
	// Prevents a fast-refresh loop from re-firing deleteid for the same
	// ID, which produces "No such song" warnings in runtime.log.
	removalsInFlight map[uint32]struct{}
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
	// Try song ID first (most reliable).
	if s.nowSongID > 0 {
		return int32(t.ID) == s.nowSongID
	}
	// Fall back to queue position.
	if s.nowSongPos >= 0 && int(s.nowSongPos) < len(s.tracks) {
		return t.Pos == uint32(s.nowSongPos)
	}
	// Last resort: title + artist match.
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
			// Auto-dedup: drop duplicate tracks (same file path), keeping
			// the first ID. Track which IDs we've recently asked MPD to
			// remove so successive refreshes don't re-fire deleteid for
			// the same ID — that race produced "No such song" spam in
			// runtime.log.
			if s.client != nil && len(s.tracks) > 0 {
				if s.removalsInFlight == nil {
					s.removalsInFlight = make(map[uint32]struct{})
				}
				// Drop entries from the in-flight set whose IDs are no
				// longer in the queue (MPD finished removing them).
				stillThere := make(map[uint32]struct{}, len(s.tracks))
				for _, t := range s.tracks {
					stillThere[t.ID] = struct{}{}
				}
				for id := range s.removalsInFlight {
					if _, ok := stillThere[id]; !ok {
						delete(s.removalsInFlight, id)
					}
				}
				seen := make(map[string]struct{}, len(s.tracks))
				for _, t := range s.tracks {
					if _, dup := seen[t.File]; dup {
						if _, pending := s.removalsInFlight[t.ID]; !pending {
							s.client.MpdCmd("mpd_remove", map[string]any{"id": t.ID})
							s.removalsInFlight[t.ID] = struct{}{}
						}
					} else {
						seen[t.File] = struct{}{}
					}
				}
			}
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

	case seekTickMsg:
		if s.nowState == "play" {
			s.nowElapsed += 1.0
			if s.nowElapsed > s.nowDuration && s.nowDuration > 0 {
				s.nowElapsed = s.nowDuration
			}
			return s, tea.Tick(time.Second, func(time.Time) tea.Msg { return seekTickMsg{} })
		}

	case ipc.MpdStatusMsg:
		s.nowTitle = m.SongTitle
		s.nowArtist = m.SongArtist
		s.nowSongID = m.SongID
		s.nowSongPos = m.SongPos
		s.nowElapsed = m.Elapsed
		s.nowDuration = m.Duration
		s.nowVolume = m.Volume
		wasPlaying := s.nowState == "play"
		s.nowState = m.State
		// External volume change clears local mute state
		if s.nowMuted && m.Volume > 0 {
			s.nowMuted = false
		}
		// Start the seek tick when playback begins.
		if m.State == "play" && !wasPlaying {
			return s, tea.Tick(time.Second, func(time.Time) tea.Msg { return seekTickMsg{} })
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
		case "D":
			// Remove duplicate tracks (same file path) from the queue,
			// keeping the first occurrence of each.
			if s.client != nil && len(s.tracks) > 0 {
				seen := make(map[string]struct{}, len(s.tracks))
				for _, t := range s.tracks {
					if _, dup := seen[t.File]; dup {
						s.client.MpdCmd("mpd_remove", map[string]any{"id": t.ID})
					} else {
						seen[t.File] = struct{}{}
					}
				}
			}
		case "V":
			return s, func() tea.Msg { return VizCycleBackendMsg{} }
		case "v":
			return s, func() tea.Msg { return VizCycleModeMsg{} }
		case "enter":
			if len(s.tracks) > 0 && s.cursor < len(s.tracks) {
				trackID := s.tracks[s.cursor].ID
				s.client.MpdCmd("mpd_play_id", map[string]any{"id": trackID})
			}
		case " ", "space":
			s.client.MpdCmd("mpd_toggle_pause", nil)
		}
	}

	return s, nil
}

// queueArtPlaceholder returns an art placeholder box that fills innerW columns.
func queueArtPlaceholder(innerW int) string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	boxStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.TextDim()).
		Width(innerW).
		Height(innerW / 2).
		Align(lipgloss.Center, lipgloss.Center)
	return boxStyle.Render(dim.Render("♪")) + "\n"
}

// Package-level album art cache (persists across value-receiver calls).
var (
	cachedArtFile     string
	cachedArtRendered string
	cachedArtWidth    int
)

// queueAlbumArt returns rendered album art for the track's directory,
// or falls back to the placeholder. Caches the result keyed by file path.
func queueAlbumArt(innerW int, trackFile string) string {
	if trackFile == "" {
		return queueArtPlaceholder(innerW)
	}

	// Check cache
	if cachedArtFile == trackFile && cachedArtWidth == innerW && cachedArtRendered != "" {
		return cachedArtRendered
	}

	// Find cover art in the track's directory
	musicDir := findMusicDir()
	if musicDir == "" {
		return queueArtPlaceholder(innerW)
	}
	dir := filepath.Dir(filepath.Join(musicDir, trackFile))
	coverPath := findCoverArt(dir)
	if coverPath == "" {
		return queueArtPlaceholder(innerW)
	}

	// Render via chafa — use Kitty protocol for supported terminals,
	// fall back to Unicode symbols for everything else.
	h := innerW / 2
	if h < 3 {
		h = 3
	}
	format := chafaFormat()
	out, err := exec.Command("chafa",
		"--format", format,
		"--size", fmt.Sprintf("%dx%d", innerW, h),
		"--animate", "off",
		coverPath,
	).Output()
	if err != nil || len(out) == 0 {
		return queueArtPlaceholder(innerW)
	}

	cachedArtFile = trackFile
	cachedArtWidth = innerW
	cachedArtRendered = strings.TrimRight(string(out), "\n") + "\n"
	return cachedArtRendered
}

// chafaFormat returns the chafa output format to use.
// Kitty/sixel protocols produce single escape sequences that can't be
// split into lines for the bordered right panel, so we always use
// symbols (Unicode half-blocks) which produce regular text rows.
func chafaFormat() string {
	return "symbols"
}

// findMusicDir reads mpd.conf to get the music directory.
func findMusicDir() string {
	home, _ := os.UserHomeDir()
	candidates := []string{
		filepath.Join(home, ".config", "mpd", "mpd.conf"),
		filepath.Join(home, ".mpd", "mpd.conf"),
		"/etc/mpd.conf",
	}
	for _, path := range candidates {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		for _, line := range strings.Split(string(data), "\n") {
			line = strings.TrimSpace(line)
			if strings.HasPrefix(line, "#") || !strings.HasPrefix(line, "music_directory") {
				continue
			}
			rest := strings.TrimSpace(line[len("music_directory"):])
			rest = strings.Trim(rest, "\"")
			if strings.HasPrefix(rest, "~") {
				rest = home + rest[1:]
			}
			return rest
		}
	}
	return ""
}

// findCoverArt looks for common cover art filenames in a directory.
func findCoverArt(dir string) string {
	names := []string{
		"cover.jpg", "cover.png", "Cover.jpg", "Cover.png",
		"folder.jpg", "folder.png", "Folder.jpg", "Folder.png",
		"front.jpg", "front.png", "Front.jpg", "Front.png",
		"album.jpg", "album.png", "Album.jpg", "Album.png",
	}
	for _, name := range names {
		p := filepath.Join(dir, name)
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	return ""
}

// queueSeekBar returns (barRow, timeRow) for the progress display.
// barRow is w chars of ━/╸/─. timeRow shows elapsed and total, padded to w chars.
func queueSeekBar(elapsed, duration float64, w int) (barRow, timeRow string) {
	// When duration == 0, return all dashes (no cursor tip)
	if duration <= 0 {
		barRow  = strings.Repeat("─", w)
		timeRow = "0:00" + strings.Repeat(" ", w-8) + "0:00"
		return
	}
	filled := int(elapsed / duration * float64(w))
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

// numVolBlocks is the number of ▮/▯ blocks in the volume bar.
// With innerR=22, 16 blocks + "  100%" = 22 chars, filling the panel exactly.
const numVolBlocks = 16

// queueVolumeBar returns (barRow, hintRow) for the volume display.
// Clicking the bar sets volume proportional to the block clicked.
func queueVolumeBar(volume uint32, muted bool) (barRow, hintRow string) {
	filled := int(volume) * numVolBlocks / 100
	empty  := numVolBlocks - filled
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

	// Wide layout: two bordered boxes side by side.
	// rightBoxW=24 gives innerR=22, which fits the widest hint "0 unmute" (22ch).
	const rightBoxW = 24  // outer width of right box (border + 22 inner + border)
	leftBoxW := w - rightBoxW  // outer width of left box
	innerL   := leftBoxW - 2   // inner content width of left box
	const innerR = 22           // inner content width of right box

	// Visualizer panel is reserved whenever the backend is not "off", even if
	// it's not currently running — the container stays visible (idle) so the
	// layout doesn't jump when playback starts/stops.
	vizEnabled := s.visualizer != nil &&
		s.visualizer.Config().Backend != components.VisualizerOff
	vizContentH := 0
	vizPanelH := 0
	if vizEnabled {
		vizContentH = s.visualizer.Config().Height
		if vizContentH < 1 {
			vizContentH = 8
		}
		vizPanelH = vizContentH + 2 // +2 for top/bottom border
	}

	// ── Responsive box height — fill all available space ──────────────────
	// Both panels stretch to use the full height above the visualizer.
	// Left (track list) shows more tracks; right panel pads below the
	// seekbar/volume when there's room, and truncates art when tight.
	innerLForCols := innerL - 2
	if innerLForCols < 10 {
		innerLForCols = 10
	}
	titleW, artistW, albumW := queueColWidths(innerLForCols)
	boxH := h - vizPanelH
	if boxH < 3 {
		boxH = 3
	}
	innerBoxH := boxH - 2

	// Track rows = innerBoxH - 1 (first inner row is column header).
	TH := innerBoxH - 1
	if TH < 1 {
		TH = 1
	}

	// ── Column headers row ────────────────────────────────────────────────
	var colHeaderRaw string
	if albumW > 0 {
		colHeaderRaw = fmt.Sprintf("   %-3s %-*s %-*s %-*s %7s",
			"#", titleW, "Title", artistW, "Artist", albumW, "Album", "Dur")
	} else {
		colHeaderRaw = fmt.Sprintf("   %-3s %-*s %-*s %7s",
			"#", titleW, "Title", artistW, "Artist", "Dur")
	}
	colHeaderStyled := dimStyle.Render(colHeaderRaw)

	// ── Track list ────────────────────────────────────────────────────────
	vl := components.NewVirtualizedList(
		len(s.tracks), s.cursor, TH,
		components.WithScrollMode(components.ScrollModeCenter),
	)
	start, end := vl.VisibleRange()
	// Always-visible per-row scrollbar (shows track even when all fit).
	barChars := components.ScrollbarChars(start, TH, len(s.tracks), dimStyle)

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
		durStr   := fmt.Sprintf("%7s", fmtMusicDuration(tr.Duration))
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

	// Append per-row scrollbar char (always reserved, even if no scrolling).
	for i := range trackLines {
		if i < len(barChars) {
			trackLines[i] = padRightANSI(trackLines[i], innerLForCols) + " " + barChars[i]
		}
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
		// trackLines are already padded to innerLForCols + space + scrollbar,
		// which should equal innerL.
		leftLines = append(leftLines, borderVert+padRightANSI(tl, innerL)+borderVert)
	}
	leftLines = append(leftLines, botLeft)

	// ── Build right bordered box ──────────────────────────────────────────
	rightContent := s.buildRightPanel(innerBoxH, albumW > 0, innerR)
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

	if vizEnabled {
		sb.WriteString(s.renderVizPanel(w, vizContentH, dimStyle, accentStyle))
	}

	// Force output to exactly h lines so the queue fills its allocation
	// with no gap and no overflow.
	out := strings.TrimRight(sb.String(), "\n")
	lines := strings.Split(out, "\n")
	for len(lines) < h {
		lines = append(lines, "")
	}
	if len(lines) > h {
		lines = lines[:h]
	}
	return strings.Join(lines, "\n")
}

// renderVizPanel returns a bordered container of width `w` and total height
// `contentH + 2`. When the visualizer is running, its output is rendered
// inside the container; otherwise the inner rows are blank (idle state) so
// the layout stays stable.
func (s MusicQueueScreen) renderVizPanel(w, contentH int, dimStyle, accentStyle lipgloss.Style) string {
	if contentH < 1 {
		contentH = 1
	}
	innerW := w - 2
	if innerW < 1 {
		innerW = 1
	}

	// Top border with a short label on the left.
	label := "Visualizer"
	if s.visualizer != nil {
		cfg := s.visualizer.Config()
		if name := cfg.Mode.String(); name != "" {
			label = "Visualizer · " + name
		}
	}
	labelRunes := len([]rune(label))
	dashCt := innerW - 3 - labelRunes
	if dashCt < 0 {
		dashCt = 0
	}
	topViz := dimStyle.Render("╭─ ") + accentStyle.Render(label) +
		dimStyle.Render(" "+strings.Repeat("─", dashCt)+"╮")
	botViz := dimStyle.Render("╰" + strings.Repeat("─", innerW) + "╯")

	var inner []string
	if s.visualizer != nil && s.visualizer.IsRunning() {
		raw := s.visualizer.Render(innerW)
		inner = strings.Split(strings.TrimRight(raw, "\n"), "\n")
	}

	// Compute a single horizontal offset that centers the visualizer inside
	// the panel. Cliamp modes have a hard-coded internal width and ignore
	// the requested width, so they'd otherwise hug the left edge. Use the
	// widest rendered line as the reference width; bars-mode (which honours
	// width) will already match innerW so its offset becomes zero.
	leftPad := 0
	if len(inner) > 0 {
		maxW := 0
		for _, ln := range inner {
			if w := lipgloss.Width(ln); w > maxW {
				maxW = w
			}
		}
		if maxW < innerW {
			leftPad = (innerW - maxW) / 2
		}
	}

	var sb strings.Builder
	sb.WriteString(topViz + "\n")
	for i := 0; i < contentH; i++ {
		var line string
		if i < len(inner) {
			if leftPad > 0 {
				line = strings.Repeat(" ", leftPad) + inner[i]
			} else {
				line = inner[i]
			}
		}
		sb.WriteString(dimStyle.Render("│") + padRightANSI(line, innerW) + dimStyle.Render("│") + "\n")
	}
	sb.WriteString(botViz + "\n")
	return sb.String()
}

// rightPanelContentHeight returns the exact number of inner rows the right
// column needs to render its members (art + metadata + seek bar + vol bar),
// mirroring buildRightPanel's own layout. showAlbum adds one extra label/value
// pair (2 rows). innerW is the right column's inner width.
func rightPanelContentHeight(innerW int, showAlbum bool) int {
	if innerW < 1 {
		innerW = 1
	}
	// queueArtPlaceholder uses Height(innerW/2) which lipgloss treats as
	// OUTER height (border-inclusive), so the rendered art is innerW/2 rows
	// total — not innerW/2 + 2. See queueArtPlaceholder's doc comment.
	artRows := innerW / 2
	metaRows := 6           // TITLE + ARTIST + DURATION, each label + value
	if showAlbum {
		metaRows = 8
	}
	seekRows := 2
	volRows := 2
	return artRows + metaRows + seekRows + volRows
}

// buildRightPanel builds the right panel lines, truncating from the bottom
// if availH is less than the full rows. showAlbum controls whether the
// album value row is rendered (mirrors whether the album column is visible).
// innerW is the inner width of the right panel box.
func (s MusicQueueScreen) buildRightPanel(availH int, showAlbum bool, innerW int) []string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle    := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle   := lipgloss.NewStyle().Foreground(theme.T.Text())

	// The right panel follows the CURSOR track so users can preview metadata
	// and duration while browsing. If the cursor is on the currently playing
	// track, the seek bar shows real elapsed/duration; otherwise it shows
	// 0:00 / track-duration with an empty progress channel.
	var selTrack *ipc.MpdTrack
	if s.cursor >= 0 && s.cursor < len(s.tracks) {
		selTrack = &s.tracks[s.cursor]
	}


	valStr := func(v string) string {
		if v == "" {
			return dimStyle.Render("—")
		}
		return textStyle.Render(truncate(v, innerW))
	}

	var lines []string

	// 1. Album art (or placeholder if no cover found)
	var trackFile string
	if selTrack != nil {
		trackFile = selTrack.File
	}
	artStr := queueAlbumArt(innerW, trackFile)
	artLines := strings.Split(strings.TrimRight(artStr, "\n"), "\n")
	lines = append(lines, artLines...)

	// 2. Metadata (label+value rows), pulled from the selected track.
	type metaField struct{ label, value string }
	var fields []metaField
	if selTrack != nil {
		fields = []metaField{
			{"TITLE", selTrack.Title},
			{"ARTIST", selTrack.Artist},
			{"DURATION", fmtMusicDuration(selTrack.Duration)},
		}
		if showAlbum {
			fields = append(fields, metaField{"ALBUM", selTrack.Album})
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

	// 3. Seek bar (2 rows). Always shows the playing track's progress,
	// regardless of which track the cursor is on.
	elapsed := s.nowElapsed
	duration := s.nowDuration
	barRow, timeRow := queueSeekBar(elapsed, duration, innerW)
	lines = append(lines, accentStyle.Render(barRow))
	lines = append(lines, dimStyle.Render(timeRow))

	// 4. Volume bar (2 rows)
	volBar, volHint := queueVolumeBar(s.nowVolume, s.nowMuted)
	lines = append(lines, accentStyle.Render(volBar))
	lines = append(lines, dimStyle.Render(volHint))

	// Truncate to availH. Seekbar and volume are essential — trim the art
	// placeholder from the top instead of cutting essential controls from
	// the bottom.
	if availH < 0 {
		availH = 0
	}
	if len(lines) > availH {
		excess := len(lines) - availH
		// artLines occupy the first len(artLines) rows; trim those first.
		trimArt := excess
		if trimArt > len(artLines) {
			trimArt = len(artLines)
		}
		lines = lines[trimArt:]
		// If still too tall after removing all art, trim from the top
		// (metadata labels) rather than losing seekbar/volume.
		if len(lines) > availH {
			lines = lines[len(lines)-availH:]
		}
	}
	return lines
}

// viewNarrow renders the original single-column queue layout for width ≤ 80.
func (s MusicQueueScreen) viewNarrow(w, h int,
	accentStyle, dimStyle, textStyle, cursorStyle lipgloss.Style,
) string {
	// Reserve viz panel if enabled so the layout doesn't jump on play/stop.
	vizEnabled := s.visualizer != nil &&
		s.visualizer.Config().Backend != components.VisualizerOff
	vizContentH := 0
	vizPanelH := 0
	if vizEnabled {
		vizContentH = s.visualizer.Config().Height
		if vizContentH < 1 {
			vizContentH = 8
		}
		vizPanelH = vizContentH + 2
	}

	// Reserve 1 row: 1 header
	listHeight := h - 1 - vizPanelH
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

	if vizEnabled {
		sb.WriteString(s.renderVizPanel(w, vizContentH, dimStyle, accentStyle))
	}
	return sb.String()
}

// HandleMouse handles a left-click within the queue's own coordinate space.
// localY 0 is the top border row of the boxes; localY 1 is the column header.
// Clicks in the right panel volume bar adjust volume.
// HandleMouse handles a left-click within the queue's own coordinate space.
//
// Wide layout (>80 cols) — the View produces:
//
//	localY 0 = top border of left/right box
//	localY 1 = column header row
//	localY 2..2+TH-1 = track rows
//	localY 2+TH = bottom border
//
// Narrow layout (≤80 cols):
//
//	localY 0 = queue header row
//	localY 1..listH = track rows
func (s MusicQueueScreen) HandleMouse(x, localY int) MusicQueueScreen {
	const rightBoxW = 24
	const innerR = 22
	isWide := s.width > 80
	leftBoxW := s.width - rightBoxW

	// Match View's viz layout exactly. The panel is reserved whenever the
	// backend is not Off (idle state renders an empty bordered container).
	vizEnabled := s.visualizer != nil &&
		s.visualizer.Config().Backend != components.VisualizerOff
	vizPanelH := 0
	if vizEnabled {
		vizContentH := s.visualizer.Config().Height
		if vizContentH < 1 {
			vizContentH = 8
		}
		vizPanelH = vizContentH + 2 // +2 for top/bottom border
	}

	// Track list height — mirror View's TH/listHeight calculation, using
	// the fixed-height-based-on-right-panel logic.
	var trackListH int
	if isWide {
		innerLForCols := leftBoxW - 2 - 2 // borders + scrollbar col
		if innerLForCols < 10 {
			innerLForCols = 10
		}
		_, _, albumW := queueColWidths(innerLForCols)
		rightNeeded := rightPanelContentHeight(innerR, albumW > 0)
		boxH := rightNeeded + 2
		if boxH > s.height-vizPanelH {
			boxH = s.height - vizPanelH
		}
		if boxH < 3 {
			boxH = 3
		}
		innerBoxH := boxH - 2
		trackListH = innerBoxH - 1
		if trackListH < 1 {
			trackListH = 1
		}
	} else {
		trackListH = s.height - 1 - vizPanelH
		if trackListH < 1 {
			trackListH = 1
		}
	}

	// ── Right-panel volume bar click (wide only) ──────────────────────────
	// buildRightPanel(innerBoxH, ..., innerR) renders:
	//   art placeholder : innerR/2 lines (Height is outer/border-inclusive)
	//   metadata        : 6 or 8 lines (3 or 4 label+value pairs)
	//   seek bar        : 2 lines
	//   volume bar      : 2 lines
	if isWide {
		innerLForCols := leftBoxW - 2 - 2 // borders + scrollbar col
		if innerLForCols < 10 {
			innerLForCols = 10
		}
		_, _, albumW := queueColWidths(innerLForCols)
		artRows := innerR / 2
		metaRows := 6
		if albumW > 0 {
			metaRows = 8
		}
		volBarInnerRow := artRows + metaRows + 2 // after art+meta+seek
		volBarLocalY := volBarInnerRow + 1        // +1 for right-box top border

		if x > leftBoxW && x <= leftBoxW+innerR+1 && localY == volBarLocalY {
			blockX := x - leftBoxW - 1
			if blockX >= 0 && blockX < numVolBlocks {
				newVol := (blockX + 1) * 100 / numVolBlocks
				if newVol > 100 {
					newVol = 100
				}
				if s.client != nil {
					s.client.MpdCmd("mpd_set_volume", map[string]any{"volume": newVol})
				}
				s.nowVolume = uint32(newVol)
				s.nowMuted = false
			}
			return s
		}
	}

	// ── Track list click ──────────────────────────────────────────────────
	var trackRow int
	if isWide {
		// localY 0 = top border, localY 1 = column header, localY 2+ = tracks.
		if localY < 2 {
			return s
		}
		trackRow = localY - 2
	} else {
		// localY 0 = queue header, localY 1+ = tracks.
		if localY < 1 {
			return s
		}
		trackRow = localY - 1
	}
	if trackRow >= trackListH {
		return s
	}

	// Recompute scroll the same way VirtualizedList (ScrollModeCenter) does.
	scroll := 0
	if len(s.tracks) > trackListH {
		scroll = s.cursor - trackListH/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(s.tracks)-trackListH {
			scroll = len(s.tracks) - trackListH
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
// Fixed overhead: 17ch (no album) or 18ch (with album). Dur column is %7s,
// and 1 extra ch reserves the gap between Dur and the right border.
func queueColWidths(L int) (titleW, artistW, albumW int) {
	if L >= 120 {
		R := L - 18
		if R < 1 {
			R = 1
		}
		titleW  = R * 40 / 100
		artistW = R * 35 / 100
		albumW  = R * 25 / 100
		titleW += R - titleW - artistW - albumW
	} else {
		R := L - 17
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
