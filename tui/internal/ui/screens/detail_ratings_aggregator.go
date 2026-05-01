package screens

// detail_ratings_aggregator.go — RATINGS section sourced from the
// elfhosted Stremio rating-aggregator addon.
//
// Reads ds.Meta.RatingsAggregator.Description (a pre-formatted multi-line
// emoji block delivered verbatim from the addon) and renders it as a
// section in the description tab. The block already carries its own
// visual frame; we strip the addon's leading / trailing separator lines
// so it sits cleanly under our section header rather than getting
// double-framed.

import (
	"strings"

	"github.com/stui/stui/pkg/theme"
)

func renderRatingsAggregatorSection(ds *DetailState, width int) string {
	_ = width
	title := theme.T.DetailSectionStyle().Render("RATINGS")

	switch ds.Meta.RatingsAggregatorStatus {
	case FetchPending:
		// Mirror the CREW pattern: keep the section anchored while we
		// wait so the layout doesn't jump when the partial lands.
		return title + "\n" + detailDim("  Loading ratings…")
	case FetchEmpty:
		// Addon has no entry for this id — hide the section entirely
		// so we don't add a hollow header below the description.
		return ""
	}

	desc := strings.TrimSpace(ds.Meta.RatingsAggregator.Description)
	if desc == "" {
		return ""
	}

	var lines []string
	for _, ln := range strings.Split(desc, "\n") {
		trimmed := strings.TrimSpace(ln)
		// Drop the addon's own separator rules (`───────────────` etc.) —
		// they're a visual artefact of its single-line Stremio rendering
		// and double-frame against our section header.
		if isRatingsSeparator(trimmed) {
			continue
		}
		if trimmed == "" {
			lines = append(lines, "")
			continue
		}
		lines = append(lines, "  "+trimmed)
	}

	if len(lines) == 0 {
		return ""
	}
	return title + "\n" + strings.Join(lines, "\n")
}

// isRatingsSeparator detects the addon's horizontal-rule decoration —
// any line composed solely of box-drawing horizontal characters.
func isRatingsSeparator(s string) bool {
	if s == "" {
		return false
	}
	for _, r := range s {
		switch r {
		case '─', '-', '━', '═':
			continue
		default:
			return false
		}
	}
	return true
}
