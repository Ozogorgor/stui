package screens

import (
	"testing"

	"github.com/stui/stui/internal/ipc"
)

func TestStreamBadge(t *testing.T) {
	cases := []struct {
		in   ipc.StreamInfo
		want string
	}{
		{ipc.StreamInfo{Quality: "1080p", Score: 87}, "1080p ★ 87"},
		{ipc.StreamInfo{Quality: "4K", Score: 100}, "4K ★ 100"},
		{ipc.StreamInfo{Quality: "", Score: 50}, "★ 50"},
	}
	for _, tc := range cases {
		got := streamBadge(tc.in)
		if got != tc.want {
			t.Errorf("streamBadge(%+v) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestBestStreamForTierExactMatch(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "1080p", Score: 80},
		{Quality: "720p", Score: 90},
	}
	got := BestStreamForTier(streams, 5) // rank 5 = 1080p
	if got == nil {
		t.Fatal("expected a match, got nil")
	}
	if got.Quality != "1080p" {
		t.Errorf("expected 1080p, got %s", got.Quality)
	}
}

func TestBestStreamForTierPicksHighestScore(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "1080p", Score: 60},
		{Quality: "1080p", Score: 90},
		{Quality: "1080p", Score: 75},
	}
	got := BestStreamForTier(streams, 5)
	if got == nil {
		t.Fatal("expected a match, got nil")
	}
	if got.Score != 90 {
		t.Errorf("expected score 90, got %d", got.Score)
	}
}

func TestBestStreamForTierNoMatch(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "720p", Score: 90},
		{Quality: "480p", Score: 70},
	}
	got := BestStreamForTier(streams, 5) // rank 5 = 1080p — not present
	if got != nil {
		t.Errorf("expected nil, got %+v", *got)
	}
}

func TestBestStreamForTierEmptyList(t *testing.T) {
	got := BestStreamForTier(nil, 5)
	if got != nil {
		t.Errorf("expected nil for empty list, got %+v", *got)
	}
}

func TestBestStreamForTierEmptyQualityNotMatched(t *testing.T) {
	streams := []ipc.StreamInfo{
		{Quality: "", Score: 999},
		{Quality: "1080p", Score: 50},
	}
	got := BestStreamForTier(streams, 5)
	if got == nil {
		t.Fatal("expected a match, got nil")
	}
	if got.Quality != "1080p" {
		t.Errorf("expected 1080p, got %q", got.Quality)
	}
}

func TestBestStreamForTierHasPrefixSemantics(t *testing.T) {
	// "1080p HDR" has prefix "1080p" → qualityScore returns 5.
	streams := []ipc.StreamInfo{
		{Quality: "1080p HDR", Score: 85},
	}
	got := BestStreamForTier(streams, 5)
	if got == nil {
		t.Fatal("expected a match for '1080p HDR' at rank 5, got nil")
	}
}
