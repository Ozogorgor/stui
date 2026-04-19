package catalogbrowser

import (
	"testing"

	"github.com/stui/stui/internal/ipc"
)

func TestDataSourceState_RoundTrip(t *testing.T) {
	s := DataSourceState{
		Items: map[ipc.EntryKind][]Entry{
			ipc.KindArtist: {{ID: "a1", Title: "Radiohead", Source: "lastfm"}},
			ipc.KindTrack:  {{ID: "t1", Title: "Creep", ArtistName: "Radiohead"}},
		},
		Cursor: Cursor{Column: 1, Row: 3, Scroll: 0},
	}
	s2 := s
	if s2.Cursor.Row != 3 {
		t.Fatalf("cursor not copied: got %+v", s2.Cursor)
	}
	if s2.Items[ipc.KindArtist][0].Title != "Radiohead" {
		t.Fatalf("item not copied: got %+v", s2.Items)
	}
}

func TestSearchStatus_ZeroValueIsInactive(t *testing.T) {
	var s SearchStatus
	if s.Active || s.Partial || s.QueryID != 0 {
		t.Fatalf("zero value should be inactive: %+v", s)
	}
}
