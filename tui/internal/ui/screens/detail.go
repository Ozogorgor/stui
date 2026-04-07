package screens

// detail.go — full-screen detail overlay renderer.
//
// Layout:
//
//  ┌──────────────────────────────────────────────────────────────────┐
//  │ ← esc  Movies  ›  Dune: Part Two                  [status pill] │  ← header
//  ├────────────┬─────────────────────────────────────────────────────┤
//  │            │  DUNE: PART TWO                            ★ 8.8   │
//  │  [POSTER]  │  2024  ·  Sci-Fi  ·  2h 46m                        │
//  │            │                                                     │
//  │            │  Denis Villeneuve continues the epic saga of Paul   │
//  │            │  Atreides as he unites with the Fremen people...    │
//  │            │                                                     │
//  │            │  CAST & CREW                                        │
//  │            │  ▸ Timothée Chalamet     Paul Atreides   → search  │
//  │            │  ▸ Zendaya               Chani           → search  │
//  │            │  ▸ Denis Villeneuve      Director        → search  │
//  │            │                                                     │
//  │            │  STREAM VIA                                         │
//  │            │  [tmdb]  [omdb]  [hello-provider]                  │
//  ├────────────┴─────────────────────────────────────────────────────┤
//  │  SIMILAR TITLES                                                  │
//  │  [card]  [card]  [card]  [card]  [card]  →                      │
//  └──────────────────────────────────────────────────────────────────┘
//
// In person mode (after following a cast link):
//
//  ┌──────────────────────────────────────────────────────────────────┐
//  │ ← esc  Movies  ›  Dune: Part Two  ›  Timothée Chalamet          │
//  ├──────────────────────────────────────────────────────────────────┤
//  │  Titles featuring  Timothée Chalamet                            │
//  │  [card] [card] [card] [card] [card]                             │
//  │  [card] [card] [card] ...                                       │
//  └──────────────────────────────────────────────────────────────────┘

import (
	"fmt"
	"image/color"
	"math"
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/bidi"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

const (
	detailPosterWidth  = 22 // chars
	detailPosterHeight = 14 // rows
	similarCardCols    = 6  // cards in the similar row
	similarRowHeight   = 8  // rows for similar section
	detailHeaderHeight = 3  // top bar + border
	detailStatusHeight = 2  // status bar
)

// RenderDetailOverlay renders the full-screen detail view.
func RenderDetailOverlay(
	ds *DetailState,
	termWidth, termHeight int,
	tab state.Tab,
	runtimeStatus string,
) string {
	if ds == nil {
		return ""
	}

	// In person mode, show a grid of that person's works
	if ds.PersonMode {
		return renderPersonMode(ds, termWidth, termHeight, tab)
	}

	return renderDetailMain(ds, termWidth, termHeight, tab)
}

// ── Main detail view ─────────────────────────────────────────────────────────

func renderDetailMain(ds *DetailState, w, h int, tab state.Tab) string {
	header := renderDetailHeader(ds, w, tab)
	similarH := similarRowHeight + 2

	// Split: poster|info section, then similar row at bottom
	mainH := h - lipgloss.Height(header) - similarH

	left := renderPosterBlock(ds, detailPosterWidth, mainH)
	right := renderInfoBlock(ds, w-detailPosterWidth-4, mainH)

	main := lipgloss.JoinHorizontal(lipgloss.Top,
		left,
		lipgloss.NewStyle().
			Width(w-detailPosterWidth-4).
			Height(mainH).
			Render(right),
	)

	similar := renderSimilarRow(ds, w, similarH)

	full := lipgloss.JoinVertical(lipgloss.Left, header, main, similar)
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(w).
		Height(h).
		Render(full)
}

// ── Header bar ───────────────────────────────────────────────────────────────

func renderDetailHeader(ds *DetailState, w int, tab state.Tab) string {
	backHint := theme.T.DetailBackStyle().Render("← esc")

	breadcrumb := theme.T.BreadcrumbStyle().Render("  " + ds.BreadcrumbTrail(tab.String()))

	// Focus hint — changes based on which zone is active
	var focusHint string
	switch ds.Focus {
	case FocusDetailCast:
		focusHint = lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).
			Render("  ↑↓ navigate  enter search  tab → providers")
	case FocusDetailProvider:
		focusHint = lipgloss.NewStyle().Foreground(theme.T.Neon()).
			Render("  ←→ select  enter ▶ play  tab → similar")
	case FocusDetailSimilar:
		focusHint = lipgloss.NewStyle().Foreground(theme.T.AccentAlt()).
			Render("  ←→ scroll  enter open  tab → cast")
	default:
		focusHint = lipgloss.NewStyle().Foreground(theme.T.TextDim()).
			Render("  ↓/j cast  tab providers  1-4 quality  esc back")
	}

	// Right: tab + runtime pill
	pill := theme.T.StatusAccentStyle().Render(" stui ")
	hintW := lipgloss.Width(backHint) + lipgloss.Width(breadcrumb) + lipgloss.Width(focusHint)
	rightW := lipgloss.Width(pill)
	gap := max(0, w-hintW-rightW-4)

	row := backHint + breadcrumb + focusHint + strings.Repeat(" ", gap) + pill

	return lipgloss.NewStyle().
		Background(theme.T.Surface()).
		BorderStyle(lipgloss.NormalBorder()).
		BorderForeground(theme.T.Border()).
		BorderBottom(true).
		Width(w - 2).
		Render(row)
}

