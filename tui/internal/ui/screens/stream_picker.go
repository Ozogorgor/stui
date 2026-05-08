package screens

// stream_picker.go — StreamPickerScreen: browse and select stream candidates.
//
// Manual mode (default):
//   ↑↓ navigate rows  tab cycle sort  r reverse  enter play  esc back
//
// Smart Auto-Pick (press 'A'):
//   Ranks all streams against the user policy, shows the best match with
//   score breakdown and a top-5 ranking.  Enter confirms, Esc returns to
//   the manual list.
//
// Policy: stream selection preferences are fetched from Rust via get_stream_policy
// on screen open and saved via set_stream_policy. See runtime/src/pipeline/policy_io.rs.

import (
	"fmt"
	"sort"
	"strings"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/actions"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/streambench"
	"github.com/stui/stui/pkg/theme"
)

// ── Sort ──────────────────────────────────────────────────────────────────────

// sortColumn identifies which field streams are sorted by.
type sortColumn int

const (
	sortByQuality  sortColumn = iota // resolution rank (4K → 1080p → 720p…)
	sortBySeeders                    // torrent seeders descending
	sortBySize                       // file size descending
	sortByProvider                   // provider name alphabetically
	sortByScore                      // runtime quality score
	sortBySpeed                      // measured/estimated transfer speed
	sortColumnCount
)

func (sc sortColumn) label() string {
	switch sc {
	case sortByQuality:
		return "Quality"
	case sortBySeeders:
		return "Seeders"
	case sortBySize:
		return "Size"
	case sortByProvider:
		return "Provider"
	case sortByScore:
		return "Score"
	case sortBySpeed:
		return "Speed"
	}
	return ""
}

// SortStreamsByQualityThenSeeders re-orders a streams slice in place so
// that higher resolutions come first, with seeders descending as the
// secondary key. Streams arrive incrementally during a streaming
// fan-out (provider-by-provider, in arrival order) so this final
// pass — fired on `streams_complete` — turns the accumulated batch
// into a canonical "best at the top" view without disturbing the
// during-wait progressive UX.
func SortStreamsByQualityThenSeeders(streams []ipc.StreamInfo) {
	sort.SliceStable(streams, func(i, j int) bool {
		qi := qualityScore(streams[i].Quality)
		qj := qualityScore(streams[j].Quality)
		if qi != qj {
			return qi > qj
		}
		return streams[i].Seeders > streams[j].Seeders
	})
}

// qualityRank maps quality label prefixes to a numeric rank (higher = better).
var qualityRank = map[string]int{
	"4k": 7, "2160p": 7, "uhd": 7,
	"1440p": 6, "2k": 6,
	"1080p": 5, "fhd": 5,
	"720p": 4, "hd": 4,
	"576p": 3,
	"480p": 2, "sd": 2,
	"360p": 1,
}

// qualityKeys maps number keys "1"–"4" to their quality tier rank and label.
var qualityKeys = map[string]struct {
	rank  int
	label string
}{
	"1": {2, "480p"},
	"2": {4, "720p"},
	"3": {5, "1080p"},
	"4": {7, "4K"},
}

func qualityScore(q string) int {
	lower := strings.ToLower(q)
	for prefix, score := range qualityRank {
		if strings.HasPrefix(lower, prefix) {
			return score
		}
	}
	return 0
}

// streamBadge builds the quality+score display label for a stream row.
// Example: "1080p ★ 87"
func streamBadge(s ipc.StreamInfo) string {
	if s.Quality == "" {
		return fmt.Sprintf("★ %d", s.Score)
	}
	return fmt.Sprintf("%s ★ %d", s.Quality, s.Score)
}

