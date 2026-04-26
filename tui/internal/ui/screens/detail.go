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
//  │  RELATED                                                        │
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
	"math"
	"strings"

	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/state"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/components/mediaheader"
	"github.com/stui/stui/pkg/bidi"
	"github.com/stui/stui/pkg/theme"
	"github.com/stui/stui/pkg/watchhistory"
)

const (
	detailPosterWidth  = mediaheader.PosterWidth
	detailPosterHeight = mediaheader.PosterHeight
	similarCardCols    = 6 // cards in the similar row
	similarRowHeight   = 8 // rows for similar section
	detailHeaderHeight = 3 // top bar + border
	detailStatusHeight = 2 // status bar
)

// Render string constants — shared across detail.go, detail_crew.go,
// detail_related.go and detail_artwork.go. Chunk 8 snapshot tests match
// the exact spelling here.
const (
	detailCrewHeader      = "CREW"
	detailRelatedHeader   = "RELATED"
	detailEmptyCredits    = "No crew or cast available"
	detailEmptyArtwork    = "No artwork available"
	detailEmptyRelated    = "No related items"
	detailLoadingCrew    = "Loading crew…"
	detailLoadingArtwork = "Loading artwork…"
	detailLoadingRelated = "Loading related…"
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
	// No separate header — breadcrumb + back-affordance live elsewhere
	// (the global status bar carries the hotkey hints via
	// DetailState.FooterText), and the top row reserved for the header
	// was worth reclaiming once RELATED needed the space.

	// Related row: reserve height only if it has content. When the
	// verb resolves empty the row is hidden entirely so the main body
	// reclaims the vertical space.
	relatedH := 0
	if ds.Meta.RelatedStatus == FetchPending ||
		(ds.Meta.RelatedStatus == FetchLoaded && len(ds.Meta.Related.Items) > 0) {
		relatedH = similarRowHeight + 2
	}

	// Artwork-status strip: only rendered while the artwork verb is
	// pending (loading). Empty state returns "" so it doesn't consume
	// a row between the main body and the related row.
	artworkStatus := renderBackdropStatusStrip(ds, w)
	statusH := 0
	if artworkStatus != "" {
		statusH = lipgloss.Height(artworkStatus)
	}

	// Split: poster|info section, then related row at bottom.
	mainH := h - relatedH - statusH

	left := renderPosterBlock(ds, detailPosterWidth, mainH)
	right := renderInfoBlock(ds, w-detailPosterWidth-4, mainH)

	main := lipgloss.JoinHorizontal(lipgloss.Top,
		left,
		lipgloss.NewStyle().
			Width(w-detailPosterWidth-4).
			Height(mainH).
			Render(right),
	)

	parts := []string{main}
	if artworkStatus != "" {
		parts = append(parts, artworkStatus)
	}
	if relatedH > 0 {
		parts = append(parts, renderRelatedRow(ds, w, relatedH))
	}
	full := lipgloss.JoinVertical(lipgloss.Left, parts...)

	// No border here — the parent View wraps this in MainCardStyle
	// (same style the grid/list screens use) so the detail overlay
	// matches the rest of STUI's chrome (rounded border, accent
	// color when focused, consistent side margins).
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(w).
		Height(h).
		Render(full)
}

// ── Header bar ───────────────────────────────────────────────────────────────

func renderDetailHeader(ds *DetailState, w int, tab state.Tab) string {
	// Header carries only the breadcrumb + back affordance now —
	// focus-specific hotkey hints have moved to the global status bar
	// via `DetailState.FooterText()` (mirrors the music-screen pattern
	// at music_browse.go:FooterText). This keeps the hint style
	// consistent with every other screen and reclaims a row on the card.
	backHint := theme.T.DetailBackStyle().Render("← esc")
	breadcrumb := theme.T.BreadcrumbStyle().Render("  " + ds.BreadcrumbTrail(tab.String()))

	hintW := lipgloss.Width(backHint) + lipgloss.Width(breadcrumb)
	gap := max(0, w-hintW-4)
	row := backHint + breadcrumb + strings.Repeat(" ", gap)

	return lipgloss.NewStyle().
		Background(theme.T.Surface()).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		BorderBottom(true).
		Width(w - 2).
		Render(row)
}

