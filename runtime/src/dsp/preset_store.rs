use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::dsp::config::{DspProfile, DspProfileConfig, OutputSampleRate};

const PRESETS_FILE: &str = "presets.json";

/// On-disk representation: only user customizations are persisted.
/// Built-in presets are never written unless the user has modified them.
/// Supports the legacy `"presets"` field name via serde alias for migration.
#[derive(Serialize, Deserialize, Default)]
struct PresetStoreDisk {
    #[serde(default, alias = "presets")]
    user_customizations: HashMap<String, DspProfileConfig>,
}

/// Preset store: built-in presets merged with user customizations at runtime.
///
/// Only user customizations (new presets and overrides to built-ins) are
/// written to disk. Built-in presets are reconstructed from code on every load.
#[derive(Debug, Clone, Default)]
pub struct PresetStore {
    /// Merged runtime view: built-ins + user customizations (user wins on conflict).
    pub presets: HashMap<String, DspProfileConfig>,
    /// User-defined customizations. This is the only data persisted to disk.
    user_customizations: HashMap<String, DspProfileConfig>,
    #[allow(dead_code)] // planned: used when saving user presets to disk
    config_dir: Option<PathBuf>,
}

#[allow(dead_code)] // planned: DSP preset management pub API, called from DspPipeline
impl PresetStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(config_dir: &PathBuf) -> Self {
        let path = config_dir.join(PRESETS_FILE);

        let user_customizations = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str::<PresetStoreDisk>(&contents) {
                    Ok(disk) => disk.user_customizations,
                    Err(e) => {
                        eprintln!("Failed to parse presets: {}", e);
                        HashMap::new()
                    }
                },
                Err(e) => {
                    eprintln!("Failed to read presets file: {}", e);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        let mut store = Self {
            config_dir: Some(config_dir.clone()),
            user_customizations,
            ..Default::default()
        };
        store.build_merged_presets();
        store
    }

    /// Rebuild the runtime `presets` map: start with built-ins, overlay user customizations.
    fn build_merged_presets(&mut self) {
        self.presets.clear();
        self.load_builtin_presets();
        for (k, v) in &self.user_customizations {
            self.presets.insert(k.clone(), v.clone());
        }
    }

    fn load_builtin_presets(&mut self) {
        self.presets
            .insert("music_default".to_string(), Self::music_default());
        self.presets
            .insert("music_jazz".to_string(), Self::music_jazz());
        self.presets
            .insert("music_classical".to_string(), Self::music_classical());
        self.presets
            .insert("music_rock".to_string(), Self::music_rock());
        self.presets
            .insert("music_electronic".to_string(), Self::music_electronic());
        self.presets
            .insert("music_pop".to_string(), Self::music_pop());
        self.presets
            .insert("music_hiphop".to_string(), Self::music_hiphop());
        self.presets
            .insert("music_acoustic".to_string(), Self::music_acoustic());
        self.presets
            .insert("movies_default".to_string(), Self::movies_default());
        self.presets
            .insert("movies_action".to_string(), Self::movies_action());
        self.presets
            .insert("movies_drama".to_string(), Self::movies_drama());
        self.presets
            .insert("movies_comedy".to_string(), Self::movies_comedy());
        self.presets
            .insert("movies_horror".to_string(), Self::movies_horror());
        self.presets
            .insert("movies_scifi".to_string(), Self::movies_scifi());
        self.presets
            .insert("movies_animation".to_string(), Self::movies_animation());
        self.presets
            .insert("night_mode".to_string(), Self::night_mode());
        self.presets.insert("podcast".to_string(), Self::podcast());
    }

    pub fn save(&self) -> Result<(), String> {
        let config_dir = self.config_dir.as_ref().ok_or("config_dir not set")?;

        fs::create_dir_all(config_dir)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;

        let disk = PresetStoreDisk {
            user_customizations: self.user_customizations.clone(),
        };

        let json = serde_json::to_string_pretty(&disk)
            .map_err(|e| format!("Failed to serialize presets: {}", e))?;

        fs::write(config_dir.join(PRESETS_FILE), json)
            .map_err(|e| format!("Failed to write presets file: {}", e))?;

        Ok(())
    }

    pub fn get_preset(&self, name: &str) -> Option<&DspProfileConfig> {
        self.presets.get(name)
    }

    /// Add or update a preset. Writes to user customizations so the change
    /// is persisted on the next call to [`save`].
    pub fn upsert_preset(&mut self, name: String, config: DspProfileConfig) {
        self.user_customizations
            .insert(name.clone(), config.clone());
        self.presets.insert(name, config);
    }

