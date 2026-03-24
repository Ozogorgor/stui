# Dither + Noise Shaping Design Spec

## Overview

Add TPDF dither with selectable noise shaping to the STUI DSP pipeline. Dither adds
low-level triangular-PDF noise before bit-depth quantization to linearize quantization
error. Noise shaping filters the error feedback signal to push quantization noise into
less audible high-frequency bands.

**Algorithm:** TPDF dither + FIR/IIR error-feedback noise shaping
**Parameters:** `bit_depth` (8–32, default 16) and `noise_shaping` (9 selectable
algorithms), fully user-configurable
**Auto-detection:** Enabled automatically when `output_target == Alsa` and
`bit_depth == 16` (the case where dither matters most)
**TUI:** Lipgloss-bordered dialog pushed from DSP Audio settings via
`screen.TransitionCmd`, same pattern as crossfeed dialog

---

## Algorithm

### TPDF Dither + Error Feedback Noise Shaping

For each interleaved stereo frame `[in_L, in_R]`:

```
// TPDF: sum of two uniform random values in [-0.5, 0.5] LSB
tpdf      = uniform() + uniform()           // triangular PDF, range [-1, 1] LSB

// Error feedback noise shaping (if shaping != None)
shaped    = tpdf + dot(coeffs, error_buf)   // add filtered past error

// Quantize to target bit depth
scale     = 2^(bit_depth - 1)
out       = round(in * scale + shaped) / scale

// Update error buffer (circular, length = len(coeffs))
error_buf = [in - out, error_buf[0..n-2]]
```

When `noise_shaping == None`, `shaped = tpdf` and no error buffer is maintained.

TPDF generation uses a xorshift64 RNG seeded at construction. State (`error_buf`,
`rng_state`) is reset whenever `bit_depth`, `noise_shaping`, or `sample_rate` changes.

### Noise Shaping Algorithms

All coefficients sourced from SoX `src/dither.c` (public domain / LGPL).

| Name | Taps | Type | Character |
|------|------|------|-----------|
| `none` | — | — | TPDF only, flat noise spectrum |
| `lipshitz` | 5 | FIR | "Minimally audible" — classic Lipshitz reference |
| `fweighted` | 9 | FIR | F-weighted psychoacoustic (Lipshitz-Noll-Subotic) |
| `modified_e_weighted` | 9 | FIR | Gentler E-weighted variant |
| `improved_e_weighted` | 9 | FIR | Stronger E-weighted variant |
| `shibata` | 20 | FIR | Aggressive, pushes noise well above 15 kHz |
| `low_shibata` | 15 | FIR | Gentler Shibata variant |
| `high_shibata` | 20 | FIR | Most aggressive Shibata variant |
| `gesemann` | 4 | IIR | Psychoacoustically shaped, efficient |

#### Shibata per-rate selection

Shibata has coefficient tables for multiple sample rates (8k / 11k / 16k / 22k / 32k /
38k / 44.1k / 48k). On each `process()` call, the nearest available table is selected
for the current `sample_rate`. When `sample_rate` changes, `error_buf` is cleared.
Low-Shibata and High-Shibata follow the same per-rate selection logic (44.1k / 48k
variants available; fall back to 44.1k for other rates).

All other algorithms use a single coefficient table regardless of sample rate.

#### Coefficient tables

**Lipshitz (`lip44`):**
```
[2.033, -2.165, 1.959, -1.590, 0.6149]
```

**F-weighted (`fwe44`):**
```
[2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847]
```

**Modified-E-weighted (`mew44`):**
```
[1.662, -1.263, 0.4827, -0.2913, 0.1268, -0.1124, 0.03252, -0.01265, -0.03524]
```

**Improved-E-weighted (`iew44`):**
```
[2.847, -4.685, 6.214, -7.184, 6.639, -5.032, 3.263, -1.632, 0.4191]
```

