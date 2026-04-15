# Music Tag Normalization Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add opt-in virtual normalization (Action B) and an opt-in user-triggered "write normalized tags to files" action (Action A) to STUI's music library.

**Architecture:** A pure `normalize()` pipeline in the Rust runtime applies lookup-fill → exception filter → algorithmic rules to every MPD tag record. Exception list is a merged bundled+user TOML with lazy mtime-hash invalidation. Action A wraps the pipeline with `lofty-rs` writes, sidecar JSON backups, streamed progress events, and a scoped `mpd update` rescan. The Go TUI adds an action menu, a "mark as exception" keybind, and a preview diff view for Action A.

**Tech Stack:** Rust (runtime), Go / Bubble Tea (TUI), `lofty-rs` (tag read/write), `sha2` (exception hashing), `toml` (config + exception files), `once_cell` (global store), MPD (library backend).

**Spec:** `docs/superpowers/specs/2026-04-15-music-tag-normalization-design.md`

**Deferred to future work (v2):** Lookup integration via MusicBrainz/ListenBrainz. The existing `listenbrainz-provider` plugin exposes only `search`/`search_releases`/`resolve`/`get_charts` — not a per-recording lookup. Adding that would require changes to the plugin SDK and ABI, which is out of scope. The pipeline keeps `lookup: Option<&LookupResult>` as a parameter for forward compatibility but always passes `None` in v1. `[music.normalize.use_lookup]` is accepted in config but inert. When lookup lands, the only change needed is `fetch_batch()` in `lookup.rs`; everything downstream already handles lookup results.

---

## File Structure

**New Rust modules** (under `runtime/src/mediacache/`):
- `normalize/mod.rs` — pipeline entry: `normalize(raw, cfg, lookup) -> NormalizedTags`
- `normalize/rules.rs` — algorithmic rules (smart title case, whitespace, track parsing)
- `normalize/year.rs` — year extraction (promoted from `mpd_bridge/bridge.rs`)
- `normalize/unusual_case.rs` — unusual-case detection heuristic
- `normalize/exceptions.rs` — exception list loader + lazy mtime hash
- `normalize/lookup.rs` — `LookupResult` type + fill-gaps merge helper (v1 data type only; populated in v2)
- `normalize/store.rs` — process-wide `ExceptionStore` singleton
- `tag_writer.rs` — `lofty-rs` wrapper: read, backup, write
- `tag_write_job.rs` — diff builder, concurrent apply, cancellation, common-ancestor computation, job registry

**Modified Rust files:**
- `runtime/Cargo.toml` — add `lofty`, `once_cell`, verify `chrono`/`serde_json`/`thiserror`
- `runtime/src/mediacache/mod.rs` — re-export `normalize`, `tag_writer`, `tag_write_job`
- `runtime/src/mpd_bridge/bridge.rs` — remove local `extract_year`; apply pipeline; add raw-value pass-through
- `runtime/src/mpd_bridge/client.rs` — add `update_library(subpath: Option<&str>)`
- `runtime/src/config/types.rs` — add `MusicNormalizeConfig` hooked into root config
- `runtime/src/ipc/v1/mod.rs` — new request/response variants + streaming progress event
- `runtime/src/main.rs` (and/or IPC dispatcher module) — route new commands; initialize global stores; hold `JobStore`/`JobRegistry`

**New config file (bundled):**
- `config/exceptions.toml` — seeded defaults

**Modified Go TUI files:**
- `tui/internal/ipc/mpd_music.go` — wire types + client methods: `MarkTagException`, `ActionATagsPreview`, `ActionATagsApply`, `ActionATagsCancel`, event subscription for progress
- `tui/internal/ui/screens/music_library.go` — "Normalize tags on disk…" menu entry, `X` keybind for mark-as-exception

**New Go TUI file:**
- `tui/internal/ui/screens/tag_normalize_preview.go` — diff preview screen + teatest

---

## Chunk 1: Normalization primitives

### Task 1.1: Move `extract_year` into its own module

**Files:**
- Create: `runtime/src/mediacache/normalize/year.rs`
- Create: `runtime/src/mediacache/normalize/mod.rs`
- Modify: `runtime/src/mediacache/mod.rs`
- Modify: `runtime/src/mpd_bridge/bridge.rs`

- [ ] **Step 1: Write year module with tests**

Create `runtime/src/mediacache/normalize/year.rs`:

```rust
//! Year extraction from messy MPD `Date:` values.

/// Extract a 4-digit year from a date string.
///
/// Returns the first run of four consecutive digits whose first digit is
/// `1` or `2` (i.e. a year in 1000–2999). Empty string on no match.
/// Handles `2017`, `2017-05-03`, `May 2017`, `03-05-2017`, etc.
pub fn extract_year(date: &str) -> String {
    if date.is_empty() { return String::new(); }
    let bytes = date.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        let slice = &bytes[i..i + 4];
        if slice.iter().all(|b| b.is_ascii_digit()) {
            let first = slice[0];
            if first == b'1' || first == b'2' {
                return std::str::from_utf8(slice).unwrap_or("").to_string();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn plain_year() { assert_eq!(extract_year("2017"), "2017"); }
    #[test] fn iso_date() { assert_eq!(extract_year("2017-05-03"), "2017"); }
    #[test] fn month_year() { assert_eq!(extract_year("May 2017"), "2017"); }
    #[test] fn dmy() { assert_eq!(extract_year("03-05-2017"), "2017"); }
    #[test] fn empty() { assert_eq!(extract_year(""), ""); }
    #[test] fn two_digit_year() { assert_eq!(extract_year("03-05-19"), ""); }
    #[test] fn junk() { assert_eq!(extract_year("not a date"), ""); }
    #[test] fn year_out_of_range() { assert_eq!(extract_year("3500"), ""); }
    #[test] fn first_match_wins() { assert_eq!(extract_year("1999 or 2001?"), "1999"); }
    #[test] fn compact_date() { assert_eq!(extract_year("20170503"), "2017"); }
}
```

Create `runtime/src/mediacache/normalize/mod.rs`:

```rust
//! Music tag normalization pipeline.
//!
//! Pure functions only. No I/O. See docs/superpowers/specs/2026-04-15-music-tag-normalization-design.md.

pub mod year;
```

Replace `runtime/src/mediacache/mod.rs` with:

```rust
//! Media cache — persists catalog grid data locally so stui can show a
//! browseable offline library when providers are unreachable or the runtime
//! fails to start.

mod store;
pub mod normalize;

pub use store::MediaCacheStore;

pub fn default_cache_path() -> std::path::PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join("stui").join("mediacache.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("mediacache.json"))
}
```

- [ ] **Step 2: Run tests**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::normalize::year`
Expected: 10 tests PASS.

- [ ] **Step 3: Remove duplicate from `mpd_bridge/bridge.rs`**

Delete lines 53–71 of `runtime/src/mpd_bridge/bridge.rs` (the rustdoc + `fn extract_year`). At the top of the file, alongside other `use crate::...` imports, add:

```rust
use crate::mediacache::normalize::year::extract_year;
```

- [ ] **Step 4: Build + existing tests**

Run: `cd runtime && cargo build -p stui-runtime && cargo test -p stui-runtime`
Expected: build succeeds, existing tests still pass (call sites unchanged — same signature).

- [ ] **Step 5: Commit**

```bash
git add runtime/src/mediacache/mod.rs \
        runtime/src/mediacache/normalize/mod.rs \
        runtime/src/mediacache/normalize/year.rs \
        runtime/src/mpd_bridge/bridge.rs
git commit -m "refactor(mediacache): extract year helper into normalize module"
```

---

### Task 1.2: Unusual-case heuristic

**Files:**
- Create: `runtime/src/mediacache/normalize/unusual_case.rs`
- Modify: `runtime/src/mediacache/normalize/mod.rs`

- [ ] **Step 1: Write heuristic + tests**

Create `runtime/src/mediacache/normalize/unusual_case.rs`:

```rust
//! Detection heuristic for stylized casing that should NOT be auto-title-cased.
//!
//! Flags single-token strings like `deadmau5`, `AC/DC`, `MGMT`, `iamamiwhoami`
//! so the algorithmic rules skip them. Exception list further overrides this
//! both ways.
//!
//! DESIGN DECISION: only SINGLE-TOKEN strings (no spaces) are eligible to be
//! flagged. Multi-word all-caps ("DARK SIDE OF THE MOON") is almost always a
//! bad tag — we title-case it. Users with legitimate multi-word stylized
//! names use the exception list.

/// Returns true if the string's casing looks stylized/deliberate (single
/// token only) and should be left alone by the title-case pass.
pub fn is_unusual_case(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) { return false; }

    let mut has_upper = false;
    let mut has_lower = false;
    let mut has_digit_in_letters = false;
    let mut has_separator = false;
    let mut last_was_alpha = false;

    for ch in trimmed.chars() {
        if ch.is_ascii_uppercase() { has_upper = true; last_was_alpha = true; }
        else if ch.is_ascii_lowercase() { has_lower = true; last_was_alpha = true; }
        else if ch.is_ascii_digit() {
            if last_was_alpha { has_digit_in_letters = true; }
            last_was_alpha = false;
        } else {
            if ch == '/' || ch == '-' || ch == '.' || ch == '!' { has_separator = true; }
            last_was_alpha = false;
        }
    }

    // Digits embedded mid-word: always stylized (deadmau5, 3OH!3).
    if has_digit_in_letters { return true; }

    // All-uppercase single token with a non-alpha separator, length >= 2: AC/DC, MGMT-Z.
    let alpha_count = trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if has_upper && !has_lower && has_separator && alpha_count >= 2 {
        return true;
    }

    // All-uppercase single-token acronyms of length >= 3: MGMT, LCD.
    // Note: 2-letter tokens like "UK" are deliberately NOT flagged to avoid
    // false positives on country codes in titles.
    if has_upper && !has_lower && alpha_count >= 3 {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn acdc() { assert!(is_unusual_case("AC/DC")); }
    #[test] fn mgmt() { assert!(is_unusual_case("MGMT")); }
    #[test] fn deadmau5() { assert!(is_unusual_case("deadmau5")); }
    #[test] fn threeohthree() { assert!(is_unusual_case("3OH!3")); }
    #[test] fn empty() { assert!(!is_unusual_case("")); }
    #[test] fn whitespace() { assert!(!is_unusual_case("   ")); }
    #[test] fn all_lower() { assert!(!is_unusual_case("the beatles")); }
    #[test] fn title_case() { assert!(!is_unusual_case("The Beatles")); }
    #[test] fn sentence_case() { assert!(!is_unusual_case("Pink floyd")); }
    #[test] fn two_letter_caps() { assert!(!is_unusual_case("UK")); }
    #[test] fn mixed_with_apostrophe() { assert!(!is_unusual_case("don't stop")); }
    #[test] fn multi_word_screaming_caps_ignored() { assert!(!is_unusual_case("DARK SIDE OF THE MOON")); }
    #[test] fn single_token_screaming() { assert!(is_unusual_case("GENESIS")); }
}
```

Append to `runtime/src/mediacache/normalize/mod.rs`:

```rust
pub mod unusual_case;
```

- [ ] **Step 2: Run tests**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::normalize::unusual_case`
Expected: 13 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/mediacache/normalize/unusual_case.rs \
        runtime/src/mediacache/normalize/mod.rs
git commit -m "feat(normalize): unusual-case detection (single-token only)"
```

---

### Task 1.3: Smart title case + whitespace + track parsing

**Files:**
- Create: `runtime/src/mediacache/normalize/rules.rs`
- Modify: `runtime/src/mediacache/normalize/mod.rs`

- [ ] **Step 1: Write module + tests**

Create `runtime/src/mediacache/normalize/rules.rs`:

```rust
//! Algorithmic normalization rules.

use super::unusual_case::is_unusual_case;

const SMALL_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "of", "in", "on", "at",
    "to", "for", "by", "vs", "nor", "per", "via",
];

