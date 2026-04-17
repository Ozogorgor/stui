package ui

import (
	"fmt"
	"strings"
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
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/bidi"
	"github.com/stui/stui/pkg/collections"
	"github.com/stui/stui/pkg/config"
	"github.com/stui/stui/pkg/keybinds"
	"github.com/stui/stui/pkg/log"
	"github.com/stui/stui/pkg/mediacache"
	"github.com/stui/stui/pkg/notify"
	"github.com/stui/stui/pkg/session"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

func getIfKey(key, target, value string) *string {
	if key == target {
		return &value
	}
	return nil
}

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

func bingeTickCmd() tea.Cmd {
	return tea.Tick(time.Second, func(time.Time) tea.Msg { return bingeTickMsg{} })
}

// ── Subtitle / audio sync overlay ─────────────────────────────────────────────

// syncOverlayState tracks which delay overlay is currently visible.
type syncOverlayState struct {
	isAudio bool    // false = subtitle, true = audio
	delay   float64 // seconds (optimistic value)
}

// syncHideMsg is sent after the sync overlay auto-dismiss timer fires.
type syncHideMsg struct{}

const syncOverlayDuration = 2 * time.Second

func syncHideCmd() tea.Cmd {
	return tea.Tick(syncOverlayDuration, func(time.Time) tea.Msg { return syncHideMsg{} })
}

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

func mpdElapsedTickCmd() tea.Cmd {
	return tea.Tick(time.Second, func(time.Time) tea.Msg { return mpdElapsedTickMsg{} })
}

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

	// Pre-seed the grid map from cache so fetchSimilar and the grid renderer
	// have data even when the runtime hasn't connected yet (or is offline).
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
		opts:              opts,
		state:             appState,
		keys:              keybinds.Default(),
		grids:             seedGrids,
		search:            ti,
		screen:            screenGrid,
		reqSeq:            new(atomic.Uint64),
		loadingSpinner:    loadingSpinner,
		visualizer:        components.NewVisualizer(components.VisualizerConfig{
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

// ── Init ──────────────────────────────────────────────────────────────────────

func (m Model) Init() tea.Cmd {
	if m.opts.NoRuntime {
		return tea.Batch(m.loadingSpinner.Tick, func() tea.Msg { return ipc.RuntimeReadyMsg{} })
	}

	return tea.Batch(m.loadingSpinner.Tick, func() tea.Msg {
		client, err := ipc.Start(m.opts.RuntimePath)
		if err != nil {
			return ipc.RuntimeErrorMsg{Err: err}
		}
		return runtimeStartedMsg{client: client}
	})
}

// fromIPC wraps a message that arrived via the IPC channel so that the
// Update switch can re-subscribe listenIPC in a single place.
type fromIPC struct{ tea.Msg }

// listenIPC returns a Cmd that blocks on the IPC message channel and
// delivers the next message as a fromIPC wrapper.  Update re-subscribes
// by returning another listenIPC after processing each message.
func listenIPC(ch <-chan tea.Msg) tea.Cmd {
	log.Info("ui: listenIPC cmd built (not yet running)")
	return func() tea.Msg {
		log.Info("ui: listenIPC cmd RUNNING — waiting for message")
		msg, ok := <-ch
		if !ok {
			log.Warn("ui: listenIPC channel closed")
			return fromIPC{ipc.RuntimeErrorMsg{Err: fmt.Errorf("IPC channel closed")}}
		}
		log.Info("ui: listenIPC got message", "type", fmt.Sprintf("%T", msg))
		return fromIPC{msg}
	}
}

type runtimeStartedMsg struct{ client *ipc.Client }

// ── Update ────────────────────────────────────────────────────────────────────

func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {

	// fromIPC unwraps a message from the IPC channel, re-subscribes the
	// listener, then dispatches the inner message through Update as normal.
	case fromIPC:
		log.Info("ui: fromIPC received", "type", fmt.Sprintf("%T", msg.Msg))
		updated, cmd := m.Update(msg.Msg)
		newModel, ok := updated.(Model)
		if !ok {
			log.Warn("ui: fromIPC dispatch — updated is not Model", "type", fmt.Sprintf("%T", updated))
			return m, cmd
		}
		m = newModel
		if m.client != nil {
			return m, tea.Batch(cmd, listenIPC(m.client.Chan()))
		}
		log.Warn("ui: fromIPC dispatch — m.client is nil, not re-subscribing listenIPC")
		return m, cmd

	case tea.WindowSizeMsg:
		m.state.Width = msg.Width
		m.state.Height = msg.Height
		m.search.SetWidth(max(20, m.innerWidth()/3))
		innerMsg := tea.WindowSizeMsg{Width: m.innerWidth(), Height: m.computeMusicHeight()}
		m.musicScreen, _ = m.musicScreen.Update(innerMsg)
		m.collectionsScreen = m.collectionsScreen.SetSize(m.innerWidth(), max(0, msg.Height-12))

	// ── Runtime lifecycle ─────────────────────────────────────────────────

	case runtimeStartedMsg:
		log.Info("ui: runtimeStartedMsg received")
		m.client = msg.client
		m.state.RuntimeStatus = state.RuntimeReady
		m.state.StatusMsg = "Loading catalog…"
		m.state.RuntimeVersion = msg.client.RuntimeVersion
		if m.opts.Verbose {
			m.client.SetTrace(true)
		}
		m.client.ListPlugins()
		// Trigger an MPD database update on connect so the library reflects
		// any music files added since the runtime last scanned.
		m.client.MpdCmd("mpd_update", nil)
		var musicInitCmd tea.Cmd
		m.musicScreen, musicInitCmd = m.musicScreen.SetClient(m.client)
		m.historyStore = watchhistory.NewIPCStore(msg.client)
		m.historyStore.Load()
		m.collectionsScreen = screens.NewCollectionsScreen(m.collectionsStore, m.historyStore)
		m.mediaCache = mediacache.NewIPCStore(msg.client)
		for tab := range m.grids {
			m.mediaCache.SaveTab(tab, m.grids[tab])
		}
		m.client.GetDspStatus()
		// Start spinner tick if still loading
		if m.state.IsLoading {
			return m, tea.Batch(m.loadingSpinner.Tick, musicInitCmd, listenIPC(m.client.Chan()))
		}
		return m, tea.Batch(musicInitCmd, listenIPC(m.client.Chan()))

	case ipc.RuntimeReadyMsg:
		m.state.RuntimeStatus = state.RuntimeReady
		m.state.StatusMsg = "Ready (dev mode)"

	case ipc.RuntimeErrorMsg:
		m.state.RuntimeStatus = state.RuntimeError
		offlineHint := ""
		if m.mediaCache != nil && m.mediaCache.TotalCount() > 0 {
			offlineHint = fmt.Sprintf(" — press O for offline library (%d cached)", m.mediaCache.TotalCount())
		}
		m.state.StatusMsg = fmt.Sprintf("Runtime error: %v%s", msg.Err, offlineHint)

	case ipc.IPCVersionMismatchMsg:
		m.state.StatusMsg = fmt.Sprintf(
			"⚠ IPC version mismatch: TUI=%d runtime=%d (v%s) — consider upgrading",
			msg.TUIVersion, msg.RuntimeVersion, msg.RuntimeSemver,
		)

	case ipc.StatusMsg:
		m.state.StatusMsg = msg.Text

	// ── Plugin events ─────────────────────────────────────────────────────

	case ipc.PluginLoadedMsg:
		if msg.Err != nil {
			m.state.StatusMsg = fmt.Sprintf("Plugin load failed: %v", msg.Err)
		} else {
			m.state.StatusMsg = fmt.Sprintf("Plugin loaded: %s", msg.Name)
		}

	case ipc.PluginListMsg:
		if msg.Err == nil {
			m.state.Plugins = make([]string, 0, len(msg.Plugins))
			for _, p := range msg.Plugins {
				m.state.Plugins = append(m.state.Plugins, p.Name)
			}
		}

	case ipc.PluginToastMsg:
		t, cmd := components.ShowToast(msg.Message, msg.IsError)
		m.activeToast = &t
		if msg.IsError {
			m.state.StatusMsg = "Plugin error: " + msg.PluginName
		} else {
			m.state.StatusMsg = "Plugin loaded: " + msg.PluginName + " v" + msg.Version
			m.state.Plugins = append(m.state.Plugins, msg.PluginName)
		}
		return m, cmd

	case components.ToastDismissMsg:
		m.activeToast = nil

	// ── Catalog grid ──────────────────────────────────────────────────────

	case ipc.GridUpdateMsg:
		m.grids[msg.Tab] = msg.Entries
		m.musicScreen, _ = m.musicScreen.Update(msg) // keep Browse catalog fresh
		if msg.Tab == m.state.ActiveTab.MediaTabID() {
			m.state.IsLoading = false
			m.state.LoadingStart = 0
			if msg.Source == "cache" {
				m.state.StatusMsg = fmt.Sprintf("Loaded %d titles from cache \u2014 refreshing\u2026", len(msg.Entries))
			} else {
				m.state.StatusMsg = fmt.Sprintf("%d titles", len(msg.Entries))
			}
		}
		// Persist live catalog data for offline browsing.
		if msg.Source == "live" && m.mediaCache != nil {
			m.mediaCache.SaveTab(msg.Tab, msg.Entries)
		}

	// ── Search results ────────────────────────────────────────────────────

	case ipc.SearchResultMsg:
		m.state.IsLoading = false
		m.state.LoadingStart = 0
		if msg.Err != nil {
			m.state.StatusMsg = fmt.Sprintf("Search error: %v", msg.Err)
			return m, nil
		}
		// Person mode search feeds into the detail overlay
		if m.detail != nil && m.detail.PersonMode {
			m.detail.PersonResults = convertSearchToCatalog(msg.Result.Items)
			m.detail.PersonLoading = false
			m.detail.PersonCursor = screens.GridCursor{}
			return m, nil
		}
		// Normal search → list screen
		m.state.Results = convertResults(msg.Result.Items)
		m.state.Cursor = 0
		m.screen = screenList
		m.state.StatusMsg = fmt.Sprintf("%d results for \u201c%s\u201d", msg.Result.Total, m.state.SearchQuery)

	// ── Screen-stack messages ───────────────────────────────────────────

	case ipc.SearchResultSelectedMsg:
		// User picked a result from SearchScreen — convert to CatalogEntry and open detail
		e := msg.Entry
		cat := ipc.CatalogEntry{
			ID:          e.ID,
			Title:       e.Title,
			Year:        e.Year,
			Genre:       e.Genre,
			Rating:      e.Rating,
			Description: e.Description,
			PosterURL:   e.PosterURL,
			Provider:    e.Provider,
			Tab:         string(e.Tab),
		}
		return m, m.openDetail(cat)

	case ipc.EpisodesLoadedMsg:
		// Episode data arrived — forwarded automatically to EpisodeScreen via RootModel
		return m, nil

	case ipc.BingeContextMsg:
		// Store binge context whenever an episode is played from EpisodeScreen.
		// Countdown only fires if BingeEnabled is true (toggled with 'b' in EpisodeScreen).
		m.bingeCtx = &msg
		m.bingeCountdown = -1
		return m, nil

	case ipc.StreamsResolvedMsg:
		// Accumulate into session-wide radar stats.
		m.streamStats.AddBatch(msg.Streams)
		if m.notifyCfg.OnStreams && len(msg.Streams) > 0 {
			body := fmt.Sprintf("%d stream(s) found", len(msg.Streams))
			notify.Send(m.notifyCfg, "✓ Streams Resolved", body, notify.UrgencyLow)
		}
		// Quality quick key auto-pick: fire when a pending tier matches this entry.
		if m.detail != nil && msg.EntryID == m.detail.Entry.ID && m.pendingQuality != 0 {
			rank := m.pendingQuality
			m.pendingQuality = 0
			qualLabel := map[int]string{2: "480p", 4: "720p", 5: "1080p", 7: "4K"}
			if best := screens.BestStreamForTier(msg.Streams, rank); best != nil && m.client != nil {
				m.client.SwitchStream(best.URL)
			} else {
				t, cmd := components.ShowToast("No "+qualLabel[rank]+" streams available", false)
				m.activeToast = &t
				return m, cmd
			}
		}
		return m, nil

	// ── Collections ───────────────────────────────────────────────────────

	case screens.CollectionOpenDetailMsg:
		return m, m.openDetail(msg.Entry)

	// ── Detail data ───────────────────────────────────────────────────────

	case ipc.DetailReadyMsg:
		if m.detail == nil {
			return m, nil
		}
		if msg.Err != nil {
			m.detail.Loading = false
			m.state.StatusMsg = fmt.Sprintf("Detail error: %v", msg.Err)
			return m, nil
		}
		m.detail.Entry = msg.Entry
		m.detail.Loading = false
		m.state.StatusMsg = msg.Entry.Title
		return m, m.fetchSimilar(msg.Entry)

	// ── Live theme update from matugen watcher ───────────────────────────
	case ipc.ThemeUpdateMsg:
		palette := theme.FromMatugen(msg.Colors)
		theme.T.Apply(palette)
		m.state.StatusMsg = "Theme updated from matugen"
		return m, func() tea.Msg { return nil }

	// ── Config file reload (hot-reload from watcher) ──────────────────────
	case config.ConfigReloadMsg:
		m.cfg = msg.Config
		if msg.Config.Interface.Theme != "matugen" {
			if p, err := config.LoadTheme(msg.Config.Interface.Theme); err == nil {
				theme.T.Apply(p)
			}
		}
		if m.watcher != nil {
			m.watcher.SetActiveTheme(msg.Config.Interface.Theme)
		}
		return m, nil

	// ── Visualizer ────────────────────────────────────────────────────────

	case components.VisualizerTickMsg:
		// Keep the animation loop alive while the visualizer is running
		if m.visualizer.IsRunning() {
			return m, m.visualizer.TickCmd()
		}

	case components.VisualizerErrMsg:
		t, cmd := components.ShowToast("Visualizer error: "+msg.Err.Error(), true)
		m.activeToast = &t
		return m, cmd

	// ── Player events ─────────────────────────────────────────────────────
	case ipc.PlayerTracksUpdatedMsg:
		m.playerTracks = msg.Tracks

	case ipc.PlayerStartedMsg:
		np := components.NewNowPlaying(msg)
		if m.detail != nil {
			m.detail.NowPlaying = np
		} else {
			m.nowPlaying = np
		}
		m.state.CurrentStream = state.CurrentStream{
			URL:      msg.Path,
			Title:    msg.Title,
			Provider: m.state.CurrentMedia.Provider,
			Duration: msg.Duration,
		}
		m.state.StatusMsg = "\u25b6 Playing: " + msg.Title
		if m.notifyCfg.OnPlayback {
			notify.Send(m.notifyCfg, "▶ Now Playing", msg.Title, notify.UrgencyLow)
		}

	case ipc.PlayerBufferingMsg:
		m.playerBuffer = &msg
		// Pre-roll or stall-guard — update/create NowPlaying in buffering state
		np := m.activeNowPlaying()
		if np == nil {
			np = &components.NowPlayingState{Buffering: true}
			if m.detail != nil {
				m.detail.NowPlaying = np
			} else {
				m.nowPlaying = np
			}
		}
		np.Buffering = true
		np.BufferReason = msg.Reason
		np.BufferFill = msg.FillPercent
		np.BufferSpeedMbps = msg.SpeedMbps
		np.BufferPreRoll = msg.PreRollSecs
		np.BufferEta = msg.EtaSecs
		if msg.Reason == "stall_guard" {
			m.state.StatusMsg = fmt.Sprintf("\u23f8 Buffering\u2026 %.0f%%  %.1f MB/s", msg.FillPercent, msg.SpeedMbps)
		} else {
			m.state.StatusMsg = fmt.Sprintf("\u23f3 Pre-roll %.0f%%  %.1f MB/s  ETA %.0fs", msg.FillPercent, msg.SpeedMbps, msg.EtaSecs)
		}

	case ipc.PlayerBufferReadyMsg:
		m.playerBuffer = nil
		np := m.activeNowPlaying()
		if np != nil {
			np.Buffering = false
		}
		m.state.StatusMsg = fmt.Sprintf("\u25b6 Ready \u2014 %.0fs buffered  slack %.2f\u00d7  %.1f MB/s",
			msg.PreRollSecs, msg.Slack, msg.SpeedMbps)

	case ipc.PlayerProgressMsg:
		if m.detail != nil && m.detail.NowPlaying != nil {
			m.detail.NowPlaying.Update(msg)
		} else if m.nowPlaying != nil {
			m.nowPlaying.Update(msg)
		}
		m.state.CurrentStream.Position = msg.Position
		if msg.Duration > 0 {
			m.state.CurrentStream.Duration = msg.Duration
		}
		// Auto-skip intro if enabled and position enters intro zone
		if m.skipIntro != nil && m.state.Settings.AutoSkipIntro && m.client != nil {
			if msg.Position >= m.skipIntro.Start && msg.Position < m.skipIntro.End {
				m.client.PlayerCommand("seek", m.skipIntro.End)
				m.skipIntro = nil
			}
		}
		// Persist watch position — throttled: save only when position
		// has moved by at least 5 seconds since the last disk write.
		if m.historyStore != nil && m.nowPlayingEntryID != "" &&
			msg.Position-m.historyLastSavedPos >= 5 {
			m.historyLastSavedPos = msg.Position
			entry := m.nowPlayingEntry
			entry.Position = msg.Position
			if msg.Duration > 0 {
				entry.Duration = msg.Duration
			}
			if m.historyStore != nil {
				m.historyStore.Upsert(entry)
			}
		}

	case ipc.PlayerTerminalTakeoverMsg:
		// mpv is about to render video inline — release the terminal so it can
		// write to stdout directly.
		if m.program != nil {
			_ = m.program.ReleaseTerminal()
		}
		m.terminalVOActive = true

	case ipc.PlayerEndedMsg:
		// If we handed off the terminal for inline video rendering, take it back.
		if m.terminalVOActive && m.program != nil {
			_ = m.program.RestoreTerminal()
			m.terminalVOActive = false
		}
		if m.detail != nil {
			m.detail.NowPlaying = nil
		}
		m.nowPlaying = nil
		m.skipIntro = nil
		m.skipCredits = nil
		m.playerBuffer = nil
		if msg.Reason == "error" {
			m.state.StatusMsg = "Playback error: " + msg.Error
		} else {
			m.state.StatusMsg = "Playback finished"
		}
		// Auto-delete: only on natural end-of-file completion, not on manual
		// quit or error.  Audio auto-delete uses the same path — the runtime
		// decides which files belong to the entry.
		if msg.Reason == "eof" && m.nowPlayingEntryID != "" && m.client != nil {
			isAudio := m.state.ActiveTab == state.TabMusic
			if (!isAudio && m.state.Settings.AutoDeleteVideo) || (isAudio && m.state.Settings.AutoDeleteAudio) {
				m.client.DeleteStream(m.nowPlayingEntryID)
			}
			// Mark watch history as completed on natural end-of-file.
			if m.historyStore != nil {
				m.historyStore.MarkCompleted(m.nowPlayingEntryID)
			}
		}
		m.nowPlayingEntryID = ""
		m.historyLastSavedPos = 0
		m.state.CurrentStream = state.CurrentStream{}
		if m.detail == nil {
			m.state.CurrentMedia = state.CurrentMedia{}
		}

		// Binge mode: start countdown if we have a next episode.
		if msg.Reason == "eof" && m.bingeCtx != nil && m.bingeCtx.BingeEnabled {
			if m.bingeCtx.CurrentIdx+1 < len(m.bingeCtx.Episodes) {
				countdown := m.state.Settings.AutoplayCountdown
				if countdown <= 0 {
					countdown = 5
				}
				m.bingeCountdown = countdown
				return m, bingeTickCmd()
			}
			// Last episode of the season — clear context.
			m.bingeCtx = nil
		}

	case syncHideMsg:
		m.syncOverlay = nil

	// ── Torrent download events ────────────────────────────────────────────

	case ipc.DownloadStartedMsg:
		if _, exists := m.downloadMap[msg.GID]; !exists {
			m.downloadOrder = append(m.downloadOrder, msg.GID)
		}
		title := msg.Title
		if title == "" {
			title = msg.URI
		}
		m.downloadMap[msg.GID] = &ipc.DownloadEntry{
			GID:    msg.GID,
			Title:  title,
			Status: "active",
		}

	case ipc.DownloadProgressMsg:
		if e, ok := m.downloadMap[msg.GID]; ok {
			e.Progress = msg.Progress
			e.Speed = msg.Speed
			e.ETA = msg.ETA
			e.Seeders = msg.Seeders
		}

	case ipc.DownloadCompleteMsg:
		if e, ok := m.downloadMap[msg.GID]; ok {
			e.Status = "complete"
			e.Progress = 1.0
			e.Files = msg.Files
			e.Speed = ""
			e.ETA = ""
			if m.notifyCfg.OnDownload {
				title := e.Title
				if title == "" {
					title = msg.GID
				}
				notify.Send(m.notifyCfg, "✓ Download Complete", title, notify.UrgencyNormal)
			}
		}

	case ipc.DownloadErrorMsg:
		if e, ok := m.downloadMap[msg.GID]; ok {
			e.Status = "error"
			e.Error = msg.Message
		}

	case bingeTickMsg:
		if m.bingeCountdown > 0 {
			m.bingeCountdown--
			if m.bingeCountdown == 0 {
				return m, m.playBingeNext()
			}
			return m, bingeTickCmd()
		}

	case configSaveTickMsg:
		if msg.seq != m.cfgSaveSeq {
			return m, nil
		}
		if m.watcher != nil {
			m.watcher.NotifyWrite()
		}
		_ = config.Save(m.cfgPath, m.cfg)
		return m, nil

	case spinner.TickMsg:
		// Tick BOTH the top-level loading spinner (Movies/Series grid) and
		// every music sub-screen — otherwise spinners inside MusicScreen
		// (library, playlists, etc.) never advance because their TickMsg
		// chain dies here.
		var spinCmd tea.Cmd
		m.loadingSpinner, spinCmd = m.loadingSpinner.Update(msg)
		var musicCmd tea.Cmd
		m.musicScreen, musicCmd = m.musicScreen.Update(msg)
		// Clear stale loading state if nothing responded within the timeout.
		if m.state.IsLoading && m.state.LoadingStart > 0 &&
			time.Since(time.Unix(m.state.LoadingStart, 0)) > 8*time.Second {
			m.state.IsLoading = false
			m.state.LoadingStart = 0
		}
		return m, tea.Batch(spinCmd, musicCmd)

	// ── MPD audio events ──────────────────────────────────────────────────

	case mpdElapsedTickMsg:
		if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
			m.mpdNowPlaying.Elapsed += 1.0
			if m.mpdNowPlaying.Duration > 0 && m.mpdNowPlaying.Elapsed > m.mpdNowPlaying.Duration {
				m.mpdNowPlaying.Elapsed = m.mpdNowPlaying.Duration
			}
			return m, mpdElapsedTickCmd()
		}

	case ipc.MpdStatusMsg:
		m.musicScreen, _ = m.musicScreen.Update(msg) // keep queue highlight in sync
		if msg.State == "stop" && (m.mpdNowPlaying == nil || m.mpdNowPlaying.State == "stop") {
			// Already stopped — skip unnecessary alloc
			break
		}
		wasPlaying := m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play"
		if m.mpdNowPlaying == nil {
			m.mpdNowPlaying = &components.MpdNowPlayingState{}
		}
		m.mpdNowPlaying.Update(msg)
		if msg.State == "stop" && msg.QueueLength == 0 {
			m.mpdNowPlaying = nil
			m.visualizer.Stop()
		} else if msg.State == "play" {
			var cmds []tea.Cmd
			if !wasPlaying {
				cmds = append(cmds, mpdElapsedTickCmd())
			}
			if !m.visualizer.IsRunning() && m.visualizer.Config().Backend != components.VisualizerOff {
				if err := m.visualizer.Start(); err == nil {
					cmds = append(cmds, m.visualizer.TickCmd())
				}
			}
			if len(cmds) > 0 {
				return m, tea.Batch(cmds...)
			}
		}

	case ipc.MpdOutputsResultMsg:
		if msg.Err != nil {
			m.state.StatusMsg = "MPD outputs error: " + msg.Err.Error()
		}
		// Outputs are displayed in a future MPD outputs overlay screen.

	// ── DSP events ────────────────────────────────────────────────────────

	case ipc.DspStatusMsg:
		if m.dspState == nil {
			m.dspState = &components.DspState{}
		}
		m.dspState.Update(msg)

	case ipc.DspBoundToMpdMsg:
		if msg.Success {
			m.state.StatusMsg = "DSP bound to MPD"
		} else {
			m.state.StatusMsg = "DSP bind failed"
		}

	case ipc.SimilarReadyMsg:
		if m.detail != nil && msg.ForID == m.detail.Entry.ID {
			if msg.Err == nil {
				m.detail.Similar = msg.Entries
			}
			m.detail.SimilarLoading = false
		}

	// ── Visualizer cycle hotkeys (from queue screen) ────────────────────

	case screens.VizCycleBackendMsg:
		cfg := m.visualizer.Config()
		// off → cliamp → cava → chroma → off
		next := (cfg.Backend + 1) % 4
		cfg.Backend = next
		backendName := "off"
		switch next {
		case components.VisualizerCliamp:
			backendName = "cliamp"
		case components.VisualizerCava:
			backendName = "cava"
		case components.VisualizerChroma:
			backendName = "chroma"
		}
		m.cfg.Visualizer.Backend = backendName
		if err := config.Save(m.cfgPath, m.cfg); err != nil {
			log.Warn("config save failed", "key", "visualizer.backend", "error", err)
		}
		return m, m.visualizer.Reconfigure(cfg)

	case screens.VizCycleModeMsg:
		cfg := m.visualizer.Config()
		// Modes are iota 0..N; cycle using the string map's length.
		const numModes = 21 // Bars..Bricks (must match visualizer.go enum)
		cfg.Mode = components.VisualizerMode((int(cfg.Mode) + 1) % numModes)
		m.cfg.Visualizer.Mode = cfg.Mode.String()
		if err := config.Save(m.cfgPath, m.cfg); err != nil {
			log.Warn("config save failed", "key", "visualizer.mode", "error", err)
		}
		return m, m.visualizer.Reconfigure(cfg)

	// ── Settings changes ─────────────────────────────────────────────────

	case screens.SettingsChangedMsg:
		// Visualizer settings are TUI-local — intercept before runtime IPC
		if strings.HasPrefix(msg.Key, "visualizer.") {
			cfg := m.visualizer.Config()
			switch msg.Key {
			case "visualizer.backend":
				if v, ok := msg.Value.(string); ok {
					cfg.Backend = components.BackendFromString(v)
					m.cfg.Visualizer.Backend = v
				}
			case "visualizer.bars":
				if v, ok := msg.Value.(int); ok {
					cfg.Bars = v
					m.cfg.Visualizer.Bars = v
				}
			case "visualizer.height":
				if v, ok := msg.Value.(int); ok {
					cfg.Height = v
					m.cfg.Visualizer.Height = v
				}
			case "visualizer.framerate":
				if v, ok := msg.Value.(int); ok {
					cfg.Framerate = v
					m.cfg.Visualizer.Framerate = v
				}
			case "visualizer.mode":
				if v, ok := msg.Value.(string); ok {
					cfg.Mode = components.VisualizerModeFromString(v)
					m.cfg.Visualizer.Mode = v
				}
			case "visualizer.peak_hold":
				if v, ok := msg.Value.(bool); ok {
					cfg.PeakHold = v
					m.cfg.Visualizer.PeakHold = v
				}
			case "visualizer.gradient":
				if v, ok := msg.Value.(bool); ok {
					cfg.Gradient = v
					m.cfg.Visualizer.Gradient = v
				}
			case "visualizer.input_method":
				if v, ok := msg.Value.(string); ok {
					cfg.InputMethod = v
					m.cfg.Visualizer.InputMethod = v
				}
			}
			return m, m.visualizer.Reconfigure(cfg)
		}
		if m.client != nil {
			// Handle storage path changes via SetStoragePaths
			switch {
			case strings.HasPrefix(msg.Key, "storage."):
				if v, ok := msg.Value.(string); ok {
					m.client.SetStoragePaths(ipc.SetStoragePathsRequest{
						Movies:   getIfKey(msg.Key, "storage.movies", v),
						Series:   getIfKey(msg.Key, "storage.series", v),
						Anime:    getIfKey(msg.Key, "storage.anime", v),
						Music:    getIfKey(msg.Key, "storage.music", v),
						Podcasts: getIfKey(msg.Key, "storage.podcasts", v),
					})
					// Keep cfg.Storage in sync locally too so subsequent
					// reads see the new value without a round-trip.
					switch msg.Key {
					case "storage.movies":
						m.cfg.Storage.Movies = v
						// Use the movies dir as the legacy video download
						// target so downloads land in the library root.
						m.state.Settings.VideoDownloadDir = v
						m.cfg.Downloads.VideoDir = v
					case "storage.series":
						m.cfg.Storage.Series = v
					case "storage.anime":
						m.cfg.Storage.Anime = v
					case "storage.music":
						m.cfg.Storage.Music = v
						// Mirror onto the legacy MusicDownloadDir slot so
						// downstream code (status messages, downloads logic)
						// picks up the new path without a separate setting.
						m.state.Settings.MusicDownloadDir = v
						m.cfg.Downloads.MusicDir = v
						// When the primary music folder changes, ask MPD
						// to rescan and reload the artist list.
						m.client.MpdCmd("mpd_update", nil)
						m.client.MpdListArtists()
					case "storage.podcasts":
						m.cfg.Storage.Podcasts = v
					}
				}
			default:
				m.client.SetConfig(msg.Key, msg.Value)
			}
		}
		// Mirror local-state flags immediately
		switch msg.Key {
		case "skipper.auto_skip_intro":
			if v, ok := msg.Value.(bool); ok {
				m.state.Settings.AutoSkipIntro = v
			}
		case "skipper.auto_skip_credits":
			if v, ok := msg.Value.(bool); ok {
				m.state.Settings.AutoSkipCredits = v
			}
		case "ui.bidi_mode":
			if v, ok := msg.Value.(string); ok {
				bidi.SetMode(bidi.Mode(v))
			}
		case "ui.color_scheme":
			if v, ok := msg.Value.(string); ok {
				switch v {
				case "high-contrast":
					theme.T.Apply(theme.HighContrast())
				case "monochrome":
					theme.T.Apply(theme.Monochrome())
				default:
					theme.T.Apply(theme.Default())
				}
			}
		case "ui.reduced_motion":
			if v, ok := msg.Value.(bool); ok {
				components.SetReducedMotion(v)
			}
		case "streaming.auto_delete_video":
			if v, ok := msg.Value.(bool); ok {
				m.state.Settings.AutoDeleteVideo = v
			}
		case "streaming.auto_delete_audio":
			if v, ok := msg.Value.(bool); ok {
				m.state.Settings.AutoDeleteAudio = v
			}
		case "streaming.benchmark_streams":
			if v, ok := msg.Value.(bool); ok {
				m.state.Settings.BenchmarkStreams = v
			}
		case "playback.autoplay_next":
			if v, ok := msg.Value.(bool); ok {
				m.state.Settings.AutoplayNext = v
			}
		case "playback.autoplay_countdown":
			if v, ok := msg.Value.(int); ok {
				m.state.Settings.AutoplayCountdown = v
			}
		case "notifications.enabled":
			if v, ok := msg.Value.(bool); ok {
				m.notifyCfg.Enabled = v
			}
		case "notifications.backend":
			if v, ok := msg.Value.(string); ok {
				m.notifyCfg.Backend = v
			}
		case "notifications.on_playback":
			if v, ok := msg.Value.(bool); ok {
				m.notifyCfg.OnPlayback = v
			}
		case "notifications.on_download":
			if v, ok := msg.Value.(bool); ok {
				m.notifyCfg.OnDownload = v
			}
		case "notifications.on_streams":
			if v, ok := msg.Value.(bool); ok {
				m.notifyCfg.OnStreams = v
			}
		case "app.debug_mode":
			if v, ok := msg.Value.(bool); ok && m.client != nil {
				m.client.SetTrace(v)
			}
		}
		// Persist to config file. Save synchronously instead of debouncing
		// so a quick quit can't drop the change.
		m.cfg = config.ApplyChange(m.cfg, msg.Key, msg.Value)
		if msg.Key == "interface.theme" {
			if p, err := config.LoadTheme(m.cfg.Interface.Theme); err == nil {
				theme.T.Apply(p)
			}
			if m.watcher != nil {
				m.watcher.SetActiveTheme(m.cfg.Interface.Theme)
			}
		}
		if m.watcher != nil {
			m.watcher.NotifyWrite()
		}
		if err := config.Save(m.cfgPath, m.cfg); err != nil {
			log.Warn("config save failed", "key", msg.Key, "path", m.cfgPath, "error", err)
			m.state.StatusMsg = "config save failed: " + err.Error()
		}
		return m, nil

	// ── Plugin settings screen ────────────────────────────────────────────

	case screens.OpenStreamRadarMsg:
		return m, screen.TransitionCmd(screens.NewStreamRadarScreen(m.streamStats), true)

	case screens.OpenRatingWeightsMsg:
		return m, screen.TransitionCmd(screens.NewRatingWeightsScreen(), true)

	case screens.OpenOfflineLibraryMsg:
		if m.mediaCache != nil {
			return m, screen.TransitionCmd(screens.NewOfflineLibraryScreen(m.mediaCache), true)
		}

	case screens.OfflineOpenDetailMsg:
		return m, m.openDetail(msg.Entry)

	case screens.ClearMediaCacheMsg:
		if m.mediaCache != nil {
			_ = m.mediaCache.Clear()
			m.state.StatusMsg = "Media cache cleared"
		}

	case screens.OpenPluginManagerMsg:
		return m, screen.TransitionCmd(screens.NewPluginManagerScreen(m.client), true)

	case screens.OpenPluginSettingsMsg:
		return m, screen.TransitionCmd(screens.NewPluginSettingsScreen(m.client), true)

	case screens.OpenPluginReposMsg:
		return m, screen.TransitionCmd(screens.NewPluginReposScreen(m.client), true)

	case screens.OpenPluginRegistryMsg:
		return m, screen.TransitionCmd(screens.NewPluginRegistryScreen(m.client), true)

	case screens.OpenKeybindsEditorMsg:
		return m, screen.TransitionCmd(screens.NewKeybindsEditorScreen(), true)

	// ── Skip detection ────────────────────────────────────────────────────

	case ipc.SkipSegmentMsg:
		switch msg.SegmentType {
		case "intro":
			m.skipIntro = &msg
		case "credits":
			m.skipCredits = &msg
		}

	// ── Music sub-tab data messages ───────────────────────────────────────

	case ipc.MpdQueueResultMsg:
		// Session: save the new queue URIs, and restore saved queue if MPD
		// returns an empty queue on the very first load after connecting.
		if msg.Err == nil {
			uris := make([]string, len(msg.Tracks))
			for i, t := range msg.Tracks {
				uris[i] = t.File
			}
			m.lastQueueURIs = uris
			var sessionCmd tea.Cmd
			if !m.queueRestored && len(msg.Tracks) == 0 && len(m.pendingQueueURIs) > 0 && m.client != nil {
				// First load is empty → restore the saved queue.
				client := m.client
				pending := m.pendingQueueURIs
				sessionCmd = func() tea.Msg {
					for _, uri := range pending {
						client.MpdCmd("mpd_add", map[string]any{"uri": uri})
					}
					return nil
				}
			}
			m.queueRestored = true
			var musicCmd tea.Cmd
			m.musicScreen, musicCmd = m.musicScreen.Update(msg)
			return m, tea.Batch(musicCmd, sessionCmd, m.sessionSaveCmd())
		}
		var cmd tea.Cmd
		m.musicScreen, cmd = m.musicScreen.Update(msg)
		return m, cmd

	case ipc.MpdQueueChangedMsg,
		ipc.MpdDirResultMsg, ipc.MpdLibraryResultMsg,
		ipc.MpdPlaylistsResultMsg, ipc.MpdPlaylistTracksResultMsg:
		var cmd tea.Cmd
		m.musicScreen, cmd = m.musicScreen.Update(msg)
		return m, cmd

	// ── Keyboard ──────────────────────────────────────────────────────────

	case tea.MouseMsg:
		return m.handleMouse(msg)

	case tea.KeyPressMsg:
		return m.handleKey(msg)
	}

	return m, nil
}

// ── Session helpers ───────────────────────────────────────────────────────────

// sessionSaveCmd returns a Cmd that writes the current session state to disk
// asynchronously so it doesn't block the render loop.
func (m Model) sessionSaveCmd() tea.Cmd {
	s := session.State{
		LastTab:         m.state.ActiveTab.String(),
		LastMusicSubTab: int(m.musicScreen.ActiveSubTab()),
		QueueURIs:       m.lastQueueURIs,
	}
	path := m.sessionPath
	return func() tea.Msg {
		_ = session.Save(path, s)
		return nil
	}
}

// ── Mouse handling ────────────────────────────────────────────────────────────

func (m Model) handleMouse(msg tea.MouseMsg) (tea.Model, tea.Cmd) {
	mouse := msg.Mouse()
	switch {
	case mouse.Button == tea.MouseWheelUp:
		return m.handleKey(tea.KeyPressMsg{Code: 'k', Text: "k"})
	case mouse.Button == tea.MouseWheelDown:
		return m.handleKey(tea.KeyPressMsg{Code: 'j', Text: "j"})
	case mouse.Button == tea.MouseRight:
		// Right-click is currently only meaningful in the Music tab so it
		// can open the per-track context dialog (Add to queue / Replace
		// queue / Add to Playlist / Create Playlist). Other tabs ignore.
		if m.state.ActiveTab != state.TabMusic {
			return m, nil
		}
		topBarY := m.overlayRowCount()
		const topBarTotalRowsR = 5
		relY := mouse.Y - topBarY - topBarTotalRowsR - 1
		cardX := mouse.X - 3
		var cmd tea.Cmd
		m.musicScreen, cmd = m.musicScreen.HandleRightMouse(cardX, relY)
		return m, cmd
	case mouse.Button == tea.MouseLeft:
		topBarY := m.overlayRowCount()
		y := mouse.Y
		x := mouse.X
		// TopBarStyle: MarginTop(1) + border-top(1) + content(1) + border-bottom(1) = 4 rows.
		// Content row (tabs/search/gear) is at topBarY+2.
		// After topbar + 1 blank gap row, main content starts at topBarY+5.
		const topBarContentOffset = 2 // MarginTop + border-top
		const topBarTotalRows = 5     // 4 topbar rows + 1 gap blank line
		if y == topBarY+topBarContentOffset {
			// Click on top tab bar — hit-test tabs, search, and gear.
			if tab, ok := m.hitTestTopTabBar(x); ok {
				m.switchTab(tab)
				return m, nil
			}
			if next, cmd, hit := m.hitTestTopBarWidgets(x); hit {
				return next, cmd
			}
			return m, nil
		}
		if m.state.ActiveTab == state.TabMusic {
			// Relay to music screen with Y relative to music content (after top bar).
			relY := y - topBarY - topBarTotalRows - 1 // -1 for MainCardStyle top border
			prev := m.musicScreen.ActiveSubTab()
			var cmd tea.Cmd
			cardX := x - 3 // subtract MainCardStyle left offset (margin+border+padding)
			m.musicScreen, cmd = m.musicScreen.HandleMouse(cardX, relY)
			if m.musicScreen.ActiveSubTab() != prev {
				return m, tea.Batch(cmd, m.sessionSaveCmd())
			}
			return m, cmd
		}
		if m.state.ActiveTab == state.TabCollections {
			var cmd tea.Cmd
			m.collectionsScreen, cmd = m.collectionsScreen.Update(msg)
			return m, cmd
		}
		if m.screen == screenList {
			// Click on a result row in the list view.
			topBarRows := topBarY + topBarTotalRows // overlay rows + topbar rows + gap
			colHeaderRow := topBarRows              // column header row
			bodyStartY := colHeaderRow + 1          // result rows start here
			bodyRow := y - bodyStartY
			if bodyRow >= 0 {
				availH := max(1, m.state.Height-9)
				start := 0
				if m.state.Cursor >= availH {
					start = m.state.Cursor - availH + 1
				}
				idx := start + bodyRow
				if idx >= 0 && idx < len(m.state.Results) {
					m.state.Cursor = idx
					m.state.Focus = state.FocusResults
				}
			}
			return m, nil
		}
	}
	return m, nil
}

// overlayRowCount returns the number of rows prepended above the main content
// by applyToast (NowPlaying bar for non-MPD playback, DSP status).
// MPD HUD is no longer prepended — it lives in the footer slot.
func (m Model) overlayRowCount() int {
	n := 0
	if m.nowPlaying != nil {
		s := components.RenderNowPlaying(m.nowPlaying, m.state.Width)
		if s != "" {
			n += strings.Count(s, "\n")
		}
	}
	if m.dspState != nil && m.dspState.Enabled {
		dspHud := components.RenderDspStatus(m.dspState, m.state.Width)
		if dspHud != "" {
			n += strings.Count(dspHud, "\n")
		}
	}
	return n
}

// hitTestTopTabBar returns the Tab the user clicked based on the X coordinate.
func (m Model) hitTestTopTabBar(x int) (state.Tab, bool) {
	pos := 3 // MarginLeft(1) + BorderLeft(1) + PaddingLeft(1)
	for _, t := range state.Tabs() {
		label := fmt.Sprintf(" %s ", t.String())
		var rendered string
		if t == m.state.ActiveTab {
			rendered = theme.T.TabActiveStyle().Render(label)
		} else {
			rendered = theme.T.TabStyle().Render(label)
		}
		w := lipgloss.Width(rendered)
		if x >= pos && x < pos+w {
			return t, true
		}
		pos += w
	}
	return 0, false
}

// hitTestTopBarWidgets returns the (possibly updated) Model, a command,
// and whether the click landed on a top-bar widget. The Model must be
// returned because focusing the search bar mutates state that needs to
// propagate back to the Bubble Tea program.
func (m Model) hitTestTopBarWidgets(x int) (Model, tea.Cmd, bool) {
	w := m.state.Width
	// Compute tab strip width (same logic as viewTopBar / hitTestTopTabBar).
	tabsW := 0
	for _, t := range state.Tabs() {
		label := fmt.Sprintf(" %s ", t.String())
		var rendered string
		if t == m.state.ActiveTab {
			rendered = theme.T.TabActiveStyle().Render(label)
		} else {
			rendered = theme.T.TabStyle().Render(label)
		}
		tabsW += lipgloss.Width(rendered)
	}

	// Compute search box and gear widths (same logic as viewTopBar).
	prefix := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Render("\u2315 ")
	var searchBox string
	switch {
	case m.state.Focus == state.FocusSearch:
		searchBox = theme.T.SearchFocusedStyle().Render(prefix + m.search.View())
	case m.search.Value() != "":
		searchBox = theme.T.SearchStyle().Render(prefix + lipgloss.NewStyle().Foreground(theme.T.Text()).Render(m.search.Value()))
	default:
		searchBox = theme.T.SearchStyle().Render(prefix + lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("Search\u2026  /"))
	}
	var gear string
	switch m.state.RuntimeStatus {
	case state.RuntimeError:
		gear = theme.T.GearStyle().Foreground(theme.T.Red()).Render("\u2699")
	case state.RuntimeReady:
		gear = theme.T.GearFocusedStyle().Render("\u2699")
	default:
		gear = theme.T.GearStyle().Render("\u2699")
	}
	searchW := lipgloss.Width(searchBox)
	gearW := lipgloss.Width(gear)

	contentW := w - 6
	spacerLeft := max(0, (contentW/2)-tabsW-(searchW/2))
	// TopBarStyle has MarginLeft(1) + BorderLeft(1) + PaddingLeft(1) = 3.
	const topBarPaddingLeft = 3
	searchStart := topBarPaddingLeft + tabsW + spacerLeft
	searchEnd := searchStart + searchW
	gearStart := searchEnd + max(0, contentW-tabsW-searchW-gearW-spacerLeft)
	gearEnd := gearStart + gearW

	switch {
	case x >= searchStart && x < searchEnd:
		m.state.Focus = state.FocusSearch
		m.state.SearchActive = true
		return m, m.search.Focus(), true
	case x >= gearStart && x < gearEnd:
		return m, screen.OpenOverlayCmd(screens.NewSettingsModel(m.client, m.cfg)), true
	}
	return m, nil, false
}

// ── Player helpers ────────────────────────────────────────────────────────────

// subSyncState builds a syncOverlayState by reading the current delay from the
// active NowPlayingState and adding delta (optimistic update before mpv echoes back).
func (m *Model) subSyncState(isAudio bool, delta float64) *syncOverlayState {
	cur := 0.0
	if np := m.activeNowPlaying(); np != nil {
		if isAudio {
			cur = np.AudioDelay
		} else {
			cur = np.SubtitleDelay
		}
	}
	return &syncOverlayState{isAudio: isAudio, delay: cur + delta}
}

// currentDownloads returns a snapshot of the download list in arrival order.
func (m *Model) currentDownloads() []*ipc.DownloadEntry {
	out := make([]*ipc.DownloadEntry, 0, len(m.downloadOrder))
	for _, gid := range m.downloadOrder {
		if e, ok := m.downloadMap[gid]; ok {
			cp := *e
			out = append(out, &cp)
		}
	}
	return out
}

// playBingeNext immediately plays the next episode in the binge context and
// returns any resulting Cmd.  Clears the countdown regardless of success.
func (m *Model) playBingeNext() tea.Cmd {
	m.bingeCountdown = -1
	if m.bingeCtx == nil {
		return nil
	}
	nextIdx := m.bingeCtx.CurrentIdx + 1
	if nextIdx >= len(m.bingeCtx.Episodes) {
		m.bingeCtx = nil
		return nil
	}
	ep := m.bingeCtx.Episodes[nextIdx]
	if m.client != nil {
		m.client.Play(ep.EntryID, ep.Provider, "", m.bingeCtx.Tab)
	}
	// Advance context index so the *following* end-of-file can queue E+2, etc.
	m.bingeCtx.CurrentIdx = nextIdx
	title := fmt.Sprintf("%s S%02dE%02d", m.bingeCtx.SeriesTitle, ep.Season, ep.Episode)
	if ep.Title != "" {
		title += " – " + ep.Title
	}
	m.nowPlayingEntryID = ep.EntryID
	m.nowPlayingEntry = watchhistory.Entry{
		ID:       ep.EntryID,
		Title:    title,
		Tab:      string(m.bingeCtx.Tab),
		Provider: ep.Provider,
		Season:   ep.Season,
		Episode:  ep.Episode,
	}
	if m.historyStore != nil {
		m.historyStore.Upsert(m.nowPlayingEntry)
	}
	m.state.StatusMsg = fmt.Sprintf("▶ %s", title)
	return nil
}

// activeNowPlaying returns whichever NowPlayingState is currently live,
// preferring the detail panel over the global one.
func (m *Model) activeNowPlaying() *components.NowPlayingState {
	if m.detail != nil && m.detail.NowPlaying != nil {
		return m.detail.NowPlaying
	}
	return m.nowPlaying
}

// ── Key routing ───────────────────────────────────────────────────────────────

func (m Model) handleKey(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	key := msg.String()

	// ── Binge countdown intercept ──────────────────────────────────────────
	// Enter/Space plays immediately; Esc/n cancels.
	if m.bingeCountdown >= 0 {
		switch key {
		case "enter", " ":
			return m, m.playBingeNext()
		case "esc", "n":
			m.bingeCountdown = -1
			m.bingeCtx = nil
			m.state.StatusMsg = "Binge cancelled"
			return m, nil
		}
	}

	// ── Quality quick keys — detail overlay only ──────────────────────────
	// Must intercept here, before ActionTab1–4 in the global action dispatch.
	if m.screen == screenDetail && m.detail != nil && !m.detail.CollectionPickerOpen {
		qualKeyRank := map[string]int{"1": 2, "2": 4, "3": 5, "4": 7}
		if rank, ok := qualKeyRank[key]; ok {
			m.pendingQuality = rank
			if m.client != nil {
				m.client.Resolve(m.detail.Entry.ID, "")
			}
			t, cmd := components.ShowToast("Resolving streams\u2026", false)
			m.activeToast = &t
			return m, cmd
		}
	}

	// ── Action-based dispatch (high-level intents, independent of key layout) ──
	if action, ok := actions.FromKey(key); ok {
		switch action {
		case actions.ActionQuit:
			if m.client != nil {
				// Stop MPD playback before exiting so music doesn't keep
				// playing after stui closes.
				if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
					m.client.MpdCmd("mpd_stop", nil)
				}
				m.client.Stop()
			}
			return m, tea.Quit
		case actions.ActionOpenSettings:
			return m, screen.OpenOverlayCmd(screens.NewSettingsModel(m.client, m.cfg))
		case actions.ActionOpenHelp:
			return m, screen.TransitionCmd(screens.NewHelpScreen(), true)
		case actions.ActionOpenSearch:
			m.state.Focus = state.FocusSearch
			m.state.SearchActive = true
			return m, m.search.Focus()
		case actions.ActionNextTab:
			next := (int(m.state.ActiveTab) + 1) % len(state.Tabs())
			m.switchTab(state.Tab(next))
			if m.state.IsLoading {
				return m, m.loadingSpinner.Tick
			}
			return m, nil
		case actions.ActionPrevTab:
			prev := (int(m.state.ActiveTab) - 1 + len(state.Tabs())) % len(state.Tabs())
			m.switchTab(state.Tab(prev))
			if m.state.IsLoading {
				return m, m.loadingSpinner.Tick
			}
			return m, nil
		case actions.ActionTab1:
			m.switchTab(state.TabMovies)
			if m.state.IsLoading {
				return m, m.loadingSpinner.Tick
			}
			return m, nil
		case actions.ActionTab2:
			m.switchTab(state.TabSeries)
			if m.state.IsLoading {
				return m, m.loadingSpinner.Tick
			}
			return m, nil
		case actions.ActionTab3:
			m.switchTab(state.TabMusic)
			if m.state.IsLoading {
				return m, m.loadingSpinner.Tick
			}
			return m, nil
		case actions.ActionTab4:
			m.switchTab(state.TabLibrary)
			return m, nil
		case actions.ActionTab5:
			m.switchTab(state.TabCollections)
			return m, nil
		}
		// Player actions handled below (need active player check)
	}

	// ── Global player controls — active whenever mpv is running ───────────
	activePlayer := m.nowPlaying
	if m.detail != nil && m.detail.NowPlaying != nil {
		activePlayer = m.detail.NowPlaying
	}
	if activePlayer != nil && m.client != nil {
		if action, ok := actions.FromKey(key); ok && action.IsPlayerAction() {
			switch action {
			case actions.ActionPlayerPause:
				m.client.PlayerCommand("cycle", "pause")
				return m, nil
			case actions.ActionPlayerSeekBack:
				m.client.PlayerCommand("seek", -10)
				return m, nil
			case actions.ActionPlayerSeekFwd:
				m.client.PlayerCommand("seek", 10)
				return m, nil
			case actions.ActionPlayerSeekBackLong:
				m.client.PlayerCommand("seek", -60)
				return m, nil
			case actions.ActionPlayerSeekFwdLong:
				m.client.PlayerCommand("seek", 60)
				return m, nil
			case actions.ActionPlayerFullscreen:
				m.client.PlayerCommand("cycle", "fullscreen")
				return m, nil
			case actions.ActionSubtitleCycle:
				m.client.PlayerCommand("cycle", "sub")
				return m, nil
			case actions.ActionSubtitleOff:
				m.client.PlayerCommand("set_property", "sid", "no")
				return m, nil
			case actions.ActionSubDelayPlus:
				m.client.PlayerCommand("add", "sub-delay", 0.1)
				m.syncOverlay = m.subSyncState(false, +0.1)
				return m, syncHideCmd()
			case actions.ActionSubDelayMinus:
				m.client.PlayerCommand("add", "sub-delay", -0.1)
				m.syncOverlay = m.subSyncState(false, -0.1)
				return m, syncHideCmd()
			case actions.ActionSubDelayReset:
				m.client.PlayerCommand("set_property", "sub-delay", 0.0)
				m.syncOverlay = &syncOverlayState{isAudio: false, delay: 0}
				return m, syncHideCmd()
			case actions.ActionAudioPicker:
				return m, screen.TransitionCmd(
					screens.NewAudioTrackPickerScreen(m.client, m.playerTracks),
					true,
				)
			case actions.ActionAudioCycle:
				m.client.PlayerCommand("cycle", "audio")
				return m, nil
			case actions.ActionAudioDelayPlus:
				m.client.PlayerCommand("add", "audio-delay", 0.1)
				m.syncOverlay = m.subSyncState(true, +0.1)
				return m, syncHideCmd()
			case actions.ActionAudioDelayMinus:
				m.client.PlayerCommand("add", "audio-delay", -0.1)
				m.syncOverlay = m.subSyncState(true, -0.1)
				return m, syncHideCmd()
			case actions.ActionAudioDelayReset:
				m.client.PlayerCommand("set_property", "audio-delay", 0.0)
				m.syncOverlay = &syncOverlayState{isAudio: true, delay: 0}
				return m, syncHideCmd()
			case actions.ActionVolumeUp:
				m.client.PlayerCommand("add", "volume", 5)
				return m, nil
			case actions.ActionVolumeDown:
				m.client.PlayerCommand("add", "volume", -5)
				return m, nil
			case actions.ActionVolumeMute:
				m.client.PlayerCommand("cycle", "mute")
				return m, nil
			case actions.ActionStreamNext:
				m.client.PlayerCommand("next_candidate", nil)
				t, toastCmd := components.ShowToast("Switching to next stream\u2026", false)
				m.activeToast = &t
				return m, toastCmd
			case actions.ActionStreamSwitch:
				// Open the full stream picker screen
				if m.detail != nil {
					return m, screen.TransitionCmd(
						screens.NewStreamPickerScreen(m.client, m.detail.Entry.Title, m.detail.Entry.ID, m.state.Settings.BenchmarkStreams),
						true,
					)
				}
				return m, nil
			case actions.ActionPlayerStop:
				m.client.PlayerStop()
				m.nowPlayingEntryID = "" // manual stop — suppress auto-delete
				return m, nil
			case actions.ActionPlayerScreenshot:
				m.client.PlayerCommand("screenshot")
				return m, nil
			case actions.ActionSkipIntro:
				if m.skipIntro != nil {
					m.client.PlayerCommand("seek", m.skipIntro.End)
					m.skipIntro = nil
				} else if m.skipCredits != nil && m.nowPlaying != nil {
					seekTo := m.nowPlaying.Duration - m.skipCredits.End + 2
					m.client.PlayerCommand("set_property", "time-pos", seekTo)
					m.skipCredits = nil
				}
				return m, nil
			}
		}
	}

	// ── MPD controls — active whenever MPD HUD is visible ────────────────
	if m.mpdNowPlaying != nil && m.client != nil {
		switch key {
		case "n":
			m.client.MpdCmd("mpd_next", nil)
			return m, nil
		case "p":
			m.client.MpdCmd("mpd_prev", nil)
			return m, nil
		case " ":
			if m.mpdNowPlaying.State == "pause" {
				m.client.MpdCmd("mpd_resume", nil)
			} else {
				m.client.MpdCmd("mpd_pause", nil)
			}
			return m, nil
		case "S":
			// Shuffle the MPD queue. Settings moved to `/~ (console-style).
			m.client.MpdCmd("mpd_shuffle", nil)
			return m, nil
		case "+", "=":
			vol := int(m.mpdNowPlaying.Volume) + 5
			if vol > 100 {
				vol = 100
			}
			m.client.MpdCmd("mpd_set_volume", map[string]any{"volume": vol})
			return m, nil
		case "-":
			vol := int(m.mpdNowPlaying.Volume) - 5
			if vol < 0 {
				vol = 0
			}
			m.client.MpdCmd("mpd_set_volume", map[string]any{"volume": vol})
			return m, nil
		case "r":
			// Cycle replay gain: off → track → album → auto → off
			next := map[string]string{
				"off": "track", "track": "album", "album": "auto", "auto": "off",
			}
			mode := next[m.mpdNowPlaying.ReplayGain]
			if mode == "" {
				mode = "auto"
			}
			m.client.MpdCmd("mpd_replay_gain", map[string]any{"mode": mode})
			return m, nil
		case "q":
			m.client.MpdCmd("mpd_clear", nil)
			m.mpdNowPlaying = nil
			return m, nil
		}
	}

	// ── DSP controls — active whenever DSP is enabled ────────────────────────
	if m.dspState != nil && m.client != nil {
		switch key {
		case "d":
			// Toggle DSP enabled/disabled
			enabled := !m.dspState.Enabled
			m.client.SetDspConfig(&enabled, nil, nil, nil, nil, nil, nil, nil, nil)
			return m, nil
		case "c":
			// Toggle convolution
			if m.dspState.Enabled {
				convEnabled := !m.dspState.ConvolutionEnabled
				m.client.SetDspConfig(nil, nil, nil, nil, nil, nil, nil, &convEnabled, nil)
			}
			return m, nil
		case "b":
			// Toggle convolution bypass
			if m.dspState.Enabled && m.dspState.ConvolutionEnabled {
				bypass := !m.dspState.ConvolutionBypass
				m.client.SetDspConfig(nil, nil, nil, nil, nil, nil, nil, nil, &bypass)
			}
			return m, nil
		case "r":
			// Refresh DSP status from runtime (re-sync UI state)
			m.client.GetDspStatus()
			return m, nil
		case "D":
			// Bind DSP to MPD output
			m.client.BindDspToMpd()
			return m, nil
		}
	}

	// Search input captures keys while focused — must come BEFORE the
	// Music/Collections tab routing, otherwise those screens consume every
	// keystroke and the search bar appears frozen (no typing, no esc).
	if m.state.Focus == state.FocusSearch {
		switch key {
		case "esc":
			m.state.Focus = state.FocusTabs
			m.search.Blur()
			m.state.SearchActive = false
			m.screen = screenGrid
			return m, nil
		case "enter":
			query := m.search.Value()
			m.state.SearchQuery = query
			m.state.Focus = state.FocusResults
			m.search.Blur()
			if query != "" {
				m.state.IsLoading = true
				m.state.LoadingStart = time.Now().Unix()
				m.state.StatusMsg = fmt.Sprintf("Searching for \u201c%s\u201d\u2026", query)
				m.dispatchSearch(query)
				return m, m.loadingSpinner.Tick
			}
		default:
			var cmd tea.Cmd
			m.search, cmd = m.search.Update(msg)
			return m, cmd
		}
		return m, nil
	}

	// Music tab owns all navigation while active
	if m.state.ActiveTab == state.TabMusic {
		prev := m.musicScreen.ActiveSubTab()
		var cmd tea.Cmd
		m.musicScreen, cmd = m.musicScreen.Update(msg)
		if m.musicScreen.ActiveSubTab() != prev {
			return m, tea.Batch(cmd, m.sessionSaveCmd())
		}
		return m, cmd
	}

	// Collections tab owns all navigation while active (unless detail is open)
	if m.state.ActiveTab == state.TabCollections && !(m.screen == screenDetail && m.detail != nil) {
		var cmd tea.Cmd
		m.collectionsScreen, cmd = m.collectionsScreen.Update(msg)
		return m, cmd
	}

	// Detail overlay captures everything while open
	if m.screen == screenDetail && m.detail != nil {
		return m.handleDetailKey(key)
	}

	// Grid navigation
	if m.screen == screenGrid {
		entries := m.currentGridEntries()
		switch key {
		case "l", "right":
			if m.cwFocused {
				cwItems := m.cwCurrentItems()
				if m.cwCursor < len(cwItems)-1 {
					m.cwCursor++
				}
				return m, nil
			}
			m.gridCursor = screens.MoveCursorRight(m.gridCursor, len(entries))
			m.state.Focus = state.FocusResults
			return m, nil
		case "h", "left":
			if m.cwFocused {
				if m.cwCursor > 0 {
					m.cwCursor--
				}
				return m, nil
			}
			m.gridCursor = screens.MoveCursorLeft(m.gridCursor)
			m.state.Focus = state.FocusResults
			return m, nil
		case "j", "down":
			if m.cwFocused {
				m.cwFocused = false
				m.gridCursor = screens.GridCursor{}
				return m, nil
			}
			m.gridCursor = screens.MoveCursorDown(m.gridCursor, len(entries))
			m.state.Focus = state.FocusResults
			return m, nil
		case "k", "up":
			if !m.cwFocused {
				cwItems := m.cwCurrentItems()
				if len(cwItems) > 0 && m.gridCursor.IsAtTopRow() {
					m.cwFocused = true
					if m.cwCursor >= len(cwItems) {
						m.cwCursor = len(cwItems) - 1
					}
					return m, nil
				}
				m.gridCursor = screens.MoveCursorUp(m.gridCursor)
				m.state.Focus = state.FocusResults
				return m, nil
			}
			return m, nil
		case "enter":
			if m.cwFocused {
				cwItems := m.cwCurrentItems()
				if len(cwItems) == 0 || m.cwCursor >= len(cwItems) {
					return m, nil
				}
				entry := cwItems[m.cwCursor]
				if entry.Provider == "" {
					return m, m.openDetail(historyEntryToCatalogEntry(entry))
				}
				if m.client == nil {
					return m, nil
				}
				tab := ipc.MediaTab(entry.Tab)
				m.nowPlayingEntryID = entry.ID
				m.nowPlayingEntry = watchhistory.Entry{
					ID:       entry.ID,
					Title:    entry.Title,
					Year:     entry.Year,
					Tab:      entry.Tab,
					Provider: entry.Provider,
					ImdbID:   entry.ImdbID,
					Season:   entry.Season,
					Episode:  entry.Episode,
				}
				if m.historyStore != nil {
					m.historyStore.Upsert(m.nowPlayingEntry)
				}
				m.state.StatusMsg = fmt.Sprintf("Resuming %s\u2026", entry.Title)
				m.client.PlayFrom(entry.ID, entry.Provider, entry.ImdbID, tab, entry.Position)
				return m, nil
			}
			idx := m.gridCursor.Index(components.CardColumns)
			if idx >= 0 && idx < len(entries) {
				return m, m.openDetail(entries[idx])
			}
			return m, nil
		case "i":
			if m.cwFocused {
				cwItems := m.cwCurrentItems()
				if len(cwItems) == 0 || m.cwCursor >= len(cwItems) {
					return m, nil
				}
				return m, m.openDetail(historyEntryToCatalogEntry(cwItems[m.cwCursor]))
			}
			return m, nil
		case "d":
			if m.cwFocused {
				if m.historyStore != nil {
					cwItems := m.cwCurrentItems()
					if len(cwItems) == 0 || m.cwCursor >= len(cwItems) {
						return m, nil
					}
					m.historyStore.Remove(cwItems[m.cwCursor].ID)
					go func() { _ = m.historyStore.Save() }()
					newItems := m.cwCurrentItems()
					if len(newItems) == 0 {
						m.cwFocused = false
					} else if m.cwCursor >= len(newItems) {
						m.cwCursor = len(newItems) - 1
					}
					return m, nil
				}
				return m, nil // historyStore not yet initialized; nothing to remove
			}
			return m, nil
		case "v":
			m.screen = screenList
			return m, nil
		}
	}

	// List navigation
	if m.screen == screenList {
		switch key {
		case "j", "down":
			m.state.Focus = state.FocusResults
			if m.state.Cursor < len(m.state.Results)-1 {
				m.state.Cursor++
			}
			return m, nil
		case "k", "up":
			m.state.Focus = state.FocusResults
			if m.state.Cursor > 0 {
				m.state.Cursor--
			}
			return m, nil
		case "enter":
			if m.state.Cursor < len(m.state.Results) {
				r := m.state.Results[m.state.Cursor]
				entry := listResultToCatalogEntry(r, m.state.ActiveTab.MediaTabID())
				return m, m.openDetail(entry)
			}
			return m, nil
		case "v":
			m.screen = screenGrid
			return m, nil
		}
	}

	// Global keys
	switch key {
	case "ctrl+c", "q":
		if m.client != nil {
			if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
				m.client.MpdCmd("mpd_stop", nil)
			}
			m.client.Stop()
		}
		return m, tea.Quit
	case "/":
		// Activate the inline search bar in the topbar.
		m.state.Focus = state.FocusSearch
		m.state.SearchActive = true
		return m, m.search.Focus()
	case "?":
		return m, screen.TransitionCmd(screens.NewHelpScreen(), true)
	case "D":
		// Open downloads manager
		entries := m.currentDownloads()
		return m, screen.TransitionCmd(screens.NewDownloadsScreen(m.client, entries), true)
	case "P":
		// Open plugin manager
		return m, screen.TransitionCmd(screens.NewPluginManagerScreen(m.client), true)
	case "O":
		// Open offline library
		if m.mediaCache != nil {
			return m, screen.TransitionCmd(screens.NewOfflineLibraryScreen(m.mediaCache), true)
		}
	case "tab":
		next := (int(m.state.ActiveTab) + 1) % len(state.Tabs())
		m.switchTab(state.Tab(next))
	case "shift+tab":
		prev := (int(m.state.ActiveTab) - 1 + len(state.Tabs())) % len(state.Tabs())
		m.switchTab(state.Tab(prev))
	case "1":
		m.switchTab(state.TabMovies)
	case "2":
		m.switchTab(state.TabSeries)
	case "3":
		m.switchTab(state.TabMusic)
	case "4":
		m.switchTab(state.TabLibrary)
	case "5":
		m.switchTab(state.TabCollections)
	case "`", "~":
		// Console-quake-style hotkey for settings — familiar from games.
		return m, screen.OpenOverlayCmd(screens.NewSettingsModel(m.client, m.cfg))
	case "esc":
		if m.cwFocused {
			m.cwFocused = false
			return m, nil
		}
		m.state.Focus = state.FocusTabs
		m.state.SearchActive = false
		m.search.Blur()
		m.screen = screenGrid
	}
	return m, nil
}

