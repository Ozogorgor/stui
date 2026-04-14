package screens

// music_library.go — Library sub-tab: Artist → Album → Track browser,
// plus a Directory mode toggle.
//
// Tag mode layout:
//
//	┌─ Artists ────────┬─ Albums ──────────────┬─ Tracks ──────────┐
//	│ ▶ Queen          │   A Night at the…     │   Bohemian Rhap… │
//	│   The Beatles    │   Jazz                │   You're My Bes… │
//	│   Led Zeppelin   │   …                   │   …               │
//	└──────────────────┴───────────────────────┴───────────────────┘
//
// Dir mode layout (toggle with D):
//
//	▸ Music / Rock / Queen /                    ← breadcrumb path
//	┌──────────────────────────────────────────┐
//	│ ▶ [dir]  A Night at the Opera            │
//	│   [dir]  Jazz                            │
//	│          Bohemian Rhapsody.flac    5:55  │
//	└──────────────────────────────────────────┘

import (
	"fmt"
	"regexp"
	"strings"
	"time"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/theme"
)

// LibraryPane identifies which column is active in tag-browse mode.
type LibraryPane int

const (
	LibPaneArtists LibraryPane = iota
	LibPaneAlbums
	LibPaneTracks
)

// libDialogCtx tells the dialog dismiss handler which option set the
// user was looking at, so we can interpret the chosen index correctly.
type libDialogCtx int

const (
	libDialogEnter      libDialogCtx = iota // Add / Replace / Cancel
	libDialogRightClick                     // Add / Replace / Add to Playlist / Create Playlist / Cancel
)

// MusicLibraryScreen is the Artist→Album→Track browser with an optional
// directory-tree view toggled with D.
type MusicLibraryScreen struct {
	Dims
	client  *ipc.Client
	dirMode bool // false = tag browser, true = directory browser

	// Tag browser state
	activePane     LibraryPane
	artists        []ipc.MpdArtist
	albums         []ipc.MpdAlbum
	songs          []ipc.MpdSong
	artistCursor   int
	albumCursor    int
	songCursor     int
	artistScroll   int
	albumScroll    int
	songScroll     int
	loadingArtists bool
	loadingAlbums  bool
	loadingSongs   bool
	// Error messages from the runtime (broken pipe, MPD ACK, etc.) so the
	// view can render an actionable hint instead of a stuck spinner.
	artistError string
	albumError  string
	songError   string

	// Dir browser state
	dirPath    []string // breadcrumb stack; empty = root
	dirEntries []ipc.MpdDirEntry
	dirCursor  int
	dirScroll  int
	loadingDir bool

	// Footer status message (e.g. "Added 'Track' to queue").
	statusMsg string

	// queueFiles tracks file paths currently in the MPD queue for dedup checks.
	queueFiles map[string]struct{}

	// Track-action dialog (shown when Enter is pressed on a track or a
	// track row is right-clicked). dialogContext describes which option
	// set is showing — Enter uses the simple Add/Replace/Cancel set, and
	// right-click uses the extended Add/Replace/Add to Playlist/Create
	// Playlist set so the option indices are interpreted correctly.
	dialogOpen    bool
	dialog        components.Dialog
	dialogSong    ipc.MpdSong
	dialogContext libDialogCtx

	spinner      components.Spinner
	loadingStart time.Time
}

// NewMusicLibraryScreen creates a new library screen and starts fetching artists.
func NewMusicLibraryScreen(client *ipc.Client) MusicLibraryScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	s := MusicLibraryScreen{
		client:         client,
		loadingArtists: true,
		spinner:        *components.NewSpinner("loading…", dimStyle),
		loadingStart:   time.Now(),
	}
	s.spinner.Start()
	if client != nil {
		client.MpdListArtists()
	}
	return s
}

// Init returns the command to fetch all artists.
func (s *MusicLibraryScreen) Init() tea.Cmd {
	s.spinner.Start()
	s.loadingStart = time.Now()
	return tea.Batch(
		s.spinner.Init(),
		func() tea.Msg {
			if s.client != nil {
				s.client.MpdListArtists()
			}
			return nil
		},
	)
}

