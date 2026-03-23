use super::{AudioOutput, OutputError};
use crate::dsp::config::DspConfig;

pub struct AlsaOutput;

impl AlsaOutput {
    pub fn new(_config: &DspConfig) -> Result<Self, OutputError> {
        Err(OutputError::DeviceNotFound("stub".into()))
    }
}

impl AudioOutput for AlsaOutput {
    fn sample_rate(&self) -> u32 { 44100 }
    fn channels(&self) -> u16 { 2 }
    fn write(&mut self, _: &[f32]) -> Result<(), OutputError> { Ok(()) }
    fn close(self: Box<Self>) {}
}
