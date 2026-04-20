package screens

// music_library_searchable.go — MusicLibraryScreen's implementation of the
// Searchable + searchableLibrary interfaces introduced in Task 6.1.
//
// Kept in its own file so the refactor surface is visible at a glance and
// the heavy file (music_library.go) doesn't grow further. Value receivers
// throughout — the searchableLibrary interface in music_screen.go is
// matched by the value type, which is what MusicScreen holds in its
// `library` field and what the any(...).( ) assertion sees.

import (
	"context"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
)

// SearchScopes returns the scope set for Music Library search: artists,
// albums, and tracks — the three columns the browser renders.
func (s MusicLibraryScreen) SearchScopes() []ipc.SearchScope {
	return []ipc.SearchScope{ipc.ScopeArtist, ipc.ScopeAlbum, ipc.ScopeTrack}
}

// SearchPlaceholder is the top-bar input's placeholder text while this
// screen is focused.
func (s MusicLibraryScreen) SearchPlaceholder() string {
	return "Search library…"
}

// StartSearch dispatches an MPD-backed search via the screen's
// MpdDataSource. The source's Search() returns a tea.Cmd that performs
// the IPC call, mutates items in place on success, and posts
// catalogbrowser.MpdSearchAppliedMsg (or MpdSearchFailedMsg on error).
//
// Empty query is a no-op; the routing layer calls RestoreView separately
// when the user clears the input.
func (s MusicLibraryScreen) StartSearch(query string) tea.Cmd {
	if query == "" || s.source == nil {
		return nil
	}
	return s.source.Search(context.Background(), query, []ipc.EntryKind{
		ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack,
	})
}

// OnScopeResults — MPD does not stream scoped results; the scoped-result
// path is for plugin-backed sources (Music Browse, movies/series). No-op
// for Music Library.
func (s MusicLibraryScreen) OnScopeResults(_ ipc.ScopeResultsMsg) (MusicLibraryScreen, tea.Cmd) {
	return s, nil
}

// OnMpdSearchResult — the MpdDataSource.Search cmd callback already
// applies results to local state and posts MpdSearchAppliedMsg. Routing
// through this method is redundant for Music Library; it's kept for
// interface symmetry with Music Browse, which needs it.
func (s MusicLibraryScreen) OnMpdSearchResult(_ ipc.MpdSearchResult) (MusicLibraryScreen, tea.Cmd) {
	return s, nil
}

// RestoreView returns the screen to its pre-search state by restoring
// the MpdDataSource's internal snapshot (captured on the first Search
// call). Safe to call when no search is active.
func (s MusicLibraryScreen) RestoreView() MusicLibraryScreen {
	if s.source != nil {
		s.source.RestoreSnapshot()
	}
	return s
}
