# Parametric EQ (Biquad) Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a 10-band parametric biquad EQ to the STUI DSP pipeline with a full-screen TUI editor featuring a braille frequency response curve.

**Architecture:** Direct Form II Transposed biquads (Audio EQ Cookbook) with per-channel stereo state, denormal protection, and automatic coefficient recomputation on sample rate change. Dynamic pipeline position (before resample when convolution disabled, after convolution when enabled). Full-screen Bubbletea EQ editor with braille curve, band table, and nudge/inline-edit controls.

**Tech Stack:** Rust (rustfft patterns already in repo), Go + Bubbletea v2, Lipgloss, encoding/json for band persistence.

**Spec:** `docs/superpowers/specs/2026-03-23-parametric-eq-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `runtime/src/dsp/config.rs` | Modify | Add `EqFilterType`, `EqBand`, `DspConfig` eq fields |
| `runtime/src/dsp/eq.rs` | Create | `BiquadFilter` + `ParametricEq` |
| `runtime/src/dsp/mod.rs` | Modify | Wire `ParametricEq` into `DspPipeline` |
| `runtime/src/config/manager.rs` | Modify | `dsp.eq_enabled`, `dsp.eq_bypass`, `dsp.eq_bands` keys |
| `tui/internal/ui/screens/eq_editor.go` | Create | Full-screen EQ editor: braille curve + band table + controls |
| `tui/internal/ui/screens/settings.go` | Modify | Add EQ entry in DSP Audio category |

---

## Chunk 1: Rust Backend

### Task 1: Add EqFilterType, EqBand, and DspConfig fields

**Files:**
- Modify: `runtime/src/dsp/config.rs`

- [ ] **Step 1: Write the failing test**

Add at the bottom of `runtime/src/dsp/config.rs`:

```rust
#[cfg(test)]
mod eq_config_tests {
    use super::*;
    use serde_json;

    #[test]
    fn eq_band_roundtrip() {
        let band = EqBand {
            enabled:     true,
            filter_type: EqFilterType::Peak,
            freq:        1000.0,
            gain_db:     3.0,
            q:           1.0,
        };
        let json = serde_json::to_string(&band).unwrap();
        let back: EqBand = serde_json::from_str(&json).unwrap();
        assert_eq!(back.freq, 1000.0);
        assert_eq!(back.gain_db, 3.0);
    }

    #[test]
    fn dsp_config_eq_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.eq_enabled);
        assert!(!cfg.eq_bypass);
        assert!(cfg.eq_bands.is_empty());
    }
}
```

- [ ] **Step 2: Run test — expect FAIL (EqBand/EqFilterType undefined)**

```bash
cd runtime && cargo test eq_config_tests -- --test-threads=1 2>&1 | grep -E "error|FAILED|ok"
```

Expected: compile error `cannot find type EqBand`

- [ ] **Step 3: Add EqFilterType and EqBand to config.rs**

Add after the `OutputTarget` block (around line 93) in `runtime/src/dsp/config.rs`:

```rust
/// Filter types for parametric EQ bands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EqFilterType {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
}

impl Default for EqFilterType {
    fn default() -> Self { Self::Peak }
}

/// A single parametric EQ band.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqBand {
    pub enabled:     bool,
    pub filter_type: EqFilterType,
    /// Center/corner frequency in Hz (clamped 20.0–20000.0).
    pub freq:        f32,
    /// Gain in dB (clamped ±20.0). Ignored for LowPass, HighPass, Notch.
    pub gain_db:     f32,
    /// Q factor (clamped 0.1–10.0).
    pub q:           f32,
}

impl Default for EqBand {
    fn default() -> Self {
        Self {
            enabled:     true,
            filter_type: EqFilterType::Peak,
            freq:        1000.0,
            gain_db:     0.0,
            q:           1.0,
        }
    }
}
```

Then add three fields to `DspConfig` (after `pipewire_role`):

```rust
    /// Enable the parametric EQ stage.
    pub eq_enabled: bool,
    /// Bypass all EQ bands (pass-through).
    pub eq_bypass:  bool,
    /// Parametric EQ band definitions (max 10).
    pub eq_bands:   Vec<EqBand>,
```

And in `DspConfig::default()`:

```rust
            eq_enabled: false,
            eq_bypass:  false,
            eq_bands:   Vec::new(),
```

- [ ] **Step 4: Run test — expect PASS**

```bash
cd runtime && cargo test eq_config_tests -- --test-threads=1 2>&1 | grep -E "FAILED|ok"
```

Expected: `test result: ok. 2 passed`

- [ ] **Step 5: Commit**

```bash
git add runtime/src/dsp/config.rs
git commit -m "feat(eq): add EqFilterType, EqBand, DspConfig eq fields"
```

---

### Task 2: Implement BiquadFilter

**Files:**
- Create: `runtime/src/dsp/eq.rs`
- Modify: `runtime/src/dsp/mod.rs` (add `pub mod eq;`)

- [ ] **Step 1: Write the failing tests**

Create `runtime/src/dsp/eq.rs` with tests only (no implementation yet):

```rust
//! Parametric EQ: biquad filter bank, Direct Form II Transposed.
//!
//! Coefficient formulas from Audio EQ Cookbook (Robert Bristow-Johnson).
//! See magnitude_db() below for the transfer function evaluation.

use super::config::{EqBand, EqFilterType};

// ── Coefficient computation ────────────────────────────────────────────────

/// Normalised biquad coefficients (b0,b1,b2 feedforward; a1,a2 feedback).
/// a1/a2 are stored as positive Cookbook values; the recurrence subtracts them.
#[derive(Debug, Clone, Copy)]
struct Coeffs { b0: f32, b1: f32, b2: f32, a1: f32, a2: f32 }

fn compute_coeffs(band: &EqBand, sample_rate: u32) -> Coeffs {
    todo!()
}

// ── BiquadFilter ───────────────────────────────────────────────────────────

pub struct BiquadFilter {
    c:           Coeffs,
    z1l: f32, z2l: f32,  // left channel state
    z1r: f32, z2r: f32,  // right channel state
    sample_rate: u32,
    pub band:    EqBand,
}

impl BiquadFilter {
    pub fn new(band: EqBand, sample_rate: u32) -> Self {
        todo!()
    }

    /// Process stereo-interleaved samples [L0,R0,L1,R1,...].
    /// Recomputes coefficients if sample_rate changed (and resets state).
    pub fn process_stereo(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        todo!()
    }
}

// ── Magnitude helper ───────────────────────────────────────────────────────

