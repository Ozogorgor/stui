//! Convolution engine for room correction filters.

use std::fs::File;
use std::io::{Read, Seek};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::config::DspConfig;

/// Convolution engine for applying room correction filters.
pub struct ConvolutionEngine {
    config: Arc<RwLock<DspConfig>>,
    filter: Option<Vec<f32>>,
    filter_length: usize,
    enabled: bool,
    bypass: bool,
}

impl ConvolutionEngine {
    /// Create a new convolution engine.
    pub fn new(config: Arc<RwLock<DspConfig>>) -> Result<Self, String> {
        let cfg = config.blocking_read();
        let filter = if let Some(ref path) = cfg.convolution_filter_path {
            Some(load_filter_file(path)?)
        } else {
            None
        };

        let filter_length = filter.as_ref().map(|f| f.len()).unwrap_or(0);
        let enabled = cfg.convolution_enabled;
        let bypass = cfg.convolution_bypass;

        if filter.is_some() {
            info!(
                length = filter_length,
                "convolution engine initialized with filter"
            );
        }

        Ok(Self {
            config: Arc::clone(&config),
            filter,
            filter_length,
            enabled,
            bypass,
        })
    }

    /// Load a convolution filter from a file.
    pub fn load_filter(&mut self, path: &str) -> Result<(), String> {
        self.filter = Some(load_filter_file(path)?);
        self.filter_length = self.filter.as_ref().map(|f| f.len()).unwrap_or(0);
        info!(
            path = path,
            length = self.filter_length,
            "convolution filter loaded"
        );
        Ok(())
    }

    /// Process audio through convolution.
    pub fn process(&self, samples: &[f32]) -> Vec<f32> {
        if !self.enabled || self.bypass || self.filter.is_none() {
            return samples.to_vec();
        }

        let filter = self.filter.as_ref().unwrap();
        let input_len = samples.len();
        let output_len = input_len + filter.len() - 1;
        let mut output = vec![0.0f32; output_len];

        // Simple convolution (O(n*m) - could be optimized with FFT)
        for i in 0..input_len {
            for j in 0..filter.len() {
                if i + j < output_len {
                    output[i + j] += samples[i] * filter[j];
                }
            }
        }

        // Trim to input size (delay compensation would adjust this)
        output.truncate(input_len);

        debug!(
            input_len = input_len,
            output_len = output.len(),
            "convolved"
        );
        output
    }

    /// Check if convolution is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled && !self.bypass && self.filter.is_some()
    }

    /// Set bypass state.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
        debug!(bypass = bypass, "convolution bypass changed");
    }

    /// Enable/disable convolution.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        debug!(enabled = enabled, "convolution enabled changed");
    }
}

/// Maximum convolution filter file size (64 MB).
const MAX_FILTER_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Load filter from WAV file.
fn load_filter_file(path: &str) -> Result<Vec<f32>, String> {
    let mut file = File::open(path).map_err(|e| format!("Failed to open filter file: {}", e))?;

    let metadata = file
        .metadata()
        .map_err(|e| format!("Failed to stat filter file: {}", e))?;
    if metadata.len() > MAX_FILTER_FILE_BYTES {
        return Err(format!(
            "Filter file exceeds maximum size of {} MB",
            MAX_FILTER_FILE_BYTES / (1024 * 1024)
        ));
    }

    // Simple WAV reader for floating-point WAV files
    let mut header = [0u8; 44];
    file.read(&mut header)
        .map_err(|e| format!("Failed to read header: {}", e))?;

    // Check RIFF header
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("Not a valid WAV file".to_string());
    }

    // Find data chunk
    let mut data_start = 12;
    let mut data_size = 0usize;

    loop {
        let mut chunk_header = [0u8; 8];
        if file
            .read(&mut chunk_header)
            .map_err(|e| format!("Failed to read chunk: {}", e))?
            == 0
        {
            break;
        }

        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]) as usize;

        if chunk_id == b"data" {
            data_start = file.stream_position().map_err(|e| e.to_string())? as usize;
            data_size = chunk_size;
            break;
        }

        // Skip this chunk
        use std::io::Seek;
        file.seek_relative(chunk_size as i64)
            .map_err(|e| e.to_string())?;
    }

    if data_size == 0 {
        return Err("No audio data found in WAV file".to_string());
    }

    // Read audio samples
    let sample_count = data_size / 4; // 32-bit float
    let mut bytes = vec![0u8; sample_count * 4];

    file.read_exact(&mut bytes)
        .map_err(|e| format!("Failed to read data: {}", e))?;

    // Convert bytes to f32 samples
    let data: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();

    info!(
        path = path,
        samples = data.len(),
        "loaded convolution filter"
    );
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> Arc<RwLock<DspConfig>> {
        Arc::new(RwLock::new(DspConfig {
            convolution_enabled: true,
            ..Default::default()
        }))
    }

    #[test]
    fn test_engine_creation() {
        let config = make_config();
        let engine = ConvolutionEngine::new(config);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_process_passthrough() {
        let config = make_config();
        let engine = ConvolutionEngine::new(config).unwrap();

        let input = vec![0.1, 0.2, 0.3, 0.4];
        let output = engine.process(&input);

        // No filter loaded, should pass through
        assert_eq!(output, input);
    }

    #[test]
    fn test_bypass() {
        let config = make_config();
        let mut engine = ConvolutionEngine::new(config).unwrap();

        engine.set_bypass(true);
        assert!(engine.bypass);

        let input = vec![0.1, 0.2, 0.3, 0.4];
        let output = engine.process(&input);

        // Bypassed, should pass through
        assert_eq!(output, input);
    }
}
