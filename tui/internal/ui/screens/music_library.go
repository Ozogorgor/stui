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
//
// Refactor note (Task 5.2):
// The 3-column rendering + generic navigation lives in catalogbrowser.Model.
// This file keeps tag/dir mode selector, MPD IPC, dialog/action menus,
// tag-normalization, and all library-specific keybinds.
// A temporary mpdLibraryStub (bottom of file) implements catalogbrowser.DataSource
// by pointing at the screen's existing MPD slices; Task 5.3 replaces it.

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
	"github.com/stui/stui/internal/ui/screens/catalogbrowser"
	"github.com/stui/stui/pkg/log"
	"github.com/stui/stui/pkg/theme"
)

// libDialogCtx tells the dialog dismiss handler which option set the
// user was looking at, so we can interpret the chosen index correctly.
// The dialog is the default action surface for Enter and right-click on
// every pane (artist/album/track) — h/l/arrows still navigate as a
// power-user shortcut, but the dialog is what the UI advertises.
type libDialogCtx int

const (
	libDialogEnter            libDialogCtx = iota // Track Enter — Add / Replace / Cancel
	libDialogRightClick                           // Track right-click — Add / Replace / Add to Playlist / Create Playlist / Cancel
	libDialogArtist                               // Artist Enter or right-click — Browse / Add all / Replace with all / Cancel
	libDialogAlbum                                // Album Enter or right-click — Browse / Add / Replace / Add to Playlist / Normalize / Cancel
	libDialogNormalizeScope                       // Scope picker: This album / This artist / Whole library / Cancel
	libDialogNormalizeConfirm                     // Preview confirm: Apply / Cancel
)

// MusicLibraryScreen is the Artist→Album→Track browser with an optional
// directory-tree view toggled with D.
type MusicLibraryScreen struct {
	Dims
	client  *ipc.Client
	dirMode bool // false = tag browser, true = directory browser

	// browser is the reusable 3-column navigation component.
	// It owns cursor/scroll state and generic j/k/h/l navigation.
	browser catalogbrowser.Model

	// source is the DataSource wired into the browser. Music Library still
	// owns MPD list fetches; source.SetAll forwards the latest slices into
	// the browser after each slice mutation. Task 6.2 will wire Search().
	source *catalogbrowser.MpdDataSource

	// Tag browser state (raw MPD data; also forwarded to source → browser)
	artists        []ipc.MpdArtist
	albums         []ipc.MpdAlbum
	songs          []ipc.MpdSong
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

	// Footer status message (e.g. "Added 'Track' to queue"). Surfaced in
	// the local hintBar for statusTTL after being set, and also forwarded
	// to the global footer exactly once when statusPending is true.
	statusMsg     string
	statusAt      time.Time
	statusPending bool

	// queueFiles tracks file paths currently in the MPD queue for dedup checks.
	queueFiles map[string]struct{}

	// Track-action dialog (shown when Enter is pressed on a track or a
	// track row is right-clicked). dialogContext describes which option
	// set is showing — Enter uses the simple Add/Replace/Cancel set, and
	// right-click uses the extended Add/Replace/Add to Playlist/Create
	// Playlist set so the option indices are interpreted correctly.
	initDone      bool // set after first successful data load; prevents stray startup events from opening dialogs
	dialogOpen    bool
	dialog        components.Dialog
	dialogSong    ipc.MpdSong
	dialogContext libDialogCtx

	// Playlist prompt state (used for "Create Playlist" and "Add to Playlist")
	playlistPrompt bool
	playlistName   string
	playlistCreate bool     // true = create (clear+add), false = add (append)
	playlistURIs   []string // URIs to add when prompt is confirmed

	// Tag normalization state
	normalizeJobID string
	normalizeRows  []ipc.TagDiffRow
	normalizeScope ipc.TagWriteScope

	spinner      components.Spinner
	loadingStart time.Time
}

// newLibraryBrowser constructs the catalogbrowser.Model backed by an MpdDataSource.
func newLibraryBrowser(src *catalogbrowser.MpdDataSource) catalogbrowser.Model {
	cols := []catalogbrowser.ColumnDef{
		{Kind: ipc.KindArtist, Label: "Artists"},
		{Kind: ipc.KindAlbum, Label: "Albums"},
		{Kind: ipc.KindTrack, Label: "Tracks"},
	}
	return catalogbrowser.New(src, cols)
}

