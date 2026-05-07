package screens

// rating_weights.go — Editable per-source weight overlay.
//
// The catalog aggregator's static per-tab profiles
// (movie/series/anime/etc.) are read-only — they live in
// runtime/src/catalog_engine/aggregator.rs and define which sources
// participate at all. THIS screen edits the user-overlay map that
// rides on top of those profiles: every source listed here gets a
// numeric weight (typical 0.0–2.0) that overrides the static
// default and pulls third-party plugin sources (future user-
// authored installs) into the composite.
//
// Flow:
//   - On open, read the current overlay from the TUI config
//     (`cfg.Providers.RatingSourceWeights`), backed by the runtime
//     config's `rating_weights` table.
//   - Up/Down navigates rows, h/Left/-  decrements by 0.1,
//     l/Right/+ increments by 0.1, 0 disables a source (weight=0),
//     1 resets to 1.0.
//   - Every adjustment emits SettingsChangedMsg{Key: "rating_weights",
//     Value: <updated map>}, which the root model forwards to the
//     runtime via SetConfig (apply_key in
//     runtime/src/config/manager.rs replaces the map atomically and
//     pushes to the aggregator overlay so the next enrichment pass
//     sees the new weights without restart).
//
//   esc/q  close

import (
	"fmt"
	"sort"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// OpenRatingWeightsMsg is emitted by the settings screen to open this view.
type OpenRatingWeightsMsg struct{}

// canonicalSources lists the rating sources stui knows about by
// default — these always appear in the editor even if the user
// hasn't tweaked them. Third-party plugin sources show up
// automatically when the user has set a weight for them in their
// config (see `Refresh()`).
var canonicalSources = []sourceMeta{
	{key: "imdb", label: "IMDb", scale: "0–10"},
	{key: "tomatometer", label: "Rotten Tomatoes", scale: "0–100%"},
	{key: "audience_score", label: "RT Audience", scale: "0–100%"},
	{key: "metacritic", label: "Metacritic", scale: "0–100"},
	{key: "tmdb", label: "TMDB", scale: "0–10"},
	{key: "aoty_critic", label: "AOTY Critic", scale: "0–100"},
	{key: "aoty_user", label: "AOTY User", scale: "0–100"},
	{key: "discogs", label: "Discogs", scale: "0–5"},
	{key: "musicbrainz", label: "MusicBrainz", scale: "0–10"},
	{key: "lastfm", label: "Last.fm (popularity)", scale: "0–10"},
	{key: "anilist", label: "AniList", scale: "0–10"},
	{key: "kitsu", label: "Kitsu", scale: "0–10"},
}

type sourceMeta struct {
	key   string
	label string
	scale string
}

// ── Screen ────────────────────────────────────────────────────────────────────

type RatingWeightsScreen struct {
	Dims
	weights map[string]float64
	rows    []sourceMeta
	cursor  int
}

// NewRatingWeightsScreen builds the editor seeded with the current
// per-source weight map. The `weights` argument is a snapshot from
// `cfg.Providers.RatingSourceWeights` — we copy on construction so
// we can edit independently and emit the full updated map back to
// the parent on every change.
func NewRatingWeightsScreen(weights map[string]float64) RatingWeightsScreen {
	// Copy to avoid mutating the caller's map until SettingsChangedMsg
	// commits the update.
	w := make(map[string]float64, len(weights))
	for k, v := range weights {
		w[k] = v
	}
	return RatingWeightsScreen{
		weights: w,
		rows:    buildRowList(w),
	}
}

// buildRowList composes the union of canonical sources and any user
// keys not in the canonical list (so installed third-party plugins
// surface even before they're added to canonicalSources). Result is
// stable-sorted: canonical order first, then extras alphabetised
// underneath.
func buildRowList(weights map[string]float64) []sourceMeta {
	rows := make([]sourceMeta, 0, len(canonicalSources)+len(weights))
	seen := make(map[string]bool, len(canonicalSources))
	for _, s := range canonicalSources {
		rows = append(rows, s)
		seen[s.key] = true
	}
	var extras []string
	for k := range weights {
		if !seen[k] {
			extras = append(extras, k)
		}
	}
	sort.Strings(extras)
	for _, k := range extras {
		rows = append(rows, sourceMeta{key: k, label: k, scale: "(custom)"})
	}
	return rows
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m RatingWeightsScreen) Init() tea.Cmd { return nil }

func (m RatingWeightsScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.setWindowSize(msg)
		return m, nil
	case tea.KeyPressMsg:
		switch msg.String() {
		case "q", "esc":
			return m, func() tea.Msg { return screen.PopMsg{} }
		case "j", "down":
			if m.cursor < len(m.rows)-1 {
				m.cursor++
			}
			return m, nil
		case "k", "up":
			if m.cursor > 0 {
				m.cursor--
			}
			return m, nil
		case "h", "left", "-":
			return m.adjust(-0.1)
		case "l", "right", "+", "=":
			return m.adjust(+0.1)
		case "0":
			return m.set(0.0)
		case "1":
			return m.set(1.0)
		}
	}
	return m, nil
}

