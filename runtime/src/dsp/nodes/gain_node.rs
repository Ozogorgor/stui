use super::DspNode;
use crate::dsp::config::DspConfig;

pub struct GainNode {
    gain_db: f32,
    enabled: bool,
    muted: bool,
}

#[allow(dead_code)] // planned: DSP gain/mute control, called from DspPipeline handle_command
impl GainNode {
    pub fn new() -> Self {
        Self {
            gain_db: 0.0,
            enabled: true,
            muted: false,
        }
    }

    pub fn set_gain(&mut self, gain_db: f32) {
        self.gain_db = gain_db.clamp(-20.0, 20.0);
    }

    pub fn adjust_gain(&mut self, delta_db: f32) {
        self.gain_db = (self.gain_db + delta_db).clamp(-20.0, 20.0);
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn mute(&mut self) {
        self.muted = true;
    }

    pub fn unmute(&mut self) {
        self.muted = false;
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }
}

impl Default for GainNode {
    fn default() -> Self {
        Self::new()
    }
}

impl DspNode for GainNode {
    fn name(&self) -> &str {
        "gain"
    }

    fn process(&mut self, samples: &mut [f32], _sample_rate: u32) -> Vec<f32> {
        if !self.enabled || self.muted || self.gain_db.abs() < 0.001 {
            return samples.to_vec();
        }

        let gain_linear = 10.0_f32.powf(self.gain_db / 20.0);

        samples.iter().map(|&s| s * gain_linear).collect()
    }

    fn is_enabled(&self) -> bool {
        self.enabled && !self.muted
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn update_config(&mut self, _config: &DspConfig) {
        // Gain is controlled exclusively through set_gain() / adjust_gain() so
        // that user volume adjustments (increment/decrement commands) are not
        // overwritten each time a config update propagates through the pipeline.
        // config.gain_db is intentionally not applied here.
    }

    fn flush(&mut self) {}
}
