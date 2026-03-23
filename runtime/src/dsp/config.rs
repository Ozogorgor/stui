//! DSP configuration and settings.

use serde::{Deserialize, Serialize};

/// Supported output sample rates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum OutputSampleRate {
    Hz96000 = 96000,
    Hz192000 = 192000,
    Hz384000 = 384000,
    Hz768000 = 768000,
}

impl Default for OutputSampleRate {
    fn default() -> Self {
        Self::Hz192000
    }
}

impl OutputSampleRate {
    pub fn value(&self) -> u32 {
        *self as u32
    }
}

/// Upsampling ratios.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum UpsampleRatio {
    Ratio1x = 1,
    Ratio2x = 2,
    Ratio4x = 4,
    Ratio8x = 8,
    Ratio16x = 16,
}

impl Default for UpsampleRatio {
    fn default() -> Self {
        Self::Ratio4x
    }
}

impl UpsampleRatio {
    pub fn value(&self) -> u32 {
        *self as u32
    }
}

/// Filter types for resampling.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FilterType {
    Fast,
    Slow,
    Synchronous,
}

impl Default for FilterType {
    fn default() -> Self {
        Self::Synchronous
    }
}

/// Output mode for DSP processing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    Pcm,
    Dsd,
    DsdToPcm,
}

impl Default for OutputMode {
    fn default() -> Self {
        Self::Pcm
    }
}

/// DSP output targets.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OutputTarget {
    PipeWire,
    RoonRaat,
    Mpd,
}

impl Default for OutputTarget {
    fn default() -> Self {
        Self::PipeWire
    }
}

/// Main DSP configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DspConfig {
    /// Enable/disable the entire DSP pipeline.
    pub enabled: bool,
    /// Target output sample rate.
    pub output_sample_rate: u32,
    /// Input sample rate (typically 44100 or 48000).
    pub input_sample_rate: u32,
    /// Upsampling ratio (multiplier from input).
    pub upsample_ratio: u32,
    /// Resampling filter type.
    pub filter_type: FilterType,
    /// Enable/disable resampling.
    pub resample_enabled: bool,
    /// Enable DSD to PCM conversion.
    pub dsd_to_pcm_enabled: bool,
    /// DSD to PCM output rate.
    pub dsd_output_rate: u32,
    /// Output mode (PCM, DSD, DSD→PCM).
    pub output_mode: OutputMode,
    /// Output target (PipeWire, RAAT, MPD).
    pub output_target: OutputTarget,
    /// Path to convolution filter file.
    pub convolution_filter_path: Option<String>,
    /// Enable/disable convolution.
    pub convolution_enabled: bool,
    /// Convolution bypass.
    pub convolution_bypass: bool,
    /// Processing buffer size (samples).
    pub buffer_size: usize,
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            output_sample_rate: 192000,
            input_sample_rate: 44100,
            upsample_ratio: 4,
            filter_type: FilterType::Synchronous,
            resample_enabled: true,
            dsd_to_pcm_enabled: false,
            dsd_output_rate: 352800,
            output_mode: OutputMode::Pcm,
            output_target: OutputTarget::PipeWire,
            convolution_filter_path: None,
            convolution_enabled: false,
            convolution_bypass: true,
            buffer_size: 4096,
        }
    }
}

/// Trait for DSP stages that can process audio.
pub trait DspStage {
    /// Process audio samples.
    /// Returns processed samples and potentially new sample rate.
    fn process(&mut self, samples: &[f32], sample_rate: u32) -> (Vec<f32>, u32);

    /// Get the name of this stage.
    fn name(&self) -> &str;
}
