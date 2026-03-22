package log

import (
	"bytes"
	"strings"
	"testing"
)

func TestSetup(t *testing.T) {
	Setup(&Config{Level: LevelInfo, Format: FormatText})
	Setup(&Config{Level: LevelDebug, Format: FormatJSON})
	Setup(nil)
}

func TestLevel(t *testing.T) {
	SetLevel(LevelDebug)
	SetLevel(LevelInfo)
	SetLevel(LevelWarn)
	SetLevel(LevelError)
	SetLevel(LevelInfo)
}

func TestIPCLogger(t *testing.T) {
	logger := NewIPCLogger()
	if logger == nil {
		t.Error("expected non-nil IPCLogger")
	}
}

func TestIPCLoggerWith(t *testing.T) {
	logger := NewIPCLogger().With("request_id", "123")
	if logger == nil {
		t.Error("expected non-nil IPCLogger")
	}
}

func TestStructuredFields(t *testing.T) {
	var buf bytes.Buffer
	SetOutput(&buf)

	Info("structured test",
		"string_field", "hello",
		"int_field", 42,
		"bool_field", true,
	)

	output := buf.String()
	for _, expected := range []string{"hello", "42", "true", "structured test"} {
		if !strings.Contains(output, expected) {
			t.Errorf("expected %s in log output: %s", expected, output)
		}
	}
}

func TestLogOutput(t *testing.T) {
	var buf bytes.Buffer
	SetOutput(&buf)

	Info("test message", "key", "value")

	output := buf.String()
	if !strings.Contains(output, "test message") {
		t.Errorf("expected message in output: %s", output)
	}
	if !strings.Contains(output, "key=value") {
		t.Errorf("expected key=value in output: %s", output)
	}
}

func TestDebugInfoWarnError(t *testing.T) {
	var buf bytes.Buffer
	SetOutput(&buf)
	SetLevel(LevelDebug)

	Debug("debug message", "level", "debug")
	Info("info message", "level", "info")
	Warn("warn message", "level", "warn")
	Error("error message", "level", "error")

	output := buf.String()
	for _, expected := range []string{"debug message", "info message", "warn message", "error message"} {
		if !strings.Contains(output, expected) {
			t.Errorf("expected %s in log output: %s", expected, output)
		}
	}
}

func TestLogLevels(t *testing.T) {
	SetLevel(LevelWarn)

	var buf bytes.Buffer
	SetOutput(&buf)

	Debug("debug should not appear", "key", "debug")
	Info("info should not appear", "key", "info")
	Warn("warn should appear", "key", "warn")
	Error("error should appear", "key", "error")

	output := buf.String()
	if strings.Contains(output, "debug") {
		t.Error("debug message should not appear at LevelWarn")
	}
	if strings.Contains(output, "info") {
		t.Error("info message should not appear at LevelWarn")
	}
	if !strings.Contains(output, "warn") {
		t.Error("warn message should appear at LevelWarn")
	}
	if !strings.Contains(output, "error") {
		t.Error("error message should appear at LevelWarn")
	}
}