/// Smart title case: capitalize principal words, lowercase small words,
/// always capitalize first/last word. Respects the unusual-case heuristic.
pub fn smart_title_case(input: &str) -> String {
    let trimmed = collapse_whitespace(input.trim());
    if trimmed.is_empty() { return String::new(); }
    if is_unusual_case(&trimmed) { return trimmed; }

    let words: Vec<&str> = trimmed.split(' ').collect();
    let last_idx = words.len().saturating_sub(1);

    words.iter().enumerate().map(|(i, w)| {
        let lower = w.to_ascii_lowercase();
        let is_small = SMALL_WORDS.contains(&lower.as_str());
        if is_small && i != 0 && i != last_idx { lower } else { capitalize_word(w) }
    }).collect::<Vec<_>>().join(" ")
}

fn capitalize_word(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = String::with_capacity(w.len());
            out.extend(first.to_uppercase());
            out.extend(chars.map(|c| c.to_ascii_lowercase()));
            out
        }
    }
}

/// Collapse runs of whitespace to a single space. Does not trim ends.
pub fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws { out.push(' '); in_ws = true; }
        } else {
            out.push(ch); in_ws = false;
        }
    }
    out
}

/// Parse `N/M` forms to the integer N. Returns 0 on no parseable integer.
pub fn parse_track_or_disc(raw: &str) -> u32 {
    raw.split('/').next().unwrap_or("").trim().parse::<u32>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn titlecase_basic() { assert_eq!(smart_title_case("the beatles"), "The Beatles"); }
    #[test] fn titlecase_small_words() { assert_eq!(smart_title_case("a hard day's night"), "A Hard Day's Night"); }
    #[test] fn titlecase_small_word_last() { assert_eq!(smart_title_case("the long and winding road"), "The Long and Winding Road"); }
    #[test] fn titlecase_preserves_acronym() { assert_eq!(smart_title_case("AC/DC"), "AC/DC"); }
    #[test] fn titlecase_preserves_stylized() { assert_eq!(smart_title_case("deadmau5"), "deadmau5"); }
    #[test] fn titlecase_trims() { assert_eq!(smart_title_case("  pink floyd  "), "Pink Floyd"); }
    #[test] fn titlecase_collapses() { assert_eq!(smart_title_case("the    wall"), "The Wall"); }
    #[test] fn titlecase_fixes_screaming_multiword() { assert_eq!(smart_title_case("DARK SIDE OF THE MOON"), "Dark Side of the Moon"); }
    #[test] fn titlecase_empty() { assert_eq!(smart_title_case(""), ""); }

    #[test] fn collapse_basic() { assert_eq!(collapse_whitespace("a  b   c"), "a b c"); }
    #[test] fn collapse_tabs() { assert_eq!(collapse_whitespace("a\tb\nc"), "a b c"); }

    #[test] fn track_plain() { assert_eq!(parse_track_or_disc("3"), 3); }
    #[test] fn track_slash() { assert_eq!(parse_track_or_disc("3/12"), 3); }
    #[test] fn track_leading_zero() { assert_eq!(parse_track_or_disc("003"), 3); }
    #[test] fn track_empty() { assert_eq!(parse_track_or_disc(""), 0); }
    #[test] fn track_junk() { assert_eq!(parse_track_or_disc("foo"), 0); }
}
```

Append to `runtime/src/mediacache/normalize/mod.rs`:

```rust
pub mod rules;
```

- [ ] **Step 2: Run tests**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::normalize::rules`
Expected: all tests PASS.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/mediacache/normalize/rules.rs \
        runtime/src/mediacache/normalize/mod.rs
git commit -m "feat(normalize): smart title case, whitespace collapse, track parser"
```

---

## Chunk 2: Exception list

### Task 2.1: Exception types, loader, and store

**Files:**
- Create: `runtime/src/mediacache/normalize/exceptions.rs`
- Modify: `runtime/src/mediacache/normalize/mod.rs`
- Modify: `runtime/Cargo.toml` (dev-dep `tempfile`)

- [ ] **Step 1: Write module + tests**

Create `runtime/src/mediacache/normalize/exceptions.rs`:

```rust
//! Exception list loader.
//!
//! Two sources, merged at load time:
//!   1. Bundled: shipped with STUI, community-maintained.
//!   2. User:    `~/.config/stui/exceptions.toml`, auto-learn + manual edits.
//!
//! Membership tests are case-insensitive on pre-normalized raw values.
//! A SHA-256 content hash over merged bytes serves as a cache key.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

#[derive(Debug, Default, Deserialize, Serialize)]
struct FileShape {
    #[serde(default)] artist: FieldList,
    #[serde(default)] album_artist: FieldList,
    #[serde(default)] album: FieldList,
    #[serde(default)] title: FieldList,
    #[serde(default)] genre: FieldList,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FieldList {
    #[serde(default)] values: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ExceptionList {
    pub artist: HashSet<String>,
    pub album_artist: HashSet<String>,
    pub album: HashSet<String>,
    pub title: HashSet<String>,
    pub genre: HashSet<String>,
    /// SHA-256 hex of merged-source bytes, for cache keys.
    pub content_hash: String,
}

impl ExceptionList {
    pub fn is_artist_protected(&self, raw: &str) -> bool {
        self.artist.contains(&raw.to_lowercase())
    }
    pub fn is_album_artist_protected(&self, raw: &str) -> bool {
        self.album_artist.contains(&raw.to_lowercase())
    }
    pub fn is_album_protected(&self, raw: &str) -> bool {
        self.album.contains(&raw.to_lowercase())
    }
    pub fn is_title_protected(&self, raw: &str) -> bool {
        self.title.contains(&raw.to_lowercase())
    }
    pub fn is_genre_protected(&self, raw: &str) -> bool {
        self.genre.contains(&raw.to_lowercase())
    }
}

fn parse_file(bytes: &[u8]) -> FileShape {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    toml::from_str::<FileShape>(s).unwrap_or_default()
}

fn read_if_exists(path: &Path) -> Option<Vec<u8>> { fs::read(path).ok() }

pub fn merge(bundled: Option<&[u8]>, user: Option<&[u8]>) -> ExceptionList {
    let b = bundled.map(parse_file).unwrap_or_default();
    let u = user.map(parse_file).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bundled.unwrap_or_default());
    hasher.update(b":");
    hasher.update(user.unwrap_or_default());
    let content_hash = format!("{:x}", hasher.finalize());

    let lower = |vs: &[String]| vs.iter().map(|s| s.to_lowercase()).collect::<HashSet<_>>();
    let mut out = ExceptionList {
        artist: lower(&b.artist.values),
        album_artist: lower(&b.album_artist.values),
        album: lower(&b.album.values),
        title: lower(&b.title.values),
        genre: lower(&b.genre.values),
        content_hash,
    };
    out.artist.extend(u.artist.values.iter().map(|s| s.to_lowercase()));
    out.album_artist.extend(u.album_artist.values.iter().map(|s| s.to_lowercase()));
    out.album.extend(u.album.values.iter().map(|s| s.to_lowercase()));
    out.title.extend(u.title.values.iter().map(|s| s.to_lowercase()));
    out.genre.extend(u.genre.values.iter().map(|s| s.to_lowercase()));
    out
}

pub struct ExceptionStore {
    bundled_path: PathBuf,
    user_path: PathBuf,
    state: RwLock<Cached>,
    reload_lock: Mutex<()>,
}

#[derive(Default)]
struct Cached {
    list: ExceptionList,
    bundled_mtime: Option<SystemTime>,
    user_mtime: Option<SystemTime>,
    initialized: bool,
}

impl ExceptionStore {
    pub fn new(bundled_path: PathBuf, user_path: PathBuf) -> Self {
        Self {
            bundled_path, user_path,
            state: RwLock::new(Cached::default()),
            reload_lock: Mutex::new(()),
        }
    }

    pub fn get(&self) -> ExceptionList {
        let b_mt = fs::metadata(&self.bundled_path).and_then(|m| m.modified()).ok();
        let u_mt = fs::metadata(&self.user_path).and_then(|m| m.modified()).ok();
        {
            let st = self.state.read().unwrap();
            if st.initialized && st.bundled_mtime == b_mt && st.user_mtime == u_mt {
                return st.list.clone();
            }
        }
        let _g = self.reload_lock.lock().unwrap();
        {
            let st = self.state.read().unwrap();
            if st.initialized && st.bundled_mtime == b_mt && st.user_mtime == u_mt {
                return st.list.clone();
            }
        }
        let bundled = read_if_exists(&self.bundled_path);
        let user = read_if_exists(&self.user_path);
        let list = merge(bundled.as_deref(), user.as_deref());
        let mut st = self.state.write().unwrap();
        st.list = list.clone();
        st.bundled_mtime = b_mt;
        st.user_mtime = u_mt;
        st.initialized = true;
        list
    }

    pub fn add_user_exception(&self, field: ExceptionField, raw_value: &str) -> std::io::Result<bool> {
        let value = raw_value.trim();
        if value.is_empty() { return Ok(false); }

        let _g = self.reload_lock.lock().unwrap();
        let mut file: FileShape = fs::read(&self.user_path).ok()
            .and_then(|b| toml::from_str(std::str::from_utf8(&b).unwrap_or("")).ok())
            .unwrap_or_default();

        let list = match field {
            ExceptionField::Artist => &mut file.artist.values,
            ExceptionField::AlbumArtist => &mut file.album_artist.values,
            ExceptionField::Album => &mut file.album.values,
            ExceptionField::Title => &mut file.title.values,
            ExceptionField::Genre => &mut file.genre.values,
        };
        if list.iter().any(|v| v.eq_ignore_ascii_case(value)) { return Ok(false); }
        list.push(value.to_string());

        if let Some(parent) = self.user_path.parent() { let _ = fs::create_dir_all(parent); }
        let serialized = toml::to_string_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(&self.user_path, serialized)?;

        let mut st = self.state.write().unwrap();
        st.initialized = false;
        Ok(true)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ExceptionField { Artist, AlbumArtist, Album, Title, Genre }

impl ExceptionField {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "artist" => Some(Self::Artist),
            "album_artist" => Some(Self::AlbumArtist),
            "album" => Some(Self::Album),
            "title" => Some(Self::Title),
            "genre" => Some(Self::Genre),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_combines_sources() {
        let bundled = br#"
            [artist]
            values = ["AC/DC"]
            [album]
            values = []
        "#;
        let user = br#"
            [artist]
            values = ["deadmau5"]
        "#;
        let list = merge(Some(bundled), Some(user));
        assert!(list.is_artist_protected("ac/dc"));
        assert!(list.is_artist_protected("DEADMAU5"));
        assert!(!list.is_album_protected("anything"));
        assert!(!list.content_hash.is_empty());
    }

    #[test]
    fn merge_empty() {
        let list = merge(None, None);
        assert!(list.artist.is_empty());
        assert!(!list.content_hash.is_empty());
    }

    #[test]
    fn hash_changes_with_content() {
        let a = merge(Some(b"[artist]\nvalues = [\"x\"]\n"), None);
        let b = merge(Some(b"[artist]\nvalues = [\"y\"]\n"), None);
        assert_ne!(a.content_hash, b.content_hash);
    }

    #[test]
    fn add_user_exception_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("exceptions.toml");
        let store = ExceptionStore::new(PathBuf::from("/nonexistent"), user_path.clone());
        assert!(store.add_user_exception(ExceptionField::Artist, "deadmau5").unwrap());
        assert!(!store.add_user_exception(ExceptionField::Artist, "deadmau5").unwrap());
        let list = store.get();
        assert!(list.is_artist_protected("DEADMAU5"));
    }

    #[test]
    fn add_user_exception_empty_noop() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExceptionStore::new(
            PathBuf::from("/nonexistent"),
            dir.path().join("exceptions.toml"),
        );
        assert!(!store.add_user_exception(ExceptionField::Artist, "   ").unwrap());
    }
}
```

Ensure `runtime/Cargo.toml` has `tempfile = "3"` under `[dev-dependencies]`. Run `grep tempfile runtime/Cargo.toml` — if absent, add it under `[dev-dependencies]`.

Append to `runtime/src/mediacache/normalize/mod.rs`:

```rust
pub mod exceptions;
```

- [ ] **Step 2: Run tests**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::normalize::exceptions`
Expected: 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/mediacache/normalize/exceptions.rs \
        runtime/src/mediacache/normalize/mod.rs \
        runtime/Cargo.toml
