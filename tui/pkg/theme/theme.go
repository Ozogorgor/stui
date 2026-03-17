package theme

// theme.go — live-swappable theme system for stui.
//
// Architecture
// ─────────────
// Colors live in an immutable Palette struct.
// A single global *Theme (T) wraps an atomic.Pointer[Palette].
// Any goroutine reads the live palette lock-free; the IPC handler swaps it
// in one atomic write via T.Apply(). The next View() call picks it up.
//
// Styles are NOT stored as globals — every render call goes through a Theme
// method that constructs a fresh lipgloss.Style from the live palette.
// lipgloss.NewStyle() is a tiny stack allocation; this is intentional.
//
// Usage:
//   theme.T.TabActive().Render(" Movies ")
//   theme.T.Accent()              // raw lipgloss.Color
//   theme.T.Apply(newPalette)     // called from IPC handler on theme_update

import (
	"fmt"
	"sync/atomic"

	"github.com/charmbracelet/lipgloss"
)

// ── Palette ───────────────────────────────────────────────────────────────────

// Palette is all semantic colors for one theme. Immutable after creation.
type Palette struct {
	Bg        lipgloss.Color
	Surface   lipgloss.Color
	Border    lipgloss.Color
	BorderFoc lipgloss.Color

	Text      lipgloss.Color
	TextDim   lipgloss.Color
	TextMuted lipgloss.Color

	Accent    lipgloss.Color // primary action color
	AccentAlt lipgloss.Color // secondary accent (cyan/teal)
	Neon      lipgloss.Color // bright highlight
	Green     lipgloss.Color
	Red       lipgloss.Color
	Yellow    lipgloss.Color

	Warn    lipgloss.Color // amber — warning indicators
	Success lipgloss.Color // green — success indicators

	TabActive   lipgloss.Color
	TabInactive lipgloss.Color
	TabText     lipgloss.Color
	TabTextDim  lipgloss.Color
}

// Default is the built-in violet/neon-on-black palette used when no
// matugen colors are present.
func Default() Palette {
	return Palette{
		Bg:        lipgloss.Color("#0a0a0f"),
		Surface:   lipgloss.Color("#0f0f1a"),
		Border:    lipgloss.Color("#1e1e2e"),
		BorderFoc: lipgloss.Color("#7c3aed"),

		Text:      lipgloss.Color("#e2e8f0"),
		TextDim:   lipgloss.Color("#4a5568"),
		TextMuted: lipgloss.Color("#718096"),

		Accent:    lipgloss.Color("#7c3aed"),
		AccentAlt: lipgloss.Color("#06b6d4"),
		Neon:      lipgloss.Color("#a855f7"),
		Green:     lipgloss.Color("#10b981"),
		Red:       lipgloss.Color("#ef4444"),
		Yellow:    lipgloss.Color("#f59e0b"),

		Warn:    lipgloss.Color("#e5c07b"),
		Success: lipgloss.Color("#98c379"),

		TabActive:   lipgloss.Color("#a855f7"),
		TabInactive: lipgloss.Color("#1a1a2e"),
		TabText:     lipgloss.Color("#e2e8f0"),
		TabTextDim:  lipgloss.Color("#4a5568"),
	}
}

// FromMatugen maps Material You color roles (from matugen --json hex) to a
// stui Palette. The input map is colors["dark"] from the JSON output.
//
// Matugen JSON structure:
//   { "colors": { "dark": { "background": "#1b1b1f", "primary": "#adc6ff", ... } } }
//
// M3 role → stui semantic mapping:
//   background          → Bg
//   surface             → Surface (use background if absent)
//   outline_variant     → Border
//   primary             → Accent + TabActive
//   secondary           → AccentAlt
//   tertiary            → Neon
//   on_surface          → Text + TabText
//   on_surface_variant  → TextMuted + TabTextDim
//   outline             → TextDim
//   surface_variant     → TabInactive + BorderFoc
//   error               → Red
//   (no direct M3 green/yellow → kept from default or derived)
func FromMatugen(dark map[string]string) Palette {
	p := Default() // start from defaults so missing keys don't break anything

	get := func(key string) lipgloss.Color {
		if v, ok := dark[key]; ok && len(v) > 0 {
			return lipgloss.Color(v)
		}
		return ""
	}

	if c := get("background"); c != "" {
		p.Bg = c
	}
	if c := get("surface"); c != "" {
		p.Surface = c
	} else if c := get("background"); c != "" {
		p.Surface = c
	}
	if c := get("outline_variant"); c != "" {
		p.Border = c
	}
	if c := get("surface_variant"); c != "" {
		p.BorderFoc = c
		p.TabInactive = c
	}
	if c := get("primary"); c != "" {
		p.Accent = c
		p.TabActive = c
	}
	if c := get("secondary"); c != "" {
		p.AccentAlt = c
	}
	if c := get("tertiary"); c != "" {
		p.Neon = c
	}
	if c := get("on_surface"); c != "" {
		p.Text = c
		p.TabText = c
	}
	if c := get("on_surface_variant"); c != "" {
		p.TextMuted = c
		p.TabTextDim = c
	}
	if c := get("outline"); c != "" {
		p.TextDim = c
	}
	if c := get("error"); c != "" {
		p.Red = c
	}

	return p
}

