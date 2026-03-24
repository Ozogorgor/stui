# Crossfeed Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add BS2B headphone crossfeed to the STUI DSP pipeline with a lipgloss dialog TUI and auto-detection of headphone devices.

**Architecture:** A first-order IIR `CrossfeedFilter` is added as the last DSP stage before `out.write()`. Four new fields on `DspConfig` (crossfeed_enabled, crossfeed_auto, crossfeed_feed_level, crossfeed_cutoff_hz) drive a `probe_headphones()` function that auto-detects headphones from the device name. A `CrossfeedDialogModel` Go screen (lipgloss-bordered, centered) is pushed from the DSP Audio settings entry via `screen.TransitionCmd`, commits all four IPC keys on close.

**Tech Stack:** Rust (std, no extra crates), Go with charm.land/bubbletea/v2 and charm.land/lipgloss/v2, existing `screen.TransitionCmd` / `SettingsChangedMsg` patterns.

**Spec:** `docs/superpowers/specs/2026-03-24-crossfeed-design.md`

---

## Chunk 1: Rust Backend

### Task 1: Add crossfeed fields to DspConfig

**Files:**
- Modify: `runtime/src/dsp/config.rs`

- [ ] **Step 1: Write the failing test**

Add to `runtime/src/dsp/config.rs` in a `#[cfg(test)]` block at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfeed_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.crossfeed_enabled);
        assert!(!cfg.crossfeed_auto);
        assert!((cfg.crossfeed_feed_level - 0.45_f32).abs() < f32::EPSILON);
        assert!((cfg.crossfeed_cutoff_hz - 700.0_f32).abs() < f32::EPSILON);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd runtime && cargo test crossfeed_defaults 2>&1 | head -20
```

Expected: compile error — fields do not exist yet.

- [ ] **Step 3: Add crossfeed fields to `DspConfig`**

In `runtime/src/dsp/config.rs`, add four fields to the end of the `DspConfig` struct (before the closing `}`):

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

Add to the `Default` impl (at the end of the `Self { ... }` block):

```rust
            crossfeed_enabled:    false,
            crossfeed_auto:       false,
            crossfeed_feed_level: 0.45,
            crossfeed_cutoff_hz:  700.0,
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cd runtime && cargo test crossfeed_defaults 2>&1
```

Expected: `test dsp::config::tests::crossfeed_defaults ... ok`

- [ ] **Step 5: Commit**

```bash
git add runtime/src/dsp/config.rs
git commit -m "feat(dsp): add crossfeed fields to DspConfig"
```

---

### Task 2: Implement CrossfeedFilter

**Files:**
- Create: `runtime/src/dsp/crossfeed.rs`
- Modify: `runtime/src/dsp/mod.rs` (add `pub mod crossfeed;`)

- [ ] **Step 1: Write the failing tests**

Create `runtime/src/dsp/crossfeed.rs` with tests only (no implementation yet):

```rust
//! BS2B headphone crossfeed filter.
//!
//! First-order IIR implementation of the Bauer stereophonic-to-binaural
//! algorithm. Blends a low-pass-filtered portion of each channel into the
//! opposite channel to reduce headphone fatigue on hard-panned content.

pub struct CrossfeedFilter {
    feed_level:  f32,
    cutoff_hz:   f32,
    sample_rate: u32,
    alpha:       f32,
    norm:        f32,
    z_l:         f32,
    z_r:         f32,
}

