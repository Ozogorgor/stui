//! AudioOutput trait and backend factory.
//!
//! Call `open_output(target, config)` to get a `Box<dyn AudioOutput>`.
//! The pipeline holds this as `Option<Box<dyn AudioOutput>>` and calls
//! `write()` at the end of every `process()` call.
//!
//! For native DSD output, call `open_dsd_output(target, dsd_rate, role)`.
//! The `role` parameter specifies the PipeWire role (e.g., "Music", "Movie").

pub mod alsa;
pub mod dsd;
pub mod pipewire;
pub mod roon;

pub use alsa::AlsaOutput;
#[allow(unused_imports)]
pub use dsd::{dsd_available, DopEncoder, DsdOutputMode};
pub use pipewire::{PipeWireDsdOutput, PipeWireOutput};
pub use roon::RoonOutput;

use super::config::{DspConfig, OutputTarget};
use tracing::warn;

/// Errors returned by `AudioOutput` implementations.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("write error: {0}")]
    WriteError(String),
    #[error("config error: {0}")]
    ConfigError(String),
}

/// Trait implemented by all audio output backends.
#[allow(dead_code)]
pub trait AudioOutput: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Write interleaved stereo f32 samples to the output.
    fn write(&mut self, samples: &[f32]) -> Result<(), OutputError>;
    /// Drain and close the output device.
    fn close(self: Box<Self>);
}

/// Trait for native DSD output via DoP (DSD over PCM) protocol.
#[allow(dead_code)]
pub trait DsdAudioOutput: Send {
    /// DSD clock rate in Hz (2 822 400 for DSD64, 5 644 800 for DSD128, etc.)
    fn dsd_rate(&self) -> u32;

    /// Write interleaved 1-bit DSD samples to the output.
    ///
    /// Each sample is represented as an `f32`: `≥ 0.0` = DSD-1, `< 0.0` = DSD-0.
    /// Samples are interleaved left/right.  The length must be a multiple of 16.
    ///
    /// Implementations encode the raw DSD bits into DoP words (see [`DopEncoder`])
    /// and stream them as S32LE PCM at `dsd_rate / 16` Hz.
    fn write_dsd(&mut self, samples: &[f32]) -> Result<(), OutputError>;

    /// Drain and close the output device.
    fn close(self: Box<Self>);
}

/// Open a PCM audio output backend for the given target.
#[allow(clippy::type_complexity)]
pub fn open_output(
    target: OutputTarget,
    config: &DspConfig,
) -> Result<Box<dyn AudioOutput>, OutputError> {
    match target {
        OutputTarget::PipeWire => match PipeWireOutput::new(config) {
            Ok(out) => Ok(Box::new(out)),
            Err(e) if is_connection_error(&e) => {
                warn!(error = %e, "PipeWire unavailable, falling back to ALSA");
                let out = AlsaOutput::new(config)?;
                Ok(Box::new(out))
            }
            Err(e) => Err(e),
        },
        OutputTarget::Alsa => Ok(Box::new(AlsaOutput::new(config)?)),
        OutputTarget::RoonRaat => {
            RoonOutput::new(config.output_sample_rate).map(|o| Box::new(o) as Box<dyn AudioOutput>)
        }
        OutputTarget::Mpd => Err(OutputError::ConfigError(
            "MPD output is not implemented in the DSP output path".into(),
        )),
    }
}

/// Open a native DSD output backend for the given target.
#[allow(clippy::type_complexity)]
pub fn open_dsd_output(
    target: OutputTarget,
    dsd_rate: u32,
    role: String,
) -> Result<Box<dyn DsdAudioOutput>, OutputError> {
    match target {
        OutputTarget::PipeWire => match PipeWireDsdOutput::new(dsd_rate, role) {
            Ok(out) => Ok(Box::new(out)),
            Err(e) => Err(e),
        },
        OutputTarget::Alsa => Err(OutputError::ConfigError(
            "native DSD over ALSA not implemented".into(),
        )),
        OutputTarget::RoonRaat | OutputTarget::Mpd => Err(OutputError::ConfigError(format!(
            "output target {:?} does not support native DSD",
            target
        ))),
    }
}

/// Returns true for errors that should trigger the PipeWire→ALSA fallback.
fn is_connection_error(e: &OutputError) -> bool {
    matches!(e, OutputError::DeviceNotFound(_))
}
