package screens

// episode.go — EpisodeScreen: season/episode browser.
//
// Layout: seasons left, episode rows right (list view only).
// A grid view existed previously and may be reintroduced; the
// scaffolding was removed to keep the file focused.
//
// 'b' toggles binge mode — BingeContextMsg is fired on play so Model can
// auto-queue the next episode when playback ends.

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/components/mediaheader"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// EpisodeScreen is the season/episode tree browser.
// To open: screen.TransitionCmd(NewEpisodeScreen(...), true)
type EpisodeScreen struct {
	Dims
	client       *ipc.Client
	title        string
	seriesID     string
	idSource     string // "tmdb" / "tvdb" / "anilist" / etc — empty = let runtime guess
	year         string // free-form year/year-range — purely for the header meta line
	genre        string // first-genre snippet — feeds the poster placeholder + meta line
	rating       string // pre-formatted star rating, e.g. "8.7"
	posterURL    string // remote poster URL (mediaheader handles cache + chafa)
	posterArt    string // pre-rendered block art (rare); takes precedence over URL
	backdropURL  string // optional; rendered under the season list when cached
	// seasonIDs (parallel to seasons): provider-native id for each
	// season slot. Empty means "use seriesID for every season" (TMDB
	// style); non-empty means "each season has its own native id"
	// (AniList chain style — LoadEpisodes uses seasonIDs[N-1] + season=1).
	seasonIDs    []string
	seasons      []int // available season numbers
	seasonCursor int
	episodes     []episodeItem // episodes for the selected season
	epCursor     int
	inEpisodes   bool // false = navigating seasons, true = navigating episodes
	loading      bool
	everLoaded   bool   // true once the first EpisodesLoadedMsg has rendered the layout.
	loadErr      string // last load failure (empty = no error). Rendered in place of the spinner.
	bingeEnabled bool   // true = auto-play next episode on end-of-file
	spinner      components.Spinner
}

// episodeItem is aliased from ipc.EpisodeEntry
type episodeItem = ipc.EpisodeEntry

// EpisodeScreenOpts carries the optional artwork/meta fields the header
// renders. The episode browser is always opened from a detail context
// that already has these values, so callers pass them through instead
// of re-fetching metadata. Zero-valued fields fall back to the existing
// title-only header without poster/meta.
type EpisodeScreenOpts struct {
	Year        string
	Genre       string
	Rating      string
	PosterURL   string
	PosterArt   string
	BackdropURL string // optional; rendered below the season list when cached
	// Seasons is the explicit list of season numbers to render in the
	// left-hand column. Pass `nil` (or empty) to fall back to a single
	// season — the safe default for providers that don't expose a count.
	Seasons []int
	// SeasonIDs is parallel to Seasons. When non-empty, season N uses
	// SeasonIDs[N-1] as its provider-native id and `season=1` in
	// LoadEpisodes (per-cour providers like AniList where each season
	// is its own catalog entry). Empty leaves LoadEpisodes using the
	// anchor SeriesID and the user-selected season number (TMDB style).
	SeasonIDs []string
}

func NewEpisodeScreen(client *ipc.Client, title, seriesID, idSource string, autoplayDefault bool, opts EpisodeScreenOpts) EpisodeScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return EpisodeScreen{
		client:       client,
		title:        title,
		seriesID:     seriesID,
		idSource:     idSource,
		year:         opts.Year,
		genre:        opts.Genre,
		rating:       opts.Rating,
		posterURL:    opts.PosterURL,
		posterArt:    opts.PosterArt,
		backdropURL:  opts.BackdropURL,
		seasonIDs:    append([]string(nil), opts.SeasonIDs...),
		loading: true,
		seasons:      seasonsOrDefault(opts.Seasons),
		bingeEnabled: autoplayDefault,
		spinner:      *components.NewSpinner("loading episodes…", dimStyle),
	}
}

func (s EpisodeScreen) Init() tea.Cmd {
	s.spinner.Start()
	if s.client != nil && s.seriesID != "" && len(s.seasons) > 0 {
		id, season := s.episodeRequestFor(0)
		s.client.LoadEpisodes(id, s.idSource, season)
	}
	// Return the spinner's tick cmd so the dancer keeps animating —
	// previous nil return left the spinner frozen on its first frame.
	return s.spinner.Init()
}

