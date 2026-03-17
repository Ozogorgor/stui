# Stream Quality Quick Keys — Design Spec

**Date:** 2026-03-17
**Status:** Approved

---

## Overview

Add number keys `1`–`4` to jump directly to a quality tier when selecting a stream. Works in two contexts: from the detail overlay (triggers resolve then auto-picks) and from the stream picker screen (filters already-resolved streams). If no stream exists at the requested tier a toast is shown and nothing else happens.

| Key | Quality tier |
|-----|-------------|
| `1` | 480p (SD) |
| `2` | 720p (HD) |
| `3` | 1080p (FHD) |
| `4` | 4K (UHD) |

---

## Key Handling Strategy

`1`–`4` are already bound globally to `ActionTab1`–`ActionTab4` (tab switching). To avoid collision, the quality shortcuts are **not** added to the action system. Instead they are handled as raw key string checks inside the specific context handlers (detail overlay and stream picker), evaluated **before** the global action dispatch. They are active only when those contexts are focused.

This means the quality shortcuts are not rebindable via the keybinds config. The tab-switch bindings remain unaffected everywhere else.

---

## Model State

One new field on `Model` in `internal/ui/ui.go`:

```go
pendingQuality int // 0 = none; 2=480p, 4=720p, 5=1080p, 7=4K (qualityRank values)
```

The value stored is the `qualityRank` integer for the target tier, not the key number:

| Key | Tier | qualityRank stored |
|-----|------|--------------------|
| `1` | 480p | 2 |
| `2` | 720p | 4 |
| `3` | 1080p | 5 |
| `4` | 4K | 7 |

---

## Helper: BestStreamForTier

A new exported function in `internal/ui/screens/stream_picker.go`:

```go
// BestStreamForTier returns the stream with the highest Score (ipc.StreamInfo.Score,
// the provider-reported quality score) whose quality label resolves to the given
// qualityRank value, or nil if none match.
//
// Uses qualityScore() for label→rank lookup (HasPrefix semantics — "1080p HDR"
// matches rank 5 just like "1080p"). Score means ipc.StreamInfo.Score, the
// provider-reported integer, not the policy-derived composite score.
func BestStreamForTier(streams []ipc.StreamInfo, rank int) *ipc.StreamInfo
```

Calls the existing unexported `qualityScore(stream.Quality)` function for each stream — no new map or lookup logic. `qualityScore` uses `strings.HasPrefix` so quality labels with suffixes ("1080p HDR", "720p 60fps") are matched correctly. Among matching streams, returns the one with the highest `ipc.StreamInfo.Score`. Returns `nil` if no stream's `Quality` field resolves to the requested rank (including streams with an empty `Quality` field, which `qualityScore` returns 0 for).

---

## Context 1: Detail Overlay

When `"1"`–`"4"` is pressed while the detail overlay is focused and no stream picker is open:

1. Set `m.pendingQuality` to the target `qualityRank`
2. Call `m.client.Resolve(entryID, provider)` (same call `ActionStreamSwitch` already triggers)
3. Show toast "Resolving streams…"

When `StreamsResolvedMsg` arrives:

- Only act on `pendingQuality` when `m.detail != nil && msg.EntryID == m.detail.Entry.ID && m.pendingQuality != 0` — guard `m.detail != nil` first to avoid a nil-pointer panic if the detail overlay was closed between the Resolve call and this message arriving; ignore messages for other entries (stale resolves, background radar, etc.)
- Call `BestStreamForTier(msg.Streams, m.pendingQuality)`
- If `nil` → show toast "No Xp streams available" (X = human tier label e.g. "1080p"), clear `pendingQuality`
- If found → call `m.client.SwitchStream(matched.URL)`, clear `pendingQuality`

Note: `SwitchStream` works both pre-playback and during playback — this is the same path the `StreamPickerScreen` uses (lines 381 and 453 in `stream_picker.go`).

**Edge case — resolve already in flight:** If `1`–`4` is pressed while a resolve is already running (e.g. the user also pressed `s`), `pendingQuality` is simply set. The next `StreamsResolvedMsg` will trigger the auto-pick. No duplicate resolve is sent.

**Known limitation:** If the stream picker was previously opened and closed, its resolved streams are not cached on the Model. A fresh resolve will be triggered. Caching resolved streams on the Model is out of scope.

---

## Context 2: Stream Picker Screen

When `"1"`–`"4"` is pressed inside `StreamPickerScreen` (checked before returning to parent `Update()`):

1. Call `BestStreamForTier(s.streams, rank)`
2. If `nil` → show toast "No Xp streams available", stay in picker
3. If found → call `s.client.SwitchStream(matched.URL)`, dismiss the picker (same path as manual stream selection)

---

## Files Changed

| File | Change |
|------|--------|
| `internal/ui/screens/stream_picker.go` | Export `BestStreamForTier`; handle raw `"1"`–`"4"` key presses in picker's `Update()` before action dispatch |
| `internal/ui/ui.go` | Add `pendingQuality int` to `Model`; handle raw `"1"`–`"4"` key presses in detail overlay handler before action dispatch; extend `StreamsResolvedMsg` handler for auto-pick when `pendingQuality != 0` |

---

## Files Unchanged

| File | Reason |
|------|--------|
| `internal/ui/actions/actions.go` | No new actions — quality keys are context-gated raw key checks to avoid collision with `ActionTab1`–`4` |
| `internal/ipc/ipc.go` | `Resolve()` and `SwitchStream()` already cover both paths — no new IPC messages needed |
| `internal/state/app_state.go` | No persistent state needed |
| `internal/ui/screens/settings.go` | No new settings items |

---

## Testing

Unit tests for `BestStreamForTier` in `internal/ui/screens/stream_picker_test.go`:

- Exact tier match → returns stream with highest `ipc.StreamInfo.Score`
- Multiple streams at same tier → picks highest `Score`
- No stream at the requested tier → returns nil
- Empty stream list → returns nil
- Stream with empty `Quality` field → not matched
