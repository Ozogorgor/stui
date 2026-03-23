# Parametric EQ (Biquad) Design

## Goal

Add a fully parametric equalizer to the STUI DSP pipeline: up to 10 biquad bands (all standard filter types), dynamic pipeline position based on active stages, and a full-screen TUI editor with a braille frequency response curve.

## Architecture

### Signal Flow

Pipeline position is resolved at `process()` time from the live config:

```
convolution disabled:  input → [EQ] → resample → DSD→PCM → output
convolution enabled:   input → resample → DSD→PCM → convolution → [EQ] → output
both disabled:         input → [EQ] → output
```

EQ before resampling corrects at native source rate (44.1/48 kHz). EQ after convolution applies personal tonal shaping on top of room correction — the audiophile-correct order.

### Modules

| Module | Language | Responsibility |
|--------|----------|----------------|
| `runtime/src/dsp/eq.rs` | Rust | Biquad engine: coefficient computation, stateful processing, denormal protection |
| `runtime/src/dsp/config.rs` | Rust | `EqBand`, `EqFilterType` structs; new fields on `DspConfig` |
| `runtime/src/dsp/mod.rs` | Rust | Add `eq: Option<ParametricEq>` to `DspPipeline`; dynamic position in `process()` |
| `runtime/src/config/manager.rs` | Rust | `dsp.eq_enabled`, `dsp.eq_bypass`, `dsp.eq_bands` (JSON blob) config keys |
| `tui/internal/ui/screens/eq_editor.go` | Go | Full-screen band editor with braille frequency response curve |

---

## Biquad Engine (`runtime/src/dsp/eq.rs`)

### Filter Types

```rust
pub enum EqFilterType {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
}
```

### BiquadFilter

Direct Form II Transposed. State variables `z1`, `z2` are per-band.

```rust
struct BiquadFilter {
    b0, b1, b2: f32,  // feedforward
    a1, a2:     f32,  // feedback
    z1, z2:     f32,  // DF-II transposed state
    sample_rate: u32, // rate coefficients were computed at
    band:        EqBand,
}
```

**Coefficient formulas**: Audio EQ Cookbook (Robert Bristow-Johnson). Each filter type maps to a specific set of formulas parameterised by `freq`, `gain_db`, and `q`.

**Sample rate tracking**: `process_stereo()` receives the current sample rate. If it differs from `self.sample_rate`, coefficients are recomputed before processing. This handles pipeline sample rate changes (e.g. DSD→PCM path switching in).

**Denormal protection**: After each sample, `z1 += 1e-25`. This keeps the state registers out of the subnormal range during silence and near-silence without any audible effect.

**Processing** (per sample, stereo interleaved):
```
y = b0*x + z1
z1 = b1*x - a1*y + z2 + 1e-25
z2 = b2*x - a2*y
```

### ParametricEq

```rust
pub struct ParametricEq {
    config:  Arc<RwLock<DspConfig>>,
    filters: Vec<BiquadFilter>,  // max 10, in band order
    enabled: bool,
    bypass:  bool,
}
```

- `process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32>` — stereo interleaved f32 in, stereo interleaved f32 out. Bypass returns `samples.to_vec()` without touching state.
- `update_bands(&mut self, bands: &[EqBand])` — rebuilds `filters` from the band list; called when config changes.
- `is_enabled(&self) -> bool` — `enabled && !bypass && !filters.is_empty()`

---

## Configuration & Persistence

### New Types in `config.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EqFilterType {
    Peak, LowShelf, HighShelf, LowPass, HighPass, Notch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqBand {
    pub enabled:     bool,
    pub filter_type: EqFilterType,
    pub freq:        f32,    // Hz, 20.0–20000.0
    pub gain_db:     f32,    // dB, ±20.0 (ignored for LP/HP/Notch)
    pub q:           f32,    // 0.1–10.0
}
```

### New `DspConfig` Fields

```rust
pub eq_enabled: bool,   // default: false
pub eq_bypass:  bool,   // default: false
pub eq_bands:   Vec<EqBand>,  // default: empty
```

### Config Manager Keys

| Key | Type | Notes |
|-----|------|-------|
| `dsp.eq_enabled` | bool | enables the EQ stage |
| `dsp.eq_bypass` | bool | bypasses all bands |
| `dsp.eq_bands` | JSON string | full band list serialized as JSON array |

`dsp.eq_bands` is sent as a single JSON-encoded string value. The manager deserializes it with `serde_json::from_str::<Vec<EqBand>>`. This avoids indexed flat keys (`dsp.eq_band.0.freq` etc.) while staying within the existing `apply_dsp_key` dispatch pattern.

---

## TUI EQ Editor (`tui/internal/ui/screens/eq_editor.go`)

### Layout

Full-screen Bubbletea model, opened as an overlay from the settings screen.

```
┌─────────────────────────────────────────────────────────────────┐
│  Parametric EQ  [enabled]  [bypass: off]                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
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

The curve zone is `termWidth × curveHeight` braille cells. Each braille cell covers 2×4 subpixels (2 columns, 4 rows), giving effective resolution of `2*termWidth` frequency bins × `4*curveHeight` amplitude steps.

1. Compute 200 log-spaced frequencies from 20Hz to 20kHz.
2. For each frequency, evaluate combined magnitude in dB by summing per-band magnitude responses (Audio EQ Cookbook transfer function `|H(jω)|`).
3. Map dB values to row positions, map frequencies to column positions.
4. Render using `github.com/rivo/uniseg` or direct Unicode braille (U+2800–U+28FF) block composition.

The 0dB reference line is always drawn as `─` characters.

Gain column shows `---` for LP, HP, and Notch bands (gain is not applicable).

### Keyboard Controls

| Key | Action |
|-----|--------|
| `a` | Add band (default: Peak, 1kHz, 0dB, Q=1.0) |
| `d` | Delete selected band |
| `space` | Toggle selected band enabled |
| `tab` / `shift+tab` | Move between bands |
| `←` / `→` | Cycle active field (type → freq → gain → Q) |
| `+` / `-` | Nudge active field (freq: ×1.05/÷1.05; gain: ±0.5dB; Q: ±0.05) |
| `e` | Open inline text input for active field |
| `b` | Toggle global EQ bypass |
| `q` / `Esc` | Close editor, persist bands to config |

Every edit triggers an immediate curve redraw and sends `dsp.eq_bands` to the runtime via the existing IPC channel.

---

## Testing

### Rust (`runtime/src/dsp/eq.rs`)

- **Per filter type**: verify `|H(jω)|` at center/corner frequency matches expected dB (±0.1dB tolerance)
- **Denormal**: process 10k silence samples; assert `z1.is_normal() || z1 == 0.0` after
- **Bypass**: output equals input exactly when bypass is true
- **Sample rate change**: process at 44100 then 192000; assert coefficients recomputed, no panic
- **Cascade**: two Peak bands; verify combined response at each center frequency

### Go (`tui/internal/ui/screens/eq_editor.go`)

- **Flat response**: no bands → curve renders as straight 0dB line
- **Single peak**: magnitude at center-freq column is the maximum rendered cell
- **teatest golden file**: full editor render with a known 3-band configuration

### Integration

- With convolution enabled: assert EQ processing occurs after convolution output
- With only resampling: assert EQ processing occurs before resampler input
