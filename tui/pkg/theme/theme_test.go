// pkg/theme/theme_test.go
package theme

import (
	"image/color"
	"testing"
)

func colorToHex(c color.Color) string {
	if rgba, ok := c.(color.RGBA); ok {
		return "#" + hexByte(rgba.R) + hexByte(rgba.G) + hexByte(rgba.B)
	}
	return ""
}

func hexByte(b uint8) string {
	const hexChars = "0123456789abcdef"
	return string([]byte{hexChars[b>>4], hexChars[b&0x0f]})
}

func TestDefaultPaletteWarn(t *testing.T) {
	p := Default()
	got := colorToHex(p.Warn)
	if got != "#e5c07b" {
		t.Errorf("Default Palette.Warn = %q, want #e5c07b", got)
	}
}

func TestDefaultPaletteSuccess(t *testing.T) {
	p := Default()
	got := colorToHex(p.Success)
	if got != "#98c379" {
		t.Errorf("Default Palette.Success = %q, want #98c379", got)
	}
}

func TestThemeWarnMethod(t *testing.T) {
	got := colorToHex(T.Warn())
	if got != "#e5c07b" {
		t.Errorf("T.Warn() = %q, want #e5c07b", got)
	}
}

func TestThemeSuccessMethod(t *testing.T) {
	got := colorToHex(T.Success())
	if got != "#98c379" {
		t.Errorf("T.Success() = %q, want #98c379", got)
	}
}

func TestTopBarStyleFocusedBorderColor(t *testing.T) {
	// TopBarStyle(true) must use BorderFoc (accent) color.
	// We can't inspect lipgloss internals directly, so we verify
	// that focused/unfocused produce different rendered output on
	// a non-empty string (different border chars will differ in ANSI).
	focused := T.TopBarStyle(true).Width(20).Render("x")
	unfocused := T.TopBarStyle(false).Width(20).Render("x")
	if focused == unfocused {
		t.Error("TopBarStyle(true) and TopBarStyle(false) should produce different output")
	}
}

func TestTopBarStyleHasAllBorders(t *testing.T) {
	s := T.TopBarStyle(false)
	if !s.GetBorderTop() || !s.GetBorderBottom() || !s.GetBorderLeft() || !s.GetBorderRight() {
		t.Error("TopBarStyle must have all four border sides enabled")
	}
}

func TestStatusBarStyleHasAllBorders(t *testing.T) {
	s := T.StatusBarStyle()
	if !s.GetBorderTop() || !s.GetBorderBottom() || !s.GetBorderLeft() || !s.GetBorderRight() {
		t.Error("StatusBarStyle must have all four border sides enabled")
	}
}

func TestMainCardStyleFocusedBorderColor(t *testing.T) {
	focused := T.MainCardStyle(true).Width(20).Render("x")
	unfocused := T.MainCardStyle(false).Width(20).Render("x")
	if focused == unfocused {
		t.Error("MainCardStyle(true) and MainCardStyle(false) should produce different output")
	}
}

func TestMainCardStyleHasAllBorders(t *testing.T) {
	s := T.MainCardStyle(false)
	if !s.GetBorderTop() || !s.GetBorderBottom() || !s.GetBorderLeft() || !s.GetBorderRight() {
		t.Error("MainCardStyle must have all four border sides enabled")
	}
}