/// Evaluate |H(ω)| in dB for normalised angular frequency ω = 2π*freq/rate.
pub fn magnitude_db(c: Coeffs, omega: f32) -> f32 {
    todo!()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn omega(freq_hz: f32, sample_rate: u32) -> f32 {
        2.0 * PI * freq_hz / sample_rate as f32
    }

    fn make_filter(ft: EqFilterType, freq: f32, gain_db: f32, q: f32, sr: u32) -> BiquadFilter {
        BiquadFilter::new(EqBand {
            enabled: true, filter_type: ft,
            freq, gain_db, q,
        }, sr)
    }

    // ── coefficient / magnitude tests ──────────────────────────────────────

    #[test]
    fn peak_center_gain() {
        // Peak +6dB at 1kHz, Q=1, 44100Hz → magnitude at 1kHz ≈ +6dB
        let f = make_filter(EqFilterType::Peak, 1000.0, 6.0, 1.0, 44100);
        let db = magnitude_db(f.c, omega(1000.0, 44100));
        assert!((db - 6.0).abs() < 0.1, "peak: got {db:.3}dB, expected 6.0dB");
    }

    #[test]
    fn lowpass_corner_attenuation() {
        // LowPass at 5kHz, Q=0.707 (Butterworth), 44100Hz → -3dB at 5kHz
        let f = make_filter(EqFilterType::LowPass, 5000.0, 0.0, 0.707, 44100);
        let db = magnitude_db(f.c, omega(5000.0, 44100));
        assert!((db - (-3.0)).abs() < 0.5, "lp corner: got {db:.3}dB, expected -3dB");
    }

    #[test]
    fn highpass_corner_attenuation() {
        let f = make_filter(EqFilterType::HighPass, 5000.0, 0.0, 0.707, 44100);
        let db = magnitude_db(f.c, omega(5000.0, 44100));
        assert!((db - (-3.0)).abs() < 0.5, "hp corner: got {db:.3}dB, expected -3dB");
    }

    #[test]
    fn notch_deep_attenuation() {
        // Notch at 1kHz, Q=10 → deep null at 1kHz
        let f = make_filter(EqFilterType::Notch, 1000.0, 0.0, 10.0, 44100);
        let db = magnitude_db(f.c, omega(1000.0, 44100));
        assert!(db < -30.0, "notch: got {db:.3}dB, expected < -30dB");
    }

    #[test]
    fn lowshelf_boost_below_corner() {
        // LowShelf +6dB at 200Hz, Q=0.707, 44100Hz → well below corner: ≈ +6dB
        let f = make_filter(EqFilterType::LowShelf, 200.0, 6.0, 0.707, 44100);
        let db = magnitude_db(f.c, omega(20.0, 44100)); // 20Hz, well below shelf
        assert!((db - 6.0).abs() < 0.5, "lowshelf: got {db:.3}dB at 20Hz, expected ~+6dB");
    }

    #[test]
    fn highshelf_boost_above_corner() {
        let f = make_filter(EqFilterType::HighShelf, 5000.0, 6.0, 0.707, 44100);
        let db = magnitude_db(f.c, omega(18000.0, 44100)); // well above shelf
        assert!((db - 6.0).abs() < 0.5, "highshelf: got {db:.3}dB at 18kHz, expected ~+6dB");
    }

    // ── denormal protection ────────────────────────────────────────────────

    #[test]
    fn denormal_protection() {
        let mut f = make_filter(EqFilterType::Peak, 1000.0, 6.0, 1.0, 44100);
        let silence = vec![0.0f32; 20000]; // 10k stereo samples
        let _ = f.process_stereo(&silence, 44100);
        // All state registers must be normal or zero
        for (name, val) in [("z1l", f.z1l), ("z2l", f.z2l), ("z1r", f.z1r), ("z2r", f.z2r)] {
            assert!(val.is_normal() || val == 0.0,
                "{name} subnormal after silence: {val:e}");
        }
    }

    // ── sample rate change ─────────────────────────────────────────────────

    #[test]
    fn sample_rate_change_recomputes_and_resets() {
        let mut f = make_filter(EqFilterType::Peak, 1000.0, 6.0, 1.0, 44100);
        let b0_before = f.c.b0;
        // Inject some non-zero state
        f.z1l = 0.5; f.z2l = 0.3;
        let tone = vec![0.1f32; 512];
        let _ = f.process_stereo(&tone, 192000); // different rate
        assert_ne!(f.c.b0, b0_before, "b0 should change after rate switch");
        // State must have been reset before processing at new rate
        // (can only verify indirectly — no panic and output is finite)
    }
}
```

Add `pub mod eq;` to `runtime/src/dsp/mod.rs` (after `pub mod convolution;`).

- [ ] **Step 2: Run tests — expect FAIL**

```bash
cd runtime && cargo test dsp::eq::tests -- --test-threads=1 2>&1 | grep -E "error|FAILED|todo"
```

Expected: compile error (todo!() panics) or "not yet implemented"

- [ ] **Step 3: Implement magnitude_db and compute_coeffs**

Replace the `todo!()` bodies in `eq.rs`:

```rust
pub fn magnitude_db(c: Coeffs, omega: f32) -> f32 {
    let (cos1, sin1) = (omega.cos(), omega.sin());
    let (cos2, sin2) = ((2.0 * omega).cos(), (2.0 * omega).sin());
    let num_re = c.b0 + c.b1 * cos1 + c.b2 * cos2;
    let num_im =        c.b1 * sin1 + c.b2 * sin2;
    let den_re = 1.0  + c.a1 * cos1 + c.a2 * cos2;
    let den_im =        c.a1 * sin1 + c.a2 * sin2;
    let ratio = (num_re * num_re + num_im * num_im)
              / (den_re * den_re + den_im * den_im);
    20.0 * ratio.sqrt().log10()
}

fn compute_coeffs(band: &EqBand, sample_rate: u32) -> Coeffs {
    use std::f32::consts::PI;
    let w0    = 2.0 * PI * band.freq / sample_rate as f32;
    let sin_w = w0.sin();
    let cos_w = w0.cos();
    let alpha = sin_w / (2.0 * band.q);

    let (b0, b1, b2, a0, a1, a2) = match band.filter_type {
        EqFilterType::Peak => {
            // A = 10^(dBgain/40)
            let a = 10.0_f32.powf(band.gain_db / 40.0);
            (
                1.0 + alpha * a,
                -2.0 * cos_w,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos_w,
                1.0 - alpha / a,
            )
        }
        EqFilterType::LowShelf => {
            let a = 10.0_f32.powf(band.gain_db / 40.0);
            let sqrt_a = a.sqrt();
            let alpha_s = sin_w / 2.0 * ((a + 1.0 / a) * (1.0 / band.q - 1.0) + 2.0).sqrt();
            (
                a * ((a + 1.0) - (a - 1.0) * cos_w + 2.0 * sqrt_a * alpha_s),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w),
                a * ((a + 1.0) - (a - 1.0) * cos_w - 2.0 * sqrt_a * alpha_s),
                (a + 1.0) + (a - 1.0) * cos_w + 2.0 * sqrt_a * alpha_s,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos_w),
                (a + 1.0) + (a - 1.0) * cos_w - 2.0 * sqrt_a * alpha_s,
            )
        }
        EqFilterType::HighShelf => {
            let a = 10.0_f32.powf(band.gain_db / 40.0);
            let sqrt_a = a.sqrt();
            let alpha_s = sin_w / 2.0 * ((a + 1.0 / a) * (1.0 / band.q - 1.0) + 2.0).sqrt();
            (
                a * ((a + 1.0) + (a - 1.0) * cos_w + 2.0 * sqrt_a * alpha_s),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w),
                a * ((a + 1.0) + (a - 1.0) * cos_w - 2.0 * sqrt_a * alpha_s),
                (a + 1.0) - (a - 1.0) * cos_w + 2.0 * sqrt_a * alpha_s,
                2.0 * ((a - 1.0) - (a + 1.0) * cos_w),
                (a + 1.0) - (a - 1.0) * cos_w - 2.0 * sqrt_a * alpha_s,
            )
        }
        EqFilterType::LowPass => (
            (1.0 - cos_w) / 2.0,
             1.0 - cos_w,
            (1.0 - cos_w) / 2.0,
             1.0 + alpha,
            -2.0 * cos_w,
             1.0 - alpha,
        ),
        EqFilterType::HighPass => (
             (1.0 + cos_w) / 2.0,
            -(1.0 + cos_w),
             (1.0 + cos_w) / 2.0,
             1.0 + alpha,
            -2.0 * cos_w,
             1.0 - alpha,
        ),
        EqFilterType::Notch => (
             1.0,
            -2.0 * cos_w,
             1.0,
             1.0 + alpha,
            -2.0 * cos_w,
             1.0 - alpha,
        ),
    };
    // Normalise by a0
    Coeffs {
        b0: b0 / a0, b1: b1 / a0, b2: b2 / a0,
        a1: a1 / a0, a2: a2 / a0,
    }
}
```

- [ ] **Step 4: Implement BiquadFilter::new and process_stereo**

```rust
impl BiquadFilter {
    pub fn new(band: EqBand, sample_rate: u32) -> Self {
        let c = compute_coeffs(&band, sample_rate);
        Self { c, z1l: 0.0, z2l: 0.0, z1r: 0.0, z2r: 0.0, sample_rate, band }
    }

