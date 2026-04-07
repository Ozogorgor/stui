package screens

// music_screen.go — Top-level Music section container with 4 sub-tabs:
// Browse, Queue, Library, Playlists.
//
// Sub-tab bar (2 lines):
//   [ Browse ]  [ Queue (N) ]  [ Library ]  [ Playlists ]
//   ────────────────────────────────────────────────────
//
// Keys:
//   [  — previous sub-tab
//   ]  — next sub-tab
//   all other keys → delegated to the active sub-screen

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

// MusicSubTab identifies which Music sub-tab is active.
type MusicSubTab int

const (
	MusicBrowse    MusicSubTab = iota // 0
	MusicQueue                        // 1
	MusicLibrary                      // 2
	MusicPlaylists                    // 3
)

// String returns a human-readable label for the sub-tab.
func (t MusicSubTab) String() string {
	switch t {
	case MusicBrowse:
		return "Browse"
	case MusicQueue:
		return "Queue"
	case MusicLibrary:
		return "Library"
	case MusicPlaylists:
		return "Playlists"
	default:
		return "Unknown"
	}
}

// MusicScreen is the top-level container for all Music sub-tabs.
type MusicScreen struct {
	Dims
	client *ipc.Client
	active MusicSubTab
	vizRef *components.Visualizer // retained so SetClient can re-apply it

	browse    MusicBrowseScreen
	queue     MusicQueueScreen
	library   MusicLibraryScreen
	playlists MusicPlaylistsScreen
}

// NewMusicScreen creates a MusicScreen with all sub-screens initialised.
// The active sub-tab defaults to MusicQueue.
func NewMusicScreen(client *ipc.Client) MusicScreen {
	return MusicScreen{
		client:    client,
		active:    MusicQueue,
		browse:    NewMusicBrowseScreen(client),
		queue:     NewMusicQueueScreen(client),
		library:   NewMusicLibraryScreen(client),
		playlists: NewMusicPlaylistsScreen(client),
	}
}

// ActiveSubTab returns the currently visible sub-tab.
func (s MusicScreen) ActiveSubTab() MusicSubTab { return s.active }

// SetVisualizer passes the visualizer reference to the queue sub-tab so it
// can render the visualizer strip inline.
func (s *MusicScreen) SetVisualizer(v *components.Visualizer) {
	s.vizRef = v
	s.queue.visualizer = v
}

// WithActiveSubTab returns a copy of s with the active sub-tab overridden.
// Used to restore the saved sub-tab preference on startup.
func (s MusicScreen) WithActiveSubTab(t MusicSubTab) MusicScreen {
	s.active = t
	return s
}

// Update handles messages and key events, delegating to the active sub-screen.
func (s MusicScreen) Update(msg tea.Msg) (MusicScreen, tea.Cmd) {
	switch m := msg.(type) {

	case tea.WindowSizeMsg:
		s.setWindowSize(m)
		// Fan out to all sub-screens.
		var b1, b2, b3, b4 tea.Cmd
		s.browse, b1 = s.browse.Update(m)
		s.queue, b2 = s.queue.Update(m)
		s.library, b3 = s.library.Update(m)
		s.playlists, b4 = s.playlists.Update(m)
		return s, tea.Batch(b1, b2, b3, b4)

	case tea.KeyPressMsg:
		switch m.String() {
		case "[":
			s.active = (s.active + 3) % 4 // go left (wrap)
			return s, nil
		case "]":
			s.active = (s.active + 1) % 4 // go right (wrap)
			return s, nil
		}
		// Delegate to active sub-screen only.
		var cmd tea.Cmd
		switch s.active {
		case MusicBrowse:
			s.browse, cmd = s.browse.Update(m)
		case MusicQueue:
			s.queue, cmd = s.queue.Update(m)
		case MusicLibrary:
			s.library, cmd = s.library.Update(m)
		case MusicPlaylists:
			s.playlists, cmd = s.playlists.Update(m)
		}
		return s, cmd

	case tea.MouseMsg:
		// Wheel events: delegate to active sub-screen as synthetic j/k keypresses.
		mouse := m.Mouse()
		if mouse.Button == tea.MouseWheelUp || mouse.Button == tea.MouseWheelDown {
			// Wheel events are handled - navigation happens through parent model
			return s, nil
		}
		return s, nil

	default:
		// Fan out all other messages to ALL sub-screens so they maintain state.
		var cmds []tea.Cmd
		var c tea.Cmd
		s.browse, c = s.browse.Update(msg)
		if c != nil {
			cmds = append(cmds, c)
		}
		s.queue, c = s.queue.Update(msg)
		if c != nil {
			cmds = append(cmds, c)
		}
		s.library, c = s.library.Update(msg)
		if c != nil {
			cmds = append(cmds, c)
		}
		s.playlists, c = s.playlists.Update(msg)
		if c != nil {
			cmds = append(cmds, c)
		}
		return s, tea.Batch(cmds...)
	}
}

