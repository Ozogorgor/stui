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

// queueColWidths(L) returns (titleW, artistW, albumW) where albumW==0 means no album column.
// Fixed overhead: prefix 3 + # 3 + space 1 + dur 6 = 13. Remaining R = L - 13.
// Wide (L>=120): title=R*40/100, artist=R*35/100, album=R*25/100, remainder to title.
// Narrow (L<120): title=R*55/100, artist=R*45/100, album=0, remainder to title.

func TestQueueColWidthsNarrow(t *testing.T) {
	// L=100, R=87: title=47 (87*55/100=47 rem 85), artist=39 (87*45/100=39 rem 15)
	// remainder = 87 - 47 - 39 = 1 goes to title → title=48
	tw, aw, alw := queueColWidths(100)
	if alw != 0 {
		t.Errorf("albumW = %d, want 0 for narrow layout", alw)
	}
	if tw+aw != 87 {
		t.Errorf("titleW(%d)+artistW(%d) = %d, want 87", tw, aw, tw+aw)
	}
	_ = tw
	_ = aw
}

func TestQueueColWidthsWide(t *testing.T) {
	// L=120, R=107: title=42, artist=37, album=26, rem=2 → title=44
	tw, aw, alw := queueColWidths(120)
	if alw == 0 {
		t.Error("albumW should be > 0 for L=120")
	}
	if tw+aw+alw != 107 {
		t.Errorf("column widths sum %d, want 107", tw+aw+alw)
	}
}

func TestQueueColWidthsExact143Terminal(t *testing.T) {
	// terminal width=143 → L=143-23=120, triggers wide layout
	L := 143 - 23
	_, _, alw := queueColWidths(L)
	if alw == 0 {
		t.Errorf("album column should appear at L=%d (terminal width 143)", L)
	}
}

func TestQueueColWidthsBelowThreshold(t *testing.T) {
	// L=119: narrow layout
	_, _, alw := queueColWidths(119)
	if alw != 0 {
		t.Errorf("album column should not appear at L=119, got albumW=%d", alw)
	}
}
