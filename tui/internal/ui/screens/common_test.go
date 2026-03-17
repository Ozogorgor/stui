// internal/ui/screens/common_test.go
package screens

import (
	"strings"
	"testing"
)

func TestHintBarLeadingIndent(t *testing.T) {
	result := hintBar("esc back")
	if !strings.HasPrefix(result, "  ") {
		t.Errorf("hintBar result %q should start with two spaces", result)
	}
}

func TestHintBarSingleHintPresent(t *testing.T) {
	result := hintBar("esc back")
	if !strings.Contains(result, "esc back") {
		t.Errorf("hintBar(%q) = %q, want it to contain the hint text", "esc back", result)
	}
}

func TestHintBarMultipleHintsPresent(t *testing.T) {
	result := hintBar("enter play", "esc back")
	if !strings.Contains(result, "enter play") {
		t.Errorf("hintBar result %q should contain 'enter play'", result)
	}
	if !strings.Contains(result, "esc back") {
		t.Errorf("hintBar result %q should contain 'esc back'", result)
	}
}

func TestHintBarNoHints(t *testing.T) {
	// Should not panic with zero arguments.
	_ = hintBar()
}