    pub fn process_stereo(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if sample_rate != self.sample_rate {
            self.c = compute_coeffs(&self.band, sample_rate);
            self.z1l = 0.0; self.z2l = 0.0;
            self.z1r = 0.0; self.z2r = 0.0;
            self.sample_rate = sample_rate;
        }
        let Coeffs { b0, b1, b2, a1, a2 } = self.c;
        let dc = 1e-25_f32;
        let mut out = Vec::with_capacity(samples.len());
        let mut iter = samples.chunks_exact(2);
        for chunk in iter.by_ref() {
            let (xl, xr) = (chunk[0], chunk[1]);
            // Left
            let yl = b0 * xl + self.z1l;
            self.z1l = b1 * xl - a1 * yl + self.z2l + dc;
            self.z2l = b2 * xl - a2 * yl + dc;
            // Right
            let yr = b0 * xr + self.z1r;
            self.z1r = b1 * xr - a1 * yr + self.z2r + dc;
            self.z2r = b2 * xr - a2 * yr + dc;
            out.push(yl);
            out.push(yr);
        }
        // Odd trailing sample (should not occur for stereo but handle gracefully)
        for &x in iter.remainder() {
            let y = b0 * x + self.z1l;
            self.z1l = b1 * x - a1 * y + self.z2l + dc;
            self.z2l = b2 * x - a2 * y + dc;
            out.push(y);
        }
        out
    }
}
```

- [ ] **Step 5: Run tests — expect PASS**

```bash
cd runtime && cargo test dsp::eq::tests -- --test-threads=1 2>&1 | grep -E "FAILED|ok"
```

Expected: `test result: ok. 7 passed`

- [ ] **Step 6: Commit**

```bash
git add runtime/src/dsp/eq.rs runtime/src/dsp/mod.rs
git commit -m "feat(eq): implement BiquadFilter with Cookbook coefficients and denormal protection"
```

---

### Task 3: Implement ParametricEq

**Files:**
- Modify: `runtime/src/dsp/eq.rs` (add ParametricEq + tests)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `eq.rs`:

```rust
    use super::ParametricEq;

    #[test]
    fn parametric_eq_cascade() {
        // Two peaks at different freqs; verify each center is boosted
        let bands = vec![
            EqBand { enabled: true, filter_type: EqFilterType::Peak,
                     freq: 500.0, gain_db: 6.0, q: 1.0 },
            EqBand { enabled: true, filter_type: EqFilterType::Peak,
                     freq: 4000.0, gain_db: 6.0, q: 1.0 },
        ];
        let mut eq = ParametricEq::new(&bands);
        // Generate a tone at 500Hz: 44100Hz, 2-channel, 2048 samples
        let sr = 44100u32;
        let tone: Vec<f32> = (0..2048).flat_map(|i| {
            let s = (2.0 * std::f32::consts::PI * 500.0 * i as f32 / sr as f32).sin();
            [s, s]
        }).collect();
        let out = eq.process(&tone, sr);
        // RMS of output should be greater than RMS of input (boosted at 500Hz)
        let rms_in  = (tone.iter().map(|x| x*x).sum::<f32>() / tone.len() as f32).sqrt();
        let rms_out = (out.iter().map(|x| x*x).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms_out > rms_in * 1.1,
            "cascade: rms_out={rms_out:.4} should be > 1.1 × rms_in={rms_in:.4}");
    }

    #[test]
    fn bypass_passes_through() {
        let bands = vec![EqBand { enabled: true, filter_type: EqFilterType::Peak,
                                  freq: 1000.0, gain_db: 12.0, q: 1.0 }];
        let mut eq = ParametricEq::new(&bands);
        eq.set_bypass(true);
        let input: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect();
        let output = eq.process(&input, 44100);
        assert_eq!(input, output, "bypass should return input unchanged");
    }

    #[test]
    fn update_bands_preserves_state_on_freq_change() {
        let band = EqBand { enabled: true, filter_type: EqFilterType::Peak,
                            freq: 1000.0, gain_db: 6.0, q: 1.0 };
        let mut eq = ParametricEq::new(&[band]);
        // Warm up state
        let tone = vec![0.5f32; 512];
        let _ = eq.process(&tone, 44100);
        let z1l_before = eq.filters[0].z1l;
        assert_ne!(z1l_before, 0.0, "state should be non-zero after processing");

        // Change freq only (same type) → state preserved
        let updated = EqBand { freq: 2000.0, ..eq.filters[0].band.clone() };
        eq.update_bands(&[updated]);
        assert_eq!(eq.filters[0].z1l, z1l_before, "state should survive freq nudge");
    }

    #[test]
    fn update_bands_resets_state_on_type_change() {
        let band = EqBand { enabled: true, filter_type: EqFilterType::Peak,
                            freq: 1000.0, gain_db: 6.0, q: 1.0 };
        let mut eq = ParametricEq::new(&[band]);
        let tone = vec![0.5f32; 512];
        let _ = eq.process(&tone, 44100);
        assert_ne!(eq.filters[0].z1l, 0.0);

        let changed = EqBand { filter_type: EqFilterType::LowPass,
                               ..eq.filters[0].band.clone() };
        eq.update_bands(&[changed]);
        assert_eq!(eq.filters[0].z1l, 0.0, "state must reset on type change");
    }

    #[test]
    fn update_bands_drops_removed() {
        let bands: Vec<EqBand> = (0..3).map(|i| EqBand {
            enabled: true, filter_type: EqFilterType::Peak,
            freq: 500.0 * (i + 1) as f32, gain_db: 3.0, q: 1.0,
        }).collect();
        let mut eq = ParametricEq::new(&bands);
        assert_eq!(eq.filters.len(), 3);
        eq.update_bands(&bands[..1]);
        assert_eq!(eq.filters.len(), 1, "removed bands must be dropped");
    }

    #[test]
    fn update_bands_truncates_at_10() {
        let bands: Vec<EqBand> = (0..12).map(|_| EqBand::default()).collect();
        let eq = ParametricEq::new(&bands);
        assert_eq!(eq.filters.len(), 10, "must not exceed 10 bands");
    }
```

- [ ] **Step 2: Run tests — expect FAIL (ParametricEq undefined)**

```bash
cd runtime && cargo test dsp::eq::tests::parametric -- --test-threads=1 2>&1 | grep -E "error|FAILED"
```

- [ ] **Step 3: Implement ParametricEq**

Add to `eq.rs` after `BiquadFilter`:

```rust
// ── ParametricEq ──────────────────────────────────────────────────────────

/// Multi-band parametric equalizer (max 10 biquad bands).
/// The caller (DspPipeline) owns the update path; no config arc stored here.
pub struct ParametricEq {
    pub filters: Vec<BiquadFilter>,
    enabled:     bool,
    bypass:      bool,
}

impl ParametricEq {
    /// Construct with initial band list. Enabled by default, bypass off.
    pub fn new(bands: &[EqBand]) -> Self {
        let mut eq = Self { filters: Vec::new(), enabled: true, bypass: false };
        eq.update_bands(bands);
        eq
    }

    pub fn set_enabled(&mut self, v: bool) { self.enabled = v; }
    pub fn set_bypass(&mut self, v: bool)  { self.bypass = v; }

    pub fn is_enabled(&self) -> bool {
        self.enabled && !self.bypass && !self.filters.is_empty()
    }

    /// Process stereo-interleaved samples through all active filters in sequence.
    /// Returns input unchanged (no state touched) when not enabled.
    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if !self.is_enabled() {
            return samples.to_vec();
        }
        let mut buf = samples.to_vec();
        for f in &mut self.filters {
            if f.band.enabled {
                buf = f.process_stereo(&buf, sample_rate);
            }
        }
        buf
    }

    /// Rebuild filter list from a new band configuration.
    ///
    /// State preservation rules:
    /// - Same index AND same filter_type → copy z1l/z2l/z1r/z2r (avoids clicks on nudge).
    /// - Type change or new index → reset state to zero.
    /// - Filters beyond `bands.len()` are dropped.
    /// - Bands beyond 10 are truncated with a warning.
    pub fn update_bands(&mut self, bands: &[EqBand]) {
        let bands = if bands.len() > 10 {
            tracing::warn!(count = bands.len(), "eq: more than 10 bands; truncating to 10");
            &bands[..10]
        } else {
            bands
        };
        // Clamp parameters defensively
        let clamped: Vec<EqBand> = bands.iter().map(|b| EqBand {
            freq:    b.freq.clamp(20.0, 20000.0),
            gain_db: b.gain_db.clamp(-20.0, 20.0),
            q:       b.q.clamp(0.1, 10.0),
            ..b.clone()
        }).collect();

        // Use default sample_rate=44100 for initial coefficient computation;
        // will be recomputed on first process() call if different.
        let default_sr = self.filters.first().map(|f| f.sample_rate).unwrap_or(44100);

        let new_filters: Vec<BiquadFilter> = clamped.iter().enumerate().map(|(i, band)| {
            let mut f = BiquadFilter::new(band.clone(), default_sr);
            // Preserve state if same index and same type
            if let Some(old) = self.filters.get(i) {
                if old.band.filter_type == band.filter_type {
                    f.z1l = old.z1l; f.z2l = old.z2l;
                    f.z1r = old.z1r; f.z2r = old.z2r;
                }
            }
            f
        }).collect();

        self.filters = new_filters;
    }
}
```

- [ ] **Step 4: Run tests — expect PASS**

```bash
cd runtime && cargo test dsp::eq::tests -- --test-threads=1 2>&1 | grep -E "FAILED|ok"
```

Expected: `test result: ok. 12 passed`

- [ ] **Step 5: Commit**

```bash
git add runtime/src/dsp/eq.rs
git commit -m "feat(eq): implement ParametricEq with cascade, state preservation, and truncation"
```

---

### Task 4: Wire ParametricEq into DspPipeline

**Files:**
- Modify: `runtime/src/dsp/mod.rs`

- [ ] **Step 1: Write the failing integration tests**

Add a `#[cfg(test)]` test module at the bottom of the `mod pipeline { ... }` block in `mod.rs`:

