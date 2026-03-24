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
pub mod eq;
pub mod raat;
pub mod output;

pub use config::{DspConfig, DspStage, FilterType, OutputMode, OutputTarget};
pub use output::{AudioOutput, OutputError, open_output};
pub use pipeline::DspPipeline;
pub use eq::ParametricEq;

mod pipeline {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tracing::{debug, info, warn};

    use super::{
        config::DspConfig,
        convolution::ConvolutionEngine,
        dsd::DsdConverter,
        eq::ParametricEq,
        output::{open_output, AudioOutput},
        resample::Resampler,
    };

    /// Main DSP processing pipeline.
    pub struct DspPipeline {
        config:        Arc<RwLock<DspConfig>>,
        resampler:     Option<Resampler>,
        dsd_converter: Option<DsdConverter>,
        convolution:   Option<ConvolutionEngine>,
        eq:            Option<ParametricEq>,
        output:        Option<Box<dyn AudioOutput>>,
    }

    impl DspPipeline {
        /// Create a new DSP pipeline with the given configuration.
        pub fn new(config: DspConfig) -> Self {
            // Take a snapshot before moving config into the Arc so all sub-components see the same state.
            let config_snap = config.clone();
            let config = Arc::new(RwLock::new(config));

            let resampler     = Resampler::new(config.clone()).ok();
            let dsd_converter = DsdConverter::new(config.clone()).ok();
            let convolution   = ConvolutionEngine::new(config.clone()).ok();

            let eq = if config_snap.eq_enabled && !config_snap.eq_bands.is_empty() {
                let mut peq = ParametricEq::new(&config_snap.eq_bands);
                peq.set_bypass(config_snap.eq_bypass);
                Some(peq)
            } else {
                None
            };

            let output = if config_snap.enabled {
                match open_output(config_snap.output_target, &config_snap) {
                    Ok(out) => Some(out),
                    Err(e)  => {
                        warn!(error = %e, "failed to open audio output — DSP will process but not deliver");
                        None
                    }
                }
            } else {
                None
            };

            info!(
                resampler   = resampler.is_some(),
                dsd         = dsd_converter.is_some(),
                convolution = convolution.is_some(),
                eq          = eq.is_some(),
                output      = output.is_some(),
                "DSP pipeline initialized"
            );

            Self {
                config,
                resampler,
                dsd_converter,
                convolution,
                eq,
                output,
            }
        }

        /// Process audio samples through the DSP pipeline.
        /// Input and output are interleaved stereo samples as f32.
        pub fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> (Vec<f32>, u32) {
            let mut input = samples.to_vec();
            let mut output_rate = sample_rate;
            let config = self.config.blocking_read();

            let eq_after_conv = config.convolution_enabled;
            let eq_before_resamp = !eq_after_conv && config.resample_enabled;
            let eq_after_dsd = !eq_after_conv && !config.resample_enabled && config.dsd_to_pcm_enabled;
            // If none of the above, EQ is the only stage (runs at the end / start)

            macro_rules! run_eq {
                () => {
                    if let Some(ref mut eq) = self.eq {
                        if eq.is_enabled() {
                            input = eq.process(&input, output_rate);
                            debug!("eq applied");
                        }
                    }
                }
            }

            if eq_before_resamp { run_eq!(); }

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

            if eq_after_dsd { run_eq!(); }

            if let Some(ref mut convolution) = self.convolution {
                if convolution.is_enabled() {
                    input = convolution.process(&input);
                    debug!("convolution applied");
                }
            }

            if eq_after_conv { run_eq!(); }

            // If no other stage was active, EQ still runs (covers "all disabled" case)
            if !eq_before_resamp && !eq_after_dsd && !eq_after_conv { run_eq!(); }

            if let Some(ref mut out) = self.output {
                if let Err(e) = out.write(&input) {
                    warn!(error = %e, "audio output write failed");
                }
            }

            (input, output_rate)
        }