// ── Poster block ──────────────────────────────────────────────────────────────

func renderPosterBlock(ds *DetailState, w, h int) string {
	var poster string

	if ds.Entry.PosterArt != "" {
		poster = ds.Entry.PosterArt
	} else {
		poster = components.RenderPosterPlaceholder(ds.Entry.Title, ds.Entry.Genre, w-4, detailPosterHeight)
	}

	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(w).
		Height(h).
		Padding(2, 2).
		Render(poster)
}

// ── Info block (right side) ───────────────────────────────────────────────────

func renderInfoBlock(ds *DetailState, w, h int) string {
	var sections []string

	// Title + rating on same line
	titleW := w - 10
	titleStr := bidi.AlignedStyle(theme.T.DetailTitleStyle().Width(titleW), ds.Entry.Title).
		Render(bidi.Apply(components.Truncate(ds.Entry.Title, titleW)))
	ratingStr := theme.T.DetailRatingStyle().Render("★ " + ds.Entry.Rating)
	titleLine := lipgloss.JoinHorizontal(lipgloss.Top, titleStr, ratingStr)
	sections = append(sections, titleLine)

	// Meta: year · genre · runtime
	metaParts := []string{}
	if ds.Entry.Year != "" {
		metaParts = append(metaParts, ds.Entry.Year)
	}
	if ds.Entry.Genre != "" {
		metaParts = append(metaParts, ds.Entry.Genre)
	}
	if ds.Entry.Runtime != "" {
		metaParts = append(metaParts, ds.Entry.Runtime)
	}
	meta := theme.T.DetailMetaStyle().Render(strings.Join(metaParts, "  ·  "))
	sections = append(sections, meta, "")

	// Description — word-wrapped to panel width with BiDi support
	if ds.Entry.Description != "" {
		lines := bidi.WordWrap(ds.Entry.Description, w-2)
		descStyle := bidi.AlignedStyle(theme.T.DetailDescStyle().Width(w-2), ds.Entry.Description)
		desc := strings.Join(lines, "\n")
		sections = append(sections, descStyle.Render(desc), "")
	} else if ds.Loading {
		sections = append(sections,
			lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("Loading details…"),
			"",
		)
	}

	// CAST & CREW
	if len(ds.Entry.Cast) > 0 {
		sections = append(sections, theme.T.DetailSectionStyle().Render("CAST & CREW"))

		for i, member := range ds.Entry.Cast {
			row := renderCastRow(member, i, ds.CastCursor, ds.Focus, w)
			sections = append(sections, row)
		}
		sections = append(sections, "")
	}

	// Continue Watching — resume position hint
	if ds.WatchHistory != nil && ds.WatchHistory.Position > 0 && !ds.WatchHistory.Completed {
		sections = append(sections, renderResumeHint(ds.WatchHistory, w))
		sections = append(sections, "")
	}

	// STREAM VIA — selectable provider badges
	if len(ds.Entry.Providers) > 0 {
		sections = append(sections, theme.T.DetailSectionStyle().Render("STREAM VIA"))
		var badges []string
		for i, p := range ds.Entry.Providers {
			focused := ds.Focus == FocusDetailProvider && i == ds.ProviderCursor
			if focused {
				badge := lipgloss.NewStyle().
					Background(theme.T.Accent()).
					Foreground(lipgloss.Color("#ffffff")).
					PaddingLeft(1).PaddingRight(1).MarginRight(1).
					Bold(true).
					BorderStyle(lipgloss.RoundedBorder()).
					BorderForeground(theme.T.Neon()).
					BorderBackground(theme.T.Accent()).
					Render("▶ " + p)
				badges = append(badges, badge)
			} else {
				badges = append(badges, theme.T.DetailProviderStyle().Render("◆ "+p))
			}
		}
		sections = append(sections, lipgloss.JoinHorizontal(lipgloss.Top, badges...))
		if ds.Focus == FocusDetailProvider {
			sections = append(sections, lipgloss.NewStyle().
				Foreground(theme.T.TextMuted()).PaddingLeft(2).
				Render("enter to play"))
		}
	}

	// NowPlaying inline bar (shown inside the info block when playing)
	if ds.NowPlaying != nil {
		sections = append(sections, "")
		sections = append(sections, components.RenderNowPlaying(ds.NowPlaying, w-4))
	}

	// Collection picker — shown when 'c' is pressed
	if ds.CollectionPickerOpen {
		sections = append(sections, "")
		sections = append(sections, renderCollectionPicker(ds, w))
	}

	content := strings.Join(sections, "\n")

	// Apply scroll offset
	lines := strings.Split(content, "\n")
	if len(lines) == 0 {
		return lipgloss.NewStyle().
			Background(theme.T.Bg()).
			Padding(1, 2).
			Width(w).
			Height(h).
			Render("")
	}
	scroll := ds.InfoScroll
	if scroll >= len(lines) {
		scroll = len(lines) - 1
	}
	visibleLines := lines[scroll:]
	// Cap to available height
	if len(visibleLines) > h-2 {
		visibleLines = visibleLines[:h-2]
	}
	content = strings.Join(visibleLines, "\n")

	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Padding(1, 2).
		Width(w).
		Height(h).
		Render(content)
}

