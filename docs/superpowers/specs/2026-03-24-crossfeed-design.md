# BS2B Crossfeed Design Spec

## Overview

Add BS2B headphone crossfeed to the STUI DSP pipeline. Crossfeed blends a
low-pass-filtered portion of each stereo channel into the opposite channel,
mimicking the natural acoustic crosstalk of speaker listening and reducing
headphone fatigue on hard-panned content.

**Algorithm:** First-order IIR (canonical BS2B / Bauer stereophonic-to-binaural)
**Parameters:** `feed_level` (0.0–0.9) and `cutoff_hz` (300–700 Hz), fully
user-configurable with three named presets
**Auto-detection:** Best-effort keyword matching on the output device name;
defaults OFF when uncertain
**TUI:** Lipgloss-bordered dialog pushed from DSP Audio settings via
`screen.TransitionCmd`

---

## Algorithm

### First-order IIR BS2B

For each interleaved stereo frame `[in_L, in_R]`, using state `z_l` / `z_r`
(previous lowpass outputs, initialised to 0.0):

```
alpha  = exp(-2π × cutoff_hz / sample_rate)
z_l    = (1 − alpha) × in_L + alpha × z_l   + 1e-25  // update state, denormal guard
z_r    = (1 − alpha) × in_R + alpha × z_r   + 1e-25
norm   = 1.0 / (1.0 + feed_level)                     // energy normalisation (exact at DC)
out_L  = norm × (in_L + feed_level × z_r)
out_R  = norm × (in_R + feed_level × z_l)
```

`norm = 1 / (1 + feed_level)` is an approximation: exact at DC, slightly
under-normalises above the cutoff frequency where the lowpass attenuates the
crossfeed term. This matches the canonical BS2B reference implementation and is
the intended behaviour.

State: `z_l`, `z_r` (f32). Coefficients (`alpha`, `norm`) are recomputed
whenever `cutoff_hz` or `sample_rate` changes. State is reset to zero on any
parameter change to prevent transients.

### Presets

| Name    | feed_level | cutoff_hz |
|---------|-----------|-----------|
| Default | 0.45      | 700       |
| Cmoy    | 0.65      | 700       |
| Jmeier  | 0.90      | 650       |

---

## Rust Backend

### `runtime/src/dsp/crossfeed.rs` (new)

```rust
pub struct CrossfeedFilter {
    feed_level:  f32,   // 0.0–0.9
    cutoff_hz:   f32,   // 300–700
    sample_rate: u32,   // initialised to 0; first process() call triggers recompute
    alpha:       f32,   // cached: exp(-2π × cutoff / sr)
    norm:        f32,   // cached: 1 / (1 + feed_level)
    z_l:         f32,   // lowpass state, left channel
    z_r:         f32,   // lowpass state, right channel
}
```

`sample_rate` is initialised to `0` in `new()`. On the first `process()` call
the `sample_rate != stored_rate` branch fires, recomputing coefficients from the
actual pipeline sample rate before processing begins.

Public API:
- `CrossfeedFilter::new(feed_level: f32, cutoff_hz: f32) -> Self`
  — `sample_rate = 0`, `z_l = z_r = 0.0`, compute initial `alpha`/`norm` at a
  nominal 44100 Hz so the struct is always valid (will recompute on first call)
- `fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32>`
  — recomputes coefficients and resets state if `sample_rate` changed;
  processes interleaved stereo pairs
- `fn set_params(&mut self, feed_level: f32, cutoff_hz: f32)` — updates params,
  recomputes coefficients, resets `z_l`/`z_r` to zero

### `runtime/src/dsp/config.rs` (modified)

Add to `DspConfig`:

```rust
/// Enable crossfeed (set manually, or overridden by auto-detect).
pub crossfeed_enabled:    bool,   // default: false
/// When true, probe_headphones() controls crossfeed_enabled at init/update.
pub crossfeed_auto:       bool,   // default: false
/// Crossfeed blend level. Clamped 0.0–0.9.
pub crossfeed_feed_level: f32,    // default: 0.45
/// Crossfeed lowpass cutoff frequency in Hz. Clamped 300.0–700.0.
pub crossfeed_cutoff_hz:  f32,    // default: 700.0
```

### `runtime/src/dsp/mod.rs` (modified)

`DspPipeline` gains:

```rust
crossfeed: Option<CrossfeedFilter>,
```