        /// Test-only: like process() but appends stage names to `log` in execution order.
        #[cfg(test)]
        pub fn process_with_log(
            &mut self,
            samples: &mut [f32],
            sample_rate: u32,
            log: &pipeline_eq_tests::StageLog,
        ) -> (Vec<f32>, u32) {
            let mut input = samples.to_vec();
            let mut output_rate = sample_rate;
            let config = self.config.blocking_read();

            let eq_after_conv   = config.convolution_enabled;
            let eq_before_resamp = !eq_after_conv && config.resample_enabled;
            let eq_after_dsd    = !eq_after_conv && !config.resample_enabled && config.dsd_to_pcm_enabled;

            macro_rules! log_eq {
                () => {
                    if let Some(ref mut eq) = self.eq {
                        if eq.is_enabled() {
                            log.record("eq");
                            input = eq.process(&input, output_rate);
                        }
                    }
                }
            }

            if eq_before_resamp { log_eq!(); }

            if let Some(ref mut resampler) = self.resampler {
                if config.resample_enabled {
                    log.record("resample");
                    input = resampler.process(&input, sample_rate);
                    if !input.is_empty() { output_rate = resampler.output_rate(); }
                }
            }

            if let Some(ref mut dsd) = self.dsd_converter {
                if config.dsd_to_pcm_enabled {
                    log.record("dsd");
                    input = dsd.convert(&input);
                }
            }

            if eq_after_dsd { log_eq!(); }

            if let Some(ref mut conv) = self.convolution {
                // Record "convolution" whenever the config says it's enabled, even if the
                // engine has no filter loaded yet — this lets tests verify EQ ordering relative
                // to the convolution stage without requiring a real filter file.
                if config.convolution_enabled {
                    log.record("convolution");
                    if conv.is_enabled() {
                        input = conv.process(&input);
                    }
                }
            }

            if eq_after_conv { log_eq!(); }
            if !eq_before_resamp && !eq_after_dsd && !eq_after_conv { log_eq!(); }

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
        pub async fn update_config(&mut self, new_cfg: DspConfig) {
            let old = self.config.read().await.clone();
            *self.config.write().await = new_cfg.clone();

            let output_changed = old.output_target  != new_cfg.output_target
                || old.alsa_device   != new_cfg.alsa_device
                || old.pipewire_role != new_cfg.pipewire_role;

            if output_changed {
                if let Some(old_out) = self.output.take() {
                    old_out.close();
                }
                if new_cfg.enabled {
                    match open_output(new_cfg.output_target, &new_cfg) {
                        Ok(out) => { self.output = Some(out); }
                        Err(e)  => { warn!(error = %e, "failed to re-open audio output"); }
                    }
                }
            }

            // Update EQ state
            if let Some(ref mut eq) = self.eq {
                eq.set_enabled(new_cfg.eq_enabled);
                eq.set_bypass(new_cfg.eq_bypass);
                eq.update_bands(&new_cfg.eq_bands);
            } else if new_cfg.eq_enabled && !new_cfg.eq_bands.is_empty() {
                let mut peq = ParametricEq::new(&new_cfg.eq_bands);
                peq.set_bypass(new_cfg.eq_bypass);
                self.eq = Some(peq);
            }
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

    #[cfg(test)]
    mod pipeline_eq_tests {
        use super::*;
        use crate::dsp::config::{DspConfig, EqBand, EqFilterType};
        use std::sync::{Arc, Mutex};

        /// Test helper: records which stages ran in order.
        #[derive(Clone, Default)]
        pub struct StageLog(pub Arc<Mutex<Vec<String>>>);

        impl StageLog {
            pub fn record(&self, name: &str) {
                self.0.lock().unwrap().push(name.to_string());
            }
            pub fn entries(&self) -> Vec<String> {
                self.0.lock().unwrap().clone()
            }
        }

        fn eq_band() -> EqBand {
            EqBand { enabled: true, filter_type: EqFilterType::Peak,
                     freq: 1000.0, gain_db: 6.0, q: 1.0 }
        }

        #[test]
        fn eq_runs_before_resample_when_convolution_disabled() {
            let cfg = DspConfig {
                enabled: true,
                eq_enabled: true,
                eq_bands: vec![eq_band()],
                resample_enabled: true,
                convolution_enabled: false,
                ..DspConfig::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let log = StageLog::default();
            let samples = vec![0.0f32; 256];
            pipeline.process_with_log(&mut samples.clone(), 44100, &log);
            let entries = log.entries();
            let eq_pos    = entries.iter().position(|s| s == "eq").unwrap_or(usize::MAX);
            let resamp_pos = entries.iter().position(|s| s == "resample").unwrap_or(usize::MAX);
            assert!(eq_pos < resamp_pos,
                "eq must run before resample; log={entries:?}");
        }

        #[test]
        fn eq_runs_after_convolution_when_enabled() {
            let cfg = DspConfig {
                enabled: true,
                eq_enabled: true,
                eq_bands: vec![eq_band()],
                convolution_enabled: true,
                ..DspConfig::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let log = StageLog::default();
            let mut samples = vec![0.0f32; 256];
            pipeline.process_with_log(&mut samples, 44100, &log);
            let entries = log.entries();
            let conv_pos = entries.iter().position(|s| s == "convolution").unwrap_or(usize::MAX);
            let eq_pos   = entries.iter().position(|s| s == "eq").unwrap_or(usize::MAX);
            assert!(conv_pos < eq_pos,
                "eq must run after convolution; log={entries:?}");
        }
    }
}
