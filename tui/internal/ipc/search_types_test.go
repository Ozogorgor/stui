package ipc

import (
	"encoding/json"
	"strings"
	"testing"
)

// ---------------------------------------------------------------------------
// SearchReq
// ---------------------------------------------------------------------------

func TestSearchReq_RoundTrip(t *testing.T) {
	req := SearchReq{
		ID:      "q1",
		Query:   "creep",
		Scopes:  []SearchScope{ScopeArtist, ScopeTrack},
		Limit:   50,
		Offset:  0,
		QueryID: 42,
	}
	b, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("json.Marshal: %v", err)
	}
	var back SearchReq
	if err := json.Unmarshal(b, &back); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if back.QueryID != 42 {
		t.Errorf("QueryID: got %d, want 42", back.QueryID)
	}
	if len(back.Scopes) != 2 {
		t.Errorf("Scopes len: got %d, want 2", len(back.Scopes))
	}
	if back.Scopes[0] != ScopeArtist {
		t.Errorf("Scopes[0]: got %q, want %q", back.Scopes[0], ScopeArtist)
	}
	if back.Scopes[1] != ScopeTrack {
		t.Errorf("Scopes[1]: got %q, want %q", back.Scopes[1], ScopeTrack)
	}
	if back.Query != "creep" {
		t.Errorf("Query: got %q, want %q", back.Query, "creep")
	}
}

func TestSearchReq_JSONKeys(t *testing.T) {
	req := SearchReq{
		ID: "x", Query: "q", Scopes: []SearchScope{ScopeAlbum}, Limit: 10, Offset: 5, QueryID: 99,
	}
	b, _ := json.Marshal(req)
	s := string(b)
	for _, key := range []string{`"id"`, `"query"`, `"scopes"`, `"limit"`, `"offset"`, `"query_id"`} {
		if !strings.Contains(s, key) {
			t.Errorf("serialized JSON missing key %s: %s", key, s)
		}
	}
}

// ---------------------------------------------------------------------------
// ScopeResultsMsg + ScopeError (tagged enum)
// ---------------------------------------------------------------------------

func TestScopeResultsMsg_RoundTrip(t *testing.T) {
	msg := ScopeResultsMsg{
		QueryID: 7,
		Scope:   ScopeTrack,
		Entries: []MediaEntry{
			{ID: "e1", Title: "Creep", Kind: KindTrack, Source: "spotify"},
		},
		Partial: true,
	}
	b, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("json.Marshal: %v", err)
	}
	var back ScopeResultsMsg
	if err := json.Unmarshal(b, &back); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if back.QueryID != 7 {
		t.Errorf("QueryID: got %d, want 7", back.QueryID)
	}
	if back.Scope != ScopeTrack {
		t.Errorf("Scope: got %q, want %q", back.Scope, ScopeTrack)
	}
	if !back.Partial {
		t.Error("Partial: got false, want true")
	}
	if len(back.Entries) != 1 || back.Entries[0].Kind != KindTrack {
		t.Errorf("Entries mismatch: %+v", back.Entries)
	}
}

func TestScopeResultsMsg_TaggedError_NoPlugins(t *testing.T) {
	msg := ScopeResultsMsg{
		QueryID: 1,
		Scope:   ScopeArtist,
		Partial: false,
		Error:   &ScopeError{Type: "no_plugins_configured"},
	}
	b, _ := json.Marshal(msg)
	s := string(b)
	if !strings.Contains(s, `"type":"no_plugins_configured"`) {
		t.Fatalf("missing tagged error in JSON: %s", s)
	}
	// Entries key should still be present (null/[]).
	if !strings.Contains(s, `"entries"`) {
		t.Errorf("entries key missing: %s", s)
	}
}

func TestScopeResultsMsg_TaggedError_AllFailed(t *testing.T) {
	msg := ScopeResultsMsg{
		QueryID: 2,
		Scope:   ScopeMovie,
		Error:   &ScopeError{Type: "all_failed"},
	}
	b, _ := json.Marshal(msg)
	if !strings.Contains(string(b), `"type":"all_failed"`) {
		t.Fatalf("missing tagged error in JSON: %s", string(b))
	}
}