// ── Detail key handler ────────────────────────────────────────────────────────

func (m Model) handleDetailKey(key string) (tea.Model, tea.Cmd) {
	ds := m.detail

	// Collection picker swallows all keys while open
	if ds.CollectionPickerOpen {
		return m.handleCollectionPickerKey(key)
	}

	switch key {
	case "c":
		// Open the inline collection picker
		if !ds.PersonMode && m.collectionsStore != nil {
			ds.CollectionPickerOpen = true
			ds.CollectionPickerCursor = 0
			ds.CollectionPickerNames = m.collectionsStore.Names()
		}
		return m, nil

	case "esc":
		if ds.PersonMode {
			if !ds.PopBreadcrumb() {
				m.screen = screenGrid
				m.detail = nil
				if !m.state.CurrentStream.IsSet() {
					m.state.CurrentMedia = state.CurrentMedia{}
				}
			}
			return m, nil
		}
		m.screen = screenGrid
		m.detail = nil
		if !m.state.CurrentStream.IsSet() {
			m.state.CurrentMedia = state.CurrentMedia{}
		}
		return m, nil

	case "q", "ctrl+c":
		// q stops playback if active; if not, quits the app
		if ds.NowPlaying != nil && m.client != nil {
			m.client.PlayerStop()
			m.nowPlayingEntryID = "" // manual stop — suppress auto-delete
			return m, nil
		}
		if m.client != nil {
			m.client.Stop()
		}
		return m, tea.Quit

	// Cycle focus zones: Info → Cast → Provider → Similar → Info
	case "tab":
		if ds.PersonMode {
			return m, nil
		}
		switch ds.Focus {
		case screens.FocusDetailInfo:
			if len(ds.Entry.Cast) > 0 {
				ds.Focus = screens.FocusDetailCast
			} else if len(ds.Entry.Providers) > 0 {
				ds.Focus = screens.FocusDetailProvider
			} else if len(ds.Similar) > 0 {
				ds.Focus = screens.FocusDetailSimilar
			}
		case screens.FocusDetailCast:
			if len(ds.Entry.Providers) > 0 {
				ds.Focus = screens.FocusDetailProvider
			} else if len(ds.Similar) > 0 {
				ds.Focus = screens.FocusDetailSimilar
			} else {
				ds.Focus = screens.FocusDetailInfo
			}
		case screens.FocusDetailProvider:
			if len(ds.Similar) > 0 {
				ds.Focus = screens.FocusDetailSimilar
			} else {
				ds.Focus = screens.FocusDetailInfo
			}
		case screens.FocusDetailSimilar:
			ds.Focus = screens.FocusDetailInfo
		}
		return m, nil

	case "j", "down":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorDown(ds.PersonCursor, len(ds.PersonResults))
		case ds.Focus == screens.FocusDetailInfo:
			ds.InfoScroll++
		case ds.Focus == screens.FocusDetailCast:
			if ds.CastCursor < len(ds.Entry.Cast)-1 {
				ds.CastCursor++
			} else if len(ds.Entry.Providers) > 0 {
				ds.Focus = screens.FocusDetailProvider
			}
		case ds.Focus == screens.FocusDetailProvider:
			if len(ds.Similar) > 0 {
				ds.Focus = screens.FocusDetailSimilar
			}
		case ds.Focus == screens.FocusDetailSimilar:
			// already at bottom
		}
		return m, nil

	case "k", "up":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorUp(ds.PersonCursor)
		case ds.Focus == screens.FocusDetailInfo:
			if ds.InfoScroll > 0 {
				ds.InfoScroll--
			}
		case ds.Focus == screens.FocusDetailCast:
			if ds.CastCursor > 0 {
				ds.CastCursor--
			} else {
				ds.Focus = screens.FocusDetailInfo
			}
		case ds.Focus == screens.FocusDetailProvider:
			if len(ds.Entry.Cast) > 0 {
				ds.Focus = screens.FocusDetailCast
			} else {
				ds.Focus = screens.FocusDetailInfo
			}
		case ds.Focus == screens.FocusDetailSimilar:
			if len(ds.Entry.Providers) > 0 {
				ds.Focus = screens.FocusDetailProvider
			} else if len(ds.Entry.Cast) > 0 {
				ds.Focus = screens.FocusDetailCast
			}
		}
		return m, nil

	case "h", "left":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorLeft(ds.PersonCursor)
		case ds.Focus == screens.FocusDetailProvider:
			if ds.ProviderCursor > 0 {
				ds.ProviderCursor--
			}
		case ds.Focus == screens.FocusDetailSimilar:
			if ds.SimilarCursor > 0 {
				ds.SimilarCursor--
			}
		}
		return m, nil

	case "l", "right":
		switch {
		case ds.PersonMode:
			ds.PersonCursor = screens.MoveCursorRight(ds.PersonCursor, len(ds.PersonResults))
		case ds.Focus == screens.FocusDetailProvider:
			if ds.ProviderCursor < len(ds.Entry.Providers)-1 {
				ds.ProviderCursor++
			}
		case ds.Focus == screens.FocusDetailSimilar:
			if ds.SimilarCursor < len(ds.Similar)-1 {
				ds.SimilarCursor++
			}
		}
		return m, nil

	case "enter":
		switch {
		case ds.PersonMode:
			idx := ds.PersonCursor.Index(components.CardColumns)
			if idx >= 0 && idx < len(ds.PersonResults) {
				ds.PushBreadcrumb(ds.PersonName)
				return m, m.openDetail(ds.PersonResults[idx])
			}

		case ds.Focus == screens.FocusDetailCast:
			member := ds.SelectedCastMember()
			if member == nil {
				return m, nil
			}
			ds.PushBreadcrumb(ds.Entry.Title)
			ds.PersonMode = true
			ds.PersonName = member.Name
			ds.PersonResults = nil
			ds.PersonLoading = true
			ds.PersonCursor = screens.GridCursor{}
			return m, m.dispatchPersonSearch(member.Name)

		case ds.Focus == screens.FocusDetailProvider:
			// ▶ Play via selected provider — resume from saved position if available
			provider := ds.SelectedProvider()
			if provider != "" && m.client != nil {
				tab := ipc.MediaTab(m.state.ActiveTab.MediaTabID())
				startPos := 0.0
				if ds.WatchHistory != nil && ds.WatchHistory.Position > 0 && !ds.WatchHistory.Completed {
					startPos = ds.WatchHistory.Position
					m.state.StatusMsg = fmt.Sprintf("Resuming via %s from %s\u2026",
						provider, formatDurationHMS(startPos))
				} else {
					m.state.StatusMsg = fmt.Sprintf("Resolving via %s\u2026", provider)
				}
				m.client.PlayFrom(ds.Entry.ID, provider, ds.Entry.ImdbID, tab, startPos)
				m.nowPlayingEntryID = ds.Entry.ID
				m.historyLastSavedPos = startPos
				season, episode := watchhistory.ParseEpisodeInfo(ds.Entry.Title)
				m.nowPlayingEntry = watchhistory.Entry{
					ID:       ds.Entry.ID,
					Title:    ds.Entry.Title,
					Year:     ds.Entry.Year,
					Tab:      ds.Entry.Tab,
					Provider: provider,
					ImdbID:   ds.Entry.ImdbID,
					Season:   season,
					Episode:  episode,
				}
				// Create/update the history record immediately so progress
				// updates have an entry to upsert into.
				if m.historyStore != nil {
					m.historyStore.Upsert(m.nowPlayingEntry)
				}
			}
			return m, nil

		case ds.Focus == screens.FocusDetailSimilar:
			idx := ds.SimilarCursor
			if idx >= 0 && idx < len(ds.Similar) {
				ds.PushBreadcrumb(ds.Entry.Title)
				return m, m.openDetail(ds.Similar[idx])
			}
		}
		return m, nil

	case "e", "E":
		// Open episode browser for series items
		if ds.Entry.Tab == "series" || ds.Entry.Tab == "Series" {
			s := screens.NewEpisodeScreen(m.client, ds.Entry.Title, ds.Entry.ID, m.state.Settings.AutoplayNext)
			return m, screen.TransitionCmd(s, true)
		}
		return m, nil

	case "s":
		// Open stream picker for the current item
		if !ds.PersonMode && m.client != nil {
			s := screens.NewStreamPickerScreen(m.client, ds.Entry.Title, ds.Entry.ID, m.state.Settings.BenchmarkStreams)
			return m, screen.TransitionCmd(s, true)
		}
		return m, nil
	}

	return m, nil
}