git commit -m "feat(normalize): exception list loader with mtime-hash invalidation"
```

---

## Chunk 3: Pipeline

### Task 3.1: Lookup type (v2-ready scaffold)

**Files:**
- Create: `runtime/src/mediacache/normalize/lookup.rs`
- Modify: `runtime/src/mediacache/normalize/mod.rs`

- [ ] **Step 1: Write scaffold + tests**

Create `runtime/src/mediacache/normalize/lookup.rs`:

```rust
//! Lookup result type.
//!
//! v1: type is defined and wired into the pipeline, but callers always pass
//! `None` — per-recording lookup isn't exposed by any plugin yet. v2 will
//! add `fetch_batch()` to populate these from ListenBrainz/MusicBrainz.

#[derive(Debug, Clone, Default)]
pub struct LookupResult {
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
}

/// Copy `src` into `dst` only when `dst` is empty (or whitespace-only).
pub fn overwrite_if_empty(dst: &mut String, src: Option<&str>) {
    if dst.trim().is_empty() {
        if let Some(s) = src { *dst = s.to_string(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn fills_empty() {
        let mut s = String::new();
        overwrite_if_empty(&mut s, Some("Pink Floyd"));
        assert_eq!(s, "Pink Floyd");
    }
    #[test] fn preserves_existing() {
        let mut s = String::from("Pink Floyd");
        overwrite_if_empty(&mut s, Some("pink floyd"));
        assert_eq!(s, "Pink Floyd");
    }
    #[test] fn whitespace_treated_as_empty() {
        let mut s = String::from("   ");
        overwrite_if_empty(&mut s, Some("Pink Floyd"));
        assert_eq!(s, "Pink Floyd");
    }
}
```

Append to `runtime/src/mediacache/normalize/mod.rs`:

```rust
pub mod lookup;
```

- [ ] **Step 2: Run tests + commit**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::normalize::lookup`

```bash
git add runtime/src/mediacache/normalize/lookup.rs \
        runtime/src/mediacache/normalize/mod.rs
git commit -m "feat(normalize): lookup type scaffold"
```

---

### Task 3.2: Top-level `normalize()` pipeline

**Files:**
- Modify: `runtime/src/mediacache/normalize/mod.rs`

- [ ] **Step 1: Replace module top with pipeline + tests**

Replace the contents of `runtime/src/mediacache/normalize/mod.rs` with:

```rust
//! Music tag normalization pipeline.
//!
//! Pure functions only. No I/O. See docs/superpowers/specs/2026-04-15-music-tag-normalization-design.md.

pub mod exceptions;
pub mod lookup;
pub mod rules;
pub mod unusual_case;
pub mod year;

use exceptions::ExceptionList;
use lookup::{overwrite_if_empty, LookupResult};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RawTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub date: String,
    pub genre: String,
    pub track: String,
    pub disc: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NormalizedTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub year: String,
    pub genre: String,
    pub track: u32,
    pub disc: u32,
}

pub struct NormalizationConfig<'a> {
    pub enabled: bool,
    pub use_lookup: bool,
    pub exceptions: &'a ExceptionList,
}

/// Apply the pipeline.
///
/// When `cfg.enabled == false`: returns a trivial conversion (year extracted,
/// numerics parsed). No casing/whitespace normalization. This preserves
/// existing behavior for the disabled path.
pub fn normalize(
    raw: &RawTags,
    cfg: &NormalizationConfig,
    lookup: Option<&LookupResult>,
) -> NormalizedTags {
    let mut artist = raw.artist.clone();
    let mut album_artist = raw.album_artist.clone();
    let mut album = raw.album.clone();
    let mut title = raw.title.clone();
    let mut date = raw.date.clone();
    let mut genre = raw.genre.clone();

    if cfg.enabled && cfg.use_lookup {
        if let Some(l) = lookup {
            overwrite_if_empty(&mut artist, l.artist.as_deref());
            overwrite_if_empty(&mut album_artist, l.album_artist.as_deref());
            overwrite_if_empty(&mut album, l.album.as_deref());
            overwrite_if_empty(&mut title, l.title.as_deref());
            overwrite_if_empty(&mut date, l.year.as_deref());
            overwrite_if_empty(&mut genre, l.genre.as_deref());
        }
    }

    if cfg.enabled {
        if !cfg.exceptions.is_artist_protected(&raw.artist) {
            artist = rules::smart_title_case(&artist);
        }
        if !cfg.exceptions.is_album_artist_protected(&raw.album_artist) {
            album_artist = rules::smart_title_case(&album_artist);
        }
        if !cfg.exceptions.is_album_protected(&raw.album) {
            album = rules::smart_title_case(&album);
        }
        if !cfg.exceptions.is_title_protected(&raw.title) {
            title = rules::smart_title_case(&title);
        }
        if !cfg.exceptions.is_genre_protected(&raw.genre) {
            genre = rules::smart_title_case(&genre);
        }
    }

    NormalizedTags {
        artist, album_artist, album, title,
        year: year::extract_year(&date),
        genre,
        track: rules::parse_track_or_disc(&raw.track),
        disc: rules::parse_track_or_disc(&raw.disc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_exceptions() -> ExceptionList { ExceptionList::default() }
    fn cfg_on<'a>(ex: &'a ExceptionList) -> NormalizationConfig<'a> {
        NormalizationConfig { enabled: true, use_lookup: false, exceptions: ex }
    }
    fn cfg_off<'a>(ex: &'a ExceptionList) -> NormalizationConfig<'a> {
        NormalizationConfig { enabled: false, use_lookup: false, exceptions: ex }
    }

    #[test]
    fn disabled_extracts_year_only() {
        let raw = RawTags { artist: "pink floyd".into(), album: "the wall".into(),
            date: "1979-11-30".into(), track: "3/12".into(), ..Default::default() };
        let ex = empty_exceptions();
        let out = normalize(&raw, &cfg_off(&ex), None);
        assert_eq!(out.artist, "pink floyd");
        assert_eq!(out.album, "the wall");
        assert_eq!(out.year, "1979");
        assert_eq!(out.track, 3);
    }

    #[test]
    fn enabled_title_cases() {
        let raw = RawTags { artist: "pink floyd".into(), album: "the wall".into(), ..Default::default() };
        let ex = empty_exceptions();
        let out = normalize(&raw, &cfg_on(&ex), None);
        assert_eq!(out.artist, "Pink Floyd");
        assert_eq!(out.album, "The Wall");
    }

    #[test]
    fn exception_vetoes_artist() {
        let raw = RawTags { artist: "deadmau5".into(), album: "random album name".into(), ..Default::default() };
        let mut ex = ExceptionList::default();
        ex.artist.insert("deadmau5".to_string());
        let out = normalize(&raw, &cfg_on(&ex), None);
        assert_eq!(out.artist, "deadmau5");
        assert_eq!(out.album, "Random Album Name");
    }

    #[test]
    fn unusual_case_preserved_without_exception() {
        let raw = RawTags { artist: "AC/DC".into(), ..Default::default() };
        let ex = empty_exceptions();
        let out = normalize(&raw, &cfg_on(&ex), None);
        assert_eq!(out.artist, "AC/DC");
    }

    #[test]
    fn lookup_fills_missing_fields() {
        let raw = RawTags { artist: "pink floyd".into(), ..Default::default() };
        let look = LookupResult { album: Some("the wall".into()), ..Default::default() };
        let ex = empty_exceptions();
        let cfg = NormalizationConfig { enabled: true, use_lookup: true, exceptions: &ex };
        let out = normalize(&raw, &cfg, Some(&look));
        assert_eq!(out.album, "The Wall");
    }

    #[test]
    fn lookup_does_not_overwrite() {
        let raw = RawTags { album: "Already Here".into(), ..Default::default() };
        let look = LookupResult { album: Some("Different Value".into()), ..Default::default() };
        let ex = empty_exceptions();
        let cfg = NormalizationConfig { enabled: true, use_lookup: true, exceptions: &ex };
        let out = normalize(&raw, &cfg, Some(&look));
        assert_eq!(out.album, "Already Here");
    }

    #[test]
    fn lookup_full_date_still_year_extracted() {
        let raw = RawTags::default();
        let look = LookupResult { year: Some("2017-05-03".into()), ..Default::default() };
        let ex = empty_exceptions();
        let cfg = NormalizationConfig { enabled: true, use_lookup: true, exceptions: &ex };
        let out = normalize(&raw, &cfg, Some(&look));
        assert_eq!(out.year, "2017");
    }
}
```

- [ ] **Step 2: Run all normalize tests**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::normalize`
Expected: all tests PASS.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/mediacache/normalize/mod.rs
git commit -m "feat(normalize): top-level pipeline with lookup fill + exception veto"
```

---

## Chunk 4: Config + bundled defaults + global store

### Task 4.1: Config schema

**Files:**
- Modify: `runtime/src/config/types.rs`

- [ ] **Step 1: Add music normalize config**

In `runtime/src/config/types.rs`, find the existing `MpdConfig` struct (line 435). Near other config struct definitions, append:

```rust
/// Music tag normalization configuration (`[music.normalize]` section).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicNormalizeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "defaults::normalize_use_lookup")]
    pub use_lookup: bool,
}

impl Default for MusicNormalizeConfig {
    fn default() -> Self {
        Self { enabled: false, use_lookup: defaults::normalize_use_lookup() }
    }
}

/// `[music]` section wrapper.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MusicConfig {
    #[serde(default)]
    pub normalize: MusicNormalizeConfig,
}
```

In the same file, inside the `mod defaults { ... }` block, add:

```rust
pub(super) fn normalize_use_lookup() -> bool { true }
```

Locate the root `Config` struct (find via `grep -n "pub struct.*Config.*{" runtime/src/config/types.rs` — look for the one containing `pub mpd: MpdConfig`). Add a field:

```rust
    #[serde(default)]
    pub music: MusicConfig,
```

Update the `Default` impl for that root struct to initialize `music: MusicConfig::default()`.

- [ ] **Step 2: Build + existing config tests**

Run: `cd runtime && cargo build -p stui-runtime && cargo test -p stui-runtime --lib config`
Expected: build succeeds; existing tests pass.

- [ ] **Step 3: Round-trip test**

Add to `runtime/src/config/types.rs` under `#[cfg(test)] mod tests { ... }` (or create if missing):

```rust
#[test]
fn music_normalize_defaults() {
    let s = "";
    let c: super::MusicConfig = toml::from_str(s).unwrap();
    assert!(!c.normalize.enabled);
    assert!(c.normalize.use_lookup);
}

#[test]
fn music_normalize_round_trip() {
    let s = r#"
        [normalize]
        enabled = true
        use_lookup = false
    "#;
    let c: super::MusicConfig = toml::from_str(s).unwrap();
    assert!(c.normalize.enabled);
    assert!(!c.normalize.use_lookup);
}
```

Run: `cd runtime && cargo test -p stui-runtime --lib config::types`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add runtime/src/config/types.rs
git commit -m "feat(config): add [music.normalize] section"
```

---

### Task 4.2: Bundled exception list

**Files:**
- Create: `config/exceptions.toml`

- [ ] **Step 1: Seed file**

Create `config/exceptions.toml`:

```toml
# Bundled exception list — community-maintained.
# Values here are NEVER title-cased by the normalizer. Case-insensitive match
# on the raw (pre-normalized) MPD tag value.
#
# Users: add your own protections to ~/.config/stui/exceptions.toml instead
# of editing this file — so you won't lose them on upgrade.
#
# PRs welcome.

[artist]
values = [
    "deadmau5",
    "AC/DC",
    "MGMT",
    "iamamiwhoami",
    "3OH!3",
    "!!!",
    "will.i.am",
    "t.A.T.u.",
]

[album_artist]
values = []

[album]
values = [
    "untitled unmastered.",
]

[title]
values = []

[genre]
values = []
```

- [ ] **Step 2: Commit**

```bash
git add config/exceptions.toml
git commit -m "feat(normalize): seed bundled exception list"
```

---

### Task 4.3: Process-wide `ExceptionStore`

**Files:**
- Create: `runtime/src/mediacache/normalize/store.rs`
- Modify: `runtime/src/mediacache/normalize/mod.rs`
- Modify: `runtime/Cargo.toml` (add `once_cell` if missing)

- [ ] **Step 1: Verify `once_cell` dep**

Run: `grep once_cell runtime/Cargo.toml`. If absent, add under `[dependencies]`:

```toml
once_cell = "1"
```

- [ ] **Step 2: Global store module**

Create `runtime/src/mediacache/normalize/store.rs`:

```rust
//! Process-wide `ExceptionStore` singleton.

use once_cell::sync::OnceCell;
use std::path::PathBuf;
use std::sync::Arc;

use super::exceptions::ExceptionStore;

static STORE: OnceCell<Arc<ExceptionStore>> = OnceCell::new();

pub fn init(bundled_path: PathBuf, user_path: PathBuf) -> Arc<ExceptionStore> {
    STORE.get_or_init(|| Arc::new(ExceptionStore::new(bundled_path, user_path))).clone()
}

pub fn global() -> Option<Arc<ExceptionStore>> { STORE.get().cloned() }

/// Default bundled path:
///   1. `<CARGO_MANIFEST_DIR>/../config/exceptions.toml` (dev checkout).
///   2. `/usr/share/stui/exceptions.toml` (installed).
/// First existing one wins; otherwise returns (1) so error messages are useful.
pub fn default_bundled_path() -> PathBuf {
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("config").join("exceptions.toml"))
        .unwrap_or_else(|| PathBuf::from("config/exceptions.toml"));
    if dev.exists() { return dev; }
    let installed = PathBuf::from("/usr/share/stui/exceptions.toml");
    if installed.exists() { return installed; }
    dev
}

