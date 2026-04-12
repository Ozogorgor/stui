//! High-quality audio resampler using the rubato library (1.x).
//!
//! FilterType dispatches to different rubato engines:
//!   Fast        → Async<f32> FixedAsync::Output  (polynomial, lower quality, low CPU)
//!   Slow        → Fft<f32>   FixedSync::Input     (FFT-based, flat passband)
//!   Synchronous → Async<f32> FixedAsync::Input    (sinc, highest quality)

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, PolynomialDegree, Resampler as _,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use super::config::{DspConfig, FilterType};

// Sinc parameters for the Synchronous (highest quality) engine.
fn sinc_params_high() -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    }
}

enum ResamplerKind {
    /// Fast — polynomial interpolation, FixedAsync::Output
    PolyOut(Async<f32>),
    /// Slow — FFT-based, FixedSync::Input
    FftIn(Fft<f32>),
    /// Synchronous — sinc interpolation, FixedAsync::Input
    SincIn(Async<f32>),
}

/// High-quality audio resampler. Stereo interleaved f32 input and output.
#[allow(dead_code)] // Used by DspPipeline internally
#[allow(clippy::type_complexity)]
pub struct Resampler {
    config: Arc<RwLock<DspConfig>>,
    input_rate: u32,
    output_rate: u32,
    chunk_size: usize,
    kind: ResamplerKind,
}