// ── Collection picker key handler ─────────────────────────────────────────────

func (m Model) handleCollectionPickerKey(key string) (tea.Model, tea.Cmd) {
	ds := m.detail
	switch key {
	case "esc":
		ds.CollectionPickerOpen = false

	case "j", "down":
		if ds.CollectionPickerCursor < len(ds.CollectionPickerNames)-1 {
			ds.CollectionPickerCursor++
		}

	case "k", "up":
		if ds.CollectionPickerCursor > 0 {
			ds.CollectionPickerCursor--
		}

	case "enter":
		if ds.CollectionPickerCursor < len(ds.CollectionPickerNames) && m.collectionsStore != nil {
			collName := ds.CollectionPickerNames[ds.CollectionPickerCursor]
			entry := collections.Entry{
				ID:       ds.Entry.ID,
				Title:    ds.Entry.Title,
				Year:     ds.Entry.Year,
				Tab:      ds.Entry.Tab,
				Provider: ds.Entry.Provider,
				ImdbID:   ds.Entry.ImdbID,
			}
			added := m.collectionsStore.AddTo(collName, entry)
			go func() { _ = m.collectionsStore.Save() }()
			ds.CollectionPickerOpen = false
			if added {
				m.state.StatusMsg = fmt.Sprintf("Added \u201c%s\u201d to %s", ds.Entry.Title, collName)
			} else {
				m.state.StatusMsg = fmt.Sprintf("Already in %s", collName)
			}
		}
	}
	return m, nil
}

