package ui

import (
	"testing"

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
