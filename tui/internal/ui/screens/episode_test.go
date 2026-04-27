package screens

// episode_test.go — unit tests for computeEpisodeViewport.
//
// The helper is the shared cursor → viewport math used by both list
// view (extraReserve=0) and grid view (extraReserve=2 for the info
// strap). These tests don't spin up Bubbletea — pure function in,
// pure tuple out.

import "testing"

func TestComputeEpisodeViewport_ListLargeMidCursor(t *testing.T) {
	// 1100 episodes, cursor halfway, normal-sized terminal.
	start, end, panelH := computeEpisodeViewport(1100, 500, 30, 0)
	if panelH <= 0 {
		t.Fatalf("panelH = %d, want > 0", panelH)
	}
	if 500 < start || 500 >= end {
		t.Errorf("cursor 500 not in [%d, %d)", start, end)
	}
	if end-start != panelH {
		t.Errorf("window size %d, want panelH %d", end-start, panelH)
	}
}

func TestComputeEpisodeViewport_ListFits(t *testing.T) {
	// 12 episodes in a 40-row terminal — chrome (20 rows) leaves 20
	// rows of panel, comfortably more than 12.
	start, end, panelH := computeEpisodeViewport(12, 0, 40, 0)
	if start != 0 || end != 12 {
		t.Errorf("got [%d, %d), want [0, 12)", start, end)
	}
	if panelH <= 12 {
		t.Errorf("panelH = %d, want > 12 (must accommodate full list)", panelH)
	}
}

func TestComputeEpisodeViewport_Empty(t *testing.T) {
	start, end, _ := computeEpisodeViewport(0, 0, 30, 0)
	if start != 0 || end != 0 {
		t.Errorf("got [%d, %d), want [0, 0)", start, end)
	}
}

func TestComputeEpisodeViewport_ListCursorAtEnd(t *testing.T) {
	start, end, _ := computeEpisodeViewport(12, 11, 30, 0)
	if 11 < start || 11 >= end {
		t.Errorf("cursor 11 not in [%d, %d)", start, end)
	}
}

func TestComputeEpisodeViewport_LargeCursorAtZero(t *testing.T) {
	start, _, _ := computeEpisodeViewport(1100, 0, 30, 0)
	if start != 0 {
		t.Errorf("cursor=0 ⇒ start = %d, want 0", start)
	}
}

func TestComputeEpisodeViewport_LargeCursorAtLast(t *testing.T) {
	start, end, _ := computeEpisodeViewport(1100, 1099, 30, 0)
	if 1099 < start || 1099 >= end {
		t.Errorf("cursor 1099 not in [%d, %d)", start, end)
	}
	if end != 1100 {
		t.Errorf("end = %d, want 1100", end)
	}
}

func TestComputeEpisodeViewport_GridReservesInfoStrap(t *testing.T) {
	// Same screen size, but grid reserves 2 rows for the info strap.
	_, _, listH := computeEpisodeViewport(110, 50, 30, 0)
	_, _, gridH := computeEpisodeViewport(110, 50, 30, 2)
	if gridH != listH-2 {
		t.Errorf("gridH = %d, listH = %d; want gridH == listH-2", gridH, listH)
	}
}

func TestComputeEpisodeViewport_TinyScreen(t *testing.T) {
	// screenH=4 → after chrome carve-out, panelH should clamp to >= 1.
	start, end, panelH := computeEpisodeViewport(12, 0, 4, 0)
	if panelH < 1 {
		t.Errorf("panelH = %d, want >= 1 (helper must clamp)", panelH)
	}
	if start < 0 || end < start {
		t.Errorf("invalid range [%d, %d)", start, end)
	}
}
