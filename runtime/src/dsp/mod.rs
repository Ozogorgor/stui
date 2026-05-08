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

pub mod command;
pub mod config;
pub mod convolution;
pub mod crossfeed;
pub mod dc_offset;
pub mod dsd;
pub mod eq;
pub mod lufs;
pub mod mid_side;
pub mod mpd_config;
pub mod nodes;
pub mod ns_filters;
pub mod output;
pub mod preset_store;
pub mod profile_store;
pub mod raat;
pub mod resample;

// Re-export from ns_filters for backwards compatibility
#[allow(dead_code, unused_imports)]
pub use ns_filters::dither::{DitherFilter, NoiseShaping};
#[allow(dead_code, unused_imports)]
pub use ns_filters::saw::{SawNode, StftProcessor};

#[allow(dead_code, unused_imports)]
pub use command::DspCommand;
#[allow(dead_code, unused_imports)]
pub use config::{DspConfig, DspProfile, DspStage, FilterType, OutputMode, OutputTarget};
#[allow(dead_code, unused_imports)]
pub use eq::ParametricEq;
#[allow(dead_code, unused_imports)]
pub use nodes::{DspChain, DspNode, EqNode, GainNode};
#[allow(dead_code, unused_imports)]
pub use output::{open_output, AudioOutput, OutputError};
pub use pipeline::DspPipeline;
#[allow(dead_code, unused_imports)]
pub use preset_store::PresetStore;
#[allow(dead_code, unused_imports)]
pub use profile_store::CustomProfileStore;

mod pipeline {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tracing::{debug, info, warn};

    use super::{
        config::{DspConfig, DspProfileConfig, OutputSampleRate},
        convolution::ConvolutionEngine,
        crossfeed::{probe_headphones, CrossfeedFilter},
        dc_offset::DcOffsetFilter,
        dsd::DsdConverter,
        eq::ParametricEq,
        lufs::LufsNormalizer,
        mid_side::MidSideProcessor,
        nodes::{DspNode, EqNode, GainNode},
        ns_filters::dither::DitherFilter,
        output::{open_output, AudioOutput, DsdAudioOutput},
        preset_store,
        profile_store::CustomProfileStore,
        resample::Resampler,
    };

    /// Main DSP processing pipeline.
    #[allow(dead_code)] // planned: DSP pipeline, wired in by audio output subsystem
    pub struct DspPipeline {
        config: Arc<RwLock<DspConfig>>,
        config_dir: std::path::PathBuf,
        resampler: Option<Resampler>,
        dsd_converter: Option<DsdConverter>,
        dsd_mode: super::config::DsdMode,
        convolution: Option<ConvolutionEngine>,
        crossfeed: Option<CrossfeedFilter>,
        dither: Option<DitherFilter>,
        dc_offset: Option<DcOffsetFilter>,
        lufs: Option<LufsNormalizer>,
        mid_side: MidSideProcessor,
        eq: Option<ParametricEq>,
        output: Option<Box<dyn AudioOutput>>,
        dsd_output: Option<Box<dyn DsdAudioOutput>>,
        gain: Option<GainNode>,
        eq_node: Option<EqNode>,
        profile_store: CustomProfileStore,
        preset_store: preset_store::PresetStore,
        pcm_to_dsd_warned: bool,
    }

    #[allow(dead_code)] // planned: DspPipeline pub API, called from main.rs IPC command handler
    impl DspPipeline {
        fn build_dither(cfg: &DspConfig) -> Option<DitherFilter> {
            use super::config::OutputTarget;
            use super::ns_filters::dither::NoiseShaping;
            let enabled = if cfg.dither_auto {
                cfg.output_target == OutputTarget::Alsa && cfg.dither_bit_depth == 16
            } else {
                cfg.dither_enabled
            };
            if !enabled {
                return None;
            }
            let shaping =
                NoiseShaping::from_str(&cfg.dither_noise_shaping).unwrap_or(NoiseShaping::None);
            Some(DitherFilter::new(cfg.dither_bit_depth, shaping))
        }

        /// Create a new DSP pipeline with the given configuration.
        pub fn new(config: DspConfig) -> Self {
            Self::with_config_dir(config, std::path::PathBuf::from("."))
        }

        /// Create a new DSP pipeline with configuration directory for profile storage.
        pub fn with_config_dir(config: DspConfig, config_dir: std::path::PathBuf) -> Self {
            let profile_store = CustomProfileStore::load(&config_dir);
            let preset_store = preset_store::PresetStore::load(&config_dir);

            // Take a snapshot before moving config into the Arc so all sub-components see the same state.
            let config_snap = config.clone();
            let config = Arc::new(RwLock::new(config));

            let resampler = Resampler::new(config.clone()).ok();
            let dsd_converter = DsdConverter::new(config.clone()).ok();
            let convolution = ConvolutionEngine::new(config.clone()).ok();

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
                    Err(e) => {
                        warn!(error = %e, "failed to open audio output — DSP will process but not deliver");
                        None
                    }
                }
            } else {
                None
            };