// View renders the sub-tab bar followed by the active sub-screen.
func (s MusicScreen) View() tea.View {
	tabBar := s.renderSubTabBar()
	// s.height is set by computeMusicHeight() in ui.go, which already accounts
	// for whether the footer is shown. Simply subtract the 2-row subtab bar.
	subH := s.height - 2
	if subH < 0 {
		subH = 0
	}
	var body string
	switch s.active {
	case MusicBrowse:
		body = s.browse.View(s.width, subH)
	case MusicQueue:
		body = s.queue.View(s.width, subH)
	case MusicLibrary:
		body = s.library.View(s.width, subH)
	case MusicPlaylists:
		body = s.playlists.View(s.width, subH)
	}
	return tea.NewView(lipgloss.JoinVertical(lipgloss.Left, tabBar, body))
}

// renderSubTabBar builds the two-line sub-tab header.
func (s MusicScreen) renderSubTabBar() string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	tabs := []MusicSubTab{MusicBrowse, MusicQueue, MusicLibrary, MusicPlaylists}
	var parts []string
	for _, t := range tabs {
		var label string
		if t == MusicQueue && len(s.queue.tracks) > 0 {
			label = fmt.Sprintf("Queue (%d)", len(s.queue.tracks))
		} else {
			label = t.String()
		}

		if t == s.active {
			parts = append(parts, accentStyle.Render("[ "+label+" ]"))
		} else {
			parts = append(parts, dimStyle.Render(label))
		}
	}

	tabLine := "  " + strings.Join(parts, "  ")
	sepW := s.width
	if sepW < 1 {
		sepW = 1
	}
	sep := dimStyle.Render(strings.Repeat("─", sepW))
	return tabLine + "\n" + sep
}

// HandleMouse routes a left-click to the correct sub-screen given a Y
// coordinate relative to the music section's own top row (i.e. after the
// app-level top bar and any overlay rows have been subtracted).
//
//	relY == 0      → sub-tab bar row (hit-test by X)
//	relY == 1      → separator line (ignored)
//	relY >= 2      → body; passed as localY = relY-2 to the active sub-screen
func (s MusicScreen) HandleMouse(x, relY int) (MusicScreen, tea.Cmd) {
	if relY == 0 {
		if tab, ok := s.hitTestSubTabBar(x); ok {
			s.active = tab
		}
		return s, nil
	}
	if relY == 1 {
		return s, nil // separator
	}
	bodyY := relY - 2
	var cmd tea.Cmd
	switch s.active {
	case MusicBrowse:
		s.browse = s.browse.HandleMouse(x, bodyY)
	case MusicQueue:
		s.queue = s.queue.HandleMouse(x, bodyY)
	case MusicLibrary:
		s.library = s.library.HandleMouse(x, bodyY)
	case MusicPlaylists:
		s.playlists, cmd = s.playlists.HandleMouse(x, bodyY)
	}
	return s, cmd
}

// hitTestSubTabBar returns the sub-tab at horizontal position x, or false if
// x falls outside all tab labels.
func (s MusicScreen) hitTestSubTabBar(x int) (MusicSubTab, bool) {
	tabs := []MusicSubTab{MusicBrowse, MusicQueue, MusicLibrary, MusicPlaylists}
	pos := 2 // "  " prefix (MainCardStyle left offset already subtracted by caller)
	for _, t := range tabs {
		var label string
		if t == MusicQueue && len(s.queue.tracks) > 0 {
			label = fmt.Sprintf("Queue (%d)", len(s.queue.tracks))
		} else {
			label = t.String()
		}
		var width int
		if t == s.active {
			width = len("[ " + label + " ]")
		} else {
			width = len(label)
		}
		if x >= pos && x < pos+width {
			return t, true
		}
		pos += width + 2 // two-space separator
	}
	return 0, false
}

// SetClient replaces the IPC client in all sub-screens, triggers initial data
// loads, and returns the updated MusicScreen along with any init cmds.
func (s MusicScreen) SetClient(client *ipc.Client) (MusicScreen, tea.Cmd) {
	s.client = client
	// Recreate sub-screens with the real client so constructors fire their
	// initial fetches (queue, artist list, playlist list).
	s.browse = NewMusicBrowseScreen(client)
	s.queue = NewMusicQueueScreen(client)
	s.queue.visualizer = s.vizRef // re-apply stored reference
	s.library = NewMusicLibraryScreen(client)
	s.playlists = NewMusicPlaylistsScreen(client)
	return s, tea.Batch(s.playlists.Init(), s.library.Init())
}