// BestStreamForTier returns the stream with the highest Score
// (ipc.StreamInfo.Score, the provider-reported integer) whose quality label
// resolves to the given qualityRank value, or nil if none match.
//
// Uses qualityScore() for label→rank lookup so "1080p HDR" matches rank 5
// just like "1080p".
func BestStreamForTier(streams []ipc.StreamInfo, rank int) *ipc.StreamInfo {
	var best *ipc.StreamInfo
	for i := range streams {
		s := &streams[i]
		if qualityScore(s.Quality) != rank {
			continue
		}
		if best == nil || s.Score > best.Score {
			best = s
		}
	}
	return best
}

// ── Policy scoring ────────────────────────────────────────────────────────────
// NOTE: Scoring logic lives in the Rust runtime (pipeline/rank.rs).
// Policy is fetched from Rust via get_stream_policy IPC on screen open.
// Go uses ipc.StreamPreferences directly; no local file I/O.

// ── Screen ────────────────────────────────────────────────────────────────────

// benchState holds the probe result for one stream URL.
type benchState struct {
	speedMbps float64 // measured (HTTP) or estimated (torrent)
	latencyMs int
	estimated bool // true = seeder-based proxy, not a real measurement
	done      bool
	err       error
}

// speedLabel formats a benchState for display in the stream list.
func (b *benchState) speedLabel() string {
	if b == nil {
		return "..."
	}
	if !b.done {
		return "..."
	}
	if b.estimated {
		return fmt.Sprintf("~%.0f Mb/s", b.speedMbps)
	}
	if b.err != nil || b.speedMbps == 0 {
		return "—"
	}
	if b.speedMbps >= 100 {
		return fmt.Sprintf("%.0f Mb/s", b.speedMbps)
	}
	return fmt.Sprintf("%.1f Mb/s", b.speedMbps)
}

// StreamPickerScreen shows all resolved stream candidates for a media item.
// The user can browse by quality/seeders and select one to play.
//
// Activated by pressing `s` during playback or from the detail overlay.
// To open: screen.TransitionCmd(NewStreamPickerScreen(client, title, entryID, benchEnabled), true)
type StreamPickerScreen struct {
	Dims
	client  *ipc.Client
	title   string
	entryID string
	streams []ipc.StreamInfo // sorted copy
	cursor  int
	loading bool

	sortCol  sortColumn
	sortDesc bool // true = descending (default for quality/seeders/size/score)

	// Smart auto-pick
	policy     ipc.StreamPreferences
	autoRanked []ipc.RankedStream // non-nil = auto-pick mode active

	// Benchmark mode
	benchEnabled bool
	benchResults map[string]*benchState // keyed by URL
	benchPending int                    // probes still running
	benchTotal   int                    // total streams to probe

	spinner components.Spinner
}

func NewStreamPickerScreen(client *ipc.Client, title, entryID string, benchEnabled bool) StreamPickerScreen {
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	return StreamPickerScreen{
		client:       client,
		title:        title,
		entryID:      entryID,
		loading:      true,
		sortCol:      sortByQuality,
		sortDesc:     true,
		benchEnabled: benchEnabled,
		benchResults: make(map[string]*benchState),
		spinner:      *components.NewSpinner("resolving streams…", dimStyle),
	}
}

func (s StreamPickerScreen) Init() tea.Cmd {
	s.spinner.Start()
	if s.client != nil {
		if s.entryID != "" {
			s.client.Resolve(s.entryID, "")
		}
		s.client.GetStreamPolicy()
	}
	return nil
}