pub fn default_user_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join("stui").join("exceptions.toml"))
        .unwrap_or_else(|| PathBuf::from("exceptions.toml"))
}
```

Append to `runtime/src/mediacache/normalize/mod.rs`:

```rust
pub mod store;
```

- [ ] **Step 3: Initialize at runtime startup**

Find the runtime's startup entry — typically a `fn main()` or `async fn run()` in `runtime/src/main.rs`. Near the start (after logging init, before the IPC server), add:

```rust
crate::mediacache::normalize::store::init(
    crate::mediacache::normalize::store::default_bundled_path(),
    crate::mediacache::normalize::store::default_user_path(),
);
```

- [ ] **Step 4: Build + commit**

Run: `cd runtime && cargo build -p stui-runtime`

```bash
git add runtime/src/mediacache/normalize/store.rs \
        runtime/src/mediacache/normalize/mod.rs \
        runtime/src/main.rs \
        runtime/Cargo.toml runtime/Cargo.lock
git commit -m "feat(normalize): process-wide exception store + bundled path resolution"
```

---

### Task 4.4: Wire pipeline into `mpd_bridge` album listings

**Files:**
- Modify: `runtime/src/mpd_bridge/bridge.rs`
- Modify: `runtime/src/ipc/v1/mod.rs`

- [ ] **Step 1: Add raw-value pass-through fields to `MpdAlbumWire`**

In `runtime/src/ipc/v1/mod.rs`, find `pub struct MpdAlbumWire` (line 646). Add AFTER the existing fields:

```rust
    /// Pre-normalized artist value, populated only when normalization changed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_artist: String,
    /// Pre-normalized album title, populated only when normalization changed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_title: String,
```

(`MpdAlbumWire` has only `title`, `artist`, `year`, `date` — album-artist/genre aren't exposed at this level so no `raw_album_artist`/`raw_genre` needed here. Track-level raw values go on `MpdSongWire`/`MpdDirEntryWire` in Task 4.5.)

- [ ] **Step 2: Add `MusicNormalizeConfig` to the bridge**

At the top of `runtime/src/mpd_bridge/bridge.rs`, alongside other `use` statements, add:

```rust
use crate::config::types::MusicNormalizeConfig;
use crate::mediacache::normalize::{self, store as norm_store, NormalizationConfig, RawTags};
```

Find `pub struct MpdBridge { ... }`. Add the field:

```rust
    normalize_cfg: MusicNormalizeConfig,
```

Update `impl MpdBridge { pub fn new(...) -> Self { ... } }` to accept `normalize_cfg: MusicNormalizeConfig` as the last parameter and store it. Update every call site (grep: `MpdBridge::new`) to pass the current value from the loaded `Config`.

- [ ] **Step 3: Apply pipeline after album list is built**

In the `list_albums`/`albums_by_artist` method (around line 317–390 per the original grep) — find the point where `out: Vec<MpdAlbumWire>` is fully populated and just before it's returned. Insert:

```rust
if self.normalize_cfg.enabled {
    let exceptions = norm_store::global().map(|s| s.get()).unwrap_or_default();
    for album in out.iter_mut() {
        let raw = RawTags {
            artist: album.artist.clone(),
            album: album.title.clone(),
            date: album.date.clone(),
            ..Default::default()
        };
        let cfg = NormalizationConfig {
            enabled: true,
            use_lookup: self.normalize_cfg.use_lookup,
            exceptions: &exceptions,
        };
        let n = normalize::normalize(&raw, &cfg, None);
        if n.artist != album.artist {
            album.raw_artist = album.artist.clone();
            album.artist = n.artist;
        }
        if n.album != album.title {
            album.raw_title = album.title.clone();
            album.title = n.album;
        }
        // year may be re-extracted but should be identical; trust pipeline.
        album.year = n.year;
    }
}
```

- [ ] **Step 4: Build + test**

Run: `cd runtime && cargo build -p stui-runtime && cargo test -p stui-runtime`
Expected: build succeeds, tests pass.

- [ ] **Step 5: Commit**

```bash
git add runtime/src/ipc/v1/mod.rs \
        runtime/src/mpd_bridge/bridge.rs \
        runtime/src/main.rs
