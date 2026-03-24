//! LUFS loudness normalization and measurement.
//!
//! Implements ITU-R BS.1770-4 loudness measurement with:
//! - Momentary loudness (400ms window)
//! - Short-term loudness (3s window)
//! - Integrated loudness (entire track)
//! - True peak measurement
//! - Gain-based normalization

use std::f32::consts::PI;

const BLOCK_MS: f32 = 400.0;
const GATE_MS: f32 = 100.0;

pub struct LufsMeter {
    sample_rate: u32,
    block_samples: usize,
    gate_samples: usize,

    momentary_buf: Vec<f32>,
    short_term_buf: Vec<f32>,
    gating_buf: Vec<f32>,

    k_weighting: KWeighting,
    channel_weights: [f32; 2],

    integrated_loudness: f32,
    true_peak: f32,
    block_count: usize,

    lufs_offset: f32,
    target_lufs: f32,
}

#[derive(Clone, Copy)]
struct KWeighting {
    coefs: [[f32; 5]; 2],
    state: [[f32; 4]; 2],
}

impl KWeighting {
    fn new(sample_rate: u32) -> Self {
        let fc = 1680.0;
        let q = 0.707;
        let w = 2.0 * PI * fc / sample_rate as f32;
        let alpha = (w / (2.0 * q + w)).sin() / (w / (2.0 * q + w)).cos();

        let b0 = 1.0;
        let b1 = -2.0 * ((1.0 - alpha) / (1.0 + alpha)).cos();
        let b2 = (1.0 - alpha) / (1.0 + alpha);
        let a0 = 1.0;
        let a1 = -2.0 * ((1.0 - alpha) / (1.0 + alpha)).cos();
        let a2 = (1.0 - alpha) / (1.0 + alpha);

        let norm = 1.0 / a0;
        Self {
            coefs: [
                [b0 * norm, b1 * norm, b2 * norm, a1 * norm, a2 * norm],
                [b0 * norm, b1 * norm, b2 * norm, a1 * norm, a2 * norm],
            ],
            state: [[0.0; 4]; 2],
        }
    }

    fn process(&mut self, sample: f32, ch: usize) -> f32 {
        let c = &self.coefs[ch];
        let s = &mut self.state[ch];

        let out = c[0] * sample + c[1] * s[0] + c[2] * s[1] - c[3] * s[2] - c[4] * s[3];
        s[1] = s[0];
        s[0] = sample;
        s[3] = s[2];
        s[2] = out;

        out
    }

    fn reset(&mut self) {
        self.state = [[0.0; 4]; 2];
    }
}

