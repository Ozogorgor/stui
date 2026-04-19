package catalogbrowser

import (
	"context"
	"errors"
	"testing"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
)

// streamingMockClient implements PluginIPCClient for tests.
type streamingMockClient struct {
	qid        uint64
	err        error
	ch         chan ipc.ScopeResultsMsg
	lastQuery  string
	lastScopes []ipc.SearchScope
}

func newStreamingMockClient() *streamingMockClient {
	return &streamingMockClient{
		qid: 1,
		ch:  make(chan ipc.ScopeResultsMsg, 16),
	}
}

func (m *streamingMockClient) Search(_ context.Context, q string, scopes []ipc.SearchScope) (uint64, <-chan ipc.ScopeResultsMsg, error) {
	m.lastQuery = q
	m.lastScopes = scopes
	if m.err != nil {
		return 0, nil, m.err
	}
	return m.qid, m.ch, nil
}

// drainCmd executes a tea.Cmd, follows any Followup cmds embedded in
// ScopeResultsAppliedMsg / StaleScopeDroppedMsg, and returns all messages
// received until SearchChannelClosedMsg or max messages are consumed.
func drainCmd(t *testing.T, cmd tea.Cmd, max int) []tea.Msg {
	t.Helper()
	out := []tea.Msg{}
	for i := 0; i < max && cmd != nil; i++ {
		msg := cmd()
		out = append(out, msg)
		switch v := msg.(type) {
		case ScopeResultsAppliedMsg:
			cmd = v.Followup
		case StaleScopeDroppedMsg:
			cmd = v.Followup
		case SearchChannelClosedMsg:
			return out
		default:
			return out
		}
	}
	return out
}

func TestPluginDataSource_HasMultipleSources_True(t *testing.T) {
	s := NewPluginDataSource(newStreamingMockClient())
	if !s.HasMultipleSources() {
		t.Fatal("plugin source should have multiple sources")
	}
}

func TestPluginDataSource_StreamsPerScopeUpdatesItems(t *testing.T) {
	client := newStreamingMockClient()
	s := NewPluginDataSource(client)

	cmd := s.Search(context.Background(), "creep",
		[]ipc.EntryKind{ipc.KindArtist, ipc.KindTrack})

	// Push two scope results then close the channel.
	client.ch <- ipc.ScopeResultsMsg{
		QueryID: 1, Scope: ipc.ScopeArtist, Partial: false,
		Entries: []ipc.MediaEntry{{ID: "a1", Title: "Radiohead", Kind: ipc.KindArtist, Source: "lastfm"}},
	}
	client.ch <- ipc.ScopeResultsMsg{
		QueryID: 1, Scope: ipc.ScopeTrack, Partial: false,
		Entries: []ipc.MediaEntry{{ID: "t1", Title: "Creep", Kind: ipc.KindTrack, Source: "spotify"}},
	}
	close(client.ch)

	msgs := drainCmd(t, cmd, 5)
	if len(msgs) < 2 {
		t.Fatalf("expected >=2 msgs, got %d", len(msgs))
	}

	if got := s.Items(ipc.KindArtist); len(got) != 1 || got[0].Source != "lastfm" {
		t.Fatalf("artist not applied: %+v", got)
	}
	if got := s.Items(ipc.KindTrack); len(got) != 1 || got[0].Title != "Creep" {
		t.Fatalf("track not applied: %+v", got)
	}
}

func TestPluginDataSource_StaleQueryIDDropped(t *testing.T) {
	client := newStreamingMockClient()
	s := NewPluginDataSource(client)

	cmd := s.Search(context.Background(), "creep", []ipc.EntryKind{ipc.KindArtist})

	// Push a result with a stale qid (different from the one returned by Search).
	client.ch <- ipc.ScopeResultsMsg{
		QueryID: 999, Scope: ipc.ScopeArtist, Partial: false,
		Entries: []ipc.MediaEntry{{ID: "ghost", Title: "Should Not Apply"}},
	}
	close(client.ch)

	drainCmd(t, cmd, 3)
	if got := s.Items(ipc.KindArtist); len(got) != 0 {
		t.Fatalf("stale id should not have applied: %+v", got)
	}
}

