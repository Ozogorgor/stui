package screens

import (
	"os"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
)

func TestCrossfeedDialogView_ContainsFields(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	v := m.View()
	for _, want := range []string{"Crossfeed", "Feed", "Cutoff"} {
		if !strings.Contains(v.Content, want) {
			t.Errorf("View().Content should contain %q", want)
		}
	}
}

func TestCrossfeedDialogView_Golden(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	got := m.View().Content

	const golden = "testdata/crossfeed_dialog_golden.txt"
	if os.Getenv("UPDATE_GOLDEN") == "1" {
		if err := os.MkdirAll("testdata", 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(golden, []byte(got), 0o644); err != nil {
			t.Fatal(err)
		}
		t.Logf("golden file updated: %s", golden)
		return
	}

	data, err := os.ReadFile(golden)
	if err != nil {
		t.Fatalf("golden file missing — run with UPDATE_GOLDEN=1 to create it: %v", err)
	}
	if string(data) != got {
		t.Errorf("View output differs from golden file.\nGot:\n%s\nWant:\n%s", got, string(data))
	}
}

func TestCrossfeedTabCyclesFields(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	if m.field != 0 {
		t.Fatalf("expected field 0, got %d", m.field)
	}
	next, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	d := next.(CrossfeedDialogModel)
	if d.field != 1 {
		t.Errorf("tab should advance to field 1, got %d", d.field)
	}
}

func TestCrossfeedPresetCycleViaP(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	if m.presetIdx != 0 {
		t.Fatalf("expected presetIdx 0, got %d", m.presetIdx)
	}
	next, _ := m.Update(tea.KeyPressMsg{Text: "p", Code: 'p'})
	d := next.(CrossfeedDialogModel)
	if d.presetIdx != 1 {
		t.Errorf("p should advance to presetIdx 1, got %d", d.presetIdx)
	}
}

func TestSettingsHasCrossfeedEntry(t *testing.T) {
	cats := defaultCategories()
	var found bool
	for _, cat := range cats {
		for _, item := range cat.items {
			if item.key == "dsp.crossfeed_enabled" {
				found = true
			}
		}
	}
	if !found {
		t.Error("settings should have a dsp.crossfeed_enabled entry")
	}
}
