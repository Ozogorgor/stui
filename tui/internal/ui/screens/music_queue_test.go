package screens

import (
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/stui/stui/internal/ipc"
)

// helper: new queue screen with a known track loaded and playing
func queueWithTrack() MusicQueueScreen {
	s := NewMusicQueueScreen(nil)
	s.tracks = []ipc.MpdTrack{
		{ID: 5, Pos: 0, Title: "Cornish Acid", Artist: "Aphex Twin", Album: "RDJ Album", Duration: 214},
	}
	s.nowSongID = 5
	s.nowDuration = 214
	s.nowElapsed = 63
	s.nowVolume = 72
	s.prevVolume = 100
	return s
}

// MpdStatusMsg captures Elapsed, Duration, Volume
func TestQueueStatusMsgCapturesFields(t *testing.T) {
	s := NewMusicQueueScreen(nil)
	msg := ipc.MpdStatusMsg{
		SongTitle:  "Cornish Acid",
		SongArtist: "Aphex Twin",
		SongID:     5,
		Elapsed:    63.0,
		Duration:   214.0,
		Volume:     72,
	}
	s2, _ := s.Update(msg)
	if s2.nowElapsed != 63.0 {
		t.Errorf("nowElapsed = %v, want 63.0", s2.nowElapsed)
	}
	if s2.nowDuration != 214.0 {
		t.Errorf("nowDuration = %v, want 214.0", s2.nowDuration)
	}
	if s2.nowVolume != 72 {
		t.Errorf("nowVolume = %v, want 72", s2.nowVolume)
	}
}

// External volume-up clears nowMuted
func TestQueueStatusMsgClearsMuteOnVolumeUp(t *testing.T) {
	s := queueWithTrack()
	s.nowMuted = true
	s.nowVolume = 0
	msg := ipc.MpdStatusMsg{Volume: 50, SongID: 5}
	s2, _ := s.Update(msg)
	if s2.nowMuted {
		t.Error("nowMuted should be cleared when external volume > 0")
	}
}

// Key "0" mutes when not muted
func TestQueueMuteKeyMutes(t *testing.T) {
	s := queueWithTrack()
	s2, _ := s.Update(tea.KeyPressMsg{Text: "0"})
	if !s2.nowMuted {
		t.Error("pressing 0 should set nowMuted=true")
	}
	if s2.prevVolume != 72 {
		t.Errorf("prevVolume = %v, want 72", s2.prevVolume)
	}
}

// Key "0" unmutes when already muted
func TestQueueMuteKeyUnmutes(t *testing.T) {
	s := queueWithTrack()
	s.nowMuted = true
	s.nowVolume = 0
	s.prevVolume = 72
	s2, _ := s.Update(tea.KeyPressMsg{Text: "0"})
	if s2.nowMuted {
		t.Error("pressing 0 when muted should set nowMuted=false")
	}
}

// Muting when volume already 0 externally: treat as mute (save prevVolume=0)
func TestQueueMuteKeyWhenAlreadyZero(t *testing.T) {
	s := queueWithTrack()
	s.nowVolume = 0
	s.nowMuted = false
	s2, _ := s.Update(tea.KeyPressMsg{Text: "0"})
	if !s2.nowMuted {
		t.Error("pressing 0 when volume=0 and not muted should set nowMuted=true")
	}
	if s2.prevVolume != 0 {
		t.Errorf("prevVolume = %v, want 0", s2.prevVolume)
	}
}

// Key "<" does nothing when nowDuration == 0
func TestQueueSeekBackNoopWhenNoDuration(t *testing.T) {
	s := NewMusicQueueScreen(nil)
	s.nowDuration = 0
	s.nowElapsed = 0
	_, cmd := s.Update(tea.KeyPressMsg{Text: "<"})
	if cmd != nil {
		t.Error("seek < should be a no-op when nowDuration == 0")
	}
}

// Key ">" does nothing when nowDuration == 0
func TestQueueSeekFwdNoopWhenNoDuration(t *testing.T) {
	s := NewMusicQueueScreen(nil)
	s.nowDuration = 0
	_, cmd := s.Update(tea.KeyPressMsg{Text: ">"})
	if cmd != nil {
		t.Error("seek > should be a no-op when nowDuration == 0")
	}
}