// episodeRequestFor returns the (series_id, season_number) pair to send
// to LoadEpisodes for the season at index `idx` in s.seasons. When
// s.seasonIDs is populated (AniList-style: each season is its own
// catalog entry), the per-season id is used and the season number is
// pinned to 1; otherwise the anchor seriesID is used and the user-
// selected season number is forwarded as-is (TMDB style).
func (s EpisodeScreen) episodeRequestFor(idx int) (string, int) {
	if idx >= 0 && idx < len(s.seasonIDs) && s.seasonIDs[idx] != "" {
		return s.seasonIDs[idx], 1
	}
	return s.seriesID, s.seasons[idx]
}

func (s EpisodeScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {

	case spinner.TickMsg:
		_, cmd := s.spinner.Update(m)
		return s, cmd

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case ipc.EpisodesLoadedMsg:
		if m.SeriesID == s.seriesID {
			s.episodes = m.Episodes
			s.epCursor = 0
			s.loading = false
			s.everLoaded = true
			s.loadErr = ""
			s.spinner.Stop()
		}

	case ipc.EpisodesLoadFailedMsg:
		if m.SeriesID == s.seriesID {
			s.loading = false
			s.loadErr = m.Reason
			s.spinner.Stop()
		}

	case tea.MouseMsg:
		// Mouse wheel scrolls the episode list.
		if s.inEpisodes {
			mouse := m.Mouse()
			if mouse.Button == tea.MouseWheelUp {
				if s.epCursor > 0 {
					s.epCursor--
				}
			} else if mouse.Button == tea.MouseWheelDown {
				if s.epCursor < len(s.episodes)-1 {
					s.epCursor++
				}
			}
		}

	case tea.KeyPressMsg:
		key := m.String()

		// ── Mode toggles (checked first so they always fire) ──────────────
		if key == "b" {
			s.bingeEnabled = !s.bingeEnabled
			return s, nil
		}

		if action, ok := actions.FromKey(key); ok {
			switch action {

			case actions.ActionNavigateDown:
				if !s.inEpisodes {
					s.loadSeason(s.seasonCursor + 1)
				} else if s.epCursor < len(s.episodes)-1 {
					s.epCursor++
				}

			case actions.ActionNavigateUp:
				if !s.inEpisodes {
					s.loadSeason(s.seasonCursor - 1)
				} else if s.epCursor > 0 {
					s.epCursor--
				}

			case actions.ActionNavigateRight:
				if !s.inEpisodes {
					s.inEpisodes = true
				}
				// In list mode right does nothing extra (enter plays)

			case actions.ActionNavigateLeft:
				if s.inEpisodes {
					s.inEpisodes = false
				}

			case actions.ActionBack:
				if s.inEpisodes {
					s.inEpisodes = false
				} else {
					return s, func() tea.Msg { return screen.PopMsg{} }
				}

			case actions.ActionSelect:
				return s, s.playSelected()
			}
		}

		// Enter also plays in both modes
		if key == "enter" && s.inEpisodes {
			return s, s.playSelected()
		}
	}
	return s, nil
}

// loadSeason switches to season at index idx (bounds-checked).
func (s *EpisodeScreen) loadSeason(idx int) {
	if idx < 0 || idx >= len(s.seasons) {
		return
	}
	s.seasonCursor = idx
	s.epCursor = 0
	s.loading = true
	s.episodes = nil
	if s.client != nil {
		id, season := s.episodeRequestFor(s.seasonCursor)
	s.client.LoadEpisodes(id, s.idSource, season)
	}
}

// playSelected returns the Cmd to play the episode under epCursor.
func (s EpisodeScreen) playSelected() tea.Cmd {
	if len(s.episodes) == 0 || s.client == nil {
		return nil
	}
	ep := s.episodes[s.epCursor]
	s.client.Play(ep.EntryID, ep.Provider, "", ipc.TabSeries)
	ctx := ipc.BingeContextMsg{
		SeriesTitle:  s.title,
		SeriesID:     s.seriesID,
		Tab:          ipc.TabSeries,
		Episodes:     append([]ipc.EpisodeEntry(nil), s.episodes...),
		CurrentIdx:   s.epCursor,
		BingeEnabled: s.bingeEnabled,
	}
	return tea.Batch(
		func() tea.Msg { return screen.PopMsg{} },
		func() tea.Msg { return ctx },
	)
}

func seasonsOrDefault(in []int) []int {
	if len(in) == 0 {
		return []int{1}
	}
	out := make([]int, len(in))
	copy(out, in)
	return out
}

// verticalSlack is a defensive margin against off-by-one in lipgloss
// height accounting. Set to 1 by default; drop to 0 if smoke-testing
// reveals a row of dead space below the footer, raise to 2 if the
// bottom border clips.
const verticalSlack = 1

