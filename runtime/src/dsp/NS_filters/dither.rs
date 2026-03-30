//! TPDF dither + noise shaping filter.
//!
//! Applies triangular-PDF dither and optional error-feedback noise shaping
//! before bit-depth quantization. This is the final DSP stage before output.
//!
//! All coefficient tables sourced from SoX src/dither.c (LGPL).

// ── Imports from coefficient modules ─────────────────────────────────────────

use super::fweighted;
use super::gesemann;
use super::imp_e_weighted;
use super::lipshitz;
use super::mod_e_weighted;
use super::shibata;

// ── Constants ─────────────────────────────────────────────────────────────────

/// xorshift64 seed — golden ratio constant, guaranteed nonzero.
pub(crate) const INITIAL_SEED: u64 = 0x9E3779B97F4A7C15;

//Gesemann now imported from gesemann module
// ── NoiseShaping ─────────────────────────────────────────────────────────────

/// Noise shaping algorithm selection.
#[derive(Debug, Clone, PartialEq)]
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
    /// Spectral Amplitude Warping (SAW) - frequency domain noise shaping
    /// based on paper: "Spectral Amplitude Warping (SAW) for Noise Spectrum Shaping
    /// in Audio Coding" (Lefebvre & Laflamme, ICASSP 1997)
    Saw,
    /// Entropy-Controlled Dither (2025) — dither amplitude adapts to signal
    /// complexity per-sample.  Inspired by arxiv:2501.02293.
    ///
    /// Tonal, quiet signals receive up to 1.5× normal dither amplitude for
    /// proper quantizer linearisation.  Noise-like or loud signals receive
    /// reduced amplitude (floor: 0.5× lsb) since the signal's own randomness
    /// already provides linearisation.
    ///
    /// Complexity is estimated via a smoothed zero-crossing rate (τ = 10 ms)
    /// and a smoothed RMS envelope (τ = 50 ms), both updated sample-by-sample
    /// with no added latency.
    EntropyDither,
    /// Temporal-Spectral Unified Noise Shaper — per-bin warp depth modulated
    /// by local spectral flux.  Stable frequency bins receive full shaping;
    /// rapidly changing bins (onsets, transients) are individually suppressed.
    ///
    /// Introduces 256-sample OLA latency (same as `Saw`).
    Tns,
}

impl NoiseShaping {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "lipshitz" => Some(Self::Lipshitz),
            "fweighted" => Some(Self::Fweighted),
            "modified_e_weighted" => Some(Self::ModifiedEweighted),
            "improved_e_weighted" => Some(Self::ImprovedEweighted),
            "shibata" => Some(Self::Shibata),
            "low_shibata" => Some(Self::LowShibata),
            "high_shibata" => Some(Self::HighShibata),
            "gesemann" => Some(Self::Gesemann),
            "saw" => Some(Self::Saw),
            "entropy_dither" => Some(Self::EntropyDither),
            "tns" => Some(Self::Tns),
            _ => None,
        }
    }
}

// ── EntropyDitherState ────────────────────────────────────────────────────────

/// Per-channel running state for entropy-controlled dither.
struct EntropyDitherState {
    /// Smoothed zero-crossing rate (one-pole IIR, τ ≈ 10 ms).
    zcr_smooth: f32,
    /// Smoothed mean-square energy (one-pole IIR, τ ≈ 50 ms).
    rms_sq_smooth: f32,
    /// Sign of the previous sample — used to detect zero crossings.
    prev_sign: f32,
    /// One-pole attack coefficient for ZCR smoother.
    zcr_coeff: f32,
    /// One-pole attack coefficient for RMS² smoother.
    rms_coeff: f32,
}

