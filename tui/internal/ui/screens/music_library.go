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
	"github.com/stui/stui/pkg/log"
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
// The dialog is the default action surface for Enter and right-click on
// every pane (artist/album/track) — h/l/arrows still navigate as a
// power-user shortcut, but the dialog is what the UI advertises.
type libDialogCtx int

const (
	libDialogEnter      libDialogCtx = iota // Track Enter — Add / Replace / Cancel
	libDialogRightClick                     // Track right-click — Add / Replace / Add to Playlist / Create Playlist / Cancel
	libDialogArtist                         // Artist Enter or right-click — Browse / Add all / Replace with all / Cancel
	libDialogAlbum                          // Album Enter or right-click — Browse / Add / Replace / Add to Playlist / Normalize / Cancel
	libDialogNormalizeScope                 // Scope picker: This album / This artist / Whole library / Cancel
	libDialogNormalizeConfirm               // Preview confirm: Apply / Cancel
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
			s.albumCursor = 0
			s.albumScroll = 0
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

	case tea.KeyPressMsg:
		// Playlist name prompt intercepts all keys when active.
		if s.playlistPrompt {
			return s.handlePlaylistPrompt(m)
		}
		if s.dirMode {
			s = s.handleDirKey(m.String())
		} else {
			s = s.handleTagKey(m.String())
		}
	}

	// Forward a freshly-set status message to the global stui footer
	// exactly once. statusMsg itself is kept so the local hintBar can echo
	// it for statusTTL; the pending flag is what prevents re-emission on
	// every subsequent Update.
	if s.statusPending {
		s.statusPending = false
		text := s.statusMsg
		return s, func() tea.Msg { return ipc.StatusMsg{Text: text} }
	}
	return s, nil
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
					a := s.albums[s.albumCursor]
					s.client.MpdListSongs(s.artists[s.artistCursor].Name, a.MpdTitle(), a.Date)
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
					a := s.albums[s.albumCursor]
					s.client.MpdListSongs(s.artists[s.artistCursor].Name, a.MpdTitle(), a.Date)
				}
			}
		case LibPaneTracks:
			if s.songCursor > 0 {
				s.songCursor--
			}
		}

	case "l", "right":
		// Hotkey shortcut: skip the dialog and dive straight into the next
		// pane. The dialog (Enter) is the advertised default; this is the
		// power-user route.
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
				a := s.albums[s.albumCursor]
				s.client.MpdListSongs(s.artists[s.artistCursor].Name, a.MpdTitle(), a.Date)
			}
		}

	case "enter":
		// Guard: don't open dialogs until the first data has loaded.
		// Prevents stray terminal events during startup from triggering
		// a dialog loop when the saved state lands on tracks.
		if !s.initDone {
			break
		}
		// Default action surface: every pane opens a dialog so the user
		// sees what's actionable instead of having to memorise hotkeys.
		s = s.openPaneDialog()

	case "h", "left":
		switch s.activePane {
		case LibPaneAlbums:
			s.activePane = LibPaneArtists
		case LibPaneTracks:
			s.activePane = LibPaneAlbums
		}

	case "R":
		if s.client != nil {
			s.client.MpdCmd("mpd_update", nil)
			s = s.setStatus("Rescanning MPD library…")
			// Re-fetch artists after a short delay for the scan to start
			s.loadingArtists = true
			s.artists = nil
			s.albums = nil
			s.songs = nil
			s.client.MpdListArtists()
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

	case "x", "X":
		s = s.handleMarkExceptionTag()
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
	switch s.activePane {
	case LibPaneArtists:
		if s.artistCursor < len(s.artists) {
			name := s.artists[s.artistCursor].Name
			s.client.MarkTagException("artist", name)
			s = s.setStatus(fmt.Sprintf("Protected artist: %s", name))
		}
	case LibPaneAlbums:
		if s.albumCursor < len(s.albums) {
			a := s.albums[s.albumCursor]
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
	case LibPaneTracks:
		if s.songCursor < len(s.songs) {
			song := s.songs[s.songCursor]
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
		scroll := libScroll(len(s.artists), s.artistCursor, listH)
		idx := scroll + dataRow
		if idx < 0 || idx >= len(s.artists) {
			return s
		}
		s.artistCursor = idx
		s.activePane = LibPaneArtists
		s = s.openPaneDialog()
		// Right-click on a track gets the extended "Add to Playlist /
		// Create Playlist" set; for artists the Enter set is enough since
		// playlist actions don't apply at the artist level yet.
	case x < 2*paneW+1:
		scroll := libScroll(len(s.albums), s.albumCursor, listH)
		idx := scroll + dataRow
		if idx < 0 || idx >= len(s.albums) {
			return s
		}
		s.albumCursor = idx
		s.activePane = LibPaneAlbums
		s = s.openPaneDialog()
	default:
		scroll := libScroll(len(s.songs), s.songCursor, listH)
		idx := scroll + dataRow
		if idx < 0 || idx >= len(s.songs) {
			return s
		}
		s.songCursor = idx
		s.activePane = LibPaneTracks
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
	switch s.activePane {
	case LibPaneArtists:
		if len(s.artists) == 0 || s.artistCursor >= len(s.artists) {
			return s
		}
		name := s.artists[s.artistCursor].Name
		s.dialogContext = libDialogArtist
		s.dialog = components.NewDialog(
			"Artist: '"+truncate(name, 28)+"'",
			[]string{"Browse albums", "Add all tracks to queue", "Replace queue with all", "Cancel"},
		)
		s.dialogOpen = true
	case LibPaneAlbums:
		if len(s.albums) == 0 || s.albumCursor >= len(s.albums) {
			return s
		}
		title := s.albums[s.albumCursor].Title
		s.dialogContext = libDialogAlbum
		s.dialog = components.NewDialog(
			"Album: '"+truncate(title, 28)+"'",
			[]string{"Browse tracks", "Add album to queue", "Replace queue with album", "Add to playlist", "Create playlist", "Normalize tags on disk…", "Cancel"},
		)
		s.dialogOpen = true
	case LibPaneTracks:
		if len(s.songs) == 0 || s.songCursor >= len(s.songs) {
			return s
		}
		song := s.songs[s.songCursor]
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
		if s.artistCursor >= len(s.artists) {
			return s
		}
		name := s.artists[s.artistCursor].Name
		switch chosen {
		case 0: // Browse albums — same as the l/right shortcut
			s.activePane = LibPaneAlbums
			s.loadingAlbums = true
			s.albums = nil
			s.songs = nil
			s.albumCursor = 0
			s.albumScroll = 0
			s.client.MpdListAlbums(name)
		case 1: // Add all tracks to queue (needs runtime mpd findadd; not wired yet)
			s = s.setStatus("Add all tracks: not implemented yet")
		case 2: // Replace queue with all (same dependency as above)
			s = s.setStatus("Replace queue with all: not implemented yet")
		}
	case libDialogAlbum:
		if s.artistCursor >= len(s.artists) || s.albumCursor >= len(s.albums) {
			return s
		}
		artist := s.artists[s.artistCursor].Name
		album := s.albums[s.albumCursor]
		switch chosen {
		case 0: // Browse tracks — same as the l/right shortcut
			s.activePane = LibPaneTracks
			s.loadingSongs = true
			s.songs = nil
			s.songCursor = 0
			s.songScroll = 0
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
			a := s.albums[s.albumCursor]
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
			if s.artistCursor < len(s.artists) {
				s.normalizeScope = ipc.TagWriteScope{Kind: "artist", Artist: s.artists[s.artistCursor].Name}
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
				a := s.albums[s.albumCursor]
				s.client.MpdListSongs(s.artists[s.artistCursor].Name, a.MpdTitle(), a.Date)
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

	var base string
	if s.dirMode {
		base = s.viewDir(w, h, accentStyle, dimStyle, textStyle)
	} else {
		base = s.viewTag(w, h, accentStyle, dimStyle, textStyle)
	}
	// Overlay the action dialog, if open. The component knows how to
	// centre itself with the lipgloss-style dotted whitespace fill, so
	// the screen just hands it the available area.
	if s.dialogOpen {
		base = s.dialog.Place(w, h)
	}
	return base
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

	// Total height budget = h. Subtract: 1 header row + 2 border rows
	// (top + bottom of the bordered container). The hint/status text
	// lives in the global footer (see ui.viewStatusBar) so we don't
	// reserve a row for it here.
	listH := h - 3
	if listH < 1 {
		listH = 1
	}

	// Track Info is the optional 4th column. It appears whenever the
	// Tracks pane is focused AND a track is selected — gives the user
	// a metadata sidebar without permanently squeezing the other panes.
	showInfo := s.activePane == LibPaneTracks &&
		s.songCursor < len(s.songs) && len(s.songs) > 0
	cols := 3
	if showInfo {
		cols = 4
	}

	paneW := w / cols
	if paneW < 10 {
		paneW = 10
	}

	borderStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 1)

	var sb strings.Builder

	// Header
	headerStr := s.tagHeader(accentStyle, dimStyle, paneW, showInfo)
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
	var infoLines []string
	if showInfo {
		infoLines = s.buildTrackInfoLines(listH, paneW, accentStyle, dimStyle, textStyle)
	}

	sep := dimStyle.Render("│")
	var paneContent strings.Builder
	for i := 0; i < listH; i++ {
		al := ""
		bl := ""
		sl := ""
		il := ""
		if i < len(artistLines) {
			al = artistLines[i]
		}
		if i < len(albumLines) {
			bl = albumLines[i]
		}
		if i < len(songLines) {
			sl = songLines[i]
		}
		if showInfo {
			if i < len(infoLines) {
				il = infoLines[i]
			}
			paneContent.WriteString(al + sep + bl + sep + sl + sep + il + "\n")
		} else {
			paneContent.WriteString(al + sep + bl + sep + sl + "\n")
		}
	}

	// Wrap in border container. TrimRight the trailing "\n" so lipgloss
	// doesn't add an extra empty content row inside the box.
	body := strings.TrimRight(paneContent.String(), "\n")
	borderedContent := borderStyle.Width(w - 2).Render(body)
	sb.WriteString(borderedContent)

	return sb.String()
}

// FooterText is what the global status bar shows while this screen is
// active. A recent status message wins for statusTTL — long enough for
// the user to read "Added 'X' to queue" — then we fall back to the
// default key-hint string. The screen used to render its own hintBar
// row; that's been collapsed into the global footer so the chrome looks
// the same as Movies/Series.
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

// tagHeader builds the column-header row. The fourth "Track Info" column
// is only included when withInfo is true (i.e. the Tracks pane is focused
// and a track is selected).
func (s MusicLibraryScreen) tagHeader(accentStyle, dimStyle lipgloss.Style, paneW int, withInfo bool) string {
	// buildPaneLines renders each row as "▶ " (or "  ") + label padded to
	// paneW-2, so the label text starts 2 cells into the column. Headers
	// must use the same 2-cell lead-in or they appear shifted left of the
	// data beneath them.
	render := func(label string, active bool) string {
		padded := fmt.Sprintf("  %-*s", paneW-2, label)
		if active {
			return accentStyle.Render(padded)
		}
		return dimStyle.Render(padded)
	}
	sep := dimStyle.Render("│")
	out := render("Artists", s.activePane == LibPaneArtists) +
		sep +
		render("Albums", s.activePane == LibPaneAlbums) +
		sep +
		render("Tracks", s.activePane == LibPaneTracks)
	if withInfo {
		// Track Info is informational; never "active" itself.
		out += sep + render("Track Info", false)
	}
	return out
}

// buildTrackInfoLines renders the right-hand metadata column for the
// currently selected track. Fields shown today are limited to what
// MpdSong carries (Title/Artist/Album/Duration/File). Bitrate, sample
// rate, codec, MusicBrainz/ListenBrainz/Last.fm/Discogs IDs, and play
// count will appear here once the runtime exposes them via extended
// IPC + the corresponding metadata plugins are enabled.
func (s MusicLibraryScreen) buildTrackInfoLines(
	listH, paneW int,
	accentStyle, dimStyle, textStyle lipgloss.Style,
) []string {
	if s.songCursor < 0 || s.songCursor >= len(s.songs) {
		return nil
	}
	song := s.songs[s.songCursor]

	// Year is sourced from the album row currently selected (cursor on
	// album maps to album cursor — same album as the one whose songs are
	// listed). Falls back to empty if the album row has no parsable year.
	year := ""
	if s.albumCursor < len(s.albums) {
		year = extractYear(s.albums[s.albumCursor].Year)
	}

	innerW := paneW - 2 // 1ch padding either side

	row := func(label, value string) []string {
		labelLine := dimStyle.Render(fmt.Sprintf(" %s", label))
		val := value
		if val == "" {
			val = "—"
		}
		// Truncate by visible rune width, not byte count.
		runes := []rune(val)
		maxVal := innerW - 2 // 1 leading space + 1 safety margin
		if maxVal < 1 {
			maxVal = 1
		}
		if len(runes) > maxVal {
			val = string(runes[:maxVal-1]) + "…"
		}
		valLine := textStyle.Render(" " + val)
		return []string{labelLine, valLine}
	}

	var lines []string
	lines = append(lines, accentStyle.Render(" Track Info"))
	lines = append(lines, "")
	lines = append(lines, row("TITLE", song.Title)...)
	lines = append(lines, row("ARTIST", song.Artist)...)
	lines = append(lines, row("ALBUM", song.Album)...)
	lines = append(lines, row("YEAR", year)...)
	lines = append(lines, row("DURATION", fmtMusicDuration(song.Duration))...)
	lines = append(lines, row("FILE", song.File)...)

	// Footer hint about extended metadata pending plugin support.
	lines = append(lines, "")
	lines = append(lines,
		dimStyle.Render(truncate(" plugin metadata coming soon", innerW)))

	// Pad/truncate to listH so adjacent columns line up.
	if len(lines) > listH {
		lines = lines[:listH]
	}
	for len(lines) < listH {
		lines = append(lines, "")
	}
	// Ensure each line is paneW visible chars (so `│` separators line up).
	for i, ln := range lines {
		if vis := lipgloss.Width(ln); vis < paneW {
			lines[i] = ln + strings.Repeat(" ", paneW-vis)
		}
	}
	return lines
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

	// Reserve 1 row: breadcrumb. Hint/status text lives in the global
	// footer (see ui.viewStatusBar), not inline.
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