// FooterText returns the focus-specific hotkey hint for the detail
// overlay — read by `viewStatusBar` so the global footer carries the
// hint the same way every other screen does.
func (d *DetailState) FooterText() string {
	// Series-specific hint: advertise `e` so users can discover the
	// episode browser. Movies don't have episodes so the hotkey is
	// silently ignored there — keep their footer cleaner.
	episodesHint := ""
	if d.Entry.Tab == "series" || d.Entry.Tab == "Series" {
		episodesHint = "e episodes · "
	}
	switch d.Focus {
	case FocusDetailCrew:
		return "↑↓ crew · tab → cast · esc back"
	case FocusDetailCast:
		return "↑↓ navigate · enter search · tab next · esc back"
	case FocusDetailEpisodes:
		return "enter open episodes · tab → providers · esc back"
	case FocusDetailProvider:
		return "←→ select · enter ▶ play · tab → related · esc back"
	case FocusDetailRelated:
		return "←→ scroll · enter open · tab → info · esc back"
	default:
		return episodesHint + "tab → crew · j/↓ cast · 1-4 quality · esc back"
	}
}

// ── Poster block ──────────────────────────────────────────────────────────────

func renderPosterBlock(ds *DetailState, w, h int) string {
	posterW := w - 4
	poster := mediaheader.RenderPoster(mediaheader.Inputs{
		Title:     ds.Entry.Title,
		Genre:     ds.Entry.Genre,
		PosterArt: ds.Entry.PosterArt,
		PosterURL: ds.Entry.PosterURL,
	}, posterW, detailPosterHeight)

	// Backdrop carousel strip — rendered directly under the poster. Emits
	// a faint loading/empty label while the "artwork" verb is in-flight or
	// resolved empty; the index indicator once it lands with data.
	carousel := renderBackdropCarousel(ds, w-4)

	body := poster
	if carousel != "" {
		body = lipgloss.JoinVertical(lipgloss.Left, poster, "", carousel)
	}

	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(w).
		Height(h).
		Padding(2, 2).
		Render(body)
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

	// Meta: year · genre · runtime · studio
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
	// Studio lands here after the "enrich" verb resolves. It's also
	// re-surfaced in the CREW section so users see it in both reading
	// positions.
	if ds.Entry.Studio != "" {
		metaParts = append(metaParts, ds.Entry.Studio)
	}
	meta := theme.T.DetailMetaStyle().Render(strings.Join(metaParts, "  ·  "))
	sections = append(sections, meta, "")

	// Description — word-wrapped to panel width with BiDi support.
	// Clamped to `descMaxLines` so a long synopsis doesn't crowd out
	// CREW / STREAM VIA / RELATED below it. Over-limit lines are
	// truncated with a trailing "…" so it's clear there's more.
	const descMaxLines = 4
	if ds.Entry.Description != "" {
		lines := bidi.WordWrap(ds.Entry.Description, w-2)
		if len(lines) > descMaxLines {
			lines = lines[:descMaxLines]
			// Append ellipsis to the last visible line, truncating
			// one character if needed to keep within panel width.
			last := lines[len(lines)-1]
			if lipgloss.Width(last)+1 > w-2 && len(last) > 0 {
				// Drop the last rune to make room for the ellipsis.
				rr := []rune(last)
				last = string(rr[:len(rr)-1])
			}
			lines[len(lines)-1] = last + "…"
		}
		descStyle := bidi.AlignedStyle(theme.T.DetailDescStyle().Width(w-2), ds.Entry.Description)
		desc := strings.Join(lines, "\n")
		sections = append(sections, descStyle.Render(desc), "")
	} else if ds.Loading {
		sections = append(sections,
			lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("Loading details…"),
			"",
		)
	}

	// CREW — directors, DoP, composer, studio. Rendered above CAST so
	// headline creatives appear first in the reading order. Hidden
	// entirely when the credits verb resolves empty.
	if crew := renderCrewSection(ds, w); crew != "" {
		sections = append(sections, crew, "")
	}

	// CAST
	if len(ds.Entry.Cast) > 0 {
		sections = append(sections, theme.T.DetailSectionStyle().Render("CAST"))

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

	// EPISODES — series-only navigable badge that opens the season /
	// episode browser (the same screen the `e` keypress launches).
	// Rendered alongside STREAM VIA so the user discovers it through
	// Tab navigation rather than needing to know the hidden keybind.
	if ds.Entry.Tab == "series" || ds.Entry.Tab == "Series" {
		sections = append(sections, theme.T.DetailSectionStyle().Render("EPISODES"))
		focused := ds.Focus == FocusDetailEpisodes
		var badge string
		if focused {
			badge = lipgloss.NewStyle().
				Background(theme.T.Accent()).
				Foreground(lipgloss.Color("#ffffff")).
				PaddingLeft(1).PaddingRight(1).MarginRight(1).
				Bold(true).
				BorderStyle(lipgloss.RoundedBorder()).
				BorderForeground(theme.T.Neon()).
				BorderBackground(theme.T.Accent()).
				Render("▶ Browse episodes")
		} else {
			badge = theme.T.DetailProviderStyle().Render("◆ Browse episodes")
		}
		sections = append(sections, badge)
		if focused {
			sections = append(sections, lipgloss.NewStyle().
				Foreground(theme.T.TextMuted()).PaddingLeft(2).
				Render("enter to open"))
		}
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
	lines := strings.Split(content, "\n")
	if len(lines) == 0 {
		return lipgloss.NewStyle().
			Background(theme.T.Bg()).
			Padding(1, 2).
			Width(w).
			Height(h).
			Render("")
	}

	// h-2 accounts for the outer Padding(1, 2) top/bottom rows; visibleH
	// is the rows of actual content the panel can show. The scrollbar
	// track is exactly visibleH cells so thumb position maps 1:1 to the
	// visible window. Reserve 1 col of width for the scrollbar character;
	// the content column shrinks accordingly so each row is
	//   <content padded to (w-4)> <space> <bar char>
	// which keeps the bar at a stable rightmost column under any focus.
	visibleH := h - 2
	if visibleH < 1 {
		visibleH = 1
	}
	scroll := ds.InfoScroll
	maxScroll := len(lines) - visibleH
	if maxScroll < 0 {
		maxScroll = 0
	}
	if scroll > maxScroll {
		scroll = maxScroll
	}
	if scroll < 0 {
		scroll = 0
	}

	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim()).Background(theme.T.Bg())
	barChars := components.ScrollbarChars(scroll, visibleH, len(lines), dim)

	// Build each visible row as: content-line (padded to contentW) + " " + bar.
	contentW := w - 4 // 2 col left pad, 1 col gap, 1 col scrollbar
	if contentW < 1 {
		contentW = 1
	}
	contentLineStyle := lipgloss.NewStyle().Width(contentW).MaxWidth(contentW)
	rows := make([]string, 0, visibleH)
	for r := 0; r < visibleH; r++ {
		idx := scroll + r
		var lineText string
		if idx < len(lines) {
			lineText = lines[idx]
		}
		row := contentLineStyle.Render(lineText) + " " + barChars[r]
		rows = append(rows, row)
	}

	body := strings.Join(rows, "\n")
	// Padding(1, 0, 1, 2) — top:1, right:0, bottom:1, left:2.  Inner
	// horizontal area is w-2; each row is (w-4) content + 1 gap + 1 bar
	// = w-2, exactly filling the inner width with no overflow.
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Padding(1, 0, 1, 2).
		Width(w).
		Height(h).
		Render(body)
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
