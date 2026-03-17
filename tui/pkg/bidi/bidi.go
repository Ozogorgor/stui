// Package bidi provides Unicode bidirectional text utilities for the stui TUI.
//
// # Modes
//
//   - "off"   — no processing; text rendered as-is
//   - "auto"  — detect directionality and adjust lipgloss alignment only;
//               relies on terminal/font BiDi rendering (default)
//   - "force" — full in-app visual reordering via golang.org/x/text/unicode/bidi
//               for terminals that don't do BiDi (Alacritty, tmux, etc.)
//
// # Usage
//
//	bidi.SetMode("auto")
//
//	// Align a lipgloss style to match text direction
//	style = bidi.AlignedStyle(style, title)
//
//	// Truncate a string respecting display width and direction
//	display := bidi.Truncate(title, maxWidth)
package bidi

import (
	"strings"
	"sync/atomic"
	"unsafe"

	"github.com/mattn/go-runewidth"
	"golang.org/x/text/unicode/bidi"

	"github.com/charmbracelet/lipgloss"
)

// Mode controls how bidirectional text is handled.
type Mode string

const (
	ModeOff   Mode = "off"
	ModeAuto  Mode = "auto"
	ModeForce Mode = "force"
)

// global holds the current mode string as a pointer for lock-free reads.
var globalMode unsafe.Pointer

func init() {
	SetMode(ModeAuto)
}

// SetMode updates the global BiDi mode. Safe to call from any goroutine.
func SetMode(m Mode) {
	s := string(m)
	atomic.StorePointer(&globalMode, unsafe.Pointer(&s))
}

// CurrentMode returns the active BiDi mode.
func CurrentMode() Mode {
	p := atomic.LoadPointer(&globalMode)
	return Mode(*(*string)(p))
}

// ── Direction detection ───────────────────────────────────────────────────────

// IsRTL reports whether the rune has a strong RTL direction.
func IsRTL(r rune) bool {
	p, _ := bidi.LookupRune(r)
	switch p.Class() {
	case bidi.R, bidi.AL, bidi.AN:
		return true
	}
	return false
}

// IsParagraphRTL reports whether the first strong directional character in s
// is RTL. Returns false for empty / purely neutral strings.
func IsParagraphRTL(s string) bool {
	for _, r := range s {
		p, _ := bidi.LookupRune(r)
		switch p.Class() {
		case bidi.R, bidi.AL, bidi.AN:
			return true
		case bidi.L:
			return false
		}
	}
	return false
}

// ── Visual reordering (force mode) ───────────────────────────────────────────

// Reorder applies the Unicode Bidirectional Algorithm to s and returns a
// visually ordered string suitable for left-to-right terminal output.
// Only has an effect in ModeForce; otherwise s is returned unchanged.
func Reorder(s string) string {
	if CurrentMode() != ModeForce || s == "" {
		return s
	}

	para := bidi.Paragraph{}
	para.SetString(s)
	ordering, err := para.Order()
	if err != nil {
		return s
	}

	var b strings.Builder
	b.Grow(len(s))
	for i := 0; i < ordering.NumRuns(); i++ {
		run := ordering.Run(i)
		// Run.String() returns the run text; RTL runs have been reversed.
		b.WriteString(run.String())
	}
	return b.String()
}

// Apply returns the text ready for display:
//   - ModeOff: returns s unchanged
//   - ModeAuto: adds Unicode directional marks for proper embedding
//   - ModeForce: returns visually reordered text
func Apply(s string) string {
	if s == "" {
		return s
	}
	switch CurrentMode() {
	case ModeOff:
		return s
	case ModeForce:
		return Reorder(s)
	default: // auto
		if IsParagraphRTL(s) {
			// Wrap in RLI/PDI so the terminal handles embedding correctly.
			return "\u2067" + s + "\u2069" // RLI … PDI
		}
		return s
	}
}

// ── Width-aware string operations ─────────────────────────────────────────────

// Truncate shortens s to at most maxWidth display cells, appending "…" when
// truncated. It is direction-aware: RTL text is truncated from the left.
func Truncate(s string, maxWidth int) string {
	if maxWidth <= 0 {
		return ""
	}
	w := runewidth.StringWidth(s)
	if w <= maxWidth {
		return s
	}

	ellipsis := "…"
	ellipsisW := 1
	limit := maxWidth - ellipsisW
	if limit <= 0 {
		return ellipsis
	}

	if IsParagraphRTL(s) {
		// For RTL text truncate from the start (left side).
		runes := []rune(s)
		total := 0
		cut := len(runes)
		for i := len(runes) - 1; i >= 0; i-- {
			cw := runewidth.RuneWidth(runes[i])
			if total+cw > limit {
				cut = i + 1
				break
			}
			total += cw
			cut = i
		}
		return ellipsis + string(runes[cut:])
	}

	// LTR: truncate from the end.
	var b strings.Builder
	b.Grow(maxWidth)
	total := 0
	for _, r := range s {
		cw := runewidth.RuneWidth(r)
		if total+cw > limit {
			break
		}
		b.WriteRune(r)
		total += cw
	}
	return b.String() + ellipsis
}

// WordWrap wraps s at maxWidth display cells, respecting word boundaries.
// For RTL paragraphs each line is individually reordered when in ModeForce.
func WordWrap(s string, maxWidth int) []string {
	if maxWidth <= 0 {
		return []string{s}
	}

	rtl := IsParagraphRTL(s)
	words := strings.Fields(s)
	var lines []string
	var cur strings.Builder
	curW := 0

	flush := func() {
		line := strings.TrimSpace(cur.String())
		if CurrentMode() == ModeForce && rtl {
			line = Reorder(line)
		}
		lines = append(lines, line)
		cur.Reset()
		curW = 0
	}

	for i, word := range words {
		ww := runewidth.StringWidth(word)
		if curW == 0 {
			cur.WriteString(word)
			curW = ww
		} else if curW+1+ww <= maxWidth {
			cur.WriteByte(' ')
			cur.WriteString(word)
			curW += 1 + ww
		} else {
			flush()
			cur.WriteString(word)
			curW = ww
		}
		if i == len(words)-1 {
			flush()
		}
	}

	if len(lines) == 0 {
		return []string{""}
	}
	return lines
}

// ── Lipgloss helpers ──────────────────────────────────────────────────────────

// AlignedStyle returns a copy of style with horizontal alignment set to match
// the dominant direction of text. RTL → Right; LTR → Left. In ModeOff the
// style is returned unchanged.
func AlignedStyle(style lipgloss.Style, text string) lipgloss.Style {
	if CurrentMode() == ModeOff {
		return style
	}
	if IsParagraphRTL(text) {
		return style.Align(lipgloss.Right)
	}
	return style.Align(lipgloss.Left)
}
