//! AudioOutput trait and backend factory.
//!
//! Call `open_output(target, config)` to get a `Box<dyn AudioOutput>`.
//! The pipeline holds this as `Option<Box<dyn AudioOutput>>` and calls
//! `write()` at the end of every `process()` call.

pub mod alsa;
pub mod pipewire;

pub use alsa::AlsaOutput;
pub use pipewire::PipeWireOutput;

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
pub trait AudioOutput: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Write interleaved stereo f32 samples to the output.
    fn write(&mut self, samples: &[f32]) -> Result<(), OutputError>;
    /// Drain and close the output device.
    fn close(self: Box<Self>);
}

/// Open an audio output backend for the given target.
///
/// PipeWire falls back to ALSA on socket/connection errors.
/// Permission denied or format errors are returned as `ConfigError`.
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
        OutputTarget::RoonRaat | OutputTarget::Mpd => Err(OutputError::ConfigError(format!(
            "output target {:?} is not implemented in the DSP output path",
            target
        ))),
    }
}

/// Returns true for errors that should trigger the PipeWire→ALSA fallback.
/// Does NOT include permission denied or format negotiation failures (those
/// are `ConfigError`). PipeWireOutput::new() must map connection errors to
/// `DeviceNotFound` and permission/format errors to `ConfigError` so this
/// helper can distinguish them.
fn is_connection_error(e: &OutputError) -> bool {
    matches!(e, OutputError::DeviceNotFound(_))
}