**Shibata 44.1k (`shi44`, 20 taps):**
```
[2.6773197650909423828, -4.8308925628662109375,  6.570110321044921875,
-7.4572014808654785156,  6.7263274192810058594, -4.8481650352478027344,
 2.0412089824676513672,  0.7006359100341796875, -2.9537565708160400391,
 4.0800385475158691406, -4.1845216751098632812,  3.3311812877655029297,
-2.1179926395416259766,  0.879302978515625,     -0.031759146600961685181,
-0.42382788658142089844, 0.47882103919982910156,-0.35490813851356506348,
 0.17496839165687561035,-0.060908168554306030273]
```

**Shibata 48k (`shi48`, 16 taps):**
```
[2.8720729351043701172, -5.0413231849670410156,  6.2442994117736816406,
-5.8483986854553222656,  3.7067542076110839844, -1.0495119094848632812,
-1.1830236911773681641,  2.1126792430877685547, -1.9094531536102294922,
 0.99913084506988525391,-0.17090806365013122559,-0.32615602016448974609,
 0.39127644896507263184,-0.26876461505889892578, 0.097676105797290802002,
-0.023473845794796943665]
```

**Low-Shibata 44.1k (`shl44`, 15 taps):**
```
[2.0833916664123535156, -3.0418450832366943359,  3.2047898769378662109,
-2.7571926116943359375,  1.4978630542755126953, -0.3427594602108001709,
-0.71733748912811279297,  1.0737057924270629883,-1.0225815773010253906,
 0.56649994850158691406,-0.20968692004680633545,-0.065378531813621520996,
 0.10322438180446624756,-0.067442022264003753662,-0.00495197344571352005]
```

**Low-Shibata 48k (`shl48`, 16 taps):**
```
[2.3925774097442626953, -3.4350297451019287109,  3.1853709220886230469,
-1.8117271661758422852, -0.20124770700931549072,  1.4759907722473144531,
-1.7210904359817504883,  0.97746700048446655273,-0.13790138065814971924,
-0.38185903429985046387,  0.27421241998672485352,  0.066584214568138122559,
-0.35223302245140075684,  0.37672343850135803223,-0.23964276909828186035,
 0.068674825131893157959]
```

**High-Shibata 44.1k (`shh44`, 20 taps):**
```
[3.0259189605712890625, -6.0268716812133789062,  9.195003509521484375,
-11.824929237365722656,  12.767142295837402344, -11.917946815490722656,
 9.1739168167114257812,  -5.3712320327758789062,  1.1393624544143676758,
 2.4484779834747314453,  -4.9719839096069335938,  6.0392003059387207031,
-5.9359521865844726562,   4.903278350830078125,  -3.5527443885803222656,
 2.1909697055816650391,  -1.1672389507293701172,  0.4903914332389831543,
-0.16519790887832641602,  0.023217858746647834778]
```

**Gesemann 44.1k (`ges44`, 4-tap IIR):**
```
feedforward: [2.2061, -0.4706, -0.2534, -0.6214]
feedback:    [1.0587,  0.0676, -0.6054, -0.2738]
```

**Gesemann 48k (`ges48`, 4-tap IIR):**
```
feedforward: [2.2374, -0.7339, -0.1251, -0.6033]
feedback:    [0.9030,  0.0116, -0.5853, -0.2571]
```

Gesemann is an IIR filter: error feedback uses both a feedforward term (past errors)
and a feedback term (past shaped-error outputs). Maintains separate `ff_buf` and `fb_buf`
state vectors. IIR state cleared on sample rate or parameter change.

---

## Rust Backend

### `runtime/src/dsp/dither.rs` (new)

```rust
pub enum NoiseShaping {
    None,
    Lipshitz,
    Fweighted,
    ModifiedEweighted,
    ImprovedEweighted,
    Shibata,
    LowShibata,
    HighShibata,
    Gesemann,
}

pub struct DitherFilter {
    bit_depth:     u32,          // 8–32
    noise_shaping: NoiseShaping,
    sample_rate:   u32,          // last seen; triggers state reset on change
    error_buf:     Vec<f32>,     // FIR error feedback (circular, length = tap count)
    ff_buf:        Vec<f32>,     // IIR feedforward buffer (Gesemann only)
    fb_buf:        Vec<f32>,     // IIR feedback buffer (Gesemann only)
    rng_state:     u64,          // xorshift64 state
}
```