// Update handles incoming messages and key events.
func (s MusicLibraryScreen) Update(msg tea.Msg) (MusicLibraryScreen, tea.Cmd) {
	switch m := msg.(type) {

	case spinner.TickMsg:
		_, cmd := s.spinner.Update(m)
		if s.loadingArtists && !s.loadingStart.IsZero() && time.Since(s.loadingStart) > 8*time.Second {
			s.loadingArtists = false
			s.spinner.Stop()
		}
		return s, cmd

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case ipc.MpdLibraryResultMsg:
		if m.Err != nil {
			// Surface the error in the right scope so the UI can show a
			// useful message instead of just freezing on the spinner.
			if m.ForAlbum != "" {
				s.songError = m.Err.Error()
				s.loadingSongs = false
			} else if m.ForArtist != "" {
				s.albumError = m.Err.Error()
				s.loadingAlbums = false
			} else {
				s.artistError = m.Err.Error()
				s.loadingArtists = false
				s.spinner.Stop()
			}
			break
		}
		// Clear any prior error in this scope on a successful response.
		s.artistError, s.albumError, s.songError = "", "", ""
		if m.ForAlbum != "" {
			// Songs for a specific album arrived
			s.songs = m.Songs
			s.loadingSongs = false
		} else if m.ForArtist != "" {
			// Albums for a specific artist arrived
			s.albums = m.Albums
			s.albumCursor = 0
			s.albumScroll = 0
			s.loadingAlbums = false
			// Pre-fetch songs for the first album
			if len(m.Albums) > 0 && s.client != nil {
				s.loadingSongs = true
				s.client.MpdListSongs(m.ForArtist, m.Albums[0].Title)
			}
		} else {
			// Artist list arrived
			s.artists = m.Artists
			s.loadingArtists = false
			s.spinner.Stop()
			// Pre-fetch albums for the first artist
			if len(m.Artists) > 0 && s.client != nil {
				s.loadingAlbums = true
				s.client.MpdListAlbums(m.Artists[0].Name)
			}
		}

	case ipc.MpdQueueResultMsg:
		if m.Err == nil {
			s.queueFiles = make(map[string]struct{}, len(m.Tracks))
			for _, t := range m.Tracks {
				s.queueFiles[t.File] = struct{}{}
			}
		}

	case ipc.MpdDirResultMsg:
		if m.Err == nil {
			s.dirEntries = m.Entries
		}
		s.loadingDir = false

	case tea.KeyPressMsg:
		if s.dirMode {
			s = s.handleDirKey(m.String())
		} else {
			s = s.handleTagKey(m.String())
		}
	}

	return s, nil
}

