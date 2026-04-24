package ipc

import (
	"encoding/json"
	"testing"
)

// The wire shape must stay byte-compatible with the Rust runtime's
// ipc::v1::metadata types — these tests exercise the Go side of the
// JSON contract and would break loudly if the Rust side drifts.

func TestDetailMetadataPartial_UnmarshalCredits(t *testing.T) {
	data := []byte(`{"entry_id":"tt1","verb":"credits","payload":{"type":"credits","cast":[],"crew":[]}}`)
	var p DetailMetadataPartial
	if err := json.Unmarshal(data, &p); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if p.EntryID != "tt1" {
		t.Errorf("entry_id = %q, want tt1", p.EntryID)
	}
	if p.Verb != "credits" {
		t.Errorf("verb = %q, want credits", p.Verb)
	}
	if p.Payload.Type != "credits" {
		t.Errorf("payload type = %q, want credits", p.Payload.Type)
	}
}

func TestDetailMetadataPartial_UnmarshalEnrichWithStudio(t *testing.T) {
	data := []byte(`{"entry_id":"tt1","verb":"enrich","payload":{"type":"enrich","studio":"Syncopy","networks":["HBO"]}}`)
	var p DetailMetadataPartial
	if err := json.Unmarshal(data, &p); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if p.Payload.Studio == nil || *p.Payload.Studio != "Syncopy" {
		t.Errorf("studio missing or wrong: %+v", p.Payload.Studio)
	}
	if len(p.Payload.Networks) != 1 || p.Payload.Networks[0] != "HBO" {
		t.Errorf("networks wrong: %v", p.Payload.Networks)
	}
}

func TestDetailMetadataPartial_UnmarshalEmpty(t *testing.T) {
	// Empty variant should round-trip without populating any field.
	data := []byte(`{"entry_id":"tt1","verb":"related","payload":{"type":"empty"}}`)
	var p DetailMetadataPartial
	if err := json.Unmarshal(data, &p); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if p.Payload.Type != "empty" {
		t.Errorf("payload type = %q, want empty", p.Payload.Type)
	}
	if len(p.Payload.Items) != 0 {
		t.Errorf("items should be empty for empty payload: %v", p.Payload.Items)
	}
}

func TestGetDetailMetadataRequest_MarshalShape(t *testing.T) {
	req := GetDetailMetadataRequest{EntryID: "tt1", IDSource: "imdb", Kind: "movies"}
	b, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	// Field names must match Rust's serde(rename_all = "snake_case")-derived
	// wire names so the runtime's deserializer accepts this payload.
	got := string(b)
	want := `{"entry_id":"tt1","id_source":"imdb","kind":"movies"}`
	if got != want {
		t.Errorf("marshal mismatch:\n got: %s\nwant: %s", got, want)
	}
}
