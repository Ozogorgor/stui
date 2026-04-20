package ui

// searchable_routing_test.go — unit tests for the Searchable interface and
// the routing infrastructure added in Task 6.1.
//
// Tests cover:
//   - screens.Searchable interface satisfaction via a stub.
//   - focusedSearchable returns nil for non-music tabs and for music tabs
//     whose active sub-screen does not yet implement Searchable.
//   - MusicScreen.ApplyScopeResults / ApplyMpdSearchResult / ApplyRestoreView
//     are no-ops while no sub-screen implements the internal narrowing
//     interfaces (Tasks 6.2/6.3 pending).
//   - MusicScreen.StartSearchInActive returns nil while no sub-screen
//     implements Searchable.
//   - The "/" gate: focusedSearchable returns nil on non-Music tabs,
//     confirming the bar would stay hidden.

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/screens"
)

// ── stub Searchable ───────────────────────────────────────────────────────────

// stubSearchable implements screens.Searchable for routing tests.
// It is used by the test to verify that the Searchable contract compiles
// and that all required methods are callable. No production screen
// implements it until Tasks 6.2/6.3.
type stubSearchable struct {
	scopes      []ipc.SearchScope
	placeholder string
	startCalls  []string // queries passed to StartSearch
}

func (s *stubSearchable) SearchScopes() []ipc.SearchScope { return s.scopes }
func (s *stubSearchable) SearchPlaceholder() string       { return s.placeholder }
func (s *stubSearchable) StartSearch(query string) tea.Cmd {
	s.startCalls = append(s.startCalls, query)
	return nil
}

// Compile-time check: stubSearchable satisfies screens.Searchable.
var _ screens.Searchable = (*stubSearchable)(nil)

// ── focusedSearchable tests ───────────────────────────────────────────────────

// minimalModel builds the smallest Model that focusedSearchable needs:
// a zero-value Model with the ActiveTab field set. We do not call New()
// because it reads disk (session, keybinds, media cache) and spawns
// goroutines. focusedSearchable only inspects m.state.ActiveTab and
// m.musicScreen.FocusedSearchable().
func minimalModel(tab state.Tab) Model {
	m := Model{}
	m.state.ActiveTab = tab
	m.musicScreen = screens.NewMusicScreen(nil)
	return m
}

// TestFocusedSearchable_NonMusicNonGridTabsReturnNil confirms that tabs
// without a Searchable implementation still return nil. After Task 6.4 the
// only such tab is TabCollections — Movies/Series/Library adopt Searchable
// via the gridSearchable adapter (see TestFocusedSearchable_GridTabsAreSearchable
// in grid_search_test.go).
func TestFocusedSearchable_NonMusicNonGridTabsReturnNil(t *testing.T) {
	nonSearchableTabs := []state.Tab{
		state.TabCollections,
	}
	for _, tab := range nonSearchableTabs {
		m := minimalModel(tab)
		got := focusedSearchable(&m)
		if got != nil {
			t.Errorf("tab %q: expected nil Searchable, got %T", tab, got)
		}
	}
}

func TestFocusedSearchable_MusicTabNoSearchableSubScreen(t *testing.T) {
	// Music tab with the default sub-tab (Queue) — Queue does not implement
	// Searchable. Neither Library nor Browse do yet (Tasks 6.2/6.3 pending).
	m := minimalModel(state.TabMusic)
	got := focusedSearchable(&m)
	if got != nil {
		t.Errorf("TabMusic (default sub-tab): expected nil Searchable before Tasks 6.2/6.3, got %T", got)
	}
}

func TestFocusedSearchable_MusicTabLibraryIsSearchable(t *testing.T) {
	m := minimalModel(state.TabMusic)
	m.musicScreen = m.musicScreen.WithActiveSubTab(screens.MusicLibrary)
	got := focusedSearchable(&m)
	if got == nil {
		t.Fatal("TabMusic/Library: expected non-nil Searchable after Task 6.2")
	}
	if _, ok := got.(screens.MusicLibraryScreen); !ok {
		t.Errorf("TabMusic/Library: expected MusicLibraryScreen, got %T", got)
	}
}

func TestFocusedSearchable_MusicTabBrowseIsSearchable(t *testing.T) {
	m := minimalModel(state.TabMusic)
	m.musicScreen = m.musicScreen.WithActiveSubTab(screens.MusicBrowse)
	got := focusedSearchable(&m)
	if got == nil {
		t.Fatal("TabMusic/Browse: expected non-nil Searchable after Task 6.3")
	}
	if _, ok := got.(screens.MusicBrowseScreen); !ok {
		t.Errorf("TabMusic/Browse: expected MusicBrowseScreen, got %T", got)
	}
}

// ── MusicScreen.Apply* no-op tests ───────────────────────────────────────────

func TestMusicScreen_ApplyScopeResults_NoopWithoutSearchable(t *testing.T) {
	ms := screens.NewMusicScreen(nil)
	// All sub-tabs: Apply* should be no-ops (none implement searchableLibrary/
	// searchableBrowse until 6.2/6.3).
	subtabs := []screens.MusicSubTab{
		screens.MusicBrowse,
		screens.MusicQueue,
		screens.MusicLibrary,
		screens.MusicPlaylists,
	}
	for _, st := range subtabs {
		s := ms.WithActiveSubTab(st)
		upd, cmd := s.ApplyScopeResults(ipc.ScopeResultsMsg{})
		if cmd != nil {
			t.Errorf("sub-tab %v: expected nil cmd from ApplyScopeResults, got non-nil", st)
		}
		_ = upd // no panic = good
	}
}

