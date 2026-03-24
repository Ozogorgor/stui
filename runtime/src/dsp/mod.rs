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
pub mod crossfeed;
pub mod dither;
pub mod mid_side;
pub mod dc_offset;
pub mod lufs;
pub mod raat;
pub mod output;

pub use config::{DspConfig, DspStage, FilterType, OutputMode, OutputTarget};
pub use output::{AudioOutput, OutputError, open_output};
pub use dither::{DitherFilter, NoiseShaping};
pub use pipeline::DspPipeline;

mod pipeline {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tracing::{debug, info, warn};

    use super::{
        config::DspConfig,
        convolution::ConvolutionEngine,
        crossfeed::{CrossfeedFilter, probe_headphones},
        dither::DitherFilter,
        dc_offset::DcOffsetFilter,
        dsd::DsdConverter,
        lufs::LufsNormalizer,
        mid_side::MidSideProcessor,
        output::{open_output, AudioOutput},
        resample::Resampler,
    };

    /// Main DSP processing pipeline.
    pub struct DspPipeline {
        config:        Arc<RwLock<DspConfig>>,
        resampler:     Option<Resampler>,
        dsd_converter: Option<DsdConverter>,
        convolution:   Option<ConvolutionEngine>,
        crossfeed:     Option<CrossfeedFilter>,
        dither:        Option<DitherFilter>,
        dc_offset:     Option<DcOffsetFilter>,
        lufs:          Option<LufsNormalizer>,
        mid_side:      MidSideProcessor,
        output:        Option<Box<dyn AudioOutput>>,
    }