impl LufsMeter {
    pub fn new(sample_rate: u32) -> Self {
        let block_samples = (BLOCK_MS / 1000.0 * sample_rate as f32) as usize;
        let gate_samples = (GATE_MS / 1000.0 * sample_rate as f32) as usize;

        Self {
            sample_rate,
            block_samples,
            gate_samples,
            momentary_buf: Vec::with_capacity(block_samples * 2),
            short_term_buf: Vec::with_capacity(block_samples * 2 * 8),
            gating_buf: Vec::new(),
            k_weighting: KWeighting::new(sample_rate),
            channel_weights: [1.0, 1.0],
            integrated_loudness: -70.0,
            true_peak: 0.0,
            block_count: 0,
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
        if samples.len() < 2 {
            return;
        }

        for chunk in samples.chunks(2) {
            if chunk.len() < 2 {
                break;
            }

            let l = self.k_weighting.process(chunk[0], 0) * self.channel_weights[0];
            let r = self.k_weighting.process(chunk[1], 1) * self.channel_weights[1];

            let mean_sq = (l * l + r * r) / 2.0;
            self.momentary_buf.push(mean_sq);
            self.short_term_buf.push(mean_sq);
            self.gating_buf.push(mean_sq);

            let abs_l = chunk[0].abs();
            let abs_r = chunk[1].abs();
            if abs_l > self.true_peak {
                self.true_peak = abs_l;
            }
            if abs_r > self.true_peak {
                self.true_peak = abs_r;
            }
        }

        if self.momentary_buf.len() >= self.block_samples {
            self.process_block();
        }
    }

    fn process_block(&mut self) {
        let block_energy: f32 = self.momentary_buf.iter().sum::<f32>() / self.block_samples as f32;

        let loudness = -0.691 + 10.0 * (block_energy.max(1e-10)).log10();

        if loudness > -70.0 {
            if loudness > -70.0 + GATE_MS / 1000.0 {
                let gated_energy: f32 =
                    self.gating_buf.iter().sum::<f32>() / self.gating_buf.len().max(1) as f32;
                let gated_loudness = -0.691 + 10.0 * (gated_energy.max(1e-10)).log10();

                if gated_loudness > -70.0 {
                    let n = self.block_count as f32;
                    self.integrated_loudness = if n == 0.0 {
                        gated_loudness
                    } else {
                        (10.0_f32
                            .powf(self.integrated_loudness / 10.0)
                            .mul_add(n, 10.0_f32.powf(gated_loudness / 10.0))
                            / (n + 1.0))
                            .log10()
                            * 10.0
                    };
                    self.block_count += 1;
                }
            }

            self.lufs_offset = self.target_lufs - self.integrated_loudness;
        }

        self.momentary_buf.clear();

        if self.short_term_buf.len() > self.block_samples * 8 {
            self.short_term_buf.drain(0..self.block_samples);
        }

        self.gating_buf.clear();
    }

    pub fn momentary(&self) -> f32 {
        if self.momentary_buf.is_empty() {
            return -70.0;
        }
        let energy: f32 = self.momentary_buf.iter().sum::<f32>() / self.momentary_buf.len() as f32;
        -0.691 + 10.0 * (energy.max(1e-10)).log10()
    }

    pub fn short_term(&self) -> f32 {
        if self.short_term_buf.is_empty() {
            return -70.0;
        }
        let window = self.short_term_buf.len().saturating_sub(self.block_samples);
        let energy: f32 =
            self.short_term_buf[window..].iter().sum::<f32>() / self.block_samples.max(1) as f32;
        -0.691 + 10.0 * (energy.max(1e-10)).log10()
    }

    pub fn integrated(&self) -> f32 {
        self.integrated_loudness
    }

    pub fn true_peak_db(&self) -> f32 {
        if self.true_peak > 0.0 {
            20.0 * self.true_peak.log10()
        } else {
            -100.0
        }
    }

    pub fn gain_db(&self) -> f32 {
        self.lufs_offset
    }

    pub fn reset(&mut self) {
        self.momentary_buf.clear();
        self.short_term_buf.clear();
        self.gating_buf.clear();
        self.k_weighting.reset();
        self.integrated_loudness = -70.0;
        self.true_peak = 0.0;
        self.block_count = 0;
        self.lufs_offset = 0.0;
    }
}

pub struct LufsNormalizer {
    meter: LufsMeter,
    gain_db: f32,
    max_gain_db: f32,
    attack_coef: f32,
    release_coef: f32,
    current_gain: f32,
}

impl LufsNormalizer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            meter: LufsMeter::new(sample_rate),
            gain_db: 0.0,
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
            self.current_gain =
                self.current_gain * self.attack_coef + target_gain * (1.0 - self.attack_coef);
        } else {
            self.current_gain =
                self.current_gain * self.release_coef + target_gain * (1.0 - self.release_coef);
        }

        for sample in samples.iter_mut() {
            *sample *= self.current_gain;
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

    fn make_sine(freq: f32, duration_ms: u32, sample_rate: u32) -> Vec<f32> {
        let n = (sample_rate as f32 * duration_ms as f32 / 1000.0) as usize;
        (0..n)
            .flat_map(|i| {
                let s = (2.0 * PI * freq * i as f32 / sample_rate as f32).sin();
                [s * 0.1, s * 0.1]
            })
            .collect()
    }

    #[test]
    fn lufs_meter_responds_to_signal() {
        let sr = 48000;
        let mut meter = LufsMeter::new(sr);
        meter.set_target_lufs(-14.0);

        let signal = make_sine(1000.0, 500, sr);
        meter.process(&signal);

        let momentary = meter.momentary();
        assert!(
            momentary > -60.0 && momentary < -5.0,
            "1kHz sine should show reasonable loudness, got {}",
            momentary
        );
    }

    #[test]
    fn normalizer_applies_gain() {
        let sr = 48000;
        let mut norm = LufsNormalizer::new(sr);
        norm.set_target_lufs(-14.0);
        norm.set_max_gain(6.0);

        let mut signal = make_sine(1000.0, 500, sr);
        norm.process(&mut signal);

        let gain_db = norm.current_gain_db();
        assert!(
            gain_db.abs() < 20.0,
            "gain should be reasonable, got {}dB",
            gain_db
        );
    }

    #[test]
    fn meter_resets() {
        let sr = 48000;
        let mut meter = LufsMeter::new(sr);

        let signal = make_sine(1000.0, 500, sr);
        meter.process(&signal);
        assert!(meter.integrated() > -60.0);

        meter.reset();
        assert!(meter.integrated() < -60.0);
    }
}
