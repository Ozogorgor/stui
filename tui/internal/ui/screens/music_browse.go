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
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
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
	client   *ipc.Client
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
			if s.cursor.Row < (len(results)+cols-1)/cols-1 {
				s.cursor.Row++
			}
		case "k", "up":
			if s.cursor.Row > 0 {
				s.cursor.Row--
			}
		case "h", "left":
			if s.cursor.Col > 0 {
				s.cursor.Col--
			}
		case "l", "right":
			maxCol := cols - 1
			idx := s.cursor.Index(cols)
			if idx < len(results)-1 {
				if s.cursor.Col < maxCol {
					s.cursor.Col++
				} else {
					s.cursor.Col = 0
					s.cursor.Row++
				}
			}
		case "enter":
			// Open the selected album's detail to show tracks. Pull
			// every nil-able pointer field through a guard — the
			// artist in particular MUST get propagated, because
			// AlbumDetailScreen.Init() needs (artist, album) to
			// fire the lastfm album.getInfo lookup. Passing "" for
			// artist made the screen sit at "Loading tracks…"
			// indefinitely (Init early-returns, no IPC request, no
			// LastFMAlbumTracksMsg ever arrives to flip loading=false).
			idx := s.cursor.Index(cols)
			if idx >= 0 && idx < len(results) && s.client != nil {
				s.selected = results[idx]
				album := s.selected
				var artist, year, genre, rating string
				if album.Artist != nil {
					artist = *album.Artist
				}
				if album.Year != nil {
					year = *album.Year
				}
				if album.Genre != nil {
					genre = *album.Genre
				}
				if album.Rating != nil {
					rating = *album.Rating
				}
				var coverURL string
				if album.PosterURL != nil {
					coverURL = *album.PosterURL
				}
				return s, screen.TransitionCmd(
					NewAlbumDetailScreen(
						s.client,
						album.Title,
						artist,
						year,
						genre,
						rating,
						coverURL,
					),
					true,
				)
			}
		case "/":
			s.cursor = GridCursor{}
		}
	}

	return s, nil
}

// HandleMouse handles a left-click within the browse screen.
// The (x, localY) we receive is the raw screen coordinate offset by
// the parent dispatcher down to "inside MusicScreen, below the
// sub-tab bar". It still includes MainCardStyle's left chrome —
// `Margin(1) + Border(1)` = 2 columns — which the grid render skips
// over, so we subtract those 2 columns before col-math. Without
// this correction, `col := x / cardW` reads x=0 as col 0 even
// though the actual card 0 starts at x=2, and the click areas
// drift right by 2 columns (col 0's hit area ends mid-card-0,
// col 1's hit area straddles cards 0 and 1, etc).
//
// Movies/Series do the equivalent subtraction at the dispatcher
// level (see ui/mouse.go: `cardX := x - 2`); we keep it local here
// because the other Music sub-tabs (queue, library, playlists)
// have list-shaped layouts and don't need the same offset.
const mainCardLeftChrome = 2

func (s MusicBrowseScreen) HandleMouse(x, localY int) MusicBrowseScreen {
	results := s.catalog
	if len(results) == 0 {
		return s
	}
	cardX := x - mainCardLeftChrome
	if cardX < 0 {
		// Click landed in MainCardStyle's left margin/border —
		// no card under the cursor.
		return s
	}

	rowH := components.CardTotalRows
	cols := components.CardColumns
	// Use inner width (accounting for MainCardStyle's margin+border+padding = 6 chars)
	termW := s.width - 6
	if termW < 10 {
		termW = 10
	}

	totalRows := (len(results) + cols - 1) / cols
	visibleRows := s.height / rowH
	if totalRows > visibleRows {
		termW -= 1 // Reserve space for scrollbar like RenderGrid does
	}
	// CardWidth returns content width. Each column includes 2-char padding on
	// left and right (the gap between cards), so cell width is cardW + 4.
	cardW := components.CardWidth(termW)
	cellW := cardW + 4
	row := localY / rowH
	col := cardX / cellW
	if col >= cols {
		// Click was past the rightmost rendered card column.
		return s
	}

	idx := row*cols + col
	if idx >= 0 && idx < len(results) {
		s.cursor = GridCursor{Row: row, Col: col}
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