// NewMusicLibraryScreen creates a new library screen and starts fetching artists.
func NewMusicLibraryScreen(client *ipc.Client) MusicLibraryScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	src := catalogbrowser.NewMpdDataSource(client)
	s := MusicLibraryScreen{
		client:         client,
		source:         src,
		browser:        newLibraryBrowser(src),
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

// syncStub forwards the current MPD slices into the MpdDataSource so the
// browser sees up-to-date data. Called after any slice mutation.
func (s *MusicLibraryScreen) syncStub() {
	s.source.SetAll(map[ipc.EntryKind][]catalogbrowser.Entry{
		ipc.KindArtist: catalogbrowser.MapMpdArtists(s.artists),
		ipc.KindAlbum:  catalogbrowser.MapMpdAlbums(s.albums),
		ipc.KindTrack:  catalogbrowser.MapMpdSongs(s.songs),
	})
}

// activePane returns the currently focused column as a 0-based index
// matching the browser's column order (0=Artists, 1=Albums, 2=Tracks).
func (s MusicLibraryScreen) activePane() int {
	return s.browser.ActiveColumn()
}

// artistCursor returns the artist cursor from the browser.
func (s MusicLibraryScreen) artistCursor() int { return s.browser.ColumnCursor(0) }

// albumCursor returns the album cursor from the browser.
func (s MusicLibraryScreen) albumCursor() int { return s.browser.ColumnCursor(1) }

// songCursor returns the song cursor from the browser.
func (s MusicLibraryScreen) songCursor() int { return s.browser.ColumnCursor(2) }

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
		s.browser.SetSize(m.Width, m.Height)

	case ipc.MpdLibraryResultMsg:
		log.Info("library: MpdLibraryResultMsg",
			"forArtist", m.ForArtist, "forAlbum", m.ForAlbum,
			"artists", len(m.Artists), "albums", len(m.Albums),
			"songs", len(m.Songs), "err", m.Err)
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
		s.initDone = true
		if m.ForAlbum != "" {
			// Songs for a specific album arrived
			s.songs = m.Songs
			s.loadingSongs = false
		} else if m.ForArtist != "" {
			// Albums for a specific artist arrived
			s.albums = m.Albums
			s.browser.SetCursor(1, 0) // reset album cursor
			s.loadingAlbums = false
			// Pre-fetch songs for the first album
			if len(m.Albums) > 0 && s.client != nil {
				s.loadingSongs = true
				s.client.MpdListSongs(m.ForArtist, m.Albums[0].MpdTitle(), m.Albums[0].Date)
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
		s.syncStub()

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

	case ipc.MarkTagExceptionResultMsg:
		if m.Err != nil {
			s.statusMsg = fmt.Sprintf("Exception failed: %v", m.Err)
			s.statusAt = time.Now()
			s.statusPending = true
		}
		return s, nil

	case ipc.ActionAPreviewResultMsg:
		if m.Err != nil {
			s = s.setStatus(fmt.Sprintf("Preview failed: %v", m.Err))
			return s, nil
		}
		if len(m.Rows) == 0 {
			s = s.setStatus("Nothing to normalize — all tags are already clean")
			return s, nil
		}
		s.normalizeJobID = m.JobID
		s.normalizeRows = m.Rows
		s.dialogContext = libDialogNormalizeConfirm
		s.dialog = components.NewDialog(
			fmt.Sprintf("Normalize %d changes across %d files?", len(m.Rows), m.TotalFiles),
			[]string{"Apply", "Cancel"},
		)
		s.dialogOpen = true
		return s, nil

	case ipc.ActionAApplyResultMsg:
		if m.Err != nil {
			s = s.setStatus(fmt.Sprintf("Apply failed: %v", m.Err))
		} else if m.Failed > 0 {
			s = s.setStatus(fmt.Sprintf("Wrote %d files (%d failed)", m.Succeeded, m.Failed))
		} else {
			s = s.setStatus(fmt.Sprintf("Wrote %d files — tags normalized", m.Succeeded))
		}
		s.normalizeJobID = ""
		s.normalizeRows = nil
		return s, nil

	case catalogbrowser.NavMsg:
		// The browser posted a navigation event — react to it for MPD pre-fetching.
		s = s.handleBrowserNav(m)
		return s, nil

	case catalogbrowser.MpdSearchAppliedMsg:
		// The DataSource already swapped items for the search result set.
		// Reset column cursors so the new (shorter) result columns don't try
		// to render an out-of-range index from the pre-search state.
		s.browser.SetCursor(0, 0)
		s.browser.SetCursor(1, 0)
		s.browser.SetCursor(2, 0)
		return s, nil

	case catalogbrowser.MpdSearchFailedMsg:
		s = s.setStatus(fmt.Sprintf("Search failed: %v", m.Err))
		return s, nil

	case tea.KeyPressMsg:
		// Playlist name prompt intercepts all keys when active.
		if s.playlistPrompt {
			return s.handlePlaylistPrompt(m)
		}
		var keyCmd tea.Cmd
		if s.dirMode {
			s = s.handleDirKey(m.String())
		} else {
			s, keyCmd = s.handleTagKey(m.String())
		}
		// Fall through to statusPending flush logic below, batching keyCmd if needed.
		if s.statusPending {
			s.statusPending = false
			text := s.statusMsg
			statusCmd := func() tea.Msg { return ipc.StatusMsg{Text: text} }
			if keyCmd != nil {
				return s, tea.Batch(keyCmd, statusCmd)
			}
			return s, statusCmd
		}
		return s, keyCmd
	}

	// This fallthrough case only applies to non-KeyPressMsg types. For KeyPress,
	// the above block handles the statusPending flush.
	if s.statusPending {
		s.statusPending = false
		text := s.statusMsg
		return s, func() tea.Msg { return ipc.StatusMsg{Text: text} }
	}
	return s, nil
}

// handleBrowserNav reacts to a catalogbrowser.NavMsg by issuing the
// appropriate MPD pre-fetch when the cursor moves within a column.
func (s MusicLibraryScreen) handleBrowserNav(nav catalogbrowser.NavMsg) MusicLibraryScreen {
	if s.client == nil {
		return s
	}
	switch nav.Column {
	case 0: // Artist cursor moved — fetch albums for the new artist
		if nav.Row < len(s.artists) {
			s.loadingAlbums = true
			s.albums = nil
			s.songs = nil
			s.browser.SetCursor(1, 0)
			s.browser.SetCursor(2, 0)
			s.syncStub()
			s.client.MpdListAlbums(s.artists[nav.Row].Name)
		}
	case 1: // Album cursor moved — fetch songs for the new album
		ac := s.artistCursor()
		if nav.Row < len(s.albums) && ac < len(s.artists) {
			s.loadingSongs = true
			s.songs = nil
			s.browser.SetCursor(2, 0)
			s.syncStub()
			a := s.albums[nav.Row]
			s.client.MpdListSongs(s.artists[ac].Name, a.MpdTitle(), a.Date)
		}
	}
	return s
}

// setStatus records a footer message, arms it for forwarding to the global
// footer on the next Update tail, and resets the local-echo clock so the
// in-screen hintBar shows it for statusTTL.
func (s MusicLibraryScreen) setStatus(msg string) MusicLibraryScreen {
	s.statusMsg = msg
	s.statusAt = time.Now()
	s.statusPending = true
	return s
}

// statusTTL is how long the library's local hintBar keeps echoing a
// recently-set status message before falling back to the default key hints.
const statusTTL = 3 * time.Second

// openPlaylistPromptDialog sets up (or refreshes) the dialog overlay that
// serves as the visible centered prompt for the playlist name input. Call
// this every time playlistName changes so the title stays in sync.
func (s MusicLibraryScreen) openPlaylistPromptDialog() MusicLibraryScreen {
	action := "Create playlist"
	if !s.playlistCreate {
		action = "Add to playlist"
	}
	s.dialog = components.NewDialog(
		fmt.Sprintf("%s: %s█", action, s.playlistName),
		[]string{"(type playlist name, Enter to confirm)"},
	)
	s.dialogOpen = true
	return s
}

// handlePlaylistPrompt processes key events while the playlist name prompt
// is active. All keys are consumed so nothing leaks to the tag/dir handlers.
func (s MusicLibraryScreen) handlePlaylistPrompt(m tea.KeyPressMsg) (MusicLibraryScreen, tea.Cmd) {
	switch m.String() {
	case "esc":
		s.playlistPrompt = false
		s.playlistName = ""
		s.playlistURIs = nil
		s.dialogOpen = false
	case "backspace":
		if len(s.playlistName) > 0 {
			runes := []rune(s.playlistName)
			s.playlistName = string(runes[:len(runes)-1])
		}
		s = s.openPlaylistPromptDialog()
	case "enter":
		if s.playlistName != "" && s.client != nil && len(s.playlistURIs) > 0 {
			name := s.playlistName
			if s.playlistCreate {
				s.client.MpdCmd("mpd_playlist_create", map[string]any{
					"name": name,
					"uris": s.playlistURIs,
				})
				s = s.setStatus(fmt.Sprintf("Created playlist '%s' with %d tracks", name, len(s.playlistURIs)))
			} else {
				for _, uri := range s.playlistURIs {
					s.client.MpdCmd("mpd_playlist_add_track", map[string]any{
						"name": name,
						"uri":  uri,
					})
				}
				s = s.setStatus(fmt.Sprintf("Added %d tracks to '%s'", len(s.playlistURIs), name))
			}
			// Trigger playlists re-fetch so the playlists tab shows the new/updated playlist.
			s.client.MpdGetPlaylists()
			s.playlistPrompt = false
			s.playlistName = ""
			s.playlistURIs = nil
			s.dialogOpen = false
		}
	default:
		if len(m.Text) > 0 {
			s.playlistName += m.Text
		}
		s = s.openPlaylistPromptDialog()
	}
	return s, nil
}

// handleTagKey processes key events in tag-browser mode.
// Library-specific keys (R, D, x/X, enter, dialogs) are handled here.
// Generic navigation (j/k/h/l) is delegated to the browser.
func (s MusicLibraryScreen) handleTagKey(key string) (MusicLibraryScreen, tea.Cmd) {
	// Dialog intercepts all keys when open.
	if s.dialogOpen {
		var chosen int
		var dismissed bool
		s.dialog, chosen, dismissed = s.dialog.Update(key)
		if dismissed {
			s = s.applyDialogChoice(chosen)
		}
		return s, nil
	}

	switch key {
	case "enter":
		// Guard: don't open dialogs until the first data has loaded.
		if !s.initDone {
			return s, nil
		}
		s = s.openPaneDialog()
		return s, nil

	case "R":
		if s.client != nil {
			s.client.MpdCmd("mpd_update", nil)
			s = s.setStatus("Rescanning MPD library…")
			// Re-fetch artists after a short delay for the scan to start
			s.loadingArtists = true
			s.artists = nil
			s.albums = nil
			s.songs = nil
			s.syncStub()
			s.client.MpdListArtists()
		}
		return s, nil

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
		return s, nil

	case "x", "X":
		s = s.handleMarkExceptionTag()
		return s, nil

	case "l", "right":
		// l key: when moving from artist → albums pane, also trigger MPD
		// pre-fetch (same as the old navigation did). We handle it here so
		// we can fire the fetch BEFORE delegating to the browser (which
		// updates the active column but doesn't know about MPD).
		active := s.activePane()
		if active == 0 && len(s.artists) > 0 && s.client != nil {
			s.loadingAlbums = true
			s.albums = nil
			s.songs = nil
			s.browser.SetCursor(1, 0)
			s.browser.SetCursor(2, 0)
			s.syncStub()
			s.client.MpdListAlbums(s.artists[s.artistCursor()].Name)
		} else if active == 1 && len(s.albums) > 0 && len(s.artists) > 0 && s.client != nil {
			s.loadingSongs = true
			s.songs = nil
			s.browser.SetCursor(2, 0)
			s.syncStub()
			a := s.albums[s.albumCursor()]
			s.client.MpdListSongs(s.artists[s.artistCursor()].Name, a.MpdTitle(), a.Date)
		}
		// Delegate to browser for the column-focus switch.
		newBrowser, cmd := s.browser.HandleKey(key)
		s.browser = newBrowser
		return s, cmd

	default:
		// Delegate remaining navigation keys (j/k/up/down/h/left) to browser.
		newBrowser, cmd := s.browser.HandleKey(key)
		s.browser = newBrowser
		return s, cmd
	}
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
			// File — open the action dialog instead of adding silently.
			uri := e.File
			if uri == "" {
				uri = e.Name
			}
			title := e.Title
			if title == "" {
				title = e.Name
			}
			s.dialogSong = ipc.MpdSong{
				File:  uri,
				Title: title,
			}
			s.dialogContext = libDialogRightClick
			s.dialog = components.NewDialog(
				"What to do with '"+truncate(title, 28)+"'?",
				[]string{"Add to queue", "Replace queue", "Add to Playlist", "Create Playlist", "Cancel"},
			)
			s.dialogOpen = true
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

	case "x", "X":
		s = s.handleMarkExceptionDir()
	}

	return s
}

