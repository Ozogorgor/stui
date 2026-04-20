package catalogbrowser

import (
	"context"
	"fmt"
	"regexp"
	"sync"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
)

// yearRegex finds the first 19xx or 20xx four-digit year anywhere in a string.
// Mirrors the same pattern in music_library.go to keep album title rendering
// identical between the stable view and search results.
var yearRegex = regexp.MustCompile(`(?:19|20)\d{2}`)

// extractYear pulls a 4-digit year out of an arbitrary date string.
func extractYear(s string) string {
	return yearRegex.FindString(s)
}

// IPCClient is the subset of ipc.Client that MpdDataSource uses.
// Defined as an interface so tests can mock it without standing up a full
// transport.
type IPCClient interface {
	MpdSearch(ctx context.Context, query string, scopes []ipc.MpdScope) (*ipc.MpdSearchResult, error)
}

// MpdDataSource is a DataSource backed by the MPD bridge. Items() returns
// state populated externally (the music_library screen still owns MPD list
// fetches today); Search() dispatches a synchronous MpdSearch IPC call
// and applies the result to local state, snapshotting prior view first
// so RestoreView restores it cleanly.
//
// HasMultipleSources() always returns false — MPD is a single backend.
//
// Concurrency: the Search tea.Cmd runs on Bubble Tea's worker goroutine and
// mutates items/status/snapshot while View/Update on the model goroutine may
// be reading them. All mutating + reading operations take mu to prevent the
// race. Mirrors PluginDataSource's discipline.
type MpdDataSource struct {
	client IPCClient

	mu sync.Mutex

	// current view (set externally by music_library, replaced by search results)
	items map[ipc.EntryKind][]Entry

	// pre-search snapshot (nil when not in a search)
	snapshot *DataSourceState

	status SearchStatus
}

// NewMpdDataSource constructs an MpdDataSource backed by the given IPCClient.
func NewMpdDataSource(client IPCClient) *MpdDataSource {
	return &MpdDataSource{
		client: client,
		items:  map[ipc.EntryKind][]Entry{},
	}
}

// SetItems replaces the current view for a kind. Called by music_library
// when it receives MPD list-result messages.
func (s *MpdDataSource) SetItems(kind ipc.EntryKind, entries []Entry) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.items[kind] = entries
}

// SetAll replaces the entire view. Convenience for music_library's
// post-list-fetch syncStub-style updates.
func (s *MpdDataSource) SetAll(items map[ipc.EntryKind][]Entry) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.items = items
}

// Items returns the current entries for the given kind.
func (s *MpdDataSource) Items(kind ipc.EntryKind) []Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Entry(nil), s.items[kind]...)
}

// HasMultipleSources returns false — MPD is a single backend.
func (s *MpdDataSource) HasMultipleSources() bool { return false }

// Status returns the current search state.
func (s *MpdDataSource) Status() SearchStatus {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.status
}

// Snapshot returns a deep copy of the current view state.
func (s *MpdDataSource) Snapshot() DataSourceState {
	s.mu.Lock()
	defer s.mu.Unlock()
	cp := make(map[ipc.EntryKind][]Entry, len(s.items))
	for k, v := range s.items {
		cp[k] = append([]Entry(nil), v...)
	}
	return DataSourceState{Items: cp}
}

// Restore replaces the current view with a prior snapshot and clears search
// state. Called when the user dismisses or clears a search.
func (s *MpdDataSource) Restore(st DataSourceState) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if st.Items == nil {
		s.items = map[ipc.EntryKind][]Entry{}
	} else {
		s.items = st.Items
	}
	s.snapshot = nil
	s.status = SearchStatus{}
}

// RestoreSnapshot restores the view to the state captured when the most
// recent search was dispatched. No-op if no search has been run (snapshot
// is nil) or it was already cleared by Restore(). Used by the Searchable
// RestoreView path when the user clears / esc's out of a search so the
// library falls back to the pre-search listing without needing the caller
// to track the snapshot itself.
func (s *MpdDataSource) RestoreSnapshot() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.snapshot != nil {
		s.items = s.snapshot.Items
		s.snapshot = nil
		s.status = SearchStatus{}
	}
}

// MpdSearchAppliedMsg is posted into the Bubbletea loop when MpdSearch
// completes successfully. music_library's Update should observe it and
// trigger a re-render. Task 6.2 wires the Searchable surface; until then
// the message is defined but not routed.
type MpdSearchAppliedMsg struct {
	QueryID uint64
	Updated bool // false when an error occurred and items were not modified
}

