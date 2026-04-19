package ipc

import (
	"testing"
	"time"
)

// newClientForTest returns a minimal *Client suitable for unit tests that
// exercise subscription-map and query-id logic without a live transport.
// The pending map is initialized so sendWithID does not panic; no goroutines
// are started and no IPC pipe is opened.
func newClientForTest() *Client {
	return &Client{
		pending: make(map[string]chan RawResponse),
	}
}

// ---------------------------------------------------------------------------
// NextQueryID
// ---------------------------------------------------------------------------

func TestClient_NextQueryID_Monotonic(t *testing.T) {
	c := newClientForTest()
	ids := []uint64{c.NextQueryID(), c.NextQueryID(), c.NextQueryID()}
	for i := 1; i < len(ids); i++ {
		if ids[i] <= ids[i-1] {
			t.Fatalf("NextQueryID not monotonic: %v", ids)
		}
	}
}

func TestClient_NextQueryID_StartsAboveZero(t *testing.T) {
	c := newClientForTest()
	id := c.NextQueryID()
	if id == 0 {
		t.Fatal("NextQueryID returned 0; first id should be >= 1")
	}
}

// ---------------------------------------------------------------------------
// Routing: basic single-scope finalization
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_Routing(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	ch := c.SubscribeScopeResults(qid, []SearchScope{ScopeArtist, ScopeTrack})

	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: false})
	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeTrack, Partial: false})

	got := make([]SearchScope, 0, 2)
	got = append(got, (<-ch).Scope)
	got = append(got, (<-ch).Scope)

	// Channel must be closed after both final messages.
	if _, open := <-ch; open {
		t.Fatal("channel should be closed after every expected scope finalized")
	}
	_ = got
}

// ---------------------------------------------------------------------------
// Routing: partial then final for a single scope
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_PartialThenFinal(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	ch := c.SubscribeScopeResults(qid, []SearchScope{ScopeArtist})

	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: true})
	m1 := <-ch
	if !m1.Partial {
		t.Fatal("expected partial=true on first message")
	}

	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: false})
	m2 := <-ch
	if m2.Partial {
		t.Fatal("expected partial=false on second message")
	}

	// After the final, channel should be closed.
	if _, open := <-ch; open {
		t.Fatal("channel should be closed after the final message for a single-scope subscription")
	}
}

// ---------------------------------------------------------------------------
// Routing: multi-scope — channel stays open until ALL scopes finalize
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_MultiScope_ChannelStaysOpenUntilAllFinal(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	ch := c.SubscribeScopeResults(qid, []SearchScope{ScopeArtist, ScopeAlbum})

	// Finalize artist only.
	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: false})
	<-ch

	// Channel should still be open (album hasn't finalized).
	select {
	case _, open := <-ch:
		if !open {
			t.Fatal("channel closed prematurely before all scopes finalized")
		}
		// A message came through — unexpected but not a fatal error here.
	default:
		// Nothing buffered — channel is open. Correct.
	}

	// Finalize album.
	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeAlbum, Partial: false})
	<-ch

	// Now both scopes are done; channel must be closed.
	if _, open := <-ch; open {
		t.Fatal("channel should be closed after all scopes finalized")
	}
}

// ---------------------------------------------------------------------------
// Stale / unknown query id — must not panic, block, or log at error
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_StaleQueryIDDropped(t *testing.T) {
	c := newClientForTest()
	// No subscriber registered for query id 9999.
	// dispatchScopeResults must return cleanly.
	c.dispatchScopeResults(ScopeResultsMsg{QueryID: 9999, Scope: ScopeArtist, Partial: false})
}

// ---------------------------------------------------------------------------
// Full buffer — overflow must not deadlock and must drop silently
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_ChannelFullDropsSilently(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	// Subscribe with three scopes so the channel isn't closed by the early
	// partial-only sends (which we never finalize in this test).
	ch := c.SubscribeScopeResults(qid, []SearchScope{ScopeArtist, ScopeAlbum, ScopeTrack})

	// Fire well more than the buffer capacity (8) with partial=true messages.
	// These must not block even when the channel fills up.
	for i := 0; i < 20; i++ {
		c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: true})
	}

	// Drain whatever landed in the buffer (up to 8).
	done := make(chan struct{})
	go func() {
		defer close(done)
		drained := 0
		for range ch {
			drained++
			if drained >= 8 {
				return
			}
		}
	}()

	select {
	case <-done:
		// Drained successfully — no deadlock.
	case <-time.After(1 * time.Second):
		t.Fatal("timed out draining; channel appears deadlocked")
	}
}

// ---------------------------------------------------------------------------
// GC: scopeSubs entry is removed after finalization
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_GCAfterFinalization(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	_ = c.SubscribeScopeResults(qid, []SearchScope{ScopeTrack})

	// Confirm the entry exists.
	if _, ok := c.scopeSubs.Load(qid); !ok {
		t.Fatal("scopeSubs entry should exist immediately after SubscribeScopeResults")
	}

	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeTrack, Partial: false})

	// Entry must be removed after the sole scope finalizes.
	if _, ok := c.scopeSubs.Load(qid); ok {
		t.Fatal("scopeSubs entry should be deleted after finalization")
	}
}

// ---------------------------------------------------------------------------
// Entries payload is forwarded correctly
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_EntriesForwarded(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	ch := c.SubscribeScopeResults(qid, []SearchScope{ScopeTrack})

	entries := []MediaEntry{
		{ID: "e1", Title: "Creep", Kind: KindTrack, Source: "spotify"},
		{ID: "e2", Title: "Fake Plastic Trees", Kind: KindTrack, Source: "spotify"},
	}
	c.dispatchScopeResults(ScopeResultsMsg{
		QueryID: qid,
		Scope:   ScopeTrack,
		Entries: entries,
		Partial: false,
	})

	msg := <-ch
	if len(msg.Entries) != 2 {
		t.Fatalf("Entries: got %d, want 2", len(msg.Entries))
	}
	if msg.Entries[0].ID != "e1" {
		t.Errorf("Entries[0].ID: got %q, want %q", msg.Entries[0].ID, "e1")
	}
	if msg.Entries[1].Title != "Fake Plastic Trees" {
		t.Errorf("Entries[1].Title: got %q, want %q", msg.Entries[1].Title, "Fake Plastic Trees")
	}
}

// ---------------------------------------------------------------------------
// Error payload is forwarded correctly
// ---------------------------------------------------------------------------

func TestClient_ScopeResults_ErrorForwarded(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	ch := c.SubscribeScopeResults(qid, []SearchScope{ScopeArtist})

	c.dispatchScopeResults(ScopeResultsMsg{
		QueryID: qid,
		Scope:   ScopeArtist,
		Partial: false,
		Error:   &ScopeError{Type: "all_failed"},
	})

	msg := <-ch
	if msg.Error == nil {
		t.Fatal("Error: got nil, want non-nil ScopeError")
	}
	if msg.Error.Type != "all_failed" {
		t.Errorf("Error.Type: got %q, want %q", msg.Error.Type, "all_failed")
	}
}