**Initialisation** (`new`): gate construction on:
- `crossfeed_auto` → `probe_headphones(&config)` result
- otherwise → `crossfeed_enabled`

**Processing** (`process`): crossfeed is the **last stage** before `out.write()`,
after EQ, convolution, and resampling. This ensures the cutoff frequency is
accurate at the final output sample rate.

**`update_config`**:
- If `crossfeed_feed_level` or `crossfeed_cutoff_hz` changed: call
  `filter.set_params(...)` on the existing `CrossfeedFilter`.
- Recreate (drop + construct) the filter when any of the following change:
  `crossfeed_auto`, `crossfeed_enabled`, `output_target`, `alsa_device`,
  `pipewire_role`. This covers all cases where `probe_headphones` might return
  a different result.

### Auto-detection: `probe_headphones`

```rust
fn probe_headphones(config: &DspConfig) -> bool {
    let haystack = match config.output_target {
        OutputTarget::Alsa      => config.alsa_device.as_deref().unwrap_or(""),
        OutputTarget::PipeWire  => &config.pipewire_role,
        _                       => return false,
    };
    let h = haystack.to_lowercase();
    h.contains("headphone") || h.contains("headset") || h.contains("earphone")
}
```

Returns `false` (default OFF) when no recognised keyword is found.

### `runtime/src/config/manager.rs` (modified)

Four new arms in `apply_dsp_key`. `as_f32` is implemented as
`as_f64(key, value)? as f32` — cast after validation, clamp after cast:

```rust
"crossfeed_enabled"    => cfg.dsp.crossfeed_enabled    = as_bool(key, value)?,
"crossfeed_auto"       => cfg.dsp.crossfeed_auto        = as_bool(key, value)?,
"crossfeed_feed_level" => cfg.dsp.crossfeed_feed_level  =
    (as_f64(key, value)? as f32).clamp(0.0_f32, 0.9_f32),
"crossfeed_cutoff_hz"  => cfg.dsp.crossfeed_cutoff_hz   =
    (as_f64(key, value)? as f32).clamp(300.0_f32, 700.0_f32),
```

No new helper function needed — inline the `as f32` cast.

---

## Go TUI

### `tui/internal/ui/screens/crossfeed_dialog.go` (new)

`CrossfeedDialogModel` implements `screen.Screen`. Rendered via
`lipgloss.Place(m.width, m.height, lipgloss.Center, lipgloss.Center, box)`
to appear as a floating dialog over a blank background.

**Dialog dimensions:** ~56 cols × 16 rows (content), centered.

**Layout:**

```
┌─────────────── Crossfeed ───────────────┐
│                                         │
│  Auto-detect   [on ]                    │
│  Enabled       [on ]  (auto: detected)  │
│                                         │
│  Feed level    0.45  ◄────────────►     │
│  Cutoff        700 Hz                   │
│                                         │
│  Presets:  [Default]  [Cmoy]  [Jmeier] │
│                                         │
│  tab next  +/- nudge  p preset  q close │
└─────────────────────────────────────────┘
```

**Fields (tab order, `field` int 0–3):**
- 0: auto-detect toggle
- 1: enabled toggle
- 2: feed_level nudge
- 3: cutoff_hz nudge

Presets are not a tab-navigable field — they are cycled exclusively via the
`p` key regardless of which field is active.

**Key bindings:**

| Key | Action |
|-----|--------|
| `tab` / `shift+tab` | Move between fields 0–3 |
| `+` / `=` | Nudge field up (feed ±0.05, cutoff ±10 Hz, toggles flip) |
| `-` / `_` | Nudge field down |
| `p` | Cycle presets (Default → Cmoy → Jmeier → Default) |
| `q` / `esc` | Commit and close |

**Commit on close:** sends four IPC messages via `sendFn`:
- `dsp.crossfeed_enabled`
- `dsp.crossfeed_auto`
- `dsp.crossfeed_feed_level`
- `dsp.crossfeed_cutoff_hz`

No per-keystroke updates (same atomicity policy as EQ editor).

**State:**

```go
type CrossfeedDialogModel struct {
    enabled   bool
    auto      bool
    feedLevel float64   // 0.0–0.9
    cutoffHz  float64   // 300–700
    field     int       // 0=auto, 1=enabled, 2=feed, 3=cutoff
    presetIdx int       // 0=Default, 1=Cmoy, 2=Jmeier; updated by 'p'
    width, height int
    sendFn    func(key string, value interface{}) tea.Cmd
}
```

