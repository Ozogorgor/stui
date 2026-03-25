//! LUFS loudness normalization and measurement.
//!
//! Implements ITU-R BS.1770-4 loudness measurement:
//! - Two-stage K-weighting biquad (high-shelf pre-filter + high-pass RLB)
//! - Momentary loudness (400ms sliding window)
//! - Short-term loudness (3s window, 30 × 100ms non-overlapping hops)
//! - Integrated loudness with two-pass gating (absolute −70 LUFS, relative −10 LU)
//! - Sample-peak measurement
//! - Gain-based normalization with configurable attack/release

use std::collections::VecDeque;

/// Absolute gating threshold (BS.1770-4 §3.2).
const ABSOLUTE_GATE: f32 = -70.0;
/// Relative gate offset below the preliminary integrated loudness (BS.1770-4 §3.2).
const RELATIVE_GATE_OFFSET: f32 = 10.0;
/// Measurement block size.
const BLOCK_MS: f32 = 400.0;
/// Hop between successive blocks — 75% overlap.
const HOP_MS: f32 = 100.0;
/// Number of 100ms hops in the 3s short-term window.
const SHORT_TERM_HOPS: usize = 30;

#[inline]
fn energy_to_lufs(energy: f32) -> f32 {
    -0.691 + 10.0 * energy.max(1e-10_f32).log10()
}

// ─── Biquad ─────────────────────────────────────────────────────────────────

/// Single-precision biquad, Direct Form II Transposed. Coefficients are
/// normalised (a₀ = 1).
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    s: [f32; 2],
}

impl Biquad {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[0] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    fn reset(&mut self) {
        self.s = [0.0; 2];
    }
}

// ─── K-Weighting ────────────────────────────────────────────────────────────

/// BS.1770-4 K-weighting: high-shelf pre-filter → high-pass (RLB).
///
/// Parameters from EBU Tech 3341 / ITU-R BS.1770-4 Annex 1.
/// Coefficients are derived analytically via the bilinear transform at the
/// given sample rate and are valid for all standard rates.
#[derive(Clone, Copy)]
struct KWeighting {
    /// High-shelf pre-filter, one instance per channel.
    s1: [Biquad; 2],
    /// High-pass (RLB weighting), one instance per channel.
    s2: [Biquad; 2],
}

impl KWeighting {
    fn new(sample_rate: u32) -> Self {
        use std::f64::consts::PI;
        let fs = sample_rate as f64;

        // ── Stage 1: high-shelf pre-filter ──────────────────────────────────
        // Models the acoustic effect of the head.
        // f₀ = 1681.97 Hz, G = +4 dB, Q ≈ 0.707
        let k = (PI * 1681.974_450_955_533_f64 / fs).tan();
        let vh = 10.0_f64.powf(3.999_843_853_973_347 / 20.0);
        let vb = 10.0_f64.powf(3.999_843_853_973_347 / 40.0);
        let q = 0.707_175_236_955_419_6_f64;
        let a0 = 1.0 + k / q + k * k;
        let s1_b = [
            ((vh + vb * k / q + k * k) / a0) as f32,
            (2.0 * (k * k - vh) / a0) as f32,
            ((vh - vb * k / q + k * k) / a0) as f32,
        ];
        let s1_a = [
            (2.0 * (k * k - 1.0) / a0) as f32,
            ((1.0 - k / q + k * k) / a0) as f32,
        ];

        // ── Stage 2: high-pass (RLB weighting) ──────────────────────────────
        // Removes low-frequency content.
        // f₀ = 38.14 Hz, Q ≈ 0.5003
        let k = (PI * 38.135_470_876_024_44_f64 / fs).tan();
        let q = 0.500_327_037_323_877_3_f64;
        let a0 = 1.0 + k / q + k * k;
        let s2_b = [
            (1.0 / a0) as f32,
            (-2.0 / a0) as f32,
            (1.0 / a0) as f32,
        ];
        let s2_a = [
            (2.0 * (k * k - 1.0) / a0) as f32,
            ((1.0 - k / q + k * k) / a0) as f32,
        ];

        let bq = |b: [f32; 3], a: [f32; 2]| Biquad { b, a, s: [0.0; 2] };
        Self {
            s1: [bq(s1_b, s1_a), bq(s1_b, s1_a)],
            s2: [bq(s2_b, s2_a), bq(s2_b, s2_a)],
        }
    }

