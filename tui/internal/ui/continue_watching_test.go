package ui

import (
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/stui/stui/pkg/watchhistory"
)

func TestCwTimeLeft(t *testing.T) {
	cases := []struct {
		pos, dur float64
		want     string
	}{
		{3600, 7200, "1h 00m left"},
		{0, 5400, "1h 30m left"},
		{5100, 5400, "5m left"},
		{5400, 5400, "0m left"},
		{3600, 0, ""},
	}
	for _, tc := range cases {
		got := cwTimeLeft(tc.pos, tc.dur)
		if got != tc.want {
			t.Errorf("cwTimeLeft(%v,%v): want %q, got %q", tc.pos, tc.dur, tc.want, got)
		}
	}
}

func TestCwSubtitle(t *testing.T) {
	cases := []struct {
		entry watchhistory.Entry
		want  string
	}{
		{
			watchhistory.Entry{Tab: "series", Season: 3, Episode: 5, Position: 300, Duration: 3900},
			"S3E5 · 1h 00m left",
		},
		{
			watchhistory.Entry{Tab: "series", Season: 0, Episode: 0, Position: 300, Duration: 3900},
			"Series · 1h 00m left",
		},
		{
			watchhistory.Entry{Tab: "movies", Position: 1800, Duration: 7200},
			"Movie · 1h 30m left",
		},
	}
	for _, tc := range cases {
		got := cwSubtitle(tc.entry)
		if got != tc.want {
			t.Errorf("cwSubtitle(%+v): want %q, got %q", tc.entry, tc.want, got)
		}
	}
}

func TestCwProgressBarLength(t *testing.T) {
	bar := cwProgressBar(0.5, 1.0, 10)
	filled := strings.Count(bar, "█")
	empty := strings.Count(bar, "░")
	if filled+empty != 10 {
		t.Errorf("expected 10 bar chars, got filled=%d empty=%d bar=%q", filled, empty, bar)
	}
	if filled != 5 {
		t.Errorf("expected 5 filled chars at 50%%, got %d", filled)
	}
}

func TestCwProgressBarFullEmpty(t *testing.T) {
	full := cwProgressBar(1.0, 1.0, 8)
	if strings.Count(full, "░") != 0 {
		t.Errorf("100%% bar should have no empty chars")
	}
	empty := cwProgressBar(0, 1.0, 8)
	if strings.Count(empty, "█") != 0 {
		t.Errorf("0%% bar should have no filled chars")
	}
}

func TestCwItems(t *testing.T) {
	store := watchhistory.Load(filepath.Join(t.TempDir(), "test-history-cw.json"))
	store.Upsert(watchhistory.Entry{ID: "m1", Tab: "movies", Position: 10, Duration: 100})
	time.Sleep(time.Second)
	store.Upsert(watchhistory.Entry{ID: "m2", Tab: "movies", Position: 20, Duration: 100})
	store.Upsert(watchhistory.Entry{ID: "s1", Tab: "series", Position: 30, Duration: 100})
	store.Upsert(watchhistory.Entry{ID: "m3", Tab: "movies", Position: 0, Duration: 100})

	got := cwItems(store, "movies")
	if len(got) != 2 {
		t.Fatalf("expected 2 movie items, got %d", len(got))
	}
	if got[0].ID != "m2" {
		t.Errorf("expected m2 first (most recently upserted), got %s", got[0].ID)
	}
	if got[1].ID != "m1" {
		t.Errorf("expected m1 second (earlier upsert), got %s", got[1].ID)
	}

	gotSeries := cwItems(store, "series")
	if len(gotSeries) != 1 || gotSeries[0].ID != "s1" {
		t.Errorf("expected 1 series item, got %v", gotSeries)
	}
}

func TestCwItemsCappedAt5(t *testing.T) {
	store := watchhistory.Load(filepath.Join(t.TempDir(), "test-history-cw-cap.json"))
	for i := 0; i < 7; i++ {
		store.Upsert(watchhistory.Entry{
			ID:          fmt.Sprintf("m%d", i),
			Tab:         "movies",
			Position:    10,
			Duration:    100,
			LastWatched: int64(i),
		})
	}
	got := cwItems(store, "movies")
	if len(got) != 5 {
		t.Errorf("expected cap of 5, got %d", len(got))
	}
}

func TestHistoryEntryToCatalogEntry(t *testing.T) {
	e := watchhistory.Entry{
		ID:       "tt1234",
		Title:    "Breaking Bad",
		Year:     "2008",
		Provider: "torrentio",
		ImdbID:   "tt0903747",
		Tab:      "series",
	}
	cat := historyEntryToCatalogEntry(e)
	if cat.ID != "tt1234" {
		t.Errorf("ID mismatch")
	}
	if cat.Title != "Breaking Bad" {
		t.Errorf("Title mismatch")
	}
	if cat.Year == nil || *cat.Year != "2008" {
		t.Errorf("Year mismatch: got %v", cat.Year)
	}
	if cat.ImdbID == nil || *cat.ImdbID != "tt0903747" {
		t.Errorf("ImdbID mismatch: got %v", cat.ImdbID)
	}
	if cat.Provider != "torrentio" {
		t.Errorf("Provider mismatch")
	}
}
