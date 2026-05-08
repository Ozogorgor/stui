// Package notify sends desktop notifications on Wayland and X11.
//
// It supports notify-send (most universal) and dunstctl (Dunst-specific).
// The "auto" backend picks whichever binary is found on $PATH first:
// notify-send → dunstctl → silent.
//
// All Send calls are fire-and-forget: they spawn a subprocess in a goroutine
// and discard errors, so slow or missing notification daemons never block
// the TUI event loop.
package notify

import (
	"os/exec"
	"sync"
)

// Urgency maps to the notification urgency level.
type Urgency string

const (
	UrgencyLow      Urgency = "low"
	UrgencyNormal   Urgency = "normal"
	UrgencyCritical Urgency = "critical"
)

// Config holds the user-configurable notification preferences.
// These are mirrored from the settings screen into the root model.
type Config struct {
	Enabled    bool   // master switch
	Backend    string // "auto", "notify-send", "dunstctl", "off"
	OnPlayback bool   // notify when mpv starts playing
	OnDownload bool   // notify when a torrent download completes
	OnStreams  bool   // notify when stream candidates are resolved
}

// DefaultConfig returns the recommended default notification config.
func DefaultConfig() Config {
	return Config{
		Enabled:    true,
		Backend:    "auto",
		OnPlayback: true,
		OnDownload: true,
		OnStreams:  false, // can be noisy on slow providers
	}
}

// ── Backend detection ─────────────────────────────────────────────────────────

var (
	detectedBackend string
	detectOnce      sync.Once
)

// resolvedBackend returns the effective binary to use ("notify-send",
// "dunstctl", or "" meaning nothing available).
func resolvedBackend(requested string) string {
	switch requested {
	case "off":
		return ""
	case "notify-send":
		if p, err := exec.LookPath("notify-send"); err == nil && p != "" {
			return "notify-send"
		}
		return ""
	case "dunstctl":
		if p, err := exec.LookPath("dunstctl"); err == nil && p != "" {
			return "dunstctl"
		}
		return ""
	default: // "auto" or anything else
		detectOnce.Do(func() {
			switch {
			case hasCmd("notify-send"):
				detectedBackend = "notify-send"
			case hasCmd("dunstctl"):
				detectedBackend = "dunstctl"
			default:
				detectedBackend = ""
			}
		})
		return detectedBackend
	}
}

func hasCmd(name string) bool {
	p, err := exec.LookPath(name)
	return err == nil && p != ""
}

// ── Send ──────────────────────────────────────────────────────────────────────

// Send fires a desktop notification in a background goroutine.
// title and body are plain text (not HTML). urgency is UrgencyLow/Normal/Critical.
// If cfg.Enabled is false or the backend resolves to nothing, this is a no-op.
func Send(cfg Config, title, body string, urgency Urgency) {
	if !cfg.Enabled {
		return
	}
	backend := resolvedBackend(cfg.Backend)
	if backend == "" {
		return
	}
	go func() {
		switch backend {
		case "notify-send":
			_ = exec.Command(
				"notify-send",
				"--app-name=STUI",
				"--urgency="+string(urgency),
				title,
				body,
			).Run()
		case "dunstctl":
			// dunstctl notify --summary "..." --body "..." --urgency low|normal|critical
			_ = exec.Command(
				"dunstctl",
				"notify",
				"--summary="+title,
				"--body="+body,
				"--urgency="+string(urgency),
			).Run()
		}
	}()
}