git commit -m "feat(mpd_bridge): normalize album listings when [music.normalize.enabled]"
```

---

### Task 4.5: Wire pipeline into browse + song listings

**Files:**
- Modify: `runtime/src/mpd_bridge/bridge.rs`
- Modify: `runtime/src/ipc/v1/mod.rs`

- [ ] **Step 1: Raw-value fields for `MpdSongWire` and `MpdDirEntryWire`**

In `runtime/src/ipc/v1/mod.rs`, find `MpdSongWire` (line 660). Add AFTER existing fields:

```rust
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_artist: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_album: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_title: String,
```

Find `MpdDirEntryWire` (line 670). Add the same three fields.

- [ ] **Step 2: Extract a helper**

In `runtime/src/mpd_bridge/bridge.rs`, near the bottom (outside `impl MpdBridge`), add:

```rust
/// Normalize a song-like record in place. Stashes raw values when the pipeline
/// changes a field. No-op when `cfg.enabled == false`.
fn apply_song_normalize(
    cfg: &MusicNormalizeConfig,
    artist: &mut String, raw_artist: &mut String,
    album: &mut String, raw_album: &mut String,
    title: &mut String, raw_title: &mut String,
) {
    if !cfg.enabled { return; }
    let exceptions = norm_store::global().map(|s| s.get()).unwrap_or_default();
    let raw = RawTags {
        artist: artist.clone(),
        album: album.clone(),
        title: title.clone(),
        ..Default::default()
    };
    let nc = NormalizationConfig {
        enabled: true,
        use_lookup: cfg.use_lookup,
        exceptions: &exceptions,
    };
    let n = normalize::normalize(&raw, &nc, None);
    if n.artist != *artist { *raw_artist = artist.clone(); *artist = n.artist; }
    if n.album != *album   { *raw_album  = album.clone();  *album  = n.album; }
    if n.title != *title   { *raw_title  = title.clone();  *title  = n.title; }
}
```

- [ ] **Step 3: Call helper in every listing method**

In each method that populates `MpdSongWire` or `MpdDirEntryWire` results (grep: `MpdSongWire {`, `MpdDirEntryWire {`), after constructing each item add:

```rust
apply_song_normalize(
    &self.normalize_cfg,
    &mut item.artist, &mut item.raw_artist,
    &mut item.album,  &mut item.raw_album,
    &mut item.title,  &mut item.raw_title,
);
```

- [ ] **Step 4: Build + smoke test**

Run: `cd runtime && cargo build -p stui-runtime && cargo test -p stui-runtime`

Manual: start stui with `[music.normalize] enabled = true`, browse a directory, confirm song/dir entries show normalized titles; with `enabled = false`, confirm they show raw values.

- [ ] **Step 5: Commit**

```bash
git add runtime/src/ipc/v1/mod.rs runtime/src/mpd_bridge/bridge.rs
git commit -m "feat(mpd_bridge): normalize song + browse listings with raw-value pass-through"
```

---

## Chunk 5: Tag writer

### Task 5.1: Add `lofty` dependency

**Files:**
- Modify: `runtime/Cargo.toml`

- [ ] **Step 1: Add dep + verify transitive deps**

Under `[dependencies]`:

```toml
lofty = "0.22"
```

Verify these already exist (grep `chrono`, `serde_json`, `thiserror`): if any missing, add:

```toml
chrono = { version = "0.4", features = ["serde"] }
serde_json = "1"
thiserror = "1"
```

Run: `cd runtime && cargo build -p stui-runtime`
Expected: compiles successfully.

- [ ] **Step 2: Commit**

```bash
git add runtime/Cargo.toml runtime/Cargo.lock
git commit -m "chore(deps): add lofty for tag read/write"
```

---

### Task 5.2: Tag writer module

**Files:**
- Create: `runtime/src/mediacache/tag_writer.rs`
- Create: `runtime/tests/fixtures/sample.mp3` (binary fixture via ffmpeg)
- Modify: `runtime/src/mediacache/mod.rs`

- [ ] **Step 1: Generate test fixture**

Run:

```bash
mkdir -p runtime/tests/fixtures
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=0.1" \
       -metadata artist="pink floyd" \
       -metadata album="the wall" \
       -metadata title="comfortably numb" \
       -c:a libmp3lame runtime/tests/fixtures/sample.mp3
```

Verify: `ls -la runtime/tests/fixtures/sample.mp3` — should be small (< 10 KB).

- [ ] **Step 2: Write tag_writer module + tests**

Create `runtime/src/mediacache/tag_writer.rs`:

```rust
//! Writes normalized tags to audio files on disk via lofty.
//!
//! Per-file flow:
//!   1. Read original tags via lofty.
//!   2. Write sidecar JSON backup (if not already present).
//!   3. Write new tags via lofty.
//!
//! Primary backup: `<file>.stui-tag-backup.json`.
//! Fallback (when audio dir is read-only): `~/.local/share/stui/tag-backups/<sha256>.json`.

use lofty::{
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{Accessor, ItemKey, TagExt},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use super::normalize::NormalizedTags;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OriginalTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub year: String,
    pub genre: String,
    pub track: u32,
    pub disc: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SidecarBackup {
    pub created: String,
    pub stui_version: String,
    pub original: OriginalTags,
}

#[derive(Debug, Clone)]
pub enum BackupLocation {
    Sidecar(PathBuf),
    Central(PathBuf),
}

#[derive(Debug)]
pub struct WriteReport {
    pub path: PathBuf,
    pub backup_location: BackupLocation,
    pub wrote_backup: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum TagWriteError {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("lofty: {0}")] Lofty(#[from] lofty::error::LoftyError),
    #[error("json: {0}")] Json(#[from] serde_json::Error),
    #[error("no tag in file")] NoTag,
    #[error("backup write failed; tag write skipped for safety")] BackupFailed,
}

pub fn read_original(path: &Path) -> Result<OriginalTags, TagWriteError> {
    let tagged = Probe::open(path)?.read()?;
    let tag = tagged.primary_tag().or(tagged.first_tag()).ok_or(TagWriteError::NoTag)?;
    Ok(OriginalTags {
        artist: tag.artist().map(|c| c.to_string()).unwrap_or_default(),
        album_artist: tag.get_string(&ItemKey::AlbumArtist).unwrap_or_default().to_string(),
        album: tag.album().map(|c| c.to_string()).unwrap_or_default(),
        title: tag.title().map(|c| c.to_string()).unwrap_or_default(),
        year: tag.year().map(|y| y.to_string()).unwrap_or_default(),
        genre: tag.genre().map(|c| c.to_string()).unwrap_or_default(),
        track: tag.track().unwrap_or(0),
        disc: tag.disk().unwrap_or(0),
    })
}

fn sidecar_path_beside(audio: &Path) -> PathBuf {
    let mut p = audio.as_os_str().to_os_string();
    p.push(".stui-tag-backup.json");
    PathBuf::from(p)
}

fn sidecar_path_central(audio: &Path) -> PathBuf {
    let abs = audio.canonicalize().unwrap_or_else(|_| audio.to_path_buf());
    let mut h = Sha256::new();
    h.update(abs.to_string_lossy().as_bytes());
    let name = format!("{:x}.json", h.finalize());
    let dir = dirs::data_local_dir()
        .map(|d| d.join("stui").join("tag-backups"))
        .unwrap_or_else(|| PathBuf::from(".stui-tag-backups"));
    dir.join(name)
}

fn write_backup_once(audio: &Path, original: &OriginalTags) -> Result<(bool, BackupLocation), TagWriteError> {
    let side = sidecar_path_beside(audio);
    if side.exists() { return Ok((false, BackupLocation::Sidecar(side))); }

    let payload = SidecarBackup {
        created: chrono::Utc::now().to_rfc3339(),
        stui_version: env!("CARGO_PKG_VERSION").to_string(),
        original: original.clone(),
    };
    let json = serde_json::to_vec_pretty(&payload)?;

    match fs::write(&side, &json) {
        Ok(()) => Ok((true, BackupLocation::Sidecar(side))),
        Err(_) => {
            let central = sidecar_path_central(audio);
            if central.exists() { return Ok((false, BackupLocation::Central(central))); }
            if let Some(parent) = central.parent() { fs::create_dir_all(parent)?; }
            fs::write(&central, &json)?;
            Ok((true, BackupLocation::Central(central)))
        }
    }
}

pub fn write_normalized(path: &Path, n: &NormalizedTags) -> Result<WriteReport, TagWriteError> {
    let original = read_original(path)?;
    let (wrote_backup, backup_location) = write_backup_once(path, &original)
        .map_err(|_| TagWriteError::BackupFailed)?;

    let mut tagged = Probe::open(path)?.read()?;
    let tag = tagged.primary_tag_mut().ok_or(TagWriteError::NoTag)?;

    tag.set_artist(n.artist.clone());
    tag.insert_text(ItemKey::AlbumArtist, n.album_artist.clone());
    tag.set_album(n.album.clone());
    tag.set_title(n.title.clone());
    if let Ok(y) = n.year.parse::<u32>() { tag.set_year(y); }
    tag.set_genre(n.genre.clone());
    if n.track > 0 { tag.set_track(n.track); }
    if n.disc > 0 { tag.set_disk(n.disc); }

    tag.save_to_path(path, lofty::config::WriteOptions::default())?;

    tracing::info!(
        path = %path.display(),
        backup = ?backup_location,
        wrote_backup,
        "tag_writer: wrote normalized tags",
    );

    Ok(WriteReport { path: path.to_path_buf(), backup_location, wrote_backup })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_bytes() -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests").join("fixtures").join("sample.mp3");
        assert!(path.exists(),
            "fixture missing at {}; regenerate via:\n  \
             ffmpeg -f lavfi -i \"sine=frequency=440:duration=0.1\" \
             -metadata artist=\"pink floyd\" -metadata album=\"the wall\" \
             -metadata title=\"comfortably numb\" -c:a libmp3lame \
             runtime/tests/fixtures/sample.mp3",
            path.display());
        fs::read(&path).unwrap()
    }

    #[test]
    fn writes_backup_and_tags() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.mp3");
        let mut f = fs::File::create(&file).unwrap();
        f.write_all(&fixture_bytes()).unwrap();
        drop(f);

        let n = NormalizedTags {
            artist: "Pink Floyd".into(),
            album: "The Wall".into(),
            title: "Comfortably Numb".into(),
            year: "1979".into(),
            ..Default::default()
        };
        let report = write_normalized(&file, &n).unwrap();
        assert!(report.wrote_backup);
        match &report.backup_location {
            BackupLocation::Sidecar(p) => assert!(p.exists()),
            BackupLocation::Central(p) => assert!(p.exists()),
        }

        let o = read_original(&file).unwrap();
        assert_eq!(o.artist, "Pink Floyd");
        assert_eq!(o.album, "The Wall");
        assert_eq!(o.title, "Comfortably Numb");

        // Backup must not be overwritten on second write.
        let report2 = write_normalized(&file, &n).unwrap();
        assert!(!report2.wrote_backup);
    }
}
```

Append to `runtime/src/mediacache/mod.rs`:

```rust
pub mod tag_writer;
```

- [ ] **Step 3: Run tests**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::tag_writer`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add runtime/src/mediacache/tag_writer.rs \
        runtime/src/mediacache/mod.rs \
        runtime/tests/fixtures/sample.mp3
git commit -m "feat(mediacache): tag writer with sidecar backup + central fallback"
```

---

### Task 5.3: MPD `update_library` command

**Files:**
- Modify: `runtime/src/mpd_bridge/client.rs` (or wherever `MpdClient` is defined)

- [ ] **Step 1: Add method**

Find the `MpdClient` impl (grep: `impl MpdClient`). Add a method:

```rust
/// Trigger MPD's `update` command, optionally scoped to a subpath.
/// Returns the job ID string returned by MPD. Fire-and-forget is fine —
/// MPD handles the rescan in the background.
pub async fn update_library(&self, subpath: Option<&str>) -> Result<String> {
    let cmd = match subpath {
        Some(p) if !p.is_empty() => format!("update \"{}\"\n", p.replace('"', "\\\"")),
        _ => "update\n".to_string(),
    };
    let resp = self.send_raw(&cmd).await?;
    // Response: "updating_db: 1\nOK\n" — extract the jobid.
    for line in resp.lines() {
        if let Some(rest) = line.strip_prefix("updating_db: ") {
            return Ok(rest.trim().to_string());
        }
    }
    Ok(String::new())
}
```

(`send_raw` or equivalent already exists — match the style of existing commands in that file. If the client uses a typed command enum, add a variant.)

- [ ] **Step 2: Expose on the bridge**

In `runtime/src/mpd_bridge/bridge.rs`, add a pass-through method:

```rust
pub async fn update_library(&self, subpath: Option<&str>) -> Result<String> {
    self.client.update_library(subpath).await
}
```

- [ ] **Step 3: Build + commit**

Run: `cd runtime && cargo build -p stui-runtime`

```bash
git add runtime/src/mpd_bridge/client.rs runtime/src/mpd_bridge/bridge.rs
git commit -m "feat(mpd_bridge): scoped update_library command"
```

---

## Chunk 6: Tag-write job infrastructure

### Task 6.1: Diff builder + common ancestor

**Files:**
- Create: `runtime/src/mediacache/tag_write_job.rs`
- Modify: `runtime/src/mediacache/mod.rs`

- [ ] **Step 1: Write module + tests**

Create `runtime/src/mediacache/tag_write_job.rs`:

```rust
//! Tag-write job: builds a diff, applies writes concurrently, supports cancel.
//!
//! Separation of concerns:
//!   - `build_diff`: pure. Turns (file, RawTags) into DiffRow skipping no-ops.
//!   - `to_wire_rows`: serialize DiffRows as one-row-per-changed-field.
//!   - `apply`: concurrent write execution. Reports (succeeded, failed, skipped).
//!   - `common_ancestor`: for scoping MPD rescan.
//!   - `JobStore` / `JobRegistry`: per-job state and cancellation flags.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::ipc::v1::TagDiffRowWire;
use crate::mediacache::normalize::{self, lookup::LookupResult, NormalizationConfig, RawTags};
use crate::mediacache::tag_writer;

#[derive(Debug, Clone)]
pub struct DiffRow {
    pub file: PathBuf,
    pub raw: RawTags,
    pub normalized: normalize::NormalizedTags,
}

/// Skip rows where the pipeline output equals the raw input byte-for-byte.
pub fn build_diff(
    files: Vec<(PathBuf, RawTags)>,
    cfg: &NormalizationConfig,
    lookups: &HashMap<PathBuf, LookupResult>,
) -> Vec<DiffRow> {
    let mut out = Vec::with_capacity(files.len());
    for (file, raw) in files {
        let lookup = lookups.get(&file);
        let normalized = normalize::normalize(&raw, cfg, lookup);
        if normalized_equals_raw(&normalized, &raw) { continue; }
        out.push(DiffRow { file, raw, normalized });
    }
    out
}

fn normalized_equals_raw(n: &normalize::NormalizedTags, r: &RawTags) -> bool {
    n.artist == r.artist
        && n.album_artist == r.album_artist
        && n.album == r.album
        && n.title == r.title
        && n.year == normalize::year::extract_year(&r.date)
        && n.genre == r.genre
        && n.track == normalize::rules::parse_track_or_disc(&r.track)
        && n.disc == normalize::rules::parse_track_or_disc(&r.disc)
}

pub fn to_wire_rows(rows: &[DiffRow]) -> Vec<TagDiffRowWire> {
    let mut out = Vec::new();
    for row in rows {
        let f = row.file.to_string_lossy().to_string();
        push_if_diff(&mut out, &f, "artist", &row.raw.artist, &row.normalized.artist);
        push_if_diff(&mut out, &f, "album_artist", &row.raw.album_artist, &row.normalized.album_artist);
        push_if_diff(&mut out, &f, "album", &row.raw.album, &row.normalized.album);
        push_if_diff(&mut out, &f, "title", &row.raw.title, &row.normalized.title);
        push_if_diff(&mut out, &f, "year", &row.raw.date, &row.normalized.year);
        push_if_diff(&mut out, &f, "genre", &row.raw.genre, &row.normalized.genre);
    }
    out
}

fn push_if_diff(out: &mut Vec<TagDiffRowWire>, file: &str, field: &str, old: &str, new: &str) {
    if old != new {
        out.push(TagDiffRowWire {
            file: file.to_string(),
            field: field.to_string(),
            old_value: old.to_string(),
            new_value: new.to_string(),
        });
    }
}

pub fn common_ancestor(files: &[PathBuf]) -> Option<PathBuf> {
    let mut iter = files.iter();
    let first = iter.next()?.clone();
    // Seed with the first file's parent (or itself if no parent).
    let mut common: PathBuf = first.parent().map(|p| p.to_path_buf()).unwrap_or(first.clone());
    for f in iter {
        while !f.starts_with(&common) {
            if !common.pop() { return None; }
        }
    }
    Some(common)
}

// ── Job state ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApplyOutcome {
    pub succeeded: usize,
    pub failed: Vec<PathBuf>,
    pub skipped_cancelled: usize,
}

#[derive(Default)]
pub struct JobStore {
    inner: Mutex<HashMap<String, Vec<DiffRow>>>,
}
impl JobStore {
    pub fn new() -> Self { Self::default() }
    pub fn insert(&self, id: String, rows: Vec<DiffRow>) {
        self.inner.lock().unwrap().insert(id, rows);
    }
    pub fn take(&self, id: &str) -> Option<Vec<DiffRow>> {
        self.inner.lock().unwrap().remove(id)
    }
}

#[derive(Default)]
pub struct JobRegistry {
    flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
}
impl JobRegistry {
    pub fn new() -> Self { Self::default() }
    pub fn register(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.flags.lock().unwrap().insert(id.to_string(), flag.clone());
        flag
    }
    pub fn cancel(&self, id: &str) -> bool {
        if let Some(flag) = self.flags.lock().unwrap().get(id) {
            flag.store(true, Ordering::Relaxed); true
        } else { false }
    }
    pub fn done(&self, id: &str) {
        self.flags.lock().unwrap().remove(id);
    }
}

// ── Apply ──────────────────────────────────────────────────────────────────

pub type ProgressSender = tokio::sync::mpsc::UnboundedSender<ApplyProgress>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApplyProgress {
    Started { job_id: String, total: usize },
    FileDone { job_id: String, file: String, ok: bool },
    Finished { job_id: String, outcome: ApplyOutcome },
}

/// Execute a diff set: writes normalized tags, concurrency capped at 4.
/// Respects the cancellation flag: files whose write has not started when
/// cancellation is observed are marked as skipped_cancelled, NOT failed.
pub async fn apply(
    job_id: String,
    rows: Vec<DiffRow>,
    cancel_flag: Arc<AtomicBool>,
    progress: Option<ProgressSender>,
) -> ApplyOutcome {
    use tokio::sync::Semaphore;
    let sem = Arc::new(Semaphore::new(4));
    let total = rows.len();
    if let Some(tx) = &progress {
        let _ = tx.send(ApplyProgress::Started { job_id: job_id.clone(), total });
    }

    let mut handles = Vec::with_capacity(total);
    for row in rows {
        let sem = sem.clone();
        let cancel = cancel_flag.clone();
        let progress = progress.clone();
        let job_id_c = job_id.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            if cancel.load(Ordering::Relaxed) {
                return FileResult::Cancelled(row.file);
            }
            let file_c = row.file.clone();
            let nd = row.normalized.clone();
            let write_res = tokio::task::spawn_blocking(move || {
                tag_writer::write_normalized(&row.file, &nd)
            }).await.unwrap();

            let ok = write_res.is_ok();
            if let Some(tx) = progress {
                let _ = tx.send(ApplyProgress::FileDone {
                    job_id: job_id_c,
                    file: file_c.to_string_lossy().to_string(),
                    ok,
                });
            }
            match write_res {
                Ok(_) => FileResult::Ok(file_c),
                Err(_) => FileResult::Failed(file_c),
            }
        }));
    }

    let mut outcome = ApplyOutcome { succeeded: 0, failed: Vec::new(), skipped_cancelled: 0 };
    for h in handles {
        match h.await.unwrap() {
            FileResult::Ok(_) => outcome.succeeded += 1,
            FileResult::Failed(p) => outcome.failed.push(p),
            FileResult::Cancelled(_) => outcome.skipped_cancelled += 1,
        }
    }
    if let Some(tx) = progress {
        let _ = tx.send(ApplyProgress::Finished { job_id, outcome: outcome.clone() });
    }
    outcome
}

