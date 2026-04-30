package screens

// album_detail.go — AlbumDetailScreen: album track browser.
//
// Layout mirrors detail.go's movie/series overlay so the visual
// identity stays consistent across the app:
//
//   ┌──────────────────────────────────────────────────────────────┐
//   │ ← esc  Music  ›  Album Title                                │
//   ├────────────┬────────────────────────────────────────────────┤
//   │            │  ALBUM TITLE                          ★ 8.5   │█
//   │  [POSTER]  │  artist · 2024 · genre                        │█
//   │            │                                                │█
//   │            │  TRACKS                                        │░
//   │            │  ▸  1.  Track One                       3:42  │░
//   │            │     2.  Track Two                       4:15  │░
//   │            │  …                                             │░
//   └────────────┴────────────────────────────────────────────────┘
//
// The right panel runs through the same scroll-with-scrollbar pipeline
// detail.go uses — see renderInfoBlock — so cursor movement translates
// directly into scroll position and the track list scrollbar matches
// detail.go's cast list scrollbar pixel-for-pixel.

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/components/mediaheader"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// Header above the track-list inside the right info column. The track
// rows that follow are indexed against this constant when computing the
// scroll position, so adding/removing meta lines means updating this.
const albumDetailMetaLines = 4 // title + meta + spacer + "TRACKS" header

type AlbumDetailScreen struct {
	Dims
	client   *ipc.Client
	title    string
	artist   string
	year     string
	genre    string
	rating   string
	coverURL string
	tracks   []ipc.AlbumTrack
	cursor   int
	loading  bool
}

func NewAlbumDetailScreen(client *ipc.Client, title, artist, year, genre, rating, coverURL string) AlbumDetailScreen {
	return AlbumDetailScreen{
		client:   client,
		title:    title,
		artist:   artist,
		year:     year,
		genre:    genre,
		rating:   rating,
		coverURL: coverURL,
		tracks:   []ipc.AlbumTrack{},
		cursor:   0,
		loading:  true,
	}
}

func (s AlbumDetailScreen) Init() tea.Cmd {
	// Fire the lastfm album.getInfo lookup as soon as the screen
	// opens. The runtime hits last.fm directly (the WASM plugin
	// can't surface tracks through the current SDK shape — see
	// runtime/src/lastfm/album_tracks.rs); the result lands as a
	// LastFMAlbumTracksMsg on the IPC channel and Update flips
	// loading=false + populates s.tracks.
	//
	// Without (client, artist, title) the lookup can't run — the
	// screen synthesises a self-message that flips loading=false
	// immediately so the View shows "No tracks found" instead of
	// sitting at the spinner forever.
	if s.client == nil || s.artist == "" || s.title == "" {
		artist, album := s.artist, s.title
		return func() tea.Msg {
			return ipc.LastFMAlbumTracksMsg{Artist: artist, Album: album}
		}
	}
	artist, album := s.artist, s.title
	client := s.client
	return func() tea.Msg {
		client.LastfmAlbumGetTracks(artist, album)
		return nil
	}
}

func (s AlbumDetailScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {
	case tea.WindowSizeMsg:
		// Without this, the embedded Dims's width/height stay at 0
		// and View()'s `if s.width < 10` early-returns an empty
		// string — producing the "blank screen on open" bug.
		// `setWindowSize` is on *Dims; Go auto-takes &s here.
		s.setWindowSize(m)
		return s, nil
	case tea.KeyPressMsg:
		switch m.String() {
		case "esc", "q":
			return s, screen.PopCmd()
		case "j", "down":
			if s.cursor < len(s.tracks)-1 {
				s.cursor++
			}
		case "k", "up":
			if s.cursor > 0 {
				s.cursor--
			}
		case "enter":
			return s, s.playSelected()
		}
	case ipc.LastFMAlbumTracksMsg:
		s.loading = false
		s.tracks = m.Tracks
	}
	return s, nil
}

func (s *AlbumDetailScreen) playSelected() tea.Cmd {
	if s.cursor >= 0 && s.cursor < len(s.tracks) && s.client != nil {
		track := s.tracks[s.cursor]
		s.client.MpdCmd("mpd_add", map[string]any{
			"uri": fmt.Sprintf("lastfm://%s/%s/%s", s.artist, s.title, track.Title),
		})
	}
	return nil
}

