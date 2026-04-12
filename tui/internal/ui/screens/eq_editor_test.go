package screens_test

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stui/stui/internal/ui/screens"
)

func TestCurveFlat(t *testing.T) {
	// No active bands → curve should be all at 0dB (centre row)
	bands := []screens.EqBand{}
	row := screens.ComputeCurveRow(bands, 44100.0, 60, 10, 0) // col=0, totalCols=60, height=10
	// Centre row index = height/2 - 1 for 0dB
	centre := 10/2 - 1
	if row != centre {
		t.Errorf("flat curve: col 0 row=%d, want %d", row, centre)
	}
}

func TestCurvePeakIsHighest(t *testing.T) {
	// Single +12dB peak at 1kHz; the column near 1kHz should be at or above centre
	bands := []screens.EqBand{{
		Enabled: true, FilterType: screens.EqFilterTypePeak,
		Freq: 1000.0, GainDB: 12.0, Q: 1.0,
	}}
	width := 120
	sampleRate := 44100.0
	// Find column for 1kHz on log scale: col = log(1000/20)/log(20000/20) * width
	var maxRow, maxCol int
	for col := 0; col < width; col++ {
		row := screens.ComputeCurveRow(bands, sampleRate, width, 20, col)
		if row < maxRow || col == 0 {
			maxRow = row
			maxCol = col
		}
	}
	_ = maxCol
	// maxRow should be above centre (smaller row index = higher on screen = more boost).
	// ComputeCurveRow maps 0dB to int((1.0 - (0+20)/40.0) * float64(height-1)) = int(0.5 * 19) = 9.
	centre := (20 - 1) / 2 // = 9, matching ComputeCurveRow's 0dB row for height=20
	if maxRow >= centre {
		t.Errorf("peak curve: maxRow=%d should be < centre=%d", maxRow, centre)
	}
}

func TestEditorView_ContainsBands(t *testing.T) {
	m := screens.NewEqEditorModel(nil, 44100.0)
	m.SetSize(120, 40)
	m.AddBand(screens.EqBand{
		Enabled: true, FilterType: screens.EqFilterTypePeak,
		Freq: 1000.0, GainDB: 3.0, Q: 1.0,
	})
	view := m.View()
	s := view.Content
	if !strings.Contains(s, "Peak") {
		t.Errorf("view should contain 'Peak', got:\n%s", s)
	}
	if !strings.Contains(s, "1000") {
		t.Errorf("view should contain '1000', got:\n%s", s)
	}
}

func TestEditorView_Golden(t *testing.T) {
	// Band config mirrors the spec layout mockup (section "TUI EQ Editor").
	m := screens.NewEqEditorModel(nil, 44100.0)
	m.SetSize(120, 40)
	m.AddBand(screens.EqBand{Enabled: true, FilterType: screens.EqFilterTypePeak, Freq: 1000, GainDB: 3.0, Q: 1.0})
	m.AddBand(screens.EqBand{Enabled: true, FilterType: screens.EqFilterTypeLowShelf, Freq: 80, GainDB: 2.0, Q: 0.71})
	m.AddBand(screens.EqBand{Enabled: false, FilterType: screens.EqFilterTypeLowPass, Freq: 18000, GainDB: 0.0, Q: 0.71})

	view := m.View()
	got := []byte(view.Content)
	goldenFile := filepath.Join("testdata", "eq_editor_golden.txt")
	if os.Getenv("UPDATE_GOLDEN") == "1" {
		_ = os.MkdirAll("testdata", 0755)
		_ = os.WriteFile(goldenFile, got, 0644)
		t.Logf("golden file updated: %s", goldenFile)
		return
	}
	want, err := os.ReadFile(goldenFile)
	if err != nil {
		t.Fatalf("golden file missing — run with UPDATE_GOLDEN=1 to create it: %v", err)
	}
	if !bytes.Equal(got, want) {
		t.Errorf("view does not match golden file.\nRun: UPDATE_GOLDEN=1 go test ./... to regenerate.\nDiff (got vs want):\n%s",
			diffStrings(string(got), string(want)))
	}
}

func TestSettingsHasEqEntry(t *testing.T) {
	// The settings model must contain a DSP Audio category with an EQ entry
	m := screens.NewSettingsModel()
	view := m.View()
	s := view.Content
	if !strings.Contains(s, "EQ") && !strings.Contains(s, "Equalizer") {
		t.Errorf("settings view should contain EQ entry, got:\n%s", s)
	}
}

// diffStrings returns a simple line-by-line diff for test output.
func diffStrings(got, want string) string {
	gotLines := strings.Split(got, "\n")
	wantLines := strings.Split(want, "\n")
	var sb strings.Builder
	for i := 0; i < len(gotLines) || i < len(wantLines); i++ {
		g, w := "", ""
		if i < len(gotLines) {
			g = gotLines[i]
		}
		if i < len(wantLines) {
			w = wantLines[i]
		}
		if g != w {
			sb.WriteString(fmt.Sprintf("line %d\n  got:  %q\n  want: %q\n", i+1, g, w))
		}
	}
	return sb.String()
}