func TestScopeResultsMsg_ParsesRustProducedJSON(t *testing.T) {
	input := `{
		"query_id": 7,
		"scope": "track",
		"entries": [],
		"partial": false,
		"error": null
	}`
	var msg ScopeResultsMsg
	if err := json.Unmarshal([]byte(input), &msg); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if msg.Scope != ScopeTrack {
		t.Errorf("Scope: got %q, want %q", msg.Scope, ScopeTrack)
	}
	if msg.QueryID != 7 {
		t.Errorf("QueryID: got %d, want 7", msg.QueryID)
	}
	if msg.Error != nil {
		t.Errorf("Error: got %+v, want nil", msg.Error)
	}
	if msg.Entries == nil {
		t.Error("Entries: got nil, want empty slice")
	}
}

func TestScopeResultsMsg_ParsesRustProducedJSON_WithError(t *testing.T) {
	input := `{
		"query_id": 3,
		"scope": "artist",
		"entries": [],
		"partial": false,
		"error": {"type": "no_plugins_configured"}
	}`
	var msg ScopeResultsMsg
	if err := json.Unmarshal([]byte(input), &msg); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if msg.Error == nil {
		t.Fatal("Error: got nil, want non-nil")
	}
	if msg.Error.Type != "no_plugins_configured" {
		t.Errorf("Error.Type: got %q, want %q", msg.Error.Type, "no_plugins_configured")
	}
}

// ---------------------------------------------------------------------------
// MpdSearchReq
// ---------------------------------------------------------------------------

func TestMpdSearchReq_RoundTrip(t *testing.T) {
	req := MpdSearchReq{
		ID:      "m1",
		Query:   "radiohead",
		Scopes:  []MpdScope{MpdScopeArtist, MpdScopeAlbum, MpdScopeTrack},
		Limit:   200,
		QueryID: 7,
	}
	b, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("json.Marshal: %v", err)
	}
	var back MpdSearchReq
	if err := json.Unmarshal(b, &back); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if back.QueryID != 7 {
		t.Errorf("QueryID: got %d, want 7", back.QueryID)
	}
	if len(back.Scopes) != 3 {
		t.Errorf("Scopes len: got %d, want 3", len(back.Scopes))
	}
	if back.Scopes[0] != MpdScopeArtist || back.Scopes[1] != MpdScopeAlbum || back.Scopes[2] != MpdScopeTrack {
		t.Errorf("Scopes: got %v", back.Scopes)
	}
}

// ---------------------------------------------------------------------------
// MpdSearchResult + MpdSearchErr (tagged enum)
// ---------------------------------------------------------------------------

func TestMpdSearchResult_RoundTrip(t *testing.T) {
	result := MpdSearchResult{
		ID:      "m1",
		QueryID: 7,
		Artists: []MpdArtist{{Name: "Radiohead"}},
		Albums:  []MpdAlbum{{Title: "OK Computer", Artist: "Radiohead", Year: "1997"}},
		Tracks:  []MpdSong{{Title: "Karma Police", Artist: "Radiohead", Album: "OK Computer", Duration: 264.0, File: "radiohead/ok_computer/karma_police.flac"}},
	}
	b, err := json.Marshal(result)
	if err != nil {
		t.Fatalf("json.Marshal: %v", err)
	}
	var back MpdSearchResult
	if err := json.Unmarshal(b, &back); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if back.QueryID != 7 {
		t.Errorf("QueryID: got %d, want 7", back.QueryID)
	}
	if len(back.Artists) != 1 || back.Artists[0].Name != "Radiohead" {
		t.Errorf("Artists mismatch: %+v", back.Artists)
	}
	if len(back.Albums) != 1 || back.Albums[0].Title != "OK Computer" {
		t.Errorf("Albums mismatch: %+v", back.Albums)
	}
	if len(back.Tracks) != 1 || back.Tracks[0].Title != "Karma Police" {
		t.Errorf("Tracks mismatch: %+v", back.Tracks)
	}
}

func TestMpdSearchErr_NotConnected(t *testing.T) {
	result := MpdSearchResult{
		ID:    "m2",
		Error: &MpdSearchErr{Type: "not_connected"},
	}
	b, _ := json.Marshal(result)
	s := string(b)
	if !strings.Contains(s, `"type":"not_connected"`) {
		t.Fatalf("missing tagged error in JSON: %s", s)
	}
}