func renderCastRow(
	member ipc.CastMember,
	idx, cursor int,
	focus DetailFocus,
	w int,
) string {
	isFocused := focus == FocusDetailCast && idx == cursor

	nameW := 24
	roleW := 22
	name := components.Truncate(member.Name, nameW)
	role := components.Truncate(member.Role, roleW)

	var nameStr, roleStr, linkStr string

	if isFocused {
		nameStr = theme.T.DetailCastFocusedStyle().Width(nameW).Render("▸ " + name)
		roleStr = theme.T.DetailRoleStyle().
			Foreground(theme.T.AccentAlt()).
			Width(roleW).
			Render(role)
		linkStr = theme.T.DetailLinkStyle().Render("  enter → search")
	} else {
		nameStr = theme.T.DetailCastStyle().Width(nameW + 2).Render("  " + name)
		roleStr = theme.T.DetailRoleStyle().Width(roleW).Render(role)
		linkStr = lipgloss.NewStyle().Foreground(theme.T.Border()).Render("  ↵")
	}

	return lipgloss.JoinHorizontal(lipgloss.Top, nameStr, roleStr, linkStr)
}

// ── Similar titles row ────────────────────────────────────────────────────────

func renderSimilarRow(ds *DetailState, w, h int) string {
	header := theme.T.SimilarHeaderStyle().Width(w - 2).Render("SIMILAR TITLES")

	if ds.SimilarLoading {
		loading := lipgloss.NewStyle().
			Foreground(theme.T.Neon()).
			PaddingLeft(2).
			Render("⠿ Loading similar titles…")
		return lipgloss.NewStyle().
			Background(theme.T.Surface()).
			Width(w).Height(h).
			Render(lipgloss.JoinVertical(lipgloss.Left, header, loading))
	}

	if len(ds.Similar) == 0 {
		empty := lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			PaddingLeft(2).
			Render("No similar titles found")
		return lipgloss.NewStyle().
			Background(theme.T.Surface()).
			Width(w).Height(h).
			Render(lipgloss.JoinVertical(lipgloss.Left, header, empty))
	}

	// Render up to similarCardCols mini cards
	cardH := h - lipgloss.Height(header) - 1
	miniW := (w - (similarCardCols+1)*2) / similarCardCols
	if miniW < 10 {
		miniW = 10
	}

	var cards []string
	start := ds.SimilarCursor
	if start >= len(ds.Similar) {
		start = 0
	}
	end := min(start+similarCardCols, len(ds.Similar))

	for i := start; i < end; i++ {
		e := ds.Similar[i]
		selected := (ds.Focus == FocusDetailSimilar && i == ds.SimilarCursor)

		// Minimal card: just colored block + title
		bg := similarCardBg(e.Title)
		inits := components.PosterInitials(e.Title)

		posterBlock := lipgloss.NewStyle().
			Background(bg).
			Width(miniW).
			Height(cardH-3).
			Align(lipgloss.Center, lipgloss.Center).
			Render(inits)

		titleStr := components.Truncate(e.Title, miniW)
		titleBlock := lipgloss.NewStyle().
			Foreground(theme.T.Text()).
			Width(miniW).
			Render(titleStr)

		var border lipgloss.Style
		if selected {
			border = lipgloss.NewStyle().
				BorderStyle(lipgloss.RoundedBorder()).
				BorderForeground(theme.T.Accent()).
				Width(miniW)
		} else {
			border = lipgloss.NewStyle().
				BorderStyle(lipgloss.RoundedBorder()).
				BorderForeground(theme.T.Border()).
				Width(miniW)
		}

		card := border.Render(lipgloss.JoinVertical(lipgloss.Left, posterBlock, titleBlock))
		cards = append(cards, card)
	}

	// Scroll arrow if more available
	if end < len(ds.Similar) {
		arrow := lipgloss.NewStyle().
			Foreground(theme.T.AccentAlt()).
			Align(lipgloss.Center, lipgloss.Center).
			Height(cardH).
			Render("›")
		cards = append(cards, arrow)
	}

	row := lipgloss.JoinHorizontal(lipgloss.Top, cards...)
	content := lipgloss.JoinVertical(lipgloss.Left, header, row)

	return lipgloss.NewStyle().
		Background(theme.T.Surface()).
		Width(w).Height(h).
		PaddingLeft(1).
		Render(content)
}

