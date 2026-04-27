package ui

import (
	"sync/atomic"
	"time"

	"charm.land/bubbles/v2/spinner"
	"charm.land/bubbles/v2/textinput"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/collections"
	"github.com/stui/stui/pkg/config"
	"github.com/stui/stui/pkg/keybinds"
	"github.com/stui/stui/pkg/mediacache"
	"github.com/stui/stui/pkg/notify"
	"github.com/stui/stui/pkg/session"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

// ── Options ───────────────────────────────────────────────────────────────────

type Options struct {
	RuntimePath string
	NoRuntime   bool
	Verbose     bool
	CfgPath     string
}

// ── Binge mode tick ───────────────────────────────────────────────────────────

// bingeTickMsg is sent every second during a binge countdown.
type bingeTickMsg struct{}

// configSaveTickMsg is sent by the debounce timer after a settings change.
// seq must match m.cfgSaveSeq; stale ticks are discarded.
type configSaveTickMsg struct{ seq int }

// searchDebounceFireMsg is sent 150 ms after a live-typing keystroke to
// trigger an incremental search. The token must match m.searchDebounceToken;
// stale ticks (from superseded keystrokes) are discarded without firing.
type searchDebounceFireMsg struct{ token uint64 }

// ── Subtitle / audio sync overlay ─────────────────────────────────────────────

// syncOverlayState tracks which delay overlay is currently visible.
type syncOverlayState struct {
	isAudio bool    // false = subtitle, true = audio
	delay   float64 // seconds (optimistic value)
}

// syncHideMsg is sent after the sync overlay auto-dismiss timer fires.
type syncHideMsg struct{}

const syncOverlayDuration = 2 * time.Second

// ── Screen mode ───────────────────────────────────────────────────────────────

// screenMode is the top-level screen state machine.
type screenMode int

const (
	screenGrid   screenMode = iota // poster grid (default)
	screenList                     // search results list
	screenDetail                   // full-screen detail overlay
)

// mpdElapsedTickMsg fires every second to keep the footer seekbar in sync.
type mpdElapsedTickMsg struct{}

// ── Model ─────────────────────────────────────────────────────────────────────

type Model struct {
	opts    Options
	state   state.AppState
	keys    keybinds.KeyMap
	search  textinput.Model
	client  *ipc.Client
	program *tea.Program
	reqSeq  *atomic.Uint64

	// Loading spinner - animates during loading state
	loadingSpinner spinner.Model

	// Grid
	grids      map[string][]ipc.CatalogEntry
	gridCursor screens.GridCursor

	// gridSearchSnapshot captures the pre-search grid contents per tab so
	// that applyRestoreView can restore the original view on Esc / cleared
	// query. Keys are state.Tab values for grid tabs (Movies/Series/Library).
	// Absence of a key means no search is active for that tab.
	gridSearchSnapshot map[state.Tab][]ipc.CatalogEntry

	// gridSearchActiveQID tracks the most recent query_id per grid tab.
	// Streamed gridScopeAppliedMsg with a mismatched QueryID is considered
	// stale (a newer search was dispatched) and the entries are dropped.
	gridSearchActiveQID map[state.Tab]uint64

	// Top-level screen
	screen screenMode

	// Detail overlay — non-nil only while screenDetail is active
	detail *screens.DetailState

	// NowPlaying — non-nil while mpv is active outside the detail overlay
	// (user navigated away from detail but playback continues)
	nowPlaying *components.NowPlayingState

	// MpdNowPlaying — non-nil while MPD is playing audio
	mpdNowPlaying *components.MpdNowPlayingState

	// DspState — non-nil while DSP is enabled
	dspState *components.DspState

	// Skip detection overlays
	skipIntro   *ipc.SkipSegmentMsg
	skipCredits *ipc.SkipSegmentMsg

	// Toast
	activeToast *components.Toast

	// Audio/subtitle tracks reported by mpv (updated on each file load)
	playerTracks []ipc.TrackInfo

	// Visualizer — drives cava/chroma subprocess while MPD is playing
	visualizer *components.Visualizer

	// Music section — owns all 4 sub-tabs (Browse/Queue/Library/Playlists)
	musicScreen screens.MusicScreen

	// Collections section — owns the Collections tab
	collectionsStore  *collections.Store
	collectionsScreen screens.CollectionsScreen

	// Watch history — tracks playback positions for resume support
	historyStore        *watchhistory.IPCStore
	historyPending      watchhistory.Entry // metadata captured when Play() is dispatched
	historyLastSavedPos float64            // last position flushed to disk (throttle)

	// nowPlayingEntry captures full metadata for the currently playing item so
	// history can be populated even after the detail overlay is closed.
	nowPlayingEntry watchhistory.Entry

	// Continue Watching
	cwCursor  int  // index of selected card in the CW row
	cwFocused bool // true when cursor is in the CW row (not the main grid)

	// Session persistence
	sessionPath      string   // path to session.json (set once in New)
	lastQueueURIs    []string // tracks in the queue as of the last MpdQueueResultMsg
	pendingQueueURIs []string // URIs to restore if MPD queue is empty on first load
	queueRestored    bool     // true once the first queue load has been processed

	// nowPlayingEntryID holds the catalog entry ID of the item currently being
	// played by mpv.  Set when Play() is dispatched; cleared on PlayerEndedMsg.
	nowPlayingEntryID string

	// Binge mode — auto-play next episode when current one finishes.
	bingeCtx       *ipc.BingeContextMsg // non-nil while an episode with binge context is playing
	bingeCountdown int                  // >=0 = countdown active (seconds left); -1 = inactive

	// Subtitle/audio sync overlay — shown briefly after delay is adjusted.
	syncOverlay *syncOverlayState // nil = hidden

	// Terminal VO mode — true while mpv has taken over the terminal for
	// inline video rendering (kitty/sixel/tct/chafa).  When true the TUI has
	// called program.ReleaseTerminal() and must call RestoreTerminal() on
	// PlayerEndedMsg.
	terminalVOActive bool

	// Torrent / aria2 download tracking.
	// downloadOrder preserves insertion order; downloadMap is for O(1) lookup.
	downloadOrder []string // GIDs in arrival order
	downloadMap   map[string]*ipc.DownloadEntry

	// Buffering overlay — non-nil while pre-roll or stall-guard is in progress.
	playerBuffer *ipc.PlayerBufferingMsg

	// Desktop notification config — mirrored from settings, applied on events.
	notifyCfg notify.Config

	// Stream quality quick keys — rank of tier user pressed (0 = none pending).
	// 2=480p  4=720p  5=1080p  7=4K  (qualityRank values from stream_picker.go)
	pendingQuality int

	// Stream Radar — accumulated stream stats for the current session.
	streamStats screens.StreamRadarStats

	// Media cache — persists catalog grid data for offline browsing.
	mediaCache mediacache.StoreInterface

	// Config persistence.
	cfg        config.Config
	cfgPath    string
	cfgSaveSeq int
	watcher    *config.Watcher

	// Live-search debounce — each keystroke increments the token and fires a
	// 150ms tick. The tick handler drops stale tokens so only the last
	// keystroke in a burst actually triggers a StartSearch call.
	searchDebounceToken uint64
}

func New(opts Options, cfg config.Config) Model {
	ti := textinput.New()
	ti.Placeholder = "Search titles, genres, people\u2026"
	ti.SetStyles(textinput.Styles{
		Blurred: textinput.StyleState{
			Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
			Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
			Prompt:      lipgloss.NewStyle().Foreground(theme.T.AccentAlt()),
		},
		Focused: textinput.StyleState{
			Text:        lipgloss.NewStyle().Foreground(theme.T.Text()),
			Placeholder: lipgloss.NewStyle().Foreground(theme.T.TextMuted()),
			Prompt:      lipgloss.NewStyle().Foreground(theme.T.Accent()),
		},
		Cursor: textinput.CursorStyle{
			Color: lipgloss.Color("#7c3aed"),
			Blink: true,
		},
	})
	ti.CharLimit = 120
	ti.SetWidth(40)

	// Load user keybinds from disk and apply them before any UI events fire.
	if kb, err := keybinds.Load(keybinds.DefaultPath()); err == nil && len(kb) > 0 {
		actions.SetUserBindings(kb)
	}

	// Load persisted session (last tab, last music sub-tab, saved queue).
	sessionPath := session.DefaultPath()
	sess := session.Load(sessionPath)

	appState := state.NewAppState()
	if sess.LastTab != "" {
		appState.ActiveTab = state.TabFromString(sess.LastTab)
		// Mirror switchTab's loading-state set so the restored tab
		// shows the loading spinner until GridUpdateMsg arrives —
		// without this the empty-state branch in RenderGrid (no
		// plugins yet, no entries yet) flashes "No metadata
		// sources" before the runtime hands data over.
		if appState.ActiveTab != state.TabCollections {
			appState.IsLoading = true
			appState.LoadingStart = time.Now().Unix()
		}
	}
	// Seed legacy download dirs from the storage config so any downstream
	// reader sees the real library paths instead of the bare default. The
	// download-dir keys are gone from settings; the storage roots act as
	// download targets too.
	if cfg.Storage.Music != "" {
		appState.Settings.MusicDownloadDir = cfg.Storage.Music
	}
	if cfg.Storage.Movies != "" {
		appState.Settings.VideoDownloadDir = cfg.Storage.Movies
	}

	ms := screens.NewMusicScreen(nil)
	if sess.LastMusicSubTab >= 0 && sess.LastMusicSubTab <= 3 {
		ms = ms.WithActiveSubTab(screens.MusicSubTab(sess.LastMusicSubTab))
	}

	collStore := collections.Load(collections.DefaultPath())
	mcStore := mediacache.Load(mediacache.DefaultPath())

	// Pre-seed the grid map from cache so the grid renderer has data
	// even when the runtime hasn't connected yet (or is offline).
	seedGrids := make(map[string][]ipc.CatalogEntry)
	for tab, ct := range mcStore.Tabs {
		seedGrids[tab] = ct.Entries
	}

	// Initialize loading spinner
	loadingSpinner := spinner.New(
		spinner.WithSpinner(spinner.Dot),
		spinner.WithStyle(lipgloss.NewStyle().Foreground(theme.T.TextDim())),
	)

	m := Model{
		opts:                opts,
		state:               appState,
		keys:                keybinds.Default(),
		grids:               seedGrids,
		gridSearchSnapshot:  make(map[state.Tab][]ipc.CatalogEntry),
		gridSearchActiveQID: make(map[state.Tab]uint64),
		search:              ti,
		screen:              screenGrid,
		reqSeq:              new(atomic.Uint64),
		loadingSpinner:      loadingSpinner,
		visualizer: components.NewVisualizer(components.VisualizerConfig{
			Backend:     components.BackendFromString(cfg.Visualizer.Backend),
			Bars:        cfg.Visualizer.Bars,
			Height:      cfg.Visualizer.Height,
			Framerate:   cfg.Visualizer.Framerate,
			Mode:        components.VisualizerModeFromString(cfg.Visualizer.Mode),
			Gradient:    cfg.Visualizer.Gradient,
			PeakHold:    cfg.Visualizer.PeakHold,
			InputMethod: cfg.Visualizer.InputMethod,
		}),
		musicScreen:       ms,
		sessionPath:       sessionPath,
		pendingQueueURIs:  sess.QueueURIs,
		collectionsStore:  collStore,
		collectionsScreen: screens.NewCollectionsScreen(collStore, nil),
		historyStore:      nil,
		bingeCountdown:    -1,
		downloadMap:       make(map[string]*ipc.DownloadEntry),
		notifyCfg:         notify.DefaultConfig(),
		mediaCache:        mcStore,
		cfg:               cfg,
		cfgPath:           opts.CfgPath,
	}
	m.musicScreen.SetVisualizer(m.visualizer)
	return m
}

func (m *Model) SetProgram(p *tea.Program) { m.program = p }