    #[inline]
    fn process(&mut self, sample: f32, ch: usize) -> f32 {
        self.s2[ch].process(self.s1[ch].process(sample))
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            self.s1[ch].reset();
            self.s2[ch].reset();
        }
    }
}

// ─── LufsMeter ──────────────────────────────────────────────────────────────

pub struct LufsMeter {
    /// Stereo-frame count per 400ms block.
    block_samples: usize,
    /// Stereo-frame count per 100ms hop.
    hop_samples: usize,

    // ── Momentary ────────────────────────────────────────────────────────────
    /// Sliding window of per-frame mean-sq values; capacity = block_samples.
    sliding_buf: VecDeque<f32>,

    // ── Short-term ───────────────────────────────────────────────────────────
    /// Mean-sq energy accumulated over the current 100ms hop.
    hop_energy_acc: f32,
    /// Per-hop energies for the 3s short-term window (last SHORT_TERM_HOPS).
    short_term_energies: VecDeque<f32>,

    /// Frames accumulated since the last hop boundary.
    frames_since_hop: usize,

    // ── Integrated ───────────────────────────────────────────────────────────
    /// Mean-sq energies of blocks that passed the absolute gate. Used for
    /// both passes of the BS.1770-4 integrated loudness algorithm.
    abs_gated_energies: Vec<f32>,
    integrated_loudness: f32,

    k_weighting: KWeighting,
    channel_weights: [f32; 2],

    /// Maximum absolute sample value seen (not BS.1770-4 true-peak).
    sample_peak: f32,
    lufs_offset: f32,
    target_lufs: f32,
}

impl LufsMeter {
    pub fn new(sample_rate: u32) -> Self {
        let block_samples = (BLOCK_MS / 1000.0 * sample_rate as f32).round() as usize;
        let hop_samples = (HOP_MS / 1000.0 * sample_rate as f32).round() as usize;

        Self {
            block_samples,
            hop_samples,
            sliding_buf: VecDeque::with_capacity(block_samples),
            hop_energy_acc: 0.0,
            short_term_energies: VecDeque::with_capacity(SHORT_TERM_HOPS + 1),
            frames_since_hop: 0,
            abs_gated_energies: Vec::new(),
            integrated_loudness: ABSOLUTE_GATE,
            k_weighting: KWeighting::new(sample_rate),
            channel_weights: [1.0, 1.0],
            sample_peak: 0.0,
            lufs_offset: 0.0,
            target_lufs: -14.0,
        }
    }

    pub fn set_target_lufs(&mut self, target: f32) {
        self.target_lufs = target.clamp(-70.0, 0.0);
    }

    pub fn set_channel_weights(&mut self, left: f32, right: f32) {
        self.channel_weights = [left, right];
    }

    pub fn process(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(2) {
            let l = self.k_weighting.process(frame[0], 0) * self.channel_weights[0];
            let r = self.k_weighting.process(frame[1], 1) * self.channel_weights[1];

            let msq = (l * l + r * r) * 0.5;

            // Momentary: maintain 400ms sliding window.
            if self.sliding_buf.len() == self.block_samples {
                self.sliding_buf.pop_front();
            }
            self.sliding_buf.push_back(msq);

            // Short-term: accumulate current 100ms hop.
            self.hop_energy_acc += msq;

            // Sample peak (not true-peak).
            let pk = frame[0].abs().max(frame[1].abs());
            if pk > self.sample_peak {
                self.sample_peak = pk;
            }

            self.frames_since_hop += 1;
            if self.frames_since_hop >= self.hop_samples {
                self.frames_since_hop = 0;
                self.on_hop();
            }
        }
        // Odd trailing sample (non-stereo remainder) is ignored.
    }

