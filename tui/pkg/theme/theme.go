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
//   theme.T.Accent()              // raw color.Color
//   theme.T.Apply(newPalette)     // called from IPC handler on theme_update

import (
	"fmt"
	"image/color"
	"sync/atomic"

	"charm.land/lipgloss/v2"
)

// ── Palette ───────────────────────────────────────────────────────────────────

// Palette is all semantic colors for one theme. Immutable after creation.
type Palette struct {
	Bg        color.Color
	Surface   color.Color
	Border    color.Color
	BorderFoc color.Color

	Text      color.Color
	TextDim   color.Color
	TextMuted color.Color

	Accent    color.Color // primary action color
	AccentAlt color.Color // secondary accent (cyan/teal)
	Neon      color.Color // bright highlight
	Green     color.Color
	Red       color.Color
	Yellow    color.Color

	Warn    color.Color // amber — warning indicators
	Success color.Color // green — success indicators

	TabActive   color.Color
	TabInactive color.Color
	TabText     color.Color
	TabTextDim  color.Color
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
//
//	{ "colors": { "dark": { "background": "#1b1b1f", "primary": "#adc6ff", ... } } }
//
// M3 role → stui semantic mapping:
//
//	background          → Bg
//	surface             → Surface (use background if absent)
//	outline_variant     → Border
//	primary             → Accent + TabActive
//	secondary           → AccentAlt
//	tertiary            → Neon
//	on_surface          → Text + TabText
//	on_surface_variant  → TextMuted + TabTextDim
//	outline             → TextDim
//	surface_variant     → TabInactive + BorderFoc
//	error               → Red
//	(no direct M3 green/yellow → kept from default or derived)
func FromMatugen(dark map[string]string) Palette {
	p := Default() // start from defaults so missing keys don't break anything

	get := func(key string) (color.Color, bool) {
		if v, ok := dark[key]; ok && len(v) > 0 {
			return lipgloss.Color(v), true
		}
		return nil, false
	}

	if c, ok := get("background"); ok {
		p.Bg = c
	}
	if c, ok := get("surface"); ok {
		p.Surface = c
	} else if c, ok := get("background"); ok {
		p.Surface = c
	}
	if c, ok := get("outline_variant"); ok {
		p.Border = c
	}
	if c, ok := get("surface_variant"); ok {
		p.BorderFoc = c
		p.TabInactive = c
	}
	if c, ok := get("primary"); ok {
		p.Accent = c
		p.TabActive = c
	}
	if c, ok := get("secondary"); ok {
		p.AccentAlt = c
	}
	if c, ok := get("tertiary"); ok {
		p.Neon = c
	}
	if c, ok := get("on_surface"); ok {
		p.Text = c
		p.TabText = c
	}
	if c, ok := get("on_surface_variant"); ok {
		p.TextMuted = c
		p.TabTextDim = c
	}
	if c, ok := get("outline"); ok {
		p.TextDim = c
	}
	if c, ok := get("error"); ok {
		p.Red = c
	}

	return p
}

// HighContrast returns a high-contrast palette with pure black/white and
// saturated accent colors, suitable for accessibility or bright environments.
func HighContrast() Palette {
	return Palette{
		Bg:        lipgloss.Color("#000000"),
		Surface:   lipgloss.Color("#0d0d0d"),
		Border:    lipgloss.Color("#ffffff"),
		BorderFoc: lipgloss.Color("#ffff00"),

		Text:      lipgloss.Color("#ffffff"),
		TextDim:   lipgloss.Color("#aaaaaa"),
		TextMuted: lipgloss.Color("#cccccc"),

		Accent:    lipgloss.Color("#ffff00"),
		AccentAlt: lipgloss.Color("#00ffff"),
		Neon:      lipgloss.Color("#ff00ff"),
		Green:     lipgloss.Color("#00ff00"),
		Red:       lipgloss.Color("#ff0000"),
		Yellow:    lipgloss.Color("#ffff00"),

		Warn:    lipgloss.Color("#ff8800"),
		Success: lipgloss.Color("#00ff00"),

		TabActive:   lipgloss.Color("#ffff00"),
		TabInactive: lipgloss.Color("#1a1a1a"),
		TabText:     lipgloss.Color("#000000"),
		TabTextDim:  lipgloss.Color("#888888"),
	}
}

// Monochrome returns a grayscale palette with no saturated colors,
// suitable for terminals with limited color support or minimal aesthetics.
func Monochrome() Palette {
	return Palette{
		Bg:        lipgloss.Color("#0a0a0a"),
		Surface:   lipgloss.Color("#111111"),
		Border:    lipgloss.Color("#333333"),
		BorderFoc: lipgloss.Color("#888888"),

		Text:      lipgloss.Color("#dddddd"),
		TextDim:   lipgloss.Color("#555555"),
		TextMuted: lipgloss.Color("#777777"),

		Accent:    lipgloss.Color("#bbbbbb"),
		AccentAlt: lipgloss.Color("#999999"),
		Neon:      lipgloss.Color("#eeeeee"),
		Green:     lipgloss.Color("#aaaaaa"),
		Red:       lipgloss.Color("#888888"),
		Yellow:    lipgloss.Color("#cccccc"),

		Warn:    lipgloss.Color("#aaaaaa"),
		Success: lipgloss.Color("#bbbbbb"),

		TabActive:   lipgloss.Color("#cccccc"),
		TabInactive: lipgloss.Color("#1a1a1a"),
		TabText:     lipgloss.Color("#000000"),
		TabTextDim:  lipgloss.Color("#666666"),
	}
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

func (t *Theme) Accent() color.Color    { return t.P().Accent }
func (t *Theme) AccentAlt() color.Color { return t.P().AccentAlt }
func (t *Theme) Neon() color.Color      { return t.P().Neon }
func (t *Theme) Red() color.Color       { return t.P().Red }
func (t *Theme) Green() color.Color     { return t.P().Green }
func (t *Theme) Yellow() color.Color    { return t.P().Yellow }
func (t *Theme) Warn() color.Color      { return t.P().Warn }
func (t *Theme) Success() color.Color   { return t.P().Success }
func (t *Theme) Bg() color.Color        { return t.P().Bg }
func (t *Theme) Surface() color.Color   { return t.P().Surface }
func (t *Theme) Border() color.Color    { return t.P().Border }
func (t *Theme) Text() color.Color      { return t.P().Text }
func (t *Theme) TextDim() color.Color   { return t.P().TextDim }
func (t *Theme) TextMuted() color.Color { return t.P().TextMuted }

// ── Chrome styles ─────────────────────────────────────────────────────────────

// TopBarStyle returns the chrome style for the top navigation bar.
// focused=true uses the accent border (when search input is active).
func (t *Theme) TopBarStyle(focused bool) lipgloss.Style {
	p := t.P()
	borderColor := p.Border
	if focused {
		borderColor = p.BorderFoc
	}
	return lipgloss.NewStyle().
		Background(p.Surface).
		Border(lipgloss.RoundedBorder()).
		BorderForeground(borderColor).
		PaddingLeft(1).PaddingRight(1).
		MarginLeft(1).MarginRight(1).MarginTop(1)
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
		Border(lipgloss.RoundedBorder(), false, true, false, true).
		BorderForeground(p.Border).
		PaddingLeft(1).PaddingRight(1)
}

func (t *Theme) SearchFocusedStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Bg).Foreground(p.Text).
		Border(lipgloss.RoundedBorder(), false, true, false, true).
		BorderForeground(p.Accent).
		PaddingLeft(1).PaddingRight(1)
}