func TestPluginDataSource_DispatchErrSurfaces(t *testing.T) {
	client := newStreamingMockClient()
	client.err = errors.New("transport down")
	s := NewPluginDataSource(client)

	cmd := s.Search(context.Background(), "x", []ipc.EntryKind{ipc.KindArtist})
	msg := cmd()
	if _, ok := msg.(SearchDispatchFailedMsg); !ok {
		t.Fatalf("expected SearchDispatchFailedMsg, got %T", msg)
	}
}

func TestPluginDataSource_RestoreReturnsPriorView(t *testing.T) {
	client := newStreamingMockClient()
	s := NewPluginDataSource(client)

	// Seed items before first search.
	s.mu.Lock()
	s.items[ipc.KindArtist] = []Entry{{ID: "before", Title: "Before"}}
	s.mu.Unlock()

	snap := s.Snapshot()

	cmd := s.Search(context.Background(), "after", []ipc.EntryKind{ipc.KindArtist})
	client.ch <- ipc.ScopeResultsMsg{
		QueryID: 1, Scope: ipc.ScopeArtist, Partial: false,
		Entries: []ipc.MediaEntry{{ID: "after", Title: "After"}},
	}
	close(client.ch)
	drainCmd(t, cmd, 3)

	if got := s.Items(ipc.KindArtist); len(got) == 0 || got[0].Title != "After" {
		t.Fatalf("mid-search state wrong: %+v", got)
	}
	s.Restore(snap)
	if got := s.Items(ipc.KindArtist); len(got) == 0 || got[0].Title != "Before" {
		t.Fatalf("restore failed: %+v", got)
	}
}

func TestPluginDataSource_YearParsed(t *testing.T) {
	year := "1997-05-21"
	entry := ipc.MediaEntry{
		ID: "a1", Title: "OK Computer", Kind: ipc.KindAlbum,
		Source: "mb", Year: &year,
	}
	entries := mapMediaEntries([]ipc.MediaEntry{entry})
	if len(entries) != 1 {
		t.Fatalf("expected 1 entry, got %d", len(entries))
	}
	if entries[0].Year != 1997 {
		t.Fatalf("expected Year=1997, got %d", entries[0].Year)
	}
}

func TestPluginDataSource_ScopeSnapshotOnlyOnFirstSearch(t *testing.T) {
	client1 := newStreamingMockClient()
	s := NewPluginDataSource(client1)

	s.mu.Lock()
	s.items[ipc.KindArtist] = []Entry{{ID: "orig", Title: "Original"}}
	s.mu.Unlock()

	// First search — snapshot should capture "Original".
	cmd1 := s.Search(context.Background(), "first", []ipc.EntryKind{ipc.KindArtist})
	client1.ch <- ipc.ScopeResultsMsg{
		QueryID: 1, Scope: ipc.ScopeArtist, Partial: false,
		Entries: []ipc.MediaEntry{{ID: "r1", Title: "Result1"}},
	}
	close(client1.ch)
	drainCmd(t, cmd1, 3)

	// Second search on same source — snapshot should NOT be overwritten.
	client2 := newStreamingMockClient()
	client2.qid = 2
	s.client = client2
	cmd2 := s.Search(context.Background(), "second", []ipc.EntryKind{ipc.KindArtist})
	client2.ch <- ipc.ScopeResultsMsg{
		QueryID: 2, Scope: ipc.ScopeArtist, Partial: false,
		Entries: []ipc.MediaEntry{{ID: "r2", Title: "Result2"}},
	}
	close(client2.ch)
	drainCmd(t, cmd2, 3)

	// Restore should go back to "Original" (the first snapshot).
	s.Restore(*s.snapshot)
	if got := s.Items(ipc.KindArtist); len(got) == 0 || got[0].Title != "Original" {
		t.Fatalf("snapshot overwritten by second search: %+v", got)
	}
}

func TestPluginDataSource_StatusBecomesInactiveOnClose(t *testing.T) {
	client := newStreamingMockClient()
	s := NewPluginDataSource(client)

	cmd := s.Search(context.Background(), "q", []ipc.EntryKind{ipc.KindTrack})

	st := s.Status()
	if !st.Active || !st.Partial {
		t.Fatalf("expected Active+Partial after Search(), got %+v", st)
	}

	close(client.ch)
	drainCmd(t, cmd, 2)

	st = s.Status()
	if st.Partial {
		t.Fatalf("expected Partial=false after channel closed, got %+v", st)
	}
}