// handleTagKey processes key events in tag-browser mode.
func (s MusicLibraryScreen) handleTagKey(key string) MusicLibraryScreen {
	// Dialog intercepts all keys when open.
	if s.dialogOpen {
		var chosen int
		var dismissed bool
		s.dialog, chosen, dismissed = s.dialog.Update(key)
		if dismissed {
			s = s.applyDialogChoice(chosen)
		}
		return s
	}

	switch key {
	case "j", "down":
		switch s.activePane {
		case LibPaneArtists:
			if s.artistCursor < len(s.artists)-1 {
				s.artistCursor++
				s.albumCursor = 0
				s.albumScroll = 0
				s.songCursor = 0
				s.songScroll = 0
				if s.client != nil && s.artistCursor < len(s.artists) {
					s.loadingAlbums = true
					s.albums = nil
					s.songs = nil
					s.client.MpdListAlbums(s.artists[s.artistCursor].Name)
				}
			}
		case LibPaneAlbums:
			if s.albumCursor < len(s.albums)-1 {
				s.albumCursor++
				s.songCursor = 0
				s.songScroll = 0
				if s.client != nil && s.albumCursor < len(s.albums) && s.artistCursor < len(s.artists) {
					s.loadingSongs = true
					s.songs = nil
					s.client.MpdListSongs(s.artists[s.artistCursor].Name, s.albums[s.albumCursor].Title)
				}
			}
		case LibPaneTracks:
			if s.songCursor < len(s.songs)-1 {
				s.songCursor++
			}
		}

	case "k", "up":
		switch s.activePane {
		case LibPaneArtists:
			if s.artistCursor > 0 {
				s.artistCursor--
				s.albumCursor = 0
				s.albumScroll = 0
				s.songCursor = 0
				s.songScroll = 0
				if s.client != nil && s.artistCursor < len(s.artists) {
					s.loadingAlbums = true
					s.albums = nil
					s.songs = nil
					s.client.MpdListAlbums(s.artists[s.artistCursor].Name)
				}
			}
		case LibPaneAlbums:
			if s.albumCursor > 0 {
				s.albumCursor--
				s.songCursor = 0
				s.songScroll = 0
				if s.client != nil && s.albumCursor < len(s.albums) && s.artistCursor < len(s.artists) {
					s.loadingSongs = true
					s.songs = nil
					s.client.MpdListSongs(s.artists[s.artistCursor].Name, s.albums[s.albumCursor].Title)
				}
			}
		case LibPaneTracks:
			if s.songCursor > 0 {
				s.songCursor--
			}
		}

	case "l", "right", "enter":
		switch s.activePane {
		case LibPaneArtists:
			s.activePane = LibPaneAlbums
			if s.client != nil && len(s.artists) > 0 {
				s.loadingAlbums = true
				s.albums = nil
				s.songs = nil
				s.albumCursor = 0
				s.albumScroll = 0
				s.client.MpdListAlbums(s.artists[s.artistCursor].Name)
			}
		case LibPaneAlbums:
			s.activePane = LibPaneTracks
			if s.client != nil && len(s.albums) > 0 && len(s.artists) > 0 {
				s.loadingSongs = true
				s.songs = nil
				s.songCursor = 0
				s.songScroll = 0
				s.client.MpdListSongs(s.artists[s.artistCursor].Name, s.albums[s.albumCursor].Title)
			}
		case LibPaneTracks:
			// Open the track-action dialog instead of adding immediately.
			if len(s.songs) > 0 && s.songCursor < len(s.songs) {
				song := s.songs[s.songCursor]
				s.dialogSong = song
				s.dialogContext = libDialogEnter
				s.dialog = components.NewDialog(
					"What to do with '"+truncate(song.Title, 28)+"'?",
					[]string{"Add to queue", "Replace queue", "Cancel"},
				)
				s.dialogOpen = true
			}
		}

	case "h", "left":
		switch s.activePane {
		case LibPaneAlbums:
			s.activePane = LibPaneArtists
		case LibPaneTracks:
			s.activePane = LibPaneAlbums
		}

	case "a":
		if s.client != nil {
			switch s.activePane {
			case LibPaneAlbums:
				if len(s.albums) > 0 && s.albumCursor < len(s.albums) && len(s.artists) > 0 {
					// Add all songs in the album — use the artist/album URI pattern
					uri := s.artists[s.artistCursor].Name + "/" + s.albums[s.albumCursor].Title
					s.client.MpdCmd("mpd_add", map[string]any{"uri": uri})
				}
			case LibPaneTracks:
				if len(s.songs) > 0 && s.songCursor < len(s.songs) {
					song := s.songs[s.songCursor]
					if _, exists := s.queueFiles[song.File]; exists {
						s.statusMsg = "'" + song.Title + "' is already in the queue"
					} else {
						s.client.MpdCmd("mpd_add", map[string]any{"uri": song.File})
						s.statusMsg = "Added '" + song.Title + "' to queue"
					}
				}
			}
		}

	case "D":
		s.dirMode = true
		s.dirPath = nil
		s.dirEntries = nil
		s.dirCursor = 0
		s.dirScroll = 0
		s.loadingDir = true
		if s.client != nil {
			s.client.MpdBrowseDir("")
		}
	}

	return s
}

// handleDirKey processes key events in directory-browser mode.
func (s MusicLibraryScreen) handleDirKey(key string) MusicLibraryScreen {
	switch key {
	case "j", "down":
		if s.dirCursor < len(s.dirEntries)-1 {
			s.dirCursor++
		}

	case "k", "up":
		if s.dirCursor > 0 {
			s.dirCursor--
		}

	case "enter", "l":
		if len(s.dirEntries) == 0 || s.dirCursor >= len(s.dirEntries) {
			break
		}
		e := s.dirEntries[s.dirCursor]
		if e.IsDir {
			s.dirPath = append(s.dirPath, e.Name)
			s.dirEntries = nil
			s.dirCursor = 0
			s.dirScroll = 0
			s.loadingDir = true
			if s.client != nil {
				s.client.MpdBrowseDir(strings.Join(s.dirPath, "/"))
			}
		} else {
			// Add file to queue
			if s.client != nil {
				uri := e.File
				if uri == "" {
					uri = e.Name
				}
				s.client.MpdCmd("mpd_add", map[string]any{"uri": uri})
			}
		}

	case "h", "esc":
		if len(s.dirPath) > 0 {
			s.dirPath = s.dirPath[:len(s.dirPath)-1]
			s.dirEntries = nil
			s.dirCursor = 0
			s.dirScroll = 0
			s.loadingDir = true
			if s.client != nil {
				s.client.MpdBrowseDir(strings.Join(s.dirPath, "/"))
			}
		}

	case "a":
		if len(s.dirEntries) == 0 || s.dirCursor >= len(s.dirEntries) || s.client == nil {
			break
		}
		e := s.dirEntries[s.dirCursor]
		uri := e.File
		if uri == "" {
			uri = e.Name
		}
		if e.IsDir {
			uri = strings.Join(append(s.dirPath, e.Name), "/")
		}
		s.client.MpdCmd("mpd_add", map[string]any{"uri": uri})

	case "D":
		s.dirMode = false
	}

	return s
}