// GearStyle uses the theme's primary accent so the settings gear stays
// recognisably "the main color" — yellow in high-contrast, purple in
// default, etc. Padding matches the focused variant for layout stability.
func (t *Theme) GearStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Accent).PaddingLeft(2).PaddingRight(1)
}

// GearFocusedStyle uses the same accent but bolds it to indicate the
// runtime is ready/active.
func (t *Theme) GearFocusedStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(t.P().Accent).Bold(true).PaddingLeft(2).PaddingRight(1)
}

// StatusBarStyle returns the chrome style for the bottom status bar.
// The statusbar is never focused — it always uses the dim border color.
func (t *Theme) StatusBarStyle() lipgloss.Style {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Surface).Foreground(p.TextMuted).
		Border(lipgloss.RoundedBorder()).
		BorderForeground(p.Border).
		PaddingLeft(2).PaddingRight(2).
		MarginLeft(1).MarginRight(1).MarginBottom(1)
}

// MainCardStyle returns the chrome style for the main content area card.
// focused=true uses the accent border (when the grid/content has keyboard focus).
func (t *Theme) MainCardStyle(focused bool) lipgloss.Style {
	p := t.P()
	borderColor := p.Border
	if focused {
		borderColor = p.BorderFoc
	}
	return lipgloss.NewStyle().
		Background(p.Bg).
		Border(lipgloss.RoundedBorder()).
		BorderForeground(borderColor).
		PaddingLeft(1).PaddingRight(1).
		MarginLeft(1).MarginRight(1)
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
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Bg).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(p.Border).
		BorderTop(true).BorderBottom(true).BorderLeft(true).BorderRight(true).
		PaddingLeft(1).PaddingRight(1)
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

