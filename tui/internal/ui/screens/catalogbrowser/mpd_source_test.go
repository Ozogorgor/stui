package catalogbrowser

import (
	"context"
	"errors"
	"testing"

	"github.com/stui/stui/internal/ipc"
)

type mockIPCClient struct {
	result     *ipc.MpdSearchResult
	err        error
	lastQuery  string
	lastScopes []ipc.MpdScope
}

func (m *mockIPCClient) MpdSearch(_ context.Context, q string, scopes []ipc.MpdScope) (*ipc.MpdSearchResult, error) {
	m.lastQuery = q
	m.lastScopes = scopes
	return m.result, m.err
}

func TestMpdDataSource_SetItemsAndItems(t *testing.T) {
	s := NewMpdDataSource(&mockIPCClient{})
	s.SetItems(ipc.KindArtist, []Entry{{ID: "a", Title: "Radiohead"}})
	if got := s.Items(ipc.KindArtist); len(got) != 1 || got[0].Title != "Radiohead" {
		t.Fatalf("Items mismatch: %+v", got)
	}
}

func TestMpdDataSource_HasMultipleSources_False(t *testing.T) {
	s := NewMpdDataSource(&mockIPCClient{})
	if s.HasMultipleSources() {
		t.Fatal("MPD should be single-source")
	}
}

func TestMpdDataSource_SearchReplacesItemsOnSuccess(t *testing.T) {
	client := &mockIPCClient{
		result: &ipc.MpdSearchResult{
			QueryID: 42,
			Artists: []ipc.MpdArtist{{Name: "Radiohead"}},
			Albums:  []ipc.MpdAlbum{{Title: "Pablo Honey", Year: "1993", Artist: "Radiohead"}},
			Tracks:  []ipc.MpdSong{{Title: "Creep", Artist: "Radiohead", Album: "Pablo Honey"}},
		},
	}
	s := NewMpdDataSource(client)
	s.SetItems(ipc.KindArtist, []Entry{{ID: "before", Title: "Before"}})

	cmd := s.Search(context.Background(), "radiohead",
		[]ipc.EntryKind{ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack})
	msg := cmd()

	applied, ok := msg.(MpdSearchAppliedMsg)
	if !ok || applied.QueryID != 42 || !applied.Updated {
		t.Fatalf("expected MpdSearchAppliedMsg{QueryID:42}, got %+v", msg)
	}
	if got := s.Items(ipc.KindArtist); len(got) != 1 || got[0].Title != "Radiohead" {
		t.Fatalf("artist not replaced: %+v", got)
	}
	if got := s.Items(ipc.KindAlbum); len(got) != 1 || got[0].Title != "(1993) Pablo Honey" {
		t.Fatalf("album year-prefix missing: %+v", got)
	}
	if got := s.Items(ipc.KindTrack); len(got) != 1 || got[0].Title != "Creep" {
		t.Fatalf("track not mapped: %+v", got)
	}
}

func TestMpdDataSource_RestoreReturnsPriorView(t *testing.T) {
	client := &mockIPCClient{
		result: &ipc.MpdSearchResult{
			Artists: []ipc.MpdArtist{{Name: "After"}},
		},
	}
	s := NewMpdDataSource(client)
	s.SetItems(ipc.KindArtist, []Entry{{ID: "before", Title: "Before"}})
	snap := s.Snapshot()

	cmd := s.Search(context.Background(), "after", []ipc.EntryKind{ipc.KindArtist})
	_ = cmd()

	if got := s.Items(ipc.KindArtist); len(got) == 0 || got[0].Title != "After" {
		t.Fatalf("expected mid-search state 'After', got %+v", got)
	}
	s.Restore(snap)
	if got := s.Items(ipc.KindArtist); len(got) == 0 || got[0].Title != "Before" {
		t.Fatalf("restore failed: %+v", got)
	}
}

func TestMpdDataSource_SearchFailedOnTransportErr(t *testing.T) {
	client := &mockIPCClient{err: errors.New("boom")}
	s := NewMpdDataSource(client)
	cmd := s.Search(context.Background(), "x", []ipc.EntryKind{ipc.KindArtist})
	msg := cmd()
	if _, ok := msg.(MpdSearchFailedMsg); !ok {
		t.Fatalf("expected MpdSearchFailedMsg, got %T", msg)
	}
}

