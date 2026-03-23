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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputTarget {
    PipeWire,
    RoonRaat,
    Mpd,
    /// Direct ALSA hardware output (hw: device, no OS mixer).
    Alsa,
}

impl Default for OutputTarget {
    fn default() -> Self {
        Self::PipeWire
    }
}

/// Filter types for parametric EQ bands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EqFilterType {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
}

impl Default for EqFilterType {
    fn default() -> Self { Self::Peak }
}

/// A single parametric EQ band.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqBand {
    pub enabled:     bool,
    pub filter_type: EqFilterType,
    /// Center/corner frequency in Hz (clamped 20.0–20000.0).
    pub freq:        f32,
    /// Gain in dB (clamped ±20.0). Ignored for LowPass, HighPass, Notch.
    pub gain_db:     f32,
    /// Q factor (clamped 0.1–10.0).
    pub q:           f32,
}

impl Default for EqBand {
    fn default() -> Self {
        Self {
            enabled:     true,
            filter_type: EqFilterType::Peak,
            freq:        1000.0,
            gain_db:     0.0,
            q:           1.0,
        }
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
    /// ALSA hardware device string (e.g. "hw:0,0"). None → "hw:0,0".
    pub alsa_device: Option<String>,
    /// PipeWire stream role ("Music" | "Production"). Production requests bypass OS resampler.
    pub pipewire_role: String,
    /// Enable the parametric EQ stage.
    pub eq_enabled: bool,
    /// Bypass all EQ bands (pass-through).
    pub eq_bypass:  bool,
    /// Parametric EQ band definitions (max 10).
    pub eq_bands:   Vec<EqBand>,
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
            alsa_device: None,
            pipewire_role: "Music".to_string(),
            eq_enabled: false,
            eq_bypass:  false,
            eq_bands:   Vec::new(),
        }
    }
}

#[cfg(test)]
mod eq_config_tests {
    use super::*;
    use serde_json;

    #[test]
    fn eq_band_roundtrip() {
        let band = EqBand {
            enabled:     true,
            filter_type: EqFilterType::Peak,
            freq:        1000.0,
            gain_db:     3.0,
            q:           1.0,
        };
        let json = serde_json::to_string(&band).unwrap();
        let back: EqBand = serde_json::from_str(&json).unwrap();
        assert_eq!(back.freq, 1000.0);
        assert_eq!(back.gain_db, 3.0);
    }

    #[test]
    fn dsp_config_eq_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.eq_enabled);
        assert!(!cfg.eq_bypass);
        assert!(cfg.eq_bands.is_empty());
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