impl CrossfeedFilter {
    pub fn new(feed_level: f32, cutoff_hz: f32) -> Self {
        todo!()
    }

    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        todo!()
    }

    pub fn set_params(&mut self, feed_level: f32, cutoff_hz: f32) {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine_stereo(freq_hz: f32, sample_rate: u32, n_samples: usize) -> Vec<f32> {
        (0..n_samples)
            .flat_map(|i| {
                let s = (2.0 * PI * freq_hz * i as f32 / sample_rate as f32).sin();
                [s, s]
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    // All-zeros input must produce all-zeros output (no state drift or denormals).
    #[test]
    fn silence_in_silence_out() {
        let mut f = CrossfeedFilter::new(0.45, 700.0);
        let silence = vec![0.0_f32; 1024];
        let out = f.process(&silence, 44100);
        assert_eq!(out.len(), 1024);
        assert!(out.iter().all(|&s| s == 0.0), "silence in must produce silence out");
    }

    // feed_level=0.0 → output must equal input exactly.
    // norm=1.0 and crossfeed term is multiplied by 0.0, so even the 1e-25 guard
    // on z_l/z_r never reaches out_L/out_R.
    #[test]
    fn feed_zero_is_passthrough() {
        let mut f = CrossfeedFilter::new(0.0, 700.0);
        let input: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).flat_map(|v| [v, -v]).collect();
        let out = f.process(&input, 44100);
        assert_eq!(out.len(), input.len());
        for (a, b) in input.iter().zip(out.iter()) {
            assert_eq!(a, b, "feed=0 must be exact passthrough");
        }
    }

    // feed_level=0.9 with 1 kHz sine: RMS(out) / RMS(in) must be within 5% of 1.0.
    // Normalisation is exact at DC; the 5% tolerance covers above-cutoff attenuation.
    #[test]
    fn feed_max_energy_preserved() {
        let sr = 44100_u32;
        let input = sine_stereo(1000.0, sr, sr as usize); // 1 second
        let mut f = CrossfeedFilter::new(0.9, 700.0);
        let out = f.process(&input, sr);
        let ratio = rms(&out) / rms(&input);
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "energy ratio {ratio:.4} out of ±5% window"
        );
    }

    // With a 1 kHz signal and cutoff=300 Hz, the crossfeed contribution at output
    // must be smaller than the direct (unfiltered) path contribution.
    #[test]
    fn lowpass_attenuates_above_cutoff() {
        let sr = 44100_u32;
        // L=sine, R=0 so we can isolate the crossfeed term on out_L.
        let input: Vec<f32> = (0..(sr as usize))
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin();
                [s, 0.0_f32]
            })
            .collect();
        let mut f = CrossfeedFilter::new(0.5, 300.0);
        let out = f.process(&input, sr);
        // Direct path contribution on out_L = norm * in_L (approx RMS of input L)
        let direct_rms: f32 = {
            let l_in: Vec<f32> = input.iter().step_by(2).copied().collect();
            rms(&l_in)
        };
        // Crossfeed contribution on out_R = norm * feed * z_l (should be attenuated)
        let crossfeed_rms: f32 = {
            let r_out: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
            rms(&r_out)
        };
        assert!(
            crossfeed_rms < direct_rms,
            "crossfeed RMS {crossfeed_rms:.4} should be < direct RMS {direct_rms:.4} at 1kHz/300Hz cutoff"
        );
    }

    // process() at 44100 then at 96000: must not panic, state resets on rate change.
    #[test]
    fn sample_rate_change_recomputes() {
        let mut f = CrossfeedFilter::new(0.45, 700.0);
        let input = vec![0.5_f32, -0.5_f32, 0.3_f32, -0.3_f32]; // 2 stereo frames
        let _ = f.process(&input, 44100);
        // After rate change the first output frame should be close to input * norm,
        // not contaminated by old state (state resets to zero on rate change).
        let out2 = f.process(&input, 96000);
        assert_eq!(out2.len(), input.len());
        // First output sample: norm * (in_L + feed * z_r). z_r is reset to 0.
        let norm = 1.0 / (1.0 + 0.45_f32);
        let expected_l = norm * 0.5_f32; // feed * z_r = 0
        assert!(
            (out2[0] - expected_l).abs() < 1e-4,
            "first out_L after rate change: got {}, expected ~{expected_l}",
            out2[0]
        );
    }

    // Near-zero input (subnormal territory): z_l/z_r must remain normal after 10 000 frames.
    #[test]
    fn denormal_guard() {
        let mut f = CrossfeedFilter::new(0.45, 700.0);
        let near_zero = vec![1e-38_f32; 20_000]; // 10 000 stereo frames
        f.process(&near_zero, 44100);
        // Access internal state via a second call that exercises z_l/z_r;
        // the test verifies no panic and that output is finite.
        let out = f.process(&near_zero, 44100);
        assert!(out.iter().all(|s| s.is_finite()), "output must be finite after near-zero input");
    }

    // probe_headphones: ALSA keyword matching.
    #[test]
    fn probe_headphones_alsa_keywords() {
        use crate::dsp::config::{DspConfig, OutputTarget};

        let make = |device: &str| -> DspConfig {
            DspConfig {
                output_target: OutputTarget::Alsa,
                alsa_device: Some(device.to_string()),
                ..Default::default()
            }
        };

        assert!(probe_headphones(&make("hw:Headphone")),     "headphone keyword");
        assert!(probe_headphones(&make("hw:Headset,0")),     "headset keyword");
        assert!(probe_headphones(&make("hw:earphone")),      "earphone keyword");
        assert!(probe_headphones(&make("hw:HEADPHONE")),     "case-insensitive");
        assert!(!probe_headphones(&make("hw:Generic")),      "no keyword → false");
    }

    // probe_headphones: non-ALSA targets always return false.
    #[test]
    fn probe_headphones_non_alsa_returns_false() {
        use crate::dsp::config::{DspConfig, OutputTarget};

        let pipewire_music = DspConfig {
            output_target: OutputTarget::PipeWire,
            pipewire_role: "Music".to_string(),
            ..Default::default()
        };
        assert!(!probe_headphones(&pipewire_music), "PipeWire Music → false");

        let roon = DspConfig {
            output_target: OutputTarget::RoonRaat,
            ..Default::default()
        };
        assert!(!probe_headphones(&roon), "RoonRaat → false");
    }
}
```

- [ ] **Step 2: Add module declaration to mod.rs**

In `runtime/src/dsp/mod.rs`, add after the existing `pub mod convolution;` line:

```rust
pub mod crossfeed;
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cd runtime && cargo test crossfeed 2>&1 | tail -20
```

Expected: `todo!()` panics or compile errors — no implementations yet.

- [ ] **Step 4: Implement CrossfeedFilter**

Replace the `CrossfeedFilter` impl stubs in `runtime/src/dsp/crossfeed.rs`:

```rust
use std::f32::consts::PI;
use crate::dsp::config::DspConfig;

