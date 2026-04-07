# Music Queue Sub-Tab Redesign — Design Spec

## Goal

Redesign the Music → Queue sub-tab to resemble rmpc's queue UI: two-column layout with a now-playing right panel (art placeholder, labeled metadata, seek bar, volume display) and the audio visualizer rendered inline at the bottom of the queue view rather than globally.

## Context

- **File:** `internal/ui/screens/music_queue.go`
- **Container:** `MusicScreen` in `music_screen.go`, rendered inside the main card
- **Existing visualizer:** `components.Visualizer` owned by `ui.go`, currently renders globally below the MPD HUD. Must be suppressed when Music→Queue is active.
- **Existing volume keys:** `+`/`-` already exist as global MPD controls in `ui.go` and send `MpdCmd("mpd_set_volume", {"volume": N})`. They are NOT re-added in the queue screen.
- **IPC types confirmed:** `MpdStatusMsg.Volume uint32` (0–100), `MpdStatusMsg.Elapsed float64`, `MpdStatusMsg.Duration float64`.

## Layout

Two modes based on terminal width:

- **`width > 80`:** Two-column layout with right panel and inline visualizer (described below).
- **`width ≤ 80`:** Fall back to the existing single-column layout unchanged. No right panel, no inline visualizer. The global visualizer suppression in `ui.go` also has a `width > 80` guard so the global viz still renders at narrow widths.

### Wide layout structure (`width > 80`)

```
<header line>                                              │ <right panel>
<column headers>                                           │
<track rows, scrollable>                                   │
                                                           │
<footer hint line>
<visualizer strip: visualizer.Config().Height rows>
```

**Column counts:**
- Right panel: fixed 22 columns
- Separator `│`: 1 column
- Left panel: `L = width - 23`

**Track list height** `TH`:
```
TH = h - 3 - vizHeight
```
where `h` is the `View(w, h int)` height argument. The `3` accounts for: 1 header line (queue count), 1 column headers row, 1 footer hint line. `vizHeight = s.visualizer.Config().Height` when `s.visualizer != nil && s.visualizer.IsRunning()`, else `0`.
If `TH < 1`, set `TH = 1`.

### Column headers row

```
   #  Title                  Artist         Album          Dur
```

Fixed columns (within left panel width `L`):
- Prefix/cursor: 3ch
- `#`: 3ch right-aligned, 1ch space
- `Duration`: 6ch right-aligned
- Fixed overhead total: `3 + 3 + 1 + 6 = 13ch`. Remaining width `R = L - 13`.

**Adaptive Album column** (shown when `L ≥ 120`, i.e., `width ≥ 143`):
- Title = `R * 40 / 100`, Artist = `R * 35 / 100`, Album = `R * 25 / 100`. Any remainder goes to Title.

**No Album column** (`L < 120`):
- Title = `R * 55 / 100`, Artist = `R * 45 / 100`. Any remainder goes to Title.

## Right Panel (22 cols fixed)

Rendered as a fixed-width column separated from the track list by a single `│`. All content is truncated to 20ch.

### When no current track

`isCurrentTrack()` is an existing method on `MusicQueueScreen`. If it matches no track (queue empty, or playing track not in loaded list), metadata value lines show `—` in dim style. Seek bar shows `────────────────────` and `0:00     0:00`. Volume bar always shows current `nowVolume`.

### Panel content top-to-bottom

**1. Art placeholder** (always rendered, no data dependency):
- 20ch wide × 9 rows, rounded border, centered `♪` glyph in dim style.

**2. Labeled metadata** (4 sections, each = 1 dim label line + 1 value line):
- `TITLE` — current track's `Title`, truncated to 20ch
- `ARTIST` — current track's `Artist`, truncated to 20ch
- `ALBUM` — current track's `Album`, truncated to 20ch
- `DURATION` — current track's `Duration` formatted as `m:ss`

**3. Progress / seek bar** (2 rows):
```
━━━━━╸──────────────    ← bar row
1:03              3:34  ← time row (elapsed left, total right, padded to 20ch)
```
- Bar width = 20ch.
- `filled = int(nowElapsed / nowDuration * 20)`, capped to 19 when `nowDuration > 0`.
- Characters: positions `0..filled-1` = `━`, position `filled` = `╸`, positions `filled+1..19` = `─`.
- When `nowDuration == 0`: all 20 chars = `─`, time row = `0:00              0:00`.
- The cursor tip character is `╸` (U+2578), not `▸`.

**4. Volume bar** (2 rows):
```
▮▮▮▮▮▮▮▯▯▯  72%        ← bar + percentage
+ vol  - vol  0 mute    ← hint row (dim style)
```
- `filled = int(nowVolume / 10)` (integer division of uint32).
- `empty = 10 - filled`.
- Bar = `filled` × `▮` + `empty` × `▯`, then two spaces, then `fmt.Sprintf("%d%%", nowVolume)`.
- When `nowVolume == 100`: filled=10, empty=0, bar = `▮▮▮▮▮▮▮▮▮▮  100%`.
- Hint line: when `nowMuted == false`: `+ vol  - vol  0 mute`; when `nowMuted == true`: `+ vol  - vol  0 unmute`. Both in dim style.