Note: no `sampleRate` field — the dialog performs no DSP computation and has no
frequency response curve to render. The Rust pipeline applies the actual cutoff
at the correct output sample rate.

The `sendFn` pattern (nil-safe callback injected at construction) is established
by the EQ editor and reviewed as the standard approach for settings dialogs.

### `tui/internal/ui/screens/settings.go` (modified)

Add to DSP Audio category (after Conv bypass):

```go
{
    label:       "Crossfeed",
    key:         "dsp.crossfeed_enabled",
    kind:        settingAction,
    description: "BS2B headphone crossfeed — blend L/R for natural stereo image",
},
```

Add to `settingAction` switch (before `default:`, following the EQ editor
pattern of constructing the screen directly in `settings.go`):

```go
case "dsp.crossfeed_enabled":
    dialog := NewCrossfeedDialogModel(func(key string, val interface{}) tea.Cmd {
        return func() tea.Msg { return SettingsChangedMsg{Key: key, Value: val} }
    })
    dialog.SetSize(m.width, m.height)
    return m, screen.TransitionCmd(dialog, true)
```

---

## Testing

### Rust (`crossfeed.rs`)

- `silence_in_silence_out` — silence in (all zeros), assert output is all zeros
- `feed_zero_is_passthrough` — feed_level=0.0 → output equals input exactly
  (norm=1.0, crossfeed term multiplied by 0.0; the `+ 1e-25` on `z_l`/`z_r`
  does not reach `out_L`/`out_R` when feed=0, so strict equality holds)
- `feed_max_energy_preserved` — feed_level=0.9, 1 kHz sine, assert
  `(RMS(out) / RMS(in) - 1.0).abs() < 0.05`; note: normalisation is exact at
  DC but approximate above cutoff, so a 5% tolerance is correct
- `lowpass_attenuates_above_cutoff` — 1 kHz sine, cutoff=300 Hz; measure
  magnitude of crossfeed contribution at output, assert it is < magnitude of
  direct path (crossfeed term must be attenuated)
- `sample_rate_change_recomputes` — call `process()` at 44100 Hz then 96000 Hz
  in sequence; assert no panic and state is reset (output first frame after
  rate change is close to input × norm, not contaminated by old state)
- `denormal_guard` — process 10 000 frames of input=1e-38 (near-zero), assert
  `z_l.is_normal() && z_r.is_normal()` after processing
- `probe_headphones_alsa_keywords` — test all three keywords
  ("hw:Headphone", "hw:Headset,0", "hw:earphone"), verify `true`; test
  "hw:Generic" → `false`; test case-insensitivity ("hw:HEADPHONE" → `true`)
- `probe_headphones_non_alsa_returns_false` — PipeWire role "Music" → `false`;
  RoonRaat → `false`

### Rust (`manager.rs`)

- `dsp_crossfeed_keys` — verify:
  - `crossfeed_enabled = true` parses correctly
  - `crossfeed_feed_level = 0.5` parses correctly
  - `crossfeed_feed_level = -0.1` clamps to `0.0`
  - `crossfeed_feed_level = 1.5` clamps to `0.9`
  - `crossfeed_cutoff_hz = 250.0` clamps to `300.0`
  - `crossfeed_cutoff_hz = 800.0` clamps to `700.0`

### Go (`crossfeed_dialog_test.go`)

- `TestCrossfeedDialogView_ContainsFields` — `View().Content` contains
  "Crossfeed", "Feed", "Cutoff"
- `TestCrossfeedDialogView_Golden` — golden file at
  `testdata/crossfeed_dialog_golden.txt`
- `TestSettingsHasCrossfeedEntry` — settings view contains "Crossfeed"

---

## File Structure

| File | Action |
|------|--------|
| `runtime/src/dsp/crossfeed.rs` | Create |
| `runtime/src/dsp/config.rs` | Modify |
| `runtime/src/dsp/mod.rs` | Modify |
| `runtime/src/config/manager.rs` | Modify |
| `tui/internal/ui/screens/crossfeed_dialog.go` | Create |
| `tui/internal/ui/screens/crossfeed_dialog_test.go` | Create |
| `tui/internal/ui/screens/settings.go` | Modify |
