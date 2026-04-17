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

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/pkg/theme"
)

// ── Config ─────────────────────────────────────────────────────────────────────

type VisualizerMode int

const (
	VisualizerModeBars   VisualizerMode = iota // standard frequency bars
	VisualizerModeMirror                       // mirrored bars (centered, symmetric)
	VisualizerModeFilled                       // filled bars with rounded tops
	VisualizerModeLED                          // LED-style discrete levels
	// CLIAMP-style FFT visualizers
	VisualizerModeWave      // oscilloscope waveform
	VisualizerModeScope     // XY oscilloscope
	VisualizerModeRetro     // 80s synthwave
	VisualizerModeMatrix    // Matrix rain
	VisualizerModeFlame     // rising flames
	VisualizerModePulse     // pulsating circle
	VisualizerModeBinary    // streaming binary
	VisualizerModeButterfly // mirrored Rorschach
	VisualizerModeTerrain   // scrolling mountains
	VisualizerModeSakura    // cherry blossoms
	VisualizerModeFirework  // exploding fireworks
	VisualizerModeGlitch    // digital glitch
	VisualizerModeLightning // electric bolts
	VisualizerModeRain      // falling rain
	VisualizerModeScatter   // particle sparkle
	VisualizerModeColumns   // thin columns
	VisualizerModeBricks    // brick wall
)

var visualizerModeStrings = map[VisualizerMode]string{
	VisualizerModeBars:      "bars",
	VisualizerModeMirror:    "mirror",
	VisualizerModeFilled:    "filled",
	VisualizerModeLED:       "led",
	VisualizerModeWave:      "wave",
	VisualizerModeScope:     "scope",
	VisualizerModeRetro:     "retro",
	VisualizerModeMatrix:    "matrix",
	VisualizerModeFlame:     "flame",
	VisualizerModePulse:     "pulse",
	VisualizerModeBinary:    "binary",
	VisualizerModeButterfly: "butterfly",
	VisualizerModeTerrain:   "terrain",
	VisualizerModeSakura:    "sakura",
	VisualizerModeFirework:  "firework",
	VisualizerModeGlitch:    "glitch",
	VisualizerModeLightning: "lightning",
	VisualizerModeRain:      "rain",
	VisualizerModeScatter:   "scatter",
	VisualizerModeColumns:   "columns",
	VisualizerModeBricks:    "bricks",
}

func (m VisualizerMode) String() string {
	if s, ok := visualizerModeStrings[m]; ok {
		return s
	}
	return "bars"
}

func VisualizerModeFromString(s string) VisualizerMode {
	switch strings.ToLower(s) {
	case "mirror":
		return VisualizerModeMirror
	case "filled":
		return VisualizerModeFilled
	case "led":
		return VisualizerModeLED
	case "wave":
		return VisualizerModeWave
	case "scope":
		return VisualizerModeScope
	case "retro":
		return VisualizerModeRetro
	case "matrix":
		return VisualizerModeMatrix
	case "flame":
		return VisualizerModeFlame
	case "pulse":
		return VisualizerModePulse
	case "binary":
		return VisualizerModeBinary
	case "butterfly":
		return VisualizerModeButterfly
	case "terrain":
		return VisualizerModeTerrain
	case "sakura":
		return VisualizerModeSakura
	case "firework":
		return VisualizerModeFirework
	case "glitch":
		return VisualizerModeGlitch
	case "lightning":
		return VisualizerModeLightning
	case "rain":
		return VisualizerModeRain
	case "scatter":
		return VisualizerModeScatter
	case "columns":
		return VisualizerModeColumns
	case "bricks":
		return VisualizerModeBricks
	default:
		return VisualizerModeBars
	}
}

// VisualizerBackend identifies which external tool drives the visualization.
type VisualizerBackend int

const (
	VisualizerOff    VisualizerBackend = iota // no visualizer
	VisualizerCliamp                          // built-in CLI FFT renderer
	VisualizerCava                            // cava --raw mode
	VisualizerChroma                          // chroma --output raw
)

// VisualizerConfig holds all user-configurable settings for the visualizer.
// These settings are TUI-local and not sent to the Rust runtime.
type VisualizerConfig struct {
	Backend     VisualizerBackend
	Bars        int            // number of frequency bars (10–60)
	Height      int            // visualizer height in terminal rows (4–20)
	Framerate   int            // target refresh rate in fps (10–60)
	Mode        VisualizerMode // visualization style
	Gradient    bool           // shade bars from accent (top) to dim (bottom)
	InputMethod string         // audio input method: "pulse" | "pipewire" | "alsa" | "fifo"
	FifoPath    string         // path to MPD FIFO (default /tmp/mpd.fifo)
	PeakHold    bool           // show peak hold indicators
}