// HandleMouse handles a left-click within the library's own coordinate space.
func (s MusicLibraryScreen) HandleMouse(x, localY int) MusicLibraryScreen {
	if s.dirMode {
		return s.handleDirMouse(x, localY)
	}
	return s.handleTagMouse(x, localY)
}

// HandleRightMouse handles a right-click. If it lands on a track row,
// it opens the per-track context dialog with all four actions
// (Add to queue, Replace queue, Add to Playlist, Create Playlist).
func (s MusicLibraryScreen) HandleRightMouse(x, localY int) MusicLibraryScreen {
	if s.dirMode || s.dialogOpen {
		return s
	}
	paneW := s.width / 3
	if paneW < 10 {
		paneW = 10
	}
	listH := s.height - 4
	if listH < 1 {
		listH = 1
	}
	// Header row (localY=0) + border (localY=listH+1) — only data rows count.
	if localY <= 0 {
		return s
	}
	dataRow := localY - 1
	if dataRow < 0 || dataRow >= listH {
		return s
	}
	// Tracks pane is the rightmost (x >= 2*paneW+1). Right-click outside
	// the tracks column is ignored.
	if x < 2*paneW+1 {
		return s
	}
	scroll := libScroll(len(s.songs), s.songCursor, listH)
	idx := scroll + dataRow
	if idx < 0 || idx >= len(s.songs) {
		return s
	}
	s.songCursor = idx
	s.activePane = LibPaneTracks
	song := s.songs[idx]
	s.dialogSong = song
	s.dialogContext = libDialogRightClick
	s.dialog = components.NewDialog(
		"Track: '"+truncate(song.Title, 28)+"'",
		[]string{"Add to queue", "Replace queue", "Add to Playlist", "Create Playlist", "Cancel"},
	)
	s.dialogOpen = true
	return s
}

// applyDialogChoice runs the action matching the chosen index, with
// the option set determined by s.dialogContext. -1 = cancel/esc.
func (s MusicLibraryScreen) applyDialogChoice(chosen int) MusicLibraryScreen {
	s.dialogOpen = false
	if s.client == nil || chosen < 0 {
		return s
	}
	switch s.dialogContext {
	case libDialogEnter:
		switch chosen {
		case 0: // Add to queue
			if _, exists := s.queueFiles[s.dialogSong.File]; exists {
				s.statusMsg = "'" + s.dialogSong.Title + "' is already in the queue"
			} else {
				s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
				s.statusMsg = "Added '" + s.dialogSong.Title + "' to queue"
			}
		case 1: // Replace queue
			s.client.MpdCmd("mpd_clear", nil)
			s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
			s.statusMsg = "Replaced queue with '" + s.dialogSong.Title + "'"
		}
	case libDialogRightClick:
		switch chosen {
		case 0: // Add to queue
			if _, exists := s.queueFiles[s.dialogSong.File]; exists {
				s.statusMsg = "'" + s.dialogSong.Title + "' is already in the queue"
			} else {
				s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
				s.statusMsg = "Added '" + s.dialogSong.Title + "' to queue"
			}
		case 1: // Replace queue
			s.client.MpdCmd("mpd_clear", nil)
			s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
			s.statusMsg = "Replaced queue with '" + s.dialogSong.Title + "'"
		case 2: // Add to Playlist (placeholder until playlist picker exists)
			s.statusMsg = "Add to Playlist: not implemented yet"
		case 3: // Create Playlist (placeholder until name prompt exists)
			s.statusMsg = "Create Playlist: not implemented yet"
		}
	}
	return s
}

