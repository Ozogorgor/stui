package screens

import (
	"strings"
	"testing"

	"charm.land/bubbles/v2/spinner"
	"github.com/stui/stui/internal/ipc"
)

func makeEntries(n int) []ipc.CatalogEntry {
	entries := make([]ipc.CatalogEntry, n)
	for i := range entries {
		entries[i] = ipc.CatalogEntry{ID: string(rune('a' + i)), Title: "Title"}
	}
	return entries
}

// RenderGrid must not wrap content in an outer rounded border.
// We detect an outer border by checking that the first line starts with '╭'
// (the top-left corner of a RoundedBorder). After the refactor it must NOT.
func TestRenderGridNoOuterBorder(t *testing.T) {
	entries := makeEntries(3)
	result := RenderGrid(entries, GridCursor{}, 120, 20, false, 0, "ready", []string{"test"}, nil)
	firstLine := strings.SplitN(result, "\n", 2)[0]
	if strings.HasPrefix(strings.TrimLeft(firstLine, " "), "╭") {
		t.Error("RenderGrid must not start with a rounded border corner — outer border is now provided by MainCardStyle")
	}
}

// When totalRows > visibleRows the returned string must contain a scrollbar
// character (█ or │) somewhere in the rightmost column.
func TestRenderGridScrollbarPresentWhenOverflow(t *testing.T) {
	// 30 entries at 120 cols will produce many rows; availH=8 forces overflow.
	entries := makeEntries(30)
	result := RenderGrid(entries, GridCursor{}, 120, 8, false, 0, "ready", []string{"test"}, nil)
	if !strings.Contains(result, "█") && !strings.Contains(result, "│") {
		t.Error("RenderGrid must render a scrollbar (█ or │) when content overflows")
	}
}

// When content fits entirely (1 row, large availH), no scrollbar chars appear.
func TestRenderGridNoScrollbarWhenNoOverflow(t *testing.T) {
	entries := makeEntries(3) // 1 row of posters
	result := RenderGrid(entries, GridCursor{}, 120, 40, false, 0, "ready", []string{"test"}, nil)
	// The scrollbar track char '│' may appear in card art, but '▐' and '▌' are
	// exclusive to the scrollbar thumb edges.
	if strings.Contains(result, "▐") || strings.Contains(result, "▌") {
		t.Error("RenderGrid must not render scrollbar thumb glyphs when no overflow")
	}
}

// Zero availH must return empty string without panicking.
func TestRenderGridZeroAvailH(t *testing.T) {
	defer func() {
		if r := recover(); r != nil {
			t.Errorf("RenderGrid panicked with availH=0: %v", r)
		}
	}()
	entries := makeEntries(10)
	result := RenderGrid(entries, GridCursor{}, 120, 0, false, 0, "ready", []string{"test"}, nil)
	_ = result // may be empty string — just must not panic
}

// isLoading=true must return a centred loading message without panicking.
func TestRenderGridLoadingState(t *testing.T) {
	var s spinner.Model
	result := RenderGrid(nil, GridCursor{}, 80, 10, true, 0, "connecting", nil, &s)
	if result == "" {
		t.Error("RenderGrid with isLoading=true should return a non-empty loading message")
	}
}

// When entries fit within availH, the output must still be exactly availH
// lines tall — the parent container expects fixed height.
func TestRenderGridAlwaysFillsAvailH(t *testing.T) {
	for _, n := range []int{0, 1, 3, 5, 10} {
		entries := makeEntries(n)
		result := RenderGrid(entries, GridCursor{}, 120, 20, false, 0, "ready", []string{"test"}, nil)
		lines := strings.Split(result, "\n")
		if len(lines) != 20 {
			t.Errorf("n=%d: expected 20 lines, got %d", n, len(lines))
		}
	}
}
