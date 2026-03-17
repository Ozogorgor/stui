package screens

// rating_weights.go — "Rating Weights" info screen (Stats for Nerds).
//
// Shows the per-profile weight ratios used by the weighted-median rating
// aggregator in the Rust runtime. Profiles are selected automatically based
// on the entry's MediaType and genre string.
//
// Layout:
//
//   ⚖  Rating Weights               weighted-median · genre-aware
//
//   Profile selected by:  genre keyword (anime, documentary, horror)
//                         or MediaType (series, music)  · else Movie default
//
//   Source            Movie  Series  Anime  Doc    Horror  Music
//   ──────────────────────────────────────────────────────────────
//   Rotten Tomatoes    35%    25%    15%    45%    25%    20%
//   IMDB               30%    35%    20%    25%    30%    20%
//   RT Audience        15%    25%    15%    10%    30%    30%
//   TMDB               10%    15%    15%    10%    10%    20%
//   AniList             —      —     35%     —      —      —
//
//   All scores normalised to 0–10 before weighting.
//   Weights re-normalised to 1.0 when sources are missing.
//   OMDB excluded — mirrors IMDB score (would double-count).
//   Anime detected by genre keyword OR presence of an AniList score.
//
//   q close

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// OpenRatingWeightsMsg is emitted by the settings screen to open this view.
type OpenRatingWeightsMsg struct{}

// weightProfile holds the weight of one source across all profiles.
// 0 means the source is not used for that profile (displayed as "—").
type weightProfile struct {
	source string
	scale  string
	movie  float64
	series float64
	anime  float64
	doc    float64
	horror float64
	music  float64
}

// profileWeights mirrors the weight tables in
// runtime/src/catalog_engine/aggregator.rs — update both if weights change.
var profileWeights = []weightProfile{
	{"Rotten Tomatoes", "0–100%", 0.35, 0.25, 0.15, 0.45, 0.25, 0.20},
	{"IMDB",            "0–10",   0.30, 0.35, 0.20, 0.25, 0.30, 0.20},
	{"RT Audience",     "0–100%", 0.15, 0.25, 0.15, 0.10, 0.30, 0.30},
	{"TMDB",            "0–10",   0.10, 0.15, 0.15, 0.10, 0.10, 0.20},
	{"AniList",         "0–100",  0.00, 0.00, 0.35, 0.00, 0.00, 0.00},
}

// profileHeaders are the column headings shown in the comparison table.
var profileHeaders = []string{"Movie", "Series", "Anime", "Doc", "Horror", "Music"}

// ── Screen ────────────────────────────────────────────────────────────────────

// RatingWeightsScreen is a static informational screen — no live data needed.
type RatingWeightsScreen struct {
	width  int
	height int
}

func NewRatingWeightsScreen() RatingWeightsScreen {
	return RatingWeightsScreen{}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m RatingWeightsScreen) Init() tea.Cmd { return nil }

func (m RatingWeightsScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
	case tea.KeyMsg:
		switch msg.String() {
		case "q", "esc":
			return m, func() tea.Msg { return screen.PopMsg{} }
		}
	}
	return m, nil
}

func (m RatingWeightsScreen) View() string {
	neon := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim  := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	bold := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	gold := lipgloss.NewStyle().Foreground(lipgloss.Color("#FFD700"))

	// ── Header ──────────────────────────────────────────────────────────
	title  := neon.Render("⚖  Rating Weights")
	sub    := dim.Render("weighted-median · genre-aware")
	header := lipgloss.JoinHorizontal(lipgloss.Top, title, "   ", sub)

	// ── Selection rule ───────────────────────────────────────────────────
	rule := dim.Render("Profile selected by: ") +
		bold.Render("genre keyword") +
		dim.Render(" (anime, documentary, horror)") + "\n  " +
		strings.Repeat(" ", len("Profile selected by: ")) +
		bold.Render("MediaType") +
		dim.Render(" (series, music)  · else Movie default")

	// ── Comparison table ─────────────────────────────────────────────────
	const (
		colSource = 16
		colScale  = 7
		colWeight = 7  // "  35%  " or "   —   "
	)

	// Header row
	hdrCells := []string{
		fmt.Sprintf("%-*s", colSource, bold.Render("Source")),
		fmt.Sprintf("%-*s", colScale,  bold.Render("Scale")),
	}
	for _, h := range profileHeaders {
		hdrCells = append(hdrCells, fmt.Sprintf("%*s", colWeight, bold.Render(h)))
	}
	hdrRow := strings.Join(hdrCells, "  ")

	divLen := colSource + colScale + (colWeight+2)*len(profileHeaders) + 2
	divider := dim.Render(strings.Repeat("─", divLen))

	// Data rows
	getWeights := func(p weightProfile) []float64 {
		return []float64{p.movie, p.series, p.anime, p.doc, p.horror, p.music}
	}

	var rows []string
	for _, p := range profileWeights {
		cells := []string{
			fmt.Sprintf("%-*s", colSource, p.source),
			fmt.Sprintf("%-*s", colScale,  dim.Render(p.scale)),
		}
		for _, w := range getWeights(p) {
			var cell string
			if w == 0.0 {
				cell = fmt.Sprintf("%*s", colWeight, dim.Render("—"))
			} else {
				cell = fmt.Sprintf("%*s", colWeight, gold.Render(fmt.Sprintf("%2.0f%%", w*100)))
			}
			cells = append(cells, cell)
		}
		rows = append(rows, strings.Join(cells, "  "))
	}

	table := strings.Join(append([]string{hdrRow, divider}, rows...), "\n  ")

	// ── Footer notes ─────────────────────────────────────────────────────
	notes := strings.Join([]string{
		dim.Render("All scores normalised to 0–10 before weighting."),
		dim.Render("Weights re-normalised to 1.0 when sources are missing."),
		dim.Render("OMDB excluded — mirrors IMDB score (would double-count)."),
		dim.Render("Anime detected by genre keyword OR presence of an AniList score."),
	}, "\n  ")

	footer := "\n\n" + hintBar("q close")

	return "  " + header +
		"\n\n  " + rule +
		"\n\n  " + table +
		"\n\n  " + notes +
		footer
}