```rust
    #[cfg(test)]
    mod pipeline_eq_tests {
        use super::*;
        use crate::dsp::config::{DspConfig, EqBand, EqFilterType};
        use std::sync::{Arc, Mutex};

        /// Test helper: records which stages ran in order.
        #[derive(Clone, Default)]
        pub struct StageLog(pub Arc<Mutex<Vec<String>>>);

        impl StageLog {
            pub fn record(&self, name: &str) {
                self.0.lock().unwrap().push(name.to_string());
            }
            pub fn entries(&self) -> Vec<String> {
                self.0.lock().unwrap().clone()
            }
        }

        fn eq_band() -> EqBand {
            EqBand { enabled: true, filter_type: EqFilterType::Peak,
                     freq: 1000.0, gain_db: 6.0, q: 1.0 }
        }

        #[test]
        fn eq_runs_before_resample_when_convolution_disabled() {
            let cfg = DspConfig {
                enabled: true,
                eq_enabled: true,
                eq_bands: vec![eq_band()],
                resample_enabled: true,
                convolution_enabled: false,
                ..DspConfig::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let log = StageLog::default();
            let samples = vec![0.0f32; 256];
            pipeline.process_with_log(&mut samples.clone(), 44100, &log);
            let entries = log.entries();
            let eq_pos    = entries.iter().position(|s| s == "eq").unwrap_or(usize::MAX);
            let resamp_pos = entries.iter().position(|s| s == "resample").unwrap_or(usize::MAX);
            assert!(eq_pos < resamp_pos,
                "eq must run before resample; log={entries:?}");
        }

        #[test]
        fn eq_runs_after_convolution_when_enabled() {
            let cfg = DspConfig {
                enabled: true,
                eq_enabled: true,
                eq_bands: vec![eq_band()],
                convolution_enabled: true,
                ..DspConfig::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let log = StageLog::default();
            let mut samples = vec![0.0f32; 256];
            pipeline.process_with_log(&mut samples, 44100, &log);
            let entries = log.entries();
            let conv_pos = entries.iter().position(|s| s == "convolution").unwrap_or(usize::MAX);
            let eq_pos   = entries.iter().position(|s| s == "eq").unwrap_or(usize::MAX);
            assert!(conv_pos < eq_pos,
                "eq must run after convolution; log={entries:?}");
        }
    }
```

- [ ] **Step 2: Run tests — expect FAIL**

```bash
cd runtime && cargo test pipeline_eq_tests -- --test-threads=1 2>&1 | grep -E "error|FAILED"
```

Expected: compile error (`process_with_log` undefined, `eq` field missing)

- [ ] **Step 3: Add ParametricEq to DspPipeline struct and new()**

In `mod pipeline { ... }` in `mod.rs`, add imports:

```rust
use super::eq::ParametricEq;
```

Add field to `DspPipeline`:

```rust
        eq: Option<ParametricEq>,
```

In `DspPipeline::new()`, after `let convolution = ...`:

```rust
            let eq = if config_snap.eq_enabled && !config_snap.eq_bands.is_empty() {
                let mut peq = ParametricEq::new(&config_snap.eq_bands);
                peq.set_bypass(config_snap.eq_bypass);
                Some(peq)
            } else {
                None
            };
```

Add `eq,` to the `Self { ... }` struct initialiser.

Update the `info!` log macro to include `eq = eq.is_some()`.

- [ ] **Step 4: Update process() to place EQ dynamically**

Replace the body of `DspPipeline::process()` with the updated version that implements dynamic EQ position. The key rule: EQ runs after convolution if convolution is enabled; otherwise before resample; otherwise after DSD→PCM if only that is enabled.

```rust
        pub fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> (Vec<f32>, u32) {
            let mut input = samples.to_vec();
            let mut output_rate = sample_rate;
            let config = self.config.blocking_read();

            let eq_after_conv = config.convolution_enabled;
            let eq_before_resamp = !eq_after_conv && config.resample_enabled;
            let eq_after_dsd = !eq_after_conv && !config.resample_enabled && config.dsd_to_pcm_enabled;
            // If none of the above, EQ is the only stage (runs at the end / start)

            macro_rules! run_eq {
                () => {
                    if let Some(ref mut eq) = self.eq {
                        if eq.is_enabled() {
                            input = eq.process(&input, output_rate);
                            debug!("eq applied");
                        }
                    }
                }
            }

            if eq_before_resamp { run_eq!(); }

            if let Some(ref mut resampler) = self.resampler {
                if config.resample_enabled {
                    input = resampler.process(&input, sample_rate);
                    if !input.is_empty() {
                        output_rate = resampler.output_rate();
                        debug!(input_rate = sample_rate, output_rate, "resampled");
                    }
                }
            }

            if let Some(ref mut dsd_converter) = self.dsd_converter {
                if config.dsd_to_pcm_enabled {
                    input = dsd_converter.convert(&input);
                    if !input.is_empty() {
                        output_rate = 352800;
                        debug!(output_rate, "DSD converted");
                    }
                }
            }

            if eq_after_dsd { run_eq!(); }

            if let Some(ref mut convolution) = self.convolution {
                if convolution.is_enabled() {
                    input = convolution.process(&input);
                    debug!("convolution applied");
                }
            }

            if eq_after_conv { run_eq!(); }

            // If no other stage was active, EQ still runs (covers "all disabled" case)
            if !eq_before_resamp && !eq_after_dsd && !eq_after_conv { run_eq!(); }

            if let Some(ref mut out) = self.output {
                if let Err(e) = out.write(&input) {
                    warn!(error = %e, "audio output write failed");
                }
            }

            (input, output_rate)
        }
```

- [ ] **Step 5: Add update_eq_config to update_config()**

In `update_config()`, after the output-change detection block, add:

```rust
            // Update EQ state
            if let Some(ref mut eq) = self.eq {
                eq.set_enabled(new_cfg.eq_enabled);
                eq.set_bypass(new_cfg.eq_bypass);
                eq.update_bands(&new_cfg.eq_bands);
            } else if new_cfg.eq_enabled && !new_cfg.eq_bands.is_empty() {
                let mut peq = ParametricEq::new(&new_cfg.eq_bands);
                peq.set_bypass(new_cfg.eq_bypass);
                self.eq = Some(peq);
            }
```

- [ ] **Step 6: Add #[cfg(test)] process_with_log**

Add after the `process()` method:

```rust
        /// Test-only: like process() but appends stage names to `log` in execution order.
        #[cfg(test)]
        pub fn process_with_log(
            &mut self,
            samples: &mut [f32],
            sample_rate: u32,
            log: &pipeline_eq_tests::StageLog,
        ) -> (Vec<f32>, u32) {
            let mut input = samples.to_vec();
            let mut output_rate = sample_rate;
            let config = self.config.blocking_read();

            let eq_after_conv   = config.convolution_enabled;
            let eq_before_resamp = !eq_after_conv && config.resample_enabled;
            let eq_after_dsd    = !eq_after_conv && !config.resample_enabled && config.dsd_to_pcm_enabled;

            macro_rules! log_eq {
                () => {
                    if let Some(ref mut eq) = self.eq {
                        if eq.is_enabled() {
                            log.record("eq");
                            input = eq.process(&input, output_rate);
                        }
                    }
                }
            }

            if eq_before_resamp { log_eq!(); }

            if let Some(ref mut resampler) = self.resampler {
                if config.resample_enabled {
                    log.record("resample");
                    input = resampler.process(&input, sample_rate);
                    if !input.is_empty() { output_rate = resampler.output_rate(); }
                }
            }

            if let Some(ref mut dsd) = self.dsd_converter {
                if config.dsd_to_pcm_enabled {
                    log.record("dsd");
                    input = dsd.convert(&input);
                }
            }

            if eq_after_dsd { log_eq!(); }

            if let Some(ref mut conv) = self.convolution {
                if conv.is_enabled() {
                    log.record("convolution");
                    input = conv.process(&input);
                }
            }

            if eq_after_conv { log_eq!(); }
            if !eq_before_resamp && !eq_after_dsd && !eq_after_conv { log_eq!(); }

            (input, output_rate)
        }
```

- [ ] **Step 7: Also export ParametricEq from dsp mod**

In the `pub use` block at the top of `mod.rs`, add:

```rust
pub use eq::ParametricEq;
```

- [ ] **Step 8: Run all DSP tests — expect PASS**

```bash
cd runtime && cargo test --release -- --test-threads=1 2>&1 | grep -E "^test result|FAILED"
```

Expected: all pass

- [ ] **Step 9: Commit**

```bash
git add runtime/src/dsp/mod.rs
git commit -m "feat(eq): wire ParametricEq into DspPipeline with dynamic position"
```