    fn on_hop(&mut self) {
        // ── Short-term: record 100ms hop energy ──────────────────────────────
        let hop_energy = self.hop_energy_acc / self.hop_samples as f32;
        self.hop_energy_acc = 0.0;
        if self.short_term_energies.len() == SHORT_TERM_HOPS {
            self.short_term_energies.pop_front();
        }
        self.short_term_energies.push_back(hop_energy);

        // ── Momentary / integrated: requires a full 400ms block ───────────────
        if self.sliding_buf.len() < self.block_samples {
            return;
        }

        let block_energy = self.sliding_buf.iter().sum::<f32>() / self.block_samples as f32;
        let block_loudness = energy_to_lufs(block_energy);

        // Two-pass integrated loudness gating.
        if block_loudness > ABSOLUTE_GATE {
            self.abs_gated_energies.push(block_energy);
            self.update_integrated();
        }

        self.lufs_offset = self.target_lufs - self.integrated_loudness;
    }

    /// Recompute integrated loudness using two-pass BS.1770-4 gating.
    ///
    /// Pass 1 — preliminary integrated: mean energy of all absolute-gated blocks.
    /// Pass 2 — final integrated: mean energy of blocks above the relative gate,
    ///          where relative gate = max(−70, preliminary − 10 LU).
    fn update_integrated(&mut self) {
        let energies = &self.abs_gated_energies;

        // Pass 1.
        let prelim_energy = energies.iter().sum::<f32>() / energies.len() as f32;
        let prelim_lufs = energy_to_lufs(prelim_energy);

        // Convert the relative gate to an energy threshold (avoids per-block
        // energy_to_lufs calls: e > 10^((L + 0.691) / 10) ↔ energy_to_lufs(e) > L).
        let rel_gate_lufs = (prelim_lufs - RELATIVE_GATE_OFFSET).max(ABSOLUTE_GATE);
        let rel_gate_energy = 10.0_f32.powf((rel_gate_lufs + 0.691) / 10.0);

        // Pass 2.
        let mut sum = 0.0_f32;
        let mut count = 0_usize;
        for &e in energies {
            if e > rel_gate_energy {
                sum += e;
                count += 1;
            }
        }

        if count > 0 {
            self.integrated_loudness = energy_to_lufs(sum / count as f32);
        }
    }

    /// Momentary loudness: mean-sq energy of the current 400ms sliding window.
    /// Returns `ABSOLUTE_GATE` (−70 LUFS) until the window is fully populated.
    pub fn momentary(&self) -> f32 {
        if self.sliding_buf.len() < self.block_samples {
            return ABSOLUTE_GATE;
        }
        energy_to_lufs(self.sliding_buf.iter().sum::<f32>() / self.block_samples as f32)
    }

    /// Short-term loudness: mean energy of the last 30 × 100ms non-overlapping hops
    /// (3s window). Returns `ABSOLUTE_GATE` when no hop data is available yet.
    pub fn short_term(&self) -> f32 {
        if self.short_term_energies.is_empty() {
            return ABSOLUTE_GATE;
        }
        energy_to_lufs(
            self.short_term_energies.iter().sum::<f32>()
                / self.short_term_energies.len() as f32,
        )
    }

    pub fn integrated(&self) -> f32 {
        self.integrated_loudness
    }

    /// Sample-peak in dBFS. **Not** BS.1770-4 true-peak (which requires 4×
    /// oversampled interpolation); this is the maximum absolute sample value seen.
    pub fn true_peak_db(&self) -> f32 {
        if self.sample_peak > 0.0 {
            20.0 * self.sample_peak.log10()
        } else {
            -100.0
        }
    }

    pub fn gain_db(&self) -> f32 {
        self.lufs_offset
    }

    pub fn reset(&mut self) {
        self.sliding_buf.clear();
        self.hop_energy_acc = 0.0;
        self.short_term_energies.clear();
        self.frames_since_hop = 0;
        self.abs_gated_energies.clear();
        self.k_weighting.reset();
        self.integrated_loudness = ABSOLUTE_GATE;
        self.sample_peak = 0.0;
        self.lufs_offset = 0.0;
    }
}

// ─── LufsNormalizer ──────────────────────────────────────────────────────────