impl CrossfeedFilter {
    pub fn new(feed_level: f32, cutoff_hz: f32) -> Self {
        let mut f = Self {
            feed_level,
            cutoff_hz,
            sample_rate: 0,  // triggers recompute on first process() call
            alpha: 0.0,
            norm:  0.0,
            z_l:   0.0,
            z_r:   0.0,
        };
        f.recompute(44100); // nominal value so struct is always valid
        f
    }

    fn recompute(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.alpha = (-2.0 * PI * self.cutoff_hz / sample_rate as f32).exp();
        self.norm  = 1.0 / (1.0 + self.feed_level);
        self.z_l   = 0.0;
        self.z_r   = 0.0;
    }

    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if sample_rate != self.sample_rate {
            self.recompute(sample_rate);
        }

        let mut out = Vec::with_capacity(samples.len());
        let mut iter = samples.chunks_exact(2);
        for frame in iter.by_ref() {
            let in_l = frame[0];
            let in_r = frame[1];

            self.z_l = (1.0 - self.alpha) * in_l + self.alpha * self.z_l + 1e-25;
            self.z_r = (1.0 - self.alpha) * in_r + self.alpha * self.z_r + 1e-25;

            out.push(self.norm * (in_l + self.feed_level * self.z_r));
            out.push(self.norm * (in_r + self.feed_level * self.z_l));
        }
        // If an odd sample is present (shouldn't happen with stereo), pass it through.
        for &s in iter.remainder() {
            out.push(s);
        }
        out
    }

    pub fn set_params(&mut self, feed_level: f32, cutoff_hz: f32) {
        self.feed_level = feed_level;
        self.cutoff_hz  = cutoff_hz;
        self.recompute(self.sample_rate);
    }
}
```

Add `probe_headphones` as a free function in the same file (used by `DspPipeline`):

```rust
/// Returns true when the configured output device name contains a headphone keyword.
/// Defaults to false (crossfeed stays OFF) when no keyword is found or the target
/// is not ALSA or PipeWire.
pub(crate) fn probe_headphones(config: &DspConfig) -> bool {
    use crate::dsp::config::OutputTarget;
    let haystack = match config.output_target {
        OutputTarget::Alsa     => config.alsa_device.as_deref().unwrap_or(""),
        OutputTarget::PipeWire => &config.pipewire_role,
        _                      => return false,
    };
    let h = haystack.to_lowercase();
    h.contains("headphone") || h.contains("headset") || h.contains("earphone")
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd runtime && cargo test crossfeed 2>&1
```

Expected: all 8 crossfeed tests pass (silence_in_silence_out, feed_zero_is_passthrough, feed_max_energy_preserved, lowpass_attenuates_above_cutoff, sample_rate_change_recomputes, denormal_guard, probe_headphones_alsa_keywords, probe_headphones_non_alsa_returns_false).

- [ ] **Step 6: Commit**

```bash
git add runtime/src/dsp/crossfeed.rs runtime/src/dsp/mod.rs
git commit -m "feat(dsp): implement CrossfeedFilter and probe_headphones"
```

---

### Task 3: Wire CrossfeedFilter into DspPipeline

**Files:**
- Modify: `runtime/src/dsp/mod.rs`

- [ ] **Step 1: Write the failing test**

Add a test module inside `mod pipeline { ... }` at the bottom of `runtime/src/dsp/mod.rs`:

```rust
    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::dsp::config::{DspConfig, OutputTarget};

        #[test]
        fn crossfeed_applied_when_enabled() {
            // Hard-pan L=1.0 R=0.0 — with crossfeed the right channel must not be zero.
            let cfg = DspConfig {
                crossfeed_enabled: true,
                crossfeed_feed_level: 0.45,
                crossfeed_cutoff_hz: 700.0,
                ..Default::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let mut input = vec![1.0_f32, 0.0_f32; 64]; // L=1.0 R=0.0
            let (out, _) = pipeline.process(&mut input, 44100);
            // Right channel after crossfeed must be > 0
            let r_max = out.iter().skip(1).step_by(2).cloned().fold(0.0_f32, f32::max);
            assert!(r_max > 0.0, "right channel should have crossfeed contribution");
        }

        #[test]
        fn crossfeed_bypassed_when_disabled() {
            let cfg = DspConfig {
                crossfeed_enabled: false,
                ..Default::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let mut input: Vec<f32> = (0..64).flat_map(|_| [1.0_f32, 0.0_f32]).collect();
            let (out, _) = pipeline.process(&mut input, 44100);
            // Right channel must remain 0.0 — no crossfeed applied
            let r_max = out.iter().skip(1).step_by(2).cloned().fold(0.0_f32, f32::max);
            assert_eq!(r_max, 0.0, "right channel must be untouched when crossfeed disabled");
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd runtime && cargo test pipeline::tests 2>&1 | tail -20
```

Expected: compile errors — `crossfeed` field doesn't exist on `DspPipeline` yet.

- [ ] **Step 3: Wire CrossfeedFilter into DspPipeline**

In `runtime/src/dsp/mod.rs`, update the `use super::` block and `DspPipeline` struct:

```rust
    use super::{
        config::DspConfig,
        convolution::ConvolutionEngine,
        crossfeed::{CrossfeedFilter, probe_headphones},
        dsd::DsdConverter,
        output::{open_output, AudioOutput},
        resample::Resampler,
    };

    pub struct DspPipeline {
        config:        Arc<RwLock<DspConfig>>,
        resampler:     Option<Resampler>,
        dsd_converter: Option<DsdConverter>,
        convolution:   Option<ConvolutionEngine>,
        crossfeed:     Option<CrossfeedFilter>,
        output:        Option<Box<dyn AudioOutput>>,
    }
```

In `new()`, add crossfeed construction after the output block (before `info!(...)`):

```rust
            let crossfeed = if config_snap.crossfeed_auto {
                if probe_headphones(&config_snap) {
                    Some(CrossfeedFilter::new(
                        config_snap.crossfeed_feed_level,
                        config_snap.crossfeed_cutoff_hz,
                    ))
                } else {
                    None
                }
            } else if config_snap.crossfeed_enabled {
                Some(CrossfeedFilter::new(
                    config_snap.crossfeed_feed_level,
                    config_snap.crossfeed_cutoff_hz,
                ))
            } else {
                None
            };
```

Update the `info!()` log and `Self { ... }` block to include `crossfeed`:

```rust
            info!(
                resampler   = resampler.is_some(),
                dsd         = dsd_converter.is_some(),
                convolution = convolution.is_some(),
                crossfeed   = crossfeed.is_some(),
                output      = output.is_some(),
                "DSP pipeline initialized"
            );

            Self {
                config,
                resampler,
                dsd_converter,
                convolution,
                crossfeed,
                output,
            }
```

In `process()`, add crossfeed as the last stage before `out.write()`:

```rust
            // Replace the existing out.write block with:
            if let Some(ref mut cf) = self.crossfeed {
                input = cf.process(&input, output_rate);
                debug!("crossfeed applied");
            }

            if let Some(ref mut out) = self.output {
                if let Err(e) = out.write(&input) {
                    warn!(error = %e, "audio output write failed");
                }
            }
```

In `update_config()`, add crossfeed update logic after the `output_changed` block:

```rust
            // Recreate crossfeed when enable state, auto flag, or output device changes.
            let crossfeed_recreate = old.crossfeed_enabled  != new_cfg.crossfeed_enabled
                || old.crossfeed_auto    != new_cfg.crossfeed_auto
                || old.output_target     != new_cfg.output_target
                || old.alsa_device       != new_cfg.alsa_device
                || old.pipewire_role     != new_cfg.pipewire_role;

            let crossfeed_params_changed = old.crossfeed_feed_level != new_cfg.crossfeed_feed_level
                || old.crossfeed_cutoff_hz != new_cfg.crossfeed_cutoff_hz;

            if crossfeed_recreate {
                self.crossfeed = if new_cfg.crossfeed_auto {
                    if probe_headphones(&new_cfg) {
                        Some(CrossfeedFilter::new(
                            new_cfg.crossfeed_feed_level,
                            new_cfg.crossfeed_cutoff_hz,
                        ))
                    } else {
                        None
                    }
                } else if new_cfg.crossfeed_enabled {
                    Some(CrossfeedFilter::new(
                        new_cfg.crossfeed_feed_level,
                        new_cfg.crossfeed_cutoff_hz,
                    ))
                } else {
                    None
                };
            } else if crossfeed_params_changed {
                if let Some(ref mut cf) = self.crossfeed {
                    cf.set_params(new_cfg.crossfeed_feed_level, new_cfg.crossfeed_cutoff_hz);
                }
            }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd runtime && cargo test 2>&1 | tail -20
```

Expected: all existing tests plus `crossfeed_applied_when_enabled` and `crossfeed_bypassed_when_disabled` pass.

- [ ] **Step 5: Commit**

```bash
git add runtime/src/dsp/mod.rs
git commit -m "feat(dsp): wire CrossfeedFilter into DspPipeline as last stage"
```

---

### Task 4: Add crossfeed keys to config manager

**Files:**
- Modify: `runtime/src/config/manager.rs`

- [ ] **Step 1: Write the failing test**

Find the `#[cfg(test)]` block in `manager.rs` (search for `mod tests`). Add this test:

```rust
    #[test]
    fn dsp_crossfeed_keys() {
        use crate::config::RuntimeConfig;

        let mut cfg = RuntimeConfig::default();

        // bool keys
        apply_dsp_key(&mut cfg, "dsp.crossfeed_enabled", &serde_json::Value::Bool(true)).unwrap();
        assert!(cfg.dsp.crossfeed_enabled);

        apply_dsp_key(&mut cfg, "dsp.crossfeed_auto", &serde_json::Value::Bool(true)).unwrap();
        assert!(cfg.dsp.crossfeed_auto);

        // feed_level: valid value
        apply_dsp_key(&mut cfg, "dsp.crossfeed_feed_level",
            &serde_json::Value::Number(serde_json::Number::from_f64(0.5).unwrap())).unwrap();
        assert!((cfg.dsp.crossfeed_feed_level - 0.5_f32).abs() < 1e-5);

        // feed_level: clamp low (-0.1 → 0.0)
        apply_dsp_key(&mut cfg, "dsp.crossfeed_feed_level",
            &serde_json::Value::Number(serde_json::Number::from_f64(-0.1).unwrap())).unwrap();
        assert_eq!(cfg.dsp.crossfeed_feed_level, 0.0_f32);

        // feed_level: clamp high (1.5 → 0.9)
        apply_dsp_key(&mut cfg, "dsp.crossfeed_feed_level",
            &serde_json::Value::Number(serde_json::Number::from_f64(1.5).unwrap())).unwrap();
        assert_eq!(cfg.dsp.crossfeed_feed_level, 0.9_f32);

        // cutoff_hz: clamp low (250.0 → 300.0)
        apply_dsp_key(&mut cfg, "dsp.crossfeed_cutoff_hz",
            &serde_json::Value::Number(serde_json::Number::from_f64(250.0).unwrap())).unwrap();
        assert_eq!(cfg.dsp.crossfeed_cutoff_hz, 300.0_f32);

        // cutoff_hz: clamp high (800.0 → 700.0)
        apply_dsp_key(&mut cfg, "dsp.crossfeed_cutoff_hz",
            &serde_json::Value::Number(serde_json::Number::from_f64(800.0).unwrap())).unwrap();
        assert_eq!(cfg.dsp.crossfeed_cutoff_hz, 700.0_f32);
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd runtime && cargo test dsp_crossfeed_keys 2>&1 | tail -10
```

Expected: `unknown dsp config key: crossfeed_enabled` errors.

- [ ] **Step 3: Add the four arms to `apply_dsp_key`**

In `runtime/src/config/manager.rs`, in `apply_dsp_key`, add before the `_ =>` wildcard arm (after `"buffer_size" => ...`):

```rust
        "crossfeed_enabled"    => cfg.dsp.crossfeed_enabled    = as_bool(key, value)?,
        "crossfeed_auto"       => cfg.dsp.crossfeed_auto        = as_bool(key, value)?,
        "crossfeed_feed_level" => cfg.dsp.crossfeed_feed_level  =
            (as_f64(key, value)? as f32).clamp(0.0_f32, 0.9_f32),
        "crossfeed_cutoff_hz"  => cfg.dsp.crossfeed_cutoff_hz   =
            (as_f64(key, value)? as f32).clamp(300.0_f32, 700.0_f32),
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cd runtime && cargo test dsp_crossfeed_keys 2>&1
```

Expected: `test config::manager::tests::dsp_crossfeed_keys ... ok`

- [ ] **Step 5: Run full Rust test suite**

```bash
cd runtime && cargo test 2>&1 | tail -5
```

Expected: all tests pass, zero failures.

- [ ] **Step 6: Commit**

```bash
git add runtime/src/config/manager.rs
git commit -m "feat(config): add crossfeed config manager keys with clamp validation"
```

---

## Chunk 2: Go TUI

### Task 5: Implement CrossfeedDialogModel

**Files:**
- Create: `tui/internal/ui/screens/crossfeed_dialog.go`
- Create: `tui/internal/ui/screens/crossfeed_dialog_test.go`

- [ ] **Step 1: Write the failing tests**

Create `tui/internal/ui/screens/crossfeed_dialog_test.go`:

```go
package screens

import (
	"os"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
)

func TestCrossfeedDialogView_ContainsFields(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	v := m.View()
	for _, want := range []string{"Crossfeed", "Feed", "Cutoff"} {
		if !strings.Contains(v.Content, want) {
			t.Errorf("View().Content should contain %q", want)
		}
	}
}

func TestCrossfeedDialogView_Golden(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	got := m.View().Content

	const golden = "testdata/crossfeed_dialog_golden.txt"
	if os.Getenv("UPDATE_GOLDEN") == "1" {
		if err := os.MkdirAll("testdata", 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(golden, []byte(got), 0o644); err != nil {
			t.Fatal(err)
		}
		t.Logf("golden file updated: %s", golden)
		return
	}

	data, err := os.ReadFile(golden)
	if err != nil {
		t.Fatalf("golden file missing — run with UPDATE_GOLDEN=1 to create it: %v", err)
	}
	if string(data) != got {
		t.Errorf("View output differs from golden file.\nGot:\n%s\nWant:\n%s", got, string(data))
	}
}

func TestCrossfeedTabCyclesFields(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	if m.field != 0 {
		t.Fatalf("expected field 0, got %d", m.field)
	}
	next, _ := m.Update(tea.KeyMsg{Type: tea.KeyTab})
	d := next.(CrossfeedDialogModel)
	if d.field != 1 {
		t.Errorf("tab should advance to field 1, got %d", d.field)
	}
}

func TestCrossfeedPresetCycleViaP(t *testing.T) {
	m := NewCrossfeedDialogModel(nil)
	m.SetSize(80, 24)
	if m.presetIdx != 0 {
		t.Fatalf("expected presetIdx 0, got %d", m.presetIdx)
	}
	next, _ := m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'p'}})
	d := next.(CrossfeedDialogModel)
	if d.presetIdx != 1 {
		t.Errorf("p should advance to presetIdx 1, got %d", d.presetIdx)
	}
}

func TestSettingsHasCrossfeedEntry(t *testing.T) {
	cats := defaultCategories()
	var found bool
	for _, cat := range cats {
		for _, item := range cat.items {
			if item.key == "dsp.crossfeed_enabled" {
				found = true
			}
		}
	}
	if !found {
		t.Error("settings should have a dsp.crossfeed_enabled entry")
	}
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd tui && go test ./internal/ui/screens/ -run "Crossfeed|TestSettingsHasCrossfeedEntry" 2>&1
```

Expected: compile errors — `CrossfeedDialogModel` and `NewCrossfeedDialogModel` undefined.

- [ ] **Step 3: Implement CrossfeedDialogModel**

Create `tui/internal/ui/screens/crossfeed_dialog.go`:

```go
package screens

// crossfeed_dialog.go — BS2B headphone crossfeed settings dialog.
//
// Layout (56 cols × 16 rows content, centered):
//
//   ┌─────────────── Crossfeed ───────────────┐
//   │                                         │
//   │  Auto-detect   [on ]                    │
//   │  Enabled       [on ]  (auto: detected)  │
//   │                                         │
//   │  Feed level    0.45  ◄────────────►     │
//   │  Cutoff        700 Hz                   │
//   │                                         │
//   │  Presets:  [Default]  [Cmoy]  [Jmeier] │
//   │                                         │
//   │  tab next  +/- nudge  p preset  q close │
//   └─────────────────────────────────────────┘
//
// Key bindings:
//   tab/shift+tab  — cycle fields 0–3
//   + / =          — nudge up   (feed ±0.05, cutoff ±10 Hz, toggles flip)
//   - / _          — nudge down
//   p              — cycle presets (Default → Cmoy → Jmeier → Default)
//   q / esc        — commit all four IPC keys and close

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stui/stui/internal/ui/screen"
	"github.com/stui/stui/pkg/theme"
)

// crossfeedPreset holds a named preset configuration.
type crossfeedPreset struct {
	name      string
	feedLevel float64
	cutoffHz  float64
}

var crossfeedPresets = []crossfeedPreset{
	{"Default", 0.45, 700},
	{"Cmoy", 0.65, 700},
	{"Jmeier", 0.90, 650},
}

// CrossfeedDialogModel is the crossfeed settings dialog screen.
type CrossfeedDialogModel struct {
	enabled   bool
	auto      bool
	feedLevel float64 // 0.0–0.9
	cutoffHz  float64 // 300–700
	field     int     // 0=auto, 1=enabled, 2=feed, 3=cutoff
	presetIdx int     // 0=Default, 1=Cmoy, 2=Jmeier
	width     int
	height    int
	sendFn    func(key string, value interface{}) tea.Cmd
}

// NewCrossfeedDialogModel constructs the dialog with default preset values.
// sendFn may be nil (used in tests).
func NewCrossfeedDialogModel(sendFn func(key string, value interface{}) tea.Cmd) CrossfeedDialogModel {
	return CrossfeedDialogModel{
		feedLevel: crossfeedPresets[0].feedLevel,
		cutoffHz:  crossfeedPresets[0].cutoffHz,
		sendFn:    sendFn,
	}
}

// SetSize satisfies the screen sizing convention.
func (m *CrossfeedDialogModel) SetSize(w, h int) {
	m.width = w
	m.height = h
}

func (m CrossfeedDialogModel) Init() tea.Cmd { return nil }

func (m CrossfeedDialogModel) Update(msg tea.Msg) (screen.Screen, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		switch {
		case msg.Type == tea.KeyTab:
			m.field = (m.field + 1) % 4
		case msg.Type == tea.KeyShiftTab:
			m.field = (m.field + 3) % 4
		case msg.Type == tea.KeyRunes && (string(msg.Runes) == "+" || string(msg.Runes) == "="):
			m.nudge(+1)
		case msg.Type == tea.KeyRunes && (string(msg.Runes) == "-" || string(msg.Runes) == "_"):
			m.nudge(-1)
		case msg.Type == tea.KeyRunes && string(msg.Runes) == "p":
			m.presetIdx = (m.presetIdx + 1) % len(crossfeedPresets)
			p := crossfeedPresets[m.presetIdx]
			m.feedLevel = p.feedLevel
			m.cutoffHz = p.cutoffHz
		case msg.Type == tea.KeyRunes && string(msg.Runes) == "q",
			msg.Type == tea.KeyEsc:
			return m, m.commit()
		}
	}
	return m, nil
}

func (m *CrossfeedDialogModel) nudge(dir int) {
	switch m.field {
	case 0:
		m.auto = dir > 0
	case 1:
		m.enabled = dir > 0
	case 2:
		m.feedLevel = clampF64(m.feedLevel+float64(dir)*0.05, 0.0, 0.9)
	case 3:
		m.cutoffHz = clampF64(m.cutoffHz+float64(dir)*10, 300, 700)
	}
}

func clampF64(v, lo, hi float64) float64 {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

func (m CrossfeedDialogModel) commit() tea.Cmd {
	if m.sendFn == nil {
		return screen.PopCmd()
	}
	return tea.Batch(
		m.sendFn("dsp.crossfeed_enabled", m.enabled),
		m.sendFn("dsp.crossfeed_auto", m.auto),
		m.sendFn("dsp.crossfeed_feed_level", m.feedLevel),
		m.sendFn("dsp.crossfeed_cutoff_hz", m.cutoffHz),
		screen.PopCmd(),
	)
}

func (m CrossfeedDialogModel) View() tea.View {
	th := theme.Current()

	boolStr := func(v bool) string {
		if v {
			return "on "
		}
		return "off"
	}

	autoStr := boolStr(m.auto)
	enabledStr := boolStr(m.enabled)

	autoNote := ""
	if m.auto {
		autoNote = "  (auto: detected)"
	}

	cursor := func(i int) string {
		if m.field == i {
			return lipgloss.NewStyle().Foreground(th.Accent).Render("▶")
		}
		return " "
	}

	lines := []string{
		"",
		fmt.Sprintf("  %s Auto-detect   [%s]", cursor(0), autoStr),
		fmt.Sprintf("  %s Enabled       [%s]%s", cursor(1), enabledStr, autoNote),
		"",
		fmt.Sprintf("  %s Feed level    %.2f", cursor(2), m.feedLevel),
		fmt.Sprintf("  %s Cutoff        %.0f Hz", cursor(3), m.cutoffHz),
		"",
	}

	// Preset buttons — highlighted if active
	presetLine := "  Presets: "
	for i, p := range crossfeedPresets {
		label := fmt.Sprintf("[%s]", p.name)
		if i == m.presetIdx {
			label = lipgloss.NewStyle().Foreground(th.Accent).Render(label)
		}
		presetLine += " " + label
	}
	lines = append(lines, presetLine, "")

	hintLine := hintBar("tab next", "+/- nudge", "p preset", "q close")
	lines = append(lines, hintLine)

	body := lipgloss.JoinVertical(lipgloss.Left, lines...)

	box := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(th.Border).
		Padding(0, 2).
		Width(54).
		Render(lipgloss.NewStyle().
			Foreground(th.Title).
			Bold(true).
			Render("  Crossfeed") + "\n" + body)

	content := lipgloss.Place(m.width, m.height,
		lipgloss.Center, lipgloss.Center, box)

	return tea.View{Content: content}
}
```

Note: `screen.PopCmd()` must exist. Check `tui/internal/ui/screen/screen.go` — if `PopCmd` is missing, add it there:

```go
// PopCmd returns a Cmd that sends a PopMsg, telling the root to pop the current screen.
func PopCmd() tea.Cmd {
    return func() tea.Msg { return PopMsg{} }
}
```

- [ ] **Step 4: Generate golden file**

```bash
cd tui && UPDATE_GOLDEN=1 go test ./internal/ui/screens/ -run TestCrossfeedDialogView_Golden
```

Expected: golden file written to `tui/internal/ui/screens/testdata/crossfeed_dialog_golden.txt`.

- [ ] **Step 5: Run all crossfeed tests**

```bash
cd tui && go test ./internal/ui/screens/ -run "Crossfeed|TestSettingsHasCrossfeedEntry" -v 2>&1
```

Expected: 4 tests pass (View_ContainsFields, View_Golden, TabCyclesFields, PresetCycleViaP). `TestSettingsHasCrossfeedEntry` will still fail — it's wired in Task 6.

- [ ] **Step 6: Commit**

```bash
git add tui/internal/ui/screens/crossfeed_dialog.go \
        tui/internal/ui/screens/crossfeed_dialog_test.go \
        tui/internal/ui/screens/testdata/crossfeed_dialog_golden.txt
git commit -m "feat(tui): implement CrossfeedDialogModel"
```

---

### Task 6: Wire crossfeed dialog into settings screen

**Files:**
- Modify: `tui/internal/ui/screens/settings.go`

- [ ] **Step 1: Verify the test fails**

```bash
cd tui && go test ./internal/ui/screens/ -run TestSettingsHasCrossfeedEntry -v 2>&1
```

Expected: FAIL — `dsp.crossfeed_enabled` not found in settings categories.

- [ ] **Step 2: Add the Crossfeed entry to DSP Audio category**

In `settings.go`, find the closing `},` of the `"Conv bypass"` entry (the last item in the DSP Audio category) and add after it:

```go
			{
				label:       "Crossfeed",
				key:         "dsp.crossfeed_enabled",
				kind:        settingAction,
				description: "BS2B headphone crossfeed — blend L/R for natural stereo image",
			},
```

- [ ] **Step 3: Add the `settingAction` case**

In `settings.go`, find the `switch item.key {` block inside the `if item.kind == settingAction {` branch. Add before the `default:` case (or before the closing brace if there is none):

```go
					case "dsp.crossfeed_enabled":
						dialog := NewCrossfeedDialogModel(func(key string, val interface{}) tea.Cmd {
							return func() tea.Msg { return SettingsChangedMsg{Key: key, Value: val} }
						})
						dialog.SetSize(m.width, m.height)
						return m, screen.TransitionCmd(dialog, true)
```

- [ ] **Step 4: Run all Go tests**

```bash
cd tui && go test ./internal/ui/screens/ -v 2>&1 | tail -30
```

Expected: all tests pass including `TestSettingsHasCrossfeedEntry`.

- [ ] **Step 5: Run full Go test suite**

```bash
cd tui && go test ./... 2>&1 | tail -10
```

Expected: all packages pass.

- [ ] **Step 6: Commit**

```bash
git add tui/internal/ui/screens/settings.go
git commit -m "feat(tui): wire crossfeed dialog into DSP Audio settings"
```

---

## Finishing

- [ ] **Run full test suites one final time**

```bash
cd /home/ozogorgor/Projects/Stui_Project/stui/.worktrees/parametric-eq
cd runtime && cargo test 2>&1 | tail -5
cd ../tui && go test ./... 2>&1 | tail -5
```

Expected: all tests pass in both runtimes.

- [ ] **Use superpowers:finishing-a-development-branch to complete the work**
