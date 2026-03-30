//! BS2B headphone crossfeed filter.
//!
//! First-order IIR implementation of the Bauer stereophonic-to-binaural
//! algorithm. Blends a low-pass-filtered portion of each channel into the
//! opposite channel to reduce headphone fatigue on hard-panned content.

pub struct CrossfeedFilter {
    feed_level: f32,
    cutoff_hz: f32,
    sample_rate: u32,
    alpha: f32,
    norm: f32,
    z_l: f32,
    z_r: f32,
}

use crate::dsp::config::DspConfig;
use std::f32::consts::PI;

impl CrossfeedFilter {
    pub fn new(feed_level: f32, cutoff_hz: f32) -> Self {
        let mut f = Self {
            feed_level,
            cutoff_hz,
            sample_rate: 0, // triggers recompute on first process() call
            alpha: 0.0,
            norm: 0.0,
            z_l: 0.0,
            z_r: 0.0,
        };
        f.recompute(44100); // nominal value so struct is always valid
        f
    }

    fn recompute(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.alpha = (-2.0 * PI * self.cutoff_hz / sample_rate as f32).exp();
        self.norm = 1.0 / (1.0 + self.feed_level);
        self.z_l = 0.0;
        self.z_r = 0.0;
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
        self.cutoff_hz = cutoff_hz;
        self.recompute(self.sample_rate);
    }

    pub fn reset(&mut self) {
        self.z_l = 0.0;
        self.z_r = 0.0;
    }
}

/// Returns true when the configured output device name contains a headphone keyword.
/// Defaults to false (crossfeed stays OFF) when no keyword is found or the target
/// is not ALSA or PipeWire.
pub(crate) fn probe_headphones(config: &DspConfig) -> bool {
    use crate::dsp::config::OutputTarget;
    let haystack = match config.output_target {
        OutputTarget::Alsa => config.alsa_device.as_deref().unwrap_or(""),
        OutputTarget::PipeWire => &config.pipewire_role,
        _ => return false,
    };
    let h = haystack.to_lowercase();
    h.contains("headphone") || h.contains("headset") || h.contains("earphone")
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

    // All-zeros input must produce near-zero output (denormal guard adds tiny 1e-25 offset).
    #[test]
    fn silence_in_silence_out() {
        let mut f = CrossfeedFilter::new(0.45, 700.0);
        let silence = vec![0.0_f32; 1024];
        let out = f.process(&silence, 44100);
        assert_eq!(out.len(), 1024);
        assert!(
            out.iter().all(|&s| s.abs() < 1e-20),
            "silence in must produce near-silence out"
        );
    }

    // feed_level=0.0 → output must equal input exactly.
    // norm=1.0 and crossfeed term is multiplied by 0.0, so even the 1e-25 guard
    // on z_l/z_r never reaches out_L/out_R when feed=0, so strict equality holds.
    #[test]
    fn feed_zero_is_passthrough() {
        let mut f = CrossfeedFilter::new(0.0, 700.0);
        let input: Vec<f32> = (0..64)
            .map(|i| i as f32 * 0.01)
            .flat_map(|v| [v, -v])
            .collect();
        let out = f.process(&input, 44100);
        assert_eq!(out.len(), input.len());
        for (a, b) in input.iter().zip(out.iter()) {
            assert_eq!(a, b, "feed=0 must be exact passthrough");
        }
    }

    // feed_level=0.9 with 100 Hz sine (well below 700 Hz cutoff): RMS(out) / RMS(in) must be within 5% of 1.0.
    // Normalisation is exact at DC; 100 Hz is below the cutoff so the LP filter passes most energy,
    // and norm = 1/(1+feed) ensures the combined output stays near unity gain.
    #[test]
    fn feed_max_energy_preserved() {
        let sr = 44100_u32;
        let input = sine_stereo(100.0, sr, sr as usize); // 1 second at 100 Hz (below 700 Hz cutoff)
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
        // After rate change the first output frame should start from fresh (zeroed) state.
        let out2 = f.process(&input, 96000);
        assert_eq!(out2.len(), input.len());
        // z_r is reset to 0 by recompute(), then updated within the first frame before out_l:
        //   z_r_new = (1 - alpha) * in_r + alpha * 0 + 1e-25
        // So out_l = norm * (in_L + feed * z_r_new), not norm * in_L.
        // The key property: output must be close to norm*in_L (within ~2% because alpha≈0.955).
        let norm = 1.0 / (1.0 + 0.45_f32);
        let approx_direct = norm * 0.5_f32;
        assert!(
            (out2[0] - approx_direct).abs() < 0.02,
            "first out_L after rate change: got {}, expected near {approx_direct} (within 2%)",
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
        assert!(
            out.iter().all(|s| s.is_finite()),
            "output must be finite after near-zero input"
        );
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

        assert!(probe_headphones(&make("hw:Headphone")), "headphone keyword");
        assert!(probe_headphones(&make("hw:Headset,0")), "headset keyword");
        assert!(probe_headphones(&make("hw:earphone")), "earphone keyword");
        assert!(probe_headphones(&make("hw:HEADPHONE")), "case-insensitive");
        assert!(!probe_headphones(&make("hw:Generic")), "no keyword → false");
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
