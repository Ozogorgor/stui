package screens

// htmltext.go — strip inline HTML and decode entities from synopsis/
// description text. AniList's GraphQL responses (and a handful of other
// providers) embed `<i>`, `<b>`, `<br>`, `&amp;`, `&#039;`, etc. for
// emphasis and line breaks; the TUI word-wraps and prints those
// verbatim, so a bare description renders with literal `<i>Chainsaw
// Man</i>` and `<br><br>` on the detail card. Calling cleanDescription
// at every render site that displays user-visible synopsis text fixes
// the regression without depending on every provider plugin to
// pre-normalize its output.

import (
	"html"
	"regexp"
	"strings"
)

var (
	brTagRE     = regexp.MustCompile(`(?i)<br\s*/?>`)
	pTagRE      = regexp.MustCompile(`(?i)</?p\s*>`)
	anyTagRE    = regexp.MustCompile(`<[^>]+>`)
	tripleNlRE  = regexp.MustCompile(`\n{3,}`)
)

// cleanDescription normalizes inline HTML and HTML entities. <br> and
// <p> become newlines; all other tags strip out; entities decode via
// the html package. Runs of 3+ newlines collapse to 2 so a
// `<br><br><br>` sequence doesn't blow up vertical space on the card.
//
// Fast path: returns input unchanged when it carries no `<` and no
// `&` — most catalog descriptions are plain text and shouldn't pay the
// regex cost.
func cleanDescription(s string) string {
	if s == "" {
		return s
	}
	if !strings.ContainsAny(s, "<&") {
		return s
	}
	s = brTagRE.ReplaceAllString(s, "\n")
	s = pTagRE.ReplaceAllString(s, "\n")
	s = anyTagRE.ReplaceAllString(s, "")
	s = html.UnescapeString(s)
	s = tripleNlRE.ReplaceAllString(s, "\n\n")
	return strings.TrimSpace(s)
}
