package ui

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screens"
)

// TestModel_RoutesDetailMetadataPartialToDetailState exercises the fromIPC
// dispatch path: a DetailMetadataPartial arriving on the IPC channel must
// be handed to m.detail.ApplyMetadataPartial so per-verb fetch state
// flips to FetchLoaded and the renderer picks up the payload.
//
// We bypass the runtime by constructing the minimum Model shape the
// handler touches (just m.detail) and calling Update directly with the
// IPC-wrapped message.
func TestModel_RoutesDetailMetadataPartialToDetailState(t *testing.T) {
	detail := screens.NewDetailState(ipc.DetailEntry{ID: "tt1"})
	m := Model{detail: &detail}

	partial := ipc.DetailMetadataPartial{
		EntryID: "tt1",
		Verb:    "credits",
		Payload: ipc.MetadataPayload{
			Type: "credits",
			Crew: []ipc.CrewWire{{Name: "Nolan", Role: "director"}},
		},
	}

	updated, _ := m.Update(partial)
	m2, ok := updated.(Model)
	if !ok {
		t.Fatalf("Update returned unexpected model type %T", updated)
	}
	if m2.detail == nil {
		t.Fatal("detail was cleared by Update")
	}
	if m2.detail.Meta.CreditsStatus != screens.FetchLoaded {
		t.Errorf("credits status = %v, want FetchLoaded", m2.detail.Meta.CreditsStatus)
	}
	if got := len(m2.detail.Meta.Credits.Crew); got != 1 {
		t.Errorf("crew len = %d, want 1", got)
	}
}

// TestModel_IgnoresDetailMetadataPartialWithNoDetail — when the user has
// closed the detail overlay before the partial lands, the message is a
// no-op and must not panic.
func TestModel_IgnoresDetailMetadataPartialWithNoDetail(t *testing.T) {
	m := Model{detail: nil}
	partial := ipc.DetailMetadataPartial{
		EntryID: "tt1",
		Verb:    "enrich",
		Payload: ipc.MetadataPayload{Type: "empty"},
	}
	updated, _ := m.Update(partial)
	if _, ok := updated.(Model); !ok {
		t.Fatalf("Update returned unexpected model type %T", updated)
	}
}

// TestModel_BackdropCarouselCyclesOnArrowKey — with the detail overlay
// open and focus on the info zone, a KeyRight should advance the
// artwork cursor. Ensures the carousel keybinding wired in ui.go
// doesn't get swallowed by the existing focus-cycle or provider
// left/right handlers.
func TestModel_BackdropCarouselCyclesOnArrowKey(t *testing.T) {
	detail := screens.NewDetailState(ipc.DetailEntry{ID: "tt1"})
	detail.Meta.Artwork = ipc.MetadataPayload{
		Type: "artwork",
		Backdrops: []ipc.ArtworkVariantWire{
			{URL: "a.jpg", SizeLabel: "hi_res"},
			{URL: "b.jpg", SizeLabel: "hi_res"},
		},
	}
	detail.Meta.ArtworkStatus = screens.FetchLoaded
	detail.Focus = screens.FocusDetailInfo
	m := Model{detail: &detail, screen: screenDetail}

	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyRight})
	m2, ok := updated.(Model)
	if !ok {
		t.Fatalf("Update returned unexpected model type %T", updated)
	}
	if m2.detail == nil {
		t.Fatal("detail was cleared by Update")
	}
	if m2.detail.Meta.ArtworkCursor != 1 {
		t.Errorf("artwork cursor after KeyRight = %d, want 1", m2.detail.Meta.ArtworkCursor)
	}

	// Wrap-around: advancing past the last backdrop returns to 0.
	updated, _ = m2.Update(tea.KeyPressMsg{Code: tea.KeyRight})
	m3 := updated.(Model)
	if m3.detail.Meta.ArtworkCursor != 0 {
		t.Errorf("artwork cursor after wrap = %d, want 0", m3.detail.Meta.ArtworkCursor)
	}
}
