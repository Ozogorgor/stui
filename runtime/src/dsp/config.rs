//! DSP configuration and settings.

use std::convert::TryFrom;

use crate::dsp::preset_store;
use serde::{Deserialize, Serialize};

/// Supported output sample rates.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum OutputSampleRate {
    Hz96000 = 96000,
    #[default]
    Hz192000 = 192000,
    Hz384000 = 384000,
    Hz768000 = 768000,
}

impl OutputSampleRate {
    #[allow(dead_code)]
    pub fn value(&self) -> u32 {
        *self as u32
    }
}

impl From<OutputSampleRate> for u32 {
    fn from(r: OutputSampleRate) -> u32 {
        r as u32
    }
}

impl TryFrom<u32> for OutputSampleRate {
    type Error = String;

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            96000 => Ok(Self::Hz96000),
            192000 => Ok(Self::Hz192000),
            384000 => Ok(Self::Hz384000),
            768000 => Ok(Self::Hz768000),
            other => Err(format!("unsupported output sample rate: {other}")),
        }
    }
}

/// Upsampling ratios.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[allow(dead_code)] // For future DSP upsampling configuration
pub enum UpsampleRatio {
    Ratio1x = 1,
    Ratio2x = 2,
    #[default]
    Ratio4x = 4,
    Ratio8x = 8,
    Ratio16x = 16,
}

impl UpsampleRatio {
    #[allow(dead_code)]
    pub fn value(&self) -> u32 {
        *self as u32
    }
}

/// Filter types for resampling.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FilterType {
    Fast,
    Slow,
    #[default]
    Synchronous,
}

/// Output mode for DSP processing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OutputMode {
    #[default]
    Pcm,
    Dsd,
    DsdToPcm,
}

/// DSP output targets.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OutputTarget {
    #[default]
    PipeWire,
    RoonRaat,
    Mpd,
    /// Direct ALSA hardware output (hw: device, no OS mixer).
    Alsa,
}

/// DSP profile presets for different media types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DspProfile {
    #[default]
    MusicDefault,
    MusicJazz,
    MusicClassical,
    MusicRock,
    MusicElectronic,
    MusicPop,
    MusicHipHop,
    MusicAcoustic,
    MoviesDefault,
    MoviesAction,
    MoviesDrama,
    MoviesComedy,
    MoviesHorror,
    MoviesSciFi,
    MoviesAnimation,
    NightMode,
    Podcast,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DspProfileConfig {
    pub name: String,
    /// Output sample rate. Validated against supported hardware rates.
    pub output_sample_rate: OutputSampleRate,
    pub resample_enabled: bool,
    pub dither_enabled: bool,
    pub dither_bit_depth: u32,
    pub dither_noise_shaping: String,
    pub lufs_enabled: bool,
    pub lufs_target: f32,
    pub lufs_max_gain_db: f32,
    pub crossfeed_enabled: bool,
    pub crossfeed_auto: bool,
    pub crossfeed_feed_level: f32,
    pub crossfeed_cutoff_hz: f32,
    pub dc_offset_enabled: bool,
    pub dc_offset_cutoff_hz: f32,
    pub ms_enabled: bool,
    pub ms_width: f32,
    /// M/S mid (center) gain. 1.0 = unity.
    pub ms_mid_gain: f32,
    /// M/S side gain. 1.0 = unity.
    pub ms_side_gain: f32,
    pub gain_db: f32,
    pub eq_preset: String,
}

impl Default for DspProfileConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: false,
            dither_bit_depth: 16,
            dither_noise_shaping: "none".to_string(),
            lufs_enabled: false,
            lufs_target: -14.0,
            lufs_max_gain_db: 12.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.45,
            crossfeed_cutoff_hz: 700.0,
            dc_offset_enabled: false,
            dc_offset_cutoff_hz: 30.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "flat".to_string(),
        }
    }
}

