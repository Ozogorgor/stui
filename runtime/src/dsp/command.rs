use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DspCommand {
    Gain(GainAdjust),
    Eq(EqPreset),
    Enable(String),
    Disable(String),
    Profile(DspProfileCmd),
    SaveProfile(String),
    LoadProfile(String),
    ListProfiles,
    DeleteProfile(String),
    Describe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GainAdjust {
    Set(f32),
    Increment(f32),
    Decrement(f32),
    Mute,
    Unmute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EqPreset {
    Flat,
    BassBoost,
    TrebleBoost,
    Vocal,
    Loudness,
    Custom(Vec<EqBand>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqBand {
    pub frequency_hz: u32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DspProfileCmd {
    Music,
    Movies,
    NightMode,
    Custom,
}

#[allow(dead_code)] // planned: DSP command parser, called from IPC/REPL layer
impl DspCommand {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();

        if input == ":dsp" || input == ":dsp describe" {
            return Some(DspCommand::Describe);
        }

        if let Some(rest) = input.strip_prefix(":gain ") {
            return Self::parse_gain(rest).map(DspCommand::Gain);
        }

        if let Some(rest) = input.strip_prefix(":dsp gain ") {
            return Self::parse_gain(rest).map(DspCommand::Gain);
        }

        if let Some(rest) = input.strip_prefix(":eq ") {
            return Self::parse_eq(rest).map(DspCommand::Eq);
        }

        if let Some(rest) = input.strip_prefix(":dsp eq ") {
            return Self::parse_eq(rest).map(DspCommand::Eq);
        }

        if let Some(rest) = input.strip_prefix(":dsp enable ") {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(DspCommand::Enable(name))
        } else if let Some(rest) = input.strip_prefix(":dsp disable ") {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(DspCommand::Disable(name))
        } else if let Some(rest) = input.strip_prefix(":dsp profile ") {
            Self::parse_profile(rest).map(DspCommand::Profile)
        } else if input == ":dsp list" || input == ":dsp profiles" {
            Some(DspCommand::ListProfiles)
        } else if let Some(rest) = input.strip_prefix(":dsp save ") {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(DspCommand::SaveProfile(name))
        } else if let Some(rest) = input.strip_prefix(":dsp load ") {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(DspCommand::LoadProfile(name))
        } else if let Some(rest) = input.strip_prefix(":dsp delete ") {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(DspCommand::DeleteProfile(name))
        } else {
            None
        }
    }

    fn parse_gain(input: &str) -> Option<GainAdjust> {
        let input = input.trim();

        match input {
            "mute"   => return Some(GainAdjust::Mute),
            "unmute" => return Some(GainAdjust::Unmute),
            _ => {}
        }

        // Strip exactly one dB suffix (case-insensitive) using strip_suffix so
        // inputs like "2dBdB" are rejected rather than silently stripped twice.
        let (digits, has_db) = if let Some(s) = input
            .strip_suffix("dB")
            .or_else(|| input.strip_suffix("DB"))
            .or_else(|| input.strip_suffix("db"))
        {
            (s.trim(), true)
        } else {
            (input, false)
        };

        if has_db {
            // Explicit dB suffix always means absolute Set (sign is part of the value).
            let val: f32 = digits.parse().ok()?;
            if val.is_nan() || val.is_infinite() {
                return None;
            }
            return Some(GainAdjust::Set(val));
        }

        // Without dB suffix: +X = increment, -X = decrement, plain X = absolute set.
        if let Some(v) = digits.strip_prefix('+') {
            let mag: f32 = v.trim().parse().ok()?;
            // Reject negative, NaN, or infinite magnitudes.
            if mag < 0.0 || mag.is_nan() || mag.is_infinite() {
                return None;
            }
            Some(GainAdjust::Increment(mag))
        } else if let Some(v) = digits.strip_prefix('-') {
            let mag: f32 = v.trim().parse().ok()?;
            // Reject negative, NaN, or infinite magnitudes.
            if mag < 0.0 || mag.is_nan() || mag.is_infinite() {
                return None;
            }
            Some(GainAdjust::Decrement(mag))
        } else {
            let val: f32 = digits.parse().ok()?;
            if val.is_nan() || val.is_infinite() {
                return None;
            }
            Some(GainAdjust::Set(val))
        }
    }

    fn parse_eq(input: &str) -> Option<EqPreset> {
        match input.trim() {
            "flat" => Some(EqPreset::Flat),
            "bass_boost" | "bass" => Some(EqPreset::BassBoost),
            "treble_boost" | "treble" => Some(EqPreset::TrebleBoost),
            "vocal" | "voice" => Some(EqPreset::Vocal),
            "loudness" => Some(EqPreset::Loudness),
            _ => None,
        }
    }

    fn parse_profile(input: &str) -> Option<DspProfileCmd> {
        match input.trim() {
            "music" => Some(DspProfileCmd::Music),
            "movies" | "movie" | "video" => Some(DspProfileCmd::Movies),
            "night" | "night_mode" => Some(DspProfileCmd::NightMode),
            "custom" => Some(DspProfileCmd::Custom),
            _ => None,
        }
    }

    pub fn help() -> &'static str {
        r#"DSP Commands:
  :gain <value>       Set gain in dB (e.g., :gain +2dB, :gain -3dB, :gain 0)
  :gain +<value>     Increment gain (e.g., :gain +1 increments by +1dB)
  :gain -<value>     Decrement gain (e.g., :gain -2 decrements by 2dB)
                       Note: Use :gain -2dB to set gain to -2dB (absolute)
  :gain mute         Mute output
  :gain unmute       Unmute output
  :eq <preset>       Set EQ preset (flat, bass_boost, treble, vocal, loudness)
  :dsp enable <name>    Enable DSP node (eq, gain, etc)
  :dsp disable <name>  Disable DSP node (eq, gain, etc)
  :dsp profile <type>  Set built-in profile (music, movies, night, custom)
  :dsp save <name>    Save current settings as custom profile
  :dsp load <name>    Load custom profile
  :dsp delete <name>  Delete custom profile
  :dsp list          List all profiles (built-in + custom)
  :dsp               Show current DSP chain description"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_commands() {
        // Note: ":gain +2dB" with "dB" suffix is parsed as Set (absolute value)
        // Use ":gain +2" for Increment
        assert!(matches!(
            DspCommand::parse(":gain +2dB"),
            Some(DspCommand::Gain(GainAdjust::Set(2.0)))
        ));
        assert!(matches!(
            DspCommand::parse(":gain +2"),
            Some(DspCommand::Gain(GainAdjust::Increment(2.0)))
        ));
        assert!(matches!(
            DspCommand::parse(":gain -3"),
            Some(DspCommand::Gain(GainAdjust::Decrement(3.0)))
        ));
        assert!(matches!(
            DspCommand::parse(":gain mute"),
            Some(DspCommand::Gain(GainAdjust::Mute))
        ));
    }

    #[test]
    fn test_profile() {
        assert!(matches!(
            DspCommand::parse(":dsp profile music"),
            Some(DspCommand::Profile(DspProfileCmd::Music))
        ));
    }

    #[test]
    fn test_describe() {
        assert!(matches!(
            DspCommand::parse(":dsp"),
            Some(DspCommand::Describe)
        ));
        assert!(matches!(
            DspCommand::parse(":dsp describe"),
            Some(DspCommand::Describe)
        ));
    }

    #[test]
    fn test_gain_absolute_set() {
        // Plain number → absolute Set
        assert!(matches!(
            DspCommand::parse(":gain 3"),
            Some(DspCommand::Gain(GainAdjust::Set(v))) if (v - 3.0).abs() < f32::EPSILON
        ));
        // Explicit negative dB suffix → absolute Set (negative value)
        assert!(matches!(
            DspCommand::parse(":gain -3dB"),
            Some(DspCommand::Gain(GainAdjust::Set(v))) if (v - (-3.0)).abs() < f32::EPSILON
        ));
        // Positive dB suffix → absolute Set
        assert!(matches!(
            DspCommand::parse(":gain 5dB"),
            Some(DspCommand::Gain(GainAdjust::Set(v))) if (v - 5.0).abs() < f32::EPSILON
        ));
        // ":dsp gain" prefix also works
        assert!(matches!(
            DspCommand::parse(":dsp gain 2"),
            Some(DspCommand::Gain(GainAdjust::Set(v))) if (v - 2.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn test_gain_invalid_inputs() {
        // Double-suffix should be rejected
        assert!(DspCommand::parse(":gain 2dBdB").is_none());
        // Negative magnitude after '+' should be rejected
        assert!(DspCommand::parse(":gain +-3").is_none());
        // Negative magnitude after '-' (double-negative) should be rejected
        assert!(DspCommand::parse(":gain --3").is_none());
        // Non-numeric
        assert!(DspCommand::parse(":gain foo").is_none());
        // Bare :gain with no argument
        assert!(DspCommand::parse(":gain").is_none());
    }

    #[test]
    fn test_gain_unmute() {
        assert!(matches!(
            DspCommand::parse(":gain unmute"),
            Some(DspCommand::Gain(GainAdjust::Unmute))
        ));
    }

    #[test]
    fn test_eq_presets() {
        assert!(matches!(
            DspCommand::parse(":eq bass_boost"),
            Some(DspCommand::Eq(EqPreset::BassBoost))
        ));
        assert!(matches!(
            DspCommand::parse(":eq bass"),
            Some(DspCommand::Eq(EqPreset::BassBoost))
        ));
        assert!(matches!(
            DspCommand::parse(":eq treble"),
            Some(DspCommand::Eq(EqPreset::TrebleBoost))
        ));
        assert!(matches!(
            DspCommand::parse(":eq vocal"),
            Some(DspCommand::Eq(EqPreset::Vocal))
        ));
        assert!(matches!(
            DspCommand::parse(":eq voice"),
            Some(DspCommand::Eq(EqPreset::Vocal))
        ));
        assert!(matches!(
            DspCommand::parse(":eq loudness"),
            Some(DspCommand::Eq(EqPreset::Loudness))
        ));
        assert!(matches!(
            DspCommand::parse(":eq flat"),
            Some(DspCommand::Eq(EqPreset::Flat))
        ));
        // ":dsp eq" prefix also works
        assert!(matches!(
            DspCommand::parse(":dsp eq flat"),
            Some(DspCommand::Eq(EqPreset::Flat))
        ));
        // Unknown preset
        assert!(DspCommand::parse(":eq unknown").is_none());
    }

    #[test]
    fn test_enable_disable() {
        assert!(matches!(
            DspCommand::parse(":dsp enable eq"),
            Some(DspCommand::Enable(ref s)) if s == "eq"
        ));
        assert!(matches!(
            DspCommand::parse(":dsp disable gain"),
            Some(DspCommand::Disable(ref s)) if s == "gain"
        ));
        // Empty name rejected
        assert!(DspCommand::parse(":dsp enable ").is_none());
        assert!(DspCommand::parse(":dsp disable ").is_none());
    }

    #[test]
    fn test_list_profiles() {
        assert!(matches!(
            DspCommand::parse(":dsp list"),
            Some(DspCommand::ListProfiles)
        ));
        assert!(matches!(
            DspCommand::parse(":dsp profiles"),
            Some(DspCommand::ListProfiles)
        ));
    }

    #[test]
    fn test_save_load_delete_profile() {
        assert!(matches!(
            DspCommand::parse(":dsp save my_profile"),
            Some(DspCommand::SaveProfile(ref s)) if s == "my_profile"
        ));
        assert!(matches!(
            DspCommand::parse(":dsp load my_profile"),
            Some(DspCommand::LoadProfile(ref s)) if s == "my_profile"
        ));
        assert!(matches!(
            DspCommand::parse(":dsp delete my_profile"),
            Some(DspCommand::DeleteProfile(ref s)) if s == "my_profile"
        ));
        // Empty name rejected
        assert!(DspCommand::parse(":dsp save ").is_none());
        assert!(DspCommand::parse(":dsp load ").is_none());
        assert!(DspCommand::parse(":dsp delete ").is_none());
    }

    #[test]
    fn test_profile_variants() {
        assert!(matches!(
            DspCommand::parse(":dsp profile movies"),
            Some(DspCommand::Profile(DspProfileCmd::Movies))
        ));
        assert!(matches!(
            DspCommand::parse(":dsp profile movie"),
            Some(DspCommand::Profile(DspProfileCmd::Movies))
        ));
        assert!(matches!(
            DspCommand::parse(":dsp profile video"),
            Some(DspCommand::Profile(DspProfileCmd::Movies))
        ));
        assert!(matches!(
            DspCommand::parse(":dsp profile night"),
            Some(DspCommand::Profile(DspProfileCmd::NightMode))
        ));
        assert!(matches!(
            DspCommand::parse(":dsp profile night_mode"),
            Some(DspCommand::Profile(DspProfileCmd::NightMode))
        ));
        assert!(matches!(
            DspCommand::parse(":dsp profile custom"),
            Some(DspCommand::Profile(DspProfileCmd::Custom))
        ));
        // Unknown profile
        assert!(DspCommand::parse(":dsp profile unknown").is_none());
    }

    #[test]
    fn test_unknown_command_returns_none() {
        assert!(DspCommand::parse(":unknown").is_none());
        assert!(DspCommand::parse("gain 3").is_none());
        assert!(DspCommand::parse("").is_none());
        assert!(DspCommand::parse(":dsp unknown_subcommand").is_none());
    }
}