func (s MusicLibraryScreen) handleTagMouse(x, localY int) MusicLibraryScreen {
	paneW := s.width / 3
	if paneW < 10 {
		paneW = 10
	}
	// listH = View's h - 2, where h = terminal_height - 2 → listH = s.height - 4
	listH := s.height - 4
	if listH < 1 {
		listH = 1
	}
	if localY == 0 {
		// Header row — switch active pane by X.
		switch {
		case x < paneW:
			s.activePane = LibPaneArtists
		case x < 2*paneW+1:
			s.activePane = LibPaneAlbums
		default:
			s.activePane = LibPaneTracks
		}
		return s
	}
	dataRow := localY - 1
	if dataRow < 0 || dataRow >= listH {
		return s
	}
	// Determine clicked pane.
	var clicked LibraryPane
	switch {
	case x < paneW:
		clicked = LibPaneArtists
	case x < 2*paneW+1:
		clicked = LibPaneAlbums
	default:
		clicked = LibPaneTracks
	}
	s.activePane = clicked
	switch clicked {
	case LibPaneArtists:
		scroll := libScroll(len(s.artists), s.artistCursor, listH)
		idx := scroll + dataRow
		if idx >= 0 && idx < len(s.artists) && idx != s.artistCursor {
			s.artistCursor = idx
			s.albumCursor = 0
			s.albumScroll = 0
			s.songCursor = 0
			s.songScroll = 0
			if s.client != nil {
				s.loadingAlbums = true
				s.albums = nil
				s.songs = nil
				s.client.MpdListAlbums(s.artists[s.artistCursor].Name)
			}
		}
	case LibPaneAlbums:
		scroll := libScroll(len(s.albums), s.albumCursor, listH)
		idx := scroll + dataRow
		if idx >= 0 && idx < len(s.albums) && idx != s.albumCursor {
			s.albumCursor = idx
			s.songCursor = 0
			s.songScroll = 0
			if s.client != nil && s.artistCursor < len(s.artists) {
				s.loadingSongs = true
				s.songs = nil
				s.client.MpdListSongs(s.artists[s.artistCursor].Name, s.albums[s.albumCursor].Title)
			}
		}
	case LibPaneTracks:
		scroll := libScroll(len(s.songs), s.songCursor, listH)
		idx := scroll + dataRow
		if idx >= 0 && idx < len(s.songs) {
			s.songCursor = idx
		}
	}
	return s
}

func (s MusicLibraryScreen) handleDirMouse(x, localY int) MusicLibraryScreen {
	listH := s.height - 4
	if listH < 1 {
		listH = 1
	}
	dataRow := localY - 1 // -1 for breadcrumb
	if dataRow < 0 || dataRow >= listH {
		return s
	}
	scroll := 0
	if len(s.dirEntries) > listH {
		scroll = s.dirCursor - listH/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(s.dirEntries)-listH {
			scroll = len(s.dirEntries) - listH
		}
	}
	idx := scroll + dataRow
	if idx >= 0 && idx < len(s.dirEntries) {
		s.dirCursor = idx
	}
	return s
}

// libScroll computes the scroll offset for a pane column, mirroring buildPaneLines.
func libScroll(n, cursor, maxH int) int {
	vl := components.NewVirtualizedList(n, cursor, maxH, components.WithScrollMode(components.ScrollModeCenter))
	start, _ := vl.VisibleRange()
	return start
}

// View renders the library screen within the given width/height constraints.
func (s MusicLibraryScreen) View(w, h int) string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())

	if s.dirMode {
		return s.viewDir(w, h, accentStyle, dimStyle, textStyle)
	}
	return s.viewTag(w, h, accentStyle, dimStyle, textStyle)
}

