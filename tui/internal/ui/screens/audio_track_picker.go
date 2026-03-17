package screens

// audio_track_picker.go — AudioTrackPickerScreen: pick an audio dub/track from mpv.
//
// Layout:
//
//   🔊  Audio Tracks
//
//   ▶  1  Japanese    Original              ← active (✓)
//      2  English     Dubbed
//      3  French      Dubbed
//
//   ↑↓ navigate   enter select   esc back

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// AudioTrackPickerScreen shows all audio tracks for the current mpv file and
// lets the user select one by pressing Enter.
type AudioTrackPickerScreen struct {
	client *ipc.Client
	tracks []ipc.TrackInfo // only audio tracks
	cursor int
	width  int
	height int
}

// NewAudioTrackPickerScreen creates the screen from the full track list returned
// by PlayerTracksUpdatedMsg. Only tracks with TrackType == "audio" are shown.
func NewAudioTrackPickerScreen(client *ipc.Client, allTracks []ipc.TrackInfo) *AudioTrackPickerScreen {
	var audio []ipc.TrackInfo
	activeCursor := 0
	for _, t := range allTracks {
		if t.TrackType == "audio" {
			if t.Selected {
				activeCursor = len(audio)
			}
			audio = append(audio, t)
		}
	}
	return &AudioTrackPickerScreen{
		client: client,
		tracks: audio,
		cursor: activeCursor,
	}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (s *AudioTrackPickerScreen) Init() tea.Cmd { return nil }

func (s *AudioTrackPickerScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case tea.WindowSizeMsg:
		s.width = msg.Width
		s.height = msg.Height

	// Live track-list updates (e.g. mpv adds an external track while open)
	case ipc.PlayerTracksUpdatedMsg:
		s.refreshTracks(msg.Tracks)

	case tea.KeyMsg:
		switch msg.String() {
		case "esc", "q":
			return s, func() tea.Msg { return screen.PopMsg{} }
		case "up", "k":
			if s.cursor > 0 {
				s.cursor--
			}
		case "down", "j":
			if s.cursor < len(s.tracks)-1 {
				s.cursor++
			}
		case "enter":
			if len(s.tracks) > 0 && s.client != nil {
				t := s.tracks[s.cursor]
				s.client.PlayerCommand("set_property", "aid", t.ID)
			}
			return s, func() tea.Msg { return screen.PopMsg{} }
		}
	}
	return s, nil
}

func (s *AudioTrackPickerScreen) refreshTracks(all []ipc.TrackInfo) {
	var audio []ipc.TrackInfo
	for _, t := range all {
		if t.TrackType == "audio" {
			audio = append(audio, t)
		}
	}
	// Preserve cursor position as best we can
	if s.cursor >= len(audio) {
		s.cursor = len(audio) - 1
	}
	if s.cursor < 0 {
		s.cursor = 0
	}
	s.tracks = audio
}

// ── View ──────────────────────────────────────────────────────────────────────

func (s *AudioTrackPickerScreen) View() string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle    := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle   := lipgloss.NewStyle().Foreground(theme.T.Text())
	activeStyle := lipgloss.NewStyle().Foreground(theme.T.Success())

	header := accentStyle.Render("🔊  Audio Tracks")

	if len(s.tracks) == 0 {
		return header + "\n\n" + dimStyle.Render("  No audio tracks found.") + "\n\n" +
			dimStyle.Render("  esc back") + "\n"
	}

	// Column widths
	idW   := 3
	langW := 10
	titleW := 26

	// Header row
	colHdr := dimStyle.Render(
		fmt.Sprintf("  %-*s  %-*s  %-*s",
			idW, "#",
			langW, "Language",
			titleW, "Title",
		),
	)

	var rows []string
	rows = append(rows, colHdr)

	for i, t := range s.tracks {
		prefix := "   "
		var rowStyle lipgloss.Style
		if i == s.cursor {
			prefix = "▶  "
			rowStyle = accentStyle
		} else {
			rowStyle = textStyle
		}

		lang  := audioLang(t)
		title := t.Title
		if title == "" {
			title = "—"
		}

		activeBadge := ""
		if t.Selected {
			activeBadge = "  " + activeStyle.Render("[✓]")
		}

		line := rowStyle.Render(fmt.Sprintf("%s%-*d  %-*s  %-*s",
			prefix,
			idW, t.ID,
			langW, truncate(lang, langW),
			titleW, truncate(title, titleW),
		)) + activeBadge

		rows = append(rows, line)
	}

	body := strings.Join(rows, "\n")
	footer := hintBar("↑↓ navigate", "enter select", "esc back")

	return header + "\n\n" + body + "\n\n" + footer + "\n"
}

// audioLang returns the best language label: title (if short) > lang > "Track N".
func audioLang(t ipc.TrackInfo) string {
	if t.Lang != "" {
		return strings.ToUpper(t.Lang)
	}
	if t.Title != "" {
		return t.Title
	}
	return fmt.Sprintf("Track %d", t.ID)
}