---

### Task 5: Add EQ config manager keys

**Files:**
- Modify: `runtime/src/config/manager.rs`

- [ ] **Step 1: Write the failing test**

Find the existing `apply_dsp_key` tests in `manager.rs`. Add:

```rust
    #[test]
    fn dsp_eq_keys() {
        use crate::dsp::config::EqFilterType;
        let mut cfg = RuntimeConfig::default();

        apply_setting(&mut cfg, "dsp.eq_enabled", &Value::Bool(true)).unwrap();
        assert!(cfg.dsp.eq_enabled);

        apply_setting(&mut cfg, "dsp.eq_bypass", &Value::Bool(true)).unwrap();
        assert!(cfg.dsp.eq_bypass);

        let bands_json = r#"[{"enabled":true,"filter_type":"peak","freq":1000.0,"gain_db":3.0,"q":1.0}]"#;
        apply_setting(&mut cfg, "dsp.eq_bands", &Value::String(bands_json.to_string())).unwrap();
        assert_eq!(cfg.dsp.eq_bands.len(), 1);
        assert_eq!(cfg.dsp.eq_bands[0].filter_type, EqFilterType::Peak);
        assert!((cfg.dsp.eq_bands[0].freq - 1000.0).abs() < 0.01);
    }
```

- [ ] **Step 2: Run test — expect FAIL**

```bash
cd runtime && cargo test dsp_eq_keys -- --test-threads=1 2>&1 | grep -E "FAILED|error"
```

- [ ] **Step 3: Add eq key handlers to apply_dsp_key**

In `apply_dsp_key` in `manager.rs`, add these match arms (after the `convolution_bypass` arm):

```rust
        "eq_enabled" => cfg.dsp.eq_enabled = as_bool(key, value)?,
        "eq_bypass"  => cfg.dsp.eq_bypass  = as_bool(key, value)?,
        "eq_bands"   => {
            let s = as_string(key, value)?;
            let bands: Vec<crate::dsp::config::EqBand> =
                serde_json::from_str(&s).map_err(|e| {
                    StuidError::config(format!("dsp.eq_bands invalid JSON: {e}"))
                })?;
            // Clamp all fields
            cfg.dsp.eq_bands = bands.into_iter().map(|b| crate::dsp::config::EqBand {
                freq:    b.freq.clamp(20.0, 20000.0),
                gain_db: b.gain_db.clamp(-20.0, 20.0),
                q:       b.q.clamp(0.1, 10.0),
                ..b
            }).collect();
        }
```

Also add `serde_json` to `Cargo.toml` if not already present (check with `grep serde_json runtime/Cargo.toml`).

- [ ] **Step 4: Run test — expect PASS**

```bash
cd runtime && cargo test dsp_eq_keys -- --test-threads=1 2>&1 | grep -E "FAILED|ok"
```

- [ ] **Step 5: Run full suite**

```bash
cd runtime && cargo test --release -- --test-threads=1 2>&1 | grep -E "^test result|FAILED"
```

- [ ] **Step 6: Commit**

```bash
git add runtime/src/config/manager.rs runtime/Cargo.toml
git commit -m "feat(eq): add dsp.eq_enabled, dsp.eq_bypass, dsp.eq_bands config keys"
```

---

## Chunk 2: TUI Frontend

### Task 6: Implement EqEditorModel

**Files:**
- Create: `tui/internal/ui/screens/eq_editor.go`
- Create: `tui/internal/ui/screens/eq_editor_test.go`

- [ ] **Step 1: Write the failing tests**

Create `tui/internal/ui/screens/eq_editor_test.go`:

```go
package screens_test

import (
    "bytes"
    "fmt"
    "os"
    "path/filepath"
    "strings"
    "testing"

    "github.com/stui/stui/internal/ui/screens"
)

func TestCurveFlat(t *testing.T) {
    // No active bands → curve should be all at 0dB (centre row)
    bands := []screens.EqBand{}
    row := screens.ComputeCurveRow(bands, 44100.0, 60, 10, 0) // col=0, totalCols=60, height=10
    // Centre row index = height/2 - 1 for 0dB
    centre := 10/2 - 1
    if row != centre {
        t.Errorf("flat curve: col 0 row=%d, want %d", row, centre)
    }
}

func TestCurvePeakIsHighest(t *testing.T) {
    // Single +12dB peak at 1kHz; the column near 1kHz should be at or above centre
    bands := []screens.EqBand{{
        Enabled: true, FilterType: screens.EqFilterTypePeak,
        Freq: 1000.0, GainDB: 12.0, Q: 1.0,
    }}
    width := 120
    sampleRate := 44100.0
    // Find column for 1kHz on log scale: col = log(1000/20)/log(20000/20) * width
    var maxRow, maxCol int
    for col := 0; col < width; col++ {
        row := screens.ComputeCurveRow(bands, sampleRate, width, 20, col)
        if row < maxRow || col == 0 {
            maxRow = row
            maxCol = col
        }
    }
    _ = maxCol
    // maxRow should be above centre (smaller row index = higher on screen = more boost).
    // ComputeCurveRow maps 0dB to int((1.0 - (0+20)/40.0) * float64(height-1)) = int(0.5 * 19) = 9.
    centre := (20 - 1) / 2 // = 9, matching ComputeCurveRow's 0dB row for height=20
    if maxRow >= centre {
        t.Errorf("peak curve: maxRow=%d should be < centre=%d", maxRow, centre)
    }
}

func TestEditorView_ContainsBands(t *testing.T) {
    m := screens.NewEqEditorModel(nil, 44100.0)
    m.SetSize(120, 40)
    m.AddBand(screens.EqBand{
        Enabled: true, FilterType: screens.EqFilterTypePeak,
        Freq: 1000.0, GainDB: 3.0, Q: 1.0,
    })
    view := m.View()
    s := view.String()
    if !strings.Contains(s, "Peak") {
        t.Errorf("view should contain 'Peak', got:\n%s", s)
    }
    if !strings.Contains(s, "1000") {
        t.Errorf("view should contain '1000', got:\n%s", s)
    }
}
```

- [ ] **Step 2: Run tests — expect FAIL**

```bash
cd tui && go test ./internal/ui/screens/... 2>&1 | grep -E "FAIL|undefined"
```

Expected: undefined: `screens.EqBand`, `screens.ComputeCurveRow`, `screens.NewEqEditorModel`

- [ ] **Step 3: Implement EqBand types and coefficient helpers**

Create `tui/internal/ui/screens/eq_editor.go`. Start with the data types and coefficient computation:

