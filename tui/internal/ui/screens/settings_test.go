package screens

import "testing"

func TestAdjustClampsAtMax(t *testing.T) {
	item := &settingItem{kind: settingInt, intVal: 5, minVal: 3, maxVal: 10}
	item.adjust(100)
	if item.intVal != 10 {
		t.Errorf("expected max 10, got %d", item.intVal)
	}
}

func TestAdjustClampsAtMin(t *testing.T) {
	item := &settingItem{kind: settingInt, intVal: 5, minVal: 3, maxVal: 10}
	item.adjust(-100)
	if item.intVal != 3 {
		t.Errorf("expected min 3, got %d", item.intVal)
	}
}

func TestAdjustNormalWithinBounds(t *testing.T) {
	item := &settingItem{kind: settingInt, intVal: 5, minVal: 3, maxVal: 10}
	item.adjust(2)
	if item.intVal != 7 {
		t.Errorf("expected 7, got %d", item.intVal)
	}
}

func TestAdjustNoBoundsWhenZero(t *testing.T) {
	// Existing items have no minVal/maxVal (zero values) — must behave unchanged.
	item := &settingItem{kind: settingInt, intVal: 100}
	item.adjust(50)
	if item.intVal != 150 {
		t.Errorf("expected 150, got %d", item.intVal)
	}
	item.adjust(-200)
	if item.intVal != -50 {
		t.Errorf("expected -50, got %d", item.intVal)
	}
}

func TestAdjustMinValZeroMeansNoBound(t *testing.T) {
	// minVal: 0 means "no lower bound" — a lower bound of exactly 0 cannot be expressed.
	// This is a known limitation of the sentinel pattern. All existing settingInt items
	// have values well above 0, so this is safe in practice.
	item := &settingItem{kind: settingInt, intVal: 5, minVal: 0, maxVal: 10}
	item.adjust(-100)
	// Should NOT clamp at 0; should go to -95 because minVal=0 means no bound.
	if item.intVal != -95 {
		t.Errorf("expected -95 (no lower bound when minVal=0), got %d", item.intVal)
	}
}

func TestSettingPathDisplayValueTildePrefix(t *testing.T) {
	// Save and restore the package-level var so this test is hermetic.
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "/home/testuser"
	item := &settingItem{kind: settingPath, strVal: "/home/testuser/Videos"}
	got := item.displayValue()
	if got != "~/Videos" {
		t.Errorf("displayValue() = %q, want %q", got, "~/Videos")
	}
}

func TestSettingPathDisplayValueNonHomePath(t *testing.T) {
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "/home/testuser"
	item := &settingItem{kind: settingPath, strVal: "/mnt/data/videos"}
	got := item.displayValue()
	if got != "/mnt/data/videos" {
		t.Errorf("displayValue() = %q, want non-home path unchanged", got)
	}
}

func TestSettingPathDisplayValueExactHomeDir(t *testing.T) {
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "/home/testuser"
	item := &settingItem{kind: settingPath, strVal: "/home/testuser"}
	got := item.displayValue()
	if got != "~" {
		t.Errorf("displayValue() for exact home dir = %q, want %q", got, "~")
	}
}

func TestSettingPathDisplayValueFallbackDot(t *testing.T) {
	// When settingsHomeDir is ".", no ~/prefix should be added.
	orig := settingsHomeDir
	defer func() { settingsHomeDir = orig }()

	settingsHomeDir = "."
	item := &settingItem{kind: settingPath, strVal: "/some/path"}
	got := item.displayValue()
	if got != "/some/path" {
		t.Errorf("displayValue() with homeDir='.': got %q, want raw path", got)
	}
}

func TestSettingPathToggleIsNoOp(t *testing.T) {
	// toggle() must not panic and must not change strVal for settingPath items.
	item := &settingItem{kind: settingPath, strVal: "/home/user/Videos"}
	item.toggle()
	if item.strVal != "/home/user/Videos" {
		t.Errorf("toggle() changed strVal: got %q", item.strVal)
	}
}

func TestSettingPathAdjustIsNoOp(t *testing.T) {
	// adjust() must not panic and must not change strVal for settingPath items.
	item := &settingItem{kind: settingPath, strVal: "/home/user/Videos"}
	item.adjust(1)
	item.adjust(-1)
	if item.strVal != "/home/user/Videos" {
		t.Errorf("adjust() changed strVal: got %q", item.strVal)
	}
}

func TestDownloadsCategoryExists(t *testing.T) {
	cats := defaultCategories()
	var found bool
	for _, c := range cats {
		if c.name == "Downloads" {
			found = true
			if len(c.items) != 2 {
				t.Errorf("Downloads category has %d items, want 2", len(c.items))
			}
			break
		}
	}
	if !found {
		t.Error("Downloads category not found in defaultCategories()")
	}
}

func TestDownloadsCategoryItemKeys(t *testing.T) {
	cats := defaultCategories()
	for _, c := range cats {
		if c.name == "Downloads" {
			keys := []string{c.items[0].key, c.items[1].key}
			want := []string{"downloads.video_dir", "downloads.music_dir"}
			for i, k := range keys {
				if k != want[i] {
					t.Errorf("item[%d].key = %q, want %q", i, k, want[i])
				}
			}
			return
		}
	}
	t.Error("Downloads category not found")
}

func TestDownloadsCategoryItemsAreSettingPath(t *testing.T) {
	cats := defaultCategories()
	for _, c := range cats {
		if c.name == "Downloads" {
			for _, item := range c.items {
				if item.kind != settingPath {
					t.Errorf("item %q has kind %v, want settingPath", item.key, item.kind)
				}
			}
			return
		}
	}
	t.Error("Downloads category not found")
}

func TestSettingChangedCmdPathEmitsString(t *testing.T) {
	item := &settingItem{kind: settingPath, key: "downloads.video_dir", strVal: "/home/user/Videos"}
	cmd := settingChangedCmd(item)
	if cmd == nil {
		t.Fatal("settingChangedCmd returned nil for settingPath item")
	}
	msg := cmd()
	scm, ok := msg.(SettingsChangedMsg)
	if !ok {
		t.Fatalf("expected SettingsChangedMsg, got %T", msg)
	}
	v, ok := scm.Value.(string)
	if !ok {
		t.Fatalf("expected Value to be string, got %T (nil means missing case)", scm.Value)
	}
	if v != "/home/user/Videos" {
		t.Errorf("Value = %q, want %q", v, "/home/user/Videos")
	}
}