func (s StreamPickerScreen) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch m := msg.(type) {

	case ipc.StreamPolicyLoadedMsg:
		if m.Err == nil {
			s.policy = m.Policy
		}
		return s, nil

	case spinner.TickMsg:
		_, cmd := s.spinner.Update(m)
		return s, cmd

	case tea.WindowSizeMsg:
		s.setWindowSize(m)

	case ipc.StreamsResolvedMsg:
		if m.EntryID == s.entryID {
			s.streams = sortStreams(m.Streams, s.sortCol, s.sortDesc)
			s.loading = false
			s.spinner.Stop()
			s.cursor = 0

			// Pre-populate benchmark results from Rust if available
			if s.benchEnabled {
				for _, st := range s.streams {
					if st.SpeedMbps > 0 || st.LatencyMs > 0 {
						// Rust already provided benchmark data
						s.benchResults[st.URL] = &benchState{
							speedMbps: st.SpeedMbps,
							latencyMs: st.LatencyMs,
							estimated: isTorrentStream(st),
							done:      true,
						}
					}
				}
				// Count streams that still need probing
				pending := 0
				for _, st := range s.streams {
					if _, exists := s.benchResults[st.URL]; !exists {
						pending++
					}
				}
				if pending > 0 {
					s.benchPending = pending
					s.benchTotal = pending
					return s, s.makeBenchCmds(s.streams)
				}
			}
		}

	case ipc.StreamBenchmarkResultMsg:
		if m.EntryID != s.entryID {
			break
		}
		// Don't overwrite pre-populated results from Rust
		if _, exists := s.benchResults[m.URL]; exists {
			break
		}
		isTorrent := false
		for _, st := range s.streams {
			if st.URL == m.URL && isTorrentStream(st) {
				isTorrent = true
				break
			}
		}
		s.benchResults[m.URL] = &benchState{
			speedMbps: m.SpeedMbps,
			latencyMs: m.LatencyMs,
			estimated: isTorrent,
			done:      true,
			err:       m.Err,
		}
		if s.benchPending > 0 {
			s.benchPending--
		}

	case ipc.StreamBenchmarkDoneMsg:
		// All probes complete — auto-sort by speed if still on speed column.
		if m.EntryID == s.entryID && s.sortCol == sortBySpeed {
			s.streams = s.sortBySpeedSlice(s.streams)
		}

	case ipc.StreamsRankedMsg:
		// Ranking result from Rust runtime
		if m.Err != nil {
			return s, func() tea.Msg {
				return ipc.StatusMsg{Text: "Ranking failed: " + m.Err.Error()}
			}
		}
		s.autoRanked = m.Ranked

	case tea.KeyPressMsg:
		key := m.String()

		// ── Auto-pick mode controls ──────────────────────────────────────
		if s.autoRanked != nil {
			switch key {
			case "enter":
				if len(s.autoRanked) > 0 && s.client != nil {
					s.client.SwitchStream(s.autoRanked[0].Stream.URL)
					return s, func() tea.Msg { return screen.PopMsg{} }
				}
			case "esc", "q":
				s.autoRanked = nil
			}
			return s, nil
		}

		// ── 'A' triggers smart auto-pick (calls Rust via IPC) ───────────
		if key == "A" && !s.loading && len(s.streams) > 0 {
			if s.client != nil {
				s.client.RankStreams(s.streams, s.policy)
				return s, func() tea.Msg {
					return ipc.StatusMsg{Text: "Ranking streams..."}
				}
			}
		}

		// ── Manual mode ───────────────────────────────────────────────────
		switch key {
		case "tab":
			next := (s.sortCol + 1) % sortColumnCount
			// Skip speed column if benchmark hasn't run yet.
			if next == sortBySpeed && !s.benchEnabled {
				next = (next + 1) % sortColumnCount
			}
			s.sortCol = next
			s.sortDesc = s.sortCol != sortByProvider
			if s.sortCol == sortBySpeed {
				s.streams = s.sortBySpeedSlice(s.streams)
			} else {
				s.streams = sortStreams(s.streams, s.sortCol, s.sortDesc)
			}
			s.cursor = 0
			return s, nil
		case "r":
			s.sortDesc = !s.sortDesc
			if s.sortCol == sortBySpeed {
				s.streams = s.sortBySpeedSlice(s.streams)
			} else {
				s.streams = sortStreams(s.streams, s.sortCol, s.sortDesc)
			}
			s.cursor = 0
			return s, nil
		}

		// 'B' — trigger benchmark (always available even when setting is off)
		if key == "B" && !s.loading && len(s.streams) > 0 {
			s.benchEnabled = true
			s.benchResults = make(map[string]*benchState)
			s.benchPending = len(s.streams)
			s.benchTotal = len(s.streams)
			return s, s.makeBenchCmds(s.streams)
		}

		// 'd' — pre-download a torrent/magnet stream without playing
		if key == "d" && !s.loading && len(s.streams) > 0 {
			st := s.streams[s.cursor]
			if isTorrentStream(st) && s.client != nil {
				s.client.DownloadStream(st.URL, s.title)
				return s, func() tea.Msg { return screen.PopMsg{} }
			}
		}

		// Quality quick keys: 1=480p, 2=720p, 3=1080p, 4=4K
		// Checked before actions.FromKey to override any global key bindings.
		if !s.loading && len(s.streams) > 0 {
			if tier, ok := qualityKeys[key]; ok {
				if best := BestStreamForTier(s.streams, tier.rank); best != nil && s.client != nil {
					s.client.SwitchStream(best.URL)
					return s, func() tea.Msg { return screen.PopMsg{} }
				}
				return s, func() tea.Msg {
					return ipc.StatusMsg{Text: "No " + tier.label + " streams available"}
				}
			}
		}

		if action, ok := actions.FromKey(key); ok {
			switch action {
			case actions.ActionNavigateDown:
				if s.cursor < len(s.streams)-1 {
					s.cursor++
				}
			case actions.ActionNavigateUp:
				if s.cursor > 0 {
					s.cursor--
				}
			case actions.ActionSelect:
				if len(s.streams) > 0 && s.client != nil {
					s.client.SwitchStream(s.streams[s.cursor].URL)
					return s, func() tea.Msg { return screen.PopMsg{} }
				}
			}
		}
	}
	return s, nil
}