```go
package screens

import (
    "encoding/json"
    "fmt"
    "math"
    "strings"
    "unicode/utf8"

    tea "charm.land/bubbletea/v2"
    "charm.land/lipgloss/v2"
    "github.com/stui/stui/internal/ui/screen"
    "github.com/stui/stui/pkg/theme"
)

// ── EQ types (mirror runtime/src/dsp/config.rs) ───────────────────────────

type EqFilterType string

const (
    EqFilterTypePeak      EqFilterType = "peak"
    EqFilterTypeLowShelf  EqFilterType = "low_shelf"
    EqFilterTypeHighShelf EqFilterType = "high_shelf"
    EqFilterTypeLowPass   EqFilterType = "low_pass"
    EqFilterTypeHighPass  EqFilterType = "high_pass"
    EqFilterTypeNotch     EqFilterType = "notch"
)

var eqFilterTypes = []EqFilterType{
    EqFilterTypePeak, EqFilterTypeLowShelf, EqFilterTypeHighShelf,
    EqFilterTypeLowPass, EqFilterTypeHighPass, EqFilterTypeNotch,
}

func (f EqFilterType) String() string {
    switch f {
    case EqFilterTypePeak:      return "Peak"
    case EqFilterTypeLowShelf:  return "LowShelf"
    case EqFilterTypeHighShelf: return "HighShelf"
    case EqFilterTypeLowPass:   return "LowPass"
    case EqFilterTypeHighPass:  return "HighPass"
    case EqFilterTypeNotch:     return "Notch"
    }
    return string(f)
}

// hasGain returns false for filter types where gain is not applicable.
func (f EqFilterType) hasGain() bool {
    return f != EqFilterTypeLowPass && f != EqFilterTypeHighPass && f != EqFilterTypeNotch
}

// EqBand represents a single parametric EQ band.
type EqBand struct {
    Enabled    bool         `json:"enabled"`
    FilterType EqFilterType `json:"filter_type"`
    Freq       float64      `json:"freq"`
    GainDB     float64      `json:"gain_db"`
    Q          float64      `json:"q"`
}

// ── Biquad coefficient + magnitude computation ────────────────────────────

type biquadCoeffs struct{ b0, b1, b2, a1, a2 float64 }

func computeBiquadCoeffs(band EqBand, sampleRate float64) biquadCoeffs {
    w0    := 2 * math.Pi * band.Freq / sampleRate
    sinW  := math.Sin(w0)
    cosW  := math.Cos(w0)
    alpha := sinW / (2 * band.Q)

    var b0, b1, b2, a0, a1, a2 float64
    switch band.FilterType {
    case EqFilterTypePeak:
        a := math.Pow(10, band.GainDB/40)
        b0 = 1 + alpha*a
        b1 = -2 * cosW
        b2 = 1 - alpha*a
        a0 = 1 + alpha/a
        a1 = -2 * cosW
        a2 = 1 - alpha/a
    case EqFilterTypeLowShelf:
        a       := math.Pow(10, band.GainDB/40)
        sqrtA   := math.Sqrt(a)
        alphaS  := sinW / 2 * math.Sqrt((a+1/a)*(1/band.Q-1)+2)
        b0 = a * ((a + 1) - (a-1)*cosW + 2*sqrtA*alphaS)
        b1 = 2 * a * ((a - 1) - (a+1)*cosW)
        b2 = a * ((a + 1) - (a-1)*cosW - 2*sqrtA*alphaS)
        a0 = (a + 1) + (a-1)*cosW + 2*sqrtA*alphaS
        a1 = -2 * ((a - 1) + (a+1)*cosW)
        a2 = (a + 1) + (a-1)*cosW - 2*sqrtA*alphaS
    case EqFilterTypeHighShelf:
        a       := math.Pow(10, band.GainDB/40)
        sqrtA   := math.Sqrt(a)
        alphaS  := sinW / 2 * math.Sqrt((a+1/a)*(1/band.Q-1)+2)
        b0 = a * ((a + 1) + (a-1)*cosW + 2*sqrtA*alphaS)
        b1 = -2 * a * ((a - 1) + (a+1)*cosW)
        b2 = a * ((a + 1) + (a-1)*cosW - 2*sqrtA*alphaS)
        a0 = (a + 1) - (a-1)*cosW + 2*sqrtA*alphaS
        a1 = 2 * ((a - 1) - (a+1)*cosW)
        a2 = (a + 1) - (a-1)*cosW - 2*sqrtA*alphaS
    case EqFilterTypeLowPass:
        b0 = (1 - cosW) / 2
        b1 = 1 - cosW
        b2 = (1 - cosW) / 2
        a0 = 1 + alpha
        a1 = -2 * cosW
        a2 = 1 - alpha
    case EqFilterTypeHighPass:
        b0 = (1 + cosW) / 2
        b1 = -(1 + cosW)
        b2 = (1 + cosW) / 2
        a0 = 1 + alpha
        a1 = -2 * cosW
        a2 = 1 - alpha
    case EqFilterTypeNotch:
        b0 = 1
        b1 = -2 * cosW
        b2 = 1
        a0 = 1 + alpha
        a1 = -2 * cosW
        a2 = 1 - alpha
    }
    return biquadCoeffs{b0/a0, b1/a0, b2/a0, a1/a0, a2/a0}
}

func biquadMagnitudeDB(c biquadCoeffs, omega float64) float64 {
    cos1, sin1 := math.Cos(omega), math.Sin(omega)
    cos2, sin2 := math.Cos(2*omega), math.Sin(2*omega)
    numRe := c.b0 + c.b1*cos1 + c.b2*cos2
    numIm :=        c.b1*sin1 + c.b2*sin2
    denRe := 1.0  + c.a1*cos1 + c.a2*cos2
    denIm :=        c.a1*sin1 + c.a2*sin2
    ratio := (numRe*numRe + numIm*numIm) / (denRe*denRe + denIm*denIm)
    if ratio <= 0 { return -100.0 }
    return 20 * math.Log10(math.Sqrt(ratio))
}

// combinedMagnitudeDB sums dB contributions of all enabled bands at freq.
func combinedMagnitudeDB(bands []EqBand, sampleRate, freqHz float64) float64 {
    omega := 2 * math.Pi * freqHz / sampleRate
    total := 0.0
    for _, b := range bands {
        if !b.Enabled { continue }
        c := computeBiquadCoeffs(b, sampleRate)
        total += biquadMagnitudeDB(c, omega)
    }
    return total
}

// ComputeCurveRow maps a frequency column to a terminal row for the braille curve.
// Returns the 0-indexed row (0 = top). col is 0-indexed within totalCols.
// height is the curve zone height in braille cells (each = 4 subpixels tall).
func ComputeCurveRow(bands []EqBand, sampleRate float64, totalCols, height, col int) int {
    // Map column to frequency (log scale)
    t := float64(col) / float64(totalCols-1)
    freq := 20.0 * math.Pow(1000.0, t) // 20Hz at t=0, 20000Hz at t=1
    db := combinedMagnitudeDB(bands, sampleRate, freq)
    db = math.Max(-20, math.Min(20, db))
    // Map +20dB → row 0, -20dB → row height-1
    row := int((1.0 - (db+20)/40.0) * float64(height-1))
    return row
}
```

- [ ] **Step 4: Implement braille renderer**

Continue in `eq_editor.go`:

```go
// ── Braille renderer ──────────────────────────────────────────────────────

// Braille Unicode 2×4 subpixel bit positions:
//   col%2=0: bits 0(row0), 1(row1), 2(row2), 6(row3)
//   col%2=1: bits 3(row0), 4(row1), 5(row2), 7(row3)
var brailleBit = [2][4]byte{
    {0, 1, 2, 6},
    {3, 4, 5, 7},
}

// renderBrailleCurve renders the frequency response curve into a string of
// braille characters. width and height are in terminal cells (each cell = 2×4 subpixels).
func renderBrailleCurve(bands []EqBand, sampleRate float64, width, height int) string {
    // cells[row][col] accumulates subpixel bits
    cells := make([][]byte, height)
    for i := range cells {
        cells[i] = make([]byte, width)
    }

    // Number of frequency samples = 2 * width (matching braille subpixel columns)
    nSamples := 2 * width
    // Total subpixel rows = 4 * height
    nRows := 4 * height
    // Centre subpixel row = nRows/2 (0dB line)
    centreSubRow := nRows / 2

    for px := 0; px < nSamples; px++ {
        t := float64(px) / float64(nSamples-1)
        freq := 20.0 * math.Pow(1000.0, t)
        db := combinedMagnitudeDB(bands, sampleRate, freq)
        db = math.Max(-20, math.Min(20, db))
        // Map dB to subpixel row: +20dB → 0, -20dB → nRows-1
        subRow := int((1.0 - (db+20)/40.0) * float64(nRows-1))
        // Map subpixel to cell
        cellCol := px / 2
        cellRow := subRow / 4
        if cellCol >= width || cellRow >= height { continue }
        bitIdx := brailleBit[px%2][subRow%4]
        cells[cellRow][cellCol] |= 1 << bitIdx
    }

    // Render 0dB reference line (overwrites with ─)
    refCellRow := centreSubRow / 4

    var sb strings.Builder
    for r := 0; r < height; r++ {
        if r > 0 { sb.WriteByte('\n') }
        for c := 0; c < width; c++ {
            if r == refCellRow && cells[r][c] == 0 {
                sb.WriteRune('─')
            } else {
                sb.WriteRune(rune(0x2800 + int(cells[r][c])))
            }
        }
    }
    return sb.String()
}
```

- [ ] **Step 5: Implement EqEditorModel (Bubbletea model)**

Continue in `eq_editor.go`. Add the model struct and Bubbletea interface:

