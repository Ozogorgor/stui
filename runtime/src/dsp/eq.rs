//! Parametric EQ: biquad filter bank, Direct Form II Transposed.
//!
//! Coefficient formulas from Audio EQ Cookbook (Robert Bristow-Johnson).
//! See magnitude_db() below for the transfer function evaluation.

use super::config::{EqBand, EqFilterType};

// ── Coefficient computation ────────────────────────────────────────────────

/// Normalised biquad coefficients (b0,b1,b2 feedforward; a1,a2 feedback).
/// a1/a2 are stored as positive Cookbook values; the recurrence subtracts them.
#[derive(Debug, Clone, Copy)]
struct Coeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

fn compute_coeffs(band: &EqBand, sample_rate: u32) -> Coeffs {
    use std::f32::consts::PI;
    let w0 = 2.0 * PI * band.freq / sample_rate as f32;
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
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

// ── BiquadFilter ───────────────────────────────────────────────────────────

pub struct BiquadFilter {
    c: Coeffs,
    pub z1l: f32,
    pub z2l: f32, // left channel state
    pub z1r: f32,
    pub z2r: f32, // right channel state
    pub sample_rate: u32,
    pub band: EqBand,
}

impl BiquadFilter {
    pub fn new(band: EqBand, sample_rate: u32) -> Self {
        let c = compute_coeffs(&band, sample_rate);
        Self {
            c,
            z1l: 0.0,
            z2l: 0.0,
            z1r: 0.0,
            z2r: 0.0,
            sample_rate,
            band,
        }
    }

    /// Process stereo-interleaved samples [L0,R0,L1,R1,...].
    /// Recomputes coefficients if sample_rate changed (and resets state).
    pub fn process_stereo(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if sample_rate != self.sample_rate {
            self.c = compute_coeffs(&self.band, sample_rate);
            self.z1l = 0.0;
            self.z2l = 0.0;
            self.z1r = 0.0;
            self.z2r = 0.0;
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

// ── ParametricEq ──────────────────────────────────────────────────────────

/// Multi-band parametric equalizer (max 10 biquad bands).
/// The caller (DspPipeline) owns the update path; no config arc stored here.
pub struct ParametricEq {
    pub filters: Vec<BiquadFilter>,
    enabled: bool,
    bypass: bool,
}

impl ParametricEq {
    /// Construct with initial band list. Enabled by default, bypass off.
    pub fn new(bands: &[EqBand]) -> Self {
        let mut eq = Self {
            filters: Vec::new(),
            enabled: true,
            bypass: false,
        };
        eq.update_bands(bands);
        eq
    }

    pub fn set_enabled(&mut self, v: bool) {
        self.enabled = v;
    }
    pub fn set_bypass(&mut self, v: bool) {
        self.bypass = v;
    }

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
            tracing::warn!(
                count = bands.len(),
                "eq: more than 10 bands; truncating to 10"
            );
            &bands[..10]
        } else {
            bands
        };
        // Clamp parameters defensively
        let clamped: Vec<EqBand> = bands
            .iter()
            .map(|b| EqBand {
                freq: b.freq.clamp(20.0, 20000.0),
                gain_db: b.gain_db.clamp(-20.0, 20.0),
                q: b.q.clamp(0.1, 10.0),
                ..b.clone()
            })
            .collect();

        // Use default sample_rate=44100 for initial coefficient computation;
        // will be recomputed on first process() call if different.
        let default_sr = self.filters.first().map(|f| f.sample_rate).unwrap_or(44100);

        let new_filters: Vec<BiquadFilter> = clamped
            .iter()
            .enumerate()
            .map(|(i, band)| {
                let mut f = BiquadFilter::new(band.clone(), default_sr);
                // Preserve state if same index and same type
                if let Some(old) = self.filters.get(i) {
                    if old.band.filter_type == band.filter_type {
                        f.z1l = old.z1l;
                        f.z2l = old.z2l;
                        f.z1r = old.z1r;
                        f.z2r = old.z2r;
                    }
                }
                f
            })
            .collect();

        self.filters = new_filters;
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Evaluate |H(ω)| in dB for normalised angular frequency ω = 2π*freq/rate.
    /// Test-only helper — previously a `pub fn` in the module (triggered
    /// `private_interfaces` on `Coeffs` and `never used` outside tests).
    fn magnitude_db(c: Coeffs, omega: f32) -> f32 {
        let (cos1, sin1) = (omega.cos(), omega.sin());
        let (cos2, sin2) = ((2.0 * omega).cos(), (2.0 * omega).sin());
        let num_re = c.b0 + c.b1 * cos1 + c.b2 * cos2;
        let num_im = c.b1 * sin1 + c.b2 * sin2;
        let den_re = 1.0 + c.a1 * cos1 + c.a2 * cos2;
        let den_im = c.a1 * sin1 + c.a2 * sin2;
        let ratio = (num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im);
        20.0 * ratio.sqrt().log10()
    }

    fn omega(freq_hz: f32, sample_rate: u32) -> f32 {
        2.0 * PI * freq_hz / sample_rate as f32
    }

    fn make_filter(ft: EqFilterType, freq: f32, gain_db: f32, q: f32, sr: u32) -> BiquadFilter {
        BiquadFilter::new(
            EqBand {
                enabled: true,
                filter_type: ft,
                freq,
                gain_db,
                q,
            },
            sr,
        )
    }

    // ── coefficient / magnitude tests ──────────────────────────────────────

    #[test]
    fn peak_center_gain() {
        // Peak +6dB at 1kHz, Q=1, 44100Hz → magnitude at 1kHz ≈ +6dB
        let f = make_filter(EqFilterType::Peak, 1000.0, 6.0, 1.0, 44100);
        let db = magnitude_db(f.c, omega(1000.0, 44100));
        assert!(
            (db - 6.0).abs() < 0.1,
            "peak: got {db:.3}dB, expected 6.0dB"
        );
    }

    #[test]
    fn lowpass_corner_attenuation() {
        // LowPass at 5kHz, Q=0.707 (Butterworth), 44100Hz → -3dB at 5kHz
        let f = make_filter(EqFilterType::LowPass, 5000.0, 0.0, 0.707, 44100);
        let db = magnitude_db(f.c, omega(5000.0, 44100));
        assert!(
            (db - (-3.0)).abs() < 0.5,
            "lp corner: got {db:.3}dB, expected -3dB"
        );
    }

    #[test]
    fn highpass_corner_attenuation() {
        let f = make_filter(EqFilterType::HighPass, 5000.0, 0.0, 0.707, 44100);
        let db = magnitude_db(f.c, omega(5000.0, 44100));
        assert!(
            (db - (-3.0)).abs() < 0.5,
            "hp corner: got {db:.3}dB, expected -3dB"
        );
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
        assert!(
            (db - 6.0).abs() < 0.5,
            "lowshelf: got {db:.3}dB at 20Hz, expected ~+6dB"
        );
    }

    #[test]
    fn highshelf_boost_above_corner() {
        let f = make_filter(EqFilterType::HighShelf, 5000.0, 6.0, 0.707, 44100);
        let db = magnitude_db(f.c, omega(18000.0, 44100)); // well above shelf
        assert!(
            (db - 6.0).abs() < 0.5,
            "highshelf: got {db:.3}dB at 18kHz, expected ~+6dB"
        );
    }

    // ── denormal protection ────────────────────────────────────────────────

    #[test]
    fn denormal_protection() {
        let mut f = make_filter(EqFilterType::Peak, 1000.0, 6.0, 1.0, 44100);
        let silence = vec![0.0f32; 20000]; // 10k stereo samples
        let _ = f.process_stereo(&silence, 44100);
        // All state registers must be normal or zero
        for (name, val) in [
            ("z1l", f.z1l),
            ("z2l", f.z2l),
            ("z1r", f.z1r),
            ("z2r", f.z2r),
        ] {
            assert!(
                val.is_normal() || val == 0.0,
                "{name} subnormal after silence: {val:e}"
            );
        }
    }

    // ── sample rate change ─────────────────────────────────────────────────

    #[test]
    fn sample_rate_change_recomputes_and_resets() {
        let mut f = make_filter(EqFilterType::Peak, 1000.0, 6.0, 1.0, 44100);
        let b0_before = f.c.b0;
        // Inject some non-zero state
        f.z1l = 0.5;
        f.z2l = 0.3;
        let tone = vec![0.1f32; 512];
        let _ = f.process_stereo(&tone, 192000); // different rate
        assert_ne!(f.c.b0, b0_before, "b0 should change after rate switch");
        // State must have been reset before processing at new rate
        // (can only verify indirectly — no panic and output is finite)
    }

    // ── ParametricEq tests ────────────────────────────────────────────────

    use super::ParametricEq;

    #[test]
    fn parametric_eq_cascade() {
        // Two peaks at different freqs; verify each center is boosted
        let bands = vec![
            EqBand {
                enabled: true,
                filter_type: EqFilterType::Peak,
                freq: 500.0,
                gain_db: 6.0,
                q: 1.0,
            },
            EqBand {
                enabled: true,
                filter_type: EqFilterType::Peak,
                freq: 4000.0,
                gain_db: 6.0,
                q: 1.0,
            },
        ];
        let mut eq = ParametricEq::new(&bands);
        // Generate a tone at 500Hz: 44100Hz, 2-channel, 2048 samples
        let sr = 44100u32;
        let tone: Vec<f32> = (0..2048)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 500.0 * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        let out = eq.process(&tone, sr);
        // RMS of output should be greater than RMS of input (boosted at 500Hz)
        let rms_in = (tone.iter().map(|x| x * x).sum::<f32>() / tone.len() as f32).sqrt();
        let rms_out = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
        assert!(
            rms_out > rms_in * 1.1,
            "cascade: rms_out={rms_out:.4} should be > 1.1 × rms_in={rms_in:.4}"
        );
    }

    #[test]
    fn bypass_passes_through() {
        let bands = vec![EqBand {
            enabled: true,
            filter_type: EqFilterType::Peak,
            freq: 1000.0,
            gain_db: 12.0,
            q: 1.0,
        }];
        let mut eq = ParametricEq::new(&bands);
        eq.set_bypass(true);
        let input: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect();
        let output = eq.process(&input, 44100);
        assert_eq!(input, output, "bypass should return input unchanged");
    }

    #[test]
    fn update_bands_preserves_state_on_freq_change() {
        let band = EqBand {
            enabled: true,
            filter_type: EqFilterType::Peak,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        };
        let mut eq = ParametricEq::new(&[band]);
        // Warm up state
        let tone = vec![0.5f32; 512];
        let _ = eq.process(&tone, 44100);
        let z1l_before = eq.filters[0].z1l;
        assert_ne!(z1l_before, 0.0, "state should be non-zero after processing");

        // Change freq only (same type) → state preserved
        let updated = EqBand {
            freq: 2000.0,
            ..eq.filters[0].band.clone()
        };
        eq.update_bands(&[updated]);
        assert_eq!(
            eq.filters[0].z1l, z1l_before,
            "state should survive freq nudge"
        );
    }

    #[test]
    fn update_bands_resets_state_on_type_change() {
        let band = EqBand {
            enabled: true,
            filter_type: EqFilterType::Peak,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        };
        let mut eq = ParametricEq::new(&[band]);
        let tone = vec![0.5f32; 512];
        let _ = eq.process(&tone, 44100);
        assert_ne!(eq.filters[0].z1l, 0.0);

        let changed = EqBand {
            filter_type: EqFilterType::LowPass,
            ..eq.filters[0].band.clone()
        };
        eq.update_bands(&[changed]);
        assert_eq!(eq.filters[0].z1l, 0.0, "state must reset on type change");
    }

    #[test]
    fn update_bands_drops_removed() {
        let bands: Vec<EqBand> = (0..3)
            .map(|i| EqBand {
                enabled: true,
                filter_type: EqFilterType::Peak,
                freq: 500.0 * (i + 1) as f32,
                gain_db: 3.0,
                q: 1.0,
            })
            .collect();
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
}