// ── Detail opening ────────────────────────────────────────────────────────────

func (m *Model) openDetail(entry ipc.CatalogEntry) tea.Cmd {
	detail := ipc.DetailEntry{
		ID:          entry.ID,
		Title:       entry.Title,
		Year:        derefStr(entry.Year),
		Genre:       derefStr(entry.Genre),
		Rating:      derefStr(entry.Rating),
		Description: derefStr(entry.Description),
		PosterURL:   derefStr(entry.PosterURL),
		Provider:    entry.Provider,
		Tab:         entry.Tab,
		ImdbID:      derefStr(entry.ImdbID),
		Providers:   []string{entry.Provider},
	}
	ds := screens.NewDetailState(detail)
	ds.SimilarLoading = true
	// Populate watch history so the detail screen can show a resume hint.
	if m.historyStore != nil {
		ds.WatchHistory = m.historyStore.Get(entry.ID)
	}
	m.detail = &ds
	m.screen = screenDetail
	m.state.CurrentMedia = state.CurrentMedia{
		ID:       entry.ID,
		Title:    entry.Title,
		Year:     derefStr(entry.Year),
		Genre:    derefStr(entry.Genre),
		Rating:   derefStr(entry.Rating),
		Tab:      m.state.ActiveTab,
		Provider: entry.Provider,
		ImdbID:   derefStr(entry.ImdbID),
	}
	return m.fetchDetailMetadata(detail)
}

