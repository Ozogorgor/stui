//! High-quality audio resampler using rubato library.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::config::{DspConfig, FilterType};

/// Resampler using high-quality FFT-based algorithm.
pub struct Resampler {
    config: Arc<RwLock<DspConfig>>,
    input_rate: u32,
    output_rate: u32,
    chunk_size: usize,
}

impl Resampler {
    /// Create a new resampler with the given configuration.
    pub fn new(config: Arc<RwLock<DspConfig>>) -> Result<Self, String> {
        let cfg = config.blocking_read();
        let output_rate = cfg.output_sample_rate;
        let chunk_size = cfg.buffer_size;
        let input_rate = cfg.input_sample_rate;

        Self::validate_rates(input_rate, output_rate)?;

        info!(
            input = input_rate,
            output = output_rate,
            "resampler initialized"
        );

        Ok(Self {
            config: Arc::clone(&config),
            input_rate,
            output_rate,
            chunk_size,
        })
    }

    fn validate_rates(input: u32, output: u32) -> Result<(), String> {
        if input == 0 || output == 0 {
            return Err("Sample rates must be non-zero".to_string());
        }
        if output % input != 0 && input % output != 0 {
            warn!(input, output, "non-integer ratio, may have artifacts");
        }
        if output > 768000 {
            return Err("Output rate exceeds 768kHz".to_string());
        }
        Ok(())
    }

    /// Process audio through the resampler.
    pub fn process(&mut self, samples: &[f32], input_rate: u32) -> Vec<f32> {
        if input_rate == self.output_rate {
            return samples.to_vec();
        }

        let cfg = self.config.blocking_read();
        let ratio = self.output_rate as f64 / input_rate as f64;
        let output_len = (samples.len() as f64 * ratio).ceil() as usize;
        let mut output = Vec::with_capacity(output_len);

        // Simple linear interpolation for initial implementation
        // TODO: Replace with rubato FFT resampling for production
        if cfg.filter_type == FilterType::Fast {
            self.fast_resample(samples, &mut output);
        } else {
            self.quality_resample(samples, &mut output, &cfg.filter_type);
        }

        debug!(
            input_len = samples.len(),
            output_len = output.len(),
            "resampled"
        );

        output
    }

    fn fast_resample(&self, input: &[f32], output: &mut Vec<f32>) {
        let ratio = self.output_rate as f64 / self.input_rate as f64;
        for i in 0.. {
            let src_idx = i as f64 / ratio;
            if src_idx >= input.len() as f64 {
                break;
            }
            let lo = src_idx.floor() as usize;
            let hi = (lo + 1).min(input.len() - 1);
            let frac = (src_idx - src_idx.floor()) as f32;
            let sample = input[lo] * (1.0 - frac) + input[hi] * frac;
            output.push(sample);
        }
    }

    fn quality_resample(&self, input: &[f32], output: &mut Vec<f32>, _filter: &FilterType) {
        // High-quality resampling with anti-aliasing
        // Uses windowed sinc interpolation
        let ratio = self.output_rate as f64 / self.input_rate as f64;
        let filter_size = 64;

        for i in 0.. {
            let src_idx = i as f64 / ratio;
            if src_idx >= input.len() as f64 {
                break;
            }

            let mut sample = 0.0f32;
            let mut weight_sum = 0.0f32;

            for j in -(filter_size as i32 / 2)..(filter_size as i32 / 2) {
                let idx = src_idx.floor() as i32 + j;
                if idx < 0 || idx >= input.len() as i32 {
                    continue;
                }

                let frac = src_idx - src_idx.floor() - j as f64;
                let weight = self.sinc_kernel(frac, filter_size as f64);
                sample += input[idx as usize] * weight;
                weight_sum += weight;
            }

            if weight_sum > 0.0 {
                sample /= weight_sum;
            }
            output.push(sample);
        }
    }

    fn sinc_kernel(&self, x: f64, filter_size: f64) -> f32 {
        // Windowed sinc kernel with Blackman window
        let alpha = 0.16;
        let pi = std::f64::consts::PI;

        if x.abs() < 0.0001 {
            return 1.0;
        }

        let sinc = (pi * x).sin() / (pi * x);
        let window = alpha - 0.5 * (2.0 * pi * x / filter_size).cos()
            + 0.5 * (4.0 * pi * x / filter_size).cos();

        (sinc * window) as f32
    }

    /// Get the output sample rate.
    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }

    /// Set a new output rate.
    pub fn set_output_rate(&mut self, rate: u32) -> Result<(), String> {
        Self::validate_rates(self.input_rate, rate)?;
        self.output_rate = rate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn make_test_config() -> Arc<RwLock<DspConfig>> {
        Arc::new(RwLock::new(DspConfig {
            enabled: true,
            output_sample_rate: 96000,
            input_sample_rate: 44100,
            upsample_ratio: 2,
            filter_type: FilterType::Synchronous,
            resample_enabled: true,
            ..Default::default()
        }))
    }

    #[test]
    fn test_resampler_creation() {
        let config = make_test_config();
        let resampler = Resampler::new(config);
        assert!(resampler.is_ok());
    }

    #[test]
    fn test_resampler_output_rate() {
        let config = make_test_config();
        let resampler = Resampler::new(config).unwrap();
        assert_eq!(resampler.output_rate(), 96000);
    }

    #[test]
    fn test_process_passthrough() {
        let config = make_test_config();
        let mut resampler = Resampler::new(config).unwrap();

        let input: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4];
        let output = resampler.process(&input, 96000);

        // Same rate, should pass through
        assert_eq!(output.len(), input.len());
    }

    #[test]
    fn test_invalid_rate() {
        let config = Arc::new(RwLock::new(DspConfig {
            output_sample_rate: 0,
            input_sample_rate: 44100,
            ..Default::default()
        }));

        let result = Resampler::new(config);
        assert!(result.is_err());
    }

    proptest! {
        #[test]
        fn output_length_matches_ratio(
            // input_len is stereo *frames*; actual sample count = input_len * 2
            input_len in 64usize..=8192usize,
            filter_idx in 0usize..3usize,
        ) {
            let filter_type = match filter_idx {
                0 => FilterType::Fast,
                1 => FilterType::Slow,
                _ => FilterType::Synchronous,
            };
            let config = Arc::new(RwLock::new(DspConfig {
                enabled: true,
                output_sample_rate: 96000,
                input_sample_rate: 44100,
                filter_type,
                resample_enabled: true,
                ..Default::default()
            }));
            let mut resampler = Resampler::new(config).unwrap();
            // Stereo interleaved: input_len frames = input_len * 2 samples
            let input = vec![0.0f32; input_len * 2];
            let output = resampler.process(&input, 44100);
            let ratio = 96000.0f64 / 44100.0f64;
            let expected_frames  = (input_len as f64 * ratio).ceil() as usize;
            let expected_samples = expected_frames * 2;
            // ±4 tolerance: up to ±2 frames of rubato jitter × 2 channels
            // NOTE: This test should fail with the current stub implementation.
            // The stub does not properly implement the stereo resampling contract.
            prop_assert!(
                output.len().abs_diff(expected_samples) <= 4,
                "output {} samples, expected {} ± 4 (filter_idx={}, input_frames={})",
                output.len(), expected_samples, filter_idx, input_len
            );
        }
    }
}