// viewTag renders the three-column tag browser.
func (s MusicLibraryScreen) viewTag(w, h int, accentStyle, dimStyle, textStyle lipgloss.Style) string {
	// While the artist list is still in-flight (or has timed out with no
	// data), show a centered spinner across the whole library area —
	// matches the Movies/Series tab loading pattern instead of stuffing
	// the spinner under the Artists column.
	if (s.loadingArtists || s.artistError != "") && len(s.artists) == 0 {
		spinView := s.spinner.View()
		if spinView == "" {
			spinView = "Loading library…"
		}
		msg := lipgloss.NewStyle().Foreground(theme.T.Neon()).Render(spinView)
		if s.artistError != "" {
			msg = lipgloss.NewStyle().Foreground(theme.T.Yellow()).
				Render("⚠ " + s.artistError)
		}
		return CenteredMsg(w, h, msg)
	}

	// Reserve 1 row: header line
	listH := h - 1
	if listH < 1 {
		listH = 1
	}

	paneW := w / 3
	if paneW < 10 {
		paneW = 10
	}

	borderStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 1)

	var sb strings.Builder

	// Header
	headerStr := s.tagHeader(accentStyle, dimStyle, paneW)
	sb.WriteString(headerStr + "\n")

	// Build each column
	artistLoadingText := s.spinner.View()
	if artistLoadingText == "" {
		artistLoadingText = "Loading…"
	}
	artistLines := s.buildPaneLines(
		s.artistNames(), s.artistCursor, s.artistScroll, listH,
		s.activePane == LibPaneArtists, s.loadingArtists, artistLoadingText, "No artists",
		paneW, accentStyle, dimStyle, textStyle,
	)
	albumLines := s.buildPaneLines(
		s.albumNames(), s.albumCursor, s.albumScroll, listH,
		s.activePane == LibPaneAlbums, s.loadingAlbums, "Loading…", "No albums",
		paneW, accentStyle, dimStyle, textStyle,
	)
	songLines := s.buildPaneLines(
		s.songNames(), s.songCursor, s.songScroll, listH,
		s.activePane == LibPaneTracks, s.loadingSongs, "Loading…", "No tracks",
		paneW, accentStyle, dimStyle, textStyle,
	)

	sep := dimStyle.Render("│")
	var paneContent strings.Builder
	for i := 0; i < listH; i++ {
		al := ""
		bl := ""
		sl := ""
		if i < len(artistLines) {
			al = artistLines[i]
		}
		if i < len(albumLines) {
			bl = albumLines[i]
		}
		if i < len(songLines) {
			sl = songLines[i]
		}
		paneContent.WriteString(al + sep + bl + sep + sl + "\n")
	}

	// Wrap in border container
	borderedContent := borderStyle.Width(w - 2).Render(paneContent.String())
	sb.WriteString(borderedContent + "\n")

	return sb.String()
}

// tagHeader builds the three-column header row.
func (s MusicLibraryScreen) tagHeader(accentStyle, dimStyle lipgloss.Style, paneW int) string {
	render := func(label string, active bool) string {
		padded := fmt.Sprintf("%-*s", paneW, label)
		if active {
			return accentStyle.Render(padded)
		}
		return dimStyle.Render(padded)
	}
	sep := dimStyle.Render("│")
	return render("Artists", s.activePane == LibPaneArtists) +
		sep +
		render("Albums", s.activePane == LibPaneAlbums) +
		sep +
		render("Tracks", s.activePane == LibPaneTracks)
}

// buildPaneLines renders one column of the tag browser.
func (s MusicLibraryScreen) buildPaneLines(
	items []string,
	cursor, scroll, maxH int,
	active, loading bool,
	loadingText, emptyText string,
	paneW int,
	accentStyle, dimStyle, textStyle lipgloss.Style,
) []string {
	var lines []string

	if loading {
		lines = append(lines, dimStyle.Render(fmt.Sprintf("  %-*s", paneW-2, loadingText)))
		for len(lines) < maxH {
			lines = append(lines, strings.Repeat(" ", paneW))
		}
		return lines
	}

	if len(items) == 0 {
		lines = append(lines, dimStyle.Render(fmt.Sprintf("  %-*s", paneW-2, emptyText)))
		for len(lines) < maxH {
			lines = append(lines, strings.Repeat(" ", paneW))
		}
		return lines
	}

	// Scrolling
	if len(items) > maxH {
		scroll = cursor - maxH/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(items)-maxH {
			scroll = len(items) - maxH
		}
	} else {
		scroll = 0
	}

	end := scroll + maxH
	if end > len(items) {
		end = len(items)
	}

	for i := scroll; i < end; i++ {
		isCursor := i == cursor
		prefix := "  "
		var style lipgloss.Style
		if isCursor && active {
			prefix = "▶ "
			style = accentStyle
		} else if isCursor {
			prefix = "▶ "
			style = textStyle
		} else {
			style = textStyle
		}
		label := truncate(items[i], paneW-2)
		line := style.Render(fmt.Sprintf("%s%-*s", prefix, paneW-2, label))
		lines = append(lines, line)
	}

	for len(lines) < maxH {
		lines = append(lines, strings.Repeat(" ", paneW))
	}
	return lines
}

