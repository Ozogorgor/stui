//! DSD (Direct Stream Digital) output support.
//!
//! Native DSD output uses DoP (DSD over PCM) protocol — DSD data is
//! encapsulated in 32-bit S32LE PCM frames with a marker byte in the MSB.
//! This allows native DSD playback on DACs that support DoP without requiring
//! kernel-level DSD driver support.
//!
//! # DoP frame layout (32-bit, big-endian view)
//!
//! ```text
//! ┌──────────┬──────────┬──────────┬──────────┐
//! │  Marker  │  L bits  │  R bits  │  0x00    │
//! │ 0x05/0xFA│  [7:0]   │  [7:0]   │ (pad)    │
//! └──────────┴──────────┴──────────┴──────────┘
//! ```
//!
//! Stored as S32LE in memory: `[0x00, R_bits, L_bits, marker]`.
//! The marker alternates between 0x05 and 0xFA on every frame.

use crate::dsp::config::DsdMode;
use crate::dsp::output::OutputError;

/// DoP (DSD over PCM) encoder.
///
/// Converts raw 1-bit DSD samples (represented as ±1.0 `f32`) into 32-bit
/// DoP words ready to be written to an S32LE PipeWire stream.
pub struct DopEncoder {
    /// Counts emitted frames; even → 0x05 marker, odd → 0xFA marker.
    frame_count: u64,
}

impl DopEncoder {
    pub fn new() -> Self {
        Self { frame_count: 0 }
    }

    /// Reset the alternating-marker counter.
    ///
    /// Call this at stream start or after a seek so the very first emitted
    /// frame always carries the 0x05 marker.
    pub fn reset(&mut self) {
        self.frame_count = 0;
    }

    /// Encode raw DSD samples into 32-bit DoP words (S32LE).
    ///
    /// # Input
    /// `dsd_samples` must be interleaved left/right 1-bit DSD represented as
    /// `f32` values where `≥ 0.0` means DSD-1 and `< 0.0` means DSD-0.
    /// The length must be a multiple of 16 (8 stereo pairs per output word).
    ///
    /// # Output
    /// One `u32` per 16 input samples.  Byte layout (big-endian view):
    /// `[marker | L_byte | R_byte | 0x00]`.  On a little-endian host this
    /// is stored in memory as `[0x00 | R_byte | L_byte | marker]`, which is
    /// the correct S32LE DoP byte order that a DoP-capable DAC expects.
    pub fn encode(&mut self, dsd_samples: &[f32]) -> Result<Vec<u32>, OutputError> {
        if dsd_samples.len() % 16 != 0 {
            return Err(OutputError::ConfigError(format!(
                "DoP encoder: input length {} is not a multiple of 16",
                dsd_samples.len()
            )));
        }

        let mut words = Vec::with_capacity(dsd_samples.len() / 16);

        for chunk in dsd_samples.chunks_exact(16) {
            // chunk layout: [L0,R0, L1,R1, …, L7,R7]
            // Pack eight L bits and eight R bits into one byte each.
            let mut l_byte: u8 = 0;
            let mut r_byte: u8 = 0;
            for i in 0..8usize {
                if chunk[i * 2] >= 0.0 {
                    l_byte |= 0x80u8 >> i;
                }
                if chunk[i * 2 + 1] >= 0.0 {
                    r_byte |= 0x80u8 >> i;
                }
            }

            let marker = if self.frame_count & 1 == 0 {
                0x05u8
            } else {
                0xFAu8
            };
            self.frame_count += 1;

            // from_be_bytes produces a u32 whose big-endian representation is
            // [marker][L][R][0x00].  On LE hosts it is stored in memory as
            // [0x00][R][L][marker] — the correct S32LE DoP byte order.
            words.push(u32::from_be_bytes([marker, l_byte, r_byte, 0x00]));
        }

        Ok(words)
    }
}