// formatDurationHMS converts seconds to a H:MM:SS or M:SS string.
func formatDurationHMS(secs float64) string {
	total := int(secs)
	h := total / 3600
	min := (total % 3600) / 60
	s := total % 60
	if h > 0 {
		return fmt.Sprintf("%d:%02d:%02d", h, min, s)
	}
	return fmt.Sprintf("%d:%02d", min, s)
}

func (m *Model) fetchDetailMetadata(entry ipc.DetailEntry) tea.Cmd {
	tabProviders := m.providersForTab()
	return func() tea.Msg {
		enriched := enrichDetail(entry, tabProviders)
		return ipc.DetailReadyMsg{Entry: enriched}
	}
}

func (m *Model) fetchSimilar(entry ipc.DetailEntry) tea.Cmd {
	grids := m.grids
	entryID := entry.ID
	tab := entry.Tab
	genre := strings.ToLower(entry.Genre)

	return func() tea.Msg {
		if entries, ok := grids[tab]; ok {
			var similar []ipc.CatalogEntry
			for _, e := range entries {
				if e.ID == entryID {
					continue
				}
				eGenre := strings.ToLower(derefStr(e.Genre))
				if genre != "" && strings.Contains(eGenre, genre) {
					similar = append(similar, e)
				}
				if len(similar) >= 12 {
					break
				}
			}
			return ipc.SimilarReadyMsg{ForID: entryID, Entries: similar}
		}
		return ipc.SimilarReadyMsg{ForID: entryID, Entries: nil}
	}
}

