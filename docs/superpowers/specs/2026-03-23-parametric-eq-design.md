# Parametric EQ (Biquad) Design

## Goal

Add a fully parametric equalizer to the STUI DSP pipeline: up to 10 biquad bands (all standard filter types), dynamic pipeline position based on active stages, and a full-screen TUI editor with a braille frequency response curve.

## Architecture

### Signal Flow

Pipeline position is resolved at `process()` time from the live config. The current sample rate at the EQ stage is passed in; biquad coefficients are recomputed automatically if it changes.

| Condition | Signal chain |
|-----------|-------------|
| convolution enabled | `input → resample → DSD→PCM → convolution → [EQ] → output` |
| convolution disabled, resample enabled | `input → [EQ] → resample → DSD→PCM → output` |
| convolution disabled, resample disabled, DSD→PCM enabled | `input → DSD→PCM → [EQ] → output` |
| all stages disabled | `input → [EQ] → output` |

The single rule: **EQ goes after convolution if enabled; otherwise before resample if enabled; otherwise after DSD→PCM if enabled; otherwise as the first and only stage.** EQ does not run before DSD→PCM because EQ-ing DSD-encoded data is meaningless.

**Rate note**: `BiquadFilter` tracks the sample rate at which its coefficients were computed. If the rate at the EQ insertion point changes (e.g., DSD→PCM switches in or out), coefficients are automatically recomputed on the next `process()` call. The Hz values in `EqBand` always represent physical frequencies — they remain valid across rate changes.

### Modules

| Module | Language | Responsibility |
|--------|----------|----------------|
| `runtime/src/dsp/eq.rs` | Rust | Biquad engine: processing loop, coefficient computation |
| `runtime/src/dsp/config.rs` | Rust | `EqFilterType`, `EqBand` structs; new fields on `DspConfig` |
| `runtime/src/dsp/mod.rs` | Rust | Add `eq: Option<ParametricEq>` to `DspPipeline`; dynamic position in `process()` |
| `runtime/src/config/manager.rs` | Rust | `dsp.eq_enabled`, `dsp.eq_bypass`, `dsp.eq_bands` (JSON blob) config keys |
| `tui/internal/ui/screens/eq_editor.go` | Go | Full-screen band editor with braille frequency response curve |

`EqFilterType` and `EqBand` are defined in `config.rs` and imported into `eq.rs`.

---

## Biquad Engine (`runtime/src/dsp/eq.rs`)