// MpdSearchFailedMsg is posted on transport error or MPD application error.
type MpdSearchFailedMsg struct {
	QueryID uint64
	Err     error
}

// Search dispatches an MpdSearch request and replaces items on success.
// A snapshot of the current view is captured on the first search call so that
// Restore() can return to the pre-search state cleanly.
func (s *MpdDataSource) Search(ctx context.Context, query string, kinds []ipc.EntryKind) tea.Cmd {
	s.mu.Lock()
	if s.snapshot == nil {
		cp := make(map[ipc.EntryKind][]Entry, len(s.items))
		for k, v := range s.items {
			cp[k] = append([]Entry(nil), v...)
		}
		s.snapshot = &DataSourceState{Items: cp}
	}
	s.status = SearchStatus{Active: true, Partial: false, Query: query}
	s.mu.Unlock()

	scopes := mpdScopesFor(kinds)

	return func() tea.Msg {
		result, err := s.client.MpdSearch(ctx, query, scopes)
		if err != nil {
			s.mu.Lock()
			s.status = SearchStatus{}
			s.mu.Unlock()
			return MpdSearchFailedMsg{Err: err}
		}
		if result.Error != nil {
			s.mu.Lock()
			s.status = SearchStatus{}
			s.mu.Unlock()
			return MpdSearchFailedMsg{
				QueryID: result.QueryID,
				Err:     fmt.Errorf("mpd: %s: %s", result.Error.Type, result.Error.Message),
			}
		}
		s.mu.Lock()
		s.items = map[ipc.EntryKind][]Entry{
			ipc.KindArtist: MapMpdArtists(result.Artists),
			ipc.KindAlbum:  MapMpdAlbums(result.Albums),
			ipc.KindTrack:  MapMpdSongs(result.Tracks),
		}
		s.status = SearchStatus{Active: true, Partial: false, Query: query, QueryID: result.QueryID}
		s.mu.Unlock()
		return MpdSearchAppliedMsg{QueryID: result.QueryID, Updated: true}
	}
}

func mpdScopesFor(kinds []ipc.EntryKind) []ipc.MpdScope {
	out := make([]ipc.MpdScope, 0, len(kinds))
	for _, k := range kinds {
		switch k {
		case ipc.KindArtist:
			out = append(out, ipc.MpdScopeArtist)
		case ipc.KindAlbum:
			out = append(out, ipc.MpdScopeAlbum)
		case ipc.KindTrack:
			out = append(out, ipc.MpdScopeTrack)
		}
	}
	return out
}

// MapMpdArtists converts wire MpdArtist rows into catalogbrowser Entry values.
// Exported so music_library's syncStub can forward its slices without
// duplicating the mapping logic.
func MapMpdArtists(rows []ipc.MpdArtist) []Entry {
	out := make([]Entry, 0, len(rows))
	for _, r := range rows {
		out = append(out, Entry{
			ID:     r.Name,
			Kind:   ipc.KindArtist,
			Title:  r.Name,
			Source: "Local",
		})
	}
	return out
}

// MapMpdAlbums converts wire MpdAlbum rows into catalogbrowser Entry values.
// Year prefix mirrors mpdLibraryStub: extractYear(a.Year) produces the
// 4-digit display year from MPD's raw Year tag (e.g. "1996" or "1996-11-01").
// Exported so music_library's syncStub can forward its slices without
// duplicating the mapping logic.
func MapMpdAlbums(rows []ipc.MpdAlbum) []Entry {
	out := make([]Entry, 0, len(rows))
	for _, r := range rows {
		title := r.Title
		if year := extractYear(r.Year); year != "" {
			title = "(" + year + ") " + r.Title
		}
		out = append(out, Entry{
			ID:         r.Title,
			Kind:       ipc.KindAlbum,
			Title:      title,
			ArtistName: r.Artist,
			Source:     "Local",
		})
	}
	return out
}

// MapMpdSongs converts wire MpdSong rows into catalogbrowser Entry values.
// Exported so music_library's syncStub can forward its slices without
// duplicating the mapping logic.
func MapMpdSongs(rows []ipc.MpdSong) []Entry {
	out := make([]Entry, 0, len(rows))
	for _, r := range rows {
		out = append(out, Entry{
			ID:         r.File,
			Kind:       ipc.KindTrack,
			Title:      r.Title,
			ArtistName: r.Artist,
			AlbumName:  r.Album,
			Duration:   uint32(r.Duration),
			Source:     "Local",
		})
	}
	return out
}