// ── Theme ─────────────────────────────────────────────────────────────────────

// Theme wraps an atomically-swappable Palette.
type Theme struct {
	p atomic.Pointer[Palette]
}

// T is the single global live Theme used by all render code.
var T = func() *Theme {
	t := &Theme{}
	def := Default()
	t.p.Store(&def)
	return t
}()

// P returns the current palette. Safe to call from any goroutine.
// Valid only for the duration of one render frame — never cache across frames.
func (t *Theme) P() *Palette { return t.p.Load() }

// Apply atomically swaps in a new palette.
// The next render frame will use the new colors automatically.
func (t *Theme) Apply(p Palette) { t.p.Store(&p) }

// ── Raw color accessors ───────────────────────────────────────────────────────

func (t *Theme) Accent() lipgloss.Color    { return t.P().Accent }
func (t *Theme) AccentAlt() lipgloss.Color { return t.P().AccentAlt }
func (t *Theme) Neon() lipgloss.Color      { return t.P().Neon }
func (t *Theme) Red() lipgloss.Color       { return t.P().Red }
func (t *Theme) Green() lipgloss.Color     { return t.P().Green }
func (t *Theme) Yellow() lipgloss.Color    { return t.P().Yellow }
func (t *Theme) Warn() lipgloss.Color      { return t.P().Warn }
func (t *Theme) Success() lipgloss.Color   { return t.P().Success }
func (t *Theme) Bg() lipgloss.Color        { return t.P().Bg }
func (t *Theme) Surface() lipgloss.Color   { return t.P().Surface }
func (t *Theme) Border() lipgloss.Color    { return t.P().Border }
func (t *Theme) Text() lipgloss.Color      { return t.P().Text }
func (t *Theme) TextDim() lipgloss.Color   { return t.P().TextDim }
func (t *Theme) TextMuted() lipgloss.Color { return t.P().TextMuted }

// ── Chrome styles ─────────────────────────────────────────────────────────────

func (t *Theme) TopBarStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Surface).
		BorderStyle(lipgloss.NormalBorder()).
		BorderForeground(p.Border).
		BorderBottom(true).
		PaddingLeft(1).PaddingRight(1)
}

func (t *Theme) TabStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.TabInactive).
		Foreground(p.TabTextDim).
		PaddingLeft(2).PaddingRight(2).MarginRight(1)
}

func (t *Theme) TabActiveStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.TabActive).
		Foreground(p.TabText).
		PaddingLeft(2).PaddingRight(2).MarginRight(1).
		Bold(true)
}

func (t *Theme) SearchStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Bg).Foreground(p.Text).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(p.Border).
		PaddingLeft(1).PaddingRight(1)
}

func (t *Theme) SearchFocusedStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Bg).Foreground(p.Text).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(p.Accent).
		PaddingLeft(1).PaddingRight(1)
}

func (t *Theme) GearStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().TextMuted).PaddingLeft(2).PaddingRight(1)
}

func (t *Theme) GearFocusedStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Neon).PaddingLeft(2).PaddingRight(1)
}

func (t *Theme) StatusBarStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Surface).Foreground(p.TextMuted).
		BorderStyle(lipgloss.NormalBorder()).
		BorderForeground(p.Border).
		BorderTop(true).
		PaddingLeft(2).PaddingRight(2)
}

func (t *Theme) StatusAccentStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Accent).Foreground(p.TabText).
		PaddingLeft(1).PaddingRight(1).Bold(true)
}