impl DspProfileConfig {
    pub fn from_config(config: &DspConfig, name: String) -> Self {
        Self {
            name,
            output_sample_rate: OutputSampleRate::try_from(config.output_sample_rate)
                .unwrap_or_default(),
            resample_enabled: config.resample_enabled,
            dither_enabled: config.dither_enabled,
            dither_bit_depth: config.dither_bit_depth,
            dither_noise_shaping: config.dither_noise_shaping.clone(),
            lufs_enabled: config.lufs_enabled,
            lufs_target: config.lufs_target,
            lufs_max_gain_db: config.lufs_max_gain_db,
            crossfeed_enabled: config.crossfeed_enabled,
            crossfeed_auto: config.crossfeed_auto,
            crossfeed_feed_level: config.crossfeed_feed_level,
            crossfeed_cutoff_hz: config.crossfeed_cutoff_hz,
            dc_offset_enabled: config.dc_offset_enabled,
            dc_offset_cutoff_hz: config.dc_offset_cutoff_hz,
            ms_enabled: config.ms_enabled,
            ms_width: config.ms_width,
            ms_mid_gain: config.ms_mid_gain,
            ms_side_gain: config.ms_side_gain,
            gain_db: config.gain_db,
            eq_preset: config.eq_preset.clone(),
        }
    }

    /// Apply this profile's settings to a live [`DspConfig`].
    ///
    /// The EQ preset is applied unconditionally unless the stored value is
    /// `"custom"`, which signals that the user has a fully hand-tuned EQ that
    /// should not be overwritten by a profile switch.
    pub fn apply_to(&self, config: &mut DspConfig) {
        config.output_sample_rate = u32::from(self.output_sample_rate);
        config.resample_enabled = self.resample_enabled;
        config.dither_enabled = self.dither_enabled;
        config.dither_bit_depth = self.dither_bit_depth;
        config.dither_noise_shaping = self.dither_noise_shaping.clone();
        config.lufs_enabled = self.lufs_enabled;
        config.lufs_target = self.lufs_target;
        config.lufs_max_gain_db = self.lufs_max_gain_db;
        config.crossfeed_enabled = self.crossfeed_enabled;
        config.crossfeed_auto = self.crossfeed_auto;
        config.crossfeed_feed_level = self.crossfeed_feed_level;
        config.crossfeed_cutoff_hz = self.crossfeed_cutoff_hz;
        config.dc_offset_enabled = self.dc_offset_enabled;
        config.dc_offset_cutoff_hz = self.dc_offset_cutoff_hz;
        config.ms_enabled = self.ms_enabled;
        config.ms_width = self.ms_width;
        config.ms_mid_gain = self.ms_mid_gain;
        config.ms_side_gain = self.ms_side_gain;
        config.gain_db = self.gain_db;
        // Apply the EQ preset from the profile, but leave the EQ untouched if
        // the user has set it to "custom" (hand-tuned per-band settings).
        if self.eq_preset != "custom" {
            config.eq_preset = self.eq_preset.clone();
        }
    }
}

impl DspProfile {
    pub fn apply(&self, config: &mut DspConfig) {
        // Custom profiles are applied via CustomProfileStore::apply_profile.
        // Calling apply() directly on a Custom variant is intentionally a no-op;
        // use the profile store to look up and apply the named profile.
        let profile_config = match self {
            DspProfile::Custom(_) => return,
            DspProfile::MusicDefault => preset_store::PresetStore::music_default(),
            DspProfile::MusicJazz => preset_store::PresetStore::music_jazz(),
            DspProfile::MusicClassical => preset_store::PresetStore::music_classical(),
            DspProfile::MusicRock => preset_store::PresetStore::music_rock(),
            DspProfile::MusicElectronic => preset_store::PresetStore::music_electronic(),
            DspProfile::MusicPop => preset_store::PresetStore::music_pop(),
            DspProfile::MusicHipHop => preset_store::PresetStore::music_hiphop(),
            DspProfile::MusicAcoustic => preset_store::PresetStore::music_acoustic(),
            DspProfile::MoviesDefault => preset_store::PresetStore::movies_default(),
            DspProfile::MoviesAction => preset_store::PresetStore::movies_action(),
            DspProfile::MoviesDrama => preset_store::PresetStore::movies_drama(),
            DspProfile::MoviesComedy => preset_store::PresetStore::movies_comedy(),
            DspProfile::MoviesHorror => preset_store::PresetStore::movies_horror(),
            DspProfile::MoviesSciFi => preset_store::PresetStore::movies_scifi(),
            DspProfile::MoviesAnimation => preset_store::PresetStore::movies_animation(),
            DspProfile::NightMode => preset_store::PresetStore::night_mode(),
            DspProfile::Podcast => preset_store::PresetStore::podcast(),
        };

        profile_config.apply_to(config);
        config.profile = self.clone();
    }