// makeBenchCmds returns one tea.Cmd per stream in the list.
// HTTP(S) streams get a real probe; torrents get an immediate seeder estimate.
func (s *StreamPickerScreen) makeBenchCmds(streams []ipc.StreamInfo) tea.Cmd {
	if len(streams) == 0 {
		return nil
	}
	entryID := s.entryID
	var cmds []tea.Cmd
	for _, st := range streams {
		url := st.URL
		seeders := st.Seeders
		if isTorrentStream(st) {
			// Instant seeder-based estimate — no network call.
			speedEst := float64(seeders) * 0.12 // rough heuristic: 100 seeds ≈ 12 Mb/s
			cmds = append(cmds, func() tea.Msg {
				return ipc.StreamBenchmarkResultMsg{
					EntryID:   entryID,
					URL:       url,
					SpeedMbps: speedEst,
				}
			})
		} else {
			// Real HTTP range probe.
			cmds = append(cmds, func() tea.Msg {
				r := streambench.Probe(url)
				return ipc.StreamBenchmarkResultMsg{
					EntryID:   entryID,
					URL:       url,
					SpeedMbps: r.SpeedMbps,
					LatencyMs: r.LatencyMs,
					Err:       r.Err,
				}
			})
		}
	}
	return tea.Batch(cmds...)
}

// isTorrentStream returns true if the stream URL is a magnet link or torrent.
func isTorrentStream(s ipc.StreamInfo) bool {
	url := strings.ToLower(s.URL)
	proto := strings.ToLower(s.Protocol)
	return strings.HasPrefix(url, "magnet:") ||
		strings.HasSuffix(url, ".torrent") ||
		proto == "magnet" ||
		proto == "torrent"
}

// ── View ──────────────────────────────────────────────────────────────────────

func (s StreamPickerScreen) View() tea.View {
	if s.autoRanked != nil {
		return tea.NewView(s.viewAutoMode())
	}
	return tea.NewView(s.viewManualMode())
}