func TestMpdDataSource_SearchFailedOnRemoteErr(t *testing.T) {
	client := &mockIPCClient{
		result: &ipc.MpdSearchResult{Error: &ipc.MpdSearchErr{Type: "not_connected"}},
	}
	s := NewMpdDataSource(client)
	cmd := s.Search(context.Background(), "x", []ipc.EntryKind{ipc.KindArtist})
	msg := cmd()
	if _, ok := msg.(MpdSearchFailedMsg); !ok {
		t.Fatalf("expected MpdSearchFailedMsg, got %T", msg)
	}
}

func TestMpdDataSource_SetAll(t *testing.T) {
	s := NewMpdDataSource(&mockIPCClient{})
	s.SetAll(map[ipc.EntryKind][]Entry{
		ipc.KindArtist: {{ID: "a", Title: "Artist A"}},
		ipc.KindAlbum:  {{ID: "b", Title: "Album B"}},
	})
	if got := s.Items(ipc.KindArtist); len(got) != 1 || got[0].Title != "Artist A" {
		t.Fatalf("SetAll artist: %+v", got)
	}
	if got := s.Items(ipc.KindAlbum); len(got) != 1 || got[0].Title != "Album B" {
		t.Fatalf("SetAll album: %+v", got)
	}
}

func TestMpdDataSource_SnapshotIsDeepCopy(t *testing.T) {
	s := NewMpdDataSource(&mockIPCClient{})
	s.SetItems(ipc.KindArtist, []Entry{{ID: "orig", Title: "Original"}})
	snap := s.Snapshot()

	// Mutate the source after snapshotting.
	s.SetItems(ipc.KindArtist, []Entry{{ID: "new", Title: "New"}})

	// Snapshot should be unchanged.
	if got := snap.Items[ipc.KindArtist]; len(got) != 1 || got[0].Title != "Original" {
		t.Fatalf("Snapshot not a deep copy: %+v", got)
	}
}

func TestMpdDataSource_SearchStatusClearedOnFailure(t *testing.T) {
	client := &mockIPCClient{err: errors.New("transport error")}
	s := NewMpdDataSource(client)
	cmd := s.Search(context.Background(), "query", []ipc.EntryKind{ipc.KindArtist})

	// Before the command runs, status should be active.
	if !s.Status().Active {
		t.Fatal("status should be Active before cmd runs")
	}

	_ = cmd()

	// After failure, status should be cleared.
	if s.Status().Active {
		t.Fatal("status should not be Active after failure")
	}
}

func TestMapMpdAlbums_YearPrefix(t *testing.T) {
	rows := []ipc.MpdAlbum{
		{Title: "Kid A", Year: "2000", Artist: "Radiohead"},
		{Title: "Pablo Honey", Year: "1993-02-22", Artist: "Radiohead"},
		{Title: "No Year", Year: "", Artist: "Radiohead"},
	}
	got := MapMpdAlbums(rows)
	if len(got) != 3 {
		t.Fatalf("expected 3 entries, got %d", len(got))
	}
	if got[0].Title != "(2000) Kid A" {
		t.Errorf("year prefix wrong: %q", got[0].Title)
	}
	if got[1].Title != "(1993) Pablo Honey" {
		t.Errorf("year from full date wrong: %q", got[1].Title)
	}
	if got[2].Title != "No Year" {
		t.Errorf("empty year should not prefix: %q", got[2].Title)
	}
}

func TestMpdScopesFor(t *testing.T) {
	scopes := mpdScopesFor([]ipc.EntryKind{ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack})
	if len(scopes) != 3 {
		t.Fatalf("expected 3 scopes, got %d", len(scopes))
	}
	if scopes[0] != ipc.MpdScopeArtist || scopes[1] != ipc.MpdScopeAlbum || scopes[2] != ipc.MpdScopeTrack {
		t.Errorf("unexpected scopes: %v", scopes)
	}
}