**Right panel total rows:** 9 (art box) + 8 (4 × label+value) + 2 (seek bar) + 2 (volume bar) = **21 rows**.

**Right panel overflow:** Available panel height = `h - 3 - vizHeight`. If panel content (21 rows) exceeds this, truncate from the bottom in this priority order (cut last-to-first): volume hint row, volume bar row, seek time row, seek bar row, metadata from bottom up (Duration, Album, Artist, Title). The art box (9 rows) is never cut — if available height < 9, render only the art box rows that fit.

## New Fields on `MusicQueueScreen`

```go
nowElapsed  float64  // from MpdStatusMsg.Elapsed
nowDuration float64  // from MpdStatusMsg.Duration
nowVolume   uint32   // from MpdStatusMsg.Volume (uint32, 0–100)
prevVolume  uint32   // volume saved before local mute toggle (default 100)
nowMuted    bool     // true when muted via the 0 key
visualizer  *components.Visualizer  // nil until SetVisualizer is called
```

Extend the existing `ipc.MpdStatusMsg` case in `Update()`:
```go
s.nowElapsed  = m.Elapsed
s.nowDuration = m.Duration
s.nowVolume   = m.Volume
// If an external client changed the volume away from 0 while we thought we were muted, clear mute state.
if s.nowMuted && m.Volume > 0 {
    s.nowMuted = false
}
```

## New Key Bindings (queue-only, added to `MusicQueueScreen.Update`)

| Key | Action | IPC |
|-----|--------|-----|
| `0` | Toggle mute | `MpdCmd("mpd_set_volume", {"volume": N})` |
| `<` | Seek −5s | `MpdCmd("mpd_seek", {"id": nowSongID, "time": N})` |
| `>` | Seek +5s | `MpdCmd("mpd_seek", {"id": nowSongID, "time": N})` |

**Note:** `+`/`-` for volume already exist globally in `ui.go` and are NOT re-added here.

**Mute toggle (`0`) logic:**
```
if nowMuted:
    send mpd_set_volume(prevVolume)
    nowMuted = false
else:
    prevVolume = nowVolume   // save current (even if 0)
    send mpd_set_volume(0)
    nowMuted = true
```
If `nowVolume == 0` and `nowMuted == false` when `0` is pressed (volume set to 0 externally), treat as mute: save `prevVolume = 0`, set `nowMuted = true`, send `setvol(0)` (no-op on server, restores local state for future unmute).

**Seek logic:**
- `<`: send `time = max(0, nowElapsed - 5)`.
- `>`: send `time = min(nowDuration, nowElapsed + 5)`. If `nowDuration == 0`, do nothing.
- Uses `nowSongID` (existing field) as the track identifier.
- `mpd_seek` is a new runtime command following the existing naming convention. Params: `{"id": int32, "time": float64}`.

## Visualizer Integration

**Wiring:** `MusicScreen` stores the visualizer pointer and exposes:
```go
func (s *MusicScreen) SetVisualizer(v *components.Visualizer) {
    s.queue.visualizer = v
}
```
Called from `ui.go` after music screen construction: `m.musicScreen.SetVisualizer(m.visualizer)`.

**Inline render:** At the end of `MusicQueueScreen.View()`, when `width > 80 && s.visualizer != nil && s.visualizer.IsRunning()`:
```go
viz := s.visualizer.RenderBars(width)
// append viz below footer hint
```
`RenderBars` outputs exactly `s.visualizer.Config().Height` rows. No additional height calculation needed.

**Visualizer width:** `RenderBars(width)` is called with the full terminal `width`, so the visualizer strip spans the entire width of the queue view (underneath both the track list and the right panel).

**Global suppression in `ui.go`:** Wrap the existing visualizer render block with:
```go
if !(m.state.ActiveTab == state.TabMusic &&
     m.musicScreen.ActiveSubTab() == screens.MusicQueue &&
     m.state.Width > 80) {
    // existing visualizer render
}
```
`screens.MusicQueue` is the constant from `internal/ui/screens` package. `ui.go` already imports that package (it references `screens.*` types elsewhere). At `width ≤ 80`, the inline viz does not render but the global viz does (no suppression).

## Files Changed

| File | Change |
|------|--------|
| `internal/ui/screens/music_queue.go` | New fields; extend `MpdStatusMsg` handler; new key bindings (`0`, `<`, `>`); full `View()` rewrite: two-column layout, column headers, right panel (art placeholder, metadata, seek bar, volume bar), visualizer strip |
| `internal/ui/screens/music_screen.go` | Add `SetVisualizer(v *components.Visualizer)` method |
| `internal/ui/ui.go` | Call `m.musicScreen.SetVisualizer(m.visualizer)` after music screen init; conditionally suppress global viz render |

## Out of Scope

- Actual album art image rendering (kitty/sixel) — placeholder box only
- Click-to-seek on the seek bar
- Mouse volume drag
