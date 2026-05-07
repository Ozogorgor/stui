package screens

// language.go — ISO 639-1 → human-readable English language name.
//
// CatalogEntry.OriginalLanguage carries a 2-letter ISO code populated
// by tmdb / kitsu / anilist plugins. The detail screen renders it as
// "Language: Japanese" instead of "Language: ja". Powered by
// golang.org/x/text/language (already in go.mod).
//
// Unknown / malformed codes are returned upper-cased so the user
// still sees something rather than blank — better than silently
// dropping a value the runtime delivered.

import (
	"strings"

	"golang.org/x/text/language"
	"golang.org/x/text/language/display"
)

func formatLanguage(code string) string {
	code = strings.TrimSpace(code)
	if code == "" {
		return ""
	}
	tag, err := language.Parse(code)
	if err != nil {
		return strings.ToUpper(code)
	}
	name := display.English.Languages().Name(tag)
	if name == "" {
		return strings.ToUpper(code)
	}
	return name
}
