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

// Layout: poster + header (title/meta/desc/resume) on top, tab bar
// below, tab body fills the remainder. The previous "single big info
// block with everything stacked" layout was replaced because it had
// cast/crew/related/streams competing for the same scroll buffer; tabs
// give each section its own scroll scope and let series and movies
// expose their per-tab affordance (Episodes vs Streams) on equal
// footing.
func renderDetailMain(ds *DetailState, w, h int, tab state.Tab) string {
	_ = tab
	// Header: poster column + info column. Poster sets a fixed height
	// (mediaheader.PosterHeight + breathing room); the info column
	// takes the same vertical slot and renders title/meta/desc/resume
	// inside it.
	headerH := detailPosterHeight + 4
	if headerH > h-6 {
		headerH = h - 6
		if headerH < 6 {
			headerH = 6
		}
	}

	leftW := detailPosterWidth
	rightW := w - leftW - 4
	if rightW < 20 {
		rightW = 20
	}

	left := renderPosterBlock(ds, leftW, headerH)
	right := renderHeaderInfo(ds, rightW, headerH)
	header := lipgloss.JoinHorizontal(lipgloss.Top,
		left,
		lipgloss.NewStyle().
			Width(rightW).
			Height(headerH).
			Render(right),
	)

	tabBar := renderDetailTabBar(ds, w)
	tabBarH := lipgloss.Height(tabBar)

	// Related carousel: a fixed-height row at the bottom of the
	// detail card showing poster mini-cards for related titles.
	// Lives outside the tab bodies so it's always visible regardless
	// of which tab is active — it's discovery, not credits/streams.
	// Hidden entirely when the related verb resolves empty.
	relatedH := 0
	if ds.Meta.RelatedStatus == FetchPending ||
		(ds.Meta.RelatedStatus == FetchLoaded && len(ds.Meta.Related.Items) > 0) {
		relatedH = similarRowHeight + 2
	}

	bodyH := h - headerH - tabBarH - relatedH
	if bodyH < 1 {
		bodyH = 1
	}
	body := renderDetailTabBody(ds, w, bodyH)

	parts := []string{header, tabBar, body}
	if relatedH > 0 {
		parts = append(parts, renderRelatedRow(ds, w, relatedH))
	}
	// No outer Style.Background.Width.Height.Render wrap — the music
	// screen returns its `JoinVertical(tabBar, body)` directly and
	// the tab-bar's bottom underline sticks cleanly to the body. The
	// extra wrap was inserting Background-styled padding cells that
	// rendered as artifacts at the tab/body boundary on detail. The
	// parent View() wraps this in MainCardStyle anyway, which
	// applies its own Background + sizing.
	return lipgloss.JoinVertical(lipgloss.Left, parts...)
}

// renderDetailTabBar emits the tab bar between the header and the tab
// body, using the same `components.RenderTabs` widget the Music screen
// uses so Movies / Series / Music share visual identity.
func renderDetailTabBar(ds *DetailState, w int) string {
	tabs := ds.AvailableTabs()
	options := make([]components.TabOption, 0, len(tabs))
	for _, t := range tabs {
		options = append(options, components.TabOption{
			Label:    t.String(),
			IsActive: t == ds.ActiveTab,
		})
	}
	return components.RenderTabs(options, theme.T.Border(), theme.T.Accent(), w)
}

// HitTestDetailTabBar maps a horizontal click x-coord to the tab the
// user landed on. Returns false when the click is outside the tab
// rectangles (e.g. in the underline that fills the rest of the row).
//
// Tab box widths track `RenderTabs`: each tab is `Padding(0, 1)` (so
// 2 cols of side padding) plus a 2-col border (left+right), totalling
// 4 cols of chrome around the label. Cumulative left edges are
// computed in lockstep so this stays correct for arbitrary numbers of
// tabs.
func (d *DetailState) HitTestDetailTabBar(x int) (DetailTab, bool) {
	tabs := d.AvailableTabs()
	cursor := 0
	for _, t := range tabs {
		w := lipgloss.Width(t.String()) + 4
		if x >= cursor && x < cursor+w {
			return t, true
		}
		cursor += w
	}
	return DetailTabDescription, false
}

