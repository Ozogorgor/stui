package screens

// music_browse.go — Browse sub-tab: catalog search for music entries via
// plugin-backed streaming search (PluginDataSource + CatalogBrowser).
//
// Displays albums in a poster grid (like movies/series). Click on an
// album to view its tracks.

import (
	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screens/catalogbrowser"
)

// MusicBrowseScreen shows the music catalog with plugin-backed search.
//
// Loading state, runtime status, and plugin list are NOT stored on the
// screen — they live at the Model level (`m.state.IsLoading`, etc.) and
// are threaded through `View()` so the global flags drive the UI in the
// same way Movies/Series do.
type MusicBrowseScreen struct {
	Dims
	client    *ipc.Client
	catalog  []ipc.CatalogEntry
	cursor   GridCursor
	source   *catalogbrowser.PluginDataSource
	selected ipc.CatalogEntry
}

// NewMusicBrowseScreen creates a new browse screen. When client is non-nil a
// PluginDataSource is wired in so that StartSearch can dispatch streaming
// queries immediately without a separate init step.
func NewMusicBrowseScreen(client *ipc.Client) MusicBrowseScreen {
	s := MusicBrowseScreen{client: client}
	if client != nil {
		s.source = catalogbrowser.NewPluginDataSource(client)
	}
	return s
}

// SetClient updates the client reference (used after initialization).
func (s *MusicBrowseScreen) SetClient(client *ipc.Client) {
	s.client = client
}

// Update handles incoming messages and key events.
func (s MusicBrowseScreen) Update(msg tea.Msg) (MusicBrowseScreen, tea.Cmd) {
	switch m := msg.(type) {

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case ipc.GridUpdateMsg:
		if m.Tab == "music" {
			s.catalog = m.Entries
			s.cursor = GridCursor{}
		}

	// ── Streaming search messages from PluginDataSource ───────────────────

	case catalogbrowser.ScopeResultsAppliedMsg:
		return s, m.Followup

	case catalogbrowser.SearchChannelClosedMsg:
		return s, nil

	case catalogbrowser.SearchDispatchFailedMsg:
		return s, nil

	case catalogbrowser.StaleScopeDroppedMsg:
		return s, m.Followup

	case tea.KeyPressMsg:
		results := s.catalog
		cols := 5 // matches CardColumns in components
		switch m.String() {
		case "j", "down":
			if s.cursor.row < (len(results)+cols-1)/cols-1 {
				s.cursor.row++
			}
		case "k", "up":
			if s.cursor.row > 0 {
				s.cursor.row--
			}
		case "h", "left":
			if s.cursor.col > 0 {
				s.cursor.col--
			}
		case "l", "right":
			maxCol := cols - 1
			idx := s.cursor.Index(cols)
			if idx < len(results)-1 {
				if s.cursor.col < maxCol {
					s.cursor.col++
				} else {
					s.cursor.col = 0
					s.cursor.row++
				}
			}
		case "enter":
			// Open the selected album's detail to show tracks
			idx := s.cursor.Index(cols)
			if idx >= 0 && idx < len(results) && s.client != nil {
				s.selected = results[idx]
			}
		case "/":
			s.cursor = GridCursor{}
		}
	}

	return s, nil
}

// HandleMouse handles a left-click within the browse screen.
func (s MusicBrowseScreen) HandleMouse(x, localY int) MusicBrowseScreen {
	results := s.catalog
	if len(results) == 0 {
		return s
	}

	cols := 5
	rowH := 11 // matches CardTotalRows in components
	row := localY / rowH
	col := x / 14 // approx card width

	idx := row*cols + col
	if idx >= 0 && idx < len(results) {
		s.cursor = GridCursor{row: row, col: col}
	}

	return s
}

// View renders the browse screen using the poster grid.
// Loading state, runtime status, plugins, and the loading-spinner
// pointer all come from the model level — same source movies/series
// use — so the loading UX (animated spinner + "Loading…" text) is
// identical across tabs.
func (s MusicBrowseScreen) View(w, h int, isLoading bool, loadingStart int64, runtimeStatus string, plugins []string, sp *spinner.Model) string {
	results := s.catalog

	availH := h
	if availH < 1 {
		availH = 1
	}

	return RenderGrid(
		results,
		s.cursor,
		w,
		availH,
		isLoading,
		loadingStart,
		runtimeStatus,
		plugins,
		sp,
	)
}

// FooterText returns the hint text.
func (s MusicBrowseScreen) FooterText() string {
	return "enter view tracks · / search · hjkls move"
}