Public API:
- `DitherFilter::new(bit_depth: u32, noise_shaping: NoiseShaping) -> Self`
- `fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32>`
  — resets state if `sample_rate` changed; processes interleaved stereo pairs
- `fn set_params(&mut self, bit_depth: u32, noise_shaping: NoiseShaping)`
  — updates params, resets all state buffers

### `runtime/src/dsp/config.rs` (modified)

Add to `DspConfig`:

```rust
/// Enable dither (set manually, or overridden by auto-detect).
pub dither_enabled:       bool,    // default: false
/// When true, auto-enable dither for ALSA 16-bit output.
pub dither_auto:          bool,    // default: false
/// Output bit depth. Clamped 8–32.
pub dither_bit_depth:     u32,     // default: 16
/// Noise shaping algorithm name. One of: "none", "lipshitz", "fweighted",
/// "modified_e_weighted", "improved_e_weighted", "shibata", "low_shibata",
/// "high_shibata", "gesemann".
pub dither_noise_shaping: String,  // default: "none"
```

### `runtime/src/dsp/mod.rs` (modified)

`DspPipeline` gains:

```rust
dither: Option<DitherFilter>,
```

**Initialisation** (`new`): gate construction on:
- `dither_auto` → `output_target == Alsa && dither_bit_depth == 16`
- otherwise → `dither_enabled`

**Processing** (`process`): dither is the **last stage** before `out.write()`,
after crossfeed. This ensures the correct output sample rate is used for
coefficient selection.

**`update_config`**: Recreate (drop + construct) when any of the following change:
`dither_enabled`, `dither_auto`, `dither_bit_depth`, `dither_noise_shaping`,
`output_target`. No partial update path — always recreate on any dither-related change
(simpler than crossfeed's split set_params / recreate logic, since all fields affect
the coefficient tables or state size).

### `runtime/src/config/manager.rs` (modified)

Four new arms in `apply_dsp_key`:

```rust
"dither_enabled"       => cfg.dsp.dither_enabled       = as_bool(key, value)?,
"dither_auto"          => cfg.dsp.dither_auto           = as_bool(key, value)?,
"dither_bit_depth"     => cfg.dsp.dither_bit_depth      =
    (as_u64(key, value)? as u32).clamp(8, 32),
"dither_noise_shaping" => cfg.dsp.dither_noise_shaping  = as_string(key, value)?,
```

---

## Go TUI

### `tui/internal/ui/screens/dither_dialog.go` (new)

`DitherDialogModel` implements `screen.Screen`. Rendered via
`lipgloss.Place(m.width, m.height, lipgloss.Center, lipgloss.Center, box)`
to appear as a floating dialog over a blank background.

**Dialog dimensions:** ~56 cols × 14 rows (content), centered.

**Layout:**

```
┌──────────────── Dither ─────────────────┐
│                                         │
│  Auto-detect   [on ]                    │
│  Enabled       [on ]  (auto: active)    │
│                                         │
│  Bit depth     16                       │
│  Noise shaping Shibata                  │
│                                         │
│  tab next  +/- adjust  q close         │
└─────────────────────────────────────────┘
```

**Fields (tab order, `field` int 0–3):**
- 0: auto-detect toggle
- 1: enabled toggle
- 2: bit depth — steps through `[8, 16, 20, 24, 32]`
- 3: noise shaping — cycles through all 9 algorithm names

**Key bindings:**

| Key | Action |
|-----|--------|
| `tab` / `shift+tab` | Move between fields 0–3 |
| `+` / `=` | Nudge field up (next step / next algorithm / toggle flip) |
| `-` / `_` | Nudge field down |
| `q` / `esc` | Commit and close |

**Commit on close:** sends four IPC messages via `sendFn`:
- `dsp.dither_enabled`
- `dsp.dither_auto`
- `dsp.dither_bit_depth`
- `dsp.dither_noise_shaping`

