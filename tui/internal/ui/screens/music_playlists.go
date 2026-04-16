package screens

// music_playlists.go — Playlists sub-tab: view and manage MPD saved playlists.

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

// MusicPlaylistsScreen displays saved MPD playlists with a track preview pane.
type MusicPlaylistsScreen struct {
	Dims
	client         *ipc.Client
	playlists      []ipc.MpdSavedPlaylist
	cursor         int
	scroll         int
	preview        []ipc.MpdSong // tracks for hovered playlist
	previewFor     string        // which playlist name the preview is for
	loadingList    bool
	loadingPreview bool
	// Save-mode: prompt user for new playlist name
	saving   bool
	saveName string
	spinner  components.Spinner
}

// NewMusicPlaylistsScreen creates a new playlists screen. Loading starts immediately.
func NewMusicPlaylistsScreen(client *ipc.Client) MusicPlaylistsScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return MusicPlaylistsScreen{
		client:      client,
		loadingList: true,
		spinner:     *components.NewSpinner("loading playlists…", dimStyle),
	}
}

// fetchPreviewCmd sets previewFor/loadingPreview and returns a Cmd that fetches
// tracks for the named playlist. Call only when s is addressable (i.e. after
// mutation via value copy pattern).
func fetchPreviewCmd(client *ipc.Client, name string) tea.Cmd {
	return func() tea.Msg {
		if client != nil {
			client.MpdGetPlaylistTracks(name)
		}
		return nil
	}
}

// hoveredPlaylistName returns the name of the playlist under the cursor, or "".
func (s MusicPlaylistsScreen) hoveredPlaylistName() string {
	if len(s.playlists) == 0 || s.cursor >= len(s.playlists) {
		return ""
	}
	return s.playlists[s.cursor].Name
}

// Init triggers the initial playlist fetch.
func (s *MusicPlaylistsScreen) Init() tea.Cmd {
	s.spinner.Start()
	return tea.Batch(
		s.spinner.Init(),
		func() tea.Msg {
			if s.client != nil {
				s.client.MpdGetPlaylists()
			}
			return nil
		},
	)
}

// Update handles incoming messages and key events.
func (s MusicPlaylistsScreen) Update(msg tea.Msg) (MusicPlaylistsScreen, tea.Cmd) {
	switch m := msg.(type) {

	case spinner.TickMsg:
		_, cmd := s.spinner.Update(m)
		return s, cmd

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case ipc.MpdPlaylistsResultMsg:
		s.loadingList = false
		s.spinner.Stop()
		if m.Err == nil {
			s.playlists = m.Playlists
		}
		if len(s.playlists) > 0 {
			name := s.playlists[0].Name
			s.previewFor = name
			s.loadingPreview = true
			return s, fetchPreviewCmd(s.client, name)
		}

	case ipc.MpdPlaylistTracksResultMsg:
		if m.Name == s.previewFor {
			s.loadingPreview = false
			if m.Err == nil {
				s.preview = m.Tracks
			}
		}

	case tea.KeyPressMsg:
		if s.saving {
			return s.updateSaveMode(m)
		}
		return s.updateNormalMode(m)
	}

	return s, nil
}

func (s MusicPlaylistsScreen) updateNormalMode(m tea.KeyPressMsg) (MusicPlaylistsScreen, tea.Cmd) {
	switch m.String() {
	case "j", "down":
		if s.cursor < len(s.playlists)-1 {
			s.cursor++
			name := s.hoveredPlaylistName()
			if name != s.previewFor {
				s.previewFor = name
				s.loadingPreview = true
				return s, fetchPreviewCmd(s.client, name)
			}
		}
	case "k", "up":
		if s.cursor > 0 {
			s.cursor--
			name := s.hoveredPlaylistName()
			if name != s.previewFor {
				s.previewFor = name
				s.loadingPreview = true
				return s, fetchPreviewCmd(s.client, name)
			}
		}
	case "enter":
		if name := s.hoveredPlaylistName(); name != "" && s.client != nil {
			s.client.MpdCmd("mpd_playlist_load", map[string]any{"name": name})
			return s, func() tea.Msg {
				s.client.MpdGetQueue()
				return nil
			}
		}
	case "a":
		if name := s.hoveredPlaylistName(); name != "" && s.client != nil {
			s.client.MpdCmd("mpd_playlist_append", map[string]any{"name": name})
		}
	case "d":
		if name := s.hoveredPlaylistName(); name != "" && s.client != nil {
			s.client.MpdCmd("mpd_playlist_delete", map[string]any{"name": name})
			return s, func() tea.Msg {
				s.client.MpdGetPlaylists()
				return nil
			}
		}
	case "s":
		s.saving = true
		s.saveName = ""
	}
	return s, nil
}

func (s MusicPlaylistsScreen) updateSaveMode(m tea.KeyPressMsg) (MusicPlaylistsScreen, tea.Cmd) {
	switch m.String() {
	case "esc":
		s.saving = false
		s.saveName = ""
	case "backspace":
		if len(s.saveName) > 0 {
			runes := []rune(s.saveName)
			s.saveName = string(runes[:len(runes)-1])
		}
	case "enter":
		if s.saveName != "" && s.client != nil {
			name := s.saveName
			s.client.MpdCmd("mpd_playlist_save", map[string]any{"name": name})
			s.saving = false
			s.saveName = ""
			return s, func() tea.Msg {
				s.client.MpdGetPlaylists()
				return nil
			}
		}
	default:
		// Append printable characters.
		if len(m.Text) > 0 {
			s.saveName += m.Text
		}
	}
	return s, nil
}