// renderDetailTabBody dispatches to the right tab renderer.
func renderDetailTabBody(ds *DetailState, w, h int) string {
	switch ds.ActiveTab {
	case DetailTabEpisodes:
		return renderEpisodesTab(ds, w, h)
	case DetailTabStreams:
		return renderStreamsTab(ds, w, h)
	default:
		return renderDescriptionTab(ds, w, h)
	}
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
//
// The hint reflects both the active tab AND the focus zone within
// it. Tab key always cycles tabs first; per-tab keys (j/k navigate,
// h/l for season ↔ episode column or provider row) come after.
func (d *DetailState) FooterText() string {
	switch d.ActiveTab {
	case DetailTabEpisodes:
		switch d.Focus {
		case FocusDetailSeasons:
			return "↑↓ seasons · → episodes · tab next tab · esc back"
		case FocusDetailEpisodes:
			return "↑↓ episodes · ← seasons · → streams · tab next tab · esc back"
		case FocusDetailEpisodeStreams:
			return "↑↓ stream · ← episodes · enter ▶ play · tab next tab · esc back"
		default:
			return "tab next tab · ↑↓ navigate · esc back"
		}
	case DetailTabStreams:
		if d.Focus == FocusDetailProvider {
			return "←→ provider · enter ▶ play · 1-4 quality · tab next tab · esc back"
		}
		return "tab next tab · 1-4 quality · esc back"
	default:
		// Description tab.
		switch d.Focus {
		case FocusDetailCrew:
			return "↑↓ crew · tab next tab · esc back"
		case FocusDetailCast:
			return "↑↓ navigate · enter search · tab next tab · esc back"
		case FocusDetailRelated:
			return "↑↓ scroll · enter open · tab next tab · esc back"
		default:
			return "tab next tab · j/↓ scroll · esc back"
		}
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

// renderHeaderInfo renders the right-of-poster column for the header
// row: title + ★ rating, meta line (year · genre · runtime · studio),
// description (word-wrapped, clamped), and the Continue Watching
// resume hint. Fixed-height — no scrollbar; everything heavier
// (CREW/CAST/RELATED/STREAM VIA/Episodes) lives in tab bodies below.
func renderHeaderInfo(ds *DetailState, w, h int) string {
	var sections []string

	// Title + ★ rating on the same line.
	titleW := w - 10
	titleStr := bidi.AlignedStyle(theme.T.DetailTitleStyle().Width(titleW), ds.Entry.Title).
		Render(bidi.Apply(components.Truncate(ds.Entry.Title, titleW)))
	ratingStr := theme.T.DetailRatingStyle().Render("★ " + ds.Entry.Rating)
	sections = append(sections, lipgloss.JoinHorizontal(lipgloss.Top, titleStr, ratingStr))

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
	if ds.Entry.Studio != "" {
		metaParts = append(metaParts, ds.Entry.Studio)
	}
	sections = append(sections,
		theme.T.DetailMetaStyle().Render(strings.Join(metaParts, "  ·  ")),
		"",
	)

	// Description — word-wrapped + clamped. AniList et al inject HTML
	// in synopses; cleanDescription normalizes before WordWrap.
	const descMaxLines = 4
	desc := cleanDescription(ds.Entry.Description)
	if desc != "" {
		lines := bidi.WordWrap(desc, w-2)
		if len(lines) > descMaxLines {
			lines = lines[:descMaxLines]
			last := lines[len(lines)-1]
			if lipgloss.Width(last)+1 > w-2 && len(last) > 0 {
				rr := []rune(last)
				last = string(rr[:len(rr)-1])
			}
			lines[len(lines)-1] = last + "…"
		}
		descStyle := bidi.AlignedStyle(theme.T.DetailDescStyle().Width(w-2), desc)
		sections = append(sections, descStyle.Render(strings.Join(lines, "\n")))
	} else if ds.Loading {
		sections = append(sections,
			lipgloss.NewStyle().Foreground(theme.T.TextDim()).Render("Loading details…"),
		)
	}

	// Language line — sits under the synopsis when known. ISO 639-1
	// codes ("ja", "en", "ko") get formatted as their English display
	// name via golang.org/x/text/language. Unknown codes fall back to
	// the upper-cased code so the user still sees something useful.
	if lang := formatLanguage(ds.Entry.OriginalLanguage); lang != "" {
		sections = append(sections,
			theme.T.DetailMetaStyle().Render("Language: "+lang),
		)
	}

	// Continue Watching — kept in the always-visible header (per user
	// preference) so the resume affordance is one glance away
	// regardless of which tab is active.
	if ds.WatchHistory != nil && ds.WatchHistory.Position > 0 && !ds.WatchHistory.Completed {
		sections = append(sections, "")
		sections = append(sections, renderResumeHint(ds.WatchHistory, w))
	}

	body := strings.Join(sections, "\n")
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Padding(1, 0, 0, 2).
		Width(w).
		Height(h).
		Render(body)
}

// scrolledTabBody renders `content` clipped to `h` rows starting at
// `scroll`, with a vertical scrollbar in the rightmost column.
// Layout per row: 2-col left pad · (w-4)-col content · 1-col bar · 1-col
// right pad. The right pad keeps the bar one column shy of the terminal
// edge so writing it doesn't trip DECAWM auto-wrap (see scrollbar memo
// in the components/scrollbar.go fix).
func scrolledTabBody(content string, scroll, w, h int) string {
	lines := strings.Split(content, "\n")
	innerH := h
	if innerH < 1 {
		innerH = 1
	}
	contentW := w - 4
	if contentW < 1 {
		contentW = 1
	}
	maxScroll := len(lines) - innerH
	if maxScroll < 0 {
		maxScroll = 0
	}
	if scroll > maxScroll {
		scroll = maxScroll
	}
	if scroll < 0 {
		scroll = 0
	}

	rows := make([]string, 0, innerH)
	for r := 0; r < innerH; r++ {
		idx := scroll + r
		var lineText string
		if idx < len(lines) {
			lineText = lines[idx]
		}
		rows = append(rows, "  "+padRightANSI(lineText, contentW))
	}
	contentBlock := strings.Join(rows, "\n")
	bar := components.Scrollbar(scroll, innerH, len(lines))
	return lipgloss.JoinHorizontal(lipgloss.Top, contentBlock, bar)
}

// renderDescriptionTab — CREW · CAST. Single scrolling pane
// (InfoScroll). RELATED moved out of the tab body and back to the
// bottom-of-detail poster carousel (`renderRelatedRow`) so it stays
// visible regardless of which tab is active. STREAM VIA + EPISODES
// have their own tabs.
func renderDescriptionTab(ds *DetailState, w, h int) string {
	var sections []string

	if rb := renderRatingsAggregatorSection(ds, w); rb != "" {
		sections = append(sections, rb, "")
	}

	if crew := renderCrewSection(ds, w); crew != "" {
		sections = append(sections, crew, "")
	}

	if len(ds.Entry.Cast) > 0 {
		sections = append(sections, theme.T.DetailSectionStyle().Render("CAST"))
		for i, member := range ds.Entry.Cast {
			sections = append(sections, renderCastRow(member, i, ds.CastCursor, ds.Focus, w))
		}
		sections = append(sections, "")
	}

	if ds.NowPlaying != nil {
		sections = append(sections, "")
		sections = append(sections, components.RenderNowPlaying(ds.NowPlaying, w-4))
	}

	if ds.CollectionPickerOpen {
		sections = append(sections, "")
		sections = append(sections, renderCollectionPicker(ds, w))
	}

	if len(sections) == 0 {
		dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
		sections = append(sections, dim.Render("  No additional details available yet."))
	}

	return scrolledTabBody(strings.Join(sections, "\n"), ds.InfoScroll, w, h)
}

// renderStreamsTab — Movies' "Streams" tab. Renders the same torrent
// search panel that lives in the per-episode streams column of the
// Episodes tab: press Enter to fan out across stream-providers,
// partials populate as they arrive, sorted on completion. Reuses the
// `EpisodeStreams` cache with the `{0,0}` sentinel key (see
// `DetailState.CurrentStreamsKey`) so the streaming pipeline serves
// both flows.
func renderStreamsTab(ds *DetailState, w, h int) string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())

	key := ds.CurrentStreamsKey()
	streams, isLoaded := ds.EpisodeStreams[key], ds.EpisodeStreamsLoaded[key]
	errMsg := ds.EpisodeStreamsError[key]
	inFlight := ds.EpisodeStreamsInFlight[key]

	var lines []string
	switch {
	case errMsg != "":
		lines = append(lines, lipgloss.NewStyle().
			Foreground(theme.T.Red()).
			Render("  "+errMsg))
	case inFlight:
		lines = append(lines, dim.Render("  Searching torrents…"))
	case !isLoaded:
		lines = append(lines, dim.Render("  Press Enter to search streams"))
	case len(streams) == 0:
		lines = append(lines, dim.Render("  No streams found."))
	default:
		for i, s := range streams {
			cursor := "  "
			lineStyle := normal
			if ds.Focus == FocusDetailEpisodeStreams && i == ds.EpisodeStreamCursor {
				cursor = "▶ "
				lineStyle = acc
			}
			parts := []string{}
			if s.Quality != "" {
				parts = append(parts, s.Quality)
			}
			if s.SizeBytes > 0 {
				parts = append(parts, humanizeSize(s.SizeBytes))
			}
			if s.Seeders > 0 {
				parts = append(parts, fmt.Sprintf("↑%d", s.Seeders))
			}
			meta := strings.Join(parts, " · ")
			if meta == "" {
				meta = s.Provider
			}
			row := cursor + lineStyle.Render(meta)
			if s.Provider != "" && len(parts) > 0 {
				row += "  " + dim.Render(s.Provider)
			}
			lines = append(lines, row)
		}
	}
	// Cursor-driven scroll: drive the scrollTabBody offset from
	// EpisodeStreamCursor (which j/k mutate) instead of ds.InfoScroll
	// (description-tab scrolling). Without this the panel stays pinned
	// to top while the cursor walks off the visible window — the user
	// presses j and the highlight visibly moves until row N where it
	// vanishes below the fold.
	_, top := windowedView(lines, ds.EpisodeStreamCursor, h)
	return scrolledTabBody(strings.Join(lines, "\n"), top, w, h)
}

// windowedView slides a `height`-tall view over `lines` so `cursor`
// stays in the visible slice, with a half-height lead-in so the
// selection isn't pinned to the top edge. Returns the visible slice
// and the top index for scrollbar rendering.
//
// When `lines` already fits, the input is returned unchanged with
// `top = 0`. Cursor is clamped into [0, len-1] to defend against
// stale indices (e.g. while partials are still streaming in and the
// list is growing).
func windowedView(lines []string, cursor, height int) (visible []string, top int) {
	if height <= 0 || len(lines) == 0 {
		return nil, 0
	}
	if len(lines) <= height {
		return lines, 0
	}
	if cursor < 0 {
		cursor = 0
	}
	if cursor >= len(lines) {
		cursor = len(lines) - 1
	}
	half := height / 2
	top = cursor - half
	if top < 0 {
		top = 0
	}
	if top+height > len(lines) {
		top = len(lines) - height
	}
	return lines[top : top+height], top
}

// renderEpisodesTab — Series-only "Episodes" tab. Three columns:
//   1. Seasons (left, ~22 cols matching the poster width above)
//   2. Episodes for the selected season (middle, flexes)
//   3. Streams for the selected episode (right, ~36 cols)
//
// The streams column is the placeholder slot for the future
// streaming pipeline — when the user lands on an episode, it'll
// populate with ranked streams (provider · quality · size · seeds)
// from a `GetStreams` / `RankStreams` IPC verb. Today it shows a
// "select an episode" / "(streams pending pipeline wire-up)" hint
// so the layout is right when the data arrives.
//
// Replaces the standalone `EpisodeScreen` for the in-detail flow —
// the keybind `e` now switches to this tab instead of pushing a new
// screen onto the stack.
func renderEpisodesTab(ds *DetailState, w, h int) string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())

	count := int(ds.Entry.SeasonCount)
	if count <= 0 {
		count = 1
	}
	// "Specials" appears as the row immediately after the regular
	// seasons when the provider has a season-0 track. The total slot
	// count includes it so the cursor + bounds math is uniform.
	totalSlots := count
	if ds.Entry.HasSpecials {
		totalSlots++
	}
	if ds.SeasonCursor < 0 {
		ds.SeasonCursor = 0
	}
	if ds.SeasonCursor >= totalSlots {
		ds.SeasonCursor = totalSlots - 1
	}

	// Column widths. Seasons is fixed at PosterWidth so it lines up
	// under the header poster. Streams claims the right ~36 cols
	// (enough for "[provider] 1080p · WEB-DL · 6.4 GB"). Episodes
	// flexes between them.
	seasonW := mediaheader.PosterWidth
	streamsW := 52
	// Body row composition is seasonW + 2 (sep) + epW + 2 (sep) + streamsW.
	// Outer wrapper uses Padding(1, 1, 1, 2), so the inner content area
	// is w - 3 cells. Reserving `- 7` (= 4 separator cols + 3 wrapper
	// pad cols) keeps the rightmost scrollbar glyph one cell shy of the
	// terminal edge, sidestepping the DECAWM auto-wrap that produced the
	// "double-line" rendering when bars landed in column w.
	epW := w - seasonW - streamsW - 7
	if epW < 20 {
		epW = 20
		if streamsW > w-seasonW-epW-7 {
			streamsW = max(20, w-seasonW-epW-7)
		}
	}

	// ── Column 1: seasons ──────────────────────────────────────────
	var seasonLines []string
	for i := 0; i < totalSlots; i++ {
		// Slots 0..count-1 → "Season 1..N"; the trailing slot (when
		// HasSpecials is set) → "Specials".
		var label string
		if ds.Entry.HasSpecials && i == count {
			label = "Specials"
		} else {
			label = fmt.Sprintf("Season %d", i+1)
		}
		var line string
		if i == ds.SeasonCursor && ds.Focus == FocusDetailSeasons {
			line = acc.Render("▶ " + label)
		} else if i == ds.SeasonCursor {
			line = normal.Render("▶ " + label)
		} else {
			line = dim.Render("  " + label)
		}
		seasonLines = append(seasonLines, line)
	}
	// Vertical content budget for all three columns. The wrapping
	// style on the body uses Padding(1, 0, 1, 2), so 2 rows are
	// already claimed for vertical breathing room — the remaining h-2
	// is what each column gets to fill (or scroll within).
	contentH := h - 2
	if contentH < 3 {
		contentH = 3
	}

	// Each column reserves 1 col on the right edge for the
	// scrollbar glyph (matches the grid.go pattern). The bar's
	// distinct foreground colour gives enough visual separation
	// from the column content without an extra gutter column.
	seasonContentW := seasonW - 1
	if seasonContentW < 1 {
		seasonContentW = 1
	}

	// ── Column 2: episodes ─────────────────────────────────────────
	curSeason := ds.SeasonNumberForCursor()
	episodes, loaded := ds.Episodes[curSeason], ds.EpisodesLoaded[curSeason]
	var epLines []string
	epCursor := 0
	currentSeasonErr := ds.EpisodesError[curSeason]
	switch {
	case currentSeasonErr != "":
		// Strip SDK error-code prefix (METADATA_FAILED:, etc.) so the
		// user sees the human-readable tail; word-wrap to the column
		// width so a long message doesn't overflow into the streams
		// column or the next row.
		clean := stripErrorPrefix(currentSeasonErr)
		red := lipgloss.NewStyle().Foreground(theme.T.Red())
		// epContentW is computed further down (epW - 1, for the bar
		// gutter); replicate inline. Subtract another 2 for the leading
		// "  " indent each emitted line gets.
		wrapAt := epW - 3
		if wrapAt < 20 {
			wrapAt = 20
		}
		for _, line := range wrapWords("Failed to load episodes: "+clean, wrapAt) {
			epLines = append(epLines, red.Render("  "+line))
		}
	case !loaded:
		epLines = []string{dim.Render("  Loading episodes…")}
	case len(episodes) == 0:
		epLines = []string{dim.Render("  No episodes for this season.")}
	default:
		for i, ep := range episodes {
			cursor := "  "
			style := normal
			if i == ds.EpisodeCursor && ds.Focus == FocusDetailEpisodes {
				cursor = "▶ "
				style = acc
			} else if i == ds.EpisodeCursor {
				cursor = "▶ "
			}
			epNum := dim.Render(fmt.Sprintf("E%02d", ep.Episode))
			title := ep.Title
			// −1 from epW for the scrollbar.
			maxTitle := epW - 13
			if maxTitle > 0 && len(title) > maxTitle {
				title = title[:maxTitle-1] + "…"
			}
			line := cursor + epNum + "  " + style.Render(title)
			if ep.AirDate != "" && len(ep.AirDate) >= 10 {
				line += "  " + dim.Render(ep.AirDate[:10])
			}
			epLines = append(epLines, line)
		}
		epCursor = ds.EpisodeCursor
	}

	// ── Column 3: streams ──────────────────────────────────────────
	// −1 from streamsW for the scrollbar glyph.
	streamsContentW := streamsW - 1
	if streamsContentW < 1 {
		streamsContentW = 1
	}
	// No "STREAMS" header / "S01EXX" line — the column position +
	// the prompts ("Press Enter to search streams" / "Pick an
	// episode first") read clearly enough on their own. Removing
	// the header also sidesteps the MarginTop(1) embedded-newline
	// trap that DetailSectionStyle introduced.
	var streamsLines []string
	streamCursorAbs := 0
	switch {
	case !loaded || len(episodes) == 0:
		streamsLines = append(streamsLines, dim.Render("  Pick an episode first."))
	case ds.EpisodeCursor < 0 || ds.EpisodeCursor >= len(episodes):
		streamsLines = append(streamsLines, dim.Render("  Pick an episode first."))
	default:
		ep := episodes[ds.EpisodeCursor]
		key := EpisodeStreamsKey{Season: ds.SeasonCursor + 1, Episode: int(ep.Episode)}
		streams, isLoaded := ds.EpisodeStreams[key], ds.EpisodeStreamsLoaded[key]
		errMsg := ds.EpisodeStreamsError[key]
		inFlight := ds.EpisodeStreamsInFlight[key]
		switch {
		case errMsg != "":
			// Streams pipeline joins per-provider failures with `"; "`.
			// Split, compact each one (stripping ERROR_CODE prefixes
			// and re-paste hints), then hard-truncate to the column
			// width so long URLs / stack traces can't bleed into the
			// layout.
			redStyle := lipgloss.NewStyle().Foreground(theme.T.Red())
			for _, part := range strings.Split(errMsg, "; ") {
				row := "  " + compactProviderError(part)
				if lipgloss.Width(row) > streamsContentW-2 {
					rr := []rune(row)
					for lipgloss.Width(string(rr)) > streamsContentW-3 && len(rr) > 0 {
						rr = rr[:len(rr)-1]
					}
					row = string(rr) + "…"
				}
				streamsLines = append(streamsLines, redStyle.Render(row))
			}
		case inFlight:
			streamsLines = append(streamsLines, dim.Render("  Searching torrents…"))
		case !isLoaded:
			streamsLines = append(streamsLines, dim.Render("  Press Enter to search streams"))
		case len(streams) == 0:
			streamsLines = append(streamsLines, dim.Render("  No streams found."))
		default:
			// First stream row sits at len(streamsLines) right now
			// (= 0 in the no-header case). Capture the offset so we
			// can translate ds.EpisodeStreamCursor into an absolute
			// slice index for windowing + scrollbar.
			firstStreamRowAbs := len(streamsLines)
			for i, s := range streams {
				cursor := "  "
				lineStyle := normal
				if ds.Focus == FocusDetailEpisodeStreams && i == ds.EpisodeStreamCursor {
					cursor = "▶ "
					lineStyle = acc
				}
				// Compact summary: quality · size · seeders. Keep
				// to one line — the streams column is narrow.
				parts := []string{}
				if s.Quality != "" {
					parts = append(parts, s.Quality)
				}
				if s.SizeBytes > 0 {
					parts = append(parts, humanizeSize(s.SizeBytes))
				}
				if s.Seeders > 0 {
					parts = append(parts, fmt.Sprintf("↑%d", s.Seeders))
				}
				meta := strings.Join(parts, " · ")
				if meta == "" {
					meta = s.Provider
				}
				row := cursor + lineStyle.Render(meta)
				if s.Provider != "" && len(parts) > 0 {
					row += "  " + dim.Render(s.Provider)
				}
				// Hard-truncate to streams column content width
				// (less the scrollbar gutter).
				if lipgloss.Width(row) > streamsContentW-2 {
					rr := []rune(row)
					if len(rr) > streamsContentW-3 {
						row = string(rr[:streamsContentW-3]) + "…"
					}
				}
				streamsLines = append(streamsLines, row)
			}
			if ds.EpisodeStreamCursor >= 0 && ds.EpisodeStreamCursor < len(streams) {
				streamCursorAbs = firstStreamRowAbs + ds.EpisodeStreamCursor
			}
		}
	}

	// Compose each column row-by-row using padRightANSI + the
	// matching scrollbar glyph. Going through `lipgloss.Width(N).
	// Render(multilineString)` was the source of the row-
	// fragmentation bug: lipgloss's block renderer interleaved
	// blank rows when fed a \n-separated string. The same manual
	// composition pattern is what `scrolledTabBody` and the
	// music_queue use, both of which render cleanly.
	epContentW := epW - 1
	if epContentW < 1 {
		epContentW = 1
	}
	// Build each column as a plain multi-line content block (no
	// embedded scrollbar glyphs), then pass content + scrollbar as
	// SEPARATE blocks to JoinHorizontal. This matches the
	// `music_queue` pattern (which renders cleanly) — the previous
	// per-row interleaving via composeColumnWithBar appeared to
	// cause visual fragmentation of the scrollbar in this screen
	// even though byte-level output looked correct.
	seasonVisible, seasonTop := windowedView(seasonLines, ds.SeasonCursor, contentH)
	seasonBar := components.Scrollbar(seasonTop, len(seasonVisible), len(seasonLines))
	seasonContentLines := make([]string, len(seasonVisible))
	for i, l := range seasonVisible {
		seasonContentLines[i] = padRightANSI(l, seasonContentW)
	}
	seasonContent := strings.Join(seasonContentLines, "\n")

	epVisible, epTop := windowedView(epLines, epCursor, contentH)
	epBar := components.Scrollbar(epTop, len(epVisible), len(epLines))
	epContentLines := make([]string, len(epVisible))
	for i, l := range epVisible {
		epContentLines[i] = padRightANSI(l, epContentW)
	}
	epContent := strings.Join(epContentLines, "\n")

	streamsVisible, streamsTop := windowedView(streamsLines, streamCursorAbs, contentH)
	streamsBar := components.Scrollbar(streamsTop, len(streamsVisible), len(streamsLines))
	streamsContentLines := make([]string, len(streamsVisible))
	for i, l := range streamsVisible {
		streamsContentLines[i] = padRightANSI(l, streamsContentW)
	}
	streamsContent := strings.Join(streamsContentLines, "\n")

	body := lipgloss.JoinHorizontal(lipgloss.Top,
		seasonContent, seasonBar,
		"  ",
		epContent, epBar,
		"  ",
		streamsContent, streamsBar,
	)
	// 1-col right padding keeps the rightmost scrollbar cell one
	// column shy of the terminal edge. Writing into column w trips
	// DECAWM auto-wrap (cursor advances to the next row), and the
	// trailing `\n` between body rows then advances it again — every
	// row would consume two visual lines.
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Padding(1, 1, 1, 2).
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

