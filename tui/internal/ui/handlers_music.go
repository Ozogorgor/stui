// handlers_music.go — Update msg handlers for the music tab (MPD
// status/queue, DSP status, visualizer ticks/cycles) plus the
// computeMusicHeight helper. Includes two cases (MpdSearchResult,
// MpdQueueChanged) that physically sit outside the music region of
// Update but belong here logically.

package ui

import (
	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/config"
	"github.com/stui/stui/pkg/log"
)

// handleMpdSearchResult handles ipc.MpdSearchResult.
// Synchronous MPD-backed search results — route to focused Searchable.
// MPD searches are Music-only, so no grid branch is needed here.
func (m Model) handleMpdSearchResult(msg ipc.MpdSearchResult) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	m.musicScreen, cmd = m.musicScreen.ApplyMpdSearchResult(msg)
	return m, cmd
}

// handleVisualizerTick handles components.VisualizerTickMsg.
func (m Model) handleVisualizerTick(msg components.VisualizerTickMsg) (tea.Model, tea.Cmd) {
	// Keep the animation loop alive while the visualizer is running
	if m.visualizer.IsRunning() {
		return m, m.visualizer.TickCmd()
	}
	return m, nil
}

// handleVisualizerErr handles components.VisualizerErrMsg.
func (m Model) handleVisualizerErr(msg components.VisualizerErrMsg) (tea.Model, tea.Cmd) {
	t, cmd := components.ShowToast("Visualizer error: "+msg.Err.Error(), true)
	m.activeToast = &t
	return m, cmd
}

// handleMpdElapsedTick handles mpdElapsedTickMsg.
func (m Model) handleMpdElapsedTick(msg mpdElapsedTickMsg) (tea.Model, tea.Cmd) {
	if m.mpdNowPlaying != nil && m.mpdNowPlaying.State == "play" {
		m.mpdNowPlaying.Elapsed += 1.0
		if m.mpdNowPlaying.Duration > 0 && m.mpdNowPlaying.Elapsed > m.mpdNowPlaying.Duration {
			m.mpdNowPlaying.Elapsed = m.mpdNowPlaying.Duration
		}
		return m, mpdElapsedTickCmd()
	}
	return m, nil
}

// handleMpdStatus handles ipc.MpdStatusMsg.
func (m Model) handleMpdStatus(msg ipc.MpdStatusMsg) (tea.Model, tea.Cmd) {
	var musicCmd tea.Cmd
	m.musicScreen, musicCmd = m.musicScreen.Update(msg) // keep queue highlight in sync
	if msg.State == "stop" && (m.mpdNowPlaying == nil || m.mpdNowPlaying.State == "stop") {
		// Already stopped — skip unnecessary alloc
		return m, nil
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
		if musicCmd != nil {
			cmds = append(cmds, musicCmd)
		}
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
	// If we didn't return above, still propagate the music screen's Cmd.
	if musicCmd != nil {
		return m, musicCmd
	}
	return m, nil
}

// handleMpdOutputsResult handles ipc.MpdOutputsResultMsg.
func (m Model) handleMpdOutputsResult(msg ipc.MpdOutputsResultMsg) (tea.Model, tea.Cmd) {
	if msg.Err != nil {
		m.state.StatusMsg = "MPD outputs error: " + msg.Err.Error()
	}
	// Outputs are displayed in a future MPD outputs overlay screen.
	return m, nil
}

// handleDspStatus handles ipc.DspStatusMsg.
func (m Model) handleDspStatus(msg ipc.DspStatusMsg) (tea.Model, tea.Cmd) {
	if m.dspState == nil {
		m.dspState = &components.DspState{}
	}
	m.dspState.Update(msg)
	return m, nil
}

// handleDspBoundToMpd handles ipc.DspBoundToMpdMsg.
func (m Model) handleDspBoundToMpd(msg ipc.DspBoundToMpdMsg) (tea.Model, tea.Cmd) {
	if msg.Success {
		m.state.StatusMsg = "DSP bound to MPD"
	} else {
		m.state.StatusMsg = "DSP bind failed"
	}
	return m, nil
}

// handleVizCycleBackend handles screens.VizCycleBackendMsg.
func (m Model) handleVizCycleBackend(msg screens.VizCycleBackendMsg) (tea.Model, tea.Cmd) {
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
}

// handleVizCycleMode handles screens.VizCycleModeMsg.
func (m Model) handleVizCycleMode(msg screens.VizCycleModeMsg) (tea.Model, tea.Cmd) {
	cfg := m.visualizer.Config()
	// Modes are iota 0..N; cycle using the string map's length.
	const numModes = 21 // Bars..Bricks (must match visualizer.go enum)
	cfg.Mode = components.VisualizerMode((int(cfg.Mode) + 1) % numModes)
	m.cfg.Visualizer.Mode = cfg.Mode.String()
	if err := config.Save(m.cfgPath, m.cfg); err != nil {
		log.Warn("config save failed", "key", "visualizer.mode", "error", err)
	}
	return m, m.visualizer.Reconfigure(cfg)
}

// handleMpdQueueResult handles ipc.MpdQueueResultMsg.
func (m Model) handleMpdQueueResult(msg ipc.MpdQueueResultMsg) (tea.Model, tea.Cmd) {
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
}

// handleMpdQueueChanged handles the tied case for music sub-tab data
// messages: ipc.MpdQueueChangedMsg, ipc.MpdDirResultMsg,
// ipc.MpdLibraryResultMsg, ipc.MpdPlaylistsResultMsg,
// ipc.MpdPlaylistTracksResultMsg.
func (m Model) handleMpdQueueChanged(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	m.musicScreen, cmd = m.musicScreen.Update(msg)
	return m, cmd
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