// ── View ──────────────────────────────────────────────────────────────────────

func (s AlbumDetailScreen) View() tea.View {
	if s.width < 10 || s.height < 4 {
		return tea.NewView("")
	}

	// MainCardStyle adds margin(1+1) + border(1+1) + padding(1+1) = 6
	// chars total horizontal chrome. Width(s.width-2) sizes the
	// bordered+padded box; the margin sits outside it. Inside that, the
	// available content area is (s.width-2) - 2*padding = s.width-4.
	innerW := s.width - 4
	if innerW < 20 {
		innerW = 20
	}

	// Vertical chrome: top + bottom border = 2 rows. No vertical
	// padding/margin on MainCardStyle, so subtract 2 for the visible
	// content area.
	innerH := s.height - 2
	if innerH < 1 {
		innerH = 1
	}

	header := s.renderHeader(innerW)
	headerH := lipgloss.Height(header)

	// Body: poster on the left, info+tracks on the right. The blank
	// row between header and body matches detail.go's vertical rhythm.
	bodyH := innerH - headerH - 1
	if bodyH < 1 {
		bodyH = 1
	}

	leftPanel := s.renderPosterBlock(mediaheader.PosterWidth, bodyH)
	rightW := innerW - mediaheader.PosterWidth - 4
	if rightW < 20 {
		rightW = 20
	}
	rightPanel := s.renderInfoBlock(rightW, bodyH)

	body := lipgloss.JoinHorizontal(lipgloss.Top, leftPanel, rightPanel)

	full := lipgloss.JoinVertical(lipgloss.Left, header, "", body)

	framed := theme.T.MainCardStyle(true).
		Width(s.width - 2).
		Height(s.height - 2).
		Render(full)
	return tea.NewView(framed)
}

// renderHeader renders the breadcrumb row at the top. Mirrors the
// movie detail's header line that lives in the global status bar — but
// since AlbumDetailScreen is its own active screen (not an overlay
// running alongside the legacy Model), it has to render its own.
func (s AlbumDetailScreen) renderHeader(w int) string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent())

	backHint := dim.Render("← esc")
	crumb := dim.Render("  Music  ›  ") + acc.Render(s.title)
	return lipgloss.NewStyle().Width(w).Render(backHint + crumb)
}

// renderPosterBlock renders the album cover on the left side of the
// body. Same poster sizing convention as detail.go: outer block width
// = mediaheader.PosterWidth (22), inner poster width = w-4 to leave
// room for the Padding(2, 2).
func (s AlbumDetailScreen) renderPosterBlock(w, h int) string {
	posterW := w - 4
	poster := mediaheader.RenderPoster(mediaheader.Inputs{
		Title:     s.title,
		Genre:     s.genre,
		PosterURL: s.coverURL,
	}, posterW, mediaheader.PosterHeight)

	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Width(w).
		Height(h).
		Padding(2, 2).
		Render(poster)
}

