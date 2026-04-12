//! DSD to PCM conversion for DACs without native DSD support.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::config::DspConfig;

/// DSD to PCM converter.
#[allow(dead_code)] // Used by DspPipeline internally
pub struct DsdConverter {
    config: Arc<RwLock<DspConfig>>,
    output_rate: u32,
}

impl DsdConverter {
    /// Create a new DSD to PCM converter.
    pub fn new(config: Arc<RwLock<DspConfig>>) -> Result<Self, String> {
        let cfg = config.blocking_read();
        let output_rate = cfg.dsd_output_rate;

        info!(output_rate = output_rate, "DSD converter initialized");

        Ok(Self {
            config: Arc::clone(&config),
            output_rate,
        })
    }

    /// Convert DSD audio to high-rate PCM.
    pub fn convert(&self, dsd_samples: &[f32]) -> Vec<f32> {
        // DSD is 1-bit audio stored as pulse density
        // Conversion to PCM uses sigma-delta demodulation

        // For now, use simple decimation-based approach
        // Production implementation would use proper SDM demodulation

        let cfg = self.config.blocking_read();
        let output_rate = cfg.dsd_output_rate;
        let input_rate = self.infer_dsd_rate(dsd_samples.len());

        let ratio = output_rate as f64 / input_rate as f64;
        let output_len = (dsd_samples.len() as f64 * ratio).ceil() as usize;
        let mut output = Vec::with_capacity(output_len);

        // Simple decimation with low-pass filtering
        let decimation = (ratio as usize).max(1);

        for chunk in dsd_samples.chunks(decimation) {
            // Average the DSD bits to get PCM value
            let sum: f32 = chunk
                .iter()
                .map(|&s| if s > 0.0 { 1.0 } else { -1.0 })
                .sum();
            let avg = sum / chunk.len() as f32;

            // Scale to 16-bit range
            let pcm_sample = avg * 32767.0;
            output.push(pcm_sample);

            // Output for stereo (duplicate for both channels)
            output.push(pcm_sample);
        }

        debug!(
            input_len = dsd_samples.len(),
            output_len = output.len(),
            "DSD converted to PCM"
        );

        output
    }

    fn infer_dsd_rate(&self, _sample_count: usize) -> u32 {
        // Try to infer DSD rate from sample count
        // DSD64 = 2.8MHz, DSD128 = 5.6MHz, DSD256 = 11.2MHz
        // This is a simplified heuristic

        // Assume standard DSD64 for now
        // TODO: Detect from file metadata or audio properties
        2822400
    }

    /// Get the output sample rate.
    #[allow(dead_code)] // pub API: used by DSP pipeline DSD output
    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }

}

/// DSD format variants.
///
/// TODO: detect DSD format from audio file metadata (SACD ISO, DSF, DFF headers)
/// and wire into `DsdConverter` so the correct input rate is used rather than the
/// hardcoded DSD64 assumption in `infer_dsd_rate`. See SCAFFOLD_TODOS.md.
#[allow(dead_code)] // pub API: used by DSP pipeline DSD output
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DsdFormat {
    Dsd64,
    Dsd128,
    Dsd256,
    Dsd512,
}

impl DsdFormat {
    #[allow(dead_code)] // pub API: used by DSP pipeline DSD output
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Dsd64 => 2822400,
            Self::Dsd128 => 5644800,
            Self::Dsd256 => 11289600,
            Self::Dsd512 => 22579200,
        }
    }

    #[allow(dead_code)] // pub API: used by DSP pipeline DSD output
    pub fn pcm_output(&self) -> u32 {
        match self {
            Self::Dsd64 => 176400,
            Self::Dsd128 => 352800,
            Self::Dsd256 => 705600,
            Self::Dsd512 => 1411200,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> Arc<RwLock<DspConfig>> {
        Arc::new(RwLock::new(DspConfig {
            dsd_to_pcm_enabled: true,
            dsd_output_rate: 352800,
            ..Default::default()
        }))
    }

    #[test]
    fn test_converter_creation() {
        let config = make_config();
        let converter = DsdConverter::new(config);
        assert!(converter.is_ok());
    }

    #[test]
    fn test_conversion() {
        let config = make_config();
        let converter = DsdConverter::new(config).unwrap();

        // Create simple DSD test signal
        let dsd: Vec<f32> = (0..4096)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();

        let pcm = converter.convert(&dsd);

        assert!(!pcm.is_empty());
        assert!(pcm.len() > dsd.len());
    }
}