func (s StreamPickerScreen) viewManualMode() string {
	accent := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	normal := lipgloss.NewStyle().Foreground(theme.T.Text())
	warn := lipgloss.NewStyle().Foreground(theme.T.Warn())
	gold := lipgloss.NewStyle().Foreground(lipgloss.Color("#f59e0b"))
	green := lipgloss.NewStyle().Foreground(theme.T.Success())

	var sb strings.Builder
	sb.WriteString("\n  " + accent.Render("⚡ Streams") + "  " + dim.Render(s.title) + "\n\n")

	if s.loading {
		sb.WriteString("  " + s.spinner.View() + "\n")
		return sb.String()
	}
	if len(s.streams) == 0 {
		sb.WriteString(dim.Render("  No streams found") + "\n")
		return sb.String()
	}

	// Virtualized list for scrollbar
	listHeight := s.height - 20
	if listHeight < 5 {
		listHeight = 5
	}
	vl := components.NewVirtualizedList(len(s.streams), s.cursor, listHeight)
	start, end := vl.VisibleRange()
	bar := components.Scrollbar(start, end-start, len(s.streams))

	// ── Sort header ───────────────────────────────────────────────────────
	arrow := "\u2193"
	if !s.sortDesc {
		arrow = "\u2191"
	}
	colW := 12
	benchActive := s.benchEnabled
	speedHeader := ""
	if benchActive {
		speedHeader = "  %-10s"
	}
	headerFmt := "  %-*s  %-16s  %-9s" + speedHeader + "  %s"
	var header string
	if benchActive {
		header = fmt.Sprintf(headerFmt, colW, "Quality", "Provider", "Size", "Speed", "Seeders")
	} else {
		header = fmt.Sprintf("  %-*s  %-16s  %-9s  %s", colW, "Quality", "Provider", "Size", "Seeders")
	}
	benchStatus := ""
	if benchActive {
		if s.benchPending > 0 {
			completed := s.benchTotal - s.benchPending
			pb := components.NewProgressBar(float64(completed), float64(s.benchTotal),
				components.WithWidth(24),
				components.WithShowValue(false),
			)
			benchStatus = dim.Render(fmt.Sprintf("  probing %d/%d ", completed, s.benchTotal)) + pb.View()
		} else if s.benchTotal > 0 {
			pb := components.NewProgressBar(1, 1,
				components.WithWidth(24),
				components.WithShowValue(false),
			)
			doneStyle := lipgloss.NewStyle().Foreground(theme.T.Success())
			benchStatus = "  " + doneStyle.Render("✓") + dim.Render(" benchmark complete ") + pb.View()
		}
	}
	sortIndicator := fmt.Sprintf("  sorted by %s %s", s.sortCol.label(), arrow)
	sb.WriteString(dim.Render(header) + "\n")
	sb.WriteString(dim.Render(sortIndicator) + benchStatus + "\n")

	// ── Stream rows ───────────────────────────────────────────────────────
	var rowLines []string
	for i := start; i < end; i++ {
		st := s.streams[i]
		isSelected := i == s.cursor
		prefix := "  "
		rowStyle := normal
		if isSelected {
			prefix = "\u25b6 "
			rowStyle = accent
		}

		label := streamBadge(st)
		qualCol := rowStyle.Render(fmt.Sprintf("%-*s", colW, label))
		provCol := dim.Render(fmt.Sprintf("%-16s", "["+st.Provider+"]"))
		sizeCol := dim.Render(fmt.Sprintf("%-9s", formatBytes(st.SizeBytes)))

		hdrBadge := "    "
		if st.HDR {
			hdrBadge = "  " + gold.Render("HDR")
		}

		seedCol := ""
		if st.Seeders > 0 {
			seedStyle := dim
			if st.Seeders >= 50 {
				seedStyle = green
			} else if st.Seeders < 10 {
				seedStyle = warn
			}
			seedCol = seedStyle.Render(fmt.Sprintf("\U0001f465 %d", st.Seeders))
		}

		speedCol := ""
		if benchActive {
			bs := s.benchResults[st.URL]
			lbl := bs.speedLabel()
			var spStyle lipgloss.Style
			switch {
			case bs == nil || !bs.done:
				spStyle = dim
			case bs.estimated:
				spStyle = warn
			case bs.speedMbps >= 20:
				spStyle = green
			case bs.speedMbps >= 5:
				spStyle = normal
			default:
				spStyle = warn
			}
			speedCol = "  " + spStyle.Render(fmt.Sprintf("%-10s", lbl))
		}

		line := prefix + qualCol + "  " + provCol + "  " + sizeCol + hdrBadge + speedCol + "  " + seedCol
		rowLines = append(rowLines, line)
	}

	borderStyle := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(0, 1)

	// Place scrollbar as a separate column adjacent to the rows, then wrap
	// with single border.
	content := lipgloss.JoinHorizontal(lipgloss.Top, strings.Join(rowLines, "\n"), " ", bar)
	if content != "" {
		sb.WriteString(borderStyle.Render(content) + "\n")
	}

	// ── Stream info panel for selected stream ────────────────────────────
	if len(s.streams) > 0 {
		sb.WriteString("\n")
		sb.WriteString(s.viewStreamInfo(s.streams[s.cursor]))
	}

	// Download hint — only shown when cursor is on a torrent stream
	downloadHint := ""
	if len(s.streams) > 0 && isTorrentStream(s.streams[s.cursor]) {
		downloadHint = "   " + dim.Render("d pre-download")
	}
	autoHint := dim.Render("A auto-pick")
	benchHint := dim.Render("B benchmark")
	sb.WriteString("\n" + hintBar("↑↓ navigate", "enter play", "tab sort", "r reverse", "esc back") +
		"   " + autoHint + "   " + benchHint + downloadHint +
		"   " + dim.Render("1-4 quality") + "\n")
	return sb.String()
}