// handleMarkExceptionTag marks the currently-selected item's tag fields as
// normalization exceptions (tag-browser mode).
func (s MusicLibraryScreen) handleMarkExceptionTag() MusicLibraryScreen {
	if s.client == nil {
		return s
	}
	switch s.activePane() {
	case 0: // Artists
		ac := s.artistCursor()
		if ac < len(s.artists) {
			name := s.artists[ac].Name
			s.client.MarkTagException("artist", name)
			s = s.setStatus(fmt.Sprintf("Protected artist: %s", name))
		}
	case 1: // Albums
		ac := s.albumCursor()
		if ac < len(s.albums) {
			a := s.albums[ac]
			raw := a.RawArtist
			if raw == "" {
				raw = a.Artist
			}
			if raw != "" {
				s.client.MarkTagException("artist", raw)
			}
			raw = a.RawTitle
			if raw == "" {
				raw = a.Title
			}
			if raw != "" {
				s.client.MarkTagException("album", raw)
			}
			s = s.setStatus(fmt.Sprintf("Protected: %s — %s", a.Artist, a.Title))
		}
	case 2: // Tracks
		sc := s.songCursor()
		if sc < len(s.songs) {
			song := s.songs[sc]
			raw := song.RawArtist
			if raw == "" {
				raw = song.Artist
			}
			if raw != "" {
				s.client.MarkTagException("artist", raw)
			}
			raw = song.RawAlbum
			if raw == "" {
				raw = song.Album
			}
			if raw != "" {
				s.client.MarkTagException("album", raw)
			}
			raw = song.RawTitle
			if raw == "" {
				raw = song.Title
			}
			if raw != "" {
				s.client.MarkTagException("title", raw)
			}
			s = s.setStatus(fmt.Sprintf("Protected: %s", song.Title))
		}
	}
	return s
}