func similarCardBg(title string) color.Color {
	colors := []color.Color{
		lipgloss.Color("#0d0d25"),
		lipgloss.Color("#0a1a0a"),
		lipgloss.Color("#1a0a0a"),
		lipgloss.Color("#0a0a1a"),
		lipgloss.Color("#1a1a00"),
		lipgloss.Color("#001a1a"),
	}
	return colors[components.TitleHash(title)%len(colors)]
}

// ── Person mode ───────────────────────────────────────────────────────────────

func renderPersonMode(ds *DetailState, w, h int, tab state.Tab) string {
	header := renderDetailHeader(ds, w, tab)

	availH := h - lipgloss.Height(header)

	var body string
	if ds.PersonLoading {
		body = CenteredMsg(w, availH,
			lipgloss.NewStyle().Foreground(theme.T.Neon()).
				Render(fmt.Sprintf("⠿  Searching for titles with %s…", ds.PersonName)),
		)
	} else if len(ds.PersonResults) == 0 {
		body = CenteredMsg(w, availH,
			lipgloss.NewStyle().Foreground(theme.T.TextDim()).
				Render(fmt.Sprintf("No results found for \u201c%s\u201d", ds.PersonName)),
		)
	} else {
		personHeader := theme.T.PersonHeaderStyle().
			Width(w - 2).
			Render(fmt.Sprintf("Titles featuring  %s", ds.PersonName))

		gridStr := RenderGrid(
			ds.PersonResults,
			ds.PersonCursor,
			w, availH-lipgloss.Height(personHeader),
			false,
			0,
			"ready",
			nil,
			nil,
		)
		body = lipgloss.JoinVertical(lipgloss.Left, personHeader, gridStr)
	}

	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(w).Height(h).
		Render(lipgloss.JoinVertical(lipgloss.Left, header, body))
}

