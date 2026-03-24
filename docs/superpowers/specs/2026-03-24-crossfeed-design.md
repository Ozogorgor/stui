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

For each interleaved stereo frame `[in_L, in_R]`:

```
alpha  = exp(-2π × cutoff_hz / sample_rate)
lp_L   = (1 − alpha) × in_L + alpha × prev_lp_L   + 1e-25  // denormal guard
lp_R   = (1 − alpha) × in_R + alpha × prev_lp_R   + 1e-25
norm   = 1.0 / (1.0 + feed_level)                           // energy normalisation
out_L  = norm × (in_L + feed_level × lp_R)
out_R  = norm × (in_R + feed_level × lp_L)
```

State: two `f32` values `z_l`, `z_r` (previous lowpass outputs). Coefficients
(`alpha`, `norm`) are recomputed whenever `cutoff_hz` or `sample_rate` changes.
State is reset to zero on any parameter change to prevent transients.

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
    sample_rate: u32,
    alpha:       f32,   // cached: exp(-2π × cutoff / sr)
    norm:        f32,   // cached: 1 / (1 + feed_level)
    z_l:         f32,   // lowpass state, left channel
    z_r:         f32,   // lowpass state, right channel
}
```

Public API:
- `CrossfeedFilter::new(feed_level: f32, cutoff_hz: f32) -> Self`
- `fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32>`
  — recomputes coefficients if `sample_rate` changed; processes interleaved stereo
- `fn set_params(&mut self, feed_level: f32, cutoff_hz: f32)` — recomputes + resets state
- `fn is_enabled(&self) -> bool` — always true (caller holds the `Option`)

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

**Initialisation** (`new`): if `crossfeed_auto`, call `probe_headphones(&config)`
and use its result to gate construction; otherwise gate on `crossfeed_enabled`.

**Processing** (`process`): crossfeed is the **last stage** before `out.write()`,
after EQ, convolution, and resampling. This ensures the cutoff frequency is
accurate at the final output sample rate.

**`update_config`**: if params changed, call `set_params()` on the existing
filter (state reset is handled internally). Recreate if `crossfeed_auto`
changed or headphone probe result changed.

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

Returns `false` (default OFF) when the device name contains no recognisable
headphone keyword.

### `runtime/src/config/manager.rs` (modified)

Four new arms in `apply_dsp_key`:

```rust
"crossfeed_enabled"    => cfg.dsp.crossfeed_enabled    = as_bool(key, value)?,
"crossfeed_auto"       => cfg.dsp.crossfeed_auto        = as_bool(key, value)?,
"crossfeed_feed_level" => cfg.dsp.crossfeed_feed_level  =
    as_f32(key, value)?.clamp(0.0, 0.9),
"crossfeed_cutoff_hz"  => cfg.dsp.crossfeed_cutoff_hz   =
    as_f32(key, value)?.clamp(300.0, 700.0),
```

(`as_f32` is a new type-coercion helper mirroring `as_f64`.)

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

**Fields (tab order):** auto-detect toggle → enabled toggle → feed_level →
cutoff_hz → preset selector

**Key bindings:**

| Key | Action |
|-----|--------|
| `tab` / `shift+tab` | Move between fields |
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
    enabled    bool
    auto       bool
    feedLevel  float64   // 0.0–0.9
    cutoffHz   float64   // 300–700
    field      int       // 0=auto, 1=enabled, 2=feed, 3=cutoff
    presetIdx  int       // cycles 0-2
    sampleRate float64
    width, height int
    sendFn     func(key string, value interface{}) tea.Cmd
}
```

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

Add to `settingAction` switch (before `default:`):

```go
case "dsp.crossfeed_enabled":
    dialog := NewCrossfeedDialogModel(func(key string, val interface{}) tea.Cmd {
        return func() tea.Msg { return SettingsChangedMsg{Key: key, Value: val} }
    }, 44100.0)
    dialog.SetSize(m.width, m.height)
    return m, screen.TransitionCmd(dialog, true)
```

---

## Testing

### Rust (`crossfeed.rs`)

- `flat_input_unity_gain` — stereo silence in, silence out
- `feed_zero_is_passthrough` — feed_level=0.0 → output equals input (within f32 precision)
- `feed_max_energy_preserved` — feed_level=0.9, assert RMS(out) ≈ RMS(in) (normalisation check)
- `lowpass_attenuates_above_cutoff` — 1 kHz sine with cutoff=300 Hz, crossfeed path is attenuated
- `sample_rate_change_recomputes` — change sample_rate mid-stream, verify no panic + state resets
- `denormal_guard` — process 10 000 frames of near-zero input, verify no subnormal values in state

### Rust (`manager.rs`)

- `dsp_crossfeed_keys` — verify all four keys parse and clamp correctly

### Go (`crossfeed_dialog_test.go`)

- `TestCrossfeedDialogView_ContainsFields` — view contains "Crossfeed", "Feed", "Cutoff"
- `TestCrossfeedDialogView_Golden` — golden file at `testdata/crossfeed_dialog_golden.txt`
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