// computeEpisodeViewport carves chrome out of `screenH` and returns
// the visible [start, end) range plus the panel height available for
// the episode panel. extraReserve subtracts trailing rows the caller
// reserves for non-list content (grid mode reserves 2 for the info
// strap; list mode passes 0).
//
// The chrome budget mirrors View(): poster header + 2 blank rows + 1
// footer line + 2 rows of MainCardStyle border + verticalSlack.
// MainCardStyle has no vertical padding, so the slack is purely
// defensive.
func computeEpisodeViewport(total, cursor, screenH, extraReserve int) (start, end, panelH int) {
	chrome := mediaheader.PosterHeight + 1 + 1 + 1 + 2 + verticalSlack
	bodyH := screenH - chrome
	if bodyH < 1 {
		bodyH = 1
	}
	panelH = bodyH - extraReserve
	if panelH < 1 {
		panelH = 1
	}
	if total == 0 {
		return 0, 0, panelH
	}
	vl := components.NewVirtualizedList(total, cursor, panelH,
		components.WithScrollMode(components.ScrollModePush))
	start, end = vl.VisibleRange()
	return start, end, panelH
}

// humanizeLoadErr trims the raw runtime/plugin error string into
// something readable. Runtime errors arrive as
// "METADATA_FAILED: unknown_id: TMDB HTTP 404: {…json…}" — fine for
// the runtime log, ugly on screen. We keep the leading code phrase
// (so users can search for it) and drop everything from the first
// `{` onward.
func humanizeLoadErr(raw string) string {
	if i := strings.IndexByte(raw, '{'); i > 0 {
		raw = strings.TrimRight(raw[:i], " :")
	}
	// Common case: TMDB returns 404 when a season doesn't exist. Surface
	// that in plain language instead of the HTTP status leak.
	if strings.Contains(raw, "HTTP 404") || strings.Contains(raw, "unknown_id") {
		return "this season doesn't exist for this series"
	}
	return raw
}

// ── View ──────────────────────────────────────────────────────────────────────

func (s EpisodeScreen) View() tea.View {
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())

	header := s.renderHeader(acc, dim)

	var body string
	switch {
	case s.loadErr != "":
		errStyle := lipgloss.NewStyle().Foreground(theme.T.Red())
		body = "  " + errStyle.Render("Couldn't load episodes") + "\n" +
			"  " + dim.Render(humanizeLoadErr(s.loadErr))
	case s.loading && !s.everLoaded:
		// Initial load — show the spinner inside the framed body so the
		// chrome stays put. Subsequent season-switches reuse the layout
		// below and surface the spinner inside the right panel only.
		body = "  " + s.spinner.View()
	default:
		// Season column matches the poster width above so the chrome
		// aligns vertically and there's room to drop a small backdrop
		// underneath the season list.
		seasonW := mediaheader.PosterWidth
		_, _, bodyH := computeEpisodeViewport(0, 0, s.height, 0)
		leftPanel := s.renderSeasonColumn(acc, dim, seasonW, bodyH)
		rightPanel := s.renderListPanel(acc, dim, seasonW)
		body = lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, "  ", rightPanel)
	}

	// Footer hints
	var bingeHint string
	if s.bingeEnabled {
		bingeHint = neon.Render("b  binge ON")
	} else {
		bingeHint = dim.Render("b  binge off")
	}
	navHint := hintBar("←→↑↓ navigate", "enter play", "esc back")
	footer := navHint + "   " + bingeHint
	_ = neon

	content := lipgloss.JoinVertical(lipgloss.Left,
		header,
		"",
		body,
		"",
		footer,
	)

	// Wrap in MainCardStyle so the chrome (rounded border, accent edge,
	// side margins) matches the grid/detail screens. Width is bounded by
	// the cached terminal width minus MainCardStyle's own margin+border
	// budget — same calculation Model.View uses for the detail overlay.
	if s.width > 4 && s.height > 4 {
		framed := theme.T.MainCardStyle(true).
			Width(s.width - 2).
			Height(s.height - 2).
			Render(content)
		return tea.NewView(framed)
	}
	return tea.NewView(content)
}

