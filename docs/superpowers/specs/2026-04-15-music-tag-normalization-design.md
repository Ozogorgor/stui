# Music Tag Normalization — Design Spec

## Goal

Give users a clean, consistent music library view by normalizing tag metadata (year, artist, album, title, genre, track/disc) and, optionally, writing the normalized values back to audio files on disk.

## Background

STUI's music library is backed by MPD. MPD exposes tags read from the audio files but **cannot write tags** — tag mutation must happen directly on the files (via a Rust tag library such as `lofty-rs`) followed by an MPD `update` rescan.

Tag metadata in user libraries is inconsistent in practice:

- Dates come in many shapes (`2017`, `2017-05-03`, `May 2017`, `03-05-2017`, and junk). A `fn extract_year` already exists at `runtime/src/mpd_bridge/bridge.rs:58` that scans for the first 4-digit run starting with `1` or `2`. It handles the above cases correctly and we reuse it verbatim.
- Artist/album/title strings have inconsistent capitalization, stray whitespace, and casing oddities.
- Lookup sources (ListenBrainz/MusicBrainz via the `listenbrainz-provider` plugin) return *canonically correct* values when a match is found but also sometimes return full dates where a year is wanted and inconsistent casing.

The feature is **opt-in at two levels**:

- **Action B — virtual normalization.** Default: off. When on, STUI shows normalized values in the library UI without touching files.
- **Action A — write normalized tags to files.** Default: off. User-triggered per-scope. Only available when B is enabled.

## Non-Goals

- **Not building a tag editor.** STUI will never provide per-field tag-editing UI. Dedicated tools (Picard, Kid3, beets) exist for that. The only tag mutation STUI performs is bulk normalization via Action A.
- **Not inferring metadata from filenames.** Normalization operates on existing tags (plus optional lookup enrichment). Filename parsing is out of scope.
- **Not syncing changes back to MusicBrainz.** Lookup is read-only.

## Design

### Architecture overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Normalization Pipeline                    │
│                                                              │
│  MPD tag data ──► Lookup (MB/LB, if enabled, fills gaps)    │
│                        │                                     │
│                        ▼                                     │
│                   Exception list filter                      │
│                   (skip protected values)                    │
│                        │                                     │
│                        ▼                                     │
│                   Algorithmic rules                          │
│                   (extract_year, smart title case, trim)     │
│                        │                                     │
│                        ▼                                     │
│                   Normalized record                          │
└─────────────────────────────────────────────────────────────┘
             │                              │
             ▼                              ▼
      Action B: display            Action A: write to file
      (catalog cache)              (lofty-rs + MPD rescan)
```

The pipeline is a pure function: `(raw_tags, exception_list, lookup_result) → normalized_tags`. It lives in a new module and is exercised identically by both actions. Action A wraps it with I/O (file writes, backups, rescan).

### New module: `runtime/src/mediacache/normalize.rs`

Single-purpose module containing the normalization pipeline. Pure functions, no I/O. Fully unit-testable.

```rust
pub struct NormalizationConfig {
    pub enabled: bool,                    // Action B master switch
    pub use_lookup: bool,                 // Use MB/LB plugin when available
    pub exceptions: ExceptionList,
}

pub struct ExceptionList {
    pub artist: HashSet<String>,          // case-insensitive membership
    pub album_artist: HashSet<String>,
    pub album: HashSet<String>,
    pub title: HashSet<String>,
    pub genre: HashSet<String>,
}

pub struct RawTags { /* mirrors MPD fields */ }
pub struct NormalizedTags { /* same shape, post-pipeline */ }

