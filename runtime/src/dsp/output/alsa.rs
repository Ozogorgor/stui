//! ALSA direct hardware output.
//!
//! Opens hw:{alsa_device} directly — no OS mixer in the signal path.
//! Uses S32LE format (falls back to F32LE if the device does not support S32LE).

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::Direction;
use tracing::{debug, info, warn};

use super::{AudioOutput, OutputError};
use crate::dsp::config::DspConfig;

pub struct AlsaOutput {
    pcm: PCM,
    sample_rate: u32,
    format: AlsaFormat,
}

#[derive(Clone, Copy)]
enum AlsaFormat {
    S32,
    F32,
}

impl AlsaOutput {
    pub fn new(config: &DspConfig) -> Result<Self, OutputError> {
        let device = config.alsa_device.as_deref().unwrap_or("hw:0,0");

        let pcm = PCM::new(device, Direction::Playback, false)
            .map_err(|e| OutputError::DeviceNotFound(format!("{device}: {e}")))?;

        let hwp = HwParams::any(&pcm).map_err(|e| OutputError::ConfigError(e.to_string()))?;

        hwp.set_channels(2)
            .map_err(|e| OutputError::ConfigError(format!("channels: {e}")))?;
        hwp.set_rate(config.output_sample_rate, alsa::ValueOr::Nearest)
            .map_err(|e| OutputError::ConfigError(format!("rate: {e}")))?;
        hwp.set_access(Access::RWInterleaved)
            .map_err(|e| OutputError::ConfigError(format!("access: {e}")))?;

        // Try S32LE first; fall back to F32LE
        let format = if hwp.set_format(Format::s32()).is_ok() {
            AlsaFormat::S32
        } else {
            hwp.set_format(Format::float()).map_err(|e| {
                OutputError::ConfigError(format!("neither S32LE nor F32LE supported: {e}"))
            })?;
            AlsaFormat::F32
        };

        hwp.set_period_size(
            config.buffer_size as alsa::pcm::Frames,
            alsa::ValueOr::Nearest,
        )
        .ok(); // advisory only — not fatal if unsupported

        pcm.hw_params(&hwp)
            .map_err(|e| OutputError::ConfigError(format!("hw_params: {e}")))?;

        // Drop hwp to release the borrow on pcm before moving pcm into Self
        drop(hwp);

        info!(
            device,
            rate = config.output_sample_rate,
            "ALSA output opened"
        );

        Ok(Self {
            pcm,
            sample_rate: config.output_sample_rate,
            format,
        })
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<(), OutputError> {
        // IMPORTANT: `pcm.io_i32()` and `pcm.io_f32()` return IO objects that borrow
        // from `self.pcm`. We must ensure the IO borrow ends (by completing the writei
        // call and letting the temporary drop) before calling `self.pcm.prepare()` in
        // the underrun recovery path. Using a sub-block ensures the IO temporary is
        // dropped before the match arm that calls self.pcm.prepare() runs.

        match self.format {
            AlsaFormat::S32 => {
                let frames: Vec<i32> = samples
                    .iter()
                    .map(|&s| (s.clamp(-1.0, 1.0) * i32::MAX as f32) as i32)
                    .collect();
                // Sub-block: IO borrow ends here, before any further borrow of self.pcm
                let result = {
                    let io = self
                        .pcm
                        .io_i32()
                        .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                    io.writei(&frames)
                };
                match result {
                    Ok(_) => Ok(()),
                    Err(e) if e.errno() == nix::errno::Errno::EPIPE as i32 => {
                        warn!("ALSA underrun (EPIPE) — recovering");
                        self.pcm
                            .prepare()
                            .map_err(|e2| OutputError::WriteError(format!("prepare: {e2}")))?;
                        let io = self
                            .pcm
                            .io_i32()
                            .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                        io.writei(&frames)
                            .map(|_| ())
                            .map_err(|e| OutputError::WriteError(format!("retry: {e}")))
                    }
                    Err(e) => Err(OutputError::WriteError(e.to_string())),
                }
            }
            AlsaFormat::F32 => {
                let result = {
                    let io = self
                        .pcm
                        .io_f32()
                        .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                    io.writei(samples)
                };
                match result {
                    Ok(_) => Ok(()),
                    Err(e) if e.errno() == nix::errno::Errno::EPIPE as i32 => {
                        warn!("ALSA underrun (EPIPE) — recovering");
                        self.pcm
                            .prepare()
                            .map_err(|e2| OutputError::WriteError(format!("prepare: {e2}")))?;
                        let io = self
                            .pcm
                            .io_f32()
                            .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                        io.writei(samples)
                            .map(|_| ())
                            .map_err(|e| OutputError::WriteError(format!("retry: {e}")))
                    }
                    Err(e) => Err(OutputError::WriteError(e.to_string())),
                }
            }
        }
    }
}

impl AudioOutput for AlsaOutput {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        2
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), OutputError> {
        debug!(frames = samples.len() / 2, "ALSA write");
        self.write_samples(samples)
    }

    fn close(self: Box<Self>) {
        if let Err(e) = self.pcm.drain() {
            warn!(error = %e, "ALSA drain on close failed");
        }
        // PCM is dropped here, which closes the device
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::config::DspConfig;

    #[test]
    fn null_sink_open_write_close() {
        // plug:null is ALSA's built-in null sink — no hardware required.
        let config = DspConfig {
            output_sample_rate: 48000,
            buffer_size: 1024,
            alsa_device: Some("plug:null".to_string()),
            ..Default::default()
        };
        let mut output = AlsaOutput::new(&config).expect("plug:null should always open");
        let silence = vec![0.0f32; 2048]; // 1024 frames × 2 channels
        output.write(&silence).expect("write to null sink");
        Box::new(output).close();
    }
}