// handleMarkExceptionDir marks the currently-selected directory entry's tag
// fields as normalization exceptions (directory-browser mode).
func (s MusicLibraryScreen) handleMarkExceptionDir() MusicLibraryScreen {
	if s.client == nil || s.dialogOpen {
		return s
	}
	if s.dirCursor >= len(s.dirEntries) {
		return s
	}
	entry := s.dirEntries[s.dirCursor]
	if entry.IsDir {
		s = s.setStatus("Can't mark a directory as exception")
		return s
	}
	raw := entry.RawArtist
	if raw == "" {
		raw = entry.Artist
	}
	if raw != "" {
		s.client.MarkTagException("artist", raw)
	}
	raw = entry.RawAlbum
	if raw == "" {
		raw = entry.Album
	}
	if raw != "" {
		s.client.MarkTagException("album", raw)
	}
	raw = entry.RawTitle
	if raw == "" {
		raw = entry.Title
	}
	if raw != "" {
		s.client.MarkTagException("title", raw)
	}
	s = s.setStatus(fmt.Sprintf("Protected tags for: %s", entry.Title))
	return s
}

// HandleMouse handles a left-click within the library's own coordinate space.
func (s MusicLibraryScreen) HandleMouse(x, localY int) MusicLibraryScreen {
	log.Info("library: HandleMouse(left)", "x", x, "localY", localY, "dirMode", s.dirMode)
	if s.dirMode {
		return s.handleDirMouse(x, localY)
	}
	return s.handleTagMouse(x, localY)
}

