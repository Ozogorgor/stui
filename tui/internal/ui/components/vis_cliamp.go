// Visualisation routines adapted from github.com/bjarneo/cliamp
// Copyright (c) bjarneo — MIT Licence

package components

import (
	"math"
	"math/cmplx"
	"strings"
	"sync"

	"charm.land/lipgloss/v2"
	"github.com/madelynnblue/go-dsp/fft"
)

const (
	visNumBands = 10
	visFFTSize  = 2048
	visNumRows  = 5
	panelWidth  = 74

	// dBRef is the normalisation reference for the band-level mapping.
	// Bands are scaled so that dBRef dBFS maps to 1.0.  Adjust if the
	// gain staging upstream changes significantly.
	dBRef = 60.0
)

var visBarBlocks = []rune{' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'}

var visBrailleBit = [4][2]rune{
	{0x01, 0x08},
	{0x02, 0x10},
	{0x04, 0x20},
	{0x40, 0x80},
}

var visBandEdges = [11]float64{20, 100, 200, 400, 800, 1600, 3200, 6400, 12800, 16000, 20000}

var (
	visLowStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("#4CAF50"))
	visMidStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("#FFC107"))
	visHighStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#F44336"))
)

type VisBandWidthFunc func(b int) int

func defaultVisBandWidth(b int) int {
	const gap = 1
	base := (panelWidth - (visNumBands-1)*gap) / visNumBands
	extra := (panelWidth - (visNumBands-1)*gap) % visNumBands
	if b < extra {
		return base + 1
	}
	return base
}

func visFracBlock(level float64, rowBottom, rowTop float64) string {
	if level >= rowTop {
		return "█"
	}
	if level > rowBottom {
		frac := (level - rowBottom) / (rowTop - rowBottom)
		idx := int(frac * float64(len(visBarBlocks)-1))
		if idx < 0 {
			idx = 0
		}
		if idx > len(visBarBlocks)-1 {
			idx = len(visBarBlocks) - 1
		}
		return string(visBarBlocks[idx])
	}
	return " "
}

func visStyleForRow(rowBottom float64) lipgloss.Style {
	switch {
	case rowBottom >= 0.6:
		return visHighStyle
	case rowBottom >= 0.3:
		return visMidStyle
	default:
		return visLowStyle
	}
}

type FftVisualizer struct {
	mu             sync.RWMutex // protects mutable state
	prev           [visNumBands]float64
	sr             float64
	buf            []float64
	waveBuf        []float64
	frame          uint64
	rows           int
	width          int // panel width in columns (set before each render)
	bandWidth      VisBandWidthFunc
	terrainHistory []float64 // persistent for terrain smoothing
}

func NewFftVisualizer(sampleRate float64) *FftVisualizer {
	w := panelWidth // initial default; overridden by SetWidth before render
	terrainHistory := make([]float64, w)
	for i := range terrainHistory {
		terrainHistory[i] = 0.5
	}
	return &FftVisualizer{
		sr:             sampleRate,
		buf:            make([]float64, visFFTSize),
		waveBuf:        make([]float64, visFFTSize),
		rows:           visNumRows,
		width:          w,
		bandWidth:      defaultVisBandWidth,
		terrainHistory: terrainHistory,
	}
}

func (v *FftVisualizer) SetWidth(w int) {
	if w < 10 {
		w = 10
	}
	v.mu.Lock()
	if w != v.width {
		v.width = w
		// Resize terrain history to match new width
		th := make([]float64, w)
		copy(th, v.terrainHistory)
		for i := len(v.terrainHistory); i < w; i++ {
			th[i] = 0.5
		}
		v.terrainHistory = th
		// Update band width function to use new width
		v.bandWidth = func(b int) int {
			const gap = 1
			base := (w - (visNumBands-1)*gap) / visNumBands
			extra := (w - (visNumBands-1)*gap) % visNumBands
			if b < extra {
				return base + 1
			}
			return base
		}
	}
	v.mu.Unlock()
}

func (v *FftVisualizer) SetRows(rows int) {
	v.mu.Lock()
	defer v.mu.Unlock()
	if rows < 1 {
		rows = 1
	}
	if rows > 20 {
		rows = 20
	}
	v.rows = rows
}