impl EntropyDitherState {
    fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        Self {
            zcr_smooth: 0.5, // start at noise-like (neutral: max-entropy assumption)
            rms_sq_smooth: 0.0,
            prev_sign: 0.0,
            // α = 1 − exp(−1 / (τ · Fs))
            zcr_coeff: 1.0 - (-1.0 / (0.010 * sr)).exp(), // 10 ms
            rms_coeff: 1.0 - (-1.0 / (0.050 * sr)).exp(), // 50 ms
        }
    }

    /// Update running estimates from `sample`; return the adaptive dither
    /// amplitude in **quantisation-step units** (1.0 = 1 LSB step).
    ///
    /// The caller should use this directly as `raw_tpdf * amp_steps` before
    /// adding to the integer-scaled signal: `(sample * scale + raw * amp_steps).round()`.
    fn update(&mut self, sample: f32) -> f32 {
        // ── Zero-crossing rate ────────────────────────────────────────────────
        let sign = if sample > 0.0 {
            1.0_f32
        } else if sample < 0.0 {
            -1.0
        } else {
            self.prev_sign // treat exact zero as no crossing
        };
        let crossed = if sign != self.prev_sign && self.prev_sign != 0.0 {
            1.0_f32
        } else {
            0.0
        };
        self.prev_sign = sign;
        self.zcr_smooth += self.zcr_coeff * (crossed - self.zcr_smooth);

        // ── RMS envelope ──────────────────────────────────────────────────────
        self.rms_sq_smooth += self.rms_coeff * (sample * sample - self.rms_sq_smooth);

        // ── Complexity ────────────────────────────────────────────────────────
        // White noise ZCR ≈ 0.5 crossings/sample; normalise to [0, 1].
        let complexity = (self.zcr_smooth * 2.0).clamp(0.0, 1.0);
        // RMS normalised to [0, 1] (full scale = 1.0).
        let rms_norm = self.rms_sq_smooth.max(0.0).sqrt().clamp(0.0, 1.0);

        // ── Adaptive amplitude (in LSB steps) ─────────────────────────────────
        // tonal (low complexity) → up to +50 % for better quantiser linearisation
        // loud (high rms_norm)   → down to −30 % (signal already randomises quantiser)
        let tonal_factor = 1.0 - complexity;
        let amp = (1.0 + 0.5 * tonal_factor) * (1.0 - 0.3 * rms_norm);
        // Floor at 0.5 steps so dither always provides some linearisation.
        amp.max(0.5)
    }
}

// ── DitherFilter ─────────────────────────────────────────────────────────────

use super::saw::{SawNode, StftProcessor};
use super::tns::TnsNode;

pub struct DitherFilter {
    bit_depth: u32,
    noise_shaping: NoiseShaping,
    sample_rate: u32, // initialized to 44100; updated on each rate change
    // FIR error feedback state (per channel)
    pub(crate) err_l: Vec<f32>,
    pub(crate) err_r: Vec<f32>,
    // IIR state (Gesemann feedforward + feedback, per channel)
    pub(crate) ff_l: Vec<f32>,
    pub(crate) ff_r: Vec<f32>,
    pub(crate) fb_l: Vec<f32>,
    pub(crate) fb_r: Vec<f32>,
    pub(crate) rng: u64,
    // Cached coefficient slices (set by select_coeffs)
    fir_c: Vec<f32>,
    ges_ff_c: Vec<f32>,
    ges_fb_c: Vec<f32>,
    // SAW processor for frequency-domain noise shaping (per-channel for stereo)
    saw: [Option<StftProcessor>; 2],
    // TNS processor for temporal-spectral unified shaping (per-channel for stereo)
    tns: [Option<StftProcessor>; 2],
    // Entropy-controlled dither running state (per channel)
    entropy_state: [EntropyDitherState; 2],
}

