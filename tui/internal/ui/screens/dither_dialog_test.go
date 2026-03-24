package screens

import (
	"os"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
)

func TestDitherDialogView_ContainsFields(t *testing.T) {
	m := NewDitherDialogModel(nil)
	m.SetSize(80, 24)
	v := m.View()
	for _, want := range []string{"Dither", "Bit depth", "Noise"} {
		if !strings.Contains(v.Content, want) {
			t.Errorf("View().Content should contain %q", want)
		}
	}
}

func TestDitherDialogView_Golden(t *testing.T) {
	m := NewDitherDialogModel(nil)
	m.SetSize(80, 24)
	got := m.View().Content

	const golden = "testdata/dither_dialog_golden.txt"
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
		t.Errorf("View output differs from golden.\nGot:\n%s\nWant:\n%s", got, string(data))
	}
}

func TestDitherTabCyclesFields(t *testing.T) {
	m := NewDitherDialogModel(nil)
	m.SetSize(80, 24)
	if m.field != 0 {
		t.Fatalf("expected field 0, got %d", m.field)
	}
	next, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	d := next.(DitherDialogModel)
	if d.field != 1 {
		t.Errorf("tab should advance to field 1, got %d", d.field)
	}
}

func TestDitherNudgeBitDepth(t *testing.T) {
	m := NewDitherDialogModel(nil)
	m.SetSize(80, 24)
	m.field = 2 // bit depth field
	startIdx := m.bitDepthIdx

	next, _ := m.Update(tea.KeyPressMsg{Text: "+", Code: '+'})
	d := next.(DitherDialogModel)
	if d.bitDepthIdx == startIdx {
		// Only fails if we're already at the end of the list
		if startIdx < len(bitDepths)-1 {
			t.Errorf("+ should advance bitDepthIdx")
		}
	}
}

func TestDitherNudgeShaping(t *testing.T) {
	m := NewDitherDialogModel(nil)
	m.SetSize(80, 24)
	m.field = 3 // noise shaping field
	startIdx := m.shapingIdx

	next, _ := m.Update(tea.KeyPressMsg{Text: "+", Code: '+'})
	d := next.(DitherDialogModel)
	expected := (startIdx + 1) % len(shapingNames)
	if d.shapingIdx != expected {
		t.Errorf("+ should advance shapingIdx to %d, got %d", expected, d.shapingIdx)
	}
}

func TestSettingsHasDitherEntry(t *testing.T) {
	cats := defaultCategories()
	var found bool
	for _, cat := range cats {
		for _, item := range cat.items {
			if item.key == "dsp.dither_enabled" {
				found = true
			}
		}
	}
	if !found {
		t.Error("settings should have a dsp.dither_enabled entry")
	}
}