enum FileResult { Ok(PathBuf), Failed(PathBuf), Cancelled(PathBuf) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_ancestor_single_file() {
        let f = PathBuf::from("/music/a/b.mp3");
        assert_eq!(common_ancestor(&[f]).unwrap(), PathBuf::from("/music/a"));
    }
    #[test]
    fn common_ancestor_same_dir() {
        let a = PathBuf::from("/music/rock/a.mp3");
        let b = PathBuf::from("/music/rock/b.mp3");
        assert_eq!(common_ancestor(&[a, b]).unwrap(), PathBuf::from("/music/rock"));
    }
    #[test]
    fn common_ancestor_divergent() {
        let a = PathBuf::from("/music/rock/a.mp3");
        let b = PathBuf::from("/music/pop/b.mp3");
        assert_eq!(common_ancestor(&[a, b]).unwrap(), PathBuf::from("/music"));
    }
    #[test]
    fn build_diff_skips_noop() {
        let ex = normalize::exceptions::ExceptionList::default();
        let cfg = NormalizationConfig { enabled: true, use_lookup: false, exceptions: &ex };
        let files = vec![(
            PathBuf::from("a.mp3"),
            RawTags { artist: "Pink Floyd".into(), album: "The Wall".into(), ..Default::default() },
        )];
        let lookups = HashMap::new();
        assert!(build_diff(files, &cfg, &lookups).is_empty());
    }
    #[test]
    fn build_diff_keeps_change() {
        let ex = normalize::exceptions::ExceptionList::default();
        let cfg = NormalizationConfig { enabled: true, use_lookup: false, exceptions: &ex };
        let files = vec![(
            PathBuf::from("a.mp3"),
            RawTags { artist: "pink floyd".into(), ..Default::default() },
        )];
        let lookups = HashMap::new();
        let diff = build_diff(files, &cfg, &lookups);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].normalized.artist, "Pink Floyd");
    }
}
```

Append to `runtime/src/mediacache/mod.rs`:

```rust
pub mod tag_write_job;
```

- [ ] **Step 2: Run tests**

Run: `cd runtime && cargo test -p stui-runtime --lib mediacache::tag_write_job`
Expected: 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/mediacache/tag_write_job.rs \
        runtime/src/mediacache/mod.rs
git commit -m "feat(mediacache): tag-write job — diff builder, apply, cancel, progress"
```

---

## Chunk 7: IPC wire schema

### Task 7.1: Request/response/event variants

**Files:**
- Modify: `runtime/src/ipc/v1/mod.rs`

- [ ] **Step 1: Add requests**

In `runtime/src/ipc/v1/mod.rs`, in the `pub enum Request { ... }` block, add at the end (before the closing brace):

```rust
    // ── Tag normalization ────────────────────────────────────────────────────
    /// Mark a raw tag value as an exception (protected from normalization).
    MarkTagException(MarkTagExceptionRequest),
    /// Compute the normalize-vs-raw diff for a scope, without writing.
    ActionATagsPreview(ActionATagsPreviewRequest),
    /// Apply a pre-computed Action A write set.
    ActionATagsApply(ActionATagsApplyRequest),
    /// Cancel an in-progress Action A run by job ID.
    ActionATagsCancel(ActionATagsCancelRequest),
```