impl Default for DopEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// DSD output mode configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsdOutputMode {
    /// No DSD output — use PCM.
    Off,
    /// Native DSD via DoP (DSD over PCM) protocol.
    Dop,
    /// Native DSD via raw DSD stream (requires kernel DSD support).
    Native,
}

impl From<DsdMode> for DsdOutputMode {
    fn from(mode: DsdMode) -> Self {
        match mode {
            DsdMode::Off => Self::Off,
            DsdMode::Dsd64 | DsdMode::Dsd128 | DsdMode::Dsd256 | DsdMode::Dsd512 => Self::Dop,
        }
    }
}

/// Returns `true` if a PipeWire socket is present in `$XDG_RUNTIME_DIR`.
///
/// This is a lightweight proxy for "DSD output might be available".
/// Whether the connected DAC actually supports DoP is determined by
/// `PipeWireDsdOutput::new` when the stream is opened.
pub fn dsd_available() -> bool {
    let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") else {
        return false;
    };
    std::path::Path::new(&runtime_dir)
        .join("pipewire-0")
        .exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_alternates_between_frames() {
        let mut enc = DopEncoder::new();
        let ones = vec![1.0f32; 32]; // 2 frames
        let words = enc.encode(&ones).unwrap();
        assert_eq!(words[0].to_be_bytes()[0], 0x05, "first frame marker");
        assert_eq!(words[1].to_be_bytes()[0], 0xFA, "second frame marker");
    }

    #[test]
    fn reset_restarts_marker_sequence() {
        let mut enc = DopEncoder::new();
        enc.encode(&vec![1.0f32; 16]).unwrap(); // frame 0 → 0x05
        enc.reset();
        let words = enc.encode(&vec![1.0f32; 16]).unwrap();
        assert_eq!(
            words[0].to_be_bytes()[0],
            0x05,
            "after reset, first marker is 0x05"
        );
    }

    #[test]
    fn bits_packed_correctly() {
        let mut enc = DopEncoder::new();
        // L alternates 1/0/1/0… → 0xAA; R alternates 0/1/0/1… → 0x55
        let samples: Vec<f32> = (0..8)
            .flat_map(|i| {
                let l = if i % 2 == 0 { 1.0f32 } else { -1.0f32 };
                let r = if i % 2 == 0 { -1.0f32 } else { 1.0f32 };
                [l, r]
            })
            .collect();
        let words = enc.encode(&samples).unwrap();
        let bytes = words[0].to_be_bytes();
        assert_eq!(bytes[0], 0x05, "marker");
        assert_eq!(bytes[1], 0xAA, "L bits");
        assert_eq!(bytes[2], 0x55, "R bits");
        assert_eq!(bytes[3], 0x00, "padding");
    }

    #[test]
    fn rejects_non_multiple_of_16() {
        let mut enc = DopEncoder::new();
        assert!(enc.encode(&[1.0f32; 15]).is_err());
        assert!(enc.encode(&[1.0f32; 1]).is_err());
    }

    #[test]
    fn accepts_empty_input() {
        let mut enc = DopEncoder::new();
        assert!(enc.encode(&[]).unwrap().is_empty());
    }

    #[test]
    fn all_dsd_ones_produce_0xff_bytes() {
        let mut enc = DopEncoder::new();
        let words = enc.encode(&vec![1.0f32; 16]).unwrap();
        let bytes = words[0].to_be_bytes();
        assert_eq!(bytes[1], 0xFF, "all-ones L");
        assert_eq!(bytes[2], 0xFF, "all-ones R");
    }

    #[test]
    fn all_dsd_zeros_produce_0x00_bytes() {
        let mut enc = DopEncoder::new();
        let words = enc.encode(&vec![-1.0f32; 16]).unwrap();
        let bytes = words[0].to_be_bytes();
        assert_eq!(bytes[1], 0x00, "all-zeros L");
        assert_eq!(bytes[2], 0x00, "all-zeros R");
    }
}