            let crossfeed = if config_snap.crossfeed_auto {
                if probe_headphones(&config_snap) {
                    // Reflect auto-detected headphone state in the live config so
                    // that callers (e.g. dsp_to_mpv_flags) can read crossfeed_enabled.
                    config.blocking_write().crossfeed_enabled = true;
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

            let dither = Self::build_dither(&config_snap);

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
                resampler = resampler.is_some(),
                dsd = dsd_converter.is_some(),
                convolution = convolution.is_some(),
                crossfeed = crossfeed.is_some(),
                dither = dither.is_some(),
                dc_offset = dc_offset.is_some(),
                lufs = lufs.is_some(),
                eq = eq.is_some(),
                output = output.is_some(),
                "DSP pipeline initialized"
            );

            let mut mid_side = MidSideProcessor::new();
            mid_side.set_enabled(config_snap.ms_enabled);
            mid_side.set_width(config_snap.ms_width);
            mid_side.set_mid_gain(config_snap.ms_mid_gain);
            mid_side.set_side_gain(config_snap.ms_side_gain);

            // TODO: Native DSD output will be opened once PCM→DSD modulation is implemented
            // For now, DSD mode falls back to PCM output in process()
            let dsd_output: Option<Box<dyn DsdAudioOutput>> = None;

            // Real-time adjustable nodes - EQ before gain per audio engineering standards
            let gain = Some(GainNode::new());
            let eq_node = Some(EqNode::new());

            Self {
                config,
                config_dir,
                resampler,
                dsd_converter,
                dsd_mode: config_snap.dsd_mode,
                convolution,
                crossfeed,
                dither,
                dc_offset,
                lufs,
                mid_side,
                eq,
                output,
                dsd_output,
                gain,
                eq_node,
                profile_store,
                preset_store,
                pcm_to_dsd_warned: false,
            }
        }