// viewStreamInfo renders a compact metadata panel for the given stream.
func (s StreamPickerScreen) viewStreamInfo(st ipc.StreamInfo) string {
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	acc := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())
	gold := lipgloss.NewStyle().Foreground(lipgloss.Color("#f59e0b"))
	green := lipgloss.NewStyle().Foreground(theme.T.Success())

	type row struct{ label, value string }
	var rows []row

	add := func(label, value string) {
		if value != "" && value != "0" {
			rows = append(rows, row{label, value})
		}
	}

	qual := streamBadge(st)
	add("Resolution", qual)
	add("Codec", st.Codec)
	add("Source", st.Source)
	add("Protocol", st.Protocol)
	add("Size", formatBytes(st.SizeBytes))
	add("Provider", st.Provider)
	if st.Seeders > 0 {
		add("Seeders", fmt.Sprintf("%d", st.Seeders))
	}
	if st.HDR {
		add("HDR", "yes")
	}
	if st.Score > 0 {
		add("Score", fmt.Sprintf("%d", st.Score))
	}

	if len(rows) == 0 {
		return ""
	}

	// Find longest label for alignment
	maxLabel := 0
	for _, r := range rows {
		if len(r.label) > maxLabel {
			maxLabel = len(r.label)
		}
	}

	var lines []string
	for _, r := range rows {
		lbl := fmt.Sprintf("%-*s", maxLabel, r.label)
		var valStyle lipgloss.Style
		switch r.label {
		case "Resolution":
			valStyle = neon
		case "Seeders":
			n, _ := fmt.Sscanf(r.value, "%d", new(int))
			_ = n
			valStyle = green
		case "HDR":
			valStyle = gold
		case "Score":
			valStyle = acc
		default:
			valStyle = lipgloss.NewStyle().Foreground(theme.T.Text())
		}
		lines = append(lines, dim.Render(lbl+" : ")+valStyle.Render(r.value))
	}

	// Split into at most two columns to save vertical space
	half := (len(lines) + 1) / 2
	var colA, colB []string
	colA = lines[:half]
	if half < len(lines) {
		colB = lines[half:]
	}

	colWidth := 26
	var sb strings.Builder
	sb.WriteString("  " + dim.Render("Stream Info") + "\n")
	maxRows := half
	for i := 0; i < maxRows; i++ {
		left := colA[i]
		right := ""
		if i < len(colB) {
			right = colB[i]
		}
		sb.WriteString("  " + fmt.Sprintf("%-*s", colWidth, left) + "  " + right + "\n")
	}

	return sb.String()
}

