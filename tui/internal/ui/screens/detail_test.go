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

func TestDetail_CrewSectionHiddenWhenEmpty(t *testing.T) {
	// Plugins returned nothing for credits — the CREW section is
	// hidden entirely rather than showing an empty placeholder. The
	// catalog title still renders so the card isn't blank.
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "The Matrix"})
	ds.Meta.CreditsStatus = FetchEmpty
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if strings.Contains(out, detailCrewHeader) {
		t.Errorf("CREW header rendered when empty: %q", out)
	}
	if strings.Contains(out, detailEmptyCredits) {
		t.Errorf("empty-credits placeholder still rendered: %q", out)
	}
	if !strings.Contains(out, "The Matrix") {
		t.Errorf("catalog title missing: %q", out)
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

func TestDetail_RelatedSectionHiddenWhenEmpty(t *testing.T) {
	// Plugins returned no related items — the row is hidden entirely
	// to reclaim vertical space rather than showing an empty placeholder.
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.Meta.RelatedStatus = FetchEmpty
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if strings.Contains(out, detailRelatedHeader) {
		t.Errorf("RELATED header rendered when empty: %q", out)
	}
	if strings.Contains(out, detailEmptyRelated) {
		t.Errorf("empty-related placeholder still rendered: %q", out)
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

// ── Task 8.1: Progressive-render snapshot tests ───────────────────────────────

// All four per-verb fetches start FetchPending; all four skeleton labels
// (crew, artwork, related) must be present. The enrich verb has no visible
// skeleton of its own — its loading state is reflected in the description
// block's existing "Loading details…" row, not tested here.
func TestDetail_AllFourLoading_ShowsSkeletons(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "X"})
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	for _, want := range []string{detailLoadingCrew, detailLoadingArtwork, detailLoadingRelated} {
		if !strings.Contains(out, want) {
			t.Errorf("missing skeleton %q in output", want)
		}
	}
}

// Credits land first; artwork + related still pending. The crew row
// should render with "Nolan" while the other sections stay skeletons.
func TestDetail_CreditsFirst_OtherSectionsStillLoading(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "X"})
	ds.Meta.Credits = ipc.MetadataPayload{
		Type: "credits",
		Crew: []ipc.CrewWire{{Name: "Nolan", Role: "director"}},
	}
	ds.Meta.CreditsStatus = FetchLoaded
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, "Nolan") {
		t.Error("credits not rendered")
	}
	if !strings.Contains(out, detailLoadingArtwork) {
		t.Error("artwork skeleton missing")
	}
	if !strings.Contains(out, detailLoadingRelated) {
		t.Error("related skeleton missing")
	}
}

// All four verbs resolved empty — the metadata-dependent sections hide
// entirely so the card isn't cluttered with empty placeholders. The
// catalog-derived info (title, year, description, stream-via) stays
// visible because it was never dependent on plugin output.
func TestDetail_AllEmpty_HidesSectionsKeepsCatalogData(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "The Matrix"})
	ds.Meta.EnrichStatus = FetchEmpty
	ds.Meta.CreditsStatus = FetchEmpty
	ds.Meta.ArtworkStatus = FetchEmpty
	ds.Meta.RelatedStatus = FetchEmpty
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, "The Matrix") {
		t.Errorf("catalog title missing when metadata empty: %q", out)
	}
	if strings.Contains(out, detailCrewHeader) {
		t.Errorf("CREW header still rendered when empty: %q", out)
	}
	if strings.Contains(out, detailRelatedHeader) {
		t.Errorf("RELATED header still rendered when empty: %q", out)
	}
	for _, label := range []string{detailEmptyCredits, detailEmptyRelated, detailEmptyArtwork} {
		if strings.Contains(out, label) {
			t.Errorf("empty placeholder %q should be hidden", label)
		}
	}
}

// ── Task 8.2: Forward-compat unknown crew role ────────────────────────────────

// CrewRole::Other(String) arrives on the Go side as a free-form lowercase
// role string. renderCrewSection only promotes a known set of "headline"
// roles (director, cinematographer, …) — anything else is currently
// filtered out. This test locks in graceful forward-compat: an unknown
// role name must not crash the render, and the presence of a crew row
// (even if empty) should keep the CREW section rendered.
//
// The stricter "must display Makoto" assertion would require a
// secondary 'Other Crew' block; that's >20 LoC and the plan explicitly
// permits Option B (assert renders gracefully) in that case. See the
// plan note under Task 8.2.
func TestDetail_UnknownCrewRoleRendersWithRoleString(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1"})
	ds.Meta.Credits = ipc.MetadataPayload{
		Type: "credits",
		Crew: []ipc.CrewWire{{
			Name: "Makoto",
			Role: "color_designer",
		}},
	}
	ds.Meta.CreditsStatus = FetchLoaded

	// Must not panic.
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)

	// The CREW header still renders — empty/unknown roles don't hide it.
	if !strings.Contains(out, detailCrewHeader) {
		t.Errorf("CREW header missing for unknown-role crew: %q", out)
	}

	// No "empty credits" fallback — the crew list is non-empty even if
	// no headline roles matched. (renderCrewSection guards on
	// len(crew) == 0 && ds.Entry.Studio == "", which is false here.)
	if strings.Contains(out, detailEmptyCredits) {
		t.Error("empty-credits fallback fired despite non-empty crew")
	}
}

// One verb empty, the others loaded — the all-empty fallback must NOT
// fire; the single-empty label replaces only its own section.
func TestDetail_OneEmpty_OthersLoaded(t *testing.T) {
	ds := NewDetailState(ipc.DetailEntry{ID: "tt1", Title: "X"})
	ds.Meta.CreditsStatus = FetchLoaded
	ds.Meta.Credits = ipc.MetadataPayload{
		Type: "credits",
		Crew: []ipc.CrewWire{{Name: "Nolan", Role: "director"}},
	}
	ds.Meta.ArtworkStatus = FetchEmpty // artwork strip is hidden entirely
	ds.Meta.RelatedStatus = FetchLoaded
	ds.Meta.Related = ipc.MetadataPayload{
		Type:  "related",
		Items: []ipc.RelatedItemWire{{Title: "Sequel"}},
	}
	out := renderDetailMain(&ds, 100, 40, state.TabMovies)
	if !strings.Contains(out, "Nolan") {
		t.Error("credits missing")
	}
	if strings.Contains(out, detailEmptyArtwork) {
		t.Error("empty-artwork placeholder should be hidden, not shown")
	}
	if !strings.Contains(out, "Sequel") {
		t.Error("related missing")
	}
}
