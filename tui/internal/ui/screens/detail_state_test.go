package screens

import (
	"testing"

	"github.com/stui/stui/internal/ipc"
)

func TestDetailState_DefaultFetchStatuses(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{})
	if ds.Meta.EnrichStatus != FetchPending {
		t.Errorf("enrich status = %v, want FetchPending", ds.Meta.EnrichStatus)
	}
	if ds.Meta.CreditsStatus != FetchPending {
		t.Errorf("credits status = %v, want FetchPending", ds.Meta.CreditsStatus)
	}
	if ds.Meta.ArtworkStatus != FetchPending {
		t.Errorf("artwork status = %v, want FetchPending", ds.Meta.ArtworkStatus)
	}
	if ds.Meta.RelatedStatus != FetchPending {
		t.Errorf("related status = %v, want FetchPending", ds.Meta.RelatedStatus)
	}
}

func TestDetailState_ApplyCreditsPartial(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.ApplyMetadataPartial(ipc.DetailMetadataPartial{
		EntryID: "tt1",
		Verb:    "credits",
		Payload: ipc.MetadataPayload{
			Type: "credits",
			Crew: []ipc.CrewWire{{Name: "Nolan", Role: "director"}},
		},
	})
	if ds.Meta.CreditsStatus != FetchLoaded {
		t.Errorf("credits status = %v, want FetchLoaded", ds.Meta.CreditsStatus)
	}
	if len(ds.Meta.Credits.Crew) != 1 {
		t.Fatalf("crew len = %d, want 1", len(ds.Meta.Credits.Crew))
	}
	if ds.Meta.Credits.Crew[0].Name != "Nolan" {
		t.Errorf("crew[0].Name = %q, want Nolan", ds.Meta.Credits.Crew[0].Name)
	}
}

func TestDetailState_StaleEntryIgnored(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.ApplyMetadataPartial(ipc.DetailMetadataPartial{
		EntryID: "tt999", // different entry — must be ignored
		Verb:    "credits",
		Payload: ipc.MetadataPayload{Type: "credits"},
	})
	if ds.Meta.CreditsStatus != FetchPending {
		t.Errorf("stale partial applied: status = %v, want FetchPending", ds.Meta.CreditsStatus)
	}
}

func TestDetailState_EnrichAppliesStudioToEntry(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	studio := "Syncopy"
	ds.ApplyMetadataPartial(ipc.DetailMetadataPartial{
		EntryID: "tt1",
		Verb:    "enrich",
		Payload: ipc.MetadataPayload{Type: "enrich", Studio: &studio},
	})
	if ds.Entry.Studio != "Syncopy" {
		t.Errorf("studio not copied to Entry: %q", ds.Entry.Studio)
	}
	if ds.Meta.EnrichStatus != FetchLoaded {
		t.Errorf("enrich status = %v, want FetchLoaded", ds.Meta.EnrichStatus)
	}
}

func TestDetailState_EmptyPayloadFlipsStatusToEmpty(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.ApplyMetadataPartial(ipc.DetailMetadataPartial{
		EntryID: "tt1",
		Verb:    "related",
		Payload: ipc.MetadataPayload{Type: "empty"},
	})
	if ds.Meta.RelatedStatus != FetchEmpty {
		t.Errorf("related status = %v, want FetchEmpty", ds.Meta.RelatedStatus)
	}
}

func TestDetailState_EnrichNetworksAndExternalIDs(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.ApplyMetadataPartial(ipc.DetailMetadataPartial{
		EntryID: "tt1",
		Verb:    "enrich",
		Payload: ipc.MetadataPayload{
			Type:        "enrich",
			Networks:    []string{"HBO", "Max"},
			ExternalIDs: map[string]string{"imdb": "tt1", "tmdb": "1"},
		},
	})
	if got := len(ds.Entry.Networks); got != 2 {
		t.Errorf("networks len = %d, want 2", got)
	}
	if got := ds.Entry.ExternalIDs["tmdb"]; got != "1" {
		t.Errorf("externalIDs[tmdb] = %q, want 1", got)
	}
}
