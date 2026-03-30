use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::dsp::config::DspProfileConfig;

const PROFILES_DIR: &str = "profiles";
const CUSTOM_PROFILES_FILE: &str = "custom_profiles.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomProfileStore {
    pub profiles: HashMap<String, DspProfileConfig>,
    #[serde(skip)]
    config_dir: Option<PathBuf>,
}

impl CustomProfileStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_profiles_path(config_dir: &PathBuf) -> PathBuf {
        config_dir.join(PROFILES_DIR).join(CUSTOM_PROFILES_FILE)
    }

    pub fn load(config_dir: &PathBuf) -> Self {
        let path = Self::get_profiles_path(config_dir);

        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str::<Self>(&contents) {
                    Ok(mut store) => {
                        store.config_dir = Some(config_dir.clone());
                        return store;
                    }
                    Err(e) => {
                        eprintln!("Failed to parse custom profiles: {}", e);
                    }
                },
                Err(e) => {
                    eprintln!("Failed to read custom profiles file: {}", e);
                }
            }
        }

        let mut store = Self::default();
        store.config_dir = Some(config_dir.clone());
        store
    }

    pub fn save(&self) -> Result<(), String> {
        let config_dir = self.config_dir.as_ref().ok_or("config_dir not set")?;

        let profiles_dir = config_dir.join(PROFILES_DIR);

        if !profiles_dir.exists() {
            fs::create_dir_all(&profiles_dir)
                .map_err(|e| format!("Failed to create profiles directory: {}", e))?;
        }

        let path = Self::get_profiles_path(config_dir);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize profiles: {}", e))?;

        fs::write(&path, json).map_err(|e| format!("Failed to write profiles file: {}", e))?;

        Ok(())
    }

    pub fn add_profile(&mut self, name: String, config: DspProfileConfig) {
        self.profiles.insert(name, config);
    }

    pub fn remove_profile(&mut self, name: &str) -> bool {
        self.profiles.remove(name).is_some()
    }

    pub fn get_profile(&self, name: &str) -> Option<&DspProfileConfig> {
        self.profiles.get(name)
    }

    pub fn list_profiles(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn apply_profile(&self, name: &str, config: &mut crate::dsp::config::DspConfig) -> bool {
        if let Some(profile) = self.get_profile(name) {
            profile.apply_to(config);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn test_save_load() {
        let temp_dir = temp_dir();
        let store_path = temp_dir.join("stui_test_profiles");

        let mut store = CustomProfileStore::load(&store_path);
        store.add_profile(
            "test_profile".to_string(),
            DspProfileConfig {
                name: "test_profile".to_string(),
                ..Default::default()
            },
        );

        store.save().unwrap();

        let loaded = CustomProfileStore::load(&store_path);
        assert!(loaded.get_profile("test_profile").is_some());

        fs::remove_dir_all(store_path).ok();
    }

    #[test]
    fn test_apply_profile() {
        use crate::dsp::config::{DspConfig, OutputSampleRate};
        let temp_dir = temp_dir();
        let store_path = temp_dir.join("stui_test_apply_profile");

        let mut store = CustomProfileStore::load(&store_path);
        store.add_profile(
            "my_profile".to_string(),
            DspProfileConfig {
                name: "my_profile".to_string(),
                output_sample_rate: OutputSampleRate::Hz96000,
                gain_db: -3.0,
                ..Default::default()
            },
        );

        let mut config = DspConfig::default();
        assert!(store.apply_profile("my_profile", &mut config));
        assert_eq!(config.output_sample_rate, 96000);
        assert!((config.gain_db - (-3.0_f32)).abs() < f32::EPSILON);

        assert!(!store.apply_profile("nonexistent", &mut config));

        fs::remove_dir_all(store_path).ok();
    }
}