// stripErrorPrefix removes the well-known SDK error-code prefix
// (`METADATA_FAILED: `, `AUTH_ERROR: `, `HTTP_ERROR: `, …) from the
// front of a runtime error string so what reaches the user is just
// the human-readable tail. Unrecognised messages pass through.
func stripErrorPrefix(s string) string {
	for _, p := range []string{
		"METADATA_FAILED: ", "AUTH_ERROR: ", "HTTP_ERROR: ",
		"PLUGIN_ERROR: ", "INVALID_REQUEST: ",
		"PARSE_ERROR: ", "parse_error: ",
	} {
		if strings.HasPrefix(s, p) {
			return strings.TrimPrefix(s, p)
		}
	}
	return s
}

// wrapWords word-wraps `s` to lines of at most `width` visible cells.
// Splits on whitespace; words longer than `width` are placed on their
// own line and may exceed it (the caller is responsible for any final
// hard-truncate). Empty input returns a single empty line.
func wrapWords(s string, width int) []string {
	if width < 1 {
		return []string{s}
	}
	var lines []string
	var cur strings.Builder
	for _, w := range strings.Fields(s) {
		if cur.Len() == 0 {
			cur.WriteString(w)
			continue
		}
		if lipgloss.Width(cur.String())+1+lipgloss.Width(w) <= width {
			cur.WriteByte(' ')
			cur.WriteString(w)
		} else {
			lines = append(lines, cur.String())
			cur.Reset()
			cur.WriteString(w)
		}
	}
	if cur.Len() > 0 {
		lines = append(lines, cur.String())
	}
	if len(lines) == 0 {
		lines = []string{""}
	}
	return lines
}

