package config

import (
	"embed"
	"io/fs"
	"os"
	"path/filepath"
	"sort"

	"charm.land/lipgloss/v2"
	"github.com/BurntSushi/toml"
	"github.com/stui/stui/pkg/theme"
)

// bundledThemes is the set of starter theme TOML files embedded in
// the binary. Written to ~/.config/stui/themes/ on first launch (see
// EnsureBundledThemes) so users have something to copy and tweak.
//
//go:embed themes/*.toml
var bundledThemes embed.FS

// ThemesDir returns ~/.config/stui/themes.
func ThemesDir() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "themes")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "stui", "themes")
}

var builtinNames = []string{"default", "high-contrast", "monochrome", "matugen"}

var builtinSet = map[string]bool{
	"default": true, "high-contrast": true, "monochrome": true, "matugen": true,
}

// LoadTheme resolves a theme name to a Palette.
// Built-in names return the corresponding Go palette.
// "matugen" returns Default() as a placeholder.
// Any other name is loaded from ThemesDir()/<name>.toml.
// Returns (Default(), error) if the file is not found or cannot be parsed.
func LoadTheme(name string) (theme.Palette, error) {
	switch name {
	case "default":
		return theme.Default(), nil
	case "high-contrast":
		return theme.HighContrast(), nil
	case "monochrome":
		return theme.Monochrome(), nil
	case "matugen":
		return theme.Default(), nil
	default:
		path := filepath.Join(ThemesDir(), name+".toml")
		return loadThemeFromPath(path)
	}
}

// themeFile mirrors the TOML structure of a user theme file.
type themeFile struct {
	Name        string `toml:"name"`
	Bg          string `toml:"bg"`
	Surface     string `toml:"surface"`
	Border      string `toml:"border"`
	BorderFoc   string `toml:"border_foc"`
	Text        string `toml:"text"`
	TextDim     string `toml:"text_dim"`
	TextMuted   string `toml:"text_muted"`
	Accent      string `toml:"accent"`
	AccentAlt   string `toml:"accent_alt"`
	Neon        string `toml:"neon"`
	Green       string `toml:"green"`
	Red         string `toml:"red"`
	Yellow      string `toml:"yellow"`
	Warn        string `toml:"warn"`
	Success     string `toml:"success"`
	TabActive   string `toml:"tab_active"`
	TabInactive string `toml:"tab_inactive"`
	TabText     string `toml:"tab_text"`
	TabTextDim  string `toml:"tab_text_dim"`
}

// loadThemeFromPath reads a theme TOML file and merges over Default().
func loadThemeFromPath(path string) (theme.Palette, error) {
	p := theme.Default()
	data, err := os.ReadFile(path)
	if err != nil {
		return p, err
	}
	var tf themeFile
	if _, err := toml.Decode(string(data), &tf); err != nil {
		return p, err
	}
	if tf.Bg != "" {
		p.Bg = lipgloss.Color(tf.Bg)
	}
	if tf.Surface != "" {
		p.Surface = lipgloss.Color(tf.Surface)
	}
	if tf.Border != "" {
		p.Border = lipgloss.Color(tf.Border)
	}
	if tf.BorderFoc != "" {
		p.BorderFoc = lipgloss.Color(tf.BorderFoc)
	}
	if tf.Text != "" {
		p.Text = lipgloss.Color(tf.Text)
	}
	if tf.TextDim != "" {
		p.TextDim = lipgloss.Color(tf.TextDim)
	}
	if tf.TextMuted != "" {
		p.TextMuted = lipgloss.Color(tf.TextMuted)
	}
	if tf.Accent != "" {
		p.Accent = lipgloss.Color(tf.Accent)
	}
	if tf.AccentAlt != "" {
		p.AccentAlt = lipgloss.Color(tf.AccentAlt)
	}
	if tf.Neon != "" {
		p.Neon = lipgloss.Color(tf.Neon)
	}
	if tf.Green != "" {
		p.Green = lipgloss.Color(tf.Green)
	}
	if tf.Red != "" {
		p.Red = lipgloss.Color(tf.Red)
	}
	if tf.Yellow != "" {
		p.Yellow = lipgloss.Color(tf.Yellow)
	}
	if tf.Warn != "" {
		p.Warn = lipgloss.Color(tf.Warn)
	}
	if tf.Success != "" {
		p.Success = lipgloss.Color(tf.Success)
	}
	if tf.TabActive != "" {
		p.TabActive = lipgloss.Color(tf.TabActive)
	}
	if tf.TabInactive != "" {
		p.TabInactive = lipgloss.Color(tf.TabInactive)
	}
	if tf.TabText != "" {
		p.TabText = lipgloss.Color(tf.TabText)
	}
	if tf.TabTextDim != "" {
		p.TabTextDim = lipgloss.Color(tf.TabTextDim)
	}
	return p, nil
}

// EnsureBundledThemes writes the binary-embedded starter theme TOML
// files into ThemesDir() the first time the directory is missing.
// Idempotent: if the directory already exists, this is a no-op so a
// user who deleted a bundled theme won't see it silently reappear on
// every launch. Per-file errors are logged and skipped — a partial
// drop is better than refusing to start.
func EnsureBundledThemes() error {
	dir := ThemesDir()
	if dir == "" {
		return nil
	}
	if _, err := os.Stat(dir); err == nil {
		// Directory already exists; preserve whatever's there.
		return nil
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	return fs.WalkDir(bundledThemes, "themes", func(path string, d fs.DirEntry, walkErr error) error {
		if walkErr != nil || d.IsDir() {
			return walkErr
		}
		data, err := bundledThemes.ReadFile(path)
		if err != nil {
			return err
		}
		out := filepath.Join(dir, filepath.Base(path))
		return os.WriteFile(out, data, 0o644)
	})
}

// ListThemes returns available theme names: built-ins first, then
// filenames (without .toml) from ThemesDir(), sorted alphabetically.
// Files whose names collide with a built-in are silently skipped.
func ListThemes() []string {
	result := make([]string, len(builtinNames))
	copy(result, builtinNames)

	entries, err := os.ReadDir(ThemesDir())
	if err != nil {
		return result
	}

	var userNames []string
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		name := e.Name()
		if filepath.Ext(name) != ".toml" {
			continue
		}
		stem := name[:len(name)-5]
		if builtinSet[stem] {
			continue
		}
		userNames = append(userNames, stem)
	}
	sort.Strings(userNames)
	return append(result, userNames...)
}