impl Resampler {
    #[allow(dead_code)] // pub API: used by DSP pipeline resampler
    pub fn new(config: Arc<RwLock<DspConfig>>) -> Result<Self, String> {
        let cfg = config.blocking_read();
        let input_rate = cfg.input_sample_rate;
        let output_rate = cfg.output_sample_rate;
        let chunk_size = cfg.buffer_size;
        let filter_type = cfg.filter_type;
        drop(cfg);

        Self::validate_rates(input_rate, output_rate)?;
        let kind = Self::build_kind(filter_type, input_rate, output_rate, chunk_size)?;

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
            kind,
        })
    }

    fn validate_rates(input: u32, output: u32) -> Result<(), String> {
        if input == 0 || output == 0 {
            return Err("sample rates must be non-zero".into());
        }
        if output > 768_000 {
            return Err("output rate exceeds 768kHz".into());
        }
        Ok(())
    }

    fn build_kind(
        filter_type: FilterType,
        input_rate: u32,
        output_rate: u32,
        chunk_size: usize,
    ) -> Result<ResamplerKind, String> {
        let ratio = output_rate as f64 / input_rate as f64;
        // rubato works on per-channel data; we have 2 channels (stereo)
        const CHANNELS: usize = 2;
        // max relative ratio variation: we don't dynamically change rates so keep tight at 1.1
        const MAX_RELATIVE: f64 = 1.1;

        match filter_type {
            FilterType::Fast => {
                let r = Async::<f32>::new_poly(
                    ratio,
                    MAX_RELATIVE,
                    PolynomialDegree::Linear,
                    chunk_size,
                    CHANNELS,
                    FixedAsync::Output,
                )
                .map_err(|e| format!("rubato Fast init: {e}"))?;
                Ok(ResamplerKind::PolyOut(r))
            }
            FilterType::Slow => {
                let r = Fft::<f32>::new(
                    input_rate as usize,
                    output_rate as usize,
                    chunk_size,
                    1,
                    CHANNELS,
                    FixedSync::Input,
                )
                .map_err(|e| format!("rubato Slow init: {e}"))?;
                Ok(ResamplerKind::FftIn(r))
            }
            FilterType::Synchronous => {
                let params = sinc_params_high();
                let r = Async::<f32>::new_sinc(
                    ratio,
                    MAX_RELATIVE,
                    &params,
                    chunk_size,
                    CHANNELS,
                    FixedAsync::Input,
                )
                .map_err(|e| format!("rubato Synchronous init: {e}"))?;
                Ok(ResamplerKind::SincIn(r))
            }
        }
    }

    /// Process interleaved stereo samples through the resampler.
    /// Returns interleaved stereo output.
    pub fn process(&mut self, samples: &[f32], input_rate: u32) -> Vec<f32> {
        if input_rate == self.output_rate {
            return samples.to_vec();
        }
        if samples.is_empty() {
            return Vec::new();
        }

        // Deinterleave: [L0,R0,L1,R1,...] → [[L0,L1,...],[R0,R1,...]]
        let n_frames = samples.len() / 2;
        let mut ch_left = Vec::with_capacity(n_frames);
        let mut ch_right = Vec::with_capacity(n_frames);
        for chunk in samples.chunks_exact(2) {
            ch_left.push(chunk[0]);
            ch_right.push(chunk[1]);
        }
        let input_channels: Vec<Vec<f32>> = vec![ch_left, ch_right];

        let out_channels = self.run_rubato(&input_channels, n_frames);

        // Reinterleave: [[L0,L1,...],[R0,R1,...]] → [L0,R0,L1,R1,...]
        let out_len = out_channels[0].len();
        let mut output = Vec::with_capacity(out_len * 2);
        for i in 0..out_len {
            output.push(out_channels[0][i]);
            output.push(out_channels[1][i]);
        }

        debug!(input = samples.len(), output = output.len(), "resampled");
        output
    }

    fn run_rubato(&mut self, input_channels: &[Vec<f32>], n_frames: usize) -> Vec<Vec<f32>> {
        // Build input adapter: SequentialSliceOfVecs wraps &[Vec<f32>]
        let input_adapter = match SequentialSliceOfVecs::new(input_channels, 2, n_frames) {
            Ok(a) => a,
            Err(e) => {
                warn!("rubato input adapter error: {e:?}");
                return vec![Vec::new(), Vec::new()];
            }
        };

        // Allocate output buffer with enough capacity (delay + output frames)
        let output_capacity = match &mut self.kind {
            ResamplerKind::PolyOut(r) => r.process_all_needed_output_len(n_frames),
            ResamplerKind::FftIn(r) => r.process_all_needed_output_len(n_frames),
            ResamplerKind::SincIn(r) => r.process_all_needed_output_len(n_frames),
        };

        // We use a mutable Vec<Vec<f32>> as the output and wrap it with SequentialSliceOfVecs
        let out_left = vec![0.0f32; output_capacity];
        let out_right = vec![0.0f32; output_capacity];
        let mut output_channels: Vec<Vec<f32>> = vec![out_left, out_right];

        let mut output_adapter = match SequentialSliceOfVecs::new_mut(
            output_channels.as_mut_slice(),
            2,
            output_capacity,
        ) {
            Ok(a) => a,
            Err(e) => {
                warn!("rubato output adapter error: {e:?}");
                return vec![Vec::new(), Vec::new()];
            }
        };

        // Process all input using chunked loop.
        // process_all_into_buffer handles arbitrary lengths and returns actual output frame count.
        let result = match &mut self.kind {
            ResamplerKind::PolyOut(r) => {
                r.process_all_into_buffer(&input_adapter, &mut output_adapter, n_frames, None)
            }
            ResamplerKind::FftIn(r) => {
                r.process_all_into_buffer(&input_adapter, &mut output_adapter, n_frames, None)
            }
            ResamplerKind::SincIn(r) => {
                r.process_all_into_buffer(&input_adapter, &mut output_adapter, n_frames, None)
            }
        };

        match result {
            Ok((_in_frames, out_frames)) => {
                // Truncate each channel to actual output length
                output_channels[0].truncate(out_frames);
                output_channels[1].truncate(out_frames);
                output_channels
            }
            Err(e) => {
                warn!("rubato process error: {e}");
                vec![Vec::new(), Vec::new()]
            }
        }
    }

    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }

    /// Reset the resampler state to clear any buffered data.
    /// Should be called on seeks or stream discontinuities.
    #[allow(dead_code)] // planned: called on seek events when DSP pipeline handles seeks
    pub fn reset(&mut self) {
        match &mut self.kind {
            ResamplerKind::PolyOut(resampler) => resampler.reset(),
            ResamplerKind::FftIn(resampler) => resampler.reset(),
            ResamplerKind::SincIn(resampler) => resampler.reset(),
        }
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
        assert!(Resampler::new(make_test_config()).is_ok());
    }

    #[test]
    fn test_resampler_output_rate() {
        let r = Resampler::new(make_test_config()).unwrap();
        assert_eq!(r.output_rate(), 96000);
    }

    #[test]
    fn test_process_passthrough() {
        let config = Arc::new(RwLock::new(DspConfig {
            output_sample_rate: 96000,
            input_sample_rate: 96000,
            ..Default::default()
        }));
        let mut r = Resampler::new(config).unwrap();
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        let output = r.process(&input, 96000);
        assert_eq!(output.len(), input.len());
    }

    #[test]
    fn test_invalid_rate() {
        let config = Arc::new(RwLock::new(DspConfig {
            output_sample_rate: 0,
            input_sample_rate: 44100,
            ..Default::default()
        }));
        assert!(Resampler::new(config).is_err());
    }

    proptest! {
        #[test]
        fn output_length_matches_ratio(
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
            // Stereo interleaved input: input_len frames = input_len*2 samples
            let input = vec![0.0f32; input_len * 2];
            let output = resampler.process(&input, 44100);
            let ratio = 96000.0f64 / 44100.0f64;
            let expected_frames = (input_len as f64 * ratio).ceil() as usize;
            let expected_samples = expected_frames * 2;
            prop_assert!(
                output.len().abs_diff(expected_samples) <= 4,
                "got {} samples, expected {} ± 4 (filter={}, input_frames={})",
                output.len(), expected_samples, filter_idx, input_len
            );
        }
    }
}