// artistNames returns the list of artist names for rendering.
func (s MusicLibraryScreen) artistNames() []string {
	names := make([]string, len(s.artists))
	for i, a := range s.artists {
		names[i] = a.Name
	}
	return names
}

// yearRegex finds the first 19xx or 20xx four-digit year anywhere in a
// string. MPD/file metadata is wildly inconsistent — values can look like
// "1996", "1996-11-01", "11/1996", "released 1996", or even "1996/2010"
// (compilation/reissue). This grabs the first plausible year regardless
// of position, ignoring leading garbage and slash-style date formats.
var yearRegex = regexp.MustCompile(`(?:19|20)\d{2}`)

// extractYear pulls a 4-digit year out of an arbitrary date string and
// returns it (or "" if no plausible year is present).
func extractYear(s string) string {
	return yearRegex.FindString(s)
}

// albumNames returns album display strings. When a release year is
// present, albums are prefixed as "(YYYY) Title" so same-titled releases
// can be distinguished. Year is extracted via extractYear so any date
// format MPD reports (1996, 1996-11-01, 11/1996, "released 1996") shows
// up consistently.
func (s MusicLibraryScreen) albumNames() []string {
	names := make([]string, len(s.albums))
	for i, a := range s.albums {
		if year := extractYear(a.Year); year != "" {
			names[i] = "(" + year + ") " + a.Title
		} else {
			names[i] = a.Title
		}
	}
	return names
}

// songNames returns the list of song titles for rendering.
func (s MusicLibraryScreen) songNames() []string {
	names := make([]string, len(s.songs))
	for i, sg := range s.songs {
		names[i] = sg.Title
	}
	return names
}

// viewDir renders the directory browser view.
func (s MusicLibraryScreen) viewDir(w, h int, accentStyle, dimStyle, textStyle lipgloss.Style) string {
	var sb strings.Builder

	// Breadcrumb
	crumb := "  ▸ /"
	if len(s.dirPath) > 0 {
		crumb = "  ▸ " + strings.Join(s.dirPath, " / ") + " /"
	}
	sb.WriteString(accentStyle.Render(crumb) + "\n")

	// Reserve 1 row: breadcrumb
	listH := h - 1
	if listH < 1 {
		listH = 1
	}

	if s.loadingDir {
		sb.WriteString(dimStyle.Render("  Loading…") + "\n")
		return sb.String()
	}

	if len(s.dirEntries) == 0 {
		sb.WriteString(dimStyle.Render("  Empty directory") + "\n")
		return sb.String()
	}

	// Scrolling
	scroll := 0
	if len(s.dirEntries) > listH {
		scroll = s.dirCursor - listH/2
		if scroll < 0 {
			scroll = 0
		}
		if scroll > len(s.dirEntries)-listH {
			scroll = len(s.dirEntries) - listH
		}
	}

	end := scroll + listH
	if end > len(s.dirEntries) {
		end = len(s.dirEntries)
	}

	for i := scroll; i < end; i++ {
		e := s.dirEntries[i]
		isCursor := i == s.dirCursor

		prefix := "  "
		if isCursor {
			prefix = "▶ "
		}

		var line string
		if e.IsDir {
			dirTag := dimStyle.Render("[dir]  ")
			name := truncate(e.Name, w-9)
			if isCursor {
				line = accentStyle.Render(prefix) + dirTag + accentStyle.Render(name)
			} else {
				line = textStyle.Render(prefix) + dirTag + textStyle.Render(name)
			}
		} else {
			dur := fmtMusicDuration(e.Duration)
			nameW := w - 9 - len(dur) - 2
			if nameW < 4 {
				nameW = 4
			}
			name := truncate(e.Name, nameW)
			durStr := fmt.Sprintf("%*s", len(dur), dur)
			if isCursor {
				line = accentStyle.Render(fmt.Sprintf("%s%-*s  %s", prefix, nameW, name, durStr))
			} else {
				line = textStyle.Render(fmt.Sprintf("%s%-*s  %s", prefix, nameW, name, durStr))
			}
		}
		sb.WriteString(line + "\n")
	}

	// Pad remaining rows
	rendered := end - scroll
	for i := rendered; i < listH; i++ {
		sb.WriteString("\n")
	}

	return sb.String()
}