pub struct LufsNormalizer {
    meter: LufsMeter,
    max_gain_db: f32,
    attack_coef: f32,
    release_coef: f32,
    current_gain: f32,
}

impl LufsNormalizer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            meter: LufsMeter::new(sample_rate),
            max_gain_db: 12.0,
            attack_coef: 0.9,
            release_coef: 0.999,
            current_gain: 1.0,
        }
    }

    pub fn set_target_lufs(&mut self, target: f32) {
        self.meter.set_target_lufs(target);
    }

    pub fn set_max_gain(&mut self, max_db: f32) {
        self.max_gain_db = max_db.clamp(0.0, 24.0);
    }

    pub fn process<'a>(&mut self, samples: &'a mut [f32]) -> &'a [f32] {
        self.meter.process(samples);

        let target_gain_db = self
            .meter
            .gain_db()
            .clamp(-self.max_gain_db, self.max_gain_db);
        let target_gain = 10.0_f32.powf(target_gain_db / 20.0);

        if target_gain < self.current_gain {
            // Gain reduction: fast attack.
            self.current_gain =
                self.current_gain * self.attack_coef + target_gain * (1.0 - self.attack_coef);
        } else {
            // Gain recovery: slow release.
            self.current_gain =
                self.current_gain * self.release_coef + target_gain * (1.0 - self.release_coef);
        }

        for s in samples.iter_mut() {
            *s *= self.current_gain;
        }

        samples
    }

    pub fn momentary(&self) -> f32 {
        self.meter.momentary()
    }

    pub fn short_term(&self) -> f32 {
        self.meter.short_term()
    }

    pub fn integrated(&self) -> f32 {
        self.meter.integrated()
    }

    pub fn true_peak_db(&self) -> f32 {
        self.meter.true_peak_db()
    }

    pub fn current_gain_db(&self) -> f32 {
        20.0 * self.current_gain.log10()
    }

    pub fn reset(&mut self) {
        self.meter.reset();
        self.current_gain = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn make_sine(freq: f32, duration_ms: u32, sample_rate: u32, amplitude: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * duration_ms as f32 / 1000.0).round() as usize;
        (0..n)
            .flat_map(|i| {
                let s = (2.0 * PI * freq * i as f32 / sample_rate as f32).sin() * amplitude;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn initial_state_is_absolute_gate() {
        let meter = LufsMeter::new(48000);
        assert!(
            (meter.momentary() - ABSOLUTE_GATE).abs() < 0.1,
            "momentary before data should be {ABSOLUTE_GATE}"
        );
        assert!(
            (meter.short_term() - ABSOLUTE_GATE).abs() < 0.1,
            "short-term before data should be {ABSOLUTE_GATE}"
        );
        assert!(
            (meter.integrated() - ABSOLUTE_GATE).abs() < 0.1,
            "integrated before data should be {ABSOLUTE_GATE}"
        );
    }

    #[test]
    fn k_weighting_rejects_dc() {
        // The high-pass (RLB) stage must block DC.
        let mut kw = KWeighting::new(48000);
        let output: Vec<f32> = (0..48000).map(|_| kw.process(1.0, 0)).collect();
        // After the filter settles (~a few thousand samples), output must be near zero.
        let tail_mean: f32 = output[40000..].iter().sum::<f32>() / 8000.0;
        assert!(
            tail_mean.abs() < 0.01,
            "K-weighting must reject DC (settled mean = {tail_mean})"
        );
    }

    #[test]
    fn k_weighting_passes_1khz() {
        // 1kHz is well above the high-pass pole (~38 Hz) and at the shelf boost.
        // The output amplitude should be larger than the input (≥ 0 dB net).
        let mut kw = KWeighting::new(48000);
        let input: Vec<f32> = (0..48000)
            .map(|i| (2.0 * PI * 1000.0 * i as f32 / 48000.0).sin())
            .collect();
        let output: Vec<f32> = input.iter().map(|&x| kw.process(x, 0)).collect();
        let in_rms: f32 = (input[4800..].iter().map(|x| x * x).sum::<f32>() / 43200.0).sqrt();
        let out_rms: f32 = (output[4800..].iter().map(|x| x * x).sum::<f32>() / 43200.0).sqrt();
        assert!(
            out_rms >= in_rms * 0.9,
            "K-weighting should pass 1kHz (in_rms={in_rms:.4} out_rms={out_rms:.4})"
        );
    }

    #[test]
    fn meter_responds_to_1khz_sine() {
        let sr = 48000;
        let mut meter = LufsMeter::new(sr);
        // 2s to fully populate sliding buf and accumulate integrated blocks.
        let signal = make_sine(1000.0, 2000, sr, 0.1);
        meter.process(&signal);

        let momentary = meter.momentary();
        assert!(
            momentary > -40.0 && momentary < -10.0,
            "1kHz sine at 0.1 amplitude: expected momentary in (-40, -10), got {momentary}"
        );

        let integrated = meter.integrated();
        assert!(
            integrated > -40.0 && integrated < -10.0,
            "1kHz sine at 0.1 amplitude: expected integrated in (-40, -10), got {integrated}"
        );
    }

    #[test]
    fn gating_excludes_silent_blocks() {
        let sr = 48000;
        let mut meter = LufsMeter::new(sr);

        // 2s of signal → builds up integrated loudness.
        meter.process(&make_sine(1000.0, 2000, sr, 0.1));
        let integrated_after_signal = meter.integrated();
        assert!(
            integrated_after_signal > ABSOLUTE_GATE,
            "integrated should be above -70 after signal"
        );

        // 2s of silence. Blocks fall below absolute gate and are excluded.
        meter.process(&vec![0.0_f32; sr as usize * 4]); // *4 for stereo × 2s

        let integrated_after_silence = meter.integrated();
        assert!(
            (integrated_after_signal - integrated_after_silence).abs() < 0.5,
            "silence must not shift integrated loudness \
             (before={integrated_after_signal:.2} after={integrated_after_silence:.2})"
        );
    }

    #[test]
    fn short_term_reflects_recent_energy() {
        let sr = 48000;
        let mut meter = LufsMeter::new(sr);

        // Populate 3s of short-term window.
        meter.process(&make_sine(1000.0, 3500, sr, 0.1));

        let st = meter.short_term();
        assert!(
            st > -40.0 && st < -10.0,
            "short-term after 3.5s of signal should be in (-40, -10), got {st}"
        );
    }

    #[test]
    fn reset_clears_all_state() {
        let sr = 48000;
        let mut meter = LufsMeter::new(sr);

        meter.process(&make_sine(1000.0, 2000, sr, 0.1));
        assert!(meter.integrated() > ABSOLUTE_GATE, "must have loudness data before reset");

        meter.reset();

        assert!(
            (meter.momentary() - ABSOLUTE_GATE).abs() < 0.1,
            "momentary must be reset"
        );
        assert!(
            (meter.integrated() - ABSOLUTE_GATE).abs() < 0.1,
            "integrated must be reset"
        );
        assert!(
            meter.short_term() <= ABSOLUTE_GATE + 0.1,
            "short-term must be reset"
        );
    }

    #[test]
    fn normalizer_gain_within_max_gain_db() {
        let sr = 48000;
        let mut norm = LufsNormalizer::new(sr);
        norm.set_target_lufs(-14.0);
        norm.set_max_gain(6.0);

        let mut signal = make_sine(1000.0, 2000, sr, 0.1);
        norm.process(&mut signal);

        let gain_db = norm.current_gain_db();
        assert!(
            gain_db >= -6.1 && gain_db <= 6.1,
            "gain must stay within ±6dB, got {gain_db}"
        );
    }

    #[test]
    fn normalizer_reset_restores_unity_gain() {
        let sr = 48000;
        let mut norm = LufsNormalizer::new(sr);
        let mut signal = make_sine(1000.0, 2000, sr, 0.1);
        norm.process(&mut signal);

        norm.reset();
        assert!(
            norm.current_gain_db().abs() < 0.01,
            "gain must be 0dB after reset, got {}",
            norm.current_gain_db()
        );
    }
}
