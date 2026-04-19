package catalogbrowser

import (
	"context"
	"strconv"
	"sync"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
)

// PluginIPCClient is the subset of ipc.Client that PluginDataSource uses.
type PluginIPCClient interface {
	Search(ctx context.Context, query string, scopes []ipc.SearchScope) (uint64, <-chan ipc.ScopeResultsMsg, error)
}

// PluginDataSource is a DataSource backed by the plugin engine. Search
// dispatches a streaming Search() request and reads ScopeResultsMsg from
// the returned channel; each message is applied to the matching column.
//
// Stale messages (query_id != active) are silently dropped; this is
// defense-in-depth against rapid retyping. The IPC client also filters
// at its subscription layer.
//
// HasMultipleSources() returns true: plugin-backed results carry source
// identity that the renderer surfaces in a Sources column.
type PluginDataSource struct {
	client PluginIPCClient

	mu       sync.Mutex
	items    map[ipc.EntryKind][]Entry
	snapshot *DataSourceState
	status   SearchStatus
	active   uint64 // current query_id; results with mismatching id ignored
}

// NewPluginDataSource constructs a PluginDataSource backed by the given client.
func NewPluginDataSource(client PluginIPCClient) *PluginDataSource {
	return &PluginDataSource{
		client: client,
		items:  map[ipc.EntryKind][]Entry{},
	}
}

// Items returns a copy of the current entries for the given kind.
// Returns a copy so the caller cannot mutate internal state.
func (s *PluginDataSource) Items(kind ipc.EntryKind) []Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Entry(nil), s.items[kind]...)
}

// HasMultipleSources returns true — plugin-backed results carry source
// identity (plugin id) that the renderer surfaces in a Sources column.
func (s *PluginDataSource) HasMultipleSources() bool { return true }

// Status returns the current search state.
func (s *PluginDataSource) Status() SearchStatus {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.status
}

// Snapshot returns a deep copy of the current view state.
func (s *PluginDataSource) Snapshot() DataSourceState {
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
func (s *PluginDataSource) Restore(st DataSourceState) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if st.Items == nil {
		s.items = map[ipc.EntryKind][]Entry{}
	} else {
		s.items = st.Items
	}
	s.snapshot = nil
	s.status = SearchStatus{}
	s.active = 0
}

// ScopeResultsAppliedMsg is posted to the Bubbletea loop after each
// applied ScopeResultsMsg. The Followup cmd, when non-nil, must be
// dispatched by the receiver — it reads the next message from the
// stream channel.
type ScopeResultsAppliedMsg struct {
	QueryID  uint64
	Scope    ipc.SearchScope
	Partial  bool
	Followup tea.Cmd
}

// SearchChannelClosedMsg is posted when the stream channel closes (all
// scopes have emitted partial=false).
type SearchChannelClosedMsg struct {
	QueryID uint64
}

// SearchDispatchFailedMsg is posted when the initial Search request
// could not be dispatched.
type SearchDispatchFailedMsg struct {
	Err error
}

// StaleScopeDroppedMsg is posted when a result with a stale query_id
// arrives. Carries a Followup cmd that continues draining.
type StaleScopeDroppedMsg struct {
	Followup tea.Cmd
}

// Search dispatches a streaming search and returns a tea.Cmd that, when
// executed, reads the first ScopeResultsMsg from the stream. The returned
// applied/closed msg carries a Followup cmd; the receiver dispatches it
// to read the next message. This pattern lets Bubbletea drive the receive
// loop without blocking goroutines.
//
// A snapshot of the current view is captured on the first search call so
// that Restore() can return to the pre-search state cleanly.
func (s *PluginDataSource) Search(ctx context.Context, query string, kinds []ipc.EntryKind) tea.Cmd {
	s.mu.Lock()
	if s.snapshot == nil {
		cp := make(map[ipc.EntryKind][]Entry, len(s.items))
		for k, v := range s.items {
			cp[k] = append([]Entry(nil), v...)
		}
		s.snapshot = &DataSourceState{Items: cp}
	}
	s.mu.Unlock()

	scopes := pluginScopesForKinds(kinds)
	qid, ch, err := s.client.Search(ctx, query, scopes)
	if err != nil {
		return func() tea.Msg { return SearchDispatchFailedMsg{Err: err} }
	}

	s.mu.Lock()
	s.active = qid
	s.status = SearchStatus{Active: true, Partial: true, Query: query, QueryID: qid}
	s.mu.Unlock()

	return s.nextScopeCmd(ch, qid)
}

