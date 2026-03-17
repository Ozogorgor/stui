package state

import (
	"os"
	"strings"
	"testing"
)

func TestDefaultSettingsVideoDir(t *testing.T) {
	s := DefaultSettings()
	if !strings.HasSuffix(s.VideoDownloadDir, string(os.PathSeparator)+"Videos") {
		t.Errorf("VideoDownloadDir = %q, expected path ending in /Videos", s.VideoDownloadDir)
	}
}

func TestDefaultSettingsMusicDir(t *testing.T) {
	s := DefaultSettings()
	if !strings.HasSuffix(s.MusicDownloadDir, string(os.PathSeparator)+"Music") {
		t.Errorf("MusicDownloadDir = %q, expected path ending in /Music", s.MusicDownloadDir)
	}
}

func TestDefaultSettingsAutoDeleteVideoStillTrue(t *testing.T) {
	// Regression: existing default must not be broken.
	s := DefaultSettings()
	if !s.AutoDeleteVideo {
		t.Error("AutoDeleteVideo should still default to true")
	}
}
