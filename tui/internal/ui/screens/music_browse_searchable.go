package screens

// music_browse_searchable.go — MusicBrowseScreen's implementation of the
// Searchable + searchableBrowse interfaces introduced in Task 6.1.
//
// Kept in its own file so the refactor surface is visible at a glance and
// the heavy file (music_browse.go) doesn't grow further. Value receivers
// throughout — the searchableBrowse interface in music_screen.go is
// matched by the value type, which is what MusicScreen holds in its
// `browse` field and what the any(...).( ) assertion sees.

import (
	"context"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screens/catalogbrowser"
)

// SearchScopes returns the scope set for Music Browse search: artists,
// albums, and tracks — the three kinds plugin-backed search supports.
func (s MusicBrowseScreen) SearchScopes() []ipc.SearchScope {
	return []ipc.SearchScope{ipc.ScopeArtist, ipc.ScopeAlbum, ipc.ScopeTrack}
}

// SearchPlaceholder is the top-bar input's placeholder text while this
// screen is focused.
func (s MusicBrowseScreen) SearchPlaceholder() string {
	return "Search music providers…"
}

// StartSearch dispatches a plugin-backed streaming search via the screen's
// PluginDataSource. The source's Search() returns a tea.Cmd that performs
// the IPC call; each ScopeResultsAppliedMsg it posts carries a Followup cmd
// that the Update must dispatch to read the next scope result from the stream.
//
// Empty query is a no-op; the routing layer calls RestoreView separately
// when the user clears the input.
func (s MusicBrowseScreen) StartSearch(query string) tea.Cmd {
	if query == "" || s.source == nil {
		return nil
	}
	return s.source.Search(context.Background(), query, []ipc.EntryKind{
		ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack,
	})
}

// OnScopeResults — PluginDataSource's cmd-callback already applies items
// and posts ScopeResultsAppliedMsg (which Update dispatches via Followup).
// Routing through this interface method is redundant; kept for symmetry
// with searchableBrowse and to mirror Music Library's OnScopeResults noop.
func (s MusicBrowseScreen) OnScopeResults(_ ipc.ScopeResultsMsg) (MusicBrowseScreen, tea.Cmd) {
	return s, nil
}

// OnMpdSearchResult — Music Browse is plugin-backed; the MPD search path
// is unused. No-op for interface symmetry with searchableBrowse.
func (s MusicBrowseScreen) OnMpdSearchResult(_ ipc.MpdSearchResult) (MusicBrowseScreen, tea.Cmd) {
	return s, nil
}

// RestoreView returns the screen to its pre-search state by restoring the
// PluginDataSource's internal snapshot (captured on the first Search call).
// Safe to call when no search is active (RestoreSnapshot is a no-op then).
func (s MusicBrowseScreen) RestoreView() MusicBrowseScreen {
	if s.source != nil {
		s.source.RestoreSnapshot()
	}
	return s
}

// Compile-time assertions: MusicBrowseScreen must satisfy both the public
// Searchable interface and the private searchableBrowse narrowing interface
// from music_screen.go. Without these, the interface assertions in
// MusicScreen.Apply* would silently degrade to no-ops at runtime.
var (
	_ Searchable                        = MusicBrowseScreen{}
	_ searchableBrowse                  = MusicBrowseScreen{}
	_ catalogbrowser.PluginIPCClient    = (*ipc.Client)(nil) // sanity: ipc.Client satisfies the source's client interface
)