// adjust nudges the current row's weight by `delta` (positive or
// negative), clamps to 0..2, and emits a SettingsChangedMsg with
// the full map so the parent model can persist + push to the
// runtime in one shot.
func (m RatingWeightsScreen) adjust(delta float64) (screen.Screen, tea.Cmd) {
	if m.cursor < 0 || m.cursor >= len(m.rows) {
		return m, nil
	}
	key := m.rows[m.cursor].key
	cur := m.weights[key]
	next := cur + delta
	if next < 0 {
		next = 0
	}
	if next > 2.0 {
		next = 2.0
	}
	// Round to one decimal so the value stays clean across many
	// nudges (otherwise float drift turns 1.0 into 0.9999999…).
	next = float64(int(next*10+0.5)) / 10.0
	m.weights[key] = next
	return m, m.emitChange()
}

// set replaces the current row's weight with `v` exactly (used by
// the "0" disable and "1" reset shortcuts).
func (m RatingWeightsScreen) set(v float64) (screen.Screen, tea.Cmd) {
	if m.cursor < 0 || m.cursor >= len(m.rows) {
		return m, nil
	}
	key := m.rows[m.cursor].key
	m.weights[key] = v
	return m, m.emitChange()
}

// emitChange ships a copy of the current map to the parent model.
// The parent persists it via `cfg.Providers.RatingSourceWeights` +
// `config.Save()` and forwards to the runtime via `SetConfig`.
// Sending the full map (not just the row that changed) keeps the
// IPC handler simple and means the runtime + TUI configs always
// converge on the same shape.
func (m RatingWeightsScreen) emitChange() tea.Cmd {
	snapshot := make(map[string]float64, len(m.weights))
	for k, v := range m.weights {
		snapshot[k] = v
	}
	return func() tea.Msg {
		return SettingsChangedMsg{Key: "rating_weights", Value: snapshot}
	}
}

func (m RatingWeightsScreen) View() tea.View {
	neon := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	bold := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	hi := lipgloss.NewStyle().Foreground(theme.T.Bg()).Background(theme.T.Accent())
	off := lipgloss.NewStyle().Foreground(theme.T.TextDim()).Italic(true)

	title := neon.Render("⚖  Rating Source Weights")
	sub := dim.Render("editable · live-pushed to runtime")
	header := lipgloss.JoinHorizontal(lipgloss.Top, title, "   ", sub)

	intro := dim.Render(
		"Each source contributes to the weighted-median composite shown on cards.\n  " +
			"Higher weight = more influence. 0 disables a source entirely. 1.0 is neutral.",
	)

	// Render the editable list. Selected row gets a highlight bar;
	// rows with weight=0 dim out so disabled sources are visually
	// quiet.
	maxLabelW := 0
	for _, r := range m.rows {
		if w := lipgloss.Width(r.label); w > maxLabelW {
			maxLabelW = w
		}
	}
	if maxLabelW < 18 {
		maxLabelW = 18
	}

	var rowLines []string
	for i, r := range m.rows {
		w := m.weights[r.key]
		valStr := fmt.Sprintf("%.1f", w)
		bar := weightBar(w)
		labelStyled := lipgloss.NewStyle().Width(maxLabelW).Render(r.label)
		scaleStyled := lipgloss.NewStyle().Width(10).Render(r.scale)
		line := fmt.Sprintf("  %s  %s  %s   %s",
			labelStyled,
			scaleStyled,
			bold.Render(valStr),
			bar,
		)
		if w == 0 {
			line = off.Render(line)
		}
		if i == m.cursor {
			line = hi.Render(line)
		}
		rowLines = append(rowLines, line)
	}
	body := ""
	for _, l := range rowLines {
		body += l + "\n"
	}

	keysHint := dim.Render(
		"↑/↓ select   ←/→ ±0.1   0 disable   1 reset   q close",
	)

	return tea.NewView("\n  " + header +
		"\n\n  " + intro +
		"\n\n" + body +
		"\n  " + keysHint)
}

// weightBar renders a tiny ASCII bar — 20 cells, each 0.1 — so
// users can eyeball a source's weight without parsing the number.
func weightBar(w float64) string {
	const cells = 20
	filled := int(w*10 + 0.5)
	if filled < 0 {
		filled = 0
	}
	if filled > cells {
		filled = cells
	}
	bar := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	out := ""
	for i := 0; i < cells; i++ {
		if i < filled {
			out += bar.Render("▮")
		} else {
			out += dim.Render("▯")
		}
	}
	return out
}