### Filter Types (defined in `config.rs`, used in `eq.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EqFilterType {
    Peak, LowShelf, HighShelf, LowPass, HighPass, Notch,
}
```

### BiquadFilter

Direct Form II Transposed. Stereo channels have independent state.

```rust
struct BiquadFilter {
    b0, b1, b2: f32,    // feedforward coefficients
    a1, a2:     f32,    // feedback coefficients (stored as-is from Cookbook;
                         // recurrence subtracts them — see processing loop)
    // Independent state per channel to prevent L/R cross-contamination.
    z1l, z2l:   f32,    // left channel state registers
    z1r, z2r:   f32,    // right channel state registers
    sample_rate: u32,   // rate coefficients were computed at
    band:        EqBand,
}
```

**Sign convention**: `a1` and `a2` are stored as the raw Audio EQ Cookbook denominator coefficients divided by `a0` (positive as the Cookbook defines). The recurrence subtracts them. Do not pre-negate.

**Coefficient formulas**: Audio EQ Cookbook (Robert Bristow-Johnson). Each `EqFilterType` maps to its specific formula set parameterised by `freq`, `gain_db`, and `q`.

**Sample rate tracking**: if `sample_rate` passed to `process_stereo()` differs from `self.sample_rate`, recompute coefficients and reset all four state registers to zero before processing.

**Denormal protection**: `1e-25f32` is added to both `z1` and `z2` state registers for each channel, inline during the state update. Both registers need protection because both can accumulate subnormals during silence.

```
// Per sample, per channel (shown for left; mirror for right):
y   = b0*xl + z1l
z1l = b1*xl - a1*y + z2l + 1e-25
z2l = b2*xl - a2*y + 1e-25
```

**`process_stereo` signature**:

```rust
fn process_stereo(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32>
```

Input is stereo-interleaved f32 `[L₀, R₀, L₁, R₁, ...]`. Output is stereo-interleaved f32 of the same length. Left samples are processed through `z1l/z2l`; right samples through `z1r/z2r` — no cross-channel state sharing. Recomputes coefficients and resets all four state registers to zero if `sample_rate` differs from `self.sample_rate`.

### Magnitude Response Formula (used in both Rust and Go)

For a biquad with coefficients `b0, b1, b2, a1, a2`, the squared magnitude at normalized angular frequency `ω = 2π * freq / sample_rate` is:

```
// Numerator (b0 is purely real — no imaginary contribution):
num_re = b0 + b1·cos(ω) + b2·cos(2ω)
num_im =      b1·sin(ω) + b2·sin(2ω)

// Denominator:
den_re = 1  + a1·cos(ω) + a2·cos(2ω)
den_im =      a1·sin(ω) + a2·sin(2ω)

magnitude_dB = 20 * log10(sqrt((num_re² + num_im²) / (den_re² + den_im²)))
```

The Go curve renderer uses this formula with coefficients computed from the same Audio EQ Cookbook equations as the Rust implementation. The Cookbook formulas for all six filter types (Peak, LowShelf, HighShelf, LowPass, HighPass, Notch) are reproduced in the implementation comments of `eq.rs`; the Go side copies those same equations.

### ParametricEq

```rust
pub struct ParametricEq {
    filters: Vec<BiquadFilter>,  // max 10, in band order
    enabled: bool,
    bypass:  bool,
}
```

No `Arc<RwLock<DspConfig>>` — the caller (`DspPipeline`) owns the update path and calls `update_bands` explicitly when the config changes.

**Public methods**:

- `fn new(bands: &[EqBand]) -> Self` — constructs with an initial band list; `enabled = true`, `bypass = false`
- `fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32>` — runs each active filter in sequence via `process_stereo`. If `!self.is_enabled()`, returns `samples.to_vec()` without touching filter state.
- `fn update_bands(&mut self, bands: &[EqBand])` — see *Band Update Rules* below.
- `fn is_enabled(&self) -> bool` — `self.enabled && !self.bypass && !self.filters.is_empty()`
- `fn set_enabled(&mut self, v: bool)` / `fn set_bypass(&mut self, v: bool)`

### Band Update Rules (`update_bands`)

`update_bands` rebuilds `filters` from the new band list. Rules:

- **Truncate at 10**: if `bands.len() > 10`, use only `bands[..10]` and log a `warn!`.
- **State preservation**: for each index `i` where `i < bands.len()` AND `i < old_filters.len()` AND `bands[i].filter_type == old_filters[i].band.filter_type`, **copy the existing state** (`z1l/z2l/z1r/z2r`) into the new filter. This prevents audible clicks when gain, freq, or Q are nudged live.
- **State reset**: reset state to zero when `filter_type` changes at an index, or when the index is new (beyond the old list length).
- **Removed bands**: filters at indices `≥ bands.len()` are dropped and their state is discarded.

Note: sample rate changes are handled separately in `process_stereo` — `update_bands` does not check or use `BiquadFilter.sample_rate`.

### Band Limit in TUI

The `a` (add) key is visually disabled (greyed out) when 10 bands are already present.

### Input Validation / Clamping

`freq`, `gain_db`, and `q` are clamped at two points:
1. **Config manager** (`apply_dsp_key`): clamp after deserializing `eq_bands` — `freq` to 20.0–20000.0, `gain_db` to ±20.0, `q` to 0.1–10.0.
2. **TUI nudge**: clamp at the limits before displaying or sending.

`ParametricEq::update_bands` also clamps defensively and logs `warn!` rather than panicking on out-of-range values. Values arriving from a manually edited config file are clamped silently.

**`enabled` flag**: toggling a band's `enabled` field does **not** reset its filter state — state is preserved so re-enabling the band produces no transient click.

**`bypass` semantics**: `bypass = true` causes `ParametricEq::process` to return `samples.to_vec()` immediately, skipping all filters and leaving all state registers unchanged.

**Inline text edit validation**: non-numeric characters are rejected character-by-character. `freq` accepts Hz integers only (no unit suffixes). A blank or unparseable value on `Enter` reverts to the previous value without committing.

---

## Configuration & Persistence

### `EqBand` (defined in `config.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqBand {
    pub enabled:     bool,
    pub filter_type: EqFilterType,
    pub freq:        f32,    // Hz, 20.0–20000.0
    pub gain_db:     f32,    // dB, ±20.0 (ignored for LowPass/HighPass/Notch)
    pub q:           f32,    // 0.1–10.0
}
```

### New `DspConfig` Fields

```rust
pub eq_enabled: bool,        // default: false
pub eq_bypass:  bool,        // default: false
pub eq_bands:   Vec<EqBand>, // default: empty
```

### Config Manager Keys

| Key | Type | Notes |
|-----|------|-------|
| `dsp.eq_enabled` | bool | enables the EQ stage |
| `dsp.eq_bypass` | bool | bypasses all bands |
| `dsp.eq_bands` | JSON string | full band list serialized as a JSON array |

`dsp.eq_bands` is sent as a single JSON-encoded string. The manager deserializes with `serde_json::from_str::<Vec<EqBand>>` then clamps all fields.

### Update Atomicity

`dsp.eq_bands` is sent **only on field commit**, not on every keystroke. Commit events:
- `Enter` — closes an inline text edit
- `space` — toggles band enabled
- `tab`/`shift+tab` — leaves a field after nudging
- `q`/`Esc` — closes the editor

This prevents partial values (e.g., `"10"` mid-way to `"1000"`) from reaching the audio pipeline.

---

## TUI EQ Editor (`tui/internal/ui/screens/eq_editor.go`)

### Layout

Full-screen Bubbletea model, opened as an overlay from the settings screen.

```
┌─────────────────────────────────────────────────────────────────┐
│  Parametric EQ  [enabled]  [bypass: off]                        │
├─────────────────────────────────────────────────────────────────┤
│  +20dB ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ │
│        ⠀⠀⠀⠀⢀⡠⠤⠒⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⠔⠒⠢⣀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ │  ← braille curve
│   0dB  ─────────────────────────────────────────────────── │
│        ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ │
│  -20dB ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ │
│         20Hz                  1kHz                   20kHz    │
├─────────────────────────────────────────────────────────────────┤
│  # │ on │ type      │   freq   │ gain  │   Q              │
│  1 │ ✓  │ Peak      │  1000 Hz │ +3.0  │ 1.00             │  ← selected
│  2 │ ✓  │ LowShelf  │    80 Hz │ +2.0  │ 0.71             │
│  3 │ ✗  │ LowPass   │ 18000 Hz │  ---  │ 0.71             │
├─────────────────────────────────────────────────────────────────┤
│  a add  d del  space toggle  tab next field  +/- nudge  e edit  │
│  b bypass  q close                                              │
└─────────────────────────────────────────────────────────────────┘
```

### Braille Curve Rendering

The curve zone is `termWidth × curveHeight` terminal cells. Each braille cell encodes a 2×4 subpixel grid, giving effective resolution of `2*termWidth` horizontal bins × `4*curveHeight` amplitude steps.

Frequency evaluation uses `2 * renderedWidth` log-spaced points so the curve always matches rendered pixel density.

Steps:
1. Compute `2 * renderedWidth` log-spaced frequencies from 20 Hz to 20 kHz.
2. For each enabled band, compute coefficients using the Audio EQ Cookbook formulas at the displayed sample rate (default 44100 Hz when no rate is known from config). Evaluate the combined magnitude in dB using the `|H(ω)|` formula from the *Magnitude Response Formula* section above, summing dB values across all active bands.
3. Map dB values (clamped to ±20dB) to row positions; map frequencies to column positions.
4. Compose braille characters (U+2800–U+28FF) by accumulating subpixel bits per cell. **No external library** — implement the 2×4 braille bit-packing directly (~30 lines). The existing codebase uses no braille library and adding one is unnecessary for this use.

The 0dB reference line is always drawn as `─` characters. When all bands are disabled or the band list is empty, only the 0dB line is rendered — no braille curve is drawn. Gain column shows `---` for LP, HP, and Notch bands.

### Keyboard Controls

| Key | Active field | Action |
|-----|-------------|--------|
| `a` | any | Add band (default: Peak, 1kHz, 0dB, Q=1.0); greyed out at 10 bands |
| `d` | any | Delete selected band |
| `space` | any | Toggle selected band enabled (commits immediately) |
| `tab` / `shift+tab` | any | Move to next/prev band row; commits any active nudge |
| `←` / `→` | any | Cycle active column: type → freq → gain → Q (gain skipped for LP/HP/Notch) |
| `+` / `-` | type | Cycle to next/prev filter type in the enum order |
| `+` / `-` | freq | ×1.05 / ÷1.05, clamped 20–20000 Hz |
| `+` / `-` | gain | +0.5dB / -0.5dB, clamped ±20dB; key has no effect on LP/HP/Notch |
| `+` / `-` | Q | +0.05 / -0.05, clamped 0.1–10.0 |
| `e` | type/freq/Q | Open inline text input; disabled for gain on LP/HP/Notch |
| `Enter` | (editing) | Commit inline text edit; sends `dsp.eq_bands` |
| `b` | any | Toggle global EQ bypass (commits immediately) |
| `q` / `Esc` | any | Close editor; commits any uncommitted nudge state |

**Gain field for LP/HP/Notch**: the gain column displays `---` and is non-interactive. `←`/`→` navigation skips it; `+`/`-` on gain has no effect; `e` on gain does nothing. This is enforced in the Bubbletea `Update` handler.

The curve redraws on every committed change.

### Golden File Test

The `teatest` golden file renders the editor at a fixed terminal size of **120 columns × 40 rows** with a known 3-band configuration (1× Peak at 1kHz +3dB Q=1.0, 1× LowShelf at 80Hz +2dB Q=0.71, 1× LowPass at 18kHz Q=0.71). The test sets `termWidth = 120` and `termHeight = 40` explicitly before rendering.

---

## Testing

### Rust (`runtime/src/dsp/eq.rs`)

- **Per filter type**: verify `|H(ω)|` at center/corner frequency matches expected dB (±0.1dB tolerance)
- **Denormal**: process 10k silence samples; assert `z1l.is_normal() || z1l == 0.0` and same for `z2l`, `z1r`, `z2r`
- **Bypass**: output slice equals input slice when `bypass = true`
- **Sample rate change**: process at 44100, then call `process` with `sample_rate = 192000`; assert `b0` changes and no panic
- **Cascade**: two Peak bands; verify combined magnitude at each center frequency
- **State preservation on nudge**: call `update_bands` with same `filter_type`, different `freq`; assert `z1l` is unchanged
- **State reset on type change**: call `update_bands` changing `Peak` to `LowPass`; assert `z1l == 0.0`
- **Removed bands**: call `update_bands` going from 3 bands to 1; assert `filters.len() == 1`

### Go (`tui/internal/ui/screens/eq_editor.go`)

- **Flat response**: no active bands → curve is a straight 0dB line
- **Single peak**: one Peak band at 1kHz; column at 1kHz is the highest amplitude cell
- **teatest golden file**: 120×40 render with the 3-band config described above

### Integration

Integration tests use a `StageLog` — a `Vec<String>` wrapped in `Arc<Mutex<_>>` that each test-instrumented stage appends its name to when `process()` is called. The log is passed into a test-only variant of `DspPipeline::process_with_log(samples, rate, log)` that appends stage names in execution order. This is a test-only API in `#[cfg(test)]`.

- `convolution_enabled = true`: log contains `[..., "convolution", "eq"]` in that order
- `convolution_enabled = false`, `resample_enabled = true`: log contains `["eq", "resample", ...]`