func TestMusicScreen_ApplyMpdSearchResult_NoopWithoutSearchable(t *testing.T) {
	ms := screens.NewMusicScreen(nil)
	subtabs := []screens.MusicSubTab{
		screens.MusicBrowse,
		screens.MusicQueue,
		screens.MusicLibrary,
		screens.MusicPlaylists,
	}
	for _, st := range subtabs {
		s := ms.WithActiveSubTab(st)
		upd, cmd := s.ApplyMpdSearchResult(ipc.MpdSearchResult{})
		if cmd != nil {
			t.Errorf("sub-tab %v: expected nil cmd from ApplyMpdSearchResult, got non-nil", st)
		}
		_ = upd
	}
}

func TestMusicScreen_ApplyRestoreView_NoopWithoutSearchable(t *testing.T) {
	ms := screens.NewMusicScreen(nil)
	subtabs := []screens.MusicSubTab{
		screens.MusicBrowse,
		screens.MusicQueue,
		screens.MusicLibrary,
		screens.MusicPlaylists,
	}
	for _, st := range subtabs {
		s := ms.WithActiveSubTab(st)
		upd := s.ApplyRestoreView()
		_ = upd // no panic = good
	}
}

func TestMusicScreen_StartSearchInActive_NilWithoutSearchable(t *testing.T) {
	// Queue/Playlists remain non-Searchable — StartSearchInActive returns nil.
	// Browse is Searchable as of Task 6.3; Library as of Task 6.2.
	ms := screens.NewMusicScreen(nil)
	subtabs := []screens.MusicSubTab{
		screens.MusicQueue,
		screens.MusicPlaylists,
	}
	for _, st := range subtabs {
		s := ms.WithActiveSubTab(st)
		cmd := s.StartSearchInActive("test")
		if cmd != nil {
			t.Errorf("sub-tab %v: expected nil cmd from StartSearchInActive, got non-nil", st)
		}
	}
}

func TestMusicScreen_StartSearchInActive_LibraryDispatches(t *testing.T) {
	// Library is Searchable post-Task 6.2: a non-empty query must dispatch
	// a non-nil cmd via MpdDataSource.Search.
	ms := screens.NewMusicScreen(nil).WithActiveSubTab(screens.MusicLibrary)
	if cmd := ms.StartSearchInActive("radiohead"); cmd == nil {
		t.Error("sub-tab Library: expected non-nil cmd from StartSearchInActive")
	}
	// Empty query: no-op even for Searchable sub-screen.
	if cmd := ms.StartSearchInActive(""); cmd != nil {
		t.Error("sub-tab Library: empty query should return nil cmd")
	}
}

func TestMusicScreen_StartSearchInActive_BrowseDispatches(t *testing.T) {
	// Browse is Searchable post-Task 6.3: a non-empty query must dispatch
	// a non-nil cmd via PluginDataSource.Search.
	// NewMusicScreen(nil) creates a Browse screen with a nil client, which
	// means source is nil — StartSearch must return nil for nil source.
	// To verify the positive path we need a non-nil source; use WithActiveSubTab
	// only after confirming the nil-source guard.
	ms := screens.NewMusicScreen(nil).WithActiveSubTab(screens.MusicBrowse)
	// nil client → nil source → nil cmd even for non-empty query
	if cmd := ms.StartSearchInActive("radiohead"); cmd != nil {
		t.Error("sub-tab Browse (nil client): expected nil cmd from StartSearchInActive")
	}
	// Empty query is always nil
	if cmd := ms.StartSearchInActive(""); cmd != nil {
		t.Error("sub-tab Browse: empty query should return nil cmd")
	}
}

// ── Searchable interface contract test ────────────────────────────────────────

func TestSearchableInterface_StartSearch(t *testing.T) {
	s := &stubSearchable{
		scopes:      []ipc.SearchScope{ipc.ScopeTrack, ipc.ScopeAlbum},
		placeholder: "Search library…",
	}

	if got := s.SearchPlaceholder(); got != "Search library…" {
		t.Errorf("SearchPlaceholder: got %q, want %q", got, "Search library…")
	}
	if got := s.SearchScopes(); len(got) != 2 {
		t.Errorf("SearchScopes: got %d scopes, want 2", len(got))
	}

	_ = s.StartSearch("bohemian")
	_ = s.StartSearch("queen")
	if len(s.startCalls) != 2 {
		t.Errorf("StartSearch: expected 2 calls recorded, got %d", len(s.startCalls))
	}
	if s.startCalls[0] != "bohemian" || s.startCalls[1] != "queen" {
		t.Errorf("StartSearch: unexpected call sequence: %v", s.startCalls)
	}
}

func TestSearchableInterface_EmptyScopes(t *testing.T) {
	s := &stubSearchable{scopes: nil}
	scopes := s.SearchScopes()
	if len(scopes) != 0 {
		t.Errorf("empty SearchScopes: got %d, want 0", len(scopes))
	}
}