func (m *Model) dispatchPersonSearch(name string) tea.Cmd {
	if m.client == nil {
		// No runtime — search local grid
		tab := m.state.ActiveTab.MediaTabID()
		entries := m.grids[tab]
		q := strings.ToLower(name)
		return func() tea.Msg {
			var matches []ipc.CatalogEntry
			for _, e := range entries {
				if strings.Contains(strings.ToLower(e.Title), q) {
					matches = append(matches, e)
				}
			}
			// Return results via SearchResultMsg so existing handler picks it up
			items := make([]ipc.MediaEntry, 0, len(matches))
			for _, e := range matches {
				items = append(items, ipc.MediaEntry{
					ID: e.ID, Title: e.Title,
					Year: e.Year, Genre: e.Genre, Rating: e.Rating,
					Provider: e.Provider,
				})
			}
			total := len(items)
			return ipc.SearchResultMsg{Result: ipc.SearchResult{Items: items, Total: total}}
		}
	}
	reqID := fmt.Sprintf("person-%d", m.reqSeq.Add(1))
	tab := ipc.MediaTab(m.state.ActiveTab.MediaTabID())
	m.client.Search(reqID, name, tab, 50, 0)
	return nil
}

func (m *Model) providersForTab() []string {
	seen := map[string]bool{}
	var out []string
	for _, e := range m.currentGridEntries() {
		if !seen[e.Provider] {
			seen[e.Provider] = true
			out = append(out, e.Provider)
		}
	}
	return out
}

