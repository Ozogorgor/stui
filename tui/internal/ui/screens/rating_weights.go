package screens

// rating_weights.go — "Rating Weights" info screen (Stats for Nerds).
//
// Shows the per-profile weight ratios used by the weighted-median rating
// aggregator in the Rust runtime. Profiles are selected automatically based
// on the entry's MediaType and genre string.
//
//   q close

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/table"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ui/components"
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
	{"IMDB", "0–10", 0.30, 0.35, 0.20, 0.25, 0.30, 0.20},
	{"RT Audience", "0–100%", 0.15, 0.25, 0.15, 0.10, 0.30, 0.30},
	{"TMDB", "0–10", 0.10, 0.15, 0.15, 0.10, 0.10, 0.20},
	{"AniList", "0–100", 0.00, 0.00, 0.35, 0.00, 0.00, 0.00},
}

// profileHeaders are the column headings shown in the comparison table.
var profileHeaders = []string{"Movie", "Series", "Anime", "Doc", "Horror", "Music"}

// ── Screen ────────────────────────────────────────────────────────────────────

// RatingWeightsScreen is a static informational screen — no live data needed.
type RatingWeightsScreen struct {
	Dims
	table  *components.SortableTable
}

func NewRatingWeightsScreen() RatingWeightsScreen {
	columns := []table.Column{
		{Title: "Source", Width: 16},
		{Title: "Scale", Width: 7},
		{Title: "Movie", Width: 7},
		{Title: "Series", Width: 7},
		{Title: "Anime", Width: 7},
		{Title: "Doc", Width: 7},
		{Title: "Horror", Width: 7},
		{Title: "Music", Width: 7},
	}

	return RatingWeightsScreen{
		table: components.NewSortableTable(columns),
	}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m RatingWeightsScreen) Init() tea.Cmd { return nil }

func (m RatingWeightsScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.setWindowSize(msg)
	case tea.KeyPressMsg:
		switch msg.String() {
		case "q", "esc":
			return m, func() tea.Msg { return screen.PopMsg{} }
		}
	}
	return m, nil
}

func (m RatingWeightsScreen) View() tea.View {
	neon := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	bold := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)

	title := neon.Render("⚖  Rating Weights")
	sub := dim.Render("weighted-median · genre-aware")
	header := lipgloss.JoinHorizontal(lipgloss.Top, title, "   ", sub)

	rule := dim.Render("Profile selected by: ") +
		bold.Render("genre keyword") +
		dim.Render(" (anime, documentary, horror)") + "\n  " +
		strings.Repeat(" ", len("Profile selected by: ")) +
		bold.Render("MediaType") +
		dim.Render(" (series, music)  · else Movie default")

	rows := make([][]string, len(profileWeights))
	for i, p := range profileWeights {
		weights := []float64{p.movie, p.series, p.anime, p.doc, p.horror, p.music}
		row := []string{p.source, p.scale}
		for _, w := range weights {
			if w == 0.0 {
				row = append(row, "—")
			} else {
				row = append(row, fmt.Sprintf("%2.0f%%", w*100))
			}
		}
		rows[i] = row
	}
	m.table.SetData(rows)

	tableH := m.height - 18
	if tableH < 5 {
		tableH = 10
	}
	m.table.SetHeight(tableH)
	m.table.SetFocused(true)

	notes := strings.Join([]string{
		dim.Render("All scores normalised to 0–10 before weighting."),
		dim.Render("Weights re-normalised to 1.0 when sources are missing."),
		dim.Render("OMDB excluded — mirrors IMDB score (would double-count)."),
		dim.Render("Anime detected by genre keyword OR presence of an AniList score."),
	}, "\n  ")

	footer := "\n\n" + hintBar("q close")

	return tea.NewView("  " + header +
		"\n\n  " + rule +
		"\n\n  " + dim.Render(m.table.View()) +
		"\n\n  " + notes +
		footer)
}