func TestMpdSearchErr_CommandFailed(t *testing.T) {
	result := MpdSearchResult{
		ID:    "m3",
		Error: &MpdSearchErr{Type: "command_failed", Message: "MPD connection refused"},
	}
	b, _ := json.Marshal(result)
	s := string(b)
	if !strings.Contains(s, `"type":"command_failed"`) {
		t.Fatalf("missing type tag in JSON: %s", s)
	}
	if !strings.Contains(s, `"message":"MPD connection refused"`) {
		t.Fatalf("missing message in JSON: %s", s)
	}
}

func TestMpdSearchErr_MessageOmittedWhenEmpty(t *testing.T) {
	err := MpdSearchErr{Type: "not_connected"}
	b, _ := json.Marshal(err)
	if strings.Contains(string(b), `"message"`) {
		t.Errorf("message key should be omitted when empty: %s", string(b))
	}
}

// ---------------------------------------------------------------------------
// MediaEntry new fields
// ---------------------------------------------------------------------------

func TestMediaEntry_NewFields_RoundTrip(t *testing.T) {
	entry := MediaEntry{
		ID:          "t1",
		Title:       "Karma Police",
		Provider:    "mpd",
		Kind:        KindTrack,
		Source:      "local",
		ArtistName:  "Radiohead",
		AlbumName:   "OK Computer",
		TrackNumber: 3,
	}
	b, err := json.Marshal(entry)
	if err != nil {
		t.Fatalf("json.Marshal: %v", err)
	}
	var back MediaEntry
	if err := json.Unmarshal(b, &back); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if back.Kind != KindTrack {
		t.Errorf("Kind: got %q, want %q", back.Kind, KindTrack)
	}
	if back.Source != "local" {
		t.Errorf("Source: got %q, want %q", back.Source, "local")
	}
	if back.ArtistName != "Radiohead" {
		t.Errorf("ArtistName: got %q, want %q", back.ArtistName, "Radiohead")
	}
	if back.TrackNumber != 3 {
		t.Errorf("TrackNumber: got %d, want 3", back.TrackNumber)
	}
}

func TestMediaEntry_NewFields_OmitEmpty(t *testing.T) {
	// Existing callers that don't set new fields should see no extra keys.
	entry := MediaEntry{ID: "x", Title: "Alien", Provider: "tmdb"}
	b, _ := json.Marshal(entry)
	s := string(b)
	for _, key := range []string{`"kind"`, `"source"`, `"artist_name"`, `"album_name"`, `"track_number"`, `"season"`, `"episode"`} {
		if strings.Contains(s, key) {
			t.Errorf("key %s should be omitted when zero: %s", key, s)
		}
	}
}

// ---------------------------------------------------------------------------
// CatalogEntry new fields
// ---------------------------------------------------------------------------

func TestCatalogEntry_NewFields_RoundTrip(t *testing.T) {
	entry := CatalogEntry{
		ID:       "c1",
		Title:    "Alien",
		Provider: "tmdb",
		Kind:     KindMovie,
		Source:   "tmdb",
	}
	b, err := json.Marshal(entry)
	if err != nil {
		t.Fatalf("json.Marshal: %v", err)
	}
	var back CatalogEntry
	if err := json.Unmarshal(b, &back); err != nil {
		t.Fatalf("json.Unmarshal: %v", err)
	}
	if back.Kind != KindMovie {
		t.Errorf("Kind: got %q, want %q", back.Kind, KindMovie)
	}
	if back.Source != "tmdb" {
		t.Errorf("Source: got %q, want %q", back.Source, "tmdb")
	}
}

func TestCatalogEntry_NewFields_OmitEmpty(t *testing.T) {
	entry := CatalogEntry{ID: "c2", Title: "Blade Runner", Provider: "plex"}
	b, _ := json.Marshal(entry)
	s := string(b)
	for _, key := range []string{`"kind"`, `"source"`} {
		if strings.Contains(s, key) {
			t.Errorf("key %s should be omitted when zero: %s", key, s)
		}
	}
}