func (m *Model) switchTab(t state.Tab) {
	m.state.ActiveTab = t
	m.state.Cursor = 0
	m.state.Results = nil
	m.gridCursor = screens.GridCursor{}
	m.cwCursor = 0
	// Set cwFocused if the new tab has in-progress items
	if m.historyStore != nil && cwTabActive(t) &&
		len(cwItems(m.historyStore, t.MediaTabID())) > 0 {
		m.cwFocused = true
	} else {
		m.cwFocused = false
	}
	m.screen = screenGrid
	m.detail = nil
	if !m.state.CurrentStream.IsSet() {
		m.state.CurrentMedia = state.CurrentMedia{}
	}
	m.state.StatusMsg = t.String()
	// Collections is local-only — no runtime grid to load.
	if t != state.TabCollections && len(m.grids[t.MediaTabID()]) == 0 {
		m.state.IsLoading = true
		m.state.LoadingStart = time.Now().Unix()
	}
	// Persist the tab choice immediately (pointer receiver — mutation is visible to caller).
	_ = session.Save(m.sessionPath, session.State{
		LastTab:         t.String(),
		LastMusicSubTab: int(m.musicScreen.ActiveSubTab()),
		QueueURIs:       m.lastQueueURIs,
	})
}

func (m *Model) dispatchSearch(query string) {
	if m.client == nil {
		m.state.IsLoading = false
		m.state.StatusMsg = "No runtime \u2014 start with API keys set"
		return
	}
	reqID := fmt.Sprintf("search-%d", m.reqSeq.Add(1))
	tab := ipc.MediaTab(m.state.ActiveTab.MediaTabID())
	m.client.Search(reqID, query, tab, 100, 0)
}

func (m Model) currentGridEntries() []ipc.CatalogEntry {
	if entries, ok := m.grids[m.state.ActiveTab.MediaTabID()]; ok {
		return entries
	}
	return nil
}

// innerWidth returns the usable content width inside MainCardStyle
// (terminal width minus margins, border, and padding: 1+1+1+1+1+1 = 6).
// Floored at 0 to prevent negative dimensions on tiny terminals.
func (m Model) innerWidth() int {
	return max(0, m.state.Width-6)
}

// computeMusicHeight returns the correct height to send to MusicScreen.
// It accounts for the HUD, the top-bar chrome, the main-card borders, and
// whether the footer (status bar) is currently visible.
//
// Fixed chrome above the main card: MarginTop(1) + topbar box(3) + gap blank(1) = 5 rows.
// Main card borders: 2 rows.  Total fixed = 7.
//
// Footer block when shown:
//
//	card MarginBottom(1) + blank separator(1) + statusBar(4) = 6 rows
//
// The statusBar's "4 rows" come from StatusBarStyle: rounded border (top
// + bottom) + 1 content row + MarginBottom(1). Earlier versions of this
// function treated it as a single row, which over-allocated the music
// body by 3 rows and pushed most of the statusBar past the bottom of
// the terminal — visible as "I can see its top border but the rest is
// overflowing" with a gap above it.
func (m Model) computeMusicHeight() int {
	const fixedRows = 7  // topbar area (5) + main-card borders (2)
	const footerRows = 6 // card MB(1) + blank(1) + footer(4)
	// HUD no longer prepended — it lives in the footer slot, so no hudRows
	// subtraction needed. Layout is stable regardless of playback state.
	return max(0, m.state.Height-fixedRows-footerRows)
}

// ── View ──────────────────────────────────────────────────────────────────────

func (m Model) View() tea.View {
	if m.state.Width == 0 {
		return tea.NewView("Loading\u2026")
	}
	var content string
	if m.screen == screenDetail && m.detail != nil {
		overlay := screens.RenderDetailOverlay(
			m.detail,
			m.state.Width,
			m.state.Height,
			m.state.ActiveTab,
			m.state.RuntimeStatus.String(),
		)
		content = m.applyToast(overlay)
	} else {
		// Hide the footer (statusbar + preceding blank line) only on the
		// Queue sub-tab, which uses every row for tracklist + visualizer.
		// Library/Browse/Playlists keep the footer for status messages.
		hidingFooter := m.state.ActiveTab == state.TabMusic &&
			m.musicScreen.ActiveSubTab() == screens.MusicQueue
		var parts []string
		parts = append(parts, m.viewTopBar(m.state.Focus == state.FocusSearch), "", m.viewMainCard(hidingFooter))
		if !hidingFooter {
			parts = append(parts, "", m.viewFooter())
		}
		base := lipgloss.JoinVertical(lipgloss.Left, parts...)
		content = m.applyToast(base)
	}
	v := tea.NewView(content)
	v.AltScreen = true
	v.MouseMode = tea.MouseModeCellMotion
	return v
}

func (m Model) applyToast(base string) string {
	// Prepend NowPlaying bar if playing outside the detail panel
	if m.nowPlaying != nil {
		np := components.RenderNowPlaying(m.nowPlaying, m.state.Width)
		if np != "" {
			base = np + base
		}
	}
	// MPD HUD is now rendered inline in the footer slot (viewFooter),
	// not prepended here. This keeps layout stable regardless of playback.
	// Prepend DSP status panel when DSP is enabled
	if m.dspState != nil && m.dspState.Enabled {
		dspHud := components.RenderDspStatus(m.dspState, m.state.Width)
		if dspHud != "" {
			base = dspHud + base
		}
	}
	// Subtitle / audio sync overlay
	if m.syncOverlay != nil {
		if s := components.RenderSyncOverlay(m.syncOverlay.isAudio, m.syncOverlay.delay, m.state.Width); s != "" {
			base = s + "\n" + base
		}
	}
	// Skip intro overlay
	if m.skipIntro != nil && m.nowPlaying != nil {
		pos := m.nowPlaying.Position
		if pos >= m.skipIntro.Start && pos <= m.skipIntro.End+15 {
			skipStr := components.RenderSkipPrompt("Intro", m.skipIntro.End, m.state.Width)
			if skipStr != "" {
				base = skipStr + base
			}
		}
	}
	// Skip credits overlay
	if m.skipCredits != nil && m.nowPlaying != nil {
		dur := m.nowPlaying.Duration
		pos := m.nowPlaying.Position
		if dur > 0 {
			fromEnd := dur - pos
			if fromEnd <= m.skipCredits.Start+15 && fromEnd >= m.skipCredits.End-5 {
				seekTo := dur - m.skipCredits.End + 2
				skipStr := components.RenderSkipPrompt("Credits", seekTo, m.state.Width)
				if skipStr != "" {
					base = skipStr + base
				}
			}
		}
	}
	// Binge countdown banner — appended below the main content
	if overlay := m.viewBingeOverlay(); overlay != "" {
		base = base + overlay
	}
	// Buffering overlay — shown while waiting for pre-roll or stall-guard
	if overlay := m.viewBufferingOverlay(); overlay != "" {
		base = base + overlay
	}
	if m.activeToast == nil {
		return base
	}
	toastStr := components.RenderToast(m.activeToast, m.state.Width, m.state.Height)
	if toastStr == "" {
		return base
	}
	return lipgloss.Place(
		m.state.Width, m.state.Height,
		lipgloss.Right, lipgloss.Bottom,
		toastStr,
		lipgloss.WithWhitespaceStyle(lipgloss.NewStyle()),
	)
}

// viewBingeOverlay renders the "next episode in Ns" countdown banner.
func (m Model) viewBingeOverlay() string {
	if m.bingeCountdown < 0 || m.bingeCtx == nil {
		return ""
	}
	nextIdx := m.bingeCtx.CurrentIdx + 1
	if nextIdx >= len(m.bingeCtx.Episodes) {
		return ""
	}
	ep := m.bingeCtx.Episodes[nextIdx]

	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())

	epLabel := fmt.Sprintf("S%02dE%02d", ep.Season, ep.Episode)
	if ep.Title != "" {
		epLabel += "  " + ep.Title
	}

	line1 := acc.Render("▶") + "  Next: " + neon.Render(m.bingeCtx.SeriesTitle) +
		"  " + dim.Render(epLabel)
	line2 := dim.Render(fmt.Sprintf("  Playing in %ds", m.bingeCountdown)) +
		"   " + acc.Render("[Enter]") + dim.Render(" play now") +
		"   " + dim.Render("[Esc] cancel")

	w := m.state.Width - 4
	if w < 40 {
		w = 40
	}
	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Padding(0, 2).
		Width(w).
		Render(line1 + "\n" + line2)

	return "\n" + box + "\n"
}

// viewBufferingOverlay renders a pre-roll / stall-guard progress bar.
func (m Model) viewBufferingOverlay() string {
	if m.playerBuffer == nil {
		return ""
	}
	buf := m.playerBuffer

	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())

	label := "Buffering"
	if buf.Reason == "stall_guard" {
		label = "Stall guard — paused"
	}

	// Progress bar: 24 chars wide
	const barW = 24
	filled := int(float64(barW) * buf.FillPercent / 100.0)
	if filled > barW {
		filled = barW
	}
	bar := strings.Repeat("█", filled) + strings.Repeat("░", barW-filled)

	pct := fmt.Sprintf("%.0f%%", buf.FillPercent)
	info := fmt.Sprintf("%s MiB/s", strings.TrimRight(strings.TrimRight(fmt.Sprintf("%.1f", buf.SpeedMbps), "0"), "."))
	if buf.EtaSecs > 0 {
		info += fmt.Sprintf("  ETA %ds", int(buf.EtaSecs))
	}
	if buf.PreRollSecs > 0 {
		info += fmt.Sprintf("  (pre-roll %ds)", int(buf.PreRollSecs))
	}

	line1 := acc.Render("⏳ "+label) + "  " + neon.Render(bar) + "  " + dim.Render(pct)
	line2 := dim.Render("   " + info)

	w := m.state.Width - 4
	if w < 44 {
		w = 44
	}
	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Padding(0, 1).
		Width(w).
		Render(line1 + "\n" + line2)

	return "\n" + box + "\n"
}