func (s StreamPickerScreen) viewAutoMode() string {
	accent := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	neon := lipgloss.NewStyle().Foreground(theme.T.Neon())
	green := lipgloss.NewStyle().Foreground(theme.T.Success())
	warn := lipgloss.NewStyle().Foreground(theme.T.Warn())
	gold := lipgloss.NewStyle().Foreground(lipgloss.Color("#f59e0b"))

	ranked := s.autoRanked
	var sb strings.Builder
	sb.WriteString("\n  " + neon.Render("✦ Smart Auto-Pick") + "  " + dim.Render(s.title) + "\n\n")

	if len(ranked) == 0 {
		sb.WriteString(dim.Render("  No streams to rank") + "\n")
		sb.WriteString("\n" + dim.Render("  [Esc] back") + "\n")
		return sb.String()
	}

	best := ranked[0]

	// ── Best match summary ────────────────────────────────────────────────
	sb.WriteString("  " + accent.Render("Best match") +
		dim.Render(fmt.Sprintf("  (ranked %d streams)", len(ranked))) + "\n\n")

	// Stream headline
	label := streamBadge(best.Stream)
	hdrTag := ""
	if best.Stream.HDR {
		hdrTag = "  " + gold.Render("HDR")
	}
	seedTag := ""
	if best.Stream.Seeders > 0 {
		seedTag = fmt.Sprintf("  \U0001f465 %d", best.Stream.Seeders)
	}
	sizeTag := ""
	if best.Stream.SizeBytes > 0 {
		sizeTag = "  " + formatBytes(best.Stream.SizeBytes)
	}
	headline := green.Render("  ▶  ") + accent.Render(label) +
		"  " + dim.Render(best.Stream.Protocol) +
		dim.Render(sizeTag+seedTag) + hdrTag
	sb.WriteString(headline + "\n")
	sb.WriteString("     " + dim.Render(fmt.Sprintf("Score: %d pts", best.Score)) + "\n")

	// Score breakdown (indented tree)
	for i, r := range best.Reasons {
		prefix := "     ├ "
		if i == len(best.Reasons)-1 {
			prefix = "     └ "
		}
		sb.WriteString(dim.Render(prefix) + dim.Render(r) + "\n")
	}
	sb.WriteString("\n")

	// ── Top-5 ranking ─────────────────────────────────────────────────────
	limit := 5
	if len(ranked) < limit {
		limit = len(ranked)
	}
	sb.WriteString("  " + dim.Render("Ranking:") + "\n")
	for i := 0; i < limit; i++ {
		r := ranked[i]
		marker := "  "
		var numStyle lipgloss.Style
		if i == 0 {
			marker = green.Render("✓ ")
			numStyle = green
		} else {
			numStyle = dim
		}
		lbl := streamBadge(r.Stream)
		seederStr := ""
		if r.Stream.Seeders > 0 {
			seedStyle := dim
			if r.Stream.Seeders >= 50 {
				seedStyle = green
			} else if r.Stream.Seeders < 10 {
				seedStyle = warn
			}
			seederStr = "  " + seedStyle.Render(fmt.Sprintf("\U0001f465 %d", r.Stream.Seeders))
		}
		sb.WriteString(fmt.Sprintf("  %s%s  %-10s  %-10s  %s pts%s\n",
			marker,
			numStyle.Render(fmt.Sprintf("%d.", i+1)),
			accent.Render(fmt.Sprintf("%-8s", lbl)),
			dim.Render(fmt.Sprintf("%-10s", r.Stream.Protocol)),
			dim.Render(fmt.Sprintf("%3d", r.Score)),
			seederStr,
		))
	}
	if len(ranked) > limit {
		sb.WriteString("  " + dim.Render(fmt.Sprintf("  … and %d more", len(ranked)-limit)) + "\n")
	}

	// ── Policy summary ────────────────────────────────────────────────────
	sb.WriteString("\n  " + dim.Render("Policy: "))
	parts := s.policyHints()
	if len(parts) == 0 {
		sb.WriteString(dim.Render("defaults"))
	} else {
		sb.WriteString(dim.Render(strings.Join(parts, "  •  ")))
	}
	sb.WriteString("\n  " + dim.Render("Policy managed by runtime (get_stream_policy / set_stream_policy)") + "\n")

	sb.WriteString("\n" + accent.Render("  [Enter]") + dim.Render(" play this stream") +
		"   " + dim.Render("[Esc] back to list") + "\n")
	return sb.String()
}