// nextScopeCmd returns a tea.Cmd that reads the next ScopeResultsMsg from ch.
// Each returned msg embeds a Followup cmd to continue the loop. The pattern
// is reentrant: a new closure is created per call, all sharing the same ch
// and qid.
func (s *PluginDataSource) nextScopeCmd(ch <-chan ipc.ScopeResultsMsg, qid uint64) tea.Cmd {
	return func() tea.Msg {
		msg, ok := <-ch
		if !ok {
			s.mu.Lock()
			// Mark search inactive only if this stream is still the active one.
			if s.active == qid {
				s.status.Partial = false
			}
			s.mu.Unlock()
			return SearchChannelClosedMsg{QueryID: qid}
		}

		s.mu.Lock()
		if msg.QueryID != s.active {
			s.mu.Unlock()
			return StaleScopeDroppedMsg{Followup: s.nextScopeCmd(ch, qid)}
		}
		kind := kindForSearchScope(msg.Scope)
		if kind != "" {
			s.items[kind] = mapMediaEntries(msg.Entries)
		}
		s.status.Partial = msg.Partial
		s.mu.Unlock()

		return ScopeResultsAppliedMsg{
			QueryID:  msg.QueryID,
			Scope:    msg.Scope,
			Partial:  msg.Partial,
			Followup: s.nextScopeCmd(ch, qid),
		}
	}
}

// mapMediaEntries converts ipc.MediaEntry values to catalogbrowser.Entry values.
func mapMediaEntries(rows []ipc.MediaEntry) []Entry {
	out := make([]Entry, 0, len(rows))
	for _, r := range rows {
		e := Entry{
			ID:          r.ID,
			Kind:        r.Kind,
			Title:       r.Title,
			Source:      r.Source,
			ArtistName:  r.ArtistName,
			AlbumName:   r.AlbumName,
			TrackNumber: r.TrackNumber,
		}
		// MediaEntry.Year is *string (raw tag from plugin); parse to uint32 via
		// the shared extractYear helper (which strips dates like "1996-11-01"
		// down to "1996") then convert to uint32.
		if r.Year != nil {
			if y, err := strconv.ParseUint(extractYear(*r.Year), 10, 32); err == nil {
				e.Year = uint32(y)
			}
		}
		out = append(out, e)
	}
	return out
}

// pluginScopesForKinds converts EntryKind values to SearchScope values.
// EntryKind and SearchScope share identical string vocabularies ("artist",
// "album", "track", etc.), so direct string conversion is safe.
func pluginScopesForKinds(kinds []ipc.EntryKind) []ipc.SearchScope {
	out := make([]ipc.SearchScope, 0, len(kinds))
	for _, k := range kinds {
		out = append(out, ipc.SearchScope(k))
	}
	return out
}

// kindForSearchScope converts a SearchScope to the matching EntryKind.
// Returns an empty string for unrecognised scopes so the caller can skip
// the update rather than corrupt the items map.
func kindForSearchScope(s ipc.SearchScope) ipc.EntryKind {
	// The two string types share the same vocabulary, so direct conversion is
	// safe. An explicit cast is used rather than a switch to avoid needing
	// updates when new kinds are added to both types simultaneously.
	k := ipc.EntryKind(s)
	switch k {
	case ipc.KindArtist, ipc.KindAlbum, ipc.KindTrack,
		ipc.KindMovie, ipc.KindSeries, ipc.KindEpisode:
		return k
	default:
		return ""
	}
}
