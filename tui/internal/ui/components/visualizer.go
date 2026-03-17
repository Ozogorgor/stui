package components

// visualizer.go — Audio frequency visualizer driven by cava or chroma.
//
// Architecture:
//
//   MpdStatusMsg (play) ──► Visualizer.Start()
//                              │
//                              ▼
//                        spawn subprocess
//                        (cava -p /tmp/stui-cava.conf   or
//                         chroma --output raw --bars N)
//                              │
//                              ▼
//                        goroutine: io.ReadFull(stdout, buf)
//                        → []float64{0.0-1.0}  (bars × framerate)
//                              │
//                        VisualizerTickMsg (tea.Tick @ fps)
//                              │
//                              ▼
//                        RenderBars() → lipgloss string
//                        inserted below the MPD HUD
//
// Wire format (raw binary, both cava and chroma):
//   Each frame is exactly cfg.Bars bytes.
//   Each byte is a bar amplitude 0-255 (0 = silence, 255 = peak).
//   Frames arrive at the configured framerate.
//
// cava raw-mode config written to /tmp/stui-cava.conf.
// chroma raw mode: chroma --output raw --bars N --fps N

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/stui/stui/pkg/theme"
)

// ── Config ─────────────────────────────────────────────────────────────────────

// VisualizerBackend identifies which external tool drives the visualization.
type VisualizerBackend int

const (
	VisualizerOff    VisualizerBackend = iota // no visualizer
	VisualizerCava                            // cava --raw mode
	VisualizerChroma                          // chroma --output raw
)

// VisualizerConfig holds all user-configurable settings for the visualizer.
// These settings are TUI-local and not sent to the Rust runtime.
type VisualizerConfig struct {
	Backend    VisualizerBackend
	Bars       int    // number of frequency bars (10–60)
	Height     int    // visualizer height in terminal rows (4–20)
	Framerate  int    // target refresh rate in fps (10–60)
	Symmetric  bool   // mirror bars left↔right
	Gradient   bool   // shade bars from accent (top) to dim (bottom)
	InputMethod string // audio input method: "pulse" | "pipewire" | "alsa"
}

// DefaultVisualizerConfig returns sensible defaults (visualizer off).
func DefaultVisualizerConfig() VisualizerConfig {
	return VisualizerConfig{
		Backend:     VisualizerOff,
		Bars:        20,
		Height:      8,
		Framerate:   20,
		Symmetric:   false,
		Gradient:    true,
		InputMethod: "pulse",
	}
}

// BackendFromString parses a backend name (as stored in settings).
func BackendFromString(s string) VisualizerBackend {
	switch strings.ToLower(s) {
	case "cava":
		return VisualizerCava
	case "chroma":
		return VisualizerChroma
	default:
		return VisualizerOff
	}
}

// ── Messages ──────────────────────────────────────────────────────────────────

// VisualizerTickMsg is dispatched at the configured framerate while the
// visualizer is running. The root model re-emits the tick on each receipt so
// the animation continues. When the visualizer stops, no further ticks are
// queued.
type VisualizerTickMsg struct{}

// VisualizerErrMsg is dispatched once when the subprocess fails to start.
type VisualizerErrMsg struct{ Err error }

// ── Visualizer ────────────────────────────────────────────────────────────────

// Visualizer manages an external audio visualizer subprocess and exposes
// normalized bar heights for rendering inside the TUI.
//
// Safe for concurrent use. The pointer is shared across Model value copies
// (Bubble Tea clones Model on every Update).
type Visualizer struct {
	mu     sync.RWMutex
	cfg    VisualizerConfig
	bars   []float64     // normalized amplitudes 0.0–1.0, len = cfg.Bars
	cancel context.CancelFunc
	done   chan struct{}  // closed when the reader goroutine exits
}

// NewVisualizer creates a Visualizer in the stopped state.
func NewVisualizer(cfg VisualizerConfig) *Visualizer {
	n := clampInt(cfg.Bars, 1, 120)
	v := &Visualizer{cfg: cfg, bars: make([]float64, n)}
	v.done = make(chan struct{})
	close(v.done) // already "done" — nothing running
	return v
}