    pub fn list_presets(&self) -> Vec<(String, String)> {
        let known: HashMap<String, String> = DspProfile::all_profiles().into_iter().collect();
        self.presets
            .iter()
            .map(|(id, config)| {
                let display = known
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| config.name.clone());
                (id.clone(), display)
            })
            .collect()
    }

    pub fn music_default() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_default".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: false,
            dither_bit_depth: 24,
            dither_noise_shaping: "none".to_string(),
            lufs_enabled: false,
            lufs_target: -14.0,
            lufs_max_gain_db: 12.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.45,
            crossfeed_cutoff_hz: 700.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 10.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn music_jazz() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_jazz".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "gesemann".to_string(),
            lufs_enabled: false,
            lufs_target: -16.0,
            lufs_max_gain_db: 6.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.3,
            crossfeed_cutoff_hz: 500.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 8.0,
            ms_enabled: true,
            ms_width: 1.2,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -1.0,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn music_classical() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_classical".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: false,
            lufs_target: -20.0,
            lufs_max_gain_db: 3.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.2,
            crossfeed_cutoff_hz: 400.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 5.0,
            ms_enabled: true,
            ms_width: 1.1,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -2.0,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn music_rock() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_rock".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: false,
            lufs_target: -12.0,
            lufs_max_gain_db: 9.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.4,
            crossfeed_cutoff_hz: 600.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 10.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -0.5,
            eq_preset: "bass_boost".to_string(),
        }
    }

    pub fn music_electronic() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_electronic".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: false,
            lufs_target: -11.0,
            lufs_max_gain_db: 9.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.35,
            crossfeed_cutoff_hz: 550.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 10.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -0.5,
            eq_preset: "bass_boost".to_string(),
        }
    }

    pub fn music_pop() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_pop".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: false,
            lufs_target: -12.0,
            lufs_max_gain_db: 9.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.35,
            crossfeed_cutoff_hz: 550.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 10.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -0.5,
            eq_preset: "vocal".to_string(),
        }
    }

    pub fn music_hiphop() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_hiphop".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: false,
            lufs_target: -11.0,
            lufs_max_gain_db: 9.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.4,
            crossfeed_cutoff_hz: 600.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 10.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -0.5,
            eq_preset: "bass_boost".to_string(),
        }
    }

    pub fn music_acoustic() -> DspProfileConfig {
        DspProfileConfig {
            name: "music_acoustic".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "gesemann".to_string(),
            lufs_enabled: false,
            lufs_target: -16.0,
            lufs_max_gain_db: 6.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.25,
            crossfeed_cutoff_hz: 450.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 8.0,
            ms_enabled: true,
            ms_width: 1.15,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -1.5,
            eq_preset: "vocal".to_string(),
        }
    }

    pub fn movies_default() -> DspProfileConfig {
        DspProfileConfig {
            name: "movies_default".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: true,
            lufs_target: -24.0,
            lufs_max_gain_db: 6.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.5,
            crossfeed_cutoff_hz: 800.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 20.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn movies_action() -> DspProfileConfig {
        DspProfileConfig {
            name: "movies_action".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: true,
            lufs_target: -22.0,
            lufs_max_gain_db: 8.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.5,
            crossfeed_cutoff_hz: 800.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 20.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn movies_drama() -> DspProfileConfig {
        DspProfileConfig {
            name: "movies_drama".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "gesemann".to_string(),
            lufs_enabled: true,
            lufs_target: -24.0,
            lufs_max_gain_db: 4.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.4,
            crossfeed_cutoff_hz: 700.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 15.0,
            ms_enabled: true,
            ms_width: 1.1,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -1.0,
            eq_preset: "vocal".to_string(),
        }
    }

    pub fn movies_comedy() -> DspProfileConfig {
        DspProfileConfig {
            name: "movies_comedy".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "gesemann".to_string(),
            lufs_enabled: true,
            lufs_target: -22.0,
            lufs_max_gain_db: 6.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.4,
            crossfeed_cutoff_hz: 700.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 15.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -0.5,
            eq_preset: "vocal".to_string(),
        }
    }

    pub fn movies_horror() -> DspProfileConfig {
        DspProfileConfig {
            name: "movies_horror".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: true,
            lufs_target: -24.0,
            lufs_max_gain_db: 6.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.55,
            crossfeed_cutoff_hz: 900.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 20.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "bass_boost".to_string(),
        }
    }

    pub fn movies_scifi() -> DspProfileConfig {
        DspProfileConfig {
            name: "movies_scifi".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "shibata".to_string(),
            lufs_enabled: true,
            lufs_target: -22.0,
            lufs_max_gain_db: 7.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.5,
            crossfeed_cutoff_hz: 800.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 20.0,
            ms_enabled: true,
            ms_width: 1.05,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn movies_animation() -> DspProfileConfig {
        DspProfileConfig {
            name: "movies_animation".to_string(),
            output_sample_rate: OutputSampleRate::Hz192000,
            resample_enabled: true,
            dither_enabled: true,
            dither_bit_depth: 24,
            dither_noise_shaping: "gesemann".to_string(),
            lufs_enabled: true,
            lufs_target: -22.0,
            lufs_max_gain_db: 6.0,
            crossfeed_enabled: false,
            crossfeed_auto: true,
            crossfeed_feed_level: 0.45,
            crossfeed_cutoff_hz: 750.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 15.0,
            ms_enabled: true,
            ms_width: 1.1,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: -0.5,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn night_mode() -> DspProfileConfig {
        DspProfileConfig {
            name: "night_mode".to_string(),
            output_sample_rate: OutputSampleRate::Hz96000,
            resample_enabled: true,
            dither_enabled: false,
            dither_bit_depth: 24,
            dither_noise_shaping: "none".to_string(),
            lufs_enabled: true,
            lufs_target: -32.0,
            lufs_max_gain_db: 3.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.4,
            crossfeed_cutoff_hz: 600.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 10.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "flat".to_string(),
        }
    }

    pub fn podcast() -> DspProfileConfig {
        DspProfileConfig {
            name: "podcast".to_string(),
            output_sample_rate: OutputSampleRate::Hz96000,
            resample_enabled: true,
            dither_enabled: false,
            dither_bit_depth: 16,
            dither_noise_shaping: "none".to_string(),
            lufs_enabled: true,
            lufs_target: -16.0,
            lufs_max_gain_db: 6.0,
            crossfeed_enabled: false,
            crossfeed_auto: false,
            crossfeed_feed_level: 0.3,
            crossfeed_cutoff_hz: 500.0,
            dc_offset_enabled: true,
            dc_offset_cutoff_hz: 15.0,
            ms_enabled: false,
            ms_width: 1.0,
            ms_mid_gain: 1.0,
            ms_side_gain: 1.0,
            gain_db: 0.0,
            eq_preset: "vocal".to_string(),
        }
    }
}