// HandleMouse handles a left-click within the playlists' own coordinate space.
// localY maps directly to body row (no header row in this view).
func (s MusicPlaylistsScreen) HandleMouse(x, localY int) (MusicPlaylistsScreen, tea.Cmd) {
	if s.saving {
		return s, nil
	}
	// bodyH = View's h - 1, where h = terminal_height - 2 → bodyH = s.height - 3
	bodyH := s.height - 3
	if bodyH < 1 {
		bodyH = 1
	}
	leftW := s.width * 30 / 100
	if leftW < 20 {
		leftW = 20
	}
	if localY < 0 || localY >= bodyH {
		return s, nil
	}
	if x < leftW {
		// Left pane: playlist list.
		scroll := s.scroll
		if s.cursor < scroll {
			scroll = s.cursor
		}
		if s.cursor >= scroll+bodyH {
			scroll = s.cursor - bodyH + 1
		}
		if scroll < 0 {
			scroll = 0
		}
		idx := scroll + localY
		if idx >= 0 && idx < len(s.playlists) && idx != s.cursor {
			s.cursor = idx
			name := s.hoveredPlaylistName()
			if name != s.previewFor {
				s.previewFor = name
				s.loadingPreview = true
				return s, fetchPreviewCmd(s.client, name)
			}
		}
	}
	return s, nil
}

// View renders the playlists screen within the given width/height constraints.
func (s MusicPlaylistsScreen) View(w, h int) string {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	altStyle := lipgloss.NewStyle().Foreground(theme.T.AccentAlt())

	bodyH := h
	if bodyH < 1 {
		bodyH = 1
	}

	if s.loadingList {
		var sb strings.Builder
		sb.WriteString("  " + s.spinner.View() + "\n")
		for i := 1; i < bodyH; i++ {
			sb.WriteString("\n")
		}
		return sb.String()
	}

	// Two-pane layout: left ~30%, right ~70%.
	leftW := w * 30 / 100
	if leftW < 20 {
		leftW = 20
	}
	if leftW > w-20 {
		leftW = w - 20
	}
	rightW := w - leftW - 1 // 1 for separator

	// Build left pane lines (playlist list).
	var leftLines []string

	if len(s.playlists) == 0 {
		leftLines = append(leftLines, dimStyle.Render("  No saved playlists"))
	} else {
		// Scrolling: keep cursor visible.
		scroll := s.scroll
		if s.cursor < scroll {
			scroll = s.cursor
		}
		if s.cursor >= scroll+bodyH {
			scroll = s.cursor - bodyH + 1
		}
		if scroll < 0 {
			scroll = 0
		}

		end := scroll + bodyH
		if end > len(s.playlists) {
			end = len(s.playlists)
		}
		for i := scroll; i < end; i++ {
			pl := s.playlists[i]
			name := truncate(pl.Name, leftW-2)
			line := "  " + fmt.Sprintf("%-*s", leftW-2, name)
			if i == s.cursor {
				leftLines = append(leftLines, accentStyle.Render(line))
			} else {
				leftLines = append(leftLines, textStyle.Render(line))
			}
		}
	}

	// Pad left pane to bodyH with fixed-width empty lines so the separator
	// column stays aligned all the way down.
	for len(leftLines) < bodyH {
		leftLines = append(leftLines, strings.Repeat(" ", leftW))
	}

	// Build right pane lines (track preview).
	var rightLines []string

	hovName := s.hoveredPlaylistName()
	if hovName != "" {
		header := altStyle.Render(fmt.Sprintf("  Tracks in %s", truncate(hovName, rightW-14)))
		rightLines = append(rightLines, header)
	} else {
		rightLines = append(rightLines, "")
	}

	if s.loadingPreview {
		rightLines = append(rightLines, dimStyle.Render("  Loading…"))
	} else {
		for _, song := range s.preview {
			if len(rightLines) >= bodyH {
				break
			}
			dur := ""
			if song.Duration > 0 {
				dur = fmtMusicDuration(song.Duration)
			}
			titleStr := truncate(song.Title, rightW-30)
			artistStr := truncate(song.Artist, 16)
			line := fmt.Sprintf("  %-*s  %-16s  %s", rightW-34, titleStr, artistStr, dur)
			rightLines = append(rightLines, textStyle.Render(line))
		}
	}

	// Pad right pane to bodyH.
	for len(rightLines) < bodyH {
		rightLines = append(rightLines, "")
	}

	borderStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 1)

	// Combine panes side by side.
	sep := dimStyle.Render("│")
	var sb strings.Builder
	var paneContent strings.Builder
	for i := 0; i < bodyH; i++ {
		ll := ""
		if i < len(leftLines) {
			ll = leftLines[i]
		}
		rr := ""
		if i < len(rightLines) {
			rr = rightLines[i]
		}
		paneContent.WriteString(ll + sep + rr + "\n")
	}

	// Wrap in border container
	borderedContent := borderStyle.Width(w - 2).Render(paneContent.String())
	sb.WriteString(borderedContent + "\n")

	// Save-mode overlay sits at the bottom while the prompt is active so
	// the user's input has somewhere prominent to land.
	if s.saving {
		prompt := fmt.Sprintf("  Save current queue as: %s_", s.saveName)
		sb.WriteString(accentStyle.Render(prompt) + "\n")
	}

	return sb.String()
}

// FooterText is what the global status bar shows while this screen is
// active. Mirrors MusicLibraryScreen.FooterText: hint by default, the
// save prompt while editing one.
func (s MusicPlaylistsScreen) FooterText() string {
	if s.saving {
		return "type name · enter save · esc cancel"
	}
	return "enter view · s save queue as · d delete · ↑↓ navigate"
}
