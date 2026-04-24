package screens

// detail_crew.go — CREW section (director / DoP / composer / studio).
//
// Reads ds.Meta.Credits.Crew populated by the "credits" verb of a
// GetDetailMetadata fan-out. Loading / empty / data variants are driven
// off ds.Meta.CreditsStatus.

import (
	"fmt"
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/pkg/theme"
)

// renderCrewSection returns a block showing director, DoP, composer, etc.
// The CREW header is always emitted so empty/loading states stay visually
// anchored in the info panel.
func renderCrewSection(ds *DetailState, width int) string {
	title := theme.T.DetailSectionStyle().Render(detailCrewHeader)

	switch ds.Meta.CreditsStatus {
	case FetchPending:
		return title + "\n" + detailDim(detailLoadingCrew)
	case FetchEmpty:
		// Plugins returned nothing — hide the section entirely rather
		// than showing an empty placeholder. Catalog data (title, year,
		// description) is still visible in the info block above.
		return ""
	}

	crew := ds.Meta.Credits.Crew
	if len(crew) == 0 && ds.Entry.Studio == "" {
		return ""
	}

	var rows []string
	// Promote headline credits first: Director, Cinematographer,
	// AnimationDirector, LeadAnimator, Composer. Everything else gets
	// folded underneath in source order.
	headline := []string{
		"director",
		"cinematographer",
		"animation_director",
		"lead_animator",
		"composer",
	}
	seen := map[int]bool{}
	for _, want := range headline {
		for i, c := range crew {
			if seen[i] {
				continue
			}
			if c.Role == want {
				rows = append(rows, renderCrewRow(c, width))
				seen[i] = true
			}
		}
	}

	// Studio: the plan wants it shown in the meta line AND here.
	if ds.Entry.Studio != "" {
		rows = append(rows, renderCrewRow(ipc.CrewWire{Name: ds.Entry.Studio, Role: "studio"}, width))
	}

	return title + "\n" + strings.Join(rows, "\n")
}

// renderCrewRow returns a single "Role  Name" line, e.g.
// "  Animation Director   Yamamoto Sayo".
func renderCrewRow(c ipc.CrewWire, width int) string {
	_ = width // caller already sized the surrounding block
	label := humanizeRole(c.Role)
	return fmt.Sprintf("  %-20s %s", label, c.Name)
}

// humanizeRole converts the snake_case role wire value (e.g.
// "animation_director") to a Title-Cased display label ("Animation Director").
func humanizeRole(wire string) string {
	if wire == "" {
		return ""
	}
	parts := strings.Split(wire, "_")
	for i, p := range parts {
		if p == "" {
			continue
		}
		parts[i] = strings.ToUpper(p[:1]) + p[1:]
	}
	return strings.Join(parts, " ")
}

// detailDim renders a faint/muted string, used for loading spinners and
// empty-state labels inside the detail overlay.
func detailDim(s string) string {
	return lipgloss.NewStyle().Faint(true).Foreground(theme.T.TextDim()).Render(s)
}