```go
// ── EqEditorModel ─────────────────────────────────────────────────────────

// eqField enumerates which column is active in the band table.
type eqField int

const (
    eqFieldType eqField = iota
    eqFieldFreq
    eqFieldGain
    eqFieldQ
)

// EqEditorModel is the full-screen parametric EQ editor screen.
// Implements screen.Screen.
type EqEditorModel struct {
    bands      []EqBand
    cursor     int     // selected band row
    field      eqField // active column
    editing    bool    // inline text input active
    editBuf    string  // current text input buffer
    enabled    bool
    bypass     bool
    sampleRate float64
    width      int
    height     int
    sendFn     func(key string, value interface{}) tea.Cmd // nil-safe
}

// NewEqEditorModel constructs the editor.
// sendFn is called to emit SettingsChangedMsg commands; pass nil in tests.
func NewEqEditorModel(sendFn func(string, interface{}) tea.Cmd, sampleRate float64) *EqEditorModel {
    if sampleRate <= 0 { sampleRate = 44100 }
    return &EqEditorModel{
        bands:      nil,
        enabled:    true,
        sampleRate: sampleRate,
        sendFn:     sendFn,
    }
}

func (m *EqEditorModel) SetSize(w, h int) { m.width = w; m.height = h }

func (m *EqEditorModel) AddBand(b EqBand) { m.bands = append(m.bands, b) }

func (m *EqEditorModel) Init() tea.Cmd { return nil }

func (m *EqEditorModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
    switch msg := msg.(type) {
    case tea.KeyMsg:
        return m.handleKey(msg)
    case tea.WindowSizeMsg:
        m.width = msg.Width
        m.height = msg.Height
    }
    return m, nil
}

func (m *EqEditorModel) handleKey(msg tea.KeyMsg) (screen.Screen, tea.Cmd) {
    if m.editing {
        return m.handleEditKey(msg)
    }
    switch msg.String() {
    case "q", "esc":
        return m, tea.Batch(m.commitBands(), func() tea.Msg { return screen.PopMsg{} })
    case "a":
        if len(m.bands) < 10 {
            m.bands = append(m.bands, EqBand{
                Enabled: true, FilterType: EqFilterTypePeak,
                Freq: 1000, GainDB: 0, Q: 1.0,
            })
            m.cursor = len(m.bands) - 1
        }
    case "d":
        if len(m.bands) > 0 {
            m.bands = append(m.bands[:m.cursor], m.bands[m.cursor+1:]...)
            if m.cursor >= len(m.bands) && m.cursor > 0 { m.cursor-- }
            return m, m.commitBands()
        }
    case " ":
        if len(m.bands) > 0 {
            m.bands[m.cursor].Enabled = !m.bands[m.cursor].Enabled
            return m, m.commitBands()
        }
    case "tab":
        m.cursor = (m.cursor + 1) % max(1, len(m.bands))
        return m, m.commitBands()
    case "shift+tab":
        if len(m.bands) > 0 {
            m.cursor = (m.cursor - 1 + len(m.bands)) % len(m.bands)
        }
        return m, m.commitBands()
    case "left":
        m.field = eqField((int(m.field) - 1 + 4) % 4)
        m.skipGainIfNotApplicable(-1)
    case "right":
        m.field = eqField((int(m.field) + 1) % 4)
        m.skipGainIfNotApplicable(+1)
    case "+", "=":
        m.nudge(+1)
    case "-", "_":
        m.nudge(-1)
    case "e":
        if len(m.bands) > 0 && !(m.field == eqFieldGain && !m.bands[m.cursor].FilterType.hasGain()) {
            m.editing = true
            m.editBuf = m.fieldValueString()
        }
    case "b":
        m.bypass = !m.bypass
        return m, m.commitBands()
    }
    return m, nil
}

func (m *EqEditorModel) skipGainIfNotApplicable(dir int) {
    if len(m.bands) == 0 { return }
    if m.field == eqFieldGain && !m.bands[m.cursor].FilterType.hasGain() {
        m.field = eqField((int(m.field) + dir + 4) % 4)
    }
}

func (m *EqEditorModel) nudge(dir int) {
    if len(m.bands) == 0 { return }
    b := &m.bands[m.cursor]
    switch m.field {
    case eqFieldType:
        idx := 0
        for i, t := range eqFilterTypes { if t == b.FilterType { idx = i; break } }
        idx = (idx + dir + len(eqFilterTypes)) % len(eqFilterTypes)
        b.FilterType = eqFilterTypes[idx]
    case eqFieldFreq:
        if dir > 0 { b.Freq *= 1.05 } else { b.Freq /= 1.05 }
        b.Freq = math.Max(20, math.Min(20000, b.Freq))
    case eqFieldGain:
        if b.FilterType.hasGain() {
            b.GainDB = math.Max(-20, math.Min(20, b.GainDB+float64(dir)*0.5))
        }
    case eqFieldQ:
        b.Q = math.Max(0.1, math.Min(10.0, b.Q+float64(dir)*0.05))
    }
}

func (m *EqEditorModel) fieldValueString() string {
    if len(m.bands) == 0 { return "" }
    b := m.bands[m.cursor]
    switch m.field {
    case eqFieldType: return string(b.FilterType)
    case eqFieldFreq: return fmt.Sprintf("%.0f", b.Freq)
    case eqFieldGain: return fmt.Sprintf("%.1f", b.GainDB)
    case eqFieldQ:    return fmt.Sprintf("%.2f", b.Q)
    }
    return ""
}

func (m *EqEditorModel) handleEditKey(msg tea.KeyMsg) (screen.Screen, tea.Cmd) {
    switch msg.String() {
    case "esc":
        m.editing = false
        m.editBuf = ""
    case "enter":
        m.commitEdit()
        m.editing = false
        m.editBuf = ""
        return m, m.commitBands()
    case "backspace":
        if len(m.editBuf) > 0 {
            _, sz := utf8.DecodeLastRuneInString(m.editBuf)
            m.editBuf = m.editBuf[:len(m.editBuf)-sz]
        }
    default:
        // tea.KeyMsg is an interface; msg.String() returns the key text (e.g. "3", ".", "-").
        s := msg.String()
        if len(s) == 1 && (s[0] >= '0' && s[0] <= '9' || s[0] == '.' || s[0] == '-') {
            m.editBuf += s
        }
    }
    return m, nil
}

func (m *EqEditorModel) commitEdit() {
    if len(m.bands) == 0 || m.editBuf == "" { return }
    b := &m.bands[m.cursor]
    var v float64
    if _, err := fmt.Sscanf(m.editBuf, "%f", &v); err != nil { return }
    switch m.field {
    case eqFieldFreq: b.Freq    = math.Max(20, math.Min(20000, v))
    case eqFieldGain: if b.FilterType.hasGain() { b.GainDB = math.Max(-20, math.Min(20, v)) }
    case eqFieldQ:    b.Q       = math.Max(0.1, math.Min(10.0, v))
    }
}

func (m *EqEditorModel) commitBands() tea.Cmd {
    if m.sendFn == nil { return nil }
    data, err := json.Marshal(m.bands)
    if err != nil { return nil }
    cmds := []tea.Cmd{
        m.sendFn("dsp.eq_bands",   string(data)),
        m.sendFn("dsp.eq_enabled", m.enabled),
        m.sendFn("dsp.eq_bypass",  m.bypass),
    }
    return tea.Batch(cmds...)
}

// Note: max() is a Go 1.21+ built-in — do NOT define it locally.
```

- [ ] **Step 6: Implement View()**

Continue in `eq_editor.go`:

```go
func (m EqEditorModel) View() tea.View {
    if m.width == 0 { return tea.NewView("  Parametric EQ\n") }

    accent  := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
    normal  := lipgloss.NewStyle().Foreground(theme.T.Text())
    dimmed  := lipgloss.NewStyle().Foreground(theme.T.Subtle())
    selected := lipgloss.NewStyle().Foreground(theme.T.Accent()).Reverse(true)

    var sb strings.Builder

    // ── Header ────────────────────────────────────────────────────────────
    ena := "enabled"
    if !m.enabled { ena = "disabled" }
    byp := "off"
    if m.bypass { byp = "on" }
    sb.WriteString(accent.Render("  Parametric EQ") + "  " +
        normal.Render("["+ena+"]") + "  " +
        normal.Render("[bypass: "+byp+"]") + "\n")
    sb.WriteString(strings.Repeat("─", m.width) + "\n")

    // ── Braille curve ─────────────────────────────────────────────────────
    curveHeight := (m.height - 10) / 4 // approx 40% of height in braille rows
    if curveHeight < 2 { curveHeight = 2 }
    curveWidth := m.width - 8 // leave room for dB labels

    activeBands := make([]EqBand, 0, len(m.bands))
    for _, b := range m.bands { if b.Enabled { activeBands = append(activeBands, b) } }

    curve := renderBrailleCurve(activeBands, m.sampleRate, curveWidth, curveHeight)
    lines := strings.Split(curve, "\n")
    for i, line := range lines {
        label := "      "
        if i == 0 { label = "+20dB " }
        if i == curveHeight/2 { label = "  0dB " }
        if i == curveHeight-1 { label = "-20dB " }
        sb.WriteString(dimmed.Render(label) + normal.Render(line) + "\n")
    }
    sb.WriteString(dimmed.Render("      ") +
        dimmed.Render("20Hz") +
        strings.Repeat(" ", (curveWidth-12)/2) +
        dimmed.Render("1kHz") +
        strings.Repeat(" ", (curveWidth-12)/2) +
        dimmed.Render("20kHz") + "\n")
    sb.WriteString(strings.Repeat("─", m.width) + "\n")

    // ── Band table ────────────────────────────────────────────────────────
    header := fmt.Sprintf("  %-3s %-3s %-10s %-9s %-7s %-6s\n",
        "#", "on", "type", "freq", "gain", "Q")
    sb.WriteString(dimmed.Render(header))

    for i, b := range m.bands {
        onStr := "✗"
        if b.Enabled { onStr = "✓" }

        gainStr := "---"
        if b.FilterType.hasGain() {
            gainStr = fmt.Sprintf("%+.1f", b.GainDB)
        }
        freqStr := fmt.Sprintf("%.0f Hz", b.Freq)
        qStr    := fmt.Sprintf("%.2f", b.Q)
        typeStr := b.FilterType.String()

        // Highlight active field on selected row
        if i == m.cursor {
            fields := []string{typeStr, freqStr, gainStr, qStr}
            for fi, fs := range fields {
                if eqField(fi) == m.field {
                    if m.editing { fields[fi] = "[" + m.editBuf + "_]" } else {
                        fields[fi] = selected.Render(fs)
                    }
                }
            }
            typeStr, freqStr, gainStr, qStr = fields[0], fields[1], fields[2], fields[3]
        }

        row := fmt.Sprintf("  %-3d %-3s %-10s %-9s %-7s %-6s\n",
            i+1, onStr, typeStr, freqStr, gainStr, qStr)

        style := normal
        if i == m.cursor { style = accent }
        sb.WriteString(style.Render(row))
    }

    if len(m.bands) == 0 {
        sb.WriteString(dimmed.Render("  (no bands — press 'a' to add)\n"))
    }

    addHint := "a add"
    if len(m.bands) >= 10 { addHint = dimmed.Render("a add") }
    sb.WriteString(strings.Repeat("─", m.width) + "\n")
    sb.WriteString(dimmed.Render(
        "  "+addHint+"  d del  space toggle  tab next  +/- nudge  e edit\n"+
        "  b bypass  q close\n"))

    return tea.NewView(sb.String())
}
```

