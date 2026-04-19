package catalogbrowser

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/stui/stui/internal/ipc"
)

func TestSourcePicker_RendersAllCandidates(t *testing.T) {
	cs := []Entry{
		{ID: "a", Title: "Creep", Kind: ipc.KindTrack, Source: "spotify-provider"},
		{ID: "b", Title: "Creep", Kind: ipc.KindTrack, Source: "soundcloud-provider"},
	}
	p := NewSourcePicker("Creep — Radiohead", cs)
	out := p.View()
	for _, want := range []string{"Creep — Radiohead", "spotify-provider", "soundcloud-provider"} {
		if !strings.Contains(out, want) {
			t.Fatalf("View missing %q:\n%s", want, out)
		}
	}
}

func TestSourcePicker_DownThenEnter(t *testing.T) {
	cs := []Entry{
		{ID: "a", Kind: ipc.KindTrack, Source: "x"},
		{ID: "b", Kind: ipc.KindTrack, Source: "y"},
	}
	p := NewSourcePicker("t", cs)

	// Down navigation
	p, _ = p.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if p.SelectedIndex() != 1 {
		t.Fatalf("cursor not advanced: %d", p.SelectedIndex())
	}

	// Enter to select
	_, cmd := p.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd == nil {
		t.Fatal("expected Enter to emit a Cmd")
	}
	msg := cmd()
	sel, ok := msg.(SourceSelectedMsg)
	if !ok || sel.Entry.ID != "b" {
		t.Fatalf("expected SourceSelectedMsg{Entry.ID=b}, got %+v", msg)
	}
}

func TestSourcePicker_VimKeysDownThenEnter(t *testing.T) {
	cs := []Entry{
		{ID: "a", Kind: ipc.KindTrack, Source: "x"},
		{ID: "b", Kind: ipc.KindTrack, Source: "y"},
	}
	p := NewSourcePicker("t", cs)

	// Vim down 'j'
	p, _ = p.Update(tea.KeyPressMsg{Code: 'j'})
	if p.SelectedIndex() != 1 {
		t.Fatalf("cursor not advanced with 'j': %d", p.SelectedIndex())
	}

	// Enter to select
	_, cmd := p.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd == nil {
		t.Fatal("expected Enter to emit a Cmd")
	}
	msg := cmd()
	sel, ok := msg.(SourceSelectedMsg)
	if !ok || sel.Entry.ID != "b" {
		t.Fatalf("expected SourceSelectedMsg{Entry.ID=b}, got %+v", msg)
	}
}

func TestSourcePicker_EscEmitsCancelled(t *testing.T) {
	p := NewSourcePicker("t", []Entry{{Source: "x"}})
	_, cmd := p.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if cmd == nil {
		t.Fatal("expected Esc to emit a Cmd")
	}
	if _, ok := cmd().(SourcePickerCancelledMsg); !ok {
		t.Fatal("expected SourcePickerCancelledMsg")
	}
}

func TestSourcePicker_CursorClamping(t *testing.T) {
	cs := []Entry{{Source: "a"}, {Source: "b"}}
	p := NewSourcePicker("t", cs)

	// Up at index 0 stays at 0
	p, _ = p.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	if p.SelectedIndex() != 0 {
		t.Fatalf("up at top: got %d", p.SelectedIndex())
	}

	// Down twice to reach end
	p, _ = p.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	p, _ = p.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if p.SelectedIndex() != 1 {
		t.Fatalf("down to end: got %d, want 1", p.SelectedIndex())
	}

	// Down past end stays at last
	p, _ = p.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if p.SelectedIndex() != 1 {
		t.Fatalf("down past end: got %d, want 1", p.SelectedIndex())
	}
}

func TestSourcePicker_EmptyCandidatesRendersEmpty(t *testing.T) {
	p := NewSourcePicker("t", nil)
	if p.View() != "" {
		t.Fatalf("empty picker should render empty string, got %q", p.View())
	}
}

func TestSourcePicker_UpWithVimKey(t *testing.T) {
	cs := []Entry{{Source: "a"}, {Source: "b"}}
	p := NewSourcePicker("t", cs)

	// Start at 1
	p, _ = p.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if p.SelectedIndex() != 1 {
		t.Fatalf("failed to move to index 1")
	}

	// Up with 'k'
	p, _ = p.Update(tea.KeyPressMsg{Code: 'k'})
	if p.SelectedIndex() != 0 {
		t.Fatalf("'k' did not move up: got %d", p.SelectedIndex())
	}
}

func TestSourcePicker_NonKeyMsgPassthrough(t *testing.T) {
	p := NewSourcePicker("t", []Entry{{Source: "x"}})
	// Send a non-key message (e.g. WindowSizeMsg)
	p2, cmd := p.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	if cmd != nil {
		t.Fatal("non-key message should not produce a Cmd")
	}
	if p2.SelectedIndex() != p.SelectedIndex() {
		t.Fatal("non-key message should not change state")
	}
}
