package screens

// detail_test.go — render-layer tests for the detail overlay.
//
// These exercise the composition points (renderDetailMain, renderInfoBlock,
// renderPosterBlock) to verify that CREW, Related, Studio-in-meta, and the
// backdrop carousel all honour the per-verb DetailMetadata + FetchStatus
// contract from Chunk 6.

import (
	"strings"
	"testing"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
)

// ── Task 7.1: CREW section ────────────────────────────────────────────────────

func TestDetail_CrewSectionShowsDirectorFromCredits(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "X"})
	ds.Meta.Credits = ipc.MetadataPayload{
		Type: "credits",
		Crew: []ipc.CrewWire{{Name: "Nolan", Role: "director"}},
	}
	ds.Meta.CreditsStatus = FetchLoaded
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, "Director") || !strings.Contains(out, "Nolan") {
		t.Errorf("crew not rendered: director/Nolan missing from output")
	}
}

func TestDetail_CrewSectionShowsEmptyLabelWhenEmpty(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "X"})
	ds.Meta.CreditsStatus = FetchEmpty
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, detailEmptyCredits) {
		t.Errorf("empty label %q missing from output", detailEmptyCredits)
	}
}

func TestDetail_CrewSectionShowsLoadingSkeleton(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "X"})
	// defaults to FetchPending
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, detailLoadingCrew) {
		t.Errorf("loading skeleton %q missing from output", detailLoadingCrew)
	}
}

// ── Task 7.2: Related section ─────────────────────────────────────────────────

func TestDetail_RelatedSectionShowsItemsFromRelatedPayload(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "X"})
	ds.Meta.Related = ipc.MetadataPayload{
		Type:  "related",
		Items: []ipc.RelatedItemWire{{ID: "tt2", IDSource: "imdb", Title: "Sequel", Kind: "movie"}},
	}
	ds.Meta.RelatedStatus = FetchLoaded
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, "Sequel") {
		t.Errorf("related not rendered: %q", out)
	}
}

func TestDetail_RelatedSectionShowsEmptyLabel(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.Meta.RelatedStatus = FetchEmpty
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, detailEmptyRelated) {
		t.Errorf("empty label %q missing from output", detailEmptyRelated)
	}
}

func TestDetail_RelatedSectionShowsLoadingSkeleton(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	// defaults to FetchPending
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, detailLoadingRelated) {
		t.Errorf("loading skeleton %q missing from output", detailLoadingRelated)
	}
}

// ── Task 7.3: Studio in meta line ─────────────────────────────────────────────

func TestDetail_InfoLineShowsStudioFromEnrich(t *testing.T) {
	entry := ipc.DetailEntry{ID: "tt1", Title: "X", Year: "2025", Runtime: "120", Genre: "Drama"}
	entry.Studio = "Syncopy"
	ds := NewDetailState(entry)
	out := renderInfoBlock(&ds, 100, 40)
	if !strings.Contains(out, "Syncopy") {
		t.Errorf("studio missing: %q", out)
	}
}

// ── Task 7.3: Backdrop carousel ───────────────────────────────────────────────

func TestDetail_BackdropCarouselShowsIndexWhenMultipleBackdrops(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.Meta.Artwork = ipc.MetadataPayload{
		Type: "artwork",
		Backdrops: []ipc.ArtworkVariantWire{
			{URL: "a.jpg", SizeLabel: "hi_res"},
			{URL: "b.jpg", SizeLabel: "hi_res"},
			{URL: "c.jpg", SizeLabel: "hi_res"},
		},
	}
	ds.Meta.ArtworkStatus = FetchLoaded
	out := renderPosterBlock(&ds, 22, 30)
	if !strings.Contains(out, "1/3") && !strings.Contains(out, "[1/3]") {
		t.Errorf("backdrop indicator missing: %q", out)
	}
}