func (v *FftVisualizer) Analyze(samples []float64) [visNumBands]float64 {
	v.mu.Lock()
	defer v.mu.Unlock()

	v.frame++

	if len(samples) > 0 {
		if cap(v.waveBuf) >= len(samples) {
			v.waveBuf = v.waveBuf[:len(samples)]
		} else {
			v.waveBuf = make([]float64, len(samples))
		}
		copy(v.waveBuf, samples)
	} else {
		v.waveBuf = v.waveBuf[:0]
	}

	var bands [visNumBands]float64
	if len(samples) == 0 {
		for b := range visNumBands {
			bands[b] = v.prev[b] * 0.8
			v.prev[b] = bands[b]
		}
		return bands
	}

	n := copy(v.buf, samples)
	for i := n; i < visFFTSize; i++ {
		v.buf[i] = 0
	}

	win := hannWindow()
	for i := range visFFTSize {
		v.buf[i] *= win[i]
	}

	spectrum := fft.FFTReal(v.buf)
	halfLen := len(spectrum) / 2

	binHz := v.sr / float64(visFFTSize)

	for b := range visNumBands {
		loIdx := int(visBandEdges[b] / binHz)
		hiIdx := int(visBandEdges[b+1] / binHz)
		if loIdx < 1 {
			loIdx = 1
		}
		if hiIdx >= halfLen {
			hiIdx = halfLen - 1
		}

		// Power-average across bins: sum |X|² then convert to dB.
		// This correctly weights wide bands relative to narrow ones.
		var sumPow float64
		count := 0
		for i := loIdx; i <= hiIdx; i++ {
			m := cmplx.Abs(spectrum[i])
			sumPow += m * m
			count++
		}
		if count > 0 {
			sumPow /= float64(count)
		}

		// Map power to [0, 1].  Reference: dBRef dBFS at the FFT output
		// (after Hann window, N=2048) maps to 1.0.  Values below the noise
		// floor (-dBRef dB) clamp to 0.
		if sumPow > 0 {
			db := 10*math.Log10(sumPow) - math.Log10(float64(visFFTSize)/2)*10
			bands[b] = (db + dBRef) / dBRef
		}
		bands[b] = math.Max(0, math.Min(1, bands[b]))

		if bands[b] > v.prev[b] {
			bands[b] = bands[b]*0.6 + v.prev[b]*0.4
		} else {
			bands[b] = bands[b]*0.25 + v.prev[b]*0.75
		}
		v.prev[b] = bands[b]
	}

	return bands
}

// hannWindow returns a read-only slice into a package-level Hann window cache.
// Calling code must not modify the returned slice.
var (
	hannWindowCache [visFFTSize]float64
	hannWindowOnce  sync.Once
)

func hannWindow() []float64 {
	hannWindowOnce.Do(func() {
		for i := 0; i < visFFTSize; i++ {
			hannWindowCache[i] = 0.5 * (1 - math.Cos(2*math.Pi*float64(i)/float64(visFFTSize-1)))
		}
	})
	return hannWindowCache[:]
}

func (v *FftVisualizer) RenderBars(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for row := range height {
		var content strings.Builder
		rowBottom := float64(height-1-row) / float64(height)
		rowTop := float64(height-row) / float64(height)

		for i, level := range bands {
			bw := v.bandWidth(i)
			block := visFracBlock(level, rowBottom, rowTop)
			for range bw {
				content.WriteString(block)
			}
			if i < visNumBands-1 {
				content.WriteByte(' ')
			}
		}
		lines[row] = visStyleForRow(rowBottom).Render(content.String())
	}

	return strings.Join(lines, "\n")
}

