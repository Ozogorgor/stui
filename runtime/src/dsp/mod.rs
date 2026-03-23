//! DSP audio processing pipeline for high-quality audio playback.
//!
//! Provides audio processing stages including:
//! - High-quality sample rate conversion (upsampling)
//! - DSD to PCM conversion
//! - Room correction convolution
//!
//! ## Usage
//!
//! ```rust
//! use stui_runtime::dsp::{DspPipeline, DspConfig};
//!
//! let config = DspConfig {
//!     enabled: true,
//!     output_sample_rate: 384000,
//!     upsample_ratio: 8,
//!     ..Default::default()
//! };
//!
//! let pipeline = DspPipeline::new(config);
//! ```

pub mod config;
pub mod resample;
pub mod dsd;
pub mod convolution;
pub mod pipewire;
pub mod raat;

pub use config::{DspConfig, DspStage, FilterType, OutputMode, OutputTarget};
pub use pipeline::DspPipeline;

mod pipeline {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tracing::{debug, info, warn};

    use super::{config::DspConfig, convolution::ConvolutionEngine, dsd::DsdConverter, resample::Resampler};

    /// Processing stages in the DSP pipeline.
    pub enum DspStageEnum {
        Resample(Resampler),
        DsdToPcm(DsdConverter),
        Convolution(ConvolutionEngine),
    }

    /// Main DSP processing pipeline.
    pub struct DspPipeline {
        config: Arc<RwLock<DspConfig>>,
        resampler: Option<Resampler>,
        dsd_converter: Option<DsdConverter>,
        convolution: Option<ConvolutionEngine>,
    }

    impl DspPipeline {
        /// Create a new DSP pipeline with the given configuration.
        pub fn new(config: DspConfig) -> Self {
            let config = Arc::new(RwLock::new(config));

            let resampler = Resampler::new(config.clone()).ok();
            let dsd_converter = DsdConverter::new(config.clone()).ok();
            let convolution = ConvolutionEngine::new(config.clone()).ok();

            info!(
                resampler = resampler.is_some(),
                dsd = dsd_converter.is_some(),
                convolution = convolution.is_some(),
                "DSP pipeline initialized"
            );

            Self {
                config,
                resampler,
                dsd_converter,
                convolution,
            }
        }

        /// Process audio samples through the DSP pipeline.
        /// Input and output are interleaved stereo samples as f32.
        pub fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> (Vec<f32>, u32) {
            let mut input = samples.to_vec();
            let mut output_rate = sample_rate;

            // Snapshot config once so all stages see a consistent view even if
            // update_config() is called concurrently from another task.
            let config = self.config.blocking_read();

            if let Some(ref mut resampler) = self.resampler {
                if config.resample_enabled {
                    input = resampler.process(&input, sample_rate);
                    if !input.is_empty() {
                        output_rate = resampler.output_rate();
                        debug!(input_rate = sample_rate, output_rate = output_rate, "resampled");
                    }
                }
            }

            if let Some(ref mut dsd_converter) = self.dsd_converter {
                if config.dsd_to_pcm_enabled {
                    input = dsd_converter.convert(&input);
                    if !input.is_empty() {
                        output_rate = 352800; // DSD128 → 352.8kHz PCM
                        debug!(output_rate = output_rate, "DSD converted");
                    }
                }
            }

            if let Some(ref mut convolution) = self.convolution {
                if convolution.is_enabled() {
                    input = convolution.process(&input);
                    debug!("convolution applied");
                }
            }

            (input, output_rate)
        }

        /// Check if DSP processing is active.
        pub fn is_active(&self) -> bool {
            let config = self.config.blocking_read();
            config.enabled
        }

        /// Get current output sample rate.
        pub fn output_sample_rate(&self) -> u32 {
            let config = self.config.blocking_read();
            config.output_sample_rate
        }

        /// Update configuration at runtime.
        pub async fn update_config(&self, config: DspConfig) {
            *self.config.write().await = config;
        }

        /// Get current configuration.
        pub async fn config(&self) -> DspConfig {
            self.config.read().await.clone()
        }

        /// Load convolution filter from file.
        pub fn load_convolution_filter(&mut self, path: &str) -> Result<(), String> {
            if let Some(ref mut conv) = self.convolution {
                conv.load_filter(path)
            } else {
                Err("Convolution not available".to_string())
            }
        }

        /// Bypass convolution filter.
        pub fn set_convolution_bypass(&mut self, bypass: bool) {
            if let Some(ref mut conv) = self.convolution {
                conv.set_bypass(bypass);
            }
        }
    }

    impl Default for DspPipeline {
        fn default() -> Self {
            Self::new(DspConfig::default())
        }
    }
}