// ── Composite helpers ─────────────────────────────────────────────────────────

// EmptyStateStyle renders a full empty-state message: icon, title, and hint line.
func (t *Theme) EmptyStateStyle(icon, title, hint string) string {
	p := t.P()
	iconStr := lipgloss.NewStyle().Foreground(p.AccentAlt).Render(icon)
	titleStr := lipgloss.NewStyle().Foreground(p.Text).Bold(true).Render(title)
	hintStr := lipgloss.NewStyle().Foreground(p.TextDim).Render(hint)
	return fmt.Sprintf("%s  %s\n    %s", iconStr, titleStr, hintStr)
}

// KeyHint renders a keyboard shortcut hint, e.g. "↑↓ navigate".
func (t *Theme) KeyHint(key, label string) string {
	p := t.P()
	keyStr := lipgloss.NewStyle().Foreground(p.AccentAlt).Bold(true).Render(key)
	labelStr := lipgloss.NewStyle().Foreground(p.TextDim).Render(label)
	return keyStr + " " + labelStr
}

// WarnPill renders text as a warning badge (amber background).
func (t *Theme) WarnPill(text string) string {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Warn).Foreground(p.Bg).
		PaddingLeft(1).PaddingRight(1).Bold(true).
		Render(text)
}

// SuccessPill renders text as a success badge (green background).
func (t *Theme) SuccessPill(text string) string {
	p := t.P()
	return lipgloss.NewStyle().
		Background(p.Success).Foreground(p.Bg).
		PaddingLeft(1).PaddingRight(1).Bold(true).
		Render(text)
}

// ── Color math ────────────────────────────────────────────────────────────────

// darken returns color c darkened by factor (0.0 = unchanged, 1.0 = black).
func darken(c color.Color, factor float64) color.Color {
	r32, g32, b32, _ := c.RGBA()
	f := 1.0 - factor
	return color.RGBA{
		R: clampU8(int(float64(r32>>8) * f)),
		G: clampU8(int(float64(g32>>8) * f)),
		B: clampU8(int(float64(b32>>8) * f)),
		A: 255,
	}
}

// lighten returns color c lightened by factor (0.0 = unchanged, 1.0 = full bright).
func lighten(c color.Color, factor float64) color.Color {
	r32, g32, b32, _ := c.RGBA()
	f := 1.0 + factor
	return color.RGBA{
		R: clampU8(int(float64(r32>>8) * f)),
		G: clampU8(int(float64(g32>>8) * f)),
		B: clampU8(int(float64(b32>>8) * f)),
		A: 255,
	}
}

func clampU8(v int) uint8 {
	if v < 0 {
		return 0
	}
	if v > 255 {
		return 255
	}
	return uint8(v)
}