    pub fn all_profiles() -> Vec<(String, String)> {
        vec![
            ("music_default".to_string(), "Music: Default".to_string()),
            ("music_jazz".to_string(), "Music: Jazz".to_string()),
            (
                "music_classical".to_string(),
                "Music: Classical".to_string(),
            ),
            ("music_rock".to_string(), "Music: Rock".to_string()),
            (
                "music_electronic".to_string(),
                "Music: Electronic".to_string(),
            ),
            ("music_pop".to_string(), "Music: Pop".to_string()),
            ("music_hiphop".to_string(), "Music: Hip-Hop".to_string()),
            ("music_acoustic".to_string(), "Music: Acoustic".to_string()),
            ("movies_default".to_string(), "Movies: Default".to_string()),
            ("movies_action".to_string(), "Movies: Action".to_string()),
            ("movies_drama".to_string(), "Movies: Drama".to_string()),
            ("movies_comedy".to_string(), "Movies: Comedy".to_string()),
            ("movies_horror".to_string(), "Movies: Horror".to_string()),
            ("movies_scifi".to_string(), "Movies: Sci-Fi".to_string()),
            (
                "movies_animation".to_string(),
                "Movies: Animation".to_string(),
            ),
            ("night_mode".to_string(), "Night Mode".to_string()),
            ("podcast".to_string(), "Podcast/Voice".to_string()),
        ]
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "music_default" => Some(DspProfile::MusicDefault),
            "music_jazz" => Some(DspProfile::MusicJazz),
            "music_classical" => Some(DspProfile::MusicClassical),
            "music_rock" => Some(DspProfile::MusicRock),
            "music_electronic" => Some(DspProfile::MusicElectronic),
            "music_pop" => Some(DspProfile::MusicPop),
            "music_hiphop" => Some(DspProfile::MusicHipHop),
            "music_acoustic" => Some(DspProfile::MusicAcoustic),
            "movies_default" => Some(DspProfile::MoviesDefault),
            "movies_action" => Some(DspProfile::MoviesAction),
            "movies_drama" => Some(DspProfile::MoviesDrama),
            "movies_comedy" => Some(DspProfile::MoviesComedy),
            "movies_horror" => Some(DspProfile::MoviesHorror),
            "movies_scifi" => Some(DspProfile::MoviesSciFi),
            "movies_sci_fi" => Some(DspProfile::MoviesSciFi), // Backwards compatibility
            "movies_animation" => Some(DspProfile::MoviesAnimation),
            "night_mode" => Some(DspProfile::NightMode),
            "podcast" => Some(DspProfile::Podcast),
            other if other.starts_with("custom:") => {
                Some(DspProfile::Custom(other[7..].to_string()))
            }
            _ => None,
        }
    }
}

/// DSD format variants for native DSD output.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DsdMode {
    /// Convert DSD to PCM for processing and output.
    #[default]
    Off,
    /// Output native DSD (DSD64 = 2.8MHz).
    Dsd64,
    /// Output native DSD (DSD128 = 5.6MHz).
    Dsd128,
    /// Output native DSD (DSD256 = 11.2MHz).
    Dsd256,
    /// Output native DSD (DSD512 = 22.58MHz).
    Dsd512,
}

