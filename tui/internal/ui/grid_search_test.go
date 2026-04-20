package ui

// grid_search_test.go — unit tests for the gridSearchable Searchable adapter
// and its helper conversion functions.
//
// Positive-path StartSearch coverage is deliberately limited here because
// exercising it requires a live *ipc.Client. The scope/placeholder matrices
// and the empty-query / nil-client guards are covered; routing and stale-QID
// handling are exercised indirectly via the gridScopeAppliedMsg unit tests
// and the Update switch.

import (
	"testing"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/screens"
)

func TestGridSearchable_ScopesPerTab(t *testing.T) {
	cases := []struct {
		tab  state.Tab
		want []ipc.SearchScope
	}{
		{state.TabMovies, []ipc.SearchScope{ipc.ScopeMovie}},
		{state.TabSeries, []ipc.SearchScope{ipc.ScopeSeries}},
		{state.TabLibrary, []ipc.SearchScope{ipc.ScopeMovie, ipc.ScopeSeries}},
	}
	for _, tc := range cases {
		g := gridSearchable{tab: tc.tab}
		got := g.SearchScopes()
		if len(got) != len(tc.want) {
			t.Fatalf("tab %v: got %d scopes, want %d (%v vs %v)", tc.tab, len(got), len(tc.want), got, tc.want)
		}
		for i := range got {
			if got[i] != tc.want[i] {
				t.Errorf("tab %v: scope[%d] = %q, want %q", tc.tab, i, got[i], tc.want[i])
			}
		}
	}
}

func TestGridSearchable_ScopesForNonGridTabIsNil(t *testing.T) {
	// Non-grid tabs should never be wrapped in a gridSearchable, but
	// defensively the scope set is nil for Music and Collections so
	// callers that accidentally construct one get zero behavior.
	for _, tab := range []state.Tab{state.TabMusic, state.TabCollections} {
		g := gridSearchable{tab: tab}
		if got := g.SearchScopes(); got != nil {
			t.Errorf("tab %v: expected nil scopes, got %v", tab, got)
		}
	}
}

func TestGridSearchable_PlaceholderPerTab(t *testing.T) {
	cases := []struct {
		tab  state.Tab
		want string
	}{
		{state.TabMovies, "Search movies\u2026"},
		{state.TabSeries, "Search series\u2026"},
		{state.TabLibrary, "Search library\u2026"},
		{state.TabCollections, "Search\u2026"},
	}
	for _, tc := range cases {
		g := gridSearchable{tab: tc.tab}
		if got := g.SearchPlaceholder(); got != tc.want {
			t.Errorf("tab %v: placeholder = %q, want %q", tc.tab, got, tc.want)
		}
	}
}

func TestGridSearchable_StartSearch_EmptyQuery(t *testing.T) {
	m := &Model{}
	m.gridSearchSnapshot = make(map[state.Tab][]ipc.CatalogEntry)
	m.gridSearchActiveQID = make(map[state.Tab]uint64)
	g := gridSearchable{m: m, tab: state.TabMovies}
	if cmd := g.StartSearch(""); cmd != nil {
		t.Error("empty query: expected nil cmd")
	}
	// No snapshot should have been captured.
	if _, has := m.gridSearchSnapshot[state.TabMovies]; has {
		t.Error("empty query: no snapshot should have been captured")
	}
}

func TestGridSearchable_StartSearch_NilClient(t *testing.T) {
	m := &Model{}
	m.grids = map[string][]ipc.CatalogEntry{"movies": nil}
	m.gridSearchSnapshot = make(map[state.Tab][]ipc.CatalogEntry)
	m.gridSearchActiveQID = make(map[state.Tab]uint64)
	g := gridSearchable{m: m, tab: state.TabMovies}
	if cmd := g.StartSearch("inception"); cmd != nil {
		t.Error("nil client: expected nil cmd")
	}
}

func TestGridSearchable_StartSearch_NilModel(t *testing.T) {
	g := gridSearchable{m: nil, tab: state.TabMovies}
	if cmd := g.StartSearch("inception"); cmd != nil {
		t.Error("nil model: expected nil cmd")
	}
}

func TestFocusedSearchable_GridTabsAreSearchable(t *testing.T) {
	gridTabs := []state.Tab{state.TabMovies, state.TabSeries, state.TabLibrary}
	for _, tab := range gridTabs {
		m := minimalModel(tab)
		got := focusedSearchable(&m)
		if got == nil {
			t.Errorf("tab %v: expected non-nil Searchable from focusedSearchable", tab)
			continue
		}
		gs, ok := got.(gridSearchable)
		if !ok {
			t.Errorf("tab %v: expected gridSearchable, got %T", tab, got)
			continue
		}
		if gs.tab != tab {
			t.Errorf("tab %v: gridSearchable.tab = %v, want %v", tab, gs.tab, tab)
		}
	}
}

func TestFocusedSearchable_CollectionsStillReturnsNil(t *testing.T) {
	m := minimalModel(state.TabCollections)
	if got := focusedSearchable(&m); got != nil {
		t.Errorf("TabCollections: expected nil Searchable, got %T", got)
	}
}

func TestMediaEntriesToCatalog_PreservesFields(t *testing.T) {
	year := "2010"
	genre := "Sci-Fi"
	rating := "8.8"
	desc := "Dream within a dream."
	poster := "https://img/incept.jpg"
	entries := []ipc.MediaEntry{
		{
			ID:          "tt1375666",
			Title:       "Inception",
			Year:        &year,
			Genre:       &genre,
			Rating:      &rating,
			Description: &desc,
			PosterURL:   &poster,
			Provider:    "tmdb",
			Tab:         ipc.TabMovies,
			Kind:        ipc.KindMovie,
			Source:      "tmdb",
		},
	}
	got := mediaEntriesToCatalog(entries)
	if len(got) != 1 {
		t.Fatalf("got %d entries, want 1", len(got))
	}
	ce := got[0]
	if ce.ID != "tt1375666" || ce.Title != "Inception" {
		t.Errorf("id/title: got %q / %q", ce.ID, ce.Title)
	}
	if ce.Year == nil || *ce.Year != "2010" {
		t.Errorf("year: got %v, want 2010", ce.Year)
	}
	if ce.Kind != ipc.KindMovie {
		t.Errorf("kind: got %q, want %q", ce.Kind, ipc.KindMovie)
	}
	if ce.Source != "tmdb" {
		t.Errorf("source: got %q, want tmdb", ce.Source)
	}
	if ce.Tab != "movies" {
		t.Errorf("tab: got %q, want movies", ce.Tab)
	}
	if ce.Description == nil || *ce.Description != desc {
		t.Errorf("description not preserved: got %v", ce.Description)
	}
	if ce.PosterURL == nil || *ce.PosterURL != poster {
		t.Errorf("poster url not preserved: got %v", ce.PosterURL)
	}
}

func TestScopeKind_ValuesAlign(t *testing.T) {
	cases := []struct {
		scope ipc.SearchScope
		want  ipc.EntryKind
	}{
		{ipc.ScopeMovie, ipc.KindMovie},
		{ipc.ScopeSeries, ipc.KindSeries},
		{ipc.ScopeAlbum, ipc.KindAlbum},
		{ipc.ScopeTrack, ipc.KindTrack},
	}
	for _, tc := range cases {
		if got := scopeKind(tc.scope); got != tc.want {
			t.Errorf("scopeKind(%q) = %q, want %q", tc.scope, got, tc.want)
		}
	}
}

// Compile-time check: gridSearchable satisfies screens.Searchable.
var _ screens.Searchable = gridSearchable{}
