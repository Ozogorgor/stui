// handlers_system.go — Update msg handlers for system / lifecycle
// (window size, runtime/IPC bootstrap, plugin lifecycle, theme &
// config, settings change, screen-routing Open*Msg) plus
// sessionSaveCmd helper.

package ui

import (
	"fmt"
	"strings"
	"time"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/bidi"
	"github.com/stui/stui/pkg/config"
	"github.com/stui/stui/pkg/log"
	"github.com/stui/stui/pkg/mediacache"
	"github.com/stui/stui/pkg/session"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

// handleWindowSize handles tea.WindowSizeMsg.
func (m Model) handleWindowSize(msg tea.WindowSizeMsg) (tea.Model, tea.Cmd) {
	m.state.Width = msg.Width
	m.state.Height = msg.Height
	m.search.SetWidth(max(20, m.innerWidth()/3))
	innerMsg := tea.WindowSizeMsg{Width: m.innerWidth(), Height: m.computeMusicHeight()}
	m.musicScreen, _ = m.musicScreen.Update(innerMsg)
	m.collectionsScreen = m.collectionsScreen.SetSize(m.innerWidth(), max(0, msg.Height-12))
	return m, nil
}

// handleRuntimeStarted handles runtimeStartedMsg.
func (m Model) handleRuntimeStarted(msg runtimeStartedMsg) (tea.Model, tea.Cmd) {
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
	// Emit a RuntimeReadyMsg so the splash-screen wrapper (cmd/stui)
	// can advance the progress bar at the runtime-handshake milestone.
	// In --no-runtime mode this same message is emitted directly from
	// Init() instead — handleRuntimeReady (below) is the unified
	// handler for both paths.
	runtimeReady := func() tea.Msg { return ipc.RuntimeReadyMsg{} }
	// Start spinner tick if still loading
	if m.state.IsLoading {
		return m, tea.Batch(m.loadingSpinner.Tick, musicInitCmd, listenIPC(m.client.Chan()), runtimeReady)
	}
	return m, tea.Batch(musicInitCmd, listenIPC(m.client.Chan()), runtimeReady)
}

// handleRuntimeReady handles ipc.RuntimeReadyMsg. In --no-runtime mode
// this is the only "runtime is ready" signal; in production mode it
// runs after handleRuntimeStarted has already set RuntimeStatus +
// StatusMsg, so we only override StatusMsg when there's actually a
// different message to set (dev mode).
func (m Model) handleRuntimeReady(msg ipc.RuntimeReadyMsg) (tea.Model, tea.Cmd) {
	m.state.RuntimeStatus = state.RuntimeReady
	if m.opts.NoRuntime {
		m.state.StatusMsg = "Ready (dev mode)"
	}
	return m, nil
}

// handleRuntimeError handles ipc.RuntimeErrorMsg.
func (m Model) handleRuntimeError(msg ipc.RuntimeErrorMsg) (tea.Model, tea.Cmd) {
	m.state.RuntimeStatus = state.RuntimeError
	offlineHint := ""
	if m.mediaCache != nil && m.mediaCache.TotalCount() > 0 {
		offlineHint = fmt.Sprintf(" — press O for offline library (%d cached)", m.mediaCache.TotalCount())
	}
	m.state.StatusMsg = fmt.Sprintf("Runtime error: %v%s", msg.Err, offlineHint)
	return m, nil
}

// handleIPCVersionMismatch handles ipc.IPCVersionMismatchMsg.
func (m Model) handleIPCVersionMismatch(msg ipc.IPCVersionMismatchMsg) (tea.Model, tea.Cmd) {
	m.state.StatusMsg = fmt.Sprintf(
		"⚠ IPC version mismatch: TUI=%d runtime=%d (v%s) — consider upgrading",
		msg.TUIVersion, msg.RuntimeVersion, msg.RuntimeSemver,
	)
	return m, nil
}

// handleStatus handles ipc.StatusMsg.
func (m Model) handleStatus(msg ipc.StatusMsg) (tea.Model, tea.Cmd) {
	m.state.StatusMsg = msg.Text
	return m, nil
}

// handlePluginLoaded handles ipc.PluginLoadedMsg.
func (m Model) handlePluginLoaded(msg ipc.PluginLoadedMsg) (tea.Model, tea.Cmd) {
	if msg.Err != nil {
		m.state.StatusMsg = fmt.Sprintf("Plugin load failed: %v", msg.Err)
	} else {
		m.state.StatusMsg = fmt.Sprintf("Plugin loaded: %s", msg.Name)
	}
	return m, nil
}

// handlePluginList handles ipc.PluginListMsg.
func (m Model) handlePluginList(msg ipc.PluginListMsg) (tea.Model, tea.Cmd) {
	if msg.Err == nil {
		m.state.Plugins = make([]string, 0, len(msg.Plugins))
		for _, p := range msg.Plugins {
			m.state.Plugins = append(m.state.Plugins, p.Name)
		}
	}
	return m, nil
}

// handlePluginToast handles ipc.PluginToastMsg.
func (m Model) handlePluginToast(msg ipc.PluginToastMsg) (tea.Model, tea.Cmd) {
	t, cmd := components.ShowToast(msg.Message, msg.IsError)
	m.activeToast = &t
	if msg.IsError {
		m.state.StatusMsg = "Plugin error: " + msg.PluginName
	} else {
		m.state.StatusMsg = "Plugin loaded: " + msg.PluginName + " v" + msg.Version
		m.state.Plugins = append(m.state.Plugins, msg.PluginName)
	}
	return m, cmd
}

// handleToastDismiss handles components.ToastDismissMsg.
func (m Model) handleToastDismiss(msg components.ToastDismissMsg) (tea.Model, tea.Cmd) {
	m.activeToast = nil
	return m, nil
}

// handleThemeUpdate handles ipc.ThemeUpdateMsg.
func (m Model) handleThemeUpdate(msg ipc.ThemeUpdateMsg) (tea.Model, tea.Cmd) {
	palette := theme.FromMatugen(msg.Colors)
	theme.T.Apply(palette)
	m.state.StatusMsg = "Theme updated from matugen"
	return m, func() tea.Msg { return nil }
}

// handleConfigReload handles config.ConfigReloadMsg.
func (m Model) handleConfigReload(msg config.ConfigReloadMsg) (tea.Model, tea.Cmd) {
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
}

// handleRainbowTick advances the package-level RainbowOffset the focused
// grid card's border reads each frame. Re-arms unconditionally so the
// animation keeps flowing — View() picks it up on the next render
// regardless of which screen is active, and off-screen the cost is just
// one Update + one scheduled callback per tick.
func (m Model) handleRainbowTick(_ rainbowTickMsg) (tea.Model, tea.Cmd) {
	// 6 deg / tick at 100ms = 60 ticks per full rotation = 6 sec cycle.
	// Slow enough to read as a calm flow, fast enough to feel alive.
	components.RainbowOffset = (components.RainbowOffset + 6) % 360
	return m, rainbowTickCmd()
}

// handleConfigSaveTick handles configSaveTickMsg.
func (m Model) handleConfigSaveTick(msg configSaveTickMsg) (tea.Model, tea.Cmd) {
	if msg.seq != m.cfgSaveSeq {
		return m, nil
	}
	if m.watcher != nil {
		m.watcher.NotifyWrite()
	}
	_ = config.Save(m.cfgPath, m.cfg)
	return m, nil
}

// handleSpinnerTick handles spinner.TickMsg.
func (m Model) handleSpinnerTick(msg spinner.TickMsg) (tea.Model, tea.Cmd) {
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
}

// handleSettingsChanged handles screens.SettingsChangedMsg.
func (m Model) handleSettingsChanged(msg screens.SettingsChangedMsg) (tea.Model, tea.Cmd) {
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
}

// handleOpenStreamRadar handles screens.OpenStreamRadarMsg.
func (m Model) handleOpenStreamRadar(msg screens.OpenStreamRadarMsg) (tea.Model, tea.Cmd) {
	return m, screen.TransitionCmd(screens.NewStreamRadarScreen(m.streamStats), true)
}

// handleOpenRatingWeights handles screens.OpenRatingWeightsMsg.
// Seeds the editor with the user's current per-source weights so
// edits start from the live state rather than empty defaults.
func (m Model) handleOpenRatingWeights(msg screens.OpenRatingWeightsMsg) (tea.Model, tea.Cmd) {
	weights := m.cfg.Providers.RatingSourceWeights
	if weights == nil {
		weights = map[string]float64{}
	}
	return m, screen.TransitionCmd(screens.NewRatingWeightsScreen(weights), true)
}

// handleOpenMetadataSources handles screens.OpenMetadataSourcesMsg.
// Hands the IPC client to the new screen so it can query the runtime
// for per-kind plugin lists; the screen owns the IPC round-trips.
func (m Model) handleOpenMetadataSources(msg screens.OpenMetadataSourcesMsg) (tea.Model, tea.Cmd) {
	return m, screen.TransitionCmd(screens.NewMetadataSourcesScreen(m.client), true)
}

// handleOpenOfflineLibrary handles screens.OpenOfflineLibraryMsg.
func (m Model) handleOpenOfflineLibrary(msg screens.OpenOfflineLibraryMsg) (tea.Model, tea.Cmd) {
	if m.mediaCache != nil {
		return m, screen.TransitionCmd(screens.NewOfflineLibraryScreen(m.mediaCache), true)
	}
	return m, nil
}

// handleOfflineOpenDetail handles screens.OfflineOpenDetailMsg.
func (m Model) handleOfflineOpenDetail(msg screens.OfflineOpenDetailMsg) (tea.Model, tea.Cmd) {
	return m, m.openDetail(msg.Entry)
}

// handleClearMediaCache handles screens.ClearMediaCacheMsg.
func (m Model) handleClearMediaCache(msg screens.ClearMediaCacheMsg) (tea.Model, tea.Cmd) {
	if m.mediaCache != nil {
		_ = m.mediaCache.Clear()
		m.state.StatusMsg = "Media cache cleared"
	}
	return m, nil
}

// handleOpenPluginManager handles screens.OpenPluginManagerMsg.
func (m Model) handleOpenPluginManager(msg screens.OpenPluginManagerMsg) (tea.Model, tea.Cmd) {
	if m.client == nil {
		m.state.StatusMsg = "Runtime not ready — try again shortly."
		return m, nil
	}
	return m, screen.TransitionCmd(screens.NewPluginManagerScreen(m.client), true)
}

// handleOpenPluginSettings handles screens.OpenPluginSettingsMsg.
func (m Model) handleOpenPluginSettings(msg screens.OpenPluginSettingsMsg) (tea.Model, tea.Cmd) {
	if m.client == nil {
		m.state.StatusMsg = "Runtime not ready — try again shortly."
		return m, nil
	}
	return m, screen.TransitionCmd(screens.NewPluginSettingsScreen(m.client), true)
}

// handleOpenPluginRepos handles screens.OpenPluginReposMsg.
func (m Model) handleOpenPluginRepos(msg screens.OpenPluginReposMsg) (tea.Model, tea.Cmd) {
	if m.client == nil {
		m.state.StatusMsg = "Runtime not ready — try again shortly."
		return m, nil
	}
	return m, screen.TransitionCmd(screens.NewPluginReposScreen(m.client), true)
}

// handleOpenPluginRegistry handles screens.OpenPluginRegistryMsg.
func (m Model) handleOpenPluginRegistry(msg screens.OpenPluginRegistryMsg) (tea.Model, tea.Cmd) {
	if m.client == nil {
		m.state.StatusMsg = "Runtime not ready — try again shortly."
		return m, nil
	}
	return m, screen.TransitionCmd(screens.NewPluginRegistryScreen(m.client), true)
}

// handleOpenKeybindsEditor handles screens.OpenKeybindsEditorMsg.
func (m Model) handleOpenKeybindsEditor(msg screens.OpenKeybindsEditorMsg) (tea.Model, tea.Cmd) {
	return m, screen.TransitionCmd(screens.NewKeybindsEditorScreen(), true)
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
