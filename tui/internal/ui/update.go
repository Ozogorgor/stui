// update.go — Bubbletea Update dispatcher. The outer switch routes
// each msg type to its topical handler in handlers_*.go (and the
// special-case handlers in mouse.go / keys.go / keys_detail.go).
// Trivial cases (1-line unwraps and dispatch-to-other-handler)
// stay inlined here.

package ui

import (
	"fmt"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/components/poster"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/config"
	"github.com/stui/stui/pkg/log"
)

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
		return m.handleWindowSize(msg)

	// ── Runtime lifecycle ─────────────────────────────────────────────────

	case runtimeStartedMsg:
		return m.handleRuntimeStarted(msg)

	case ipc.RuntimeReadyMsg:
		return m.handleRuntimeReady(msg)

	case ipc.RuntimeErrorMsg:
		return m.handleRuntimeError(msg)

	case ipc.IPCVersionMismatchMsg:
		return m.handleIPCVersionMismatch(msg)

	case ipc.StatusMsg:
		return m.handleStatus(msg)

	// ── Plugin events ─────────────────────────────────────────────────────

	case ipc.PluginLoadedMsg:
		return m.handlePluginLoaded(msg)

	case ipc.PluginListMsg:
		return m.handlePluginList(msg)

	case ipc.PluginToastMsg:
		return m.handlePluginToast(msg)

	case ipc.SubtitleFetchedMsg:
		return m.handleSubtitleFetched(msg)

	case ipc.SubtitleSearchFailedMsg:
		return m.handleSubtitleSearchFailed(msg)

	case poster.PostersUpdatedMsg:
		return m.handlePostersUpdated(msg)

	case components.ChafaRenderedMsg:
		// Async chafa render finished. The worker has already written
		// the result to the disk cache; the next View() pass will hit
		// L2 and replace the placeholder. We just need to re-subscribe
		// to the next render notification — Bubbletea will trigger a
		// re-render naturally.
		return m, components.ChafaPollCmd()

	case components.ToastDismissMsg:
		return m.handleToastDismiss(msg)

	// ── Catalog grid ──────────────────────────────────────────────────────

	case ipc.CatalogStaleMsg:
		return m.handleCatalogStale(msg)

	case ipc.GridUpdateMsg:
		return m.handleGridUpdate(msg)

	// ── Search results ────────────────────────────────────────────────────

	// Streaming plugin-backed scope results — route to focused Searchable.
	// Music sub-screens consume ipc.ScopeResultsMsg directly via
	// MusicScreen.ApplyScopeResults. Grid tabs (Movies/Series/Library)
	// receive their scope data via gridScopeAppliedMsg instead — the
	// streaming loop in readNextGridScope wraps each channel read into a
	// gridScopeAppliedMsg, so ipc.ScopeResultsMsg itself is Music-only.
	case ipc.ScopeResultsMsg:
		return m.handleScopeResults(msg)

	// Synchronous MPD-backed search results — route to focused Searchable.
	// MPD searches are Music-only, so no grid branch is needed here.
	case ipc.MpdSearchResult:
		return m.handleMpdSearchResult(msg)

	// Streaming grid search scope results — emitted by readNextGridScope.
	// We drop stale results (mismatched QueryID) but keep draining the
	// channel so it closes cleanly. For Movies/Series (single scope) we
	// overwrite m.grids[tab]; for Library (Movie+Series) we accumulate by
	// replacing only the per-scope slice of entries.
	case gridScopeAppliedMsg:
		return m.handleGridScopeApplied(msg)

	case gridSearchClosedMsg:
		return m.handleGridSearchClosed(msg)

	case gridSearchFailedMsg:
		return m.handleGridSearchFailed(msg)

	// Person-mode search result — feeds the cast-member overlay.
	// Produced by dispatchPersonSearch (no-runtime local path).
	// TODO(Task 7.0): migrate dispatchPersonSearch to streaming ScopeResults.
	case ipc.SearchResultMsg:
		return m.handleSearchResult(msg)

	case ipc.EpisodesLoadedMsg:
		return m.handleEpisodesLoaded(msg)

	case ipc.BingeContextMsg:
		return m.handleBingeContext(msg)

	case ipc.StreamsResolvedMsg:
		return m.handleStreamsResolved(msg)

	// ── Collections ───────────────────────────────────────────────────────

	case screens.CollectionOpenDetailMsg:
		return m.handleCollectionOpenDetail(msg)

	// ── Detail data ───────────────────────────────────────────────────────

	case ipc.DetailReadyMsg:
		return m.handleDetailReady(msg)

	case ipc.DetailMetadataPartial:
		return m.handleDetailMetadataPartial(msg)

	// ── Live theme update from matugen watcher ───────────────────────────
	case ipc.ThemeUpdateMsg:
		return m.handleThemeUpdate(msg)

	// ── Config file reload (hot-reload from watcher) ──────────────────────
	case config.ConfigReloadMsg:
		return m.handleConfigReload(msg)

	// ── Visualizer ────────────────────────────────────────────────────────

	case components.VisualizerTickMsg:
		return m.handleVisualizerTick(msg)

	case components.VisualizerErrMsg:
		return m.handleVisualizerErr(msg)

	// ── Player events ─────────────────────────────────────────────────────
	case ipc.PlayerTracksUpdatedMsg:
		return m.handlePlayerTracksUpdated(msg)

	case ipc.PlayerStartedMsg:
		return m.handlePlayerStarted(msg)

	case ipc.PlayerBufferingMsg:
		return m.handlePlayerBuffering(msg)

	case ipc.PlayerBufferReadyMsg:
		return m.handlePlayerBufferReady(msg)

	case ipc.PlayerProgressMsg:
		return m.handlePlayerProgress(msg)

	case ipc.PlayerTerminalTakeoverMsg:
		return m.handlePlayerTerminalTakeover(msg)

	case ipc.PlayerEndedMsg:
		return m.handlePlayerEnded(msg)

	case syncHideMsg:
		return m.handleSyncHide(msg)

	// ── Torrent download events ────────────────────────────────────────────

	case ipc.DownloadStartedMsg:
		return m.handleDownloadStarted(msg)

	case ipc.DownloadProgressMsg:
		return m.handleDownloadProgress(msg)

	case ipc.DownloadCompleteMsg:
		return m.handleDownloadComplete(msg)

	case ipc.DownloadErrorMsg:
		return m.handleDownloadError(msg)

	case bingeTickMsg:
		return m.handleBingeTick(msg)

	case configSaveTickMsg:
		return m.handleConfigSaveTick(msg)

	case searchDebounceFireMsg:
		return m.handleSearchDebounceFire(msg)

	case spinner.TickMsg:
		return m.handleSpinnerTick(msg)

	// ── MPD audio events ──────────────────────────────────────────────────

	case mpdElapsedTickMsg:
		return m.handleMpdElapsedTick(msg)

	case ipc.MpdStatusMsg:
		return m.handleMpdStatus(msg)

	case ipc.MpdOutputsResultMsg:
		return m.handleMpdOutputsResult(msg)

	// ── DSP events ────────────────────────────────────────────────────────

	case ipc.DspStatusMsg:
		return m.handleDspStatus(msg)

	case ipc.DspBoundToMpdMsg:
		return m.handleDspBoundToMpd(msg)

	// ── Visualizer cycle hotkeys (from queue screen) ────────────────────

	case screens.VizCycleBackendMsg:
		return m.handleVizCycleBackend(msg)

	case screens.VizCycleModeMsg:
		return m.handleVizCycleMode(msg)

	// ── Settings changes ─────────────────────────────────────────────────

	case screens.SettingsChangedMsg:
		return m.handleSettingsChanged(msg)

	// ── Plugin settings screen ────────────────────────────────────────────

	case screens.OpenStreamRadarMsg:
		return m.handleOpenStreamRadar(msg)

	case screens.OpenRatingWeightsMsg:
		return m.handleOpenRatingWeights(msg)

	case screens.OpenOfflineLibraryMsg:
		return m.handleOpenOfflineLibrary(msg)

	case screens.OfflineOpenDetailMsg:
		return m.handleOfflineOpenDetail(msg)

	case screens.ClearMediaCacheMsg:
		return m.handleClearMediaCache(msg)

	case screens.OpenPluginManagerMsg:
		return m.handleOpenPluginManager(msg)

	case screens.OpenPluginSettingsMsg:
		return m.handleOpenPluginSettings(msg)

	case screens.OpenPluginReposMsg:
		return m.handleOpenPluginRepos(msg)

	case screens.OpenPluginRegistryMsg:
		return m.handleOpenPluginRegistry(msg)

	case screens.OpenKeybindsEditorMsg:
		return m.handleOpenKeybindsEditor(msg)

	// ── Skip detection ────────────────────────────────────────────────────

	case ipc.SkipSegmentMsg:
		return m.handleSkipSegment(msg)

	// ── Music sub-tab data messages ───────────────────────────────────────

	case ipc.MpdQueueResultMsg:
		return m.handleMpdQueueResult(msg)

	case ipc.MpdQueueChangedMsg,
		ipc.MpdDirResultMsg, ipc.MpdLibraryResultMsg,
		ipc.MpdPlaylistsResultMsg, ipc.MpdPlaylistTracksResultMsg:
		return m.handleMpdQueueChanged(msg)

	// ── Keyboard ──────────────────────────────────────────────────────────

	case tea.MouseMsg:
		return m.handleMouse(msg)

	case tea.KeyPressMsg:
		return m.handleKey(msg)

	default:
		// Forward unhandled messages (e.g. seekTickMsg) to the music screen
		// so sub-screen ticks and custom messages aren't silently dropped.
		if m.state.ActiveTab == state.TabMusic {
			var cmd tea.Cmd
			m.musicScreen, cmd = m.musicScreen.Update(msg)
			return m, cmd
		}
	}

	return m, nil
}
