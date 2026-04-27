package components

// scrollbar_test.go — unit tests for the unified Scrollbar function.
//
// These tests pin down the behavior the spec mandates:
//  - Always renders the track even when items fit (all-thumb).
//  - Returns exactly viewH lines with viewH-1 newlines.
//  - Each line is exactly 1 visual cell wide.
//  - Scroll clamping (negative → 0, over-scroll → maxScroll).
//  - Empty / zero / single-cell edge cases.

import (
	"strings"
	"testing"

	"charm.land/lipgloss/v2"
)

// stripANSI returns the visible chars by repeatedly trimming ANSI codes.
// We don't rely on it for width assertions (lipgloss.Width does that);
// we use it to assert the THUMB/TRACK char identity.
func visibleChars(s string) string {
	// lipgloss does not export a strip helper; we strip here by walking
	// the bytes and dropping CSI sequences (ESC '[' ... 'm').
	var out strings.Builder
	for i := 0; i < len(s); i++ {
		if s[i] == 0x1b && i+1 < len(s) && s[i+1] == '[' {
			j := i + 2
			for j < len(s) && s[j] != 'm' {
				j++
			}
			i = j
			continue
		}
		out.WriteByte(s[i])
	}
	return out.String()
}

func TestScrollbar_EverythingFits(t *testing.T) {
	out := Scrollbar(0, 10, 5)
	if got := strings.Count(out, "\n"); got != 9 {
		t.Errorf("newline count = %d, want 9", got)
	}
	vis := visibleChars(out)
	if got := strings.Count(vis, "█"); got != 10 {
		t.Errorf("thumb char count = %d, want 10 (all-thumb when items fit)", got)
	}
	if strings.Contains(vis, "░") {
		t.Errorf("track char appeared when items fit; want all-thumb")
	}
}

func TestScrollbar_EmptyList(t *testing.T) {
	out := Scrollbar(0, 10, 0)
	vis := visibleChars(out)
	if got := strings.Count(vis, "█"); got != 10 {
		t.Errorf("empty list ⇒ all-thumb; got %d █ chars, want 10", got)
	}
}

func TestScrollbar_SingleCell(t *testing.T) {
	out := Scrollbar(0, 1, 100)
	if strings.Contains(out, "\n") {
		t.Errorf("viewH=1 should produce no newlines; got %q", out)
	}
	vis := visibleChars(out)
	if vis != "█" {
		t.Errorf("single-cell at scroll=0 should be thumb; got %q", vis)
	}
}

func TestScrollbar_LargeOverflowScrollZero(t *testing.T) {
	out := Scrollbar(0, 10, 100)
	lines := strings.Split(out, "\n")
	if len(lines) != 10 {
		t.Fatalf("line count = %d, want 10", len(lines))
	}
	// Thumb should start at line 0.
	if visibleChars(lines[0]) != "█" {
		t.Errorf("line 0 = %q, want thumb at scroll=0", visibleChars(lines[0]))
	}
	// And the last line should be track.
	last := visibleChars(lines[len(lines)-1])
	if last != "░" {
		t.Errorf("last line = %q, want track at scroll=0/total=100", last)
	}
}

func TestScrollbar_LargeOverflowScrollEnd(t *testing.T) {
	out := Scrollbar(90, 10, 100)
	lines := strings.Split(out, "\n")
	if visibleChars(lines[len(lines)-1]) != "█" {
		t.Errorf("last line should be thumb at end-scroll; got %q",
			visibleChars(lines[len(lines)-1]))
	}
}

func TestScrollbar_LargeOverflowScrollMid(t *testing.T) {
	out := Scrollbar(45, 10, 100)
	lines := strings.Split(out, "\n")
	// Neither first nor last line is thumb when mid-scroll.
	first := visibleChars(lines[0])
	last := visibleChars(lines[len(lines)-1])
	if first == "█" || last == "█" {
		t.Errorf("mid-scroll: first=%q last=%q; want thumb in middle", first, last)
	}
	// thumbH = viewH*viewH/total = 100/100 = 1 cell.
	if got := strings.Count(visibleChars(out), "█"); got != 1 {
		t.Errorf("thumb cell count = %d, want 1 (thumbH = 10*10/100 = 1)", got)
	}
}

func TestScrollbar_NegativeScrollClamps(t *testing.T) {
	out := Scrollbar(-5, 10, 100)
	want := Scrollbar(0, 10, 100)
	if out != want {
		t.Errorf("negative scroll should clamp to 0; outputs differ")
	}
}

func TestScrollbar_OverScrollClamps(t *testing.T) {
	out := Scrollbar(200, 10, 100)
	want := Scrollbar(90, 10, 100)
	if out != want {
		t.Errorf("over-scroll should clamp to maxScroll; outputs differ")
	}
}

func TestScrollbar_ZeroViewH(t *testing.T) {
	if got := Scrollbar(0, 0, 100); got != "" {
		t.Errorf("viewH=0 ⇒ empty; got %q", got)
	}
	if got := Scrollbar(0, -1, 100); got != "" {
		t.Errorf("viewH=-1 ⇒ empty; got %q", got)
	}
}

func TestScrollbar_LineWidthInvariant(t *testing.T) {
	out := Scrollbar(45, 10, 100)
	for i, line := range strings.Split(out, "\n") {
		if w := lipgloss.Width(line); w != 1 {
			t.Errorf("line %d width = %d, want 1; line=%q", i, w, line)
		}
	}
}
