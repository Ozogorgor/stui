package screens

import (
	"context"
	"testing"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screens/catalogbrowser"
)

// Compile-time assertions: MusicBrowseScreen must satisfy both the public
// Searchable interface and the private searchableBrowse narrowing interface
// from music_screen.go. Without these, the interface assertions in
// MusicScreen.Apply* would silently degrade to no-ops at runtime.
var (
	_ Searchable       = MusicBrowseScreen{}
	_ searchableBrowse = MusicBrowseScreen{}
)

func TestMusicBrowse_SearchScopes(t *testing.T) {
	var s MusicBrowseScreen
	got := s.SearchScopes()
	want := []ipc.SearchScope{ipc.ScopeArtist, ipc.ScopeAlbum, ipc.ScopeTrack}
	if len(got) != len(want) {
		t.Fatalf("scopes len: got %d want %d", len(got), len(want))
	}
	for i := range got {
		if got[i] != want[i] {
			t.Fatalf("scope[%d]: got %v want %v", i, got[i], want[i])
		}
	}
}

func TestMusicBrowse_SearchPlaceholder(t *testing.T) {
	var s MusicBrowseScreen
	if s.SearchPlaceholder() == "" {
		t.Fatal("placeholder must be non-empty")
	}
}

func TestMusicBrowse_StartSearch_Empty(t *testing.T) {
	// Empty query is a no-op even with a real source.
	s := MusicBrowseScreen{source: catalogbrowser.NewPluginDataSource(&streamingMockClient{
		ch: make(chan ipc.ScopeResultsMsg),
	})}
	if s.StartSearch("") != nil {
		t.Fatal("empty query should return nil cmd")
	}
}

func TestMusicBrowse_StartSearch_NilSource(t *testing.T) {
	var s MusicBrowseScreen // source == nil
	if s.StartSearch("anything") != nil {
		t.Fatal("nil source should return nil cmd")
	}
}

func TestMusicBrowse_StartSearch_RealSourceReturnsCmd(t *testing.T) {
	ch := make(chan ipc.ScopeResultsMsg)
	s := MusicBrowseScreen{source: catalogbrowser.NewPluginDataSource(&streamingMockClient{ch: ch})}
	cmd := s.StartSearch("radiohead")
	if cmd == nil {
		t.Fatal("non-empty query with real source should return a cmd")
	}
}

func TestMusicBrowse_OnScopeResults_Noop(t *testing.T) {
	var s MusicBrowseScreen
	_, cmd := s.OnScopeResults(ipc.ScopeResultsMsg{})
	if cmd != nil {
		t.Fatal("OnScopeResults should return nil cmd")
	}
}

func TestMusicBrowse_OnMpdSearchResult_Noop(t *testing.T) {
	var s MusicBrowseScreen
	_, cmd := s.OnMpdSearchResult(ipc.MpdSearchResult{})
	if cmd != nil {
		t.Fatal("OnMpdSearchResult should return nil cmd")
	}
}

func TestMusicBrowse_RestoreView_NilSourceIsNoop(t *testing.T) {
	var s MusicBrowseScreen // source == nil; must not panic
	_ = s.RestoreView()
}

// streamingMockClient satisfies catalogbrowser.PluginIPCClient.
// Returns the given channel so tests can control the stream.
type streamingMockClient struct {
	ch chan ipc.ScopeResultsMsg
}

func (m *streamingMockClient) Search(_ context.Context, _ string, _ []ipc.SearchScope) (uint64, <-chan ipc.ScopeResultsMsg, error) {
	return 1, m.ch, nil
}
