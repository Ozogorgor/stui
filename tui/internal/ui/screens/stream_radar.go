package screens

// stream_radar.go — "Stream Radar" stats screen.
//
// Shows an accumulated histogram of resolved stream candidates, grouped by
// Resolution, Provider, and Protocol. Data is collected in the root model
// across the whole session and passed in on open; while this screen is on
// the stack it also processes live StreamsResolvedMsg updates.
//
// Layout (three columns, bar charts):
//
//   ⚡ Stream Radar                    30 streams · 5 searches
//
//   Resolution          Provider             Protocol
//   ─────────────────   ─────────────────    ─────────────────
//   1080p  ██████  18   Torrentio  ████  12  torrent  ████  20
//   720p   ████    10   TorrentGx  ██     5  http     ██     8
//   4K     ██       5   YTS        ██     3  magnet   █      2
//   480p   █        2
//
//   HDR: 8
//
//   q close

import (
	"fmt"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// ── Stats struct (also used by ui.go to accumulate cross-session data) ────────

// StreamRadarStats holds accumulated stream resolution statistics for the
// current session. Zero value is usable (all maps nil → treated as empty).
type StreamRadarStats struct {
	TotalBatches int
	TotalStreams int
	Resolution   map[string]int
	Provider     map[string]int
	Protocol     map[string]int
	HDRCount     int
}

// AddBatch folds a slice of StreamInfo into the stats.
func (s *StreamRadarStats) AddBatch(streams []ipc.StreamInfo) {
	if s.Resolution == nil {
		s.Resolution = make(map[string]int)
		s.Provider = make(map[string]int)
		s.Protocol = make(map[string]int)
	}
	s.TotalBatches++
	for _, st := range streams {
		s.TotalStreams++
		if q := radarQualityKey(st); q != "" {
			s.Resolution[q]++
		}
		if st.Provider != "" {
			s.Provider[st.Provider]++
		}
		proto := st.Protocol
		if proto == "" {
			proto = "unknown"
		}
		s.Protocol[proto]++
		if st.HDR {
			s.HDRCount++
		}
	}
}

// radarQualityKey normalises stream quality to a display label.
func radarQualityKey(st ipc.StreamInfo) string {
	q := st.Quality
	if q == "" {
		q = st.Label
	}
	// normalise common aliases
	switch {
	case strings.Contains(q, "2160") || strings.EqualFold(q, "4k") || strings.EqualFold(q, "uhd"):
		return "4K"
	case strings.Contains(q, "1440"):
		return "1440p"
	case strings.Contains(q, "1080"):
		return "1080p"
	case strings.Contains(q, "720"):
		return "720p"
	case strings.Contains(q, "480"):
		return "480p"
	case strings.Contains(q, "360"):
		return "360p"
	}
	if q != "" {
		return q
	}
	return ""
}

// ── Screen ────────────────────────────────────────────────────────────────────

// OpenStreamRadarMsg is emitted by the settings screen to trigger the radar.
type OpenStreamRadarMsg struct{}

// StreamRadarScreen displays the accumulated stream stats.
type StreamRadarScreen struct {
	Dims
	stats  StreamRadarStats
}

// NewStreamRadarScreen creates the screen with a pre-populated stats snapshot.
func NewStreamRadarScreen(stats StreamRadarStats) StreamRadarScreen {
	return StreamRadarScreen{stats: stats}
}

// ── screen.Screen interface ───────────────────────────────────────────────────

func (m StreamRadarScreen) Init() tea.Cmd { return nil }

func (m StreamRadarScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {

	case tea.WindowSizeMsg:
		m.setWindowSize(msg)

	case ipc.StreamsResolvedMsg:
		// Live update while radar is on the stack.
		m.stats.AddBatch(msg.Streams)

	case tea.KeyPressMsg:
		switch msg.String() {
		case "q", "esc":
			return m, func() tea.Msg { return screen.PopMsg{} }
		}
	}
	return m, nil
}

func (m StreamRadarScreen) View() tea.View {
	neon := lipgloss.NewStyle().Foreground(theme.T.Accent())
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	bold := lipgloss.NewStyle().Foreground(theme.T.Text()).Bold(true)
	gold := lipgloss.NewStyle().Foreground(lipgloss.Color("#FFD700"))

	// ── Header ──────────────────────────────────────────────────────────
	title := neon.Render("⚡ Stream Radar")
	summary := dim.Render(fmt.Sprintf(
		"%d stream(s) across %d search(es)",
		m.stats.TotalStreams, m.stats.TotalBatches,
	))
	header := lipgloss.JoinHorizontal(lipgloss.Top,
		title,
		"   ",
		summary,
	)

	if m.stats.TotalStreams == 0 {
		empty := dim.Render("No streams resolved yet — browse and open a title to populate the radar.")
		footer := "\n" + hintBar("q close")
		return tea.NewView(header + "\n\n  " + empty + footer)
	}

	// ── Column widths ────────────────────────────────────────────────────
	avail := m.width - 4
	if avail < 40 {
		avail = 40
	}
	colW := avail / 3

	// ── Render one column ─────────────────────────────────────────────────
	renderCol := func(heading string, counts map[string]int) string {
		if len(counts) == 0 {
			return lipgloss.NewStyle().Width(colW).Render(bold.Render(heading) + "\n" + dim.Render("—"))
		}

		// Sort by count desc, then label asc for ties.
		type kv struct {
			k string
			v int
		}
		pairs := make([]kv, 0, len(counts))
		maxVal := 0
		for k, v := range counts {
			pairs = append(pairs, kv{k, v})
			if v > maxVal {
				maxVal = v
			}
		}
		sort.Slice(pairs, func(i, j int) bool {
			if pairs[i].v != pairs[j].v {
				return pairs[i].v > pairs[j].v
			}
			return pairs[i].k < pairs[j].k
		})

		barMax := colW - 18 // label(8) + space(2) + bar + space(1) + count(5)
		if barMax < 4 {
			barMax = 4
		}

		divider := dim.Render(strings.Repeat("─", colW-2))
		var lines []string
		lines = append(lines, bold.Render(heading))
		lines = append(lines, divider)
		for _, p := range pairs {
			barLen := 0
			if maxVal > 0 {
				barLen = (p.v * barMax) / maxVal
				if barLen < 1 {
					barLen = 1
				}
			}
			bar := neon.Render(strings.Repeat("█", barLen))
			label := fmt.Sprintf("%-8s", truncateStr(p.k, 8))
			count := dim.Render(fmt.Sprintf("%3d", p.v))
			lines = append(lines, label+"  "+bar+"  "+count)
		}
		return lipgloss.NewStyle().Width(colW).Render(strings.Join(lines, "\n"))
	}

	resCol := renderCol("Resolution", m.stats.Resolution)
	provCol := renderCol("Provider", m.stats.Provider)
	protoCol := renderCol("Protocol", m.stats.Protocol)

	body := lipgloss.JoinHorizontal(lipgloss.Top, resCol, provCol, protoCol)

	// ── HDR line ─────────────────────────────────────────────────────────
	hdrLine := ""
	if m.stats.HDRCount > 0 {
		hdrPct := 0
		if m.stats.TotalStreams > 0 {
			hdrPct = (m.stats.HDRCount * 100) / m.stats.TotalStreams
		}
		hdrLine = "\n  " + gold.Render(fmt.Sprintf("HDR streams: %d  (%d%%)", m.stats.HDRCount, hdrPct))
	}

	footer := "\n\n" + hintBar("q close")

	return tea.NewView("  " + header + "\n\n" + body + hdrLine + footer)
}
