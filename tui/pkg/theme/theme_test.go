// pkg/theme/theme_test.go
package theme

import "testing"

func TestDefaultPaletteWarn(t *testing.T) {
	p := Default()
	if string(p.Warn) != "#e5c07b" {
		t.Errorf("Default Palette.Warn = %q, want #e5c07b", p.Warn)
	}
}

func TestDefaultPaletteSuccess(t *testing.T) {
	p := Default()
	if string(p.Success) != "#98c379" {
		t.Errorf("Default Palette.Success = %q, want #98c379", p.Success)
	}
}

func TestThemeWarnMethod(t *testing.T) {
	if got := T.Warn(); string(got) != "#e5c07b" {
		t.Errorf("T.Warn() = %q, want #e5c07b", got)
	}
}

func TestThemeSuccessMethod(t *testing.T) {
	if got := T.Success(); string(got) != "#98c379" {
		t.Errorf("T.Success() = %q, want #98c379", got)
	}
}