// Reconfigure atomically stops the current process, applies the new config,
// and restarts if the new backend is not Off.
// Returns a Cmd that fires VisualizerErrMsg if the subprocess fails to start.
func (v *Visualizer) Reconfigure(cfg VisualizerConfig) tea.Cmd {
	v.Stop()
	v.mu.Lock()
	v.cfg = cfg
	v.bars = make([]float64, clampInt(cfg.Bars, 1, 120))
	v.mu.Unlock()
	if cfg.Backend == VisualizerOff {
		return nil
	}
	if err := v.Start(); err != nil {
		return func() tea.Msg { return VisualizerErrMsg{Err: err} }
	}
	return v.TickCmd()
}

// Start spawns the visualizer subprocess and reads bar data in the background.
// Callers should check whether the backend binary is installed first.
func (v *Visualizer) Start() error {
	v.Stop()

	ctx, cancel := context.WithCancel(context.Background())
	v.cancel = cancel
	done := make(chan struct{})
	v.done = done

	v.mu.RLock()
	cfg := v.cfg
	v.mu.RUnlock()

	cmd, err := buildCmd(ctx, cfg)
	if err != nil {
		cancel()
		close(done)
		return err
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		cancel()
		close(done)
		return fmt.Errorf("visualizer: stdout pipe: %w", err)
	}
	if err := cmd.Start(); err != nil {
		cancel()
		close(done)
		return fmt.Errorf("visualizer: start %s: %w", cmd.Path, err)
	}

	bars := clampInt(cfg.Bars, 1, 120)
	go func() {
		defer close(done)
		defer cmd.Wait() //nolint:errcheck
		buf := make([]byte, bars)
		for {
			if _, err := io.ReadFull(stdout, buf); err != nil {
				return // context cancelled or process died
			}
			v.mu.Lock()
			for i, b := range buf {
				if i < len(v.bars) {
					v.bars[i] = float64(b) / 255.0
				}
			}
			v.mu.Unlock()
		}
	}()

	return nil
}

// Stop kills the subprocess and waits for the reader goroutine to exit.
// Safe to call when already stopped.
func (v *Visualizer) Stop() {
	if v.cancel != nil {
		v.cancel()
		v.cancel = nil
	}
	<-v.done // wait for goroutine
	// Reset bar heights so the display goes quiet immediately
	v.mu.Lock()
	for i := range v.bars {
		v.bars[i] = 0
	}
	v.mu.Unlock()
	// Re-initialize to a closed "done" so future Stop calls are safe
	v.done = make(chan struct{})
	close(v.done)
}

// IsRunning reports whether the subprocess is currently active.
func (v *Visualizer) IsRunning() bool {
	return v.cancel != nil
}

// Config returns the current config (read-safe).
func (v *Visualizer) Config() VisualizerConfig {
	v.mu.RLock()
	defer v.mu.RUnlock()
	return v.cfg
}

// TickCmd returns a Bubble Tea Cmd that fires VisualizerTickMsg once after one
// frame interval. The model should re-emit TickCmd on each receipt to create
// the animation loop.
func (v *Visualizer) TickCmd() tea.Cmd {
	fps := v.cfg.Framerate
	if fps <= 0 {
		fps = 20
	}
	d := time.Second / time.Duration(fps)
	return tea.Tick(d, func(time.Time) tea.Msg { return VisualizerTickMsg{} })
}

// ── Subprocess builders ───────────────────────────────────────────────────────

func buildCmd(ctx context.Context, cfg VisualizerConfig) (*exec.Cmd, error) {
	switch cfg.Backend {
	case VisualizerCava:
		return buildCavaCmd(ctx, cfg)
	case VisualizerChroma:
		return buildChromaCmd(ctx, cfg)
	}
	return nil, fmt.Errorf("visualizer: no backend selected")
}

