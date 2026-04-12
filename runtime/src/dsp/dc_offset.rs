//! DC offset (high-pass) filter.
//!
//! First-order IIR high-pass filter to remove DC offset and very low frequency
//! drift from audio signals. DC offset can cause speaker cone displacement and
//! reduce headroom.
//!
//! Typical cutoff: 5-20 Hz. Higher cutoffs like 80 Hz also remove rumble.

pub struct DcOffsetFilter {
    cutoff_hz: f32,
    sample_rate: u32,
    /// IIR coefficient for the one-pole low-pass estimate.
    /// The HPF output is `y = x - z`, where z tracks the low-frequency content.
    alpha: f32,
    /// `1.0 - alpha`, precomputed to avoid per-sample subtraction.
    one_minus_alpha: f32,
    z_l: f32,
    z_r: f32,
}

use std::f32::consts::PI;

impl DcOffsetFilter {
    /// Default cutoff frequency in Hz for DC offset removal.
    /// Typical values: 5-20 Hz for DC removal, 80 Hz also removes rumble.
    #[allow(dead_code)] // planned: DSP configuration helper
    pub fn default_cutoff_hz() -> f32 {
        10.0 // Standard for DC offset removal
    }

    pub fn new(cutoff_hz: f32) -> Self {
        let mut f = Self {
            cutoff_hz,
            sample_rate: 0,
            alpha: 0.0,
            one_minus_alpha: 0.0,
            z_l: 0.0,
            z_r: 0.0,
        };
        f.recompute(44100);
        f
    }

    fn recompute(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        let rc = 1.0 / (2.0 * PI * self.cutoff_hz);
        let dt = 1.0 / sample_rate as f32;
        self.alpha = rc / (rc + dt);
        self.one_minus_alpha = 1.0 - self.alpha;
        // State (z_l, z_r) is preserved across sample rate changes to avoid clicks.
    }

    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if sample_rate != self.sample_rate {
            self.recompute(sample_rate);
        }

        let alpha = self.alpha;
        let beta = self.one_minus_alpha;
        let mut out = Vec::with_capacity(samples.len());
        let mut iter = samples.chunks_exact(2);

        for frame in iter.by_ref() {
            let in_l = frame[0];
            let in_r = frame[1];

            // One-pole LPF: z tracks DC/LF content. y = x - z removes it.
            // Correct formula: z[n] = α*z[n-1] + (1-α)*x[n]  (DC gain of LPF = 1)
            self.z_l = alpha * self.z_l + beta * in_l;
            self.z_r = alpha * self.z_r + beta * in_r;

            let out_l = in_l - self.z_l;
            let out_r = in_r - self.z_r;

            out.push(out_l);
            out.push(out_r);
        }

        for &s in iter.remainder() {
            out.push(s);
        }
        out
    }

    #[allow(dead_code)] // pub API: used by DcOffsetNode
    pub fn process_mono(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if sample_rate != self.sample_rate {
            self.recompute(sample_rate);
        }

        let alpha = self.alpha;
        let beta = self.one_minus_alpha;
        let mut out = Vec::with_capacity(samples.len());

        for &in_s in samples {
            self.z_l = alpha * self.z_l + beta * in_s;
            let out_s = in_s - self.z_l;
            out.push(out_s);
        }
        out
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        self.cutoff_hz = cutoff_hz;
        self.recompute(self.sample_rate);
    }

    /// Reset filter state to prevent audio artifacts at track boundaries.
    #[allow(dead_code)] // planned: called on seek events when DSP pipeline handles seeks
    pub fn reset(&mut self) {
        self.z_l = 0.0;
        self.z_r = 0.0;
    }

    pub fn cutoff(&self) -> f32 {
        self.cutoff_hz
    }
}

impl Default for DcOffsetFilter {
    fn default() -> Self {
        Self::new(10.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    #[test]
    fn dc_component_removed() {
        let mut f = DcOffsetFilter::new(10.0);
        let sr = 44100_u32;
        let dc_offset = 0.5_f32;
        let signal: Vec<f32> = (0..sr as usize)
            .flat_map(|i| {
                let s = (2.0 * PI * 440.0 * i as f32 / sr as f32).sin() * 0.5;
                [s + dc_offset, s + dc_offset]
            })
            .collect();

        // Pre-warm the filter to steady state with one pass, then measure the second.
        f.process(&signal, sr);
        let out = f.process(&signal, sr);

        let avg: f32 = out.iter().sum::<f32>() / out.len() as f32;
        assert!(
            avg.abs() < 0.001,
            "DC offset should be removed after settling, got average {}",
            avg
        );
    }

    #[test]
    fn dc_gain_is_zero_at_steady_state() {
        // Verify the theoretical steady-state DC gain is zero.
        // For z[n] = α*z[n-1] + (1-α)*x[n], y = x-z, steady-state DC output = 0.
        let mut f = DcOffsetFilter::new(10.0);
        let sr = 44100_u32;
        let dc = 0.8_f32;
        let signal = vec![dc; sr as usize * 2]; // 1s of DC on both channels

        // Run long enough to fully settle (>>5τ, τ ≈ 666 samples at 44100 Hz).
        f.process(&signal, sr);
        f.process(&signal, sr);
        f.process(&signal, sr);
        let out = f.process(&signal, sr);

        let avg: f32 = out.iter().sum::<f32>() / out.len() as f32;
        assert!(
            avg.abs() < 1e-4,
            "settled DC should be rejected, got {}",
            avg
        );
    }

    #[test]
    fn audio_signal_preserved() {
        let mut f = DcOffsetFilter::new(10.0);
        let sr = 44100_u32;
        let signal: Vec<f32> = (0..sr as usize)
            .flat_map(|i| {
                let s = (2.0 * PI * 1000.0 * i as f32 / sr as f32).sin() * 0.7;
                [s, s]
            })
            .collect();

        let out = f.process(&signal, sr);
        let in_rms = rms(&signal);
        let out_rms = rms(&out);
        let ratio = out_rms / in_rms;

        assert!(
            (ratio - 1.0).abs() < 0.1,
            "Audio RMS should be preserved, ratio={:.4}",
            ratio
        );
    }

    #[test]
    fn sample_rate_change_recomputes() {
        let mut f = DcOffsetFilter::new(10.0);
        let input = vec![0.1_f32, -0.1_f32, 0.2_f32, -0.2_f32];
        let _ = f.process(&input, 44100);
        let out2 = f.process(&input, 96000);
        assert_eq!(out2.len(), input.len());
        assert!(out2.iter().all(|s| s.is_finite()));
    }
}
