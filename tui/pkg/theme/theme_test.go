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