func buildCavaCmd(ctx context.Context, cfg VisualizerConfig) (*exec.Cmd, error) {
	bars := clampInt(cfg.Bars, 1, 200)
	fps := clampInt(cfg.Framerate, 1, 200)
	method := cfg.InputMethod
	if method == "" {
		method = "pulse"
	}

	cfgContent := fmt.Sprintf(`[general]
bars = %d
framerate = %d

[input]
method = %s
source = auto

[output]
method = raw
raw_target = /dev/stdout
data_format = binary
bit_format = 8bit

[smoothing]
monstercat = 1
gravity = 100
`, bars, fps, method)

	cfgPath := filepath.Join(os.TempDir(), "stui-cava.conf")
	if err := os.WriteFile(cfgPath, []byte(cfgContent), 0600); err != nil {
		return nil, fmt.Errorf("cava: write config: %w", err)
	}
	return exec.CommandContext(ctx, "cava", "-p", cfgPath), nil
}

func buildChromaCmd(ctx context.Context, cfg VisualizerConfig) (*exec.Cmd, error) {
	// chroma (https://github.com/yuri-xyz/chroma) raw binary output.
	// Each frame is cfg.Bars bytes (0-255 per bar), same protocol as cava 8-bit raw.
	return exec.CommandContext(ctx, "chroma",
		"--output", "raw",
		"--bars", fmt.Sprintf("%d", clampInt(cfg.Bars, 1, 200)),
		"--fps", fmt.Sprintf("%d", clampInt(cfg.Framerate, 1, 200)),
	), nil
}

// ── Rendering ─────────────────────────────────────────────────────────────────

// blockRunes maps sub-cell fill amount (0–8 eighths) to Unicode block chars.
var blockRunes = []rune{' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'}

// RenderBars renders the current frequency bars into a multi-line lipgloss
// string ready for embedding in the MPD HUD. width is the available columns.
func (v *Visualizer) RenderBars(width int) string {
	v.mu.RLock()
	raw := make([]float64, len(v.bars))
	copy(raw, v.bars)
	cfg := v.cfg
	v.mu.RUnlock()

	if len(raw) == 0 || width < 3 {
		return ""
	}

	bars := raw

	// Symmetric: mirror the bar array around the centre
	if cfg.Symmetric {
		n := len(bars)
		mirrored := make([]float64, n*2)
		for i, b := range bars {
			mirrored[n-1-i] = b
			mirrored[n+i] = b
		}
		bars = mirrored
	}

	// Each bar occupies 2 columns (glyph + space); fit to available width
	maxBars := width / 2
	if maxBars < 1 {
		maxBars = 1
	}
	if len(bars) > maxBars {
		bars = bars[:maxBars]
	}

	height := clampInt(cfg.Height, 1, 40)

	// Build per-bar rune columns
	cols := make([][]rune, len(bars))
	for i, h := range bars {
		col := make([]rune, height)
		// Total sub-cell fill (0 to height*8 eighths)
		totalEighths := int(h * float64(height) * 8.0)
		for row := 0; row < height; row++ {
			// row 0 = top row, row height-1 = bottom row
			rowsBelow := height - 1 - row
			remaining := totalEighths - rowsBelow*8
			switch {
			case remaining <= 0:
				col[row] = ' '
			case remaining >= 8:
				col[row] = '█'
			default:
				col[row] = blockRunes[remaining]
			}
		}
		cols[i] = col
	}

	// Calculate left padding to centre the bars
	totalW := len(cols)*2 - 1
	padLeft := (width - totalW) / 2
	if padLeft < 0 {
		padLeft = 0
	}
	indent := strings.Repeat(" ", padLeft)

	// Render rows top-to-bottom
	accent := theme.T.Accent()
	dim := theme.T.TextDim()
	var sb strings.Builder

	for row := 0; row < height; row++ {
		sb.WriteString(indent)

		var rowStyle lipgloss.Style
		if cfg.Gradient {
			// Top rows → accent colour, bottom rows → dim
			frac := float64(height-1-row) / float64(height-1)
			if frac >= 0.45 {
				rowStyle = lipgloss.NewStyle().Foreground(accent)
			} else {
				rowStyle = lipgloss.NewStyle().Foreground(dim)
			}
		} else {
			rowStyle = lipgloss.NewStyle().Foreground(accent)
		}

		for j, col := range cols {
			sb.WriteString(rowStyle.Render(string(col[row])))
			if j < len(cols)-1 {
				sb.WriteRune(' ')
			}
		}
		sb.WriteRune('\n')
	}

	return sb.String()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

func clampInt(v, lo, hi int) int {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}
