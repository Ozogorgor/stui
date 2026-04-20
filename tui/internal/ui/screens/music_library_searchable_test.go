package screens

import (
	"context"
	"testing"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screens/catalogbrowser"
)

// Compile-time assertions: MusicLibraryScreen must satisfy both the public
// Searchable interface and the private searchableLibrary narrowing
// interface from music_screen.go. Without these, the interface assertions
// in MusicScreen.Apply* would silently degrade to no-ops at runtime.
var (
	_ Searchable        = MusicLibraryScreen{}
	_ searchableLibrary = MusicLibraryScreen{}
)

func TestMusicLibrary_SearchScopes(t *testing.T) {
	var s MusicLibraryScreen
	got := s.SearchScopes()
	want := []ipc.SearchScope{ipc.ScopeArtist, ipc.ScopeAlbum, ipc.ScopeTrack}
	if len(got) != len(want) {
		t.Fatalf("scopes len: got %v want %v", got, want)
	}
	for i := range got {
		if got[i] != want[i] {
			t.Fatalf("scope[%d]: got %v want %v", i, got[i], want[i])
		}
	}
}

func TestMusicLibrary_SearchPlaceholder(t *testing.T) {
	var s MusicLibraryScreen
	if s.SearchPlaceholder() == "" {
		t.Fatal("placeholder must be non-empty")
	}
}

func TestMusicLibrary_StartSearch_EmptyQueryReturnsNil(t *testing.T) {
	// Even with a real source, empty query is a no-op.
	s := MusicLibraryScreen{source: catalogbrowser.NewMpdDataSource(&mockLibraryClient{})}
	if s.StartSearch("") != nil {
		t.Fatal("empty query should return nil cmd")
	}
}

func TestMusicLibrary_StartSearch_NilSourceReturnsNil(t *testing.T) {
	var s MusicLibraryScreen // source == nil
	if s.StartSearch("anything") != nil {
		t.Fatal("nil source should return nil cmd")
	}
}

func TestMusicLibrary_StartSearch_RealSourceReturnsCmd(t *testing.T) {
	s := MusicLibraryScreen{source: catalogbrowser.NewMpdDataSource(&mockLibraryClient{})}
	cmd := s.StartSearch("radiohead")
	if cmd == nil {
		t.Fatal("non-empty query with real source should return a cmd")
	}
}

func TestMusicLibrary_OnScopeResults_Noop(t *testing.T) {
	var s MusicLibraryScreen
	_, cmd := s.OnScopeResults(ipc.ScopeResultsMsg{})
	if cmd != nil {
		t.Fatal("OnScopeResults should return nil cmd")
	}
}

func TestMusicLibrary_OnMpdSearchResult_Noop(t *testing.T) {
	var s MusicLibraryScreen
	_, cmd := s.OnMpdSearchResult(ipc.MpdSearchResult{})
	if cmd != nil {
		t.Fatal("OnMpdSearchResult should return nil cmd")
	}
}

func TestMusicLibrary_RestoreView_NilSourceIsNoop(t *testing.T) {
	var s MusicLibraryScreen // source == nil; must not panic
	_ = s.RestoreView()
}

// mockLibraryClient satisfies catalogbrowser.IPCClient. Returns a zero
// MpdSearchResult so Search() dispatch paths can be exercised without
// standing up the real IPC transport.
type mockLibraryClient struct{}

func (mockLibraryClient) MpdSearch(_ context.Context, _ string, _ []ipc.MpdScope) (*ipc.MpdSearchResult, error) {
	return &ipc.MpdSearchResult{}, nil
}