func (v *FftVisualizer) RenderWave() string {
	v.mu.RLock()
	waveBuf := make([]float64, len(v.waveBuf))
	copy(waveBuf, v.waveBuf)
	height := v.rows
	v.mu.RUnlock()
	if len(waveBuf) == 0 {
		return strings.Repeat(" ", v.width)
	}

	var lines []string
	for row := 0; row < height; row++ {
		var b strings.Builder
		for col := 0; col < v.width; col++ {
			idx := (col * len(waveBuf)) / v.width
			if idx >= len(waveBuf) {
				idx = len(waveBuf) - 1
			}
			val := waveBuf[idx]
			sampleRow := int((val + 1) / 2 * float64(height-1))
			if sampleRow == height-1-row {
				b.WriteRune('█')
			} else if sampleRow == height-2-row && row < height-1 {
				b.WriteRune('▄')
			} else {
				b.WriteRune(' ')
			}
		}
		lines = append(lines, b.String())
	}

	var result strings.Builder
	for i, line := range lines {
		result.WriteString(visStyleForRow(float64(i) / float64(height)).Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderScope() string {
	v.mu.RLock()
	waveBuf := make([]float64, len(v.waveBuf))
	copy(waveBuf, v.waveBuf)
	height := v.rows
	v.mu.RUnlock()
	if len(waveBuf) < 2 {
		return strings.Repeat(" ", v.width)
	}

	lines := make([]string, height)
	for i := range lines {
		lines[i] = strings.Repeat(" ", v.width)
	}

	numPoints := len(waveBuf)
	if numPoints > v.width {
		numPoints = v.width
	}
	step := len(waveBuf) / numPoints

	for i := 0; i < numPoints; i++ {
		idx := i * step
		if idx >= len(waveBuf) {
			break
		}
		val := waveBuf[idx]
		y := int((val + 1) / 2 * float64(height-1))
		y = height - 1 - y
		if y >= 0 && y < height {
			old := lines[y]
			if i < len(old) {
				runes := []rune(old)
				runes[i] = '█'
				lines[y] = string(runes)
			}
		}
	}

	var result strings.Builder
	for i, line := range lines {
		result.WriteString(visStyleForRow(float64(i) / float64(height)).Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderRetro(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	var lines []string

	for row := 0; row < height; row++ {
		var b strings.Builder
		rowPos := float64(height - 1 - row)

		for col := 0; col < v.width; col++ {
			bandIdx := (col * visNumBands) / v.width
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			level := bands[bandIdx]
			barHeight := int(level * float64(height))
			isBar := rowPos < float64(barHeight)

			distFromCenter := float64(visAbs(col - v.width/2))
			perspective := 1.0 - (distFromCenter/float64(v.width/2))*0.3

			if isBar && perspective > 0.3 {
				b.WriteRune('█')
			} else if row == height-1 {
				if col%4 == 0 {
					b.WriteRune('·')
				} else {
					b.WriteRune('─')
				}
			} else {
				gradient := (rowPos + distFromCenter*0.5) / float64(height)
				if gradient > 0.6 {
					b.WriteRune(' ')
				} else if gradient > 0.3 {
					b.WriteRune('░')
				} else {
					b.WriteRune('▒')
				}
			}
		}
		lines = append(lines, b.String())
	}

	var result strings.Builder
	for i, line := range lines {
		result.WriteString(visStyleForRow(float64(i) / float64(height)).Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderMatrix(bands [visNumBands]float64) string {
	v.mu.RLock()
	frame := int(v.frame)
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	matrixChars := []rune{'ア', 'カ', 'サ', 'タ', 'ナ', 'ハ', 'マ', 'ヤ', 'ラ', 'ワ', '0', '1'}

	for row := 0; row < height; row++ {
		var b strings.Builder
		for col := 0; col < v.width; col++ {
			bandIdx := (col * visNumBands) / v.width
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			energy := bands[bandIdx]
			threshold := 0.3 + 0.7*energy

			if (frame+row+col)%7 == 0 {
				b.WriteRune(matrixChars[(col+row)%len(matrixChars)])
			} else if float64(row)/float64(height) < threshold {
				idx := ((frame / 3) + col + row) % len(matrixChars)
				b.WriteRune(matrixChars[idx])
			} else {
				b.WriteRune(' ')
			}
		}
		lines[row] = b.String()
	}

	var result strings.Builder
	green := lipgloss.NewStyle().Foreground(lipgloss.Color("#00FF00"))
	for i, line := range lines {
		result.WriteString(green.Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderFlame(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	flameChars := []rune{' ', ' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█', '█', '▓', '▒', '░'}

	for row := 0; row < height; row++ {
		var b strings.Builder

		for col := 0; col < v.width; col++ {
			bandIdx := (col * visNumBands) / v.width
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			energy := bands[bandIdx]
			flameHeight := int(energy * float64(height) * 1.5)
			if flameHeight > len(flameChars)-1 {
				flameHeight = len(flameChars) - 1
			}

			distFromCenter := float64(visAbs(col - v.width/2))
			fade := 1.0 - (distFromCenter/float64(v.width/2))*0.6

			actualHeight := int(float64(flameHeight) * fade)
			rowFromBottom := height - 1 - row

			if rowFromBottom <= actualHeight && rowFromBottom >= 0 {
				idx := rowFromBottom
				if idx > len(flameChars)-1 {
					idx = len(flameChars) - 1
				}
				b.WriteRune(flameChars[idx])
			} else {
				b.WriteRune(' ')
			}
		}

		rowStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("#FF4500"))
		if row > height/2 {
			rowStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#FFD700"))
		}
		if row < height/3 {
			rowStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#FF0000"))
		}
		lines[row] = rowStyle.Render(b.String())
	}

	return strings.Join(lines, "\n")
}

func (v *FftVisualizer) RenderPulse(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	centerX := v.width / 2
	centerY := height / 2

	var lines []string
	for range height {
		lines = append(lines, strings.Repeat(" ", v.width))
	}

	var totalEnergy float64
	for _, b := range bands {
		totalEnergy += b
	}
	avgEnergy := totalEnergy / float64(visNumBands)

	// Cap radius to avoid O(360×radius) blowup at high energy levels.
	// Maximum useful radius is half the smaller dimension.
	maxRadius := centerX - 2
	if centerY-2 < maxRadius {
		maxRadius = centerY - 2
	}
	if maxRadius < 2 {
		maxRadius = 2
	}
	radius := int(avgEnergy * float64(maxRadius))
	if radius < 2 {
		radius = 2
	}

	// Draw perimeter only (plus one inner ring for thickness), not filled,
	// to keep cost at O(360) regardless of radius.
	for angle := 0; angle < 360; angle += 2 {
		rad := float64(angle) * math.Pi / 180
		for _, r := range []int{radius, radius - 1} {
			if r < 1 {
				continue
			}
			x := centerX + int(float64(r)*math.Cos(rad))
			y := centerY - int(float64(r)*math.Sin(rad))
			if x >= 0 && x < v.width && y >= 0 && y < height {
				runes := []rune(lines[y])
				if (angle/2+r)%2 == 0 {
					runes[x] = '█'
				} else {
					runes[x] = '▒'
				}
				lines[y] = string(runes)
			}
		}
	}

	var result strings.Builder
	for i, line := range lines {
		result.WriteString(visStyleForRow(float64(i) / float64(height)).Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderBinary(bands [visNumBands]float64) string {
	v.mu.RLock()
	frame := int(v.frame)
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for row := 0; row < height; row++ {
		var b strings.Builder
		for col := 0; col < v.width; col++ {
			bandIdx := (col * visNumBands) / v.width
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			energy := bands[bandIdx]
			threshold := 0.2 + 0.6*energy

			isOne := ((frame + row + col) % 3) == 0

			if isOne && float64(row)/float64(height) < threshold {
				b.WriteRune('1')
			} else if !isOne && float64(row)/float64(height) < threshold*0.8 {
				b.WriteRune('0')
			} else {
				b.WriteRune(' ')
			}
		}
		lines[row] = b.String()
	}

	var result strings.Builder
	cyan := lipgloss.NewStyle().Foreground(lipgloss.Color("#00FFFF"))
	for i, line := range lines {
		result.WriteString(cyan.Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderButterfly(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for row := range height {
		var b strings.Builder
		rowBottom := float64(height-1-row) / float64(height)
		rowTop := float64(height-row) / float64(height)

		for col := 0; col < v.width; col++ {
			mirrorCol := v.width - 1 - col
			lo := col
			if mirrorCol < lo {
				lo = mirrorCol
			}
			bandIdx := (lo * visNumBands) / (v.width / 2)
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			level := bands[bandIdx]
			block := visFracBlock(level, rowBottom, rowTop)
			b.WriteString(block)
		}
		lines[row] = visStyleForRow(rowBottom).Render(b.String())
	}

	return strings.Join(lines, "\n")
}

func (v *FftVisualizer) RenderTerrain(bands [visNumBands]float64) string {
	// Capture height once under a single write lock alongside the terrain update
	// so that height and terrainHistory are always consistent.
	v.mu.Lock()
	height := v.rows
	for col := 0; col < v.width; col++ {
		bandIdx := (col * visNumBands) / v.width
		if bandIdx >= visNumBands {
			bandIdx = visNumBands - 1
		}
		target := bands[bandIdx]
		v.terrainHistory[col] = v.terrainHistory[col]*0.7 + target*0.3
	}
	historyCopy := make([]float64, v.width)
	copy(historyCopy, v.terrainHistory)
	v.mu.Unlock()

	lines := make([]string, height)

	for row := 0; row < height; row++ {
		var b strings.Builder
		rowY := float64(height - 1 - row)

		for col := 0; col < v.width; col++ {
			terrainH := historyCopy[col] * float64(height-2)
			if rowY <= terrainH {
				if rowY > terrainH-1 {
					b.WriteRune('▄')
				} else {
					b.WriteRune('█')
				}
			} else if rowY < 2 {
				b.WriteRune('~')
			} else {
				b.WriteRune(' ')
			}
		}

		rowStyle := visStyleForRow(float64(row) / float64(height))
		lines[row] = rowStyle.Render(b.String())
	}

	return strings.Join(lines, "\n")
}

func (v *FftVisualizer) RenderSakura(bands [visNumBands]float64) string {
	v.mu.RLock()
	frame := int(v.frame)
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for i := range height {
		lines[i] = strings.Repeat(" ", v.width)
	}

	var totalEnergy float64
	for _, b := range bands {
		totalEnergy += b
	}
	avgEnergy := totalEnergy / float64(visNumBands)
	numParticles := 5 + int(avgEnergy*20) // 5–25 particles based on energy

	sakura := []rune{'✿', '❀', '✾', '❃', '·'}

	// Each particle gets a per-index LCG seed so trajectories are independent
	// and organic rather than all advancing at the same rate.
	for i := 0; i < numParticles; i++ {
		seed := uint32(i)*2654435761 + 1
		px := int(seed>>17) % v.width
		py := (frame/2 + int(seed>>23)%height + i*3) % height
		age := (frame + i) % 20

		if py >= 0 && py < height && px >= 0 && px < v.width {
			runes := []rune(lines[py])
			if age < 10 {
				runes[px] = sakura[age%len(sakura)]
			} else {
				runes[px] = '·'
			}
			lines[py] = string(runes)
		}
	}

	var result strings.Builder
	pink := lipgloss.NewStyle().Foreground(lipgloss.Color("#FFB7C5"))
	for i, line := range lines {
		result.WriteString(pink.Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderFirework(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for i := range height {
		lines[i] = strings.Repeat(" ", v.width)
	}

	var totalEnergy float64
	for _, b := range bands {
		totalEnergy += b
	}

	// Lower threshold so the firework shows at moderate energy levels too.
	// Idle animation (slow pulse) plays when quiet.
	const burstThreshold = 0.8
	if totalEnergy > burstThreshold {
		centerX := v.width / 2
		centerY := height / 2
		maxR := centerX - 2
		if centerY-2 < maxR {
			maxR = centerY - 2
		}
		radius := int((totalEnergy - burstThreshold) / (float64(visNumBands) - burstThreshold) * float64(maxR))
		if radius < 1 {
			radius = 1
		}

		for angle := 0; angle < 360; angle += 15 {
			rad := float64(angle) * math.Pi / 180
			for r := 0; r <= radius; r++ {
				x := centerX + int(float64(r)*math.Cos(rad))
				y := centerY - int(float64(r)*math.Sin(rad))
				if x >= 0 && x < v.width && y >= 0 && y < height {
					runes := []rune(lines[y])
					runes[x] = '✦'
					lines[y] = string(runes)
				}
			}
		}
	} else {
		// Idle: single slow-breathing dot at centre
		cx := v.width / 2
		cy := height / 2
		if cy >= 0 && cy < height && cx >= 0 && cx < v.width {
			runes := []rune(lines[cy])
			runes[cx] = '·'
			lines[cy] = string(runes)
		}
	}

	var result strings.Builder
	white := lipgloss.NewStyle().Foreground(lipgloss.Color("#FFFFFF"))
	for i, line := range lines {
		result.WriteString(white.Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderGlitch(bands [visNumBands]float64) string {
	v.mu.RLock()
	frame := int(v.frame)
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for row := range height {
		rowChars := make([]rune, v.width)
		for i := range rowChars {
			rowChars[i] = ' '
		}

		rowBottom := float64(height-1-row) / float64(height)
		rowTop := float64(height-row) / float64(height)

		isGlitchRow := (frame+row)%8 == 0

		for col := 0; col < v.width; col++ {
			bandIdx := (col * visNumBands) / v.width
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			level := bands[bandIdx]

			displayCol := col
			if isGlitchRow && (frame%3) == 0 {
				offset := (frame + col) % 7
				displayCol = col + offset - 3
			}

			if displayCol >= 0 && displayCol < v.width {
				block := visFracBlock(level, rowBottom, rowTop)
				if isGlitchRow && (frame%5) == 0 && col%4 == 0 {
					rowChars[displayCol] = '▓'
				} else if len(block) > 0 {
					rowChars[displayCol] = []rune(block)[0]
				}
			}
		}
		lines[row] = string(rowChars)
	}

	var result strings.Builder
	magenta := lipgloss.NewStyle().Foreground(lipgloss.Color("#FF00FF"))
	cyan := lipgloss.NewStyle().Foreground(lipgloss.Color("#00FFFF"))

	for i, line := range lines {
		if i%2 == 0 {
			result.WriteString(magenta.Render(line))
		} else {
			result.WriteString(cyan.Render(line))
		}
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderLightning(bands [visNumBands]float64) string {
	v.mu.RLock()
	frame := int(v.frame)
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for i := range height {
		lines[i] = strings.Repeat(" ", v.width)
	}

	trebleEnergy := bands[visNumBands-1] + bands[visNumBands-2]

	if trebleEnergy > 0.5 {
		startX := v.width/2 + (frame%20 - 10)

		points := []struct{ x, y int }{{startX, 0}}
		currentX := startX
		for y := 1; y < height; y++ {
			offset := (frame + y) % 7
			currentX += offset - 3
			if currentX < 2 {
				currentX = 2
			}
			if currentX > v.width-3 {
				currentX = v.width - 3
			}
			points = append(points, struct{ x, y int }{currentX, y})
		}

		for _, p := range points {
			if p.y < height && p.x >= 0 && p.x < v.width {
				runes := []rune(lines[p.y])
				runes[p.x] = '⚡'
				if p.x+1 < v.width {
					runes[p.x+1] = 'ϟ'
				}
				lines[p.y] = string(runes)
			}
		}
	}

	var result strings.Builder
	yellow := lipgloss.NewStyle().Foreground(lipgloss.Color("#FFFF00"))
	for i, line := range lines {
		result.WriteString(yellow.Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderRain(bands [visNumBands]float64) string {
	v.mu.RLock()
	frame := int(v.frame)
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for row := 0; row < height; row++ {
		var b strings.Builder

		for col := 0; col < v.width; col++ {
			bandIdx := (col * visNumBands) / v.width
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			energy := bands[bandIdx]
			speed := 2 + int(energy*3)

			pos := (frame*speed + col*2) % (height * 3)
			dropRow := height - 1 - (pos % height)

			if row == dropRow {
				b.WriteRune('│')
			} else if row == dropRow-1 && energy > 0.3 {
				b.WriteRune('·')
			} else {
				b.WriteRune(' ')
			}
		}
		lines[row] = b.String()
	}

	var result strings.Builder
	blue := lipgloss.NewStyle().Foreground(lipgloss.Color("#4169E1"))
	for i, line := range lines {
		result.WriteString(blue.Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderScatter(bands [visNumBands]float64) string {
	v.mu.RLock()
	frame := int(v.frame)
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	for i := range height {
		lines[i] = strings.Repeat(" ", v.width)
	}

	for col := 0; col < v.width; col++ {
		bandIdx := (col * visNumBands) / v.width
		if bandIdx >= visNumBands {
			bandIdx = visNumBands - 1
		}

		energy := bands[bandIdx]
		numDots := int(energy * float64(height/2))

		for d := 0; d < numDots; d++ {
			offset := ((frame + col*7 + d*13) * 17) % (height * 3)
			y := offset % height

			if y < len(lines) && col < len(lines[y]) {
				runes := []rune(lines[y])
				dotChar := '·'
				if (frame+d)%3 == 0 {
					dotChar = '•'
				} else if (frame+d)%5 == 0 {
					dotChar = '°'
				}
				runes[col] = dotChar
				lines[y] = string(runes)
			}
		}
	}

	var result strings.Builder
	for i, line := range lines {
		result.WriteString(visStyleForRow(float64(i) / float64(height)).Render(line))
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

func (v *FftVisualizer) RenderColumns(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	cols := visNumBands
	colWidth := v.width / cols
	extra := v.width % cols

	for row := 0; row < height; row++ {
		var b strings.Builder

		for c := 0; c < cols; c++ {
			bandIdx := c
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			level := bands[bandIdx]
			barHeight := int(level * float64(height))

			w := colWidth
			if c < extra {
				w++
			}

			if height-1-row < barHeight {
				for i := 0; i < w; i++ {
					b.WriteRune('▎')
				}
			} else {
				for i := 0; i < w; i++ {
					b.WriteRune(' ')
				}
			}
		}
		lines[row] = visStyleForRow(float64(row) / float64(height)).Render(b.String())
	}

	return strings.Join(lines, "\n")
}

func (v *FftVisualizer) RenderBricks(bands [visNumBands]float64) string {
	v.mu.RLock()
	height := v.rows
	v.mu.RUnlock()
	lines := make([]string, height)

	brickChars := []rune{' ', '▌', '▐', '█'}

	for row := 0; row < height; row++ {
		var b strings.Builder

		offset := 0
		if row%2 == 1 {
			offset = 2
		}

		for col := 0; col < v.width; col++ {
			realCol := col - offset
			bandIdx := (realCol * visNumBands) / v.width
			if bandIdx < 0 {
				bandIdx = 0
			}
			if bandIdx >= visNumBands {
				bandIdx = visNumBands - 1
			}

			level := bands[bandIdx]
			brickHeight := int(level * float64(height) * 0.8)
			rowFromBottom := height - 1 - row

			if rowFromBottom < brickHeight {
				charIdx := 3
				if col%8 == 0 || col%8 == 7 {
					charIdx = 1
				} else if col%8 == 1 || col%8 == 6 {
					charIdx = 2
				}
				b.WriteRune(brickChars[charIdx])
			} else if rowFromBottom == brickHeight && brickHeight > 0 {
				b.WriteRune('─')
			} else {
				b.WriteRune(' ')
			}
		}
		lines[row] = b.String()
	}

	var result strings.Builder
	brown := lipgloss.NewStyle().Foreground(lipgloss.Color("#8B4513"))
	red := lipgloss.NewStyle().Foreground(lipgloss.Color("#A52A2A"))

	for i, line := range lines {
		if i%3 == 0 {
			result.WriteString(red.Render(line))
		} else {
			result.WriteString(brown.Render(line))
		}
		if i < len(lines)-1 {
			result.WriteRune('\n')
		}
	}
	return result.String()
}

// visAbs returns the absolute value of n.  Named visAbs to avoid shadowing
// the Go 1.21 built-in min/max and potential future abs built-in.
func visAbs(n int) int {
	if n < 0 {
		return -n
	}
	return n
}
