package ipc

import (
	"encoding/json"
	"strings"
	"testing"
	"time"
)

// ---------------------------------------------------------------------------
// Client.Search — subscription wiring
//
// Client.Search requires a live transport (stdin pipe) to send the request.
// Rather than spinning up a subprocess, we test the subscription-side
// plumbing directly via SubscribeScopeResults + dispatchScopeResults —
// the same primitives Search uses internally.  The wire-format correctness
// of SearchReq is already covered by TestSearchReq_* in search_types_test.go.
// ---------------------------------------------------------------------------

// TestSearch_SubscriptionReceivesResults verifies that a subscription
// registered for a given qid receives scope results dispatched for that id
// and that the channel is closed once all expected scopes finalize.
func TestSearch_SubscriptionReceivesResults(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()

	scopes := []SearchScope{ScopeArtist, ScopeTrack}
	ch := c.SubscribeScopeResults(qid, scopes)

	// Simulate runtime pushing two final scope_results frames (what the
	// readLoop + dispatchUnsolicited would do in production).
	c.dispatchScopeResults(ScopeResultsMsg{
		QueryID: qid, Scope: ScopeArtist,
		Entries: []MediaEntry{{ID: "a1", Title: "Radiohead", Kind: KindArtist}},
		Partial: false,
	})
	c.dispatchScopeResults(ScopeResultsMsg{
		QueryID: qid, Scope: ScopeTrack,
		Entries: []MediaEntry{{ID: "t1", Title: "Creep", Kind: KindTrack}},
		Partial: false,
	})

	got := make([]ScopeResultsMsg, 0, 2)
	timeout := time.After(1 * time.Second)
	for len(got) < 2 {
		select {
		case msg, open := <-ch:
			if !open {
				break
			}
			got = append(got, msg)
		case <-timeout:
			t.Fatalf("timed out waiting for scope results; got %d/2", len(got))
		}
		if len(got) == 2 {
			break
		}
	}
	if len(got) != 2 {
		t.Fatalf("expected 2 scope results; got %d", len(got))
	}

	// Channel must be closed after both final messages (GC).
	select {
	case _, open := <-ch:
		if open {
			t.Fatal("channel should be closed after all scopes finalized")
		}
	case <-time.After(200 * time.Millisecond):
		t.Fatal("timed out waiting for channel close")
	}
}

// TestSearch_NoLeakOnUnknownQID verifies that a stale scope_results frame
// (for a qid with no subscriber) does not panic or block.
func TestSearch_NoLeakOnUnknownQID(t *testing.T) {
	c := newClientForTest()
	// No subscription registered — must return cleanly.
	c.dispatchScopeResults(ScopeResultsMsg{QueryID: 99999, Scope: ScopeMovie, Partial: false})
}

// TestSearch_QueryIDStrictlyMonotonic verifies that each search call would get
// a distinct, increasing qid. We call NextQueryID directly (the same call
// Client.Search makes internally) to avoid needing a live transport.
func TestSearch_QueryIDStrictlyMonotonic(t *testing.T) {
	c := newClientForTest()
	ids := make([]uint64, 5)
	for i := range ids {
		ids[i] = c.NextQueryID()
	}
	for i := 1; i < len(ids); i++ {
		if ids[i] <= ids[i-1] {
			t.Fatalf("query ids not strictly monotonic: %v", ids)
		}
	}
}

// TestSearch_ScopeSubsCleanedUpAfterAllFinal verifies that scopeSubs is GC'd
// once every registered scope emits partial=false.
func TestSearch_ScopeSubsCleanedUpAfterAllFinal(t *testing.T) {
	c := newClientForTest()
	qid := c.NextQueryID()
	_ = c.SubscribeScopeResults(qid, []SearchScope{ScopeAlbum, ScopeArtist})

	if _, ok := c.scopeSubs.Load(qid); !ok {
		t.Fatal("scopeSubs entry must exist immediately after SubscribeScopeResults")
	}

	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeAlbum, Partial: false})
	// Still one scope remaining.
	if _, ok := c.scopeSubs.Load(qid); !ok {
		t.Fatal("scopeSubs entry must still exist until all scopes finalize")
	}

	c.dispatchScopeResults(ScopeResultsMsg{QueryID: qid, Scope: ScopeArtist, Partial: false})
	if _, ok := c.scopeSubs.Load(qid); ok {
		t.Fatal("scopeSubs entry must be removed after all scopes finalize")
	}
}