- [ ] **Step 7: Add teatest golden file test**

Add to `eq_editor_test.go`. This test uses only stdlib (`bytes`, `fmt`, `os`, `path/filepath`) — no external test framework needed:

```go
func TestEditorView_Golden(t *testing.T) {
    // Band config mirrors the spec layout mockup (section "TUI EQ Editor").
    m := screens.NewEqEditorModel(nil, 44100.0)
    m.SetSize(120, 40)
    m.AddBand(screens.EqBand{Enabled: true,  FilterType: screens.EqFilterTypePeak,     Freq: 1000,  GainDB: 3.0, Q: 1.0})
    m.AddBand(screens.EqBand{Enabled: true,  FilterType: screens.EqFilterTypeLowShelf, Freq: 80,    GainDB: 2.0, Q: 0.71})
    m.AddBand(screens.EqBand{Enabled: false, FilterType: screens.EqFilterTypeLowPass,  Freq: 18000, GainDB: 0.0, Q: 0.71})

    view := m.View()
    got  := []byte(view.String())
    goldenFile := filepath.Join("testdata", "eq_editor_golden.txt")
    if os.Getenv("UPDATE_GOLDEN") == "1" {
        _ = os.MkdirAll("testdata", 0755)
        _ = os.WriteFile(goldenFile, got, 0644)
        t.Logf("golden file updated: %s", goldenFile)
        return
    }
    want, err := os.ReadFile(goldenFile)
    if err != nil {
        t.Fatalf("golden file missing — run with UPDATE_GOLDEN=1 to create it: %v", err)
    }
    if !bytes.Equal(got, want) {
        t.Errorf("view does not match golden file.\nRun: UPDATE_GOLDEN=1 go test ./... to regenerate.\nDiff (got vs want):\n%s",
            diffStrings(string(got), string(want)))
    }
}

// diffStrings returns a simple line-by-line diff for test output.
func diffStrings(got, want string) string {
    gotLines  := strings.Split(got,  "\n")
    wantLines := strings.Split(want, "\n")
    var sb strings.Builder
    for i := 0; i < len(gotLines) || i < len(wantLines); i++ {
        g, w := "", ""
        if i < len(gotLines)  { g = gotLines[i] }
        if i < len(wantLines) { w = wantLines[i] }
        if g != w {
            sb.WriteString(fmt.Sprintf("line %d\n  got:  %q\n  want: %q\n", i+1, g, w))
        }
    }
    return sb.String()
}
```

Generate the initial golden file:

```bash
cd tui && UPDATE_GOLDEN=1 go test ./internal/ui/screens/... -run TestEditorView_Golden -v 2>&1
```

Expected: `golden file updated: testdata/eq_editor_golden.txt`

- [ ] **Step 8: Run tests — expect PASS**

```bash
cd tui && go test ./internal/ui/screens/... -run TestCurve -v 2>&1 | grep -E "PASS|FAIL"
cd tui && go test ./internal/ui/screens/... -run TestEditorView -v 2>&1 | grep -E "PASS|FAIL"
```

Expected: `PASS`

- [ ] **Step 9: Commit**

```bash
git add tui/internal/ui/screens/eq_editor.go tui/internal/ui/screens/eq_editor_test.go tui/internal/ui/screens/testdata/eq_editor_golden.txt
git commit -m "feat(eq): implement TUI EQ editor with braille curve and band controls"
```

---

### Task 7: Wire EQ editor into settings screen

**Files:**
- Modify: `tui/internal/ui/screens/settings.go`

- [ ] **Step 1: Write the failing test**

Add to `eq_editor_test.go`:

```go
func TestSettingsHasEqEntry(t *testing.T) {
    // The settings model must contain a DSP Audio category with an EQ entry
    m := screens.NewSettingsModel()
    view := m.View()
    s := view.String()
    if !strings.Contains(s, "EQ") && !strings.Contains(s, "Equalizer") {
        t.Errorf("settings view should contain EQ entry, got:\n%s", s)
    }
}
```

- [ ] **Step 2: Run test — expect FAIL**

```bash
cd tui && go test ./internal/ui/screens/... -run TestSettingsHasEqEntry -v 2>&1 | grep -E "PASS|FAIL"
```

- [ ] **Step 3: Add EQ entry to settings.go**

In `settings.go`, inside the DSP Audio category items slice (after the convolution filter entries), add:

```go
                {
                    label:       "Parametric EQ",
                    key:         "dsp.eq_enabled",
                    kind:        settingAction,
                    description: "Open parametric EQ band editor (biquad, up to 10 bands)",
                },
```

`settings.go` already has a `case settingAction:` block (around line 373) with an inner `switch item.key { ... default: ... }`. The `default:` arm is the last case (line 389). Insert the new `case "dsp.eq_enabled":` **before the `default:` line** — Go rejects a `case` after `default` with a compile error:

```go
            // Inside the existing:  if item.kind == settingAction { switch item.key {
            // ADD before the existing `default:` arm at line 389:
            case "dsp.eq_enabled":
                // Launch full-screen EQ editor.
                // sampleRate defaults to 44100 — the actual runtime rate is not
                // available at settings-screen level; the editor recomputes correctly
                // once the pipeline passes the real rate into process().
                editor := NewEqEditorModel(func(key string, val interface{}) tea.Cmd {
                    return func() tea.Msg { return SettingsChangedMsg{Key: key, Value: val} }
                }, 44100.0)
                editor.SetSize(m.width, m.height)
                // TransitionCmd is the standard pattern; RootModel intercepts the
                // resulting screen.TransitionMsg at root.go:70 — no root changes needed.
                return m, screen.TransitionCmd(editor, true)
```

(`m.width` and `m.height` are already on `SettingsModel`; no new fields needed.)

- [ ] **Step 4: Run tests — expect PASS**

```bash
cd tui && go test ./internal/ui/screens/... 2>&1 | grep -E "^ok|FAIL"
```

- [ ] **Step 5: Verify full build**

```bash
cd tui && go build ./... 2>&1
cd ../runtime && cargo build 2>&1 | grep "^error"
```

Expected: no errors

- [ ] **Step 6: Commit**

```bash
git add tui/internal/ui/screens/settings.go tui/internal/ui/screens/eq_editor_test.go
git commit -m "feat(eq): wire EQ editor into DSP Audio settings with transition"
```

---

## Post-implementation: run full test suite

- [ ] **Run Rust tests**

```bash
cd runtime && cargo test --release -- --test-threads=1 2>&1 | grep -E "^test result|FAILED"
```

Expected: all pass, 0 failed

- [ ] **Run Go tests**

```bash
cd tui && go test ./... 2>&1 | grep -E "^ok|FAIL"
```

Expected: all pass