Alongside the existing request structs, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkTagExceptionRequest {
    pub field: String,     // "artist" | "album_artist" | "album" | "title" | "genre"
    pub raw_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsPreviewRequest {
    pub scope: TagWriteScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TagWriteScope {
    Album { artist: String, album: String, date: String },
    Artist { artist: String },
    Library,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsApplyRequest {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsCancelRequest {
    pub job_id: String,
}
```

- [ ] **Step 2: Add responses**

In the `pub enum Response { ... }` block:

```rust
    MarkTagException(MarkTagExceptionResponse),
    ActionATagsPreview(ActionATagsPreviewResponse),
    ActionATagsApply(ActionATagsApplyResponse),
    ActionATagsCancel(ActionATagsCancelResponse),
```

Add response structs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkTagExceptionResponse {
    pub id: String,
    pub added: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagDiffRowWire {
    pub file: String,
    pub field: String,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsPreviewResponse {
    pub id: String,
    pub job_id: String,
    pub rows: Vec<TagDiffRowWire>,
    pub total_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsApplyResponse {
    pub id: String,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_cancelled: usize,
    pub failures: Vec<String>,
    pub rescan_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionATagsCancelResponse {
    pub id: String,
    pub cancelled: bool,
}
```

- [ ] **Step 3: Add event variants (streaming progress)**

Find the `pub enum Event { ... }` block (search file). If there isn't one, check how streaming results are emitted today (e.g., the `ipc_batcher` or a `pub enum Message`). Add a variant:

```rust
    TagApplyProgress(TagApplyProgressEvent),
```

Plus the payload:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TagApplyProgressEvent {
    Started { job_id: String, total: usize },
    FileDone { job_id: String, file: String, ok: bool },
    Finished {
        job_id: String,
        succeeded: usize,
        failed: usize,
        skipped_cancelled: usize,
    },
}
```

If there's no existing `Event` enum, add the variant to whatever broadcast mechanism exists (grep for `broadcast::`/`Event::` or similar in `runtime/src/ipc/`). If none, document here that the TUI will poll `ActionATagsApply` status via a dedicated `TagJobStatus` request — a request-only fallback is acceptable; the preview screen can also just show a spinner and block on the single `Apply` response.

- [ ] **Step 4: Build**

Run: `cd runtime && cargo build -p stui-runtime`

- [ ] **Step 5: Commit**

```bash
git add runtime/src/ipc/v1/mod.rs
git commit -m "feat(ipc): tag-normalization request/response + progress event variants"
```

---

## Chunk 8: IPC handlers

### Task 8.1: Thread `JobStore` + `JobRegistry` into dispatcher state

**Files:**
- Modify: wherever the IPC dispatcher lives (grep: `Request::MpdList(` in `runtime/src/`)

- [ ] **Step 1: Find dispatch site**

Run: `grep -rn "Request::MpdList(" runtime/src/` — note the file (likely `main.rs` or an `ipc/server.rs`).

- [ ] **Step 2: Add state fields**

In the struct that holds the dispatcher's mutable state (usually a state struct the dispatcher function closes over), add:

```rust
    pub tag_job_store: Arc<crate::mediacache::tag_write_job::JobStore>,
    pub tag_job_registry: Arc<crate::mediacache::tag_write_job::JobRegistry>,
    /// Optional progress sender for `TagApplyProgress` events. When `None`
    /// (no event broadcast infrastructure), the apply handler just returns
    /// the final summary in `ActionATagsApplyResponse` without streaming.
    pub progress_tx: Option<crate::mediacache::tag_write_job::ProgressSender>,
```

Initialize at construction: `tag_job_store`/`tag_job_registry` as `Arc::new(Default::default())`. For `progress_tx`: if the runtime already has an event broadcast channel, wire an adapter that maps `ApplyProgress` → `Event::TagApplyProgress` and put the sender half here. If not, use `None`.

- [ ] **Step 3: Build + commit**

Run: `cd runtime && cargo build -p stui-runtime`

```bash
git add runtime/src/main.rs  # or wherever state lives
git commit -m "feat(ipc): thread tag-job state through dispatcher"
```

---

### Task 8.2: `MarkTagException` handler

**Files:**
- Modify: IPC dispatcher file

- [ ] **Step 1: Add handler branch**

Inside the big `match request { ... }`:

```rust
Request::MarkTagException(r) => {
    let field = match crate::mediacache::normalize::exceptions::ExceptionField::from_str(&r.field) {
        Some(f) => f,
        None => return Ok(make_error_response(req_id, format!("unknown field: {}", r.field))),
    };
    let store = crate::mediacache::normalize::store::global()
        .ok_or_else(|| anyhow::anyhow!("exception store not initialized"))?;
    let added = store.add_user_exception(field, &r.raw_value)?;
    Response::MarkTagException(MarkTagExceptionResponse { id: req_id.clone(), added })
}
```

(`make_error_response` / `req_id` names match the existing dispatcher's conventions — adapt.)

- [ ] **Step 2: Integration test**

Create `runtime/tests/tag_exception_roundtrip.rs`:

```rust
use std::fs;
use std::path::PathBuf;

#[test]
fn store_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.toml");
    let user = dir.path().join("user.toml");
    fs::write(&bundled, "[artist]\nvalues = []\n").unwrap();

    let store = stui_runtime::mediacache::normalize::exceptions::ExceptionStore::new(
        bundled, user.clone(),
    );
    use stui_runtime::mediacache::normalize::exceptions::ExceptionField;
    assert!(store.add_user_exception(ExceptionField::Artist, "deadmau5").unwrap());
    assert!(!store.add_user_exception(ExceptionField::Artist, "deadmau5").unwrap());
    assert!(store.get().is_artist_protected("DEADMAU5"));
    let content = fs::read_to_string(&user).unwrap();
    assert!(content.contains("deadmau5"));
}
```

Verify the crate name `stui_runtime` matches the lib name in `Cargo.toml` (earlier grep showed `name = "stui-runtime"` with lib name `stui_runtime`). If the library isn't configured with a `lib` target that exposes these modules, check `runtime/src/lib.rs` — add `pub mod mediacache;` if needed.

Run: `cd runtime && cargo test -p stui-runtime --test tag_exception_roundtrip`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/main.rs runtime/tests/tag_exception_roundtrip.rs
git commit -m "feat(ipc): MarkTagException handler"
```

---

### Task 8.3: `gather_scope_files` helper + `ActionATagsPreview` handler

**Files:**
- Modify: `runtime/src/mpd_bridge/bridge.rs`
- Modify: IPC dispatcher

- [ ] **Step 1: Add `gather_scope_files` on the bridge**

In `runtime/src/mpd_bridge/bridge.rs`, add:

```rust
use crate::ipc::v1::TagWriteScope;
use std::path::PathBuf;

impl MpdBridge {
    /// Gather (absolute_path, RawTags) for every file in a tag-write scope.
    /// Uses MPD's `find` command under the hood; relies on `music_dir` from config
    /// to convert MPD-relative paths into absolute filesystem paths.
    pub async fn gather_scope_files(
        &self,
        scope: &TagWriteScope,
        music_dir: &std::path::Path,
    ) -> Result<Vec<(PathBuf, RawTags)>> {
        let raw_lines = match scope {
            TagWriteScope::Album { artist, album, date } => {
                // Prefer album+artist+date triple to disambiguate re-releases.
                let query = if date.is_empty() {
                    format!("find \"(album == \\\"{}\\\") AND (artist == \\\"{}\\\")\"\n",
                        mpd_escape(album), mpd_escape(artist))
                } else {
                    format!("find \"(album == \\\"{}\\\") AND (artist == \\\"{}\\\") AND (date == \\\"{}\\\")\"\n",
                        mpd_escape(album), mpd_escape(artist), mpd_escape(date))
                };
                self.client.send_raw(&query).await?
            }
            TagWriteScope::Artist { artist } => {
                self.client.send_raw(&format!(
                    "find \"(artist == \\\"{}\\\")\"\n", mpd_escape(artist)
                )).await?
            }
            TagWriteScope::Library => {
                self.client.send_raw("listallinfo\n").await?
            }
        };

        let mut out = Vec::new();
        let mut cur: Option<(PathBuf, RawTags)> = None;
        for line in raw_lines.lines() {
            if let Some(rel) = line.strip_prefix("file: ") {
                if let Some(done) = cur.take() { out.push(done); }
                cur = Some((music_dir.join(rel.trim()), RawTags::default()));
                continue;
            }
            let Some((_, raw)) = cur.as_mut() else { continue };
            match line.split_once(": ") {
                Some(("Artist", v)) => raw.artist = v.to_string(),
                Some(("AlbumArtist", v)) => raw.album_artist = v.to_string(),
                Some(("Album", v)) => raw.album = v.to_string(),
                Some(("Title", v)) => raw.title = v.to_string(),
                Some(("Date", v)) => raw.date = v.to_string(),
                Some(("Genre", v)) => raw.genre = v.to_string(),
                Some(("Track", v)) => raw.track = v.to_string(),
                Some(("Disc", v)) => raw.disc = v.to_string(),
                _ => {}
            }
        }
        if let Some(done) = cur { out.push(done); }
        Ok(out)
    }
}

fn mpd_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
```

- [ ] **Step 2: Preview handler**

In the dispatcher:

```rust
Request::ActionATagsPreview(r) => {
    use crate::mediacache::{normalize::{self, store as norm_store, NormalizationConfig}, tag_write_job};
    let music_dir = config.mpd.music_dir.clone()
        .ok_or_else(|| anyhow::anyhow!("[mpd.music_dir] not configured"))?;
    let raw_files = bridge.gather_scope_files(&r.scope, &music_dir).await?;
    let exceptions = norm_store::global().map(|s| s.get()).unwrap_or_default();
    let cfg = NormalizationConfig {
        enabled: true,
        use_lookup: config.music.normalize.use_lookup,
        exceptions: &exceptions,
    };
    let lookups = std::collections::HashMap::new();   // v1: always empty.
    let diff = tag_write_job::build_diff(raw_files, &cfg, &lookups);
    let total_files = diff.len();
    let rows = tag_write_job::to_wire_rows(&diff);
    let job_id = uuid::Uuid::new_v4().to_string();
    state.tag_job_store.insert(job_id.clone(), diff);
    Response::ActionATagsPreview(ActionATagsPreviewResponse {
        id: req_id.clone(),
        job_id,
        rows,
        total_files,
    })
}
```

Verify `uuid` is in `runtime/Cargo.toml` (grep). Add if missing: `uuid = { version = "1", features = ["v4"] }`.

- [ ] **Step 3: Build**

Run: `cd runtime && cargo build -p stui-runtime`

- [ ] **Step 4: Commit**

```bash
git add runtime/src/mpd_bridge/bridge.rs \
        runtime/src/main.rs \
        runtime/Cargo.toml runtime/Cargo.lock
git commit -m "feat(ipc): ActionATagsPreview handler + gather_scope_files"
```

---

### Task 8.4: `ActionATagsApply` + cancel handlers

**Files:**
- Modify: IPC dispatcher

- [ ] **Step 1: Apply handler**

```rust
Request::ActionATagsApply(r) => {
    use crate::mediacache::tag_write_job;
    let diff = state.tag_job_store.take(&r.job_id)
        .ok_or_else(|| anyhow::anyhow!("unknown job id"))?;
    let cancel_flag = state.tag_job_registry.register(&r.job_id);
    let files_for_rescan: Vec<PathBuf> = diff.iter().map(|d| d.file.clone()).collect();
    // Progress channel: wired into the existing event broadcast if available;
    // otherwise None (TUI will poll or just wait for completion).
    let progress_tx = state.progress_tx.clone();  // Option<ProgressSender>, may be None
    let outcome = tag_write_job::apply(
        r.job_id.clone(), diff, cancel_flag, progress_tx,
    ).await;
    state.tag_job_registry.done(&r.job_id);
    let rescan_path = tag_write_job::common_ancestor(&files_for_rescan)
        .map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    if !rescan_path.is_empty() {
        if let Err(e) = bridge.update_library(Some(&rescan_path)).await {
            tracing::warn!(error = %e, "mpd rescan after tag write failed");
        }
    }
    Response::ActionATagsApply(ActionATagsApplyResponse {
        id: req_id.clone(),
        succeeded: outcome.succeeded,
        failed: outcome.failed.len(),
        skipped_cancelled: outcome.skipped_cancelled,
        failures: outcome.failed.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        rescan_path,
    })
}

Request::ActionATagsCancel(r) => {
    let cancelled = state.tag_job_registry.cancel(&r.job_id);
    Response::ActionATagsCancel(ActionATagsCancelResponse {
        id: req_id.clone(), cancelled,
    })
}
```

For `progress_tx`: the dispatcher state likely has access to the event broadcast channel used for other streaming events. Find how `Event::SomethingStreaming` events are emitted today (grep `fn emit_event`, `event_tx`, `broadcast::Sender`). Wrap an adapter that turns `ApplyProgress` into `Event::TagApplyProgress` and forwards. If no event infrastructure exists, pass `None` and the TUI will not receive streaming progress (the `ActionATagsApply` response still returns the final summary).

- [ ] **Step 2: Build + commit**

Run: `cd runtime && cargo build -p stui-runtime`

```bash
git add runtime/src/main.rs
git commit -m "feat(ipc): ActionATagsApply + Cancel handlers with optional progress streaming"
```

---

## Chunk 9: Go TUI — IPC client

### Task 9.1: Wire types + client methods

**Files:**
- Modify: `tui/internal/ipc/mpd_music.go`

- [ ] **Step 1: Add types**

Append types matching the Rust wire shapes:

```go
type MarkTagExceptionRequest struct {
    Type     string `json:"type"`
    Field    string `json:"field"`
    RawValue string `json:"raw_value"`
}

type MarkTagExceptionResponse struct {
    ID    string `json:"id"`
    Added bool   `json:"added"`
}

type TagWriteScope struct {
    Kind   string `json:"kind"`              // "album" | "artist" | "library"
    Artist string `json:"artist,omitempty"`
    Album  string `json:"album,omitempty"`
    Date   string `json:"date,omitempty"`
}

type ActionATagsPreviewRequest struct {
    Type  string        `json:"type"`
    Scope TagWriteScope `json:"scope"`
}

type TagDiffRow struct {
    File     string `json:"file"`
    Field    string `json:"field"`
    OldValue string `json:"old_value"`
    NewValue string `json:"new_value"`
}

type ActionATagsPreviewResponse struct {
    ID         string       `json:"id"`
    JobID      string       `json:"job_id"`
    Rows       []TagDiffRow `json:"rows"`
    TotalFiles int          `json:"total_files"`
}

type ActionATagsApplyRequest struct {
    Type  string `json:"type"`
    JobID string `json:"job_id"`
}

type ActionATagsApplyResponse struct {
    ID                string   `json:"id"`
    Succeeded         int      `json:"succeeded"`
    Failed            int      `json:"failed"`
    SkippedCancelled  int      `json:"skipped_cancelled"`
    Failures          []string `json:"failures"`
    RescanPath        string   `json:"rescan_path"`
}

type ActionATagsCancelRequest struct {
    Type  string `json:"type"`
    JobID string `json:"job_id"`
}

type ActionATagsCancelResponse struct {
    ID        string `json:"id"`
    Cancelled bool   `json:"cancelled"`
}
```

- [ ] **Step 2: Add client methods (match existing `sendRequest` pattern)**

```go
func (c *Client) MarkTagException(field, rawValue string) (*MarkTagExceptionResponse, error) {
    req := MarkTagExceptionRequest{Type: "mark_tag_exception", Field: field, RawValue: rawValue}
    var resp MarkTagExceptionResponse
    if err := c.sendRequest(req, &resp); err != nil { return nil, err }
    return &resp, nil
}

func (c *Client) ActionATagsPreview(scope TagWriteScope) (*ActionATagsPreviewResponse, error) {
    req := ActionATagsPreviewRequest{Type: "action_a_tags_preview", Scope: scope}
    var resp ActionATagsPreviewResponse
    if err := c.sendRequest(req, &resp); err != nil { return nil, err }
    return &resp, nil
}

func (c *Client) ActionATagsApply(jobID string) (*ActionATagsApplyResponse, error) {
    req := ActionATagsApplyRequest{Type: "action_a_tags_apply", JobID: jobID}
    var resp ActionATagsApplyResponse
    if err := c.sendRequest(req, &resp); err != nil { return nil, err }
    return &resp, nil
}

func (c *Client) ActionATagsCancel(jobID string) (*ActionATagsCancelResponse, error) {
    req := ActionATagsCancelRequest{Type: "action_a_tags_cancel", JobID: jobID}
    var resp ActionATagsCancelResponse
    if err := c.sendRequest(req, &resp); err != nil { return nil, err }
    return &resp, nil
}
```

(Adjust signatures to match the existing style in `mpd_music.go` — the actual helper may be `c.Send` or `c.request` rather than `sendRequest`.)

- [ ] **Step 3: Build**

Run: `cd tui && go build ./...`

- [ ] **Step 4: Commit**

```bash
git add tui/internal/ipc/mpd_music.go
git commit -m "feat(tui-ipc): tag normalization client methods"
```

---

## Chunk 10: Go TUI — library keybind + menu

### Task 10.1: Mark-as-exception keybind on library rows

**Files:**
- Modify: `tui/internal/ui/screens/music_library.go`

- [ ] **Step 1: Cell → field mapping**

Inspect the library screen struct (grep: `type.*Screen struct`) to find how the current row/column is tracked. The library has album rows with columns Artist / Album / Year (confirm via reading the file). Add a helper:

```go
// currentCellField returns the tag field for the focused cell, or ""
// if the cell doesn't map to a normalizable field.
func (s *MusicLibraryScreen) currentCellField() (field, rawValue string) {
    row := s.selectedAlbum()
    if row == nil { return "", "" }
    switch s.focusedColumn {
    case columnArtist:
        if row.RawArtist != "" { return "artist", row.RawArtist }
        return "artist", row.Artist
    case columnAlbum:
        if row.RawTitle != "" { return "album", row.RawTitle }
        return "album", row.Album  // confusingly the wire field is `title`, but it's the album name
    default:
        return "", ""
    }
}
```

(Column constants and `selectedAlbum()` accessor may differ — read the file and adapt. The key idea: prefer `raw_*` when present so the exception protects the original tag value, not the normalized display.)

- [ ] **Step 2: Keybind handler**

In the screen's `Update` method, inside the `tea.KeyMsg` branch:

```go
case "X", "x":
    field, raw := s.currentCellField()
    if field == "" {
        s.statusMsg = "no tag field focused"
        return s, nil
    }
    return s, func() tea.Msg {
        resp, err := s.ipc.MarkTagException(field, raw)
        if err != nil {
            return statusMsg{text: fmt.Sprintf("mark exception failed: %v", err)}
        }
        if resp.Added {
            return statusMsg{text: fmt.Sprintf("protected %q", raw)}
        }
        return statusMsg{text: fmt.Sprintf("%q already protected", raw)}
    }
```

(Adapt to the existing `statusMsg` type in the codebase.)

- [ ] **Step 3: Build + smoke test**

Run: `cd tui && go build ./...`

Manual: start stui with normalization enabled, highlight an album row, press `X` on the artist cell. Confirm:
- Status line says "protected X".
- `~/.config/stui/exceptions.toml` contains the raw value.
- Refresh the library and confirm the raw value is now shown (un-normalized).

- [ ] **Step 4: Commit**

```bash
git add tui/internal/ui/screens/music_library.go
git commit -m "feat(tui): X keybind to mark tag cell as normalization exception"
```

---

### Task 10.2: "Normalize tags on disk…" menu entry

**Files:**
- Modify: `tui/internal/ui/screens/music_library.go`

- [ ] **Step 1: Menu entry**

Find the existing right-click / action menu (recent commit `b5733bb` added it). Add a new entry: **"Normalize tags on disk…"**. Show only when `config.Music.Normalize.Enabled == true` (fetch from runtime via existing config IPC, or cache on startup).

Selecting the entry opens a sub-menu (reuse whatever submenu pattern the codebase uses, or just a three-way list-picker):
- "This album" → `TagWriteScope{Kind: "album", Artist: row.Artist, Album: row.Title, Date: row.Date}`
- "This artist" → `TagWriteScope{Kind: "artist", Artist: row.Artist}`
- "Whole library" → `TagWriteScope{Kind: "library"}`

Each choice dispatches `ipc.ActionATagsPreview(scope)`. On response, push the `TagNormalizePreviewScreen` (Task 11.1) with the returned `JobID` and `Rows`.

- [ ] **Step 2: Build**

Run: `cd tui && go build ./...`

- [ ] **Step 3: Commit**

```bash
git add tui/internal/ui/screens/music_library.go
git commit -m "feat(tui): 'Normalize tags on disk…' action menu entry + scope sub-menu"
```

---

## Chunk 11: Go TUI — preview diff screen

### Task 11.1: Preview screen

**Files:**
- Create: `tui/internal/ui/screens/tag_normalize_preview.go`

- [ ] **Step 1: Screen implementation**

Create `tui/internal/ui/screens/tag_normalize_preview.go`:

```go
package screens

import (
    "fmt"
    "strings"

    tea "github.com/charmbracelet/bubbletea"
    "github.com/charmbracelet/lipgloss"

    "<module-path>/tui/internal/ipc"
)

var (
    dimStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("240"))
    errorStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("9"))
    headerStyle = lipgloss.NewStyle().Bold(true)
)

type TagNormalizePreviewScreen struct {
    jobID    string
    rows     []ipc.TagDiffRow
    excluded map[int]bool
    cursor   int
    status   string
    ipc      *ipc.Client
    height   int
    width    int
    applied  bool
    outcome  *ipc.ActionATagsApplyResponse
}

type statusMsg struct{ text string }
type popScreenMsg struct{}
type applyDoneMsg struct {
    resp *ipc.ActionATagsApplyResponse
    err  error
}

func NewTagNormalizePreview(cli *ipc.Client, jobID string, rows []ipc.TagDiffRow) *TagNormalizePreviewScreen {
    return &TagNormalizePreviewScreen{
        jobID: jobID, rows: rows, excluded: map[int]bool{}, ipc: cli,
    }
}

func (s *TagNormalizePreviewScreen) Init() tea.Cmd { return nil }

func (s *TagNormalizePreviewScreen) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
    switch m := msg.(type) {
    case tea.WindowSizeMsg:
        s.height, s.width = m.Height, m.Width
    case tea.KeyMsg:
        if s.applied { // after apply, any key closes the screen
            return s, func() tea.Msg { return popScreenMsg{} }
        }
        switch m.String() {
        case "j", "down":
            if s.cursor < len(s.rows)-1 { s.cursor++ }
        case "k", "up":
            if s.cursor > 0 { s.cursor-- }
        case "x":
            if s.cursor < len(s.rows) {
                row := s.rows[s.cursor]
                s.excluded[s.cursor] = true
                return s, func() tea.Msg {
                    if s.ipc != nil {
                        _, _ = s.ipc.MarkTagException(row.Field, row.OldValue)
                    }
                    return statusMsg{text: "excluded " + row.OldValue}
                }
            }
        case "enter", "y":
            return s, s.applyCmd()
        case "esc", "q":
            _, _ = s.ipc.ActionATagsCancel(s.jobID)
            return s, func() tea.Msg { return popScreenMsg{} }
        }
    case statusMsg:
        s.status = m.text
    case applyDoneMsg:
        s.applied = true
        if m.err != nil {
            s.status = "apply failed: " + m.err.Error()
        } else {
            s.outcome = m.resp
        }
    }
    return s, nil
}

func (s *TagNormalizePreviewScreen) applyCmd() tea.Cmd {
    return func() tea.Msg {
        resp, err := s.ipc.ActionATagsApply(s.jobID)
        return applyDoneMsg{resp: resp, err: err}
    }
}

func (s *TagNormalizePreviewScreen) View() string {
    var b strings.Builder
    b.WriteString(headerStyle.Render("Normalize Tags — Preview"))
    b.WriteString("\n\n")
    if s.applied {
        return s.renderOutcome()
    }
    for i, r := range s.rows {
        prefix := "  "
        if i == s.cursor { prefix = "> " }
        line := fmt.Sprintf("%s%s [%s]: %q → %q", prefix, r.File, r.Field, r.OldValue, r.NewValue)
        if s.excluded[i] {
            line = dimStyle.Render(line + "  (excluded)")
        }
        b.WriteString(line)
        b.WriteString("\n")
    }
    b.WriteString("\n[enter] apply   [x] exclude row   [esc] cancel\n")
    if s.status != "" {
        b.WriteString("\n" + s.status + "\n")
    }
    return b.String()
}

func (s *TagNormalizePreviewScreen) renderOutcome() string {
    var b strings.Builder
    b.WriteString(headerStyle.Render("Normalize Tags — Done"))
    b.WriteString("\n\n")
    if s.outcome == nil {
        b.WriteString(errorStyle.Render(s.status))
        return b.String()
    }
    o := s.outcome
    fmt.Fprintf(&b, "wrote %d  ·  failed %d  ·  skipped %d\n", o.Succeeded, o.Failed, o.SkippedCancelled)
    if o.RescanPath != "" {
        fmt.Fprintf(&b, "mpd rescan: %s\n", o.RescanPath)
    }
    if len(o.Failures) > 0 {
        b.WriteString("\n" + errorStyle.Render("Failures:") + "\n")
        for _, f := range o.Failures {
            b.WriteString("  " + f + "\n")
        }
    }
    b.WriteString("\n[any key] close\n")
    return b.String()
}
```

Replace `<module-path>` with the actual Go module path (grep `go.mod` for `module`).

- [ ] **Step 2: teatest**

Create `tui/internal/ui/screens/tag_normalize_preview_test.go`:

```go
package screens

import (
    "io"
    "strings"
    "testing"
    "time"

    tea "github.com/charmbracelet/bubbletea"
    "github.com/charmbracelet/x/exp/teatest"

    "<module-path>/tui/internal/ipc"
)

func readAll(t *testing.T, r io.Reader) string {
    t.Helper()
    b, err := io.ReadAll(r)
    if err != nil { t.Fatalf("read: %v", err) }
    return string(b)
}

func TestPreviewScreenExcludeRow(t *testing.T) {
    rows := []ipc.TagDiffRow{
        {File: "a.mp3", Field: "artist", OldValue: "pink floyd", NewValue: "Pink Floyd"},
        {File: "b.mp3", Field: "artist", OldValue: "deadmau5",   NewValue: "Deadmau5"},
    }
    m := NewTagNormalizePreview(nil, "job-1", rows)
    tm := teatest.NewTestModel(t, m, teatest.WithInitialTermSize(120, 30))
    defer tm.Quit()
    tm.Send(tea.KeyMsg{Type: tea.KeyDown})
    tm.Send(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'x'}})
    // Give the command goroutine a moment to produce the statusMsg.
    time.Sleep(50 * time.Millisecond)
    tm.Send(tea.KeyMsg{Type: tea.KeyEsc})
    body := readAll(t, tm.FinalOutput(t))
    if !strings.Contains(body, "excluded") {
        t.Fatalf("expected excluded marker in output; got:\n%s", body)
    }
}
```

Note: because `NewTagNormalizePreview(nil, ...)` passes a nil IPC client, the `x` handler's `s.ipc.MarkTagException(...)` will panic. Fix: guard in the handler — `if s.ipc != nil { ... }`. Update the production code above to reflect this. (An alternative is to build a fake IPC client; the guard is simpler.)

Add guard to the screen (`Update`, under `case "x":`):

```go
if s.ipc != nil { _, _ = s.ipc.MarkTagException(row.Field, row.OldValue) }
```

- [ ] **Step 3: Run test**

Run: `cd tui && go test ./internal/ui/screens/ -run TestPreviewScreenExcludeRow -v`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tui/internal/ui/screens/tag_normalize_preview.go \
        tui/internal/ui/screens/tag_normalize_preview_test.go
git commit -m "feat(tui): tag-normalization preview screen + teatest"
```

---

## Chunk 12: Documentation

### Task 12.1: README + sample config

**Files:**
- Modify: `README.md`
- Modify: `config/stui.toml`

- [ ] **Step 1: README section**

Under the "Music library" (or "Features") heading, add:

```markdown
### Music tag normalization (opt-in)

STUI can normalize messy tag metadata (year, artist, album, title, genre) for display in the library, and — optionally — write normalized values back to your files.

Enable virtual normalization in `~/.config/stui/stui.toml`:

    [music.normalize]
    enabled = true
    use_lookup = true   # v2 only: when lookup plugin ships, fills missing fields from ListenBrainz

In the library view:

- Press `X` on any artist/album cell to protect its raw value from future normalization. This appends to `~/.config/stui/exceptions.toml`.
- Open the action menu and choose **Normalize tags on disk…** to rewrite tags inside the audio files themselves. You'll see a preview diff; nothing is written until you press Enter to confirm.

Exception list layering (lowest to highest priority):
  1. **Bundled** — `config/exceptions.toml` in the STUI repo. Community-maintained; PRs welcome.
  2. **User** — `~/.config/stui/exceptions.toml`. Your manual edits and auto-learned entries.

Tag writes create a sidecar backup next to each file: `<file>.stui-tag-backup.json`. If the audio directory is read-only, backups go to `~/.local/share/stui/tag-backups/`.
```

- [ ] **Step 2: Sample config**

Add to `config/stui.toml` (sample/default):

```toml
[music.normalize]
enabled = false
use_lookup = true
```

- [ ] **Step 3: Commit**

```bash
git add README.md config/stui.toml
git commit -m "docs(music): tag normalization user guide"
```

---

## Verification checklist (run before declaring done)

- [ ] `cd runtime && cargo test -p stui-runtime` — all runtime tests pass
- [ ] `cd tui && go test ./...` — all TUI tests pass
- [ ] Manual smoke with `enabled = false`: library looks identical to pre-change behavior
- [ ] Manual smoke with `enabled = true`: messy test library displays cleanly; AC/DC and deadmau5 are untouched via bundled exceptions
- [ ] Mark-as-exception (`X`) adds to user TOML and re-renders the cell with raw value after reload
- [ ] Action A preview: diff view opens, `x` excludes a row, `enter` writes files, sidecar backups appear next to files
- [ ] Re-run Action A on same files: backups are NOT overwritten; tags reflect latest normalization rules
- [ ] MPD rescan triggers after write (scoped to common ancestor); library refreshes with new values
- [ ] With read-only audio directory: backups go to `~/.local/share/stui/tag-backups/`
- [ ] Large-library smoke: Action A on 1000+ tracks completes, cancel works mid-run (in-flight completes, rest skipped)

---

## Notes for the implementing engineer

- **DRY, YAGNI, TDD.** Every task is test-first. Don't skip the failing-test step.
- **One commit per task.** Don't batch.
- **Follow existing patterns.** The IPC dispatcher, config loader, and TUI screen conventions are already established — match them.
- **If a prerequisite differs from the plan** (e.g., a struct is named differently), grep first, adapt the task, proceed. Don't restructure beyond what a task requires.
- **Lookup is deferred to v2.** `use_lookup` config flag and the `LookupResult` type both exist but lookups are always empty in v1. When a future plugin exposes `lookup_recording`, implement `normalize::lookup::fetch_batch` — that's the only plumbing change needed.
- **When in doubt, read the spec.** `docs/superpowers/specs/2026-04-15-music-tag-normalization-design.md` is the source of truth for behavior.