impl DsdMode {
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Dsd64 => 2822400,
            Self::Dsd128 => 5644800,
            Self::Dsd256 => 11289600,
            Self::Dsd512 => 22579200,
        }
    }
}

/// Main DSP configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DspConfig {
    /// DSP profile preset (music, movies, night_mode, custom).
    pub profile: DspProfile,
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
    /// Native DSD output mode (off, dsd64, dsd128, dsd256, dsd512).
    pub dsd_mode: DsdMode,
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
    /// Enable crossfeed (set manually, or overridden by auto-detect).
    pub crossfeed_enabled: bool,
    /// When true, probe_headphones() controls crossfeed_enabled at init/update.
    pub crossfeed_auto: bool,
    /// Crossfeed blend level. Clamped 0.0–0.9.
    pub crossfeed_feed_level: f32,
    /// Crossfeed lowpass cutoff frequency in Hz. Clamped 300.0–700.0.
    pub crossfeed_cutoff_hz: f32,
    /// Enable dither (set manually, or overridden by auto-detect).
    pub dither_enabled: bool,
    /// When true, auto-enable dither when output_target == Alsa && dither_bit_depth == 16.
    pub dither_auto: bool,
    /// Output bit depth for quantization. Clamped 8–32.
    pub dither_bit_depth: u32,
    /// Noise shaping algorithm. One of: "none"|"lipshitz"|"fweighted"|"modified_e_weighted"|
    /// "improved_e_weighted"|"shibata"|"low_shibata"|"high_shibata"|"gesemann".
    pub dither_noise_shaping: String,
    /// EQ preset name (flat, bass_boost, treble_boost, vocal, loudness).
    pub eq_preset: String,
    /// Master gain in dB (-20 to +20).
    pub gain_db: f32,
    /// Enable Mid/Side processing.
    pub ms_enabled: bool,
    /// M/S stereo width. 1.0 = normal, 0.0 = mono, >1.0 = wider.
    pub ms_width: f32,
    /// M/S mid (center) gain. 1.0 = unity.
    pub ms_mid_gain: f32,
    /// M/S side gain. 1.0 = unity.
    pub ms_side_gain: f32,
    /// Enable DC offset (high-pass) filter.
    pub dc_offset_enabled: bool,
    /// DC offset filter cutoff frequency in Hz. Typical: 5-20 Hz.
    pub dc_offset_cutoff_hz: f32,
    /// Enable LUFS loudness normalization.
    pub lufs_enabled: bool,
    /// Target LUFS value for normalization. Typical: -14 to -24 LUFS.
    pub lufs_target: f32,
    /// Maximum gain limit in dB for LUFS normalization.
    pub lufs_max_gain_db: f32,
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            profile: DspProfile::default(),
            enabled: false,
            output_sample_rate: 192000,
            input_sample_rate: 44100,
            upsample_ratio: 4,
            filter_type: FilterType::Synchronous,
            resample_enabled: true,
            dsd_to_pcm_enabled: false,
            dsd_output_rate: 352800,
            dsd_mode: DsdMode::Off,
            output_mode: OutputMode::Pcm,
            output_target: OutputTarget::PipeWire,
            convolution_filter_path: None,
            convolution_enabled: false,
            convolution_bypass: true,
            buffer_size: 4096,
            alsa_device: None,
            pipewire_role: "Music".to_string(),
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.45,
            crossfeed_cutoff_hz: 700.0,
            dither_enabled: false,
            dither_auto: false,
            dither_bit_depth: 16,
            dither_noise_shaping: "none".to_string(),
            eq_preset: "flat".to_string(),
            gain_db: 0.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            dc_offset_enabled: false,
            dc_offset_cutoff_hz: 10.0,
            lufs_enabled: false,
            lufs_target: -14.0,
            lufs_max_gain_db: 12.0,
        }
    }
}

/// Trait for DSP stages that can process audio.
#[allow(dead_code)] // For future DSP pipeline construction
pub trait DspStage {
    /// Process audio samples.
    /// Returns processed samples and potentially new sample rate.
    fn process(&mut self, samples: &[f32], sample_rate: u32) -> (Vec<f32>, u32);