// ---------------------------------------------------------------------------
// Client.MpdSearch — wire-format round-trip
//
// MpdSearch also requires a live transport. We validate the wire format here
// (what the method would serialize and what it expects back) without calling
// the method itself, since the method signature is tested by compilation and
// the field contract is covered by search_types_test.go.
// ---------------------------------------------------------------------------

// TestMpdSearch_RequestWireFormat verifies the JSON shape that MpdSearch
// constructs internally matches the Rust-side ipc::v1::MpdSearchRequest.
func TestMpdSearch_RequestWireFormat(t *testing.T) {
	req := MpdSearchReq{
		ID:      "req-42",
		Query:   "radiohead",
		Scopes:  []MpdScope{MpdScopeArtist, MpdScopeAlbum, MpdScopeTrack},
		Limit:   200,
		QueryID: 42,
	}
	b, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("json.Marshal MpdSearchReq: %v", err)
	}
	s := string(b)
	for _, key := range []string{`"id"`, `"query"`, `"scopes"`, `"limit"`, `"query_id"`} {
		if !strings.Contains(s, key) {
			t.Errorf("MpdSearchReq JSON missing key %s: %s", key, s)
		}
	}
	// Verify scope values are correct wire strings.
	for _, scope := range []string{`"artist"`, `"album"`, `"track"`} {
		if !strings.Contains(s, scope) {
			t.Errorf("MpdSearchReq JSON missing scope %s: %s", scope, s)
		}
	}
}

// TestMpdSearch_ResponseDecoding verifies that an MpdSearchResult decoded from
// the wire format (as the runtime returns) arrives intact.
func TestMpdSearch_ResponseDecoding(t *testing.T) {
	// Simulate what the Rust runtime returns.
	wire := `{
		"id": "req-1",
		"query_id": 1,
		"artists": [{"name": "Radiohead"}],
		"albums": [{"title": "OK Computer", "artist": "Radiohead", "year": "1997"}],
		"tracks": [{"title": "Karma Police", "artist": "Radiohead", "album": "OK Computer", "duration": 264.0, "file": "rh/ok/karma.flac"}],
		"error": null
	}`
	var result MpdSearchResult
	if err := json.Unmarshal([]byte(wire), &result); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if result.QueryID != 1 {
		t.Errorf("QueryID: got %d, want 1", result.QueryID)
	}
	if len(result.Artists) != 1 || result.Artists[0].Name != "Radiohead" {
		t.Errorf("Artists: %+v", result.Artists)
	}
	if len(result.Albums) != 1 || result.Albums[0].Title != "OK Computer" {
		t.Errorf("Albums: %+v", result.Albums)
	}
	if len(result.Tracks) != 1 || result.Tracks[0].Title != "Karma Police" {
		t.Errorf("Tracks: %+v", result.Tracks)
	}
	if result.Error != nil {
		t.Errorf("Error: got %+v, want nil", result.Error)
	}
}

// TestMpdSearch_ErrorResponse verifies that an MpdSearchErr payload decodes
// correctly (not_connected case).
func TestMpdSearch_ErrorResponse(t *testing.T) {
	wire := `{
		"id": "req-2",
		"query_id": 2,
		"artists": [],
		"albums": [],
		"tracks": [],
		"error": {"type": "not_connected"}
	}`
	var result MpdSearchResult
	if err := json.Unmarshal([]byte(wire), &result); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if result.Error == nil {
		t.Fatal("Error: got nil, want non-nil")
	}
	if result.Error.Type != "not_connected" {
		t.Errorf("Error.Type: got %q, want %q", result.Error.Type, "not_connected")
	}
}