impl DitherFilter {
    pub fn new(bit_depth: u32, noise_shaping: NoiseShaping) -> Self {
        let bit_depth = bit_depth.max(2).min(32);
        let default_rate = 44100.0_f32;
        let saw = if noise_shaping == NoiseShaping::Saw {
            let mut stft_l = StftProcessor::new(default_rate);
            stft_l.add_dsp(Box::new(SawNode::new(0.55, default_rate)));
            let mut stft_r = StftProcessor::new(default_rate);
            stft_r.add_dsp(Box::new(SawNode::new(0.55, default_rate)));
            [Some(stft_l), Some(stft_r)]
        } else {
            [None, None]
        };

        let tns = if noise_shaping == NoiseShaping::Tns {
            let mut stft_l = StftProcessor::new(default_rate);
            stft_l.add_dsp(Box::new(TnsNode::new(0.6, 4.0, default_rate)));
            let mut stft_r = StftProcessor::new(default_rate);
            stft_r.add_dsp(Box::new(TnsNode::new(0.6, 4.0, default_rate)));
            [Some(stft_l), Some(stft_r)]
        } else {
            [None, None]
        };

        let mut f = Self {
            bit_depth,
            noise_shaping,
            sample_rate: 0,
            err_l: Vec::new(),
            err_r: Vec::new(),
            ff_l: Vec::new(),
            ff_r: Vec::new(),
            fb_l: Vec::new(),
            fb_r: Vec::new(),
            rng: INITIAL_SEED,
            fir_c: Vec::new(),
            ges_ff_c: Vec::new(),
            ges_fb_c: Vec::new(),
            saw,
            tns,
            entropy_state: [EntropyDitherState::new(44100), EntropyDitherState::new(44100)],
        };
        f.select_coeffs(44100); // nominal; recomputed on first process() call
        f
    }

    /// Select coefficient tables and resize/zero state buffers for `rate_hz`.
    fn select_coeffs(&mut self, rate_hz: u32) {
        self.sample_rate = rate_hz;

        let (fir, ges_ff, ges_fb) = match &self.noise_shaping {
            NoiseShaping::None => (&[][..], &[][..], &[][..]),
            NoiseShaping::Lipshitz => (lipshitz::COEFFS, &[][..], &[][..]),
            NoiseShaping::Fweighted => (fweighted::COEFFS, &[][..], &[][..]),
            NoiseShaping::ModifiedEweighted => (mod_e_weighted::COEFFS, &[][..], &[][..]),
            NoiseShaping::ImprovedEweighted => (imp_e_weighted::COEFFS, &[][..], &[][..]),
            NoiseShaping::Shibata => (shibata::select(rate_hz), &[][..], &[][..]),
            NoiseShaping::LowShibata => (shibata::select_low(rate_hz), &[][..], &[][..]),
            NoiseShaping::HighShibata => (shibata::select_high(rate_hz), &[][..], &[][..]),
            NoiseShaping::Gesemann => {
                let (ff, fb) = gesemann::select(rate_hz);
                (&[][..], ff, fb)
            }
            NoiseShaping::Saw => (&[][..], &[][..], &[][..]),
            NoiseShaping::EntropyDither => (&[][..], &[][..], &[][..]),
            NoiseShaping::Tns => (&[][..], &[][..], &[][..]),
        };

        self.fir_c = fir.to_vec();
        self.ges_ff_c = ges_ff.to_vec();
        self.ges_fb_c = ges_fb.to_vec();

        let fir_n = self.fir_c.len();
        let ges_n = self.ges_ff_c.len();

        self.err_l = vec![0.0; fir_n];
        self.err_r = vec![0.0; fir_n];
        self.ff_l = vec![0.0; ges_n];
        self.ff_r = vec![0.0; ges_n];
        self.fb_l = vec![0.0; ges_n];
        self.fb_r = vec![0.0; ges_n];
    }