    /// Get the name of this stage.
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfeed_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.crossfeed_enabled);
        assert!(!cfg.crossfeed_auto);
        assert!((cfg.crossfeed_feed_level - 0.45_f32).abs() < f32::EPSILON);
        assert!((cfg.crossfeed_cutoff_hz - 700.0_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn dither_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.dither_enabled);
        assert!(!cfg.dither_auto);
        assert_eq!(cfg.dither_bit_depth, 16);
        assert_eq!(cfg.dither_noise_shaping, "none");
    }

    #[test]
    fn ms_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.ms_enabled);
        assert!((cfg.ms_width - 1.0_f32).abs() < f32::EPSILON);
        assert!((cfg.ms_mid_gain - 1.0_f32).abs() < f32::EPSILON);
        assert!((cfg.ms_side_gain - 1.0_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn dc_offset_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.dc_offset_enabled);
        assert!((cfg.dc_offset_cutoff_hz - 10.0_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn lufs_defaults() {
        let cfg = DspConfig::default();
        assert!(!cfg.lufs_enabled);
        assert!((cfg.lufs_target - (-14.0_f32)).abs() < f32::EPSILON);
        assert!((cfg.lufs_max_gain_db - 12.0_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn output_sample_rate_roundtrip() {
        for &rate in &[96000u32, 192000, 384000, 768000] {
            let sr = OutputSampleRate::try_from(rate).expect("valid rate");
            assert_eq!(u32::from(sr), rate);
        }
    }

    #[test]
    fn output_sample_rate_invalid() {
        assert!(OutputSampleRate::try_from(44100u32).is_err());
    }

    #[test]
    fn apply_to_copies_all_fields() {
        let profile = DspProfileConfig {
            name: "test".to_string(),
            output_sample_rate: OutputSampleRate::Hz96000,
            resample_enabled: false,
            dither_enabled: true,
            dither_bit_depth: 16,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: true,
            lufs_target: -23.0,
            lufs_max_gain_db: 5.0,
            crossfeed_enabled: true,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.3,
            crossfeed_cutoff_hz: 500.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 8.0,
            ms_enabled: true,
            ms_width: 1.2,
            ms_mid_gain: 0.9,
            ms_side_gain: 1.1,
            gain_db: -3.0,
            eq_preset: "vocal".to_string(),
        };
        let mut config = DspConfig::default();
        profile.apply_to(&mut config);

        assert_eq!(config.output_sample_rate, 96000);
        assert!(!config.resample_enabled);
        assert!(config.dither_enabled);
        assert_eq!(config.dither_noise_shaping, "shibata");
        assert!(config.lufs_enabled);
        assert!((config.lufs_target - (-23.0_f32)).abs() < f32::EPSILON);
        assert!(config.crossfeed_enabled);
        assert!(config.ms_enabled);
        assert!((config.ms_width - 1.2_f32).abs() < f32::EPSILON);
        assert_eq!(config.eq_preset, "vocal");
    }

    #[test]
    fn apply_to_preserves_custom_eq() {
        let profile = DspProfileConfig {
            eq_preset: "custom".to_string(),
            ..DspProfileConfig::default()
        };
        let mut config = DspConfig::default();
        config.eq_preset = "bass_boost".to_string();
        profile.apply_to(&mut config);
        // "custom" eq_preset must not overwrite the live setting.
        assert_eq!(config.eq_preset, "bass_boost");
    }

    #[test]
    fn apply_to_resets_eq_to_flat() {
        // Previously, switching to a flat-EQ preset would leave the old EQ active.
        // Now apply_to applies "flat" unconditionally.
        let mut config = DspConfig::default();
        config.eq_preset = "bass_boost".to_string();
        let flat_profile = DspProfileConfig {
            eq_preset: "flat".to_string(),
            ..DspProfileConfig::default()
        };
        flat_profile.apply_to(&mut config);
        assert_eq!(config.eq_preset, "flat");
    }
}
