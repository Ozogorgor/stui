// handlers_player.go — Update msg handlers for playback (mpv
// lifecycle, buffering, progress, subtitle/audio sync, binge mode,
// skip segments) plus their helpers (subSyncState, playBingeNext,
// activeNowPlaying).

package ui

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screens"
	"github.com/stui/stui/pkg/notify"
	"github.com/stui/stui/pkg/watchhistory"
)

// handleSubtitleFetched handles ipc.SubtitleFetchedMsg.
func (m Model) handleSubtitleFetched(msg ipc.SubtitleFetchedMsg) (tea.Model, tea.Cmd) {
	t, cmd := components.ShowToast(
		fmt.Sprintf("Subtitle: %s · %s", msg.Language, msg.Provider),
		false,
	)
	m.activeToast = &t
	return m, cmd
}

// handleSubtitleSearchFailed handles ipc.SubtitleSearchFailedMsg.
func (m Model) handleSubtitleSearchFailed(msg ipc.SubtitleSearchFailedMsg) (tea.Model, tea.Cmd) {
	t, cmd := components.ShowToast(
		fmt.Sprintf("Subtitle search failed: %s", msg.Reason),
		true,
	)
	m.activeToast = &t
	return m, cmd
}

// handleBingeContext handles ipc.BingeContextMsg.
func (m Model) handleBingeContext(msg ipc.BingeContextMsg) (tea.Model, tea.Cmd) {
	// Store binge context whenever an episode is played from EpisodeScreen.
	// Countdown only fires if BingeEnabled is true (toggled with 'b' in EpisodeScreen).
	m.bingeCtx = &msg
	m.bingeCountdown = -1
	return m, nil
}

// handleStreamsResolved handles ipc.StreamsResolvedMsg.
func (m Model) handleStreamsResolved(msg ipc.StreamsResolvedMsg) (tea.Model, tea.Cmd) {
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
}

// handlePlayerTracksUpdated handles ipc.PlayerTracksUpdatedMsg.
func (m Model) handlePlayerTracksUpdated(msg ipc.PlayerTracksUpdatedMsg) (tea.Model, tea.Cmd) {
	m.playerTracks = msg.Tracks
	return m, nil
}

// handlePlayerStarted handles ipc.PlayerStartedMsg.
func (m Model) handlePlayerStarted(msg ipc.PlayerStartedMsg) (tea.Model, tea.Cmd) {
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
	m.state.StatusMsg = "▶ Playing: " + msg.Title
	if m.notifyCfg.OnPlayback {
		notify.Send(m.notifyCfg, "▶ Now Playing", msg.Title, notify.UrgencyLow)
	}
	return m, nil
}

// handlePlayerBuffering handles ipc.PlayerBufferingMsg.
func (m Model) handlePlayerBuffering(msg ipc.PlayerBufferingMsg) (tea.Model, tea.Cmd) {
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
		m.state.StatusMsg = fmt.Sprintf("⏸ Buffering… %.0f%%  %.1f MB/s", msg.FillPercent, msg.SpeedMbps)
	} else {
		m.state.StatusMsg = fmt.Sprintf("⏳ Pre-roll %.0f%%  %.1f MB/s  ETA %.0fs", msg.FillPercent, msg.SpeedMbps, msg.EtaSecs)
	}
	return m, nil
}

// handlePlayerBufferReady handles ipc.PlayerBufferReadyMsg.
func (m Model) handlePlayerBufferReady(msg ipc.PlayerBufferReadyMsg) (tea.Model, tea.Cmd) {
	m.playerBuffer = nil
	np := m.activeNowPlaying()
	if np != nil {
		np.Buffering = false
	}
	m.state.StatusMsg = fmt.Sprintf("▶ Ready — %.0fs buffered  slack %.2f×  %.1f MB/s",
		msg.PreRollSecs, msg.Slack, msg.SpeedMbps)
	return m, nil
}

// handlePlayerProgress handles ipc.PlayerProgressMsg.
func (m Model) handlePlayerProgress(msg ipc.PlayerProgressMsg) (tea.Model, tea.Cmd) {
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
	return m, nil
}

// handlePlayerTerminalTakeover handles ipc.PlayerTerminalTakeoverMsg.
func (m Model) handlePlayerTerminalTakeover(msg ipc.PlayerTerminalTakeoverMsg) (tea.Model, tea.Cmd) {
	// mpv is about to render video inline — release the terminal so it can
	// write to stdout directly.
	if m.program != nil {
		_ = m.program.ReleaseTerminal()
	}
	m.terminalVOActive = true
	return m, nil
}

// handlePlayerEnded handles ipc.PlayerEndedMsg.
func (m Model) handlePlayerEnded(msg ipc.PlayerEndedMsg) (tea.Model, tea.Cmd) {
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
	return m, nil
}

// handleSyncHide handles syncHideMsg.
func (m Model) handleSyncHide(msg syncHideMsg) (tea.Model, tea.Cmd) {
	m.syncOverlay = nil
	return m, nil
}

// handleBingeTick handles bingeTickMsg.
func (m Model) handleBingeTick(msg bingeTickMsg) (tea.Model, tea.Cmd) {
	if m.bingeCountdown > 0 {
		m.bingeCountdown--
		if m.bingeCountdown == 0 {
			return m, m.playBingeNext()
		}
		return m, bingeTickCmd()
	}
	return m, nil
}

// handleSkipSegment handles ipc.SkipSegmentMsg.
func (m Model) handleSkipSegment(msg ipc.SkipSegmentMsg) (tea.Model, tea.Cmd) {
	switch msg.SegmentType {
	case "intro":
		m.skipIntro = &msg
	case "credits":
		m.skipCredits = &msg
	}
	return m, nil
}

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