// HandleRightMouse handles a right-click. The dialog is the advertised
// default action surface, so right-click is wired for every pane —
// artists, albums, and tracks — not just the tracks column.
func (s MusicLibraryScreen) HandleRightMouse(x, localY int) MusicLibraryScreen {
	log.Info("library: HandleRightMouse", "x", x, "localY", localY, "dirMode", s.dirMode, "dialogOpen", s.dialogOpen)
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

	// Decide which pane the click landed in, mirror handleTagMouse's hit-test.
	switch {
	case x < paneW:
		scroll := s.browser.ColScroll(0, listH)
		idx := scroll + dataRow
		if idx < 0 || idx >= len(s.artists) {
			return s
		}
		s.browser.SetCursor(0, idx)
		s.browser.SetActiveColumn(0)
		s = s.openPaneDialog()
		// Right-click on a track gets the extended "Add to Playlist /
		// Create Playlist" set; for artists the Enter set is enough since
		// playlist actions don't apply at the artist level yet.
	case x < 2*paneW+1:
		scroll := s.browser.ColScroll(1, listH)
		idx := scroll + dataRow
		if idx < 0 || idx >= len(s.albums) {
			return s
		}
		s.browser.SetCursor(1, idx)
		s.browser.SetActiveColumn(1)
		s = s.openPaneDialog()
	default:
		scroll := s.browser.ColScroll(2, listH)
		idx := scroll + dataRow
		if idx < 0 || idx >= len(s.songs) {
			return s
		}
		s.browser.SetCursor(2, idx)
		s.browser.SetActiveColumn(2)
		// Tracks keep the extended right-click dialog (4 actions) rather
		// than the slimmer Enter set used by openPaneDialog.
		song := s.songs[idx]
		s.dialogSong = song
		s.dialogContext = libDialogRightClick
		s.dialog = components.NewDialog(
			"Track: '"+truncate(song.Title, 28)+"'",
			[]string{"Add to queue", "Replace queue", "Add to Playlist", "Create Playlist", "Cancel"},
		)
		s.dialogOpen = true
	}
	return s
}