func (t *Theme) ColHeaderStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Foreground(p.AccentAlt).
		BorderStyle(lipgloss.NormalBorder()).
		BorderForeground(p.Border).
		BorderBottom(true).PaddingLeft(1).Bold(true)
}

func (t *Theme) ResultRowStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Text).PaddingLeft(1)
}

func (t *Theme) ResultRowAltStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Foreground(p.Text).
		Background(darken(p.Bg, 0.3)).
		PaddingLeft(1)
}

func (t *Theme) ResultRowSelectedStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Accent).Foreground(p.TabText).
		PaddingLeft(1).Bold(true)
}

func (t *Theme) ResultRowHoveredStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.TabInactive).Foreground(p.Neon).PaddingLeft(1)
}

func (t *Theme) ResultsPanelStyle() lipgloss.Style {
	return lipgloss.NewStyle().Background(t.P().Bg).PaddingLeft(1).PaddingRight(1)
}

// ── Detail panel styles ───────────────────────────────────────────────────────

func (t *Theme) DetailBackStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().Foreground(p.TextMuted).Background(p.Surface).
		PaddingLeft(2).PaddingRight(2)
}

func (t *Theme) DetailTitleStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Text).Bold(true)
}

func (t *Theme) DetailMetaStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().TextMuted)
}

func (t *Theme) DetailRatingStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Yellow).Bold(true)
}

func (t *Theme) DetailSectionStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().AccentAlt).Bold(true).MarginTop(1)
}

func (t *Theme) DetailCastStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Text).PaddingLeft(2)
}

func (t *Theme) DetailCastFocusedStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Foreground(p.Neon).
		Background(darken(p.Accent, 0.75)).
		PaddingLeft(2).Bold(true)
}

func (t *Theme) DetailRoleStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().TextDim)
}

func (t *Theme) DetailLinkStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Accent)
}

func (t *Theme) DetailProviderStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Foreground(p.AccentAlt).
		Background(darken(p.AccentAlt, 0.85)).
		PaddingLeft(1).PaddingRight(1).MarginRight(1).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(p.AccentAlt)
}

func (t *Theme) DetailDescStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Text)
}

func (t *Theme) SimilarHeaderStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Foreground(p.AccentAlt).Bold(true).
		BorderStyle(lipgloss.NormalBorder()).
		BorderForeground(p.Border).
		BorderTop(true).PaddingTop(1)
}

func (t *Theme) BreadcrumbStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().TextDim)
}

func (t *Theme) PersonHeaderStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Neon).Bold(true).PaddingLeft(1)
}

// ── Toast styles ──────────────────────────────────────────────────────────────

func (t *Theme) ToastSuccessStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Accent).Foreground(lipgloss.Color("#ffffff")).Bold(true).
		Padding(0, 2).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(p.Neon).
		BorderBackground(p.Accent)
}

func (t *Theme) ToastErrorStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Red).Foreground(lipgloss.Color("#ffffff")).Bold(true).
		Padding(0, 2).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(lighten(p.Red, 0.3)).
		BorderBackground(p.Red)
}

// ── Color math ────────────────────────────────────────────────────────────────

// darken returns color c darkened by factor (0.0 = unchanged, 1.0 = black).
func darken(c lipgloss.Color, factor float64) lipgloss.Color {
	r, g, b := hexToRGB(string(c))
	f := 1.0 - factor
	return rgbToColor(int(float64(r)*f), int(float64(g)*f), int(float64(b)*f))
}

// lighten returns color c lightened by factor (0.0 = unchanged, 1.0 = full bright).
func lighten(c lipgloss.Color, factor float64) lipgloss.Color {
	r, g, b := hexToRGB(string(c))
	f := 1.0 + factor
	return rgbToColor(int(float64(r)*f), int(float64(g)*f), int(float64(b)*f))
}

func hexToRGB(hex string) (int, int, int) {
	if len(hex) < 7 {
		return 0, 0, 0
	}
	var r, g, b int
	fmt.Sscanf(hex[1:], "%02x%02x%02x", &r, &g, &b)
	return r, g, b
}

func rgbToColor(r, g, b int) lipgloss.Color {
	clamp := func(v int) int {
		if v < 0 {
			return 0
		}
		if v > 255 {
			return 255
		}
		return v
	}
	return lipgloss.Color(fmt.Sprintf("#%02x%02x%02x", clamp(r), clamp(g), clamp(b)))
}

