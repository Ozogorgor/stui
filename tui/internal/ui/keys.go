// keys.go — top-level keypress router. Dispatches by key (action-based
// where the action is bound) and by focus context. Detail-overlay
// keys delegate to handleDetailKey (keys_detail.go); collection
// picker keys delegate to handleCollectionPickerKey.

package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/watchhistory"
)

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
			t, cmd := components.ShowToast("Resolving streams…", false)
			m.activeToast = &t
			return m, cmd
		}
	}

	// ── Action-based dispatch (high-level intents, independent of key layout) ──
	// Skip when search is focused — letter keys should go to the textinput.
	if m.state.Focus == state.FocusSearch {
		// Jump straight to the search handler below.
	} else if action, ok := actions.FromKey(key); ok {
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
			if s := focusedSearchable(&m); s != nil {
				m.search.Placeholder = s.SearchPlaceholder()
				m.state.Focus = state.FocusSearch
				m.state.SearchActive = true
				return m, m.search.Focus()
			}
			// Focused screen is not Searchable — ignore the keystroke.
		case actions.ActionNextTab:
			// In detail view tab cycles focus zones (info → crew → cast →
			// providers → related), not the top tab bar. Fall through to
			// handleDetailKey instead of switching tabs.
			if m.screen == screenDetail && m.detail != nil {
				break
			}
			next := (int(m.state.ActiveTab) + 1) % len(state.Tabs())
			m.switchTab(state.Tab(next))
			if m.state.IsLoading {
				return m, m.loadingSpinner.Tick
			}
			return m, nil
		case actions.ActionPrevTab:
			if m.screen == screenDetail && m.detail != nil {
				break
			}
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

	// Manual catalog refresh — R works on any grid-style tab (Movies,
	// Series, Music). Wipes the runtime's in-mem SearchCache for the
	// active tab and re-dispatches the provider fan-out. Refreshed
	// entries arrive via the existing GridUpdate stream, so the
	// daemon's ack is fire-and-forget. Placed before the music tab
	// delegation block so it isn't swallowed by sub-tab key routing.
	if key == "R" && m.screen == screenGrid && m.client != nil {
		label := state.Tab(m.state.ActiveTab).String()
		m.client.CatalogRefresh(strings.ToLower(label))
		m.state.StatusMsg = fmt.Sprintf("Refreshing %s…", label)
		return m, nil
	}

	// ── Global player controls — active whenever mpv is running ───────────
	// All ambient-mode handlers below short-circuit when the search input
	// is focused, so the user can type letters that happen to be bound to
	// player/DSP/MPD actions ("d", "n", "p", "r", " ", etc.) without them
	// being intercepted as commands.
	activePlayer := m.nowPlaying
	if m.detail != nil && m.detail.NowPlaying != nil {
		activePlayer = m.detail.NowPlaying
	}
	// MPD pause: space toggles MPD pause from any screen when MPD is playing.
	if m.state.Focus != state.FocusSearch && m.mpdNowPlaying != nil && m.mpdNowPlaying.State != "stop" && m.client != nil {
		if action, ok := actions.FromKey(key); ok && action == actions.ActionPlayerPause {
			m.client.MpdCmd("mpd_toggle_pause", nil)
			return m, nil
		}
	}
	if m.state.Focus != state.FocusSearch && activePlayer != nil && m.client != nil {
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
				t, toastCmd := components.ShowToast("Switching to next stream…", false)
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
	// Yield to the search input when it has focus — otherwise letters in a
	// query like "dune" get swallowed by the `n` → next-track handler and
	// the user can't type at all.
	if m.state.Focus != state.FocusSearch && m.mpdNowPlaying != nil && m.client != nil {
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
	// Yield to the search input when it has focus; otherwise typing the
	// letter `d` (or c / b / r / D) in a query gets captured as a DSP
	// toggle / convolution switch, and the user can't complete their search.
	if m.state.Focus != state.FocusSearch && m.dspState != nil && m.client != nil {
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
			// Restore the focused screen's pre-search state if it is Searchable.
			if s := focusedSearchable(&m); s != nil {
				m.applyRestoreView()
			}
			m.state.Focus = state.FocusTabs
			m.search.Blur()
			m.search.Reset()
			m.state.SearchActive = false
			m.screen = screenGrid
			return m, nil
		case "enter":
			query := m.search.Value()
			m.state.SearchQuery = query
			if query == "" {
				// Empty query — restore view and dismiss bar.
				if s := focusedSearchable(&m); s != nil {
					m.applyRestoreView()
				}
				m.state.Focus = state.FocusTabs
				m.search.Blur()
				m.search.Reset()
				m.state.SearchActive = false
				return m, nil
			}
			// Dispatch search through the focused Searchable. The search
			// bar is only ever activated when focusedSearchable is non-nil
			// (see the `/` / ActionOpenSearch handlers), so the nil case
			// here is defensive — fall through silently.
			if s := focusedSearchable(&m); s != nil {
				m.state.StatusMsg = fmt.Sprintf("Searching for “%s”…", query)
				// Hand focus back to the grid so j/k/arrows/enter
				// navigate the streamed results instead of being
				// re-captured by the textinput. The query string stays
				// visible in the bar so the user can see what they
				// searched for; pressing `/` re-focuses for refinement.
				m.search.Blur()
				m.state.Focus = state.FocusTabs
				m.state.SearchActive = false
				return m, s.StartSearch(query)
			}
			m.state.Focus = state.FocusTabs
			m.search.Blur()
			m.search.Reset()
			m.state.SearchActive = false
			return m, nil
		default:
			var cmd tea.Cmd
			m.search, cmd = m.search.Update(msg)
			// Debounce live typing: increment the token and schedule a 150ms
			// tick. If the user types again before the tick fires, the token
			// advances and the earlier tick is silently dropped in the
			// searchDebounceFireMsg handler.
			if focusedSearchable(&m) != nil {
				m.searchDebounceToken++
				tok := m.searchDebounceToken
				debounceCmd := tea.Tick(150*time.Millisecond, func(time.Time) tea.Msg {
					return searchDebounceFireMsg{token: tok}
				})
				return m, tea.Batch(cmd, debounceCmd)
			}
			return m, cmd
		}
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
				m.state.StatusMsg = fmt.Sprintf("Resuming %s…", entry.Title)
				m.client.PlayFrom(entry.ID, entry.Provider, entry.ImdbID, tab, entry.Position)
				return m, nil
			}
			idx := m.gridCursor.Index(components.CardColumns)
			if idx >= 0 && idx < len(entries) {
				return m, m.openDetail(entries[idx])
			}
			return m, nil
		case "p":
			// Direct play: open detail (so the user has somewhere to land
			// when they Esc out of the picker) AND open the stream picker
			// in one keypress. The MPD prev-track `p` higher up only fires
			// when mpdNowPlaying != nil, so this branch is only reached
			// when no MPD audio is active — no conflict.
			if m.cwFocused || m.client == nil {
				return m, nil
			}
			idx := m.gridCursor.Index(components.CardColumns)
			if idx < 0 || idx >= len(entries) {
				return m, nil
			}
			entry := entries[idx]
			detailCmd := m.openDetail(entry)
			pickerCmd := screen.TransitionCmd(
				screens.NewStreamPickerScreen(m.client, entry.Title, entry.ID, m.state.Settings.BenchmarkStreams),
				true,
			)
			return m, tea.Batch(detailCmd, pickerCmd)
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
	case "ctrl+c":
		if m.client != nil {
			if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
				m.client.MpdCmd("mpd_stop", nil)
			}
			m.client.Stop()
		}
		return m, tea.Quit
	case "q":
		// `q` is only a quit shortcut when the search bar isn't focused —
		// otherwise typing a query that contains 'q' (e.g. "squid game",
		// "american queer", "the queen") would kill the app mid-word.
		// ctrl+c above remains a hard quit from any focus.
		if m.state.Focus != state.FocusSearch {
			if m.client != nil {
				if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
					m.client.MpdCmd("mpd_stop", nil)
				}
				m.client.Stop()
			}
			return m, tea.Quit
		}
	case "/":
		// Activate the inline search bar only when the focused screen is
		// Searchable. Non-Searchable screens ignore the keystroke entirely.
		if s := focusedSearchable(&m); s != nil {
			m.search.Placeholder = s.SearchPlaceholder()
			m.state.Focus = state.FocusSearch
			m.state.SearchActive = true
			return m, m.search.Focus()
		}
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
