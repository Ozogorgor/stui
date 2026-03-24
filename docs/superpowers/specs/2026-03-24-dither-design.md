# Dither + Noise Shaping Design Spec

## Overview

Add TPDF dither with selectable noise shaping to the STUI DSP pipeline. Dither adds
low-level triangular-PDF noise before bit-depth quantization to linearize quantization
error. Noise shaping filters the error feedback signal to push quantization noise into
less audible high-frequency bands.

**Algorithm:** TPDF dither + FIR/IIR error-feedback noise shaping
**Parameters:** `bit_depth` (8–32, default 16) and `noise_shaping` (9 selectable
algorithms), fully user-configurable
**Auto-detection:** When `dither_auto = true`, dither is enabled automatically if
`output_target == Alsa && dither_bit_depth == 16` (the case where dither matters most).
Same two-field pattern as crossfeed: user must opt in to auto mode.
**TUI:** Lipgloss-bordered dialog pushed from DSP Audio settings via
`screen.TransitionCmd`, same pattern as crossfeed dialog

---

## Algorithm

### TPDF Dither + Error Feedback Noise Shaping

Input is assumed to be interleaved stereo; `samples.len()` must be even.
Odd-length input is a caller error — `process()` panics in debug builds.

For each interleaved stereo frame `[in_L, in_R]`, apply per-channel:

**FIR algorithms (all except Gesemann and None):**
```
tpdf      = uniform() + uniform()           // TPDF, range [-1, 1] LSB
shaped    = tpdf + dot(coeffs, error_buf)   // add filtered past error
scale     = 2^(bit_depth - 1)
out       = round(in * scale + shaped) / scale
error_buf = [in - out, error_buf[0..n-2]]  // shift in new error
```

**IIR algorithm (Gesemann only):**
```
tpdf      = uniform() + uniform()
shaped    = tpdf + dot(ff_coeffs, ff_buf) - dot(fb_coeffs, fb_buf)
scale     = 2^(bit_depth - 1)
out       = round(in * scale + shaped) / scale
ff_buf    = [in - out, ff_buf[0..n-2]]     // feedforward: past quantization errors
fb_buf    = [shaped,   fb_buf[0..n-2]]     // feedback: past shaped values (IIR memory)
```
The feedback term is subtracted (`-`) to match the standard IIR difference equation
convention (`y[n] = b·x[n] - a·y[n-1]`). The SoX `ges44`/`ges48` coefficients are
provided in this sign convention.

**No noise shaping (None):**
```
tpdf      = uniform() + uniform()
scale     = 2^(bit_depth - 1)
out       = round(in * scale + tpdf) / scale
```
No error buffer allocated or updated.

When `bit_depth == 32`, dither is a no-op: return input unchanged (f32 has 24-bit
mantissa; quantizing to 32 integer bits would exceed f32 precision and is meaningless).

TPDF generation uses a xorshift64 RNG seeded at construction. All state (`error_buf`,
`ff_buf`, `fb_buf`, `rng_state`) is reset whenever `bit_depth`, `noise_shaping`, or
`sample_rate` changes.

### Noise Shaping Algorithms

All coefficients sourced from SoX `src/dither.c` (public domain / LGPL).

| Name | Taps | Type | Character |
|------|------|------|-----------|
| `none` | — | — | TPDF only, flat noise spectrum |
| `lipshitz` | 5 | FIR | "Minimally audible" — classic Lipshitz reference |
| `fweighted` | 9 | FIR | F-weighted psychoacoustic (Lipshitz-Noll-Subotic) |
| `modified_e_weighted` | 9 | FIR | Gentler E-weighted variant |
| `improved_e_weighted` | 9 | FIR | Stronger E-weighted variant |
| `shibata` | 16–20 | FIR | Aggressive, pushes noise well above 15 kHz |
| `low_shibata` | 15–16 | FIR | Gentler Shibata variant |
| `high_shibata` | 20 | FIR | Most aggressive Shibata variant |
| `gesemann` | 4 | IIR | Psychoacoustically shaped, efficient |

#### Shibata per-rate selection

Shibata has coefficient tables at 8k / 11k / 16k / 22k / 32k / 37.8k / 44.1k / 48k.
The active table is selected once when `sample_rate` is first set or changes (same
event that clears `error_buf`). The table whose rate is closest to `sample_rate` is
chosen (ties go to the higher rate). For rates above 48k (e.g. 96k, 192k), the 48k
table is used. The pipeline invariant guarantees stereo interleaved input; all stages
including dither operate on stereo frames only.

Low-Shibata has tables at 44.1k and 48k only; all other rates fall back to whichever
is nearer (≤46050 Hz → 44.1k table, >46050 Hz → 48k table).

High-Shibata has a single 44.1k table; used at all sample rates (no per-rate selection).

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

**Shibata 8k (`shi08`, 20 taps, inverted sign convention):**
```
[-1.202863335609436,   -0.94103097915649414, -0.67878556251525879,
 -0.57650017738342285, -0.50004476308822632, -0.44349345564842224,
 -0.37833768129348755, -0.34028723835945129, -0.29413089156150818,
 -0.24994957447052002, -0.21715600788593292, -0.18792112171649933,
 -0.15268312394618988, -0.12135542929172516, -0.099610626697540283,
 -0.075273610651493073,-0.048787496984004974,-0.042586319148540497,
 -0.028991291299462318,-0.011869125068187714]
```