// renderHeader composes the poster + title block at the top of the
// screen. Mirrors the layout RenderDetailOverlay uses so the two screens
// share the same visual identity.
func (s EpisodeScreen) renderHeader(acc, dim lipgloss.Style) string {
	posterW := mediaheader.PosterWidth - 4
	poster := mediaheader.RenderPoster(mediaheader.Inputs{
		Title:     s.title,
		Genre:     s.genre,
		PosterArt: s.posterArt,
		PosterURL: s.posterURL,
	}, posterW, mediaheader.PosterHeight)

	posterCol := lipgloss.NewStyle().
		Width(mediaheader.PosterWidth).
		Height(mediaheader.PosterHeight).
		Padding(0, 2).
		Render(poster)

	// Title + meta on the right of the poster.
	titleW := s.width - mediaheader.PosterWidth - 6
	if titleW < 20 {
		titleW = 20
	}
	titleLine := acc.Width(titleW).Render("\U0001f4fa " + s.title)

	metaParts := []string{}
	if s.year != "" {
		metaParts = append(metaParts, s.year)
	}
	if s.genre != "" {
		metaParts = append(metaParts, s.genre)
	}
	if s.rating != "" {
		metaParts = append(metaParts, "★ "+s.rating)
	}
	meta := dim.Render(strings.Join(metaParts, "  ·  "))

	titleBlock := lipgloss.JoinVertical(lipgloss.Left,
		titleLine,
		"",
		meta,
	)
	titleCol := lipgloss.NewStyle().
		Width(titleW).
		Padding(1, 0).
		Render(titleBlock)

	return lipgloss.JoinHorizontal(lipgloss.Top, posterCol, titleCol)
}

// renderSeasonColumn returns the season list with an optional backdrop
// chafa-rendered below it. Sized to match the poster column above so
// the chrome aligns vertically.
func (s EpisodeScreen) renderSeasonColumn(acc, dim lipgloss.Style, w, bodyH int) string {
	seasons := s.renderSeasonList(acc, dim, w)

	const backdropHeight = mediaheader.PosterHeight / 2
	backdrop := mediaheader.RenderBackdrop(s.backdropURL, w-2, backdropHeight)
	var content string
	if backdrop == "" {
		content = seasons
	} else {
		backdropBlock := lipgloss.NewStyle().
			Width(w).
			Padding(0, 1).
			Render(backdrop)
		content = lipgloss.JoinVertical(lipgloss.Left, seasons, "", backdropBlock)
	}

	if bodyH < 1 {
		return content
	}
	return lipgloss.NewStyle().Height(bodyH).Render(content)
}

func (s EpisodeScreen) renderSeasonList(acc, dim lipgloss.Style, w int) string {
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())
	var lines []string
	for i, sn := range s.seasons {
		cursor := "  "
		var style lipgloss.Style
		switch {
		case i == s.seasonCursor && !s.inEpisodes:
			cursor = "▶ "
			style = acc
		case i == s.seasonCursor:
			cursor = "▶ "
			style = normal
		default:
			style = dim
		}
		lines = append(lines, style.Render(fmt.Sprintf("%sSeason %d", cursor, sn)))
	}
	return lipgloss.NewStyle().Width(w).Render(strings.Join(lines, "\n"))
}

func (s EpisodeScreen) renderListPanel(acc, dim lipgloss.Style, seasonW int) string {
	if s.loading && len(s.episodes) == 0 {
		return s.spinner.View()
	}
	if len(s.episodes) == 0 {
		return ""
	}

	total := len(s.episodes)
	start, end, panelH := computeEpisodeViewport(total, s.epCursor, s.height, 0)

	// -8 preserved from prior code (gutter + season border + padding);
	// -2 reserves the gap (1 col) + scrollbar (1 col) on the right.
	epW := s.width - seasonW - 8 - 2
	if epW < 20 {
		epW = 20
	}
	contentW := epW

	normal := lipgloss.NewStyle().Foreground(theme.T.Text())

	rows := make([]string, 0, panelH)
	for r := 0; r < panelH; r++ {
		idx := start + r
		var line string
		if idx < end {
			ep := s.episodes[idx]
			cursor := "  "
			var style lipgloss.Style
			if idx == s.epCursor && s.inEpisodes {
				cursor = "▶ "
				style = acc
			} else {
				style = normal
			}
			epNum := fmt.Sprintf("E%02d", ep.Episode)
			title := ep.Title
			maxT := contentW - 10
			if maxT > 0 && len(title) > maxT {
				title = title[:maxT-1] + "\u2026"
			}
			raw := cursor + dim.Render(epNum) + "  " + style.Render(title)
			if ep.AirDate != "" {
				raw += "  " + dim.Render(ep.AirDate[:min(len(ep.AirDate), 10)])
			}
			line = raw
		}
		rows = append(rows, padRightANSI(line, contentW))
	}
	content := strings.Join(rows, "\n")
	return lipgloss.JoinHorizontal(lipgloss.Top,
		content, " ", components.Scrollbar(start, panelH, total),
	)
}