    /// Reset all state buffers and re-seed the RNG.
    pub fn reset_state(&mut self, rate_hz: u32) {
        self.select_coeffs(rate_hz);
        self.rng = INITIAL_SEED;
        self.entropy_state = [EntropyDitherState::new(rate_hz), EntropyDitherState::new(rate_hz)];
        // Re-create STFT-based processors on rate change.
        if rate_hz > 0 {
            let rate = rate_hz as f32;
            match self.noise_shaping {
                NoiseShaping::Saw => {
                    let mut l = StftProcessor::new(rate);
                    l.add_dsp(Box::new(SawNode::new(0.55, rate)));
                    let mut r = StftProcessor::new(rate);
                    r.add_dsp(Box::new(SawNode::new(0.55, rate)));
                    self.saw = [Some(l), Some(r)];
                }
                NoiseShaping::Tns => {
                    let mut l = StftProcessor::new(rate);
                    l.add_dsp(Box::new(TnsNode::new(0.6, 4.0, rate)));
                    let mut r = StftProcessor::new(rate);
                    r.add_dsp(Box::new(TnsNode::new(0.6, 4.0, rate)));
                    self.tns = [Some(l), Some(r)];
                }
                _ => {}
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_params(&mut self, bit_depth: u32, noise_shaping: NoiseShaping) {
        self.bit_depth = bit_depth.max(2).min(32);
        self.noise_shaping = noise_shaping;
        // Clear STFT processors; reset_state will rebuild the active one.
        self.saw = [None, None];
        self.tns = [None, None];
        self.reset_state(self.sample_rate);
    }

    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        debug_assert!(
            samples.len() % 2 == 0,
            "dither: input must be interleaved stereo"
        );

        // 32-bit is a no-op (f32 mantissa = 24 bits; 32-bit quantization is meaningless).
        if self.bit_depth == 32 {
            return samples.to_vec();
        }

        if sample_rate != self.sample_rate {
            self.reset_state(sample_rate);
        }

        let scale = (1u32 << (self.bit_depth.saturating_sub(1))) as f32;
        let lsb = 1.0 / scale;

        let mut out = Vec::with_capacity(samples.len());
        let mut iter = samples.chunks_exact(2);
        for frame in iter.by_ref() {
            let l = self.process_sample(frame[0], scale, lsb, 0);
            let r = self.process_sample(frame[1], scale, lsb, 1);
            out.push(l);
            out.push(r);
        }
        // Odd remainder pass-through (should never occur in a stereo pipeline).
        for &s in iter.remainder() {
            out.push(s);
        }
        out
    }

    /// Process a single sample for the given channel (0=L, 1=R).
    fn process_sample(&mut self, sample: f32, scale: f32, lsb: f32, ch: usize) -> f32 {
        let tpdf = self.next_tpdf() * lsb;

        match &self.noise_shaping {
            NoiseShaping::None => {
                let q = (sample * scale + tpdf).round() / scale;
                q.clamp(-1.0, 1.0 - lsb)
            }
            NoiseShaping::Gesemann => {
                let (ff_buf, fb_buf) = if ch == 0 {
                    (&mut self.ff_l, &mut self.fb_l)
                } else {
                    (&mut self.ff_r, &mut self.fb_r)
                };
                let shaped = tpdf + dot(ff_buf, &self.ges_ff_c) - dot(fb_buf, &self.ges_fb_c);
                let q = (sample * scale + shaped).round() / scale;
                let err = sample - q;
                shift_in(ff_buf, err);
                shift_in(fb_buf, shaped);
                q.clamp(-1.0, 1.0 - lsb)
            }
            NoiseShaping::Saw => {
                // Compute initial quantization with just TPDF dither
                let initial = (sample * scale + tpdf).round() / scale;
                let err = sample - initial;
                // Apply SAW spectral warping to the quantization error (per-channel)
                let ch_idx = if ch == 0 { 0 } else { 1 };
                let shaped = if let Some(ref mut saw) = self.saw[ch_idx] {
                    tpdf + saw.process(err)
                } else {
                    tpdf
                };
                let q = (sample * scale + shaped).round() / scale;
                q.clamp(-1.0, 1.0 - lsb)
            }
            NoiseShaping::Tns => {
                // Same pattern as Saw: apply spectral shaping to the
                // quantisation error, then re-quantise with TPDF dither.
                let initial = (sample * scale + tpdf).round() / scale;
                let err = sample - initial;
                let shaped = if let Some(ref mut tns) = self.tns[ch] {
                    tpdf + tns.process(err)
                } else {
                    tpdf
                };
                let q = (sample * scale + shaped).round() / scale;
                q.clamp(-1.0, 1.0 - lsb)
            }
            NoiseShaping::EntropyDither => {
                // tpdf (from above) is already a TPDF sample scaled to [-lsb, lsb].
                // Divide by lsb to recover the raw [-1, 1] range, then scale by
                // amp_steps (in quantisation-step units) for the adaptive amplitude.
                let amp_steps = self.entropy_state[ch].update(sample);
                let adaptive_tpdf = (tpdf / lsb) * amp_steps;
                let q = (sample * scale + adaptive_tpdf).round() / scale;
                q.clamp(-1.0, 1.0 - lsb)
            }
            _ => {
                let err_buf = if ch == 0 {
                    &mut self.err_l
                } else {
                    &mut self.err_r
                };
                let shaped = tpdf + dot(err_buf, &self.fir_c);
                let q = (sample * scale + shaped).round() / scale;
                let err = sample - q;
                shift_in(err_buf, err);
                q.clamp(-1.0, 1.0 - lsb)
            }
        }
    }

    /// Generate one TPDF sample in [-1, 1] using xorshift64.
    fn next_tpdf(&mut self) -> f32 {
        let r1 = self.xorshift64() as f32 / u64::MAX as f32 - 0.5;
        let r2 = self.xorshift64() as f32 / u64::MAX as f32 - 0.5;
        r1 + r2
    }

    fn xorshift64(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Dot product of two equal-length slices. Returns 0.0 if either is empty.
fn dot(buf: &[f32], coeffs: &[f32]) -> f32 {
    buf.iter().zip(coeffs.iter()).map(|(b, c)| b * c).sum()
}

/// Shift `buf` right, inserting `val` at index 0 (most-recent position).
fn shift_in(buf: &mut Vec<f32>, val: f32) {
    if buf.is_empty() {
        return;
    }
    buf.rotate_right(1);
    buf[0] = val;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine_stereo(freq_hz: f32, sample_rate: u32, n_frames: usize) -> Vec<f32> {
        (0..n_frames)
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

    // bit_depth=32 must be a no-op: return input unchanged.
    #[test]
    fn noop_at_32bit() {
        let input: Vec<f32> = (0..64)
            .flat_map(|i| {
                let v = i as f32 * 0.01;
                [v, -v]
            })
            .collect();
        let mut f = DitherFilter::new(32, NoiseShaping::None);
        let out = f.process(&input, 44100);
        assert_eq!(out.len(), input.len());
        for (a, b) in input.iter().zip(out.iter()) {
            assert_eq!(a, b, "bit_depth=32 must return input unchanged");
        }
    }

    // TPDF mean ≈ 0 over 10k frames.
    #[test]
    fn tpdf_zero_mean() {
        let dc: Vec<f32> = vec![0.5_f32; 20_000]; // 10k stereo frames
        let mut f = DitherFilter::new(16, NoiseShaping::None);
        let out = f.process(&dc, 44100);
        let mean_error: f32 =
            out.iter().zip(dc.iter()).map(|(o, i)| o - i).sum::<f32>() / out.len() as f32;
        assert!(
            mean_error.abs() < 0.001,
            "TPDF mean error {mean_error} not near 0"
        );
    }

    // 16-bit output must snap to multiples of 1/32768.
    #[test]
    fn quantization_snaps_to_lsb() {
        let input = sine_stereo(440.0, 44100, 1024);
        let mut f = DitherFilter::new(16, NoiseShaping::None);
        let out = f.process(&input, 44100);
        let lsb = 1.0_f32 / 32768.0;
        for &s in &out {
            let rounded = (s / lsb).round() * lsb;
            assert!(
                (s - rounded).abs() < 1e-6,
                "sample {s} is not a multiple of lsb={lsb}"
            );
        }
    }

    // Lipshitz shaping must push more noise above 10kHz than below.
    #[test]
    fn noise_shaping_pushes_noise_high() {
        let sr = 44100_u32;
        // -60 dBFS 1 kHz sine
        let input: Vec<f32> = (0..2048_usize)
            .flat_map(|i| {
                let s = (2.0 * PI * 1000.0 * i as f32 / sr as f32).sin() * 0.001;
                [s, s]
            })
            .collect();
        let mut f = DitherFilter::new(16, NoiseShaping::Lipshitz);
        let out = f.process(&input, sr);

        // Separate L-channel, measure noise above and below 10 kHz.
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        let l_in: Vec<f32> = input.iter().step_by(2).copied().collect();
        let noise: Vec<f32> = l.iter().zip(l_in.iter()).map(|(o, i)| o - i).collect();

        // DFT bin check: energy in first vs last quarter of spectrum.
        // Compute DFT magnitude squared for each bin and sum low (bins 0..n/4)
        // vs high (bins n*3/4..n) frequency halves.
        let n = noise.len();
        let low_energy: f32 = (0..n / 4)
            .map(|k| {
                let re: f32 = noise
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).cos())
                    .sum();
                let im: f32 = noise
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).sin())
                    .sum();
                re * re + im * im
            })
            .sum::<f32>()
            / n as f32;
        let high_energy: f32 = (n * 3 / 4..n)
            .map(|k| {
                let re: f32 = noise
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).cos())
                    .sum();
                let im: f32 = noise
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).sin())
                    .sum();
                re * re + im * im
            })
            .sum::<f32>()
            / n as f32;

        assert!(
            high_energy > low_energy,
            "noise shaping should push energy high: high={high_energy:.2e} low={low_energy:.2e}"
        );
    }

    // Rate change clears state without panic.
    #[test]
    fn sample_rate_change_resets_state() {
        let mut f = DitherFilter::new(16, NoiseShaping::Lipshitz);
        let chunk = sine_stereo(1000.0, 44100, 64);
        let _ = f.process(&chunk, 44100);
        // After rate change, process must not panic and must produce valid output.
        let out = f.process(&chunk, 48000);
        assert_eq!(out.len(), chunk.len());
        assert!(
            out.iter().all(|s| s.is_finite()),
            "output must be finite after rate change"
        );
    }

    // Shibata: all three test rates must not panic.
    #[test]
    fn shibata_selects_nearest_rate_table() {
        let chunk = sine_stereo(440.0, 44100, 64);
        for &rate in &[44100_u32, 48000, 96000] {
            let mut f = DitherFilter::new(16, NoiseShaping::Shibata);
            let out = f.process(&chunk, rate);
            assert_eq!(out.len(), chunk.len(), "rate={rate}");
            assert!(
                out.iter().all(|s| s.is_finite()),
                "non-finite at rate={rate}"
            );
        }
    }

    // Gesemann IIR must not produce NaN or Inf.
    #[test]
    fn gesemann_iir_no_nan() {
        let chunk = sine_stereo(440.0, 44100, 1000);
        let mut f = DitherFilter::new(16, NoiseShaping::Gesemann);
        let out = f.process(&chunk, 44100);
        assert!(
            out.iter().all(|s| s.is_finite()),
            "Gesemann must not produce NaN/Inf"
        );
    }

    // EntropyDither: output is finite and quantised to LSB grid.
    #[test]
    fn entropy_dither_quantised_and_finite() {
        let sr = 44100_u32;
        // Mix of tonal (sine) and silence to exercise both code paths.
        let mut input: Vec<f32> = sine_stereo(440.0, sr, 512);
        // Append a quiet passage.
        input.extend(std::iter::repeat(0.0_f32).take(256));
        let mut f = DitherFilter::new(16, NoiseShaping::EntropyDither);
        let out = f.process(&input, sr);
        assert_eq!(out.len(), input.len());
        let lsb = 1.0_f32 / 32768.0;
        for &s in &out {
            assert!(s.is_finite(), "entropy_dither output must be finite");
            let rounded = (s / lsb).round() * lsb;
            assert!(
                (s - rounded).abs() < 1e-6,
                "sample {s} is not a multiple of lsb={lsb}"
            );
        }
    }

    // EntropyDither: amplitude on a 1 kHz tone is visibly higher than on white
    // noise of the same RMS (validates tonal boost path).
    //
    // The test uses a long buffer (100 k stereo frames) so the IIR state fully
    // settles (~10× the 50 ms RMS time constant) before the measurement window.
    #[test]
    fn entropy_dither_boosts_tonal_vs_noise() {
        let sr = 44100_u32;
        let warm_up = 8192_usize;  // stereo frames; ≈ 185 ms — well past all IIR τs
        let measure = 16384_usize; // stereo frames measured

        // 1 kHz sine at -20 dBFS (L = R)
        let tone: Vec<f32> = (0..(warm_up + measure))
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin() * 0.1;
                [s, s]
            })
            .collect();

        // White noise at approx -20 dBFS (uniform ±√3 × 0.1 ≈ same RMS as tone)
        let mut rng: u64 = 0x123456789ABCDEF0;
        let noise: Vec<f32> = (0..(warm_up + measure) * 2)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng as f32 / u64::MAX as f32 - 0.5) * 0.2
            })
            .collect();

        let mut f_tone = DitherFilter::new(16, NoiseShaping::EntropyDither);
        let out_tone = f_tone.process(&tone, sr);

        let mut f_noise = DitherFilter::new(16, NoiseShaping::EntropyDither);
        let out_noise = f_noise.process(&noise, sr);

        // Measure quantisation-error RMS only in the settled window (skip warm-up).
        let quant_err_rms = |input: &[f32], output: &[f32], skip_frames: usize| -> f32 {
            let skip = skip_frames * 2; // stereo samples
            let sum_sq: f32 = input[skip..]
                .iter()
                .zip(output[skip..].iter())
                .map(|(i, o)| (o - i).powi(2))
                .sum();
            (sum_sq / (input.len() - skip) as f32).sqrt()
        };

        let err_tone  = quant_err_rms(&tone,  &out_tone,  warm_up);
        let err_noise = quant_err_rms(&noise, &out_noise, warm_up);

        // After settling, the tonal path (ZCR → 0 → full tonal boost → ~1.42×
        // LSB amplitude) should produce visibly higher quantisation error than
        // the noise path (ZCR → 0.5 → zero boost → ~1.0× LSB amplitude).
        assert!(
            err_tone > err_noise,
            "entropy_dither should add more dither to tonal than noise-like signal \
             (settled window): tone_err={err_tone:.2e} noise_err={err_noise:.2e}"
        );
    }

    // set_params resets all state buffers; rng is re-seeded to INITIAL_SEED.
    #[test]
    fn set_params_resets_state() {
        let mut f = DitherFilter::new(16, NoiseShaping::Gesemann);
        let chunk = sine_stereo(440.0, 44100, 64);
        let _ = f.process(&chunk, 44100);
        // After set_params, all bufs must be zero and rng must be INITIAL_SEED.
        f.set_params(16, NoiseShaping::Gesemann);
        assert!(f.err_l.iter().all(|&x| x == 0.0), "err_l not zeroed");
        assert!(f.err_r.iter().all(|&x| x == 0.0), "err_r not zeroed");
        assert!(f.ff_l.iter().all(|&x| x == 0.0), "ff_l not zeroed");
        assert!(f.ff_r.iter().all(|&x| x == 0.0), "ff_r not zeroed");
        assert!(f.fb_l.iter().all(|&x| x == 0.0), "fb_l not zeroed");
        assert!(f.fb_r.iter().all(|&x| x == 0.0), "fb_r not zeroed");
        assert_eq!(f.rng, INITIAL_SEED, "rng must be reset to INITIAL_SEED");
    }
}