No per-keystroke updates (same atomicity policy as EQ editor and crossfeed dialog).

**State:**

```go
type DitherDialogModel struct {
    enabled      bool
    auto         bool
    bitDepth     int        // index into [8,16,20,24,32]
    shapingIdx   int        // index into algorithm name list
    field        int        // 0=auto, 1=enabled, 2=bitDepth, 3=shaping
    width, height int
    sendFn       func(key string, value interface{}) tea.Cmd
}

var bitDepths    = []int{"8", "16", "20", "24", "32"}
var shapingNames = []string{
    "none", "lipshitz", "fweighted", "modified_e_weighted",
    "improved_e_weighted", "shibata", "low_shibata", "high_shibata", "gesemann",
}
```

### `tui/internal/ui/screens/settings.go` (modified)

Add to DSP Audio category (after Crossfeed):

```go
{
    label:       "Dither",
    key:         "dsp.dither_enabled",
    kind:        settingAction,
    description: "TPDF dither + noise shaping — reduce quantization artifacts at output",
},
```

Add to `settingAction` switch:

```go
case "dsp.dither_enabled":
    dialog := NewDitherDialogModel(func(key string, val interface{}) tea.Cmd {
        return func() tea.Msg { return SettingsChangedMsg{Key: key, Value: val} }
    })
    dialog.SetSize(m.width, m.height)
    return m, screen.TransitionCmd(dialog, true)
```

---

## Testing

### Rust (`dither.rs`)

- `silence_in_silence_out_32bit` — all-zero input, `bit_depth=32`, `NoiseShaping::None`
  → output equals input exactly (no quantization at full float precision)
- `tpdf_zero_mean` — process 10,000 frames of 0.5 full-scale DC, `NoiseShaping::None`;
  assert mean of (output − input) < 0.001 (TPDF is unbiased)
- `quantization_snaps_to_lsb` — `bit_depth=16`; assert all output values are exact
  multiples of `1.0 / 32768.0` (within f32 epsilon)
- `noise_shaping_pushes_noise_high` — Lipshitz shaping, 1 kHz sine at −60 dBFS,
  2048 frames; measure noise energy below 10 kHz vs above 10 kHz; assert
  `energy_high > energy_low` (shaping has pushed noise up)
- `sample_rate_change_resets_state` — process at 44100 then 48000; assert no panic
  and output first frame after rate change is not contaminated by old error state
- `shibata_selects_nearest_rate_table` — call `process()` at 44100, 48000, 96000;
  assert no panic (96000 falls back to 48000 table)
- `gesemann_iir_no_nan` — process 1000 frames of sine; assert no NaN or Inf in output
- `set_params_resets_state` — call `set_params` mid-stream; assert error_buf is zeroed

### Rust (`manager.rs`)

- `dsp_dither_keys`:
  - `dither_enabled = true` parses correctly
  - `dither_bit_depth = 16` parses correctly
  - `dither_bit_depth = 4` clamps to 8
  - `dither_bit_depth = 64` clamps to 32
  - `dither_noise_shaping = shibata` parses correctly
  - Unknown `dither_noise_shaping` value returns error

### Go (`dither_dialog_test.go`)

- `TestDitherDialogView_ContainsFields` — `View().Content` contains "Dither",
  "Bit depth", "Noise"
- `TestDitherDialogView_Golden` — golden file at
  `testdata/dither_dialog_golden.txt`
- `TestSettingsHasDitherEntry` — settings view contains "Dither"

---

## File Structure

| File | Action |
|------|--------|
| `runtime/src/dsp/dither.rs` | Create |
| `runtime/src/dsp/config.rs` | Modify |
| `runtime/src/dsp/mod.rs` | Modify |
| `runtime/src/config/manager.rs` | Modify |
| `tui/internal/ui/screens/dither_dialog.go` | Create |
| `tui/internal/ui/screens/dither_dialog_test.go` | Create |
| `tui/internal/ui/screens/settings.go` | Modify |
