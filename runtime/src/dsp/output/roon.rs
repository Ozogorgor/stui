//! Roon RAAT audio output backend.
//!
//! Audio bytes are accepted and dropped — actual transport to a Roon endpoint
//! requires the RAAT TCP framing layer which is not yet implemented.
//! This stub allows the DSP pipeline to target Roon without crashing.

use super::{AudioOutput, OutputError};
use tracing::debug;

/// No-op `AudioOutput` for the Roon RAAT target.
pub struct RoonOutput {
    sample_rate: u32,
}

impl RoonOutput {
    pub fn new(sample_rate: u32) -> Result<Self, OutputError> {
        // RAAT implementation is a placeholder - audio will be dropped
        Ok(Self { sample_rate })
    }
}

impl AudioOutput for RoonOutput {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u16 {
        2
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), OutputError> {
        // RAAT framing not yet implemented — drop samples silently
        debug!(
            samples = samples.len(),
            "RoonOutput: dropping audio (RAAT not yet implemented)"
        );
        Ok(())
    }

    fn close(self: Box<Self>) {}
}