// renderInfoBlock renders the right panel: title + rating, meta
// (artist · year · genre), and the track list. Same scroll-with-
// scrollbar pipeline as detail.go's renderInfoBlock so the chrome
// matches pixel-for-pixel.
func (s AlbumDetailScreen) renderInfoBlock(w, h int) string {
	var sections []string

	// Title + rating on the same line.
	titleW := w - 10
	if titleW < 10 {
		titleW = 10
	}
	titleText := s.title
	if lipgloss.Width(titleText) > titleW {
		titleText = truncateRune(titleText, titleW)
	}
	titleStr := theme.T.DetailTitleStyle().Width(titleW).Render(titleText)
	var ratingStr string
	if s.rating != "" {
		ratingStr = theme.T.DetailRatingStyle().Render("★ " + s.rating)
	}
	sections = append(sections, lipgloss.JoinHorizontal(lipgloss.Top, titleStr, ratingStr))

	// Meta: artist · year · genre.
	metaParts := []string{}
	if s.artist != "" {
		metaParts = append(metaParts, s.artist)
	}
	if s.year != "" {
		metaParts = append(metaParts, s.year)
	}
	if s.genre != "" {
		metaParts = append(metaParts, s.genre)
	}
	if len(metaParts) > 0 {
		sections = append(sections, theme.T.DetailMetaStyle().Render(strings.Join(metaParts, "  ·  ")))
	} else {
		sections = append(sections, "")
	}
	sections = append(sections, "")

	// Track list — see albumDetailMetaLines comment above; if you
	// change the number of pre-track lines here, update that constant.
	sections = append(sections, theme.T.DetailSectionStyle().Render("TRACKS"))

	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	switch {
	case s.loading:
		sections = append(sections, dim.Render("  Loading tracks…"))
	case len(s.tracks) == 0:
		sections = append(sections, dim.Render("  No tracks found"))
	default:
		for i, t := range s.tracks {
			sections = append(sections, s.renderTrackRow(i, t, w-4))
		}
	}

	// Build content lines, then carve a visible window of height h-2
	// around the cursor (same approach as detail.go renderInfoBlock).
	content := strings.Join(sections, "\n")
	lines := strings.Split(content, "\n")

	// Reserve 1 col gap + 1 col scrollbar on the right (same as
	// detail.go), so each rendered row is contentW chars wide.
	contentW := w - 4
	if contentW < 1 {
		contentW = 1
	}

	visibleH := h - 2
	if visibleH < 1 {
		visibleH = 1
	}

	maxScroll := len(lines) - visibleH
	if maxScroll < 0 {
		maxScroll = 0
	}

	// Follow the cursor: scroll just enough to keep the active track
	// row in view. cursorLine is the absolute row index of the active
	// track in `lines`.
	cursorLine := albumDetailMetaLines + s.cursor
	scroll := 0
	if cursorLine >= visibleH {
		scroll = cursorLine - visibleH + 1
	}
	if scroll > maxScroll {
		scroll = maxScroll
	}

	contentLineStyle := lipgloss.NewStyle().Width(contentW).MaxWidth(contentW)
	rows := make([]string, 0, visibleH)
	for r := 0; r < visibleH; r++ {
		idx := scroll + r
		var lineText string
		if idx < len(lines) {
			lineText = lines[idx]
		}
		rows = append(rows, contentLineStyle.Render(lineText))
	}

	body := lipgloss.JoinHorizontal(lipgloss.Top,
		strings.Join(rows, "\n"),
		" ",
		components.Scrollbar(scroll, visibleH, len(lines)),
	)
	return lipgloss.NewStyle().
		Background(theme.T.Bg()).
		Padding(1, 0, 1, 2).
		Width(w).
		Height(h).
		Render(body)
}

func (s AlbumDetailScreen) renderTrackRow(i int, t ipc.AlbumTrack, w int) string {
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())

	var cursor string
	var titleStyle lipgloss.Style
	if i == s.cursor {
		cursor = acc.Render("▸ ")
		titleStyle = acc
	} else {
		cursor = "  "
		titleStyle = normal
	}

	numStr := dim.Render(fmt.Sprintf("%2d.", t.Number))
	durStr := ""
	if t.Duration != "" {
		durStr = "  " + dim.Render(t.Duration)
	}
	durW := lipgloss.Width(durStr)

	// 2 (cursor) + 3 (num) + 1 (gap) + duration + 1 trailing slack
	titleW := w - 2 - 3 - 1 - durW - 1
	if titleW < 5 {
		titleW = 5
	}
	title := t.Title
	if lipgloss.Width(title) > titleW {
		title = truncateRune(title, titleW)
	}

	return cursor + numStr + " " + titleStyle.Render(title) + durStr
}

// truncateRune trims `s` to at most `w` cells, replacing the trailing
// rune with "…" when truncation occurred. Rune-aware so it doesn't cut
// multi-byte characters in half.
func truncateRune(s string, w int) string {
	if w <= 1 {
		return "…"
	}
	rr := []rune(s)
	if len(rr) <= w {
		return s
	}
	return string(rr[:w-1]) + "…"
}