pub fn normalize(
    raw: &RawTags,
    cfg: &NormalizationConfig,
    lookup: Option<&LookupResult>,
) -> NormalizedTags;
```

### Field rules

| Field | Rule |
|---|---|
| Year | Reuse `extract_year` from `mpd_bridge/bridge.rs`. Promote to a shared helper in `normalize.rs`. |
| Artist | Smart title case + trim + collapse internal whitespace |
| Album Artist | Same as Artist |
| Album | Same as Artist |
| Title | Same as Artist |
| Genre | Smart title case + trim |
| Track / Disc | Strip leading zeros; parse `N/M` forms to the integer `N`. |

**Smart title case rules:**

1. Capitalize principal words.
2. Lowercase small words: `a, an, the, and, or, but, of, in, on, at, to, for, by, vs, nor, per, via`.
3. Always capitalize the first and last word.
4. **Unusual-case heuristic (kept):** if the raw string already has mixed or stylized case (e.g., `deadmau5`, `AC/DC`, `MGMT`, `iamamiwhoami`), leave it alone. Detection: string contains both upper and lower letters in a non-title-case pattern, OR is all-uppercase with length ≥ 2 and contains a non-alpha separator (`/`, `-`), OR contains digits embedded in letters.
5. Exceptions list (below) overrides the heuristic in both directions.

### Normalization source hierarchy (lookup + rules)

When the `listenbrainz-provider` plugin is enabled and a confident match is available, its values are used to **fill missing fields only** — never to overwrite populated fields. After lookup fills gaps, **algorithmic rules run on every field regardless of source**, so lookup results (which can be all-lowercase or contain full dates) still get normalized.

This resolves the "lookup returned 2017-05-03 instead of 2017" and "lookup returned all-lowercase" cases surfaced during brainstorming.

### Exception list

- **Scope per entry:** exact string value per field (`artist = "deadmau5"` protects that artist's `artist` field only, not their album or title fields).
- **Match:** case-insensitive exact match on the **pre-normalized** (raw) value. Pre-normalized match is deliberate: avoids round-tripping through normalization just to check protection status.
- **Sources (merged at load time):**
  1. Bundled exception list shipped with STUI, maintained in the repo at `config/exceptions.toml`. Community-curated, PR-accepted.
  2. User-local overrides at `~/.config/stui/exceptions.toml` (path follows existing STUI config conventions). Written by auto-learn or manually edited.
- **TOML shape:**
  ```toml
  [artist]
  values = ["deadmau5", "iamamiwhoami", "AC/DC"]

  [album]
  values = ["untitled unmastered."]

  [title]
  values = []
  ```
- **Auto-learn trigger:** a new keybind ("mark as exception") in the library UI and in the Action A preview diff. Pressing it:
  1. Reads the raw (pre-normalized) value for the selected field.
  2. If the raw value is empty or whitespace-only: no-op, surface a brief status message ("nothing to protect"). Otherwise:
  3. Appends it to the user-local `exceptions.toml` (no duplicate writes — check membership first).
  4. Invalidates the relevant cache entry so next render reflects the exception.
- **Removal:** users edit `exceptions.toml` directly. No in-app removal action — keeps the UI simple and the TOML file authoritative.

### Settings (in `config/stui.toml`)

```toml
[music.normalize]
enabled = false                # Action B master switch, default off
use_lookup = true              # only effective when enabled = true AND a lookup plugin is active
# Action A has no config — it's a user-triggered action, not a setting.
```

### Action B — virtual normalization in the catalog cache

When `music.normalize.enabled = true`:

1. `mpd_bridge` continues to fetch raw tags from MPD unchanged.
2. A new normalization pass runs over each directory/album result before it reaches the TUI.
3. Normalized values are cached in `mediacache`. Cache key includes the exception-list version hash (SHA-256 of the merged bundled+user TOML content) so edits to the exception list invalidate stale entries. The exception-list loader watches both TOML files (bundled + user) for mtime changes on next access — no background watcher thread; recompute hash lazily when MPD data is fetched.
4. The TUI displays normalized values. No raw-value toggle (YAGNI).

Zero overhead when `enabled = false` — the pass is skipped entirely, no allocations.

### Action A — write normalized tags to files

User-triggered from an action menu in the library UI. Only exposed when Action B is enabled.

**Scope menu (user chooses per invocation):**

- Currently-selected album
- Currently-selected artist (all their albums)
- Whole library

**Flow:**

1. Build the diff set by running the pipeline over the chosen scope and comparing to raw. Skip no-change rows.
2. Open a **preview diff view**: per-track rows showing `field: old → new`. Always shown; never skipped even for a single album.
3. From the diff view, user can:
   - Confirm → proceed to write.
   - Cancel → abort, no changes.
   - Mark row as exception → drops it from the write set and appends to `exceptions.toml`. Recompute diff; re-render.
4. On confirm:
   - For each file in the write set: read original tags via `lofty-rs`, write a sidecar backup containing the original tags (only if a backup for this file doesn't already exist — never overwrite a prior backup), then apply the normalized tags. Backup location preference: `<file>.stui-tag-backup.json` next to the audio file. Fallback when that directory is read-only or write fails: `~/.local/share/stui/tag-backups/<sha256-of-absolute-path>.json`. Always log which location was used.
   - Stream in batches with a progress bar. Cancellable mid-run.
   - **Cancellation semantics:** in-flight writes for files already started are allowed to complete (we never abandon a file mid-write); cancellation prevents *new* writes from starting. Files that completed before cancellation are considered normalized; files that hadn't started are untouched. Backup-before-write ordering guarantees no data loss either way.
   - Parallel file writes capped at 4.
5. After all writes complete: trigger `mpd update` automatically, scoped to the narrowest common ancestor path of all touched files (avoids whole-library rescan when only one album was normalized).

**Sidecar backup format:**

```json
{
  "created": "2026-04-15T10:30:00Z",
  "stui_version": "0.X.Y",
  "original": {
    "artist": "...",
    "album": "...",
    "title": "...",
    "year": "...",
    "genre": "..."
  }
}
```

Users can restore manually from sidecars (documented, not automated — YAGNI). The sidecar existence also serves as a "this file has been normalized" marker.

**Failure handling:**

- If a single file fails (permission error, corrupt header, etc.), log it, add to an end-of-run report, continue with the rest. Don't abort the batch.
- If the backup write fails, skip the tag write for that file. Never write without a successful backup.

## Component map

| Component | Path | Responsibility |
|---|---|---|
| Normalization pipeline | `runtime/src/mediacache/normalize.rs` | Pure `(raw, cfg, lookup) → normalized` |
| Exception list loader | `runtime/src/mediacache/normalize.rs` | Load/merge bundled + user TOML, hash for cache keying |
| MPD bridge hook | `runtime/src/mpd_bridge/bridge.rs` | Call pipeline on fetched tags when B is enabled |
| Tag writer | `runtime/src/mediacache/tag_writer.rs` | `lofty-rs` wrapping: read, backup, write, rescan |
| Lookup adapter | `runtime/src/mediacache/normalize.rs` | Thin call into existing `listenbrainz-provider` IPC; fills gaps only |
| TUI library action menu | `tui/internal/ui/screens/music_library.go` | "Normalize tags on disk…" entry + scope picker |
| TUI preview diff view | `tui/internal/ui/screens/` (new file) | Diff rendering, confirm/cancel/exception keybinds |
| Bundled exception list | `config/exceptions.toml` | Community-maintained defaults |

## Testing

- **Unit tests (Rust):** pipeline fixtures for every field rule, the unusual-case heuristic (deadmau5, AC/DC, MGMT, iamamiwhoami, normal-cased names), exception-list matching, lookup-fills-gaps-but-rules-always-run.
- **Unit tests (Rust):** tag writer against a temp directory with sample MP3/FLAC files; verify sidecar is written before tag write; verify failure in one file doesn't abort batch.
- **Integration test (Rust):** end-to-end through `mpd_bridge` with a mock MPD client returning messy tags; assert normalized output.
- **TUI test (Go, `teatest`):** library screen with normalization on/off, mark-as-exception keybind, Action A preview diff with confirm/cancel/exception paths.

## Performance

- Action B runs on every MPD response when enabled. Cost: one pipeline pass per directory listing. Cached at `mediacache` layer keyed by MPD response hash + exception-list hash + lookup-enabled flag.
- Action A is I/O bound. Batched streaming (no whole-library-in-memory diff), progress bar, cancellable, parallel file writes capped at 4.
- When B is disabled, zero overhead — pipeline never runs, no allocations.

## Open questions for future work

- **Restore from sidecar:** currently manual. A "restore original tags" action could be added later once we see if users actually want it.
- **Per-field granular exceptions** (e.g., "for artist `Foo`, never normalize *any* of Foo's fields"): deferred; exact-string-per-field is simpler and covers the common case.
- **Filename-based metadata inference:** out of scope.