// compactProviderError takes one entry from the streams-pipeline error
// list (e.g. `"jackett-provider: HTTP_ERROR: HTTP 0: error sending
// request for url (...)"`) and returns a short, human-readable form
// (e.g. `"jackett: network error"`).
//
// The runtime joins per-provider failures with `"; "`; the renderer
// splits on that and pipes each segment through this helper before
// truncating to the streams column width. Strips the `-provider`
// suffix on plugin names (cosmetic; redundant in this UI), then
// pattern-matches on the well-known SDK error-code prefixes
// (`AUTH_ERROR`, `HTTP_ERROR`, `METADATA_FAILED`) and the supervisor
// timeout string.
func compactProviderError(part string) string {
	idx := strings.Index(part, ": ")
	if idx == -1 {
		return part
	}
	name := strings.TrimSuffix(part[:idx], "-provider")
	rest := part[idx+2:]
	switch {
	case rest == "timed out" || strings.HasPrefix(rest, "plugin call timed out"):
		return name + ": timed out"
	case strings.HasPrefix(rest, "HTTP_ERROR: HTTP 0"):
		// HTTP 0 = client-side failure (DNS / connection refused / TLS).
		return name + ": network error"
	case strings.HasPrefix(rest, "HTTP_ERROR: HTTP "):
		after := strings.TrimPrefix(rest, "HTTP_ERROR: HTTP ")
		if i := strings.IndexAny(after, ":"); i > 0 {
			return name + ": HTTP " + after[:i]
		}
		return name + ": HTTP error"
	case strings.HasPrefix(rest, "PARSE_ERROR:") || strings.HasPrefix(rest, "parse_error:"):
		// Plugin received JSON in an unexpected shape — usually means
		// the upstream API changed format or returned an error object.
		// Don't try to render the serde-ese ("invalid type: map,
		// expected ...") in a tiny column.
		return name + ": bad response"
	}
	// Generic path for AUTH_ERROR / METADATA_FAILED / unknown formats:
	// drop the SDK code prefix (if any) and trim verbose remediation
	// hints after `" - "`.
	msg := stripErrorPrefix(rest)
	if i := strings.Index(msg, " - "); i > 0 {
		msg = msg[:i]
	}
	return name + ": " + msg
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

// humanizeSize formats a byte count as `"1.2 GB"` / `"640 MB"` /
// `"42 KB"` for the streams column. Decimal units (1000-based) since
// torrent sites display sizes in those.
func humanizeSize(b int64) string {
	const (
		gb = 1_000_000_000
		mb = 1_000_000
		kb = 1_000
	)
	switch {
	case b >= gb:
		return fmt.Sprintf("%.1f GB", float64(b)/float64(gb))
	case b >= mb:
		return fmt.Sprintf("%.0f MB", float64(b)/float64(mb))
	case b >= kb:
		return fmt.Sprintf("%.0f KB", float64(b)/float64(kb))
	default:
		return fmt.Sprintf("%d B", b)
	}
}

// ── Helpers ───────────────────────────────────────────────────────────────────
