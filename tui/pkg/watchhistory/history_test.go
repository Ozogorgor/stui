package watchhistory_test

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/stui/stui/pkg/watchhistory"
)

func TestEntrySeasonEpisodeJSONOmitempty(t *testing.T) {
	// When both fields are zero, they should be omitted from JSON.
	e := watchhistory.Entry{ID: "tt0", Title: "Movie"}
	data, err := json.Marshal(e)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	s := string(data)
	if strings.Contains(s, "season") {
		t.Errorf("expected 'season' to be omitted when zero, got: %s", s)
	}
	if strings.Contains(s, "episode") {
		t.Errorf("expected 'episode' to be omitted when zero, got: %s", s)
	}

	// When non-zero, they should appear in JSON.
	e2 := watchhistory.Entry{ID: "tt1", Title: "Show", Season: 3, Episode: 5}
	data2, err := json.Marshal(e2)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	s2 := string(data2)
	if !strings.Contains(s2, `"season":3`) {
		t.Errorf("expected 'season' in JSON, got: %s", s2)
	}
	if !strings.Contains(s2, `"episode":5`) {
		t.Errorf("expected 'episode' in JSON, got: %s", s2)
	}
}

func TestParseEpisodeInfo(t *testing.T) {
	cases := []struct {
		title   string
		season  int
		episode int
	}{
		{"Breaking Bad S03E05", 3, 5},
		{"The Bear s2e1", 2, 1},
		{"Some Movie", 0, 0},
		{"Show S1E10 Finale", 1, 10},
		{"No pattern here", 0, 0},
	}
	for _, tc := range cases {
		s, e := watchhistory.ParseEpisodeInfo(tc.title)
		if s != tc.season || e != tc.episode {
			t.Errorf("ParseEpisodeInfo(%q): want (%d,%d), got (%d,%d)",
				tc.title, tc.season, tc.episode, s, e)
		}
	}
}