func (m Model) viewMainCard(footerHidden bool) string {
	focused := m.state.Focus != state.FocusSearch
	inner := m.viewMain()
	style := theme.T.MainCardStyle(focused).Width(m.state.Width - 2)
	if !footerHidden {
		style = style.MarginBottom(1)
	}
	return style.Render(inner)
}

func (m Model) viewMain() string {
	if m.state.ActiveTab == state.TabMusic {
		return m.musicScreen.View().Content
	}
	if m.state.ActiveTab == state.TabCollections {
		return m.collectionsScreen.View().Content
	}
	// Continue Watching row (Movies and Series tabs only)
	var cwSection string
	if items := m.cwCurrentItems(); len(items) > 0 {
		cwSection = renderContinueWatchingRow(items, m.cwCursor, m.cwFocused, m.innerWidth())
	}

	if m.screen == screenGrid || !m.state.SearchActive {
		availH := max(0, m.state.Height-12)
		grid := screens.RenderGrid(
			m.currentGridEntries(),
			m.gridCursor,
			m.innerWidth(),
			availH,
			m.state.IsLoading,
			m.state.LoadingStart,
			m.state.RuntimeStatus.String(),
			m.state.Plugins,
			&m.loadingSpinner,
		)
		if cwSection != "" {
			return lipgloss.JoinVertical(lipgloss.Left, cwSection, grid)
		}
		return grid
	}
	return lipgloss.JoinVertical(lipgloss.Left,
		m.viewColumnHeaders(),
		m.viewResults(),
	)
}

func (m Model) viewTopBar(focused bool) string {
	w := m.state.Width
	var tabParts []string
	for _, t := range state.Tabs() {
		s := fmt.Sprintf(" %s ", t.String())
		if t == m.state.ActiveTab {
			tabParts = append(tabParts, theme.T.TabActiveStyle().Render(s))
		} else {
			tabParts = append(tabParts, theme.T.TabStyle().Render(s))
		}
	}
	tabs := lipgloss.JoinHorizontal(lipgloss.Top, tabParts...)

	prefix := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Render("\u2315 ")
	var searchBox string
	switch {
	case m.state.Focus == state.FocusSearch:
		searchBox = theme.T.SearchFocusedStyle().Render(prefix + m.search.View())
	case m.search.Value() != "":
		searchBox = theme.T.SearchStyle().Render(prefix + lipgloss.NewStyle().Foreground(theme.T.Text()).Render(m.search.Value()))
	default:
		searchBox = theme.T.SearchStyle().Render(prefix + lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("Search\u2026  /"))
	}

	var gear string
	switch m.state.RuntimeStatus {
	case state.RuntimeError:
		gear = theme.T.GearStyle().Foreground(theme.T.Red()).Render("\u2699")
	case state.RuntimeReady:
		gear = theme.T.GearFocusedStyle().Render("\u2699")
	default:
		gear = theme.T.GearStyle().Render("\u2699")
	}

	tabsW := lipgloss.Width(tabs)
	searchW := lipgloss.Width(searchBox)
	gearW := lipgloss.Width(gear)
	contentW := w - 6
	spacerLeft := max(0, (contentW/2)-tabsW-(searchW/2))
	spacerRight := max(0, contentW-tabsW-searchW-gearW-spacerLeft)

	row := tabs + strings.Repeat(" ", spacerLeft) + searchBox + strings.Repeat(" ", spacerRight) + gear
	return theme.T.TopBarStyle(focused).Width(w - 2).Render(row)
}

func (m Model) viewColumnHeaders() string {
	w := m.innerWidth()
	col := func(s string, width int) string { return theme.T.ColHeaderStyle().Width(width).Render(s) }
	titleW := w/2 - 2
	yearW, genreW, ratingW := 6, 14, 8
	provW := max(10, w-titleW-yearW-genreW-ratingW-5)
	return lipgloss.JoinHorizontal(lipgloss.Top,
		col("Title", titleW), col("Year", yearW),
		col("Genre", genreW), col("Rating", ratingW),
		col("Provider", provW),
	)
}

func (m Model) viewResults() string {
	w := m.innerWidth()
	availH := max(1, m.state.Height-9)

	if len(m.state.Results) == 0 {
		return screens.CenteredMsg(w, availH, lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("No results"))
	}

	// Virtualized list for scrollbar
	vl := components.NewVirtualizedList(len(m.state.Results), m.state.Cursor, availH)
	scrollbar := vl.VerticalScrollbar(1, lipgloss.NewStyle().Foreground(theme.T.TextDim()))

	titleW := w/2 - 2
	yearW, genreW, ratingW := 6, 14, 8
	provW := max(10, w-titleW-yearW-genreW-ratingW-5)

	start, end := vl.VisibleRange()

	var rows []string
	for i := start; i < end; i++ {
		r := m.state.Results[i]
		row := fmt.Sprintf("%-*s  %-*s  %-*s  %-*s  %-*s",
			titleW-2, truncate(r.Title, titleW-2),
			yearW-1, truncate(r.Year, yearW-1),
			genreW-1, truncate(r.Genre, genreW-1),
			ratingW-1, truncate(r.Rating, ratingW-1),
			provW-1, truncate(r.Provider, provW-1),
		)
		var styled string
		switch {
		case i == m.state.Cursor && m.state.Focus == state.FocusResults:
			styled = theme.T.ResultRowSelectedStyle().Width(w - 2).Render(row)
		case i == m.state.Cursor:
			styled = theme.T.ResultRowHoveredStyle().Width(w - 2).Render(row)
		case i%2 == 0:
			styled = theme.T.ResultRowStyle().Width(w - 2).Render(row)
		default:
			styled = theme.T.ResultRowAltStyle().Width(w - 2).Render(row)
		}
		rows = append(rows, styled)
	}

	// Add scrollbar characters to each row
	if scrollbar != "" && len(rows) > 0 {
		scrollRunes := []rune(scrollbar)
		for i := range rows {
			scrollChar := " "
			if i < len(scrollRunes) {
				scrollChar = string(scrollRunes[i])
			}
			rows[i] = rows[i] + " " + scrollChar
		}
	}

	return theme.T.ResultsPanelStyle().Width(w).Height(availH).Render(strings.Join(rows, "\n"))
}

// viewFooter renders either the compact MPD now-playing bar (when playing)
// or the normal status bar (when stopped/paused). Same size either way.
func (m Model) viewFooter() string {
	if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
		return m.viewMpdFooter()
	}
	return m.viewStatusBar()
}

func fmtDuration(secs float64) string {
	m := int(secs) / 60
	s := int(secs) % 60
	return fmt.Sprintf("%d:%02d", m, s)
}

// viewMpdFooter renders a compact now-playing bar that fits in the footer slot.
func (m Model) viewMpdFooter() string {
	w := m.state.Width
	np := m.mpdNowPlaying
	accentStyle := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextMuted())
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())

	// State icon
	icon := accentStyle.Render("▶")

	// Artist - Title (truncated to fit)
	track := np.Title
	if np.Artist != "" {
		track = np.Artist + " — " + track
	}

	// Time
	elapsed := fmtDuration(np.Elapsed)
	total := fmtDuration(np.Duration)
	timeStr := dimStyle.Render(fmt.Sprintf(" %s/%s ", elapsed, total))

	// Volume
	volStr := dimStyle.Render(fmt.Sprintf(" %d%% ", np.Volume))

	// Seekbar — fill remaining space
	contentW := w - 8 // account for StatusBarStyle margins/padding/border
	fixedW := 2 + lipgloss.Width(timeStr) + lipgloss.Width(volStr) + 2 // icon + gaps
	trackMaxW := (contentW - fixedW) / 2
	if trackMaxW < 10 {
		trackMaxW = 10
	}
	if len([]rune(track)) > trackMaxW {
		track = string([]rune(track)[:trackMaxW-1]) + "…"
	}
	trackStr := textStyle.Render(" " + track + " ")

	barW := contentW - 2 - lipgloss.Width(trackStr) - lipgloss.Width(timeStr) - lipgloss.Width(volStr)
	if barW < 5 {
		barW = 5
	}
	var seekBar string
	if np.Duration > 0 {
		filled := int(np.Elapsed / np.Duration * float64(barW))
		if filled > barW {
			filled = barW
		}
		seekBar = accentStyle.Render(strings.Repeat("━", filled)) +
			dimStyle.Render(strings.Repeat("─", barW-filled))
	} else {
		seekBar = dimStyle.Render(strings.Repeat("─", barW))
	}

	bar := icon + trackStr + seekBar + timeStr + volStr
	return theme.T.StatusBarStyle().Width(w - 2).Render(bar)
}

func (m Model) viewStatusBar() string {
	w := m.state.Width

	var pill string
	switch m.state.RuntimeStatus {
	case state.RuntimeReady:
		pill = theme.T.StatusAccentStyle().Render(" stui ")
	case state.RuntimeConnecting:
		pill = theme.T.StatusAccentStyle().Background(theme.T.Yellow()).Render(" stui ")
	case state.RuntimeError:
		pill = theme.T.StatusAccentStyle().Background(theme.T.Red()).Render(" stui ")
	default:
		pill = theme.T.StatusAccentStyle().Background(theme.T.TextDim()).Render(" stui ")
	}

	var screenIndicator string
	switch m.screen {
	case screenGrid:
		screenIndicator = lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).Render("  \u25a6 grid")
	case screenList:
		screenIndicator = lipgloss.NewStyle().Foreground(theme.T.TextMuted()).Render("  \u2261 list")
	case screenDetail:
		screenIndicator = lipgloss.NewStyle().Foreground(theme.T.Neon()).Render("  \u25c8 detail")
	}

	// While the Music tab is active each sub-screen publishes its own
	// hint/status text; that supersedes the global StatusMsg slot so the
	// stale "Added X to queue" line from a previous action doesn't sit
	// in the footer forever (the sub-screens apply their own statusTTL
	// before reverting to a key-hint string).
	statusText := m.state.StatusMsg
	if m.state.ActiveTab == state.TabMusic {
		if t := m.musicScreen.FooterText(); t != "" {
			statusText = t
		}
	}
	statusMsg := lipgloss.NewStyle().Foreground(theme.T.TextMuted()).Render("  " + statusText)

	count := len(m.currentGridEntries())
	if m.screen == screenList {
		count = len(m.state.Results)
	}
	right := lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).
		Render(fmt.Sprintf("%s  %d titles  v toggle ", m.state.ActiveTab.String(), count))

	contentW := w - 8
	gap := max(0, contentW-lipgloss.Width(pill)-lipgloss.Width(screenIndicator)-lipgloss.Width(statusMsg)-lipgloss.Width(right))
	bar := pill + screenIndicator + statusMsg + strings.Repeat(" ", gap) + right
	return theme.T.StatusBarStyle().Width(w - 2).Render(bar)
}

// ── Data conversion helpers ───────────────────────────────────────────────────

func convertResults(items []ipc.MediaEntry) []state.ResultItem {
	out := make([]state.ResultItem, 0, len(items))
	for _, item := range items {
		r := state.ResultItem{ID: item.ID, Title: item.Title, Provider: item.Provider}
		if item.Year != nil {
			r.Year = *item.Year
		}
		if item.Genre != nil {
			r.Genre = *item.Genre
		}
		if item.Rating != nil {
			r.Rating = *item.Rating
		}
		out = append(out, r)
	}
	return out
}

func convertSearchToCatalog(items []ipc.MediaEntry) []ipc.CatalogEntry {
	out := make([]ipc.CatalogEntry, 0, len(items))
	for _, item := range items {
		out = append(out, ipc.CatalogEntry{
			ID:       item.ID,
			Title:    item.Title,
			Year:     item.Year,
			Genre:    item.Genre,
			Rating:   item.Rating,
			Provider: item.Provider,
			Tab:      string(item.Tab),
		})
	}
	return out
}

func listResultToCatalogEntry(r state.ResultItem, tab string) ipc.CatalogEntry {
	y, g, rt := r.Year, r.Genre, r.Rating
	return ipc.CatalogEntry{
		ID: r.ID, Title: r.Title,
		Year: &y, Genre: &g, Rating: &rt,
		Provider: r.Provider, Tab: tab,
	}
}

// enrichDetail populates fields that aren't available from the basic catalog.
// This is a placeholder until the real metadata endpoint is wired.
func enrichDetail(entry ipc.DetailEntry, providers []string) ipc.DetailEntry {
	if len(providers) > 0 && len(entry.Providers) == 0 {
		entry.Providers = providers
	}
	if len(entry.Cast) == 0 {
		entry.Cast = []ipc.CastMember{
			{Name: "Director", Role: "Director", RoleType: "crew"},
			{Name: "Lead Actor", Role: "Lead Role", RoleType: "cast"},
			{Name: "Supporting Actor", Role: "Supporting Role", RoleType: "cast"},
		}
	}
	return entry
}

func derefStr(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func truncate(s string, maxLen int) string {
	if maxLen <= 0 {
		return ""
	}
	runes := []rune(s)
	if len(runes) <= maxLen {
		return s
	}
	if maxLen <= 3 {
		return string(runes[:maxLen])
	}
	return string(runes[:maxLen-1]) + "\u2026"
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