// DefaultVisualizerConfig returns sensible defaults (visualizer off).
func DefaultVisualizerConfig() VisualizerConfig {
	return VisualizerConfig{
		Backend:     VisualizerOff,
		Bars:        20,
		Height:      8,
		Framerate:   20,
		Mode:        VisualizerModeBars,
		Gradient:    true,
		InputMethod: "pulse",
		PeakHold:    true,
	}
}

// BackendFromString parses a backend name (as stored in settings).
func BackendFromString(s string) VisualizerBackend {
	switch strings.ToLower(s) {
	case "cliamp":
		return VisualizerCliamp
	case "cava":
		return VisualizerCava
	case "chroma":
		return VisualizerChroma
	default:
		return VisualizerCliamp // default to cliamp
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

const peakHoldFrames = 30 // frames to hold peak before decaying

type Visualizer struct {
	mu            sync.RWMutex
	cfg           VisualizerConfig
	bars          []float64 // normalized amplitudes 0.0–1.0, len = cfg.Bars
	peaks         []float64 // peak hold values (decay over time)
	peakAge       []int     // frames since last peak update
	cancel        context.CancelFunc
	done          chan struct{}  // closed when the reader goroutine exits
	fftViz        *FftVisualizer // persistent FFT visualizer for CLIAMP modes
	runningCliamp bool           // true when cliamp backend is active
}

// NewVisualizer creates a Visualizer in the stopped state.
func NewVisualizer(cfg VisualizerConfig) *Visualizer {
	n := clampInt(cfg.Bars, 1, 120)
	v := &Visualizer{
		cfg:     cfg,
		bars:    make([]float64, n),
		peaks:   make([]float64, n),
		peakAge: make([]int, n),
		fftViz:  NewFftVisualizer(44100),
	}
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
	n := clampInt(cfg.Bars, 1, 120)
	v.bars = make([]float64, n)
	v.peaks = make([]float64, n)
	v.peakAge = make([]int, n)
	v.mu.Unlock()
	if cfg.Backend == VisualizerOff {
		return nil
	}
	if cfg.Backend == VisualizerCliamp {
		v.runningCliamp = true
		v.startFifoReader(cfg)
		return v.TickCmd()
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
					val := float64(b) / 255.0
					v.bars[i] = val
					if cfg.PeakHold {
						if val >= v.peaks[i] {
							v.peaks[i] = val
							v.peakAge[i] = 0
						} else {
							v.peakAge[i]++
							if v.peakAge[i] > peakHoldFrames {
								v.peaks[i] = val
							}
						}
					}
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
	v.runningCliamp = false
	<-v.done // wait for goroutine
	// Reset bar heights so the display goes quiet immediately
	v.mu.Lock()
	for i := range v.bars {
		v.bars[i] = 0
		v.peaks[i] = 0
		v.peakAge[i] = 0
	}
	v.mu.Unlock()
	// Re-initialize to a closed "done" so future Stop calls are safe
	v.done = make(chan struct{})
	close(v.done)
}

// startFifoReader opens the MPD FIFO and feeds PCM samples to the FFT
// analyzer in a background goroutine. The goroutine exits when Stop() is
// called (which sets runningCliamp = false and closes the done channel).
func (v *Visualizer) startFifoReader(cfg VisualizerConfig) {
	fifoPath := cfg.FifoPath
	if fifoPath == "" {
		fifoPath = "/tmp/mpd.fifo"
	}

	ctx, cancel := context.WithCancel(context.Background())
	v.cancel = cancel
	done := make(chan struct{})
	v.done = done

	go func() {
		defer close(done)

		f, err := os.OpenFile(fifoPath, os.O_RDONLY, 0)
		if err != nil {
			return
		}
		defer f.Close()

		// MPD FIFO: 16-bit signed LE, 2 channels, 44100 Hz.
		// Read 2048 frames (4 bytes each = 8192 bytes) per chunk.
		const frameSize = 4 // 2 bytes × 2 channels
		const chunkFrames = 2048
		buf := make([]byte, chunkFrames*frameSize)
		samples := make([]float64, chunkFrames)

		for {
			select {
			case <-ctx.Done():
				return
			default:
			}

			n, err := f.Read(buf)
			if err != nil || n < frameSize {
				continue
			}

			// Decode 16-bit signed LE stereo → mono float64
			nFrames := n / frameSize
			if nFrames > chunkFrames {
				nFrames = chunkFrames
			}
			for i := 0; i < nFrames; i++ {
				off := i * frameSize
				// Left channel (16-bit signed LE)
				left := int16(buf[off]) | int16(buf[off+1])<<8
				// Right channel
				right := int16(buf[off+2]) | int16(buf[off+3])<<8
				// Mix to mono, normalize to -1.0..1.0
				samples[i] = (float64(left) + float64(right)) / (2.0 * 32768.0)
			}

			// Feed to FFT analyzer and update bars
			bands := v.fftViz.Analyze(samples[:nFrames])
			v.mu.Lock()
			for i := 0; i < len(v.bars) && i < visNumBands; i++ {
				v.bars[i] = bands[i]
			}
			v.mu.Unlock()
		}
	}()
}

// IsRunning reports whether the subprocess is currently active.
func (v *Visualizer) IsRunning() bool {
	return v.cancel != nil || v.runningCliamp
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
	bars := clampInt(cfg.Bars, 1, 200)
	fps := clampInt(cfg.Framerate, 1, 200)

	args := []string{
		"--output", "raw",
		"--bars", fmt.Sprintf("%d", bars),
		"--fps", fmt.Sprintf("%d", fps),
	}

	if cfg.InputMethod != "" {
		switch cfg.InputMethod {
		case "alsa":
			args = append(args, "--backend", "alsa")
		case "pipewire":
			args = append(args, "--backend", "pipewire")
		}
	}

	return exec.CommandContext(ctx, "chroma", args...), nil
}

func IsChromaInstalled() bool {
	_, err := exec.LookPath("chroma")
	return err == nil
}

func IsCavaInstalled() bool {
	_, err := exec.LookPath("cava")
	return err == nil
}

// ── Rendering ─────────────────────────────────────────────────────────────────

var blockRunes = []rune{' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'}

var ledChars = []rune{' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'}

func (v *Visualizer) RenderBars(width int) string {
	v.mu.RLock()
	raw := make([]float64, len(v.bars))
	copy(raw, v.bars)
	peaks := make([]float64, len(v.peaks))
	copy(peaks, v.peaks)
	cfg := v.cfg
	v.mu.RUnlock()

	if len(raw) == 0 || width < 3 {
		return ""
	}

	bars := raw

	switch cfg.Mode {
	case VisualizerModeMirror:
		n := len(bars)
		mirrored := make([]float64, n*2)
		for i, b := range bars {
			mirrored[n-1-i] = b
			mirrored[n+i] = b
		}
		bars = mirrored
		peakMirrored := make([]float64, n*2)
		for i, p := range peaks {
			peakMirrored[n-1-i] = p
			peakMirrored[n+i] = p
		}
		peaks = peakMirrored
	}

	maxBars := width / 2
	if maxBars < 1 {
		maxBars = 1
	}
	if len(bars) > maxBars {
		bars = bars[:maxBars]
		peaks = peaks[:maxBars]
	}

	height := clampInt(cfg.Height, 1, 40)

	cols := make([][]rune, len(bars))
	peakCols := make([]bool, len(bars))

	for i, h := range bars {
		col := make([]rune, height)
		peakEighths := int(peaks[i] * float64(height) * 8.0)
		peakCols[i] = cfg.PeakHold && peakEighths > 0

		switch cfg.Mode {
		case VisualizerModeLED:
			for row := 0; row < height; row++ {
				threshold := float64(height-row) / float64(height)
				if h >= threshold {
					col[row] = '█'
				} else {
					col[row] = ' '
				}
			}
		default:
			totalEighths := int(h * float64(height) * 8.0)
			for row := 0; row < height; row++ {
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
		}
		cols[i] = col
	}

	totalW := len(cols)*2 - 1
	padLeft := (width - totalW) / 2
	if padLeft < 0 {
		padLeft = 0
	}
	indent := strings.Repeat(" ", padLeft)

	accent := theme.T.Accent()
	dim := theme.T.TextDim()
	gold := lipgloss.Color("#FFD700")
	var sb strings.Builder

	for row := 0; row < height; row++ {
		sb.WriteString(indent)

		for j, col := range cols {
			var rowStyle lipgloss.Style
			if cfg.Gradient {
				frac := float64(height-1-row) / float64(height-1)
				if frac >= 0.45 {
					rowStyle = lipgloss.NewStyle().Foreground(accent)
				} else {
					rowStyle = lipgloss.NewStyle().Foreground(dim)
				}
			} else {
				rowStyle = lipgloss.NewStyle().Foreground(accent)
			}

			r := col[row]
			if r != ' ' {
				if peakCols[j] {
					threshold := float64(height-row) / float64(height)
					if peaks[j] >= threshold && (row == 0 || col[row-1] == ' ') {
						rowStyle = lipgloss.NewStyle().Foreground(gold)
					}
				}
				sb.WriteString(rowStyle.Render(string(r)))
			} else {
				sb.WriteString(" ")
			}
			if j < len(cols)-1 {
				sb.WriteRune(' ')
			}
		}
		sb.WriteRune('\n')
	}

	return sb.String()
}

// RenderCliampStyle renders FFT-based visualizers (requires FFT data from runtime).
// This is called when VisualizerMode is one of the CLIAMP-style modes.
// The bands parameter should contain normalized FFT band data (0.0-1.0 per band).
func (v *Visualizer) RenderCliampStyle(bands [visNumBands]float64) string {
	v.mu.RLock()
	cfg := v.cfg
	fftViz := v.fftViz
	v.mu.RUnlock()

	fftViz.SetRows(clampInt(cfg.Height, 1, 20))

	switch cfg.Mode {
	case VisualizerModeWave:
		return fftViz.RenderWave()
	case VisualizerModeScope:
		return fftViz.RenderScope()
	case VisualizerModeRetro:
		return fftViz.RenderRetro(bands)
	case VisualizerModeMatrix:
		return fftViz.RenderMatrix(bands)
	case VisualizerModeFlame:
		return fftViz.RenderFlame(bands)
	case VisualizerModePulse:
		return fftViz.RenderPulse(bands)
	case VisualizerModeBinary:
		return fftViz.RenderBinary(bands)
	case VisualizerModeButterfly:
		return fftViz.RenderButterfly(bands)
	case VisualizerModeTerrain:
		return fftViz.RenderTerrain(bands)
	case VisualizerModeSakura:
		return fftViz.RenderSakura(bands)
	case VisualizerModeFirework:
		return fftViz.RenderFirework(bands)
	case VisualizerModeGlitch:
		return fftViz.RenderGlitch(bands)
	case VisualizerModeLightning:
		return fftViz.RenderLightning(bands)
	case VisualizerModeRain:
		return fftViz.RenderRain(bands)
	case VisualizerModeScatter:
		return fftViz.RenderScatter(bands)
	case VisualizerModeColumns:
		return fftViz.RenderColumns(bands)
	case VisualizerModeBricks:
		return fftViz.RenderBricks(bands)
	default:
		return v.RenderBars(panelWidth)
	}
}

// IsCliampMode returns true if the visualizer mode uses FFT-based rendering.
func (v *Visualizer) IsCliampMode() bool {
	v.mu.RLock()
	mode := v.cfg.Mode
	v.mu.RUnlock()

	return mode >= VisualizerModeWave
}

// Render dispatches to RenderBars or RenderCliampStyle depending on the active
// mode. Callers should use this instead of calling RenderBars directly.
func (v *Visualizer) Render(width int) string {
	if v.IsCliampMode() {
		return v.RenderCliampStyle(v.barsToBands())
	}
	return v.RenderBars(width)
}

// barsToBands maps the raw bar amplitudes from the backend subprocess into the
// 10-band array expected by the FFT visualizer renderers.
func (v *Visualizer) barsToBands() [visNumBands]float64 {
	v.mu.RLock()
	bars := v.bars
	v.mu.RUnlock()

	var out [visNumBands]float64
	n := len(bars)
	if n == 0 {
		return out
	}
	for i := range out {
		lo := i * n / visNumBands
		hi := (i + 1) * n / visNumBands
		if hi <= lo {
			hi = lo + 1
		}
		if hi > n {
			hi = n
		}
		var sum float64
		for _, b := range bars[lo:hi] {
			sum += b
		}
		out[i] = sum / float64(hi-lo)
	}
	return out
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
