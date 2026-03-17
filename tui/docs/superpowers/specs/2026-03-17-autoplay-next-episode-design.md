# Autoplay Next Episode — Design Spec

**Date:** 2026-03-17
**Status:** Approved

---

## Overview

Make the existing binge/autoplay behaviour persistent and configurable from the Settings screen. Currently, auto-playing the next episode requires pressing `b` in the episode screen each session. Two new settings items expose this as a global default: a toggle (on/off) and a configurable countdown duration.

---

## Settings Items

Two items added to the **Playback** category in `internal/ui/screens/settings.go`, after the existing "Keep open" entry:

| Label | Key | Kind | Default |
|-------|-----|------|---------|
| Auto-play next episode | `playback.autoplay_next` | `settingBool` | `false` |
| Auto-play countdown | `playback.autoplay_countdown` | `settingInt` | `5` |

The countdown item is always editable regardless of the toggle state — no conditional visibility. Valid range for countdown: 3–30 seconds.

`settingItem` currently has no `min`/`max` fields. Add them:

```go
type settingItem struct {
    // … existing fields …
    minVal int  // lower bound for settingInt (0 = no bound)
    maxVal int  // upper bound for settingInt (0 = no bound)
}
```

Update the `adjust()` method to clamp after increment/decrement:

```go
// after intVal += delta:
if s.maxVal > 0 && s.intVal > s.maxVal {
    s.intVal = s.maxVal
}
if s.minVal > 0 && s.intVal < s.minVal {
    s.intVal = s.minVal
}
```

The countdown item is declared with `minVal: 3, maxVal: 30`. All existing `settingInt` items without `minVal`/`maxVal` are unaffected (zero values = no clamping).

---

## State

Add two fields to `state.Settings` in `internal/state/app_state.go`:

```go
AutoplayNext      bool
AutoplayCountdown int  // seconds; 0 treated as 5 in countdown logic
```

`AutoplayNext` defaults to `false` via Go zero value — correct. `AutoplayCountdown` defaults to `0`; any value `<= 0` is treated as `5` in the countdown logic, matching the current hardcoded default.

---

## Settings Change Handler

In the `SettingsChangedMsg` handler in `internal/ui/ui.go`, add two new cases alongside the existing `skipper.*` mirrors:

```go
case "playback.autoplay_next":
    if v, ok := msg.Value.(bool); ok {
        m.state.Settings.AutoplayNext = v
    }
case "playback.autoplay_countdown":
    if v, ok := msg.Value.(int); ok {
        m.state.Settings.AutoplayCountdown = v
    }
```

---

## Countdown Duration

Replace the hardcoded `m.bingeCountdown = 5` at the end-of-file handler in `ui.go`:

```go
countdown := m.state.Settings.AutoplayCountdown
if countdown <= 0 {
    countdown = 5
}
m.bingeCountdown = countdown
```

---

## Episode Screen Integration

The `EpisodeScreen` constructor (or equivalent open/initialise function) gains an `autoplayDefault bool` parameter. `ui.go` passes `m.state.Settings.AutoplayNext` when creating the screen.

`bingeEnabled` on `EpisodeScreen` is initialised from `autoplayDefault` rather than always `false`. The `b` key toggle remains unchanged — it overrides the default for the current session.

The binge overlay, countdown UI, `viewBingeOverlay()`, and `playBingeNext()` are **unchanged**.

---

## Files Changed

| File | Change |
|------|--------|
| `internal/state/app_state.go` | Add `AutoplayNext bool`, `AutoplayCountdown int` to `Settings` struct |
| `internal/ui/screens/settings.go` | Add `minVal`/`maxVal` to `settingItem`; clamp in `adjust()`; add 2 items to Playback category |
| `internal/ui/screens/episode.go` | Add `autoplayDefault bool` param to constructor/open; initialise `bingeEnabled` from it |
| `internal/ui/ui.go` | 2 new `SettingsChangedMsg` cases; pass `AutoplayNext` when opening EpisodeScreen; replace hardcoded countdown `5` |

---

## Files Unchanged

| File | Reason |
|------|--------|
| `internal/ipc/ipc.go` | No new IPC messages needed |
| `internal/ui/ui.go` binge overlay | `viewBingeOverlay()` unchanged |
| `internal/ui/ui.go` playBingeNext | `playBingeNext()` unchanged |
| `internal/ui/screens/episode.go` `b` key | Per-session override unchanged |

---

## Edge Cases

- **Countdown `<= 0`:** Treated as 5 seconds — safe fallback if setting is never explicitly set.
- **Countdown out of range:** `minVal: 3, maxVal: 30` on the countdown `settingItem` clamps user input in `adjust()`; no additional validation needed in `ui.go`.
- **Settings not yet received from runtime:** `AutoplayNext` is `false` by default, so autoplay is opt-in — safe at startup.
- **Episode screen opened before settings loaded:** Same as above — defaults to off.
- **`b` key still works:** Per-session override is preserved; users can disable autoplay for a single viewing session without touching settings.