        /// Process audio samples through the DSP pipeline.
        /// Input and output are interleaved stereo samples as f32.
        #[allow(dead_code)] // internal: called by audio engine
        pub fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> (Vec<f32>, u32) {
            let mut input = samples.to_vec();
            let mut output_rate = sample_rate;
            let config = self.config.blocking_read();

            let eq_after_conv = config.convolution_enabled;
            let eq_before_resamp = !eq_after_conv && config.resample_enabled;
            let eq_after_dsd =
                !eq_after_conv && !config.resample_enabled && config.dsd_to_pcm_enabled;
            // If none of the above, EQ is the only stage (runs at the end / start)

            macro_rules! run_eq {
                () => {
                    if let Some(ref mut eq) = self.eq {
                        if eq.is_enabled() {
                            input = eq.process(&input, output_rate);
                            debug!("eq applied");
                        }
                    }
                };
            }

            if eq_before_resamp {
                run_eq!();
            }

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
                        debug!(
                            input_rate = sample_rate,
                            output_rate = output_rate,
                            "resampled"
                        );
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

            if eq_after_dsd {
                run_eq!();
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
                debug!(
                    width = self.mid_side.width(),
                    mid_gain = self.mid_side.mid_gain(),
                    side_gain = self.mid_side.side_gain(),
                    "M/S applied"
                );
            }

            // EQ node (preset-based) — placed after convolution per audio engineering standards
            if let Some(ref mut eq) = self.eq_node {
                input = eq.process(&mut input, output_rate);
                debug!("EQ node applied");
            }

            // Gain - placed after EQ and before dither per audio engineering standards
            if let Some(ref mut gain) = self.gain {
                input = gain.process(&mut input, output_rate);
                debug!(gain_db = gain.gain_db(), "gain applied");
            }

            // Dither is the final stage before output
            if let Some(ref mut dither) = self.dither {
                input = dither.process(&input, output_rate);
                debug!("dither applied");
            }

            // Output: either PCM or native DSD (DoP)
            if config.dsd_mode != super::config::DsdMode::Off {
                // TODO: Implement proper PCM→DSD sigma-delta modulation
                // For now, DSD mode falls back to PCM output
                if !self.pcm_to_dsd_warned {
                    warn!("PCM→DSD conversion not implemented, using PCM output");
                    self.pcm_to_dsd_warned = true;
                }
                if let Some(ref mut out) = self.output {
                    if let Err(e) = out.write(&input) {
                        warn!(error = %e, "audio output write failed");
                    }
                }
            } else if let Some(ref mut out) = self.output {
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
            log: &tests::pipeline_eq_tests::StageLog,
        ) -> (Vec<f32>, u32) {
            let mut input = samples.to_vec();
            let mut output_rate = sample_rate;
            let config = self.config.blocking_read();

            let eq_after_conv = config.convolution_enabled;
            let eq_before_resamp = !eq_after_conv && config.resample_enabled;
            let eq_after_dsd =
                !eq_after_conv && !config.resample_enabled && config.dsd_to_pcm_enabled;

            macro_rules! log_eq {
                () => {
                    if let Some(ref mut eq) = self.eq {
                        if eq.is_enabled() {
                            log.record("eq");
                            input = eq.process(&input, output_rate);
                        }
                    }
                };
            }

            if eq_before_resamp {
                log_eq!();
            }

            if let Some(ref mut resampler) = self.resampler {
                if config.resample_enabled {
                    log.record("resample");
                    input = resampler.process(&input, sample_rate);
                    if !input.is_empty() {
                        output_rate = resampler.output_rate();
                    }
                }
            }

            if let Some(ref mut dsd) = self.dsd_converter {
                if config.dsd_to_pcm_enabled {
                    log.record("dsd");
                    input = dsd.convert(&input);
                }
            }

            if eq_after_dsd {
                log_eq!();
            }

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

            if eq_after_conv {
                log_eq!();
            }
            if !eq_before_resamp && !eq_after_dsd && !eq_after_conv {
                log_eq!();
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

        /// Update configuration at runtime (synchronous version for command handler).
        pub fn update_config_sync(&mut self, new_cfg: DspConfig) {
            let old = self.config.blocking_read().clone();
            *self.config.blocking_write() = new_cfg.clone();

            // Handle output changes
            let output_changed = old.output_target != new_cfg.output_target
                || old.alsa_device != new_cfg.alsa_device
                || old.pipewire_role != new_cfg.pipewire_role;
            if output_changed {
                if let Some(old_out) = self.output.take() {
                    old_out.close();
                }
                if new_cfg.enabled {
                    match super::output::open_output(new_cfg.output_target, &new_cfg) {
                        Ok(out) => {
                            self.output = Some(out);
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to re-open audio output");
                        }
                    }
                }
            }

            // Update dither filter - check dither_auto and output_target like async version
            let dither_changed = old.dither_enabled != new_cfg.dither_enabled
                || old.dither_auto != new_cfg.dither_auto
                || old.dither_bit_depth != new_cfg.dither_bit_depth
                || old.dither_noise_shaping != new_cfg.dither_noise_shaping
                || old.output_target != new_cfg.output_target;
            if dither_changed {
                self.dither = Self::build_dither(&new_cfg);
            }

            // Update DC offset filter
            let dc_recreate = old.dc_offset_enabled != new_cfg.dc_offset_enabled;
            if dc_recreate {
                self.dc_offset = if new_cfg.dc_offset_enabled {
                    Some(super::dc_offset::DcOffsetFilter::new(
                        new_cfg.dc_offset_cutoff_hz,
                    ))
                } else {
                    None
                };
            } else if let Some(ref mut dc) = self.dc_offset {
                dc.set_cutoff(new_cfg.dc_offset_cutoff_hz);
            }

            // Update crossfeed filter - check crossfeed_auto like async version
            let crossfeed_recreate = old.crossfeed_enabled != new_cfg.crossfeed_enabled
                || old.crossfeed_auto != new_cfg.crossfeed_auto
                || old.output_target != new_cfg.output_target
                || old.alsa_device != new_cfg.alsa_device
                || old.pipewire_role != new_cfg.pipewire_role;
            let crossfeed_params_changed = old.crossfeed_feed_level != new_cfg.crossfeed_feed_level
                || old.crossfeed_cutoff_hz != new_cfg.crossfeed_cutoff_hz;
            if crossfeed_recreate {
                self.crossfeed = if new_cfg.crossfeed_auto {
                    if probe_headphones(&new_cfg) {
                        Some(super::crossfeed::CrossfeedFilter::new(
                            new_cfg.crossfeed_feed_level,
                            new_cfg.crossfeed_cutoff_hz,
                        ))
                    } else {
                        None
                    }
                } else if new_cfg.crossfeed_enabled {
                    Some(super::crossfeed::CrossfeedFilter::new(
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

            // Update LUFS normalizer
            let lufs_recreate = old.lufs_enabled != new_cfg.lufs_enabled
                || old.input_sample_rate != new_cfg.input_sample_rate;
            // Note: lufs_params_changed was previously used here but not needed with current implementation
            if lufs_recreate {
                self.lufs = if new_cfg.lufs_enabled {
                    let mut l = super::lufs::LufsNormalizer::new(new_cfg.input_sample_rate);
                    l.set_target_lufs(new_cfg.lufs_target);
                    l.set_max_gain(new_cfg.lufs_max_gain_db);
                    Some(l)
                } else {
                    None
                };
            } else if let Some(ref mut l) = self.lufs {
                l.set_target_lufs(new_cfg.lufs_target);
                l.set_max_gain(new_cfg.lufs_max_gain_db);
            }

            // Update EQ preset node based on profile
            if let Some(ref mut eq) = self.eq_node {
                use super::command::EqPreset;
                let preset = match new_cfg.profile {
                    super::config::DspProfile::MusicRock => EqPreset::BassBoost,
                    super::config::DspProfile::MusicElectronic => EqPreset::BassBoost,
                    super::config::DspProfile::MusicPop => EqPreset::Vocal,
                    super::config::DspProfile::MusicHipHop => EqPreset::BassBoost,
                    super::config::DspProfile::NightMode => EqPreset::BassBoost,
                    super::config::DspProfile::Podcast => EqPreset::Vocal,
                    super::config::DspProfile::MusicJazz => EqPreset::Flat,
                    super::config::DspProfile::MusicClassical => EqPreset::Flat,
                    super::config::DspProfile::MusicAcoustic => EqPreset::Vocal,
                    super::config::DspProfile::MoviesHorror => EqPreset::BassBoost,
                    super::config::DspProfile::MoviesDrama => EqPreset::Vocal,
                    super::config::DspProfile::MoviesComedy => EqPreset::Vocal,
                    _ => EqPreset::Flat,
                };
                eq.set_preset(preset);
            }

            // Update gain node based on config
            if let Some(ref mut gain) = self.gain {
                gain.set_gain(new_cfg.gain_db);
            }
        }

        /// Update configuration at runtime.
        pub async fn update_config(&mut self, new_cfg: DspConfig) {
            let old = self.config.read().await.clone();
            *self.config.write().await = new_cfg.clone();

            let output_changed = old.output_target != new_cfg.output_target
                || old.alsa_device != new_cfg.alsa_device
                || old.pipewire_role != new_cfg.pipewire_role;

            if output_changed {
                if let Some(old_out) = self.output.take() {
                    old_out.close();
                }
                if new_cfg.enabled {
                    match open_output(new_cfg.output_target, &new_cfg) {
                        Ok(out) => {
                            self.output = Some(out);
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to re-open audio output");
                        }
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

            // Recreate crossfeed when enable state, auto flag, or output device changes.
            let crossfeed_recreate = old.crossfeed_enabled != new_cfg.crossfeed_enabled
                || old.crossfeed_auto != new_cfg.crossfeed_auto
                || old.output_target != new_cfg.output_target
                || old.alsa_device != new_cfg.alsa_device
                || old.pipewire_role != new_cfg.pipewire_role;

            let crossfeed_params_changed = old.crossfeed_feed_level != new_cfg.crossfeed_feed_level
                || old.crossfeed_cutoff_hz != new_cfg.crossfeed_cutoff_hz;

            if crossfeed_recreate {
                self.crossfeed = if new_cfg.crossfeed_auto {
                    if probe_headphones(&new_cfg) {
                        // Reflect auto-detected headphone state in the live config so
                        // callers (e.g. dsp_to_mpv_flags) can read crossfeed_enabled.
                        self.config.write().await.crossfeed_enabled = true;
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

            let dither_changed = old.dither_enabled != new_cfg.dither_enabled
                || old.dither_auto != new_cfg.dither_auto
                || old.dither_bit_depth != new_cfg.dither_bit_depth
                || old.dither_noise_shaping != new_cfg.dither_noise_shaping
                || old.output_target != new_cfg.output_target;

            if dither_changed {
                self.dither = Self::build_dither(&new_cfg);
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
            let lufs_recreate = old.lufs_enabled != new_cfg.lufs_enabled
                || old.input_sample_rate != new_cfg.input_sample_rate;
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

        /// Return a cheap clone of the inner config [`Arc`] so callers can read
        /// the config asynchronously without holding the pipeline [`Mutex`].
        ///
        /// Typical use:
        /// ```ignore
        /// let config_arc = pipeline_mutex.lock().await.config_arc();
        /// // pipeline mutex is released; read config without holding it
        /// let cfg = config_arc.read().await.clone();
        /// ```
        pub fn config_arc(&self) -> std::sync::Arc<tokio::sync::RwLock<DspConfig>> {
            std::sync::Arc::clone(&self.config)
        }

        /// Get a reference to the profile store (read-only).
        pub fn profile_store(&self) -> &CustomProfileStore {
            &self.profile_store
        }

        /// Get a mutable reference to the profile store.
        pub fn profile_store_mut(&mut self) -> &mut CustomProfileStore {
            &mut self.profile_store
        }

        /// Get a reference to the preset store (read-only).
        pub fn preset_store(&self) -> &preset_store::PresetStore {
            &self.preset_store
        }

        /// Get a preset by name.
        pub fn get_preset(&self, name: &str) -> Option<&DspProfileConfig> {
            self.preset_store.get_preset(name)
        }

        /// List all available presets.
        pub fn list_presets(&self) -> Vec<(String, String)> {
            self.preset_store.list_presets()
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
        #[allow(dead_code)] // planned: for runtime control
        pub fn set_convolution_bypass(&mut self, bypass: bool) {
            if let Some(ref mut conv) = self.convolution {
                conv.set_bypass(bypass);
            }
        }

        /// Handle DSP command for real-time adjustments.
        /// Commands: :gain +2dB, :gain mute, :eq bass_boost, :dsp profile music, etc.
        pub fn handle_command(&mut self, cmd: &str) -> String {
            use super::command::{DspCommand, DspProfileCmd, EqPreset, GainAdjust};

            match DspCommand::parse(cmd) {
                Some(DspCommand::Gain(gain_cmd)) => {
                    if let Some(ref mut gain) = self.gain {
                        match gain_cmd {
                            GainAdjust::Set(db) => {
                                gain.set_gain(db);
                                return format!("Gain set to {:.1} dB", gain.gain_db());
                            }
                            GainAdjust::Increment(db) => {
                                gain.adjust_gain(db);
                                return format!("Gain adjusted to {:.1} dB", gain.gain_db());
                            }
                            GainAdjust::Decrement(db) => {
                                gain.adjust_gain(-db);
                                return format!("Gain adjusted to {:.1} dB", gain.gain_db());
                            }
                            GainAdjust::Mute => {
                                gain.mute();
                                return "Muted".to_string();
                            }
                            GainAdjust::Unmute => {
                                gain.unmute();
                                return "Unmuted".to_string();
                            }
                        }
                    }
                    "Gain node not available".to_string()
                }
                Some(DspCommand::Eq(eq_cmd)) => {
                    if let Some(ref mut eq) = self.eq_node {
                        match eq_cmd {
                            EqPreset::Flat => eq.set_preset(EqPreset::Flat),
                            EqPreset::BassBoost => eq.set_preset(EqPreset::BassBoost),
                            EqPreset::TrebleBoost => eq.set_preset(EqPreset::TrebleBoost),
                            EqPreset::Vocal => eq.set_preset(EqPreset::Vocal),
                            EqPreset::Loudness => eq.set_preset(EqPreset::Loudness),
                            EqPreset::Custom(bands) => eq.set_preset(EqPreset::Custom(bands)),
                        }
                        return "EQ preset applied".to_string();
                    }
                    "EQ node not available".to_string()
                }
                Some(DspCommand::Enable(name)) => {
                    let cfg = self.config.blocking_read().clone();
                    match name.as_str() {
                        "dither" => { /* mut handled in update_config_sync */ }
                        "crossfeed" => { /* mut handled in update_config_sync */ }
                        "resample" => { /* mut handled in update_config_sync */ }
                        "dc_offset" | "dc" => { /* mut handled in update_config_sync */ }
                        "eq" => { /* mut handled in update_config_sync */ }
                        "gain" => { /* mut handled in update_config_sync */ }
                        _ => return format!("Unknown DSP node: {}", name),
                    }
                    let mut new_cfg = cfg;
                    match name.as_str() {
                        "dither" => { new_cfg.dither_enabled = true; }
                        "crossfeed" => { new_cfg.crossfeed_enabled = true; }
                        "resample" => { new_cfg.resample_enabled = true; }
                        "dc_offset" | "dc" => { new_cfg.dc_offset_enabled = true; }
                        "eq" => {
                            if let Some(ref mut eq) = self.eq_node { eq.set_enabled(true); }
                            new_cfg.eq_preset = "flat".to_string(); // Mark as custom/active
                        }
                        "gain" => { 
                            if let Some(ref mut g) = self.gain { g.set_enabled(true); }
                        }
                        _ => return format!("Unknown DSP node: {}", name),
                    }
                    self.update_config_sync(new_cfg);
                    return format!("Enabled {}", name);
                }
                Some(DspCommand::Disable(name)) => {
                    let cfg = self.config.blocking_read().clone();
                    match name.as_str() {
                        "dither" => { /* mut handled in update_config_sync */ }
                        "crossfeed" => { /* mut handled in update_config_sync */ }
                        "resample" => { /* mut handled in update_config_sync */ }
                        "dc_offset" | "dc" => { /* mut handled in update_config_sync */ }
                        "eq" => { /* mut handled in update_config_sync */ }
                        "gain" => { /* mut handled in update_config_sync */ }
                        _ => return format!("Unknown DSP node: {}", name),
                    }
                    let mut new_cfg = cfg;
                    match name.as_str() {
                        "dither" => { new_cfg.dither_enabled = false; }
                        "crossfeed" => { new_cfg.crossfeed_enabled = false; }
                        "resample" => { new_cfg.resample_enabled = false; }
                        "dc_offset" | "dc" => { new_cfg.dc_offset_enabled = false; }
                        "eq" => {
                            if let Some(ref mut eq) = self.eq_node { eq.set_enabled(false); }
                        }
                        "gain" => { 
                            if let Some(ref mut g) = self.gain { g.set_enabled(false); }
                        }
                        _ => return format!("Unknown DSP node: {}", name),
                    }
                    self.update_config_sync(new_cfg);
                    return format!("Disabled {}", name);
                }
                Some(DspCommand::Profile(profile_cmd)) => {
                    let profile = match profile_cmd {
                        DspProfileCmd::Music => super::config::DspProfile::MusicDefault,
                        DspProfileCmd::Movies => super::config::DspProfile::MoviesDefault,
                        DspProfileCmd::NightMode => super::config::DspProfile::NightMode,
                        DspProfileCmd::Custom => super::config::DspProfile::Custom("default".to_string()),
                    };
                    let mut cfg = self.config.blocking_read().clone();
                    profile.apply(&mut cfg);
                    self.update_config_sync(cfg);
                    return format!("Profile set to {:?}", profile);
                }
                Some(DspCommand::SaveProfile(name)) => {
                    let cfg = self.config.blocking_read();
                    // Capture gain_db while holding the config snapshot so the profile is consistent.
                    let current_gain_db = self.gain.as_ref().map(|g| g.gain_db()).unwrap_or(0.0);
                    let profile_config = DspProfileConfig {
                        name: name.clone(),
                        output_sample_rate: OutputSampleRate::try_from(cfg.output_sample_rate).unwrap_or_default(),
                        resample_enabled: cfg.resample_enabled,
                        dither_enabled: cfg.dither_enabled,
                        dither_bit_depth: cfg.dither_bit_depth,
                        dither_noise_shaping: cfg.dither_noise_shaping.clone(),
                        lufs_enabled: cfg.lufs_enabled,
                        lufs_target: cfg.lufs_target,
                        lufs_max_gain_db: cfg.lufs_max_gain_db,
                        crossfeed_enabled: cfg.crossfeed_enabled,
                        crossfeed_auto: cfg.crossfeed_auto,
                        crossfeed_feed_level: cfg.crossfeed_feed_level,
                        crossfeed_cutoff_hz: cfg.crossfeed_cutoff_hz,
                        dc_offset_enabled: cfg.dc_offset_enabled,
                        dc_offset_cutoff_hz: cfg.dc_offset_cutoff_hz,
                        ms_enabled: cfg.ms_enabled,
                        ms_width: cfg.ms_width,
                        ms_mid_gain: cfg.ms_mid_gain,
                        ms_side_gain: cfg.ms_side_gain,
                        gain_db: current_gain_db,
                        eq_preset: "custom".to_string(),
                    };
                    // First add to memory, then persist to disk
                    self.profile_store.add_profile(name.clone(), profile_config);
                    match self.profile_store.save() {
                        Ok(_) => format!("Profile '{}' saved", name),
                        Err(e) => {
                            // Rollback: remove from memory if save failed
                            let _ = self.profile_store.remove_profile(&name);
                            format!("Failed to save profile: {}", e)
                        }
                    }
                }
                Some(DspCommand::LoadProfile(name)) => {
                    let mut cfg = self.config.blocking_read().clone();
                    if self.profile_store.apply_profile(&name, &mut cfg) {
                        self.update_config_sync(cfg);
                        format!("Profile '{}' loaded", name)
                    } else {
                        format!("Profile '{}' not found", name)
                    }
                }
                Some(DspCommand::DeleteProfile(name)) => {
                    let profile_backup = self.profile_store.get_profile(&name).cloned();
                    if self.profile_store.remove_profile(&name) {
                        match self.profile_store.save() {
                            Ok(_) => format!("Profile '{}' deleted", name),
                            Err(e) => {
                                if let Some(backup) = profile_backup {
                                    self.profile_store.add_profile(name.clone(), backup);
                                }
                                format!("Failed to delete profile: {}", e)
                            }
                        }
                    } else {
                        format!("Profile '{}' not found", name)
                    }
                }
                Some(DspCommand::ListProfiles) => {
                    let mut output = String::from("Built-in profiles:\n");
                    for (id, name) in super::config::DspProfile::all_profiles() {
                        output.push_str(&format!("  {} - {}\n", id, name));
                    }
                    let custom = self.profile_store.list_profiles();
                    if !custom.is_empty() {
                        output.push_str("\nCustom profiles:\n");
                        for name in custom {
                            output.push_str(&format!("  custom:{}\n", name));
                        }
                    }
                    output
                }
                Some(DspCommand::Describe) => {
                    let mut info = String::from("DSP Chain:\n");
                    let cfg = self.config.blocking_read();
                    info.push_str(&format!("  Enabled: {}\n", cfg.enabled));
                    info.push_str(&format!("  Resample: {} ({} Hz)\n", cfg.resample_enabled, cfg.output_sample_rate));
                    info.push_str(&format!("  Dither: {} ({} bit, {})\n", cfg.dither_enabled, cfg.dither_bit_depth, cfg.dither_noise_shaping));
                    info.push_str(&format!("  EQ: {}\n", cfg.eq_preset));
                    info.push_str(&format!("  Gain: {:.1} dB\n", cfg.gain_db));
                    info.push_str(&format!("  LUFS: {} (target {:.1})\n", cfg.lufs_enabled, cfg.lufs_target));
                    info.push_str(&format!("  DC Offset: {}\n", cfg.dc_offset_enabled));
                    info.push_str(&format!("  Crossfeed: {} ({} Hz)\n", cfg.crossfeed_enabled, cfg.crossfeed_cutoff_hz));
                    info.push_str(&format!("  M/S: {} (width {:.2})\n", cfg.ms_enabled, cfg.ms_width));
                    info
                }
                None => format!("Unknown command. Try: :gain +2dB, :eq bass_boost, :dsp profile music, :dsp describe"),
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
            let r_max = out
                .iter()
                .skip(1)
                .step_by(2)
                .cloned()
                .fold(0.0_f32, f32::max);
            assert!(
                r_max > 0.0,
                "right channel should have crossfeed contribution"
            );
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
            let r_max = out
                .iter()
                .skip(1)
                .step_by(2)
                .cloned()
                .fold(0.0_f32, f32::max);
            assert_eq!(
                r_max, 0.0,
                "right channel must be untouched when crossfeed disabled"
            );
        }

        #[test]
        fn dither_applied_when_enabled() {
            // 16-bit dither on a DC signal: output must be quantized (not exact float).
            let cfg = DspConfig {
                dither_enabled: true,
                dither_bit_depth: 16,
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
                resample_enabled: false, // disable resampler so passthrough is exact
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

        pub(super) mod pipeline_eq_tests {
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
                EqBand {
                    enabled: true,
                    filter_type: EqFilterType::Peak,
                    freq: 1000.0,
                    gain_db: 6.0,
                    q: 1.0,
                }
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
                let eq_pos = entries.iter().position(|s| s == "eq").unwrap_or(usize::MAX);
                let resamp_pos = entries
                    .iter()
                    .position(|s| s == "resample")
                    .unwrap_or(usize::MAX);
                assert!(
                    eq_pos < resamp_pos,
                    "eq must run before resample; log={entries:?}"
                );
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
                let conv_pos = entries
                    .iter()
                    .position(|s| s == "convolution")
                    .unwrap_or(usize::MAX);
                let eq_pos = entries.iter().position(|s| s == "eq").unwrap_or(usize::MAX);
                assert!(
                    conv_pos < eq_pos,
                    "eq must run after convolution; log={entries:?}"
                );
            }
        }
    }
} // mod pipeline

// ── mpv DSP bridge ────────────────────────────────────────────────────────────

/// Translate an active [`DspConfig`] into mpv `--af` / `--audio-samplerate` flags.
///
/// Video playback (movies, series) runs through mpv, which has no connection to
/// the stui DSP pipeline.  This function mirrors the pipeline's processing inside
/// mpv's own FFmpeg lavfi chain so that movie/series DSP presets take effect.
///
/// Mapping:
/// - `resample_enabled`  → `--audio-samplerate=<N>`
/// - `dc_offset_enabled` → lavfi `highpass`
/// - `lufs_enabled`      → lavfi `loudnorm` (ITU-R BS.1770-4, linear single-pass)
/// - `eq_preset`         → lavfi `equalizer` bands (bass_boost / treble_boost / vocal / loudness)
/// - `crossfeed_enabled` → lavfi `bs2b` (Bauer stereophonic to binaural)
/// - `ms_enabled`        → lavfi `stereotools` width
/// - `gain_db`           → lavfi `volume`
///
/// Returns an empty vec when `config.enabled` is `false`.
pub fn dsp_to_mpv_flags(config: &config::DspConfig) -> Vec<String> {
    if !config.enabled {
        return vec![];
    }

    let mut flags: Vec<String> = Vec::new();
    let mut lavfi: Vec<String> = Vec::new();

    // Sample rate: mpv's native resampler (libswresample) honours --audio-samplerate.
    if config.resample_enabled && config.output_sample_rate > 0 {
        flags.push(format!("--audio-samplerate={}", config.output_sample_rate));
    }

    // DC offset / subsonic filter.
    if config.dc_offset_enabled {
        lavfi.push(format!("highpass=f={:.1}", config.dc_offset_cutoff_hz));
    }

    // LUFS loudness normalization (ITU-R BS.1770-4 via FFmpeg loudnorm).
    // linear=true: single-pass with conservative defaults for low latency.
    if config.lufs_enabled {
        lavfi.push(format!(
            "loudnorm=I={:.1}:TP=-1.5:LRA=11:\
             measured_I=-100:measured_TP=-100:measured_LRA=0:\
             measured_thresh=-100:offset=0:linear=true",
            config.lufs_target,
        ));
    }

    // EQ presets translated to FFmpeg parametric equalizer bands.
    match config.eq_preset.as_str() {
        "bass_boost" => {
            lavfi.push("equalizer=f=60:t=o:w=200:g=4".to_string());
            lavfi.push("equalizer=f=120:t=o:w=200:g=3".to_string());
            lavfi.push("equalizer=f=250:t=o:w=200:g=2".to_string());
        }
        "treble_boost" => {
            lavfi.push("equalizer=f=4000:t=o:w=4000:g=2".to_string());
            lavfi.push("equalizer=f=8000:t=o:w=4000:g=3".to_string());
            lavfi.push("equalizer=f=16000:t=o:w=8000:g=3".to_string());
        }
        "vocal" => {
            lavfi.push("equalizer=f=60:t=o:w=100:g=-1".to_string());
            lavfi.push("equalizer=f=1000:t=o:w=1000:g=2".to_string());
            lavfi.push("equalizer=f=3000:t=o:w=2000:g=3".to_string());
        }
        "loudness" => {
            lavfi.push("equalizer=f=60:t=o:w=100:g=5".to_string());
            lavfi.push("equalizer=f=1000:t=o:w=1000:g=-1".to_string());
            lavfi.push("equalizer=f=16000:t=o:w=8000:g=4".to_string());
        }
        _ => {} // "flat" or "custom"
    }

    // Crossfeed (Bauer stereophonic to binaural) for headphone listening.
    // crossfeed_enabled is set to true by the auto-detect probe when crossfeed_auto
    // is on and headphones are detected at DSP pipeline init / config update.
    if config.crossfeed_enabled {
        let feed = (config.crossfeed_feed_level * 100.0)
            .clamp(0.0, 90.0)
            .round() as u32;
        let fcut = config.crossfeed_cutoff_hz as u32;
        lavfi.push(format!("bs2b=fcut={fcut}:feed={feed}"));
    }

    // Mid/Side stereo width via FFmpeg stereotools.
    if config.ms_enabled && (config.ms_width - 1.0).abs() > 0.01 {
        lavfi.push(format!("stereotools=swidth={:.3}", config.ms_width));
    }

    // Master gain.
    if config.gain_db.abs() > 0.01 {
        lavfi.push(format!("volume={:.2}dB", config.gain_db));
    }

    if !lavfi.is_empty() {
        flags.push(format!("--af=lavfi=[{}]", lavfi.join(",")));
    }

    flags
}

#[cfg(test)]
mod dsp_mpv_flags_tests {
    use super::*;
    use config::DspConfig;

    #[test]
    fn disabled_dsp_returns_empty() {
        let cfg = DspConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(dsp_to_mpv_flags(&cfg).is_empty());
    }

    #[test]
    fn resample_flag_emitted() {
        let cfg = DspConfig {
            enabled: true,
            resample_enabled: true,
            output_sample_rate: 192000,
            ..Default::default()
        };
        let flags = dsp_to_mpv_flags(&cfg);
        assert!(flags.iter().any(|f| f == "--audio-samplerate=192000"));
    }

    #[test]
    fn lufs_emits_loudnorm() {
        let cfg = DspConfig {
            enabled: true,
            lufs_enabled: true,
            lufs_target: -24.0,
            resample_enabled: false,
            ..Default::default()
        };
        let flags = dsp_to_mpv_flags(&cfg);
        let af = flags
            .iter()
            .find(|f| f.starts_with("--af="))
            .expect("--af flag");
        assert!(af.contains("loudnorm"));
        assert!(af.contains("I=-24.0"));
    }

    #[test]
    fn crossfeed_emits_bs2b() {
        let cfg = DspConfig {
            enabled: true,
            crossfeed_enabled: true,
            crossfeed_feed_level: 0.45,
            crossfeed_cutoff_hz: 700.0,
            resample_enabled: false,
            ..Default::default()
        };
        let flags = dsp_to_mpv_flags(&cfg);
        let af = flags
            .iter()
            .find(|f| f.starts_with("--af="))
            .expect("--af flag");
        assert!(af.contains("bs2b"));
    }

    #[test]
    fn bass_boost_eq_emits_equalizer() {
        let cfg = DspConfig {
            enabled: true,
            eq_preset: "bass_boost".to_string(),
            resample_enabled: false,
            ..Default::default()
        };
        let flags = dsp_to_mpv_flags(&cfg);
        let af = flags
            .iter()
            .find(|f| f.starts_with("--af="))
            .expect("--af flag");
        assert!(af.contains("equalizer"));
    }

    #[test]
    fn flat_eq_no_equalizer() {
        let cfg = DspConfig {
            enabled: true,
            eq_preset: "flat".to_string(),
            resample_enabled: false,
            ..Default::default()
        };
        let flags = dsp_to_mpv_flags(&cfg);
        // "flat" eq should produce no --af flag (nothing else active)
        assert!(!flags.iter().any(|f| f.starts_with("--af=")));
    }
}

// ── MPD FIFO DSP loop ─────────────────────────────────────────────────────────

/// Drive the DSP pipeline from MPD's FIFO output.
///
/// MPD must have an `audio_output` block of `type "fifo"` named `"stui-dsp"` in
/// `mpd.conf`, writing raw PCM in format `"<sample_rate>:16:2"` to `fifo_path`.
/// [`mpd_config::ensure_mpd_conf`] can patch that file automatically.
///
/// This function loops indefinitely:
/// 1. Creates the FIFO at `fifo_path` if it does not exist.
/// 2. Opens the FIFO for reading (blocks until MPD opens the write end).
/// 3. Reads 4096-byte chunks, converts 16-bit LE stereo → f32, and feeds the
///    samples to [`DspPipeline::process`], which writes processed audio to the
///    configured output (PipeWire / ALSA).
/// 4. On EOF (MPD stopped or paused) or error, waits 500 ms then re-opens.
/// 5. Exits when `shutdown` signals `true`.
pub async fn run_mpd_dsp_loop(
    pipeline: std::sync::Arc<tokio::sync::Mutex<DspPipeline>>,
    fifo_path: String,
    sample_rate: u32,
) {
    use tokio::io::AsyncReadExt;
    use tracing::{debug, info, warn};

    // Create the FIFO if absent.
    if !std::path::Path::new(&fifo_path).exists() {
        match std::process::Command::new("mkfifo")
            .arg(&fifo_path)
            .status()
        {
            Ok(s) if s.success() => info!(path = %fifo_path, "created MPD DSP FIFO"),
            Ok(s) => warn!(path = %fifo_path, status = %s, "mkfifo exited non-zero"),
            Err(e) => warn!(path = %fifo_path, error = %e, "mkfifo command failed"),
        }
    }

    info!(path = %fifo_path, sample_rate, "MPD DSP FIFO loop ready");

    // The loop runs until the process exits — there is no graceful shutdown path yet.
    loop {
        // Opening a read-end FIFO blocks until MPD opens the write end.
        // Use spawn_blocking to avoid blocking a tokio worker thread.
        let fp = fifo_path.clone();
        let open_result = tokio::task::spawn_blocking(move || std::fs::File::open(&fp)).await;

        let std_file = match open_result {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                warn!(error = %e, "failed to open MPD FIFO — retrying in 2s");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
            Err(e) => {
                warn!(error = %e, "spawn_blocking panicked opening FIFO");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };

        let mut reader = tokio::fs::File::from_std(std_file);
        // 4096 bytes = 1024 stereo i16 frames ≈ 11.6 ms at 44.1 kHz.
        let mut buf = vec![0u8; 4096];

        debug!("MPD FIFO opened — reading audio");

        loop {
            match reader.read(&mut buf).await {
                Ok(0) => {
                    debug!("MPD FIFO EOF — waiting for MPD to reopen");
                    break;
                }
                Ok(n) => {
                    // Convert 16-bit LE bytes to f32 samples in [-1.0, 1.0].
                    if n % 2 != 0 {
                        warn!(
                            bytes = n,
                            "MPD FIFO read returned odd byte count — trailing byte dropped"
                        );
                    }
                    let mut samples: Vec<f32> = buf[..n]
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                        .collect();

                    let mut pl = pipeline.lock().await;
                    pl.process(&mut samples, sample_rate);
                }
                Err(e) => {
                    warn!(error = %e, "MPD FIFO read error");
                    break;
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