**Shibata 11k (`shi11`, 20 taps, inverted sign convention):**
```
[-0.9264228343963623,  -0.98695987462997437, -0.631156325340271,
 -0.51966935396194458, -0.39738872647285461, -0.35679301619529724,
 -0.29720726609230042, -0.26310476660728455, -0.21719355881214142,
 -0.18561814725399017, -0.15404847264289856, -0.12687471508979797,
 -0.10339745879173279, -0.083688631653785706,-0.05875682458281517,
 -0.046893671154975891,-0.027950936928391457,-0.020740609616041183,
 -0.009366452693939209,-0.0060260160826146603]
```

**Shibata 16k (`shi16`, 20 taps, inverted sign convention):**
```
[-0.37251132726669312, -0.81423574686050415, -0.55010956525802612,
 -0.47405767440795898, -0.32624706625938416, -0.3161766529083252,
 -0.2286367267370224,  -0.22916607558727264, -0.19565616548061371,
 -0.18160104751586914, -0.15423151850700378, -0.14104481041431427,
 -0.11844276636838913, -0.097583092749118805,-0.076493598520755768,
 -0.068106919527053833,-0.041881654411554337,-0.036922425031661987,
 -0.019364040344953537,-0.014994367957115173]
```

**Shibata 22k (`shi22`, 20 taps, inverted sign convention):**
```
[ 0.056581053882837296,-0.56956905126571655, -0.40727734565734863,
 -0.33870288729667664, -0.29810553789138794, -0.19039161503314972,
 -0.16510021686553955, -0.13468159735202789, -0.096633769571781158,
 -0.081049129366874695,-0.064953058958053589,-0.054459091275930405,
 -0.043378707021474838,-0.03660014271736145, -0.026256965473294258,
 -0.018786206841468811,-0.013387725688517094,-0.0090983230620622635,
 -0.0026585909072309732,-0.00042083300650119781]
```

**Shibata 32k (`shi32`, 20 taps, inverted sign convention):**
```
[ 0.82118552923202515, -1.0063692331314087,   0.62341964244842529,
 -1.0447187423706055,   0.64532512426376343, -0.87615132331848145,
  0.52219754457473755, -0.67434263229370117,  0.44954317808151245,
 -0.52557498216629028,  0.34567299485206604, -0.39618203043937683,
  0.26791760325431824, -0.28936097025871277,  0.1883765310049057,
 -0.19097308814525604,  0.10431359708309174, -0.10633844882249832,
  0.046832218766212463,-0.039653312414884567]
```

**Shibata 37.8k (`shi38`, 16 taps):**
```
[ 1.6335992813110351562, -2.2615492343902587891,  2.4077029228210449219,
 -2.6341717243194580078,  2.1440362930297851562, -1.8153258562088012695,
  1.0816224813461303711, -0.70302653312683105469, 0.15991993248462677002,
  0.041549518704414367676,-0.29416576027870178223, 0.2518316805362701416,
 -0.27766478061676025391,  0.15785403549671173096,-0.10165894031524658203,
  0.016833892092108726501]
```

Note: `shi08`–`shi32` are marked "inverted" in the SoX source (generated by the
`dmaker` tool). Use these coefficients with the standard `+ dot(coeffs, error_buf)`
formula — the inversion is already baked into the coefficient values.

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

**Initialisation** (`new`) and **`update_config`**: the auto condition is evaluated
statically at config-apply time (not in the audio path). Gate construction on:
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
    as_u32(key, value)?.clamp(8, 32),
"dither_noise_shaping" => {
    let s = as_string(key, value)?;
    match s.as_str() {
        "none" | "lipshitz" | "fweighted" | "modified_e_weighted" |
        "improved_e_weighted" | "shibata" | "low_shibata" | "high_shibata" | "gesemann"
            => cfg.dsp.dither_noise_shaping = s,
        _ => return Err(format!("unknown dither_noise_shaping value: {s}")),
    }
},
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
- `dsp.dither_enabled` — `m.enabled` (bool)
- `dsp.dither_auto` — `m.auto` (bool)
- `dsp.dither_bit_depth` — `bitDepths[m.bitDepth]` (the resolved int value, not the index)
- `dsp.dither_noise_shaping` — `shapingNames[m.shapingIdx]` (the algorithm name string)

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

var bitDepths    = []int{8, 16, 20, 24, 32}
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

- `noop_at_32bit` — DC input at 0.5 full-scale, `bit_depth=32`; assert output equals
  input exactly (bit_depth=32 is the no-op path — dither is skipped entirely; a
  non-zero input is required so the test distinguishes the no-op path from a broken
  dither path that happens to pass on silence)
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
- `set_params_resets_state` — call `set_params` mid-stream with Gesemann shaping active;
  assert `error_buf`, `ff_buf`, and `fb_buf` are all zeroed after the call;
  `rng_state` is re-seeded to the construction seed (not zeroed — a zero xorshift64
  state is stuck; verify `rng_state != 0` after reset)

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