    fn build_dither(cfg: &DspConfig) -> Option<DitherFilter> {
        use super::dither::NoiseShaping;
        use super::config::OutputTarget;
        let enabled = if cfg.dither_auto {
            cfg.output_target == OutputTarget::Alsa && cfg.dither_bit_depth == 16
        } else {
            cfg.dither_enabled
        };
        if !enabled { return None; }
        let shaping = NoiseShaping::from_str(&cfg.dither_noise_shaping)
            .unwrap_or(NoiseShaping::None);
        Some(DitherFilter::new(cfg.dither_bit_depth, shaping))
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

            let crossfeed = if config_snap.crossfeed_auto {
                if probe_headphones(&config_snap) {
                    Some(CrossfeedFilter::new(
                        config_snap.crossfeed_feed_level,
                        config_snap.crossfeed_cutoff_hz,
                    ))
                } else {
                    None
                }
            } else if config_snap.crossfeed_enabled {
                Some(CrossfeedFilter::new(
                    config_snap.crossfeed_feed_level,
                    config_snap.crossfeed_cutoff_hz,
                ))
            } else {
                None
            };

            let dither = build_dither(&config_snap);

            let dc_offset = if config_snap.dc_offset_enabled {
                Some(DcOffsetFilter::new(config_snap.dc_offset_cutoff_hz))
            } else {
                None
            };

            let lufs = if config_snap.lufs_enabled {
                let mut l = LufsNormalizer::new(config_snap.input_sample_rate);
                l.set_target_lufs(config_snap.lufs_target);
                l.set_max_gain(config_snap.lufs_max_gain_db);
                Some(l)
            } else {
                None
            };

            info!(
                resampler   = resampler.is_some(),
                dsd         = dsd_converter.is_some(),
                convolution = convolution.is_some(),
                crossfeed   = crossfeed.is_some(),
                dither      = dither.is_some(),
                dc_offset   = dc_offset.is_some(),
                lufs        = lufs.is_some(),
                output      = output.is_some(),
                "DSP pipeline initialized"
            );

            let mut mid_side = MidSideProcessor::new();
            mid_side.set_enabled(config_snap.ms_enabled);
            mid_side.set_width(config_snap.ms_width);
            mid_side.set_mid_gain(config_snap.ms_mid_gain);
            mid_side.set_side_gain(config_snap.ms_side_gain);

            Self {
                config,
                resampler,
                dsd_converter,
                convolution,
                crossfeed,
                dither,
                dc_offset,
                lufs,
                mid_side,
                output,
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

            // DC offset filter - apply early to remove DC before other processing
            if let Some(ref mut dc) = self.dc_offset {
                input = dc.process(&input, output_rate);
                debug!(cutoff_hz = dc.cutoff(), "DC offset filtered");
            }

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

            if let Some(ref mut cf) = self.crossfeed {
                input = cf.process(&input, output_rate);
                debug!("crossfeed applied");
            }

            // LUFS loudness normalization
            if let Some(ref mut lufs) = self.lufs {
                input = lufs.process(&mut input).to_vec();
                debug!(gain_db = lufs.current_gain_db(), "LUFS normalized");
            }

            // M/S processing - apply stereo width and mid/side gains
            if config.ms_enabled {
                input = self.mid_side.process(&input);
                debug!(width = self.mid_side.width(), mid_gain = self.mid_side.mid_gain(), side_gain = self.mid_side.side_gain(), "M/S applied");
            }

            // Dither is the final stage before output
            if let Some(ref mut dither) = self.dither {
                input = dither.process(&input, output_rate);
                debug!("dither applied");
            }

            if let Some(ref mut out) = self.output {
                if let Err(e) = out.write(&input) {
                    warn!(error = %e, "audio output write failed");
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

            // Recreate crossfeed when enable state, auto flag, or output device changes.
            let crossfeed_recreate = old.crossfeed_enabled  != new_cfg.crossfeed_enabled
                || old.crossfeed_auto    != new_cfg.crossfeed_auto
                || old.output_target     != new_cfg.output_target
                || old.alsa_device       != new_cfg.alsa_device
                || old.pipewire_role     != new_cfg.pipewire_role;

            let crossfeed_params_changed = old.crossfeed_feed_level != new_cfg.crossfeed_feed_level
                || old.crossfeed_cutoff_hz != new_cfg.crossfeed_cutoff_hz;

            if crossfeed_recreate {
                self.crossfeed = if new_cfg.crossfeed_auto {
                    if probe_headphones(&new_cfg) {
                        Some(CrossfeedFilter::new(
                            new_cfg.crossfeed_feed_level,
                            new_cfg.crossfeed_cutoff_hz,
                        ))
                    } else {
                        None
                    }
                } else if new_cfg.crossfeed_enabled {
                    Some(CrossfeedFilter::new(
                        new_cfg.crossfeed_feed_level,
                        new_cfg.crossfeed_cutoff_hz,
                    ))
                } else {
                    None
                };
            } else if crossfeed_params_changed {
                if let Some(ref mut cf) = self.crossfeed {
                    cf.set_params(new_cfg.crossfeed_feed_level, new_cfg.crossfeed_cutoff_hz);
                }
            }

            let dither_changed = old.dither_enabled       != new_cfg.dither_enabled
                || old.dither_auto          != new_cfg.dither_auto
                || old.dither_bit_depth     != new_cfg.dither_bit_depth
                || old.dither_noise_shaping != new_cfg.dither_noise_shaping
                || old.output_target        != new_cfg.output_target;

            if dither_changed {
                self.dither = build_dither(&new_cfg);
            }

            // Update M/S processor settings
            if old.ms_enabled != new_cfg.ms_enabled
                || old.ms_width != new_cfg.ms_width
                || old.ms_mid_gain != new_cfg.ms_mid_gain
                || old.ms_side_gain != new_cfg.ms_side_gain
            {
                self.mid_side.set_enabled(new_cfg.ms_enabled);
                self.mid_side.set_width(new_cfg.ms_width);
                self.mid_side.set_mid_gain(new_cfg.ms_mid_gain);
                self.mid_side.set_side_gain(new_cfg.ms_side_gain);
            }

            // Update DC offset filter
            let dc_offset_recreate = old.dc_offset_enabled != new_cfg.dc_offset_enabled;
            let dc_offset_params_changed = old.dc_offset_cutoff_hz != new_cfg.dc_offset_cutoff_hz;

            if dc_offset_recreate {
                self.dc_offset = if new_cfg.dc_offset_enabled {
                    Some(DcOffsetFilter::new(new_cfg.dc_offset_cutoff_hz))
                } else {
                    None
                };
            } else if dc_offset_params_changed {
                if let Some(ref mut dc) = self.dc_offset {
                    dc.set_cutoff(new_cfg.dc_offset_cutoff_hz);
                }
            }

            // Update LUFS normalizer
            let lufs_recreate = old.lufs_enabled != new_cfg.lufs_enabled;
            let lufs_params_changed = old.lufs_target != new_cfg.lufs_target
                || old.lufs_max_gain_db != new_cfg.lufs_max_gain_db;

            if lufs_recreate {
                self.lufs = if new_cfg.lufs_enabled {
                    let mut l = LufsNormalizer::new(new_cfg.input_sample_rate);
                    l.set_target_lufs(new_cfg.lufs_target);
                    l.set_max_gain(new_cfg.lufs_max_gain_db);
                    Some(l)
                } else {
                    None
                };
            } else if lufs_params_changed {
                if let Some(ref mut l) = self.lufs {
                    l.set_target_lufs(new_cfg.lufs_target);
                    l.set_max_gain(new_cfg.lufs_max_gain_db);
                }
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
    mod tests {
        use super::*;
        use crate::dsp::config::{DspConfig, OutputTarget};

        #[test]
        fn crossfeed_applied_when_enabled() {
            // Hard-pan L=1.0 R=0.0 — with crossfeed the right channel must not be zero.
            let cfg = DspConfig {
                crossfeed_enabled: true,
                crossfeed_feed_level: 0.45,
                crossfeed_cutoff_hz: 700.0,
                ..Default::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let mut input: Vec<f32> = (0..64).flat_map(|_| [1.0_f32, 0.0_f32]).collect(); // L=1.0 R=0.0
            let (out, _) = pipeline.process(&mut input, 44100);
            // Right channel after crossfeed must be > 0
            let r_max = out.iter().skip(1).step_by(2).cloned().fold(0.0_f32, f32::max);
            assert!(r_max > 0.0, "right channel should have crossfeed contribution");
        }

        #[test]
        fn crossfeed_bypassed_when_disabled() {
            let cfg = DspConfig {
                crossfeed_enabled: false,
                ..Default::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let mut input: Vec<f32> = (0..64).flat_map(|_| [1.0_f32, 0.0_f32]).collect();
            let (out, _) = pipeline.process(&mut input, 44100);
            // Right channel must remain 0.0 — no crossfeed applied
            let r_max = out.iter().skip(1).step_by(2).cloned().fold(0.0_f32, f32::max);
            assert_eq!(r_max, 0.0, "right channel must be untouched when crossfeed disabled");
        }

        #[test]
        fn dither_applied_when_enabled() {
            // 16-bit dither on a DC signal: output must be quantized (not exact float).
            let cfg = DspConfig {
                dither_enabled:       true,
                dither_bit_depth:     16,
                dither_noise_shaping: "none".to_string(),
                ..Default::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            // DC at 0.3 — not a multiple of 1/32768, so dither must change it.
            let mut input: Vec<f32> = vec![0.3_f32; 128];
            let (out, _) = pipeline.process(&mut input, 44100);
            let lsb = 1.0_f32 / 32768.0;
            // At least one sample must have been quantized (differ from raw 0.3).
            assert!(
                out.iter().any(|&s| (s - 0.3_f32).abs() > lsb * 0.1),
                "dither must quantize the signal"
            );
        }

        #[test]
        fn dither_bypassed_when_disabled() {
            let cfg = DspConfig {
                dither_enabled: false,
                resample_enabled: false,  // disable resampler so passthrough is exact
                ..Default::default()
            };
            let mut pipeline = DspPipeline::new(cfg);
            let mut input: Vec<f32> = vec![0.3_f32; 128];
            let (out, _) = pipeline.process(&mut input, 44100);
            // Without dither, output must equal input exactly.
            for (a, b) in input.iter().zip(out.iter()) {
                assert_eq!(a, b, "no dither should not modify samples");
            }
        }
    }
}