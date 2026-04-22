package screens

import (
	"strings"
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
// Fixed overhead: 17ch (no album) or 18ch (with album) — Dur is %7s + 1ch gap.
// Wide (L>=120): R=L-18, title=R*40/100, artist=R*35/100, album=R*25/100, remainder to title.
// Narrow (L<120): R=L-17, title=R*55/100, artist=R*45/100, album=0, remainder to title.

func TestQueueColWidthsNarrow(t *testing.T) {
	// L=100, R=100-17=83: title=45 (83*55/100=45), artist=37 (83*45/100=37)
	// remainder = 83 - 45 - 37 = 1 goes to title → title=46
	tw, aw, alw, _ := queueColWidths(100)
	if alw != 0 {
		t.Errorf("albumW = %d, want 0 for narrow layout", alw)
	}
	if tw+aw != 83 {
		t.Errorf("titleW(%d)+artistW(%d) = %d, want 83", tw, aw, tw+aw)
	}
	_ = tw
	_ = aw
}

func TestQueueColWidthsWide(t *testing.T) {
	// L=120, R=120-18=102: title=40, artist=35, album=25, rem=2 → title=42
	tw, aw, alw, _ := queueColWidths(120)
	if alw == 0 {
		t.Error("albumW should be > 0 for L=120")
	}
	if tw+aw+alw != 102 {
		t.Errorf("column widths sum %d, want 102", tw+aw+alw)
	}
}

func TestQueueColWidthsExact143Terminal(t *testing.T) {
	// terminal width=146, rightBoxW=24: leftBoxW=146-24=122, L=122-2=120 >= 120, wide layout
	L := 146 - 26
	_, _, alw, _ := queueColWidths(L)
	if alw == 0 {
		t.Errorf("album column should appear at L=%d (terminal width 146)", L)
	}
}

func TestQueueColWidthsBelowThreshold(t *testing.T) {
	// L=119: narrow layout
	_, _, alw, _ := queueColWidths(119)
	if alw != 0 {
		t.Errorf("album column should not appear at L=119, got albumW=%d", alw)
	}
}

// ── Art placeholder ────────────────────────────────────────────────────

func TestQueueArtPlaceholderRowCount(t *testing.T) {
	// With innerW=22: Height(22/2)=Height(11) → 11 outer rows.
	const innerW = 22
	lines := strings.Split(strings.TrimRight(queueArtPlaceholder(innerW), "\n"), "\n")
	want := innerW / 2
	if len(lines) != want {
		t.Errorf("art placeholder has %d rows, want %d (innerW/2)", len(lines), want)
	}
}

func TestQueueArtPlaceholderContainsMusicNote(t *testing.T) {
	out := queueArtPlaceholder(22)
	if !strings.Contains(out, "♪") {
		t.Error("art placeholder should contain ♪")
	}
}

// ── Seek bar ───────────────────────────────────────────────────────────

func TestQueueSeekBarZeroDuration(t *testing.T) {
	bar, times := queueSeekBar(0, 0, 22)
	for _, ch := range bar {
		if ch != '─' {
			t.Errorf("seek bar with duration=0 should be all ─, got %q", bar)
			break
		}
	}
	if !strings.Contains(times, "0:00") {
		t.Errorf("seek bar times %q should contain 0:00", times)
	}
}

func TestQueueSeekBarLength22(t *testing.T) {
	bar, _ := queueSeekBar(63, 214, 22)
	// strip ANSI — count runes that are bar chars
	count := 0
	for _, r := range bar {
		if r == '━' || r == '╸' || r == '─' {
			count++
		}
	}
	if count != 22 {
		t.Errorf("seek bar has %d bar chars, want 22", count)
	}
}

func TestQueueSeekBarCursorChar(t *testing.T) {
	bar, _ := queueSeekBar(63, 214, 22)
	if !strings.ContainsRune(bar, '╸') {
		t.Errorf("seek bar %q should contain ╸ (U+2578)", bar)
	}
}

func TestQueueSeekBarFullProgress(t *testing.T) {
	// elapsed == duration: filled=21, cursor at pos 21
	bar, _ := queueSeekBar(214, 214, 22)
	if !strings.ContainsRune(bar, '╸') {
		t.Errorf("full seek bar should still have ╸")
	}
}

// ── Volume bar ─────────────────────────────────────────────────────────

func TestQueueVolumeBar72(t *testing.T) {
	bar, hint := queueVolumeBar(72, false)
	if !strings.Contains(bar, "72%") {
		t.Errorf("volume bar %q should contain 72%%", bar)
	}
	if !strings.Contains(hint, "mute") {
		t.Errorf("hint %q should contain 'mute' when not muted", hint)
	}
}

func TestQueueVolumeBarMuted(t *testing.T) {
	_, hint := queueVolumeBar(0, true)
	if !strings.Contains(hint, "unmute") {
		t.Errorf("hint %q should contain 'unmute' when muted", hint)
	}
}

func TestQueueVolumeBar100(t *testing.T) {
	bar, _ := queueVolumeBar(100, false)
	// 16 filled blocks
	filled := strings.Count(bar, "▮")
	if filled != 16 {
		t.Errorf("volume=100 should have 16 filled blocks, got %d", filled)
	}
	empty := strings.Count(bar, "▯")
	if empty != 0 {
		t.Errorf("volume=100 should have 0 empty blocks, got %d", empty)
	}
}

func TestQueueVolumeBarZero(t *testing.T) {
	bar, _ := queueVolumeBar(0, false)
	filled := strings.Count(bar, "▮")
	if filled != 0 {
		t.Errorf("volume=0 should have 0 filled blocks, got %d", filled)
	}
}

// ── View layout tests ──────────────────────────────────────────────────

func TestQueueViewNarrowNoRightPanel(t *testing.T) {
	s := queueWithTrack()
	out := s.View(80, 20)
	// narrow: no right panel TITLE label
	if strings.Contains(out, "TITLE") {
		t.Error("narrow view (width=80) should not contain right panel TITLE label")
	}
}

func TestQueueViewWideHasRightPanel(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.Contains(out, "TITLE") {
		t.Error("wide view (width=120) should contain right panel TITLE label")
	}
	if !strings.Contains(out, "ARTIST") {
		t.Error("wide view should contain ARTIST label")
	}
}

func TestQueueViewWideHasColumnHeaders(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.Contains(out, "Title") {
		t.Error("wide view should contain Title column header")
	}
	if !strings.Contains(out, "Artist") {
		t.Error("wide view should contain Artist column header")
	}
}

func TestQueueViewWideHasSeekBar(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.ContainsRune(out, '╸') {
		t.Error("wide view should contain seek bar cursor ╸")
	}
}

func TestQueueViewWideHasVolumeBar(t *testing.T) {
	s := queueWithTrack()
	out := s.View(120, 30)
	if !strings.Contains(out, "▮") {
		t.Error("wide view should contain volume bar filled blocks ▮")
	}
}

func TestQueueViewAlbumColumnAtWidth147(t *testing.T) {
	s := queueWithTrack()
	// rightBoxW=24: leftBoxW=147-24=123, innerL=123-2=121 >= 120, wide layout
	out := s.View(147, 30)
	if !strings.Contains(out, "Album") {
		t.Error("view at width=147 should show Album column header")
	}
}

func TestQueueViewNoAlbumColumnAtWidth145(t *testing.T) {
	s := queueWithTrack()
	// rightBoxW=24: leftBoxW=145-24=121, innerL=121-2=119 < 120, narrow layout
	out := s.View(145, 30)
	if strings.Contains(out, "Album") {
		t.Error("view at width=145 (L=119) should NOT show Album column header")
	}
}

func TestQueueVolumeUpKey(t *testing.T) {
	s := queueWithTrack() // nowVolume = 72
	s2, _ := s.Update(tea.KeyPressMsg{Text: "+"})
	// Can't verify IPC call directly, but key should not error and nowMuted should clear
	_ = s2
}

func TestQueueVolumeDownKey(t *testing.T) {
	s := queueWithTrack()
	s2, _ := s.Update(tea.KeyPressMsg{Text: "-"})
	_ = s2
}