// policyHints returns a short summary of active (non-default) policy settings.
func (s StreamPickerScreen) policyHints() []string {
	p := s.policy
	var parts []string
	if p.PreferProtocol != "" {
		parts = append(parts, "prefer "+p.PreferProtocol)
	}
	if p.MaxResolution != "" {
		parts = append(parts, "max "+p.MaxResolution)
	}
	if p.MaxSizeMB > 0 {
		parts = append(parts, fmt.Sprintf("max %d MB", p.MaxSizeMB))
	}
	if p.MinSeeders > 0 {
		parts = append(parts, fmt.Sprintf("min %d seeders", p.MinSeeders))
	}
	if len(p.AvoidLabels) > 0 {
		parts = append(parts, "avoid "+strings.Join(p.AvoidLabels, "/"))
	}
	if p.PreferHDR {
		parts = append(parts, "prefer HDR")
	}
	return parts
}

// ── Sorting ───────────────────────────────────────────────────────────────────

func sortStreams(streams []ipc.StreamInfo, col sortColumn, desc bool) []ipc.StreamInfo {
	out := make([]ipc.StreamInfo, len(streams))
	copy(out, streams)

	sort.SliceStable(out, func(i, j int) bool {
		a, b := out[i], out[j]
		less := streamLess(a, b, col)
		if desc {
			return !less
		}
		return less
	})
	return out
}

// streamLess returns true if a should come before b (ascending order).
func streamLess(a, b ipc.StreamInfo, col sortColumn) bool {
	switch col {
	case sortByQuality:
		sa, sb := qualityScore(a.Quality), qualityScore(b.Quality)
		if sa != sb {
			return sa < sb
		}
		return a.Score < b.Score
	case sortBySeeders:
		return a.Seeders < b.Seeders
	case sortBySize:
		return a.SizeBytes < b.SizeBytes
	case sortByProvider:
		return a.Provider < b.Provider
	case sortByScore:
		return a.Score < b.Score
	}
	return false
}

// sortBySpeedSlice sorts streams by benchmark speed descending.
// Streams without a result yet sort last.
func (s StreamPickerScreen) sortBySpeedSlice(streams []ipc.StreamInfo) []ipc.StreamInfo {
	out := make([]ipc.StreamInfo, len(streams))
	copy(out, streams)
	sort.SliceStable(out, func(i, j int) bool {
		si := s.benchResults[out[i].URL]
		sj := s.benchResults[out[j].URL]
		vi := 0.0
		vj := 0.0
		if si != nil && si.done {
			vi = si.speedMbps
		}
		if sj != nil && sj.done {
			vj = sj.speedMbps
		}
		return vi > vj // descending
	})
	return out
}

// ── Helpers ───────────────────────────────────────────────────────────────────

// formatBytes formats a byte count as a human-readable size string.
func formatBytes(b int64) string {
	if b <= 0 {
		return "\u2014"
	}
	const (
		KB = 1024
		MB = KB * 1024
		GB = MB * 1024
	)
	switch {
	case b >= GB:
		return fmt.Sprintf("%.1f GB", float64(b)/GB)
	case b >= MB:
		return fmt.Sprintf("%.0f MB", float64(b)/MB)
	default:
		return fmt.Sprintf("%.0f KB", float64(b)/KB)
	}
}