// ── Collection picker ─────────────────────────────────────────────────────────

// renderCollectionPicker renders an inline "Add to collection" picker block
// inside the info panel. Shown when ds.CollectionPickerOpen is true.
func renderCollectionPicker(ds *DetailState, w int) string {
	header := theme.T.DetailSectionStyle().Render("ADD TO COLLECTION")

	if len(ds.CollectionPickerNames) == 0 {
		empty := lipgloss.NewStyle().
			Foreground(theme.T.TextDim()).
			PaddingLeft(2).
			Render("No collections — press 5 to manage them")
		return lipgloss.JoinVertical(lipgloss.Left, header, empty)
	}

	var rows []string
	for i, name := range ds.CollectionPickerNames {
		if i == ds.CollectionPickerCursor {
			rows = append(rows, theme.T.TabActiveStyle().
				PaddingLeft(2).
				Render("▸ "+name))
		} else {
			rows = append(rows, lipgloss.NewStyle().
				Foreground(theme.T.TextDim()).
				PaddingLeft(2).
				Render("  "+name))
		}
	}

	hint := lipgloss.NewStyle().
		Foreground(theme.T.TextMuted()).
		PaddingLeft(2).
		Render("↑↓ navigate  enter add  esc cancel")

	_ = w
	return lipgloss.JoinVertical(lipgloss.Left,
		append([]string{header}, append(rows, hint)...)...,
	)
}

// ── Resume hint ───────────────────────────────────────────────────────────────

// renderResumeHint renders a "Continue Watching" progress block for items that
// have been partially watched. It shows a progress bar, elapsed/total time, and
// a hint that the provider selection will automatically resume from this point.
func renderResumeHint(h *watchhistory.Entry, w int) string {
	header := theme.T.DetailSectionStyle().Render("CONTINUE WATCHING")

	posStr := formatDetailDuration(h.Position)
	var durStr, pctStr string
	if h.Duration > 0 {
		durStr = " / " + formatDetailDuration(h.Duration)
		pct := int(math.Min(h.Position/h.Duration, 1.0) * 100)
		pctStr = fmt.Sprintf("  %d%%", pct)
	}
	timeStr := lipgloss.NewStyle().
		Foreground(theme.T.Text()).
		Render(posStr + durStr + pctStr)

	// Progress bar
	barW := min(w-4, 40)
	bar := renderDetailProgressBar(h.Position, h.Duration, barW)

	hint := lipgloss.NewStyle().
		Foreground(theme.T.TextMuted()).
		Render("Select a provider below to resume automatically")

	return lipgloss.JoinVertical(lipgloss.Left,
		header,
		"  "+bar+"  "+timeStr,
		"  "+hint,
	)
}

func renderDetailProgressBar(pos, dur float64, width int) string {
	if width <= 0 {
		return ""
	}
	var fraction float64
	if dur > 0 {
		fraction = math.Min(pos/dur, 1.0)
	}
	filled := int(float64(width) * fraction)
	empty := width - filled
	bar := strings.Repeat("█", filled) + strings.Repeat("░", empty)
	return lipgloss.NewStyle().Foreground(theme.T.Accent()).Render(bar)
}

func formatDetailDuration(secs float64) string {
	total := int(secs)
	h := total / 3600
	m := (total % 3600) / 60
	s := total % 60
	if h > 0 {
		return fmt.Sprintf("%d:%02d:%02d", h, m, s)
	}
	return fmt.Sprintf("%d:%02d", m, s)
}

// ── Helpers ───────────────────────────────────────────────────────────────────