// openPaneDialog builds the right action dialog for the currently-focused
// pane. Track dialogs use the existing Add/Replace/Cancel set; artist and
// album dialogs lead with the navigation option (Browse) so Enter still
// feels like "go deeper" when that's all the user wanted.
func (s MusicLibraryScreen) openPaneDialog() MusicLibraryScreen {
	switch s.activePane() {
	case 0: // Artists
		ac := s.artistCursor()
		if len(s.artists) == 0 || ac >= len(s.artists) {
			return s
		}
		name := s.artists[ac].Name
		s.dialogContext = libDialogArtist
		s.dialog = components.NewDialog(
			"Artist: '"+truncate(name, 28)+"'",
			[]string{"Browse albums", "Add all tracks to queue", "Replace queue with all", "Cancel"},
		)
		s.dialogOpen = true
	case 1: // Albums
		ac := s.albumCursor()
		if len(s.albums) == 0 || ac >= len(s.albums) {
			return s
		}
		title := s.albums[ac].Title
		s.dialogContext = libDialogAlbum
		s.dialog = components.NewDialog(
			"Album: '"+truncate(title, 28)+"'",
			[]string{"Browse tracks", "Add album to queue", "Replace queue with album", "Add to playlist", "Create playlist", "Normalize tags on disk…", "Cancel"},
		)
		s.dialogOpen = true
	case 2: // Tracks
		sc := s.songCursor()
		if len(s.songs) == 0 || sc >= len(s.songs) {
			return s
		}
		song := s.songs[sc]
		s.dialogSong = song
		s.dialogContext = libDialogRightClick
		s.dialog = components.NewDialog(
			"What to do with '"+truncate(song.Title, 28)+"'?",
			[]string{"Add to queue", "Replace queue", "Add to Playlist", "Create Playlist", "Cancel"},
		)
		s.dialogOpen = true
	}
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
				s = s.setStatus("'" + s.dialogSong.Title + "' is already in the queue")
			} else {
				s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
				s = s.setStatus("Added '" + s.dialogSong.Title + "' to queue")
			}
		case 1: // Replace queue
			s.client.MpdCmd("mpd_clear", nil)
			s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
			s = s.setStatus("Replaced queue with '" + s.dialogSong.Title + "'")
		}
	case libDialogRightClick:
		switch chosen {
		case 0: // Add to queue
			if _, exists := s.queueFiles[s.dialogSong.File]; exists {
				s = s.setStatus("'" + s.dialogSong.Title + "' is already in the queue")
			} else {
				s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
				s = s.setStatus("Added '" + s.dialogSong.Title + "' to queue")
			}
		case 1: // Replace queue
			s.client.MpdCmd("mpd_clear", nil)
			s.client.MpdCmd("mpd_add", map[string]any{"uri": s.dialogSong.File})
			s = s.setStatus("Replaced queue with '" + s.dialogSong.Title + "'")
		case 2: // Add to Playlist
			s.playlistPrompt = true
			s.playlistCreate = false
			s.playlistName = ""
			s.playlistURIs = []string{s.dialogSong.File}
			s = s.openPlaylistPromptDialog()
		case 3: // Create Playlist
			s.playlistPrompt = true
			s.playlistCreate = true
			s.playlistName = ""
			s.playlistURIs = []string{s.dialogSong.File}
			s = s.openPlaylistPromptDialog()
		}
	case libDialogArtist:
		ac := s.artistCursor()
		if ac >= len(s.artists) {
			return s
		}
		name := s.artists[ac].Name
		switch chosen {
		case 0: // Browse albums — same as the l/right shortcut
			s.browser.SetActiveColumn(1)
			s.loadingAlbums = true
			s.albums = nil
			s.songs = nil
			s.browser.SetCursor(1, 0)
			s.syncStub()
			s.client.MpdListAlbums(name)
		case 1: // Add all tracks to queue (needs runtime mpd findadd; not wired yet)
			s = s.setStatus("Add all tracks: not implemented yet")
		case 2: // Replace queue with all (same dependency as above)
			s = s.setStatus("Replace queue with all: not implemented yet")
		}
	case libDialogAlbum:
		ac := s.artistCursor()
		alc := s.albumCursor()
		if ac >= len(s.artists) || alc >= len(s.albums) {
			return s
		}
		artist := s.artists[ac].Name
		album := s.albums[alc]
		switch chosen {
		case 0: // Browse tracks — same as the l/right shortcut
			s.browser.SetActiveColumn(2)
			s.loadingSongs = true
			s.songs = nil
			s.browser.SetCursor(2, 0)
			s.syncStub()
			s.client.MpdListSongs(artist, album.MpdTitle(), album.Date)
		case 1: // Add album to queue — relies on having songs already loaded
			added := 0
			for _, song := range s.songs {
				if _, exists := s.queueFiles[song.File]; exists {
					continue
				}
				s.client.MpdCmd("mpd_add", map[string]any{"uri": song.File})
				added++
			}
			s = s.setStatus(fmt.Sprintf("Added %d tracks from '%s' to queue", added, album.Title))
		case 2: // Replace queue with album
			s.client.MpdCmd("mpd_clear", nil)
			for _, song := range s.songs {
				s.client.MpdCmd("mpd_add", map[string]any{"uri": song.File})
			}
			s = s.setStatus(fmt.Sprintf("Replaced queue with '%s' (%d tracks)", album.Title, len(s.songs)))
		case 3: // Add to playlist
			s.playlistPrompt = true
			s.playlistCreate = false
			s.playlistName = ""
			if len(s.songs) > 0 {
				uris := make([]string, 0, len(s.songs))
				for _, song := range s.songs {
					uris = append(uris, song.File)
				}
				s.playlistURIs = uris
				s = s.openPlaylistPromptDialog()
			} else {
				s = s.setStatus("Browse the album's tracks first (press Enter), then try again")
				s.playlistPrompt = false
			}
		case 4: // Create playlist
			s.playlistPrompt = true
			s.playlistCreate = true
			s.playlistName = ""
			if len(s.songs) > 0 {
				uris := make([]string, 0, len(s.songs))
				for _, song := range s.songs {
					uris = append(uris, song.File)
				}
				s.playlistURIs = uris
				s = s.openPlaylistPromptDialog()
			} else {
				s = s.setStatus("Browse the album's tracks first (press Enter), then try again")
				s.playlistPrompt = false
			}
		case 5: // Normalize tags on disk…
			s.dialogContext = libDialogNormalizeScope
			a := s.albums[alc]
			s.normalizeScope = ipc.TagWriteScope{Kind: "album", Artist: artist, Album: a.Title, Date: a.Date}
			s.dialog = components.NewDialog(
				"Normalize tags on disk",
				[]string{"This album", "This artist", "Whole library", "Cancel"},
			)
			s.dialogOpen = true
		}
	case libDialogNormalizeScope:
		switch chosen {
		case 0: // This album — scope already set
		case 1: // This artist
			ac := s.artistCursor()
			if ac < len(s.artists) {
				s.normalizeScope = ipc.TagWriteScope{Kind: "artist", Artist: s.artists[ac].Name}
			}
		case 2: // Whole library
			s.normalizeScope = ipc.TagWriteScope{Kind: "library"}
		default:
			return s // Cancel
		}
		s.client.ActionATagsPreview(s.normalizeScope)
		s = s.setStatus("Computing preview…")
	case libDialogNormalizeConfirm:
		if chosen == 0 { // Apply
			s.client.ActionATagsApply(s.normalizeJobID)
			s = s.setStatus("Writing normalized tags…")
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
			s.browser.SetActiveColumn(0)
		case x < 2*paneW+1:
			s.browser.SetActiveColumn(1)
		default:
			s.browser.SetActiveColumn(2)
		}
		return s
	}
	dataRow := localY - 1
	if dataRow < 0 || dataRow >= listH {
		return s
	}
	// Determine clicked pane.
	var clicked int
	switch {
	case x < paneW:
		clicked = 0
	case x < 2*paneW+1:
		clicked = 1
	default:
		clicked = 2
	}
	s.browser.SetActiveColumn(clicked)
	switch clicked {
	case 0: // Artists
		scroll := s.browser.ColScroll(0, listH)
		idx := scroll + dataRow
		if idx >= 0 && idx < len(s.artists) && idx != s.artistCursor() {
			s.browser.SetCursor(0, idx)
			s.browser.SetCursor(1, 0)
			s.browser.SetCursor(2, 0)
			if s.client != nil {
				s.loadingAlbums = true
				s.albums = nil
				s.songs = nil
				s.syncStub()
				s.client.MpdListAlbums(s.artists[idx].Name)
			}
		}
	case 1: // Albums
		scroll := s.browser.ColScroll(1, listH)
		idx := scroll + dataRow
		if idx >= 0 && idx < len(s.albums) && idx != s.albumCursor() {
			s.browser.SetCursor(1, idx)
			s.browser.SetCursor(2, 0)
			if s.client != nil && s.artistCursor() < len(s.artists) {
				s.loadingSongs = true
				s.songs = nil
				s.syncStub()
				a := s.albums[idx]
				s.client.MpdListSongs(s.artists[s.artistCursor()].Name, a.MpdTitle(), a.Date)
			}
		}
	case 2: // Tracks
		scroll := s.browser.ColScroll(2, listH)
		idx := scroll + dataRow
		if idx >= 0 && idx < len(s.songs) {
			s.browser.SetCursor(2, idx)
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

// View renders the library screen within the given width/height constraints.
func (s MusicLibraryScreen) View(w, h int) string {
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())

	var base string
	if s.dirMode {
		base = s.viewDir(w, h, accentStyle, dimStyle, textStyle)
	} else {
		base = s.viewTag(w, h, accentStyle, dimStyle, textStyle)
	}
	// Overlay the action dialog, if open.
	if s.dialogOpen {
		base = s.dialog.Place(w, h)
	}
	return base
}

// viewTag renders the three-column tag browser by delegating to catalogbrowser.Model.
func (s MusicLibraryScreen) viewTag(w, h int, accentStyle, dimStyle, textStyle lipgloss.Style) string {
	// While the artist list is still in-flight (or has timed out with no
	// data), show a centered spinner across the whole library area.
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

	// Track Info is the optional 4th column. It appears whenever the
	// Tracks pane is focused AND a track is selected.
	sc := s.songCursor()
	showInfo := s.activePane() == 2 && sc < len(s.songs) && len(s.songs) > 0

	// Compute listH so buildTrackInfoLines can pad to the right height.
	listH := h - 3
	if listH < 1 {
		listH = 1
	}

	// We need paneW to build the track info lines. Mirror the browser's
	// calculation: total cols = 3 (or 4 with info), inner content w-6
	// minus separators, divided by cols.
	totalCols := 3
	if showInfo {
		totalCols = 4
	}
	contentW := w - 6 - (totalCols - 1)
	paneW := contentW / totalCols
	if paneW < 10 {
		paneW = 10
	}

	// Prepare extra info column if needed.
	var extraCol *catalogbrowser.ExtraColumn
	if showInfo {
		extraCol = &catalogbrowser.ExtraColumn{
			Label: "Track Info",
			Lines: s.buildTrackInfoLines(listH, paneW, accentStyle, dimStyle, textStyle),
		}
	}

	// Build per-column labels used in the browser's loading/empty display.
	spinnerText := s.spinner.View()
	if spinnerText == "" {
		spinnerText = "Loading…"
	}

	s.browser.SetSize(w, h)
	return s.browser.View(
		accentStyle, dimStyle, textStyle,
		[]bool{s.loadingArtists, s.loadingAlbums, s.loadingSongs},
		[]string{spinnerText, "Loading…", "Loading…"},
		[]string{"No artists", "No albums", "No tracks"},
		extraCol,
	)
}

// FooterText is what the global status bar shows while this screen is
// active. A recent status message wins for statusTTL — long enough for
// the user to read "Added 'X' to queue" — then we fall back to the
// default key-hint string.
func (s MusicLibraryScreen) FooterText() string {
	if s.playlistPrompt {
		action := "Add to playlist"
		if s.playlistCreate {
			action = "Create playlist"
		}
		return fmt.Sprintf("%s: %s█  (enter confirm / esc cancel)", action, s.playlistName)
	}
	if s.statusMsg != "" && !s.statusAt.IsZero() && time.Since(s.statusAt) < statusTTL {
		return s.statusMsg
	}
	if s.dirMode {
		return "enter open · h back · a add · D tag mode"
	}
	return "enter action · h/l navigate · ↑↓ cursor · D dir mode"
}

// buildTrackInfoLines renders the right-hand metadata column for the
// currently selected track.
func (s MusicLibraryScreen) buildTrackInfoLines(
	listH, paneW int,
	accentStyle, dimStyle, textStyle lipgloss.Style,
) []string {
	sc := s.songCursor()
	if sc < 0 || sc >= len(s.songs) {
		return nil
	}
	song := s.songs[sc]

	year := ""
	alc := s.albumCursor()
	if alc < len(s.albums) {
		year = extractYear(s.albums[alc].Year)
	}

	innerW := paneW - 2

	row := func(label, value string) []string {
		labelLine := dimStyle.Width(paneW).Render(fmt.Sprintf(" %s", label))
		val := value
		if val == "" {
			val = "—"
		}
		maxRunes := paneW - 2
		if maxRunes < 3 {
			maxRunes = 3
		}
		runes := []rune(val)
		if len(runes) > maxRunes {
			val = string(runes[:maxRunes-1]) + "…"
		}
		valLine := textStyle.Width(paneW).Render(" " + val)
		return []string{labelLine, valLine}
	}

	var lines []string
	lines = append(lines, accentStyle.Width(paneW).Render(" Track Info"))
	lines = append(lines, "")
	lines = append(lines, row("TITLE", song.Title)...)
	lines = append(lines, row("ARTIST", song.Artist)...)
	lines = append(lines, row("ALBUM", song.Album)...)
	lines = append(lines, row("YEAR", year)...)
	lines = append(lines, row("DURATION", fmtMusicDuration(song.Duration))...)
	lines = append(lines, row("FILE", song.File)...)

	lines = append(lines, "")
	lines = append(lines,
		dimStyle.Render(truncate(" plugin metadata coming soon", innerW)))

	if len(lines) > listH {
		lines = lines[:listH]
	}
	for len(lines) < listH {
		lines = append(lines, "")
	}
	for i, ln := range lines {
		vis := lipgloss.Width(ln)
		if vis < paneW {
			lines[i] = ln + strings.Repeat(" ", paneW-vis)
		} else if vis > paneW {
			lines[i] = lipgloss.NewStyle().MaxWidth(paneW).Render(ln)
		}
	}
	return lines
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

// yearRegex finds the first 19xx or 20xx four-digit year anywhere in a string.
var yearRegex = regexp.MustCompile(`(?:19|20)\d{2}`)

// extractYear pulls a 4-digit year out of an arbitrary date string.
func extractYear(s string) string {
	return yearRegex.FindString(s)
}
