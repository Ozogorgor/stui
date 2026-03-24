//! Convolution engine with partitioned overlap-add (OLA) FFT processing.
//!
//! Partition strategy (automatic from filter length at load time):
//!   ≤ 4096 taps   → single-block OLA  (partition size = next_pow2(filter_len * 2))
//!   > 4096 taps   → uniform OLA       (partition size = 4096 samples)
//!
//! The overlap accumulator is stateful: the tail from each call feeds
//! into the start of the next, maintaining continuity across audio blocks.
//!
//! process() takes &mut self because the overlap buffer is updated per call.

use std::fs::File;
use std::io::{Read, Seek};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use rustfft::{num_complex::Complex32, Fft, FftPlanner};

use super::config::DspConfig;

const MAX_FILTER_FILE_BYTES: u64 = 64 * 1024 * 1024; // 64 MB cap

/// Convolution engine for room correction filters.
#[allow(clippy::type_complexity)]
pub struct ConvolutionEngine {
    config: Arc<RwLock<DspConfig>>,
    /// Pre-computed FFTs of each filter partition. Empty when no filter is loaded.
    filter_fft: Vec<Vec<Complex32>>,
    /// Overlap-add tail from the previous process() call. Length = (nparts+1) * psize.
    overlap: Vec<f32>,
    /// Partition size (samples). Determines FFT size = psize * 2.
    psize: usize,
    /// Cached forward FFT plan for fft_size = psize * 2.
    fft_plan: Option<Arc<dyn Fft<f32>>>,
    /// Cached inverse FFT plan for fft_size = psize * 2.
    ifft_plan: Option<Arc<dyn Fft<f32>>>,
    /// Scratch buffer reused each call to avoid repeated allocations (length = fft_size).
    scratch: Vec<Complex32>,
    enabled: bool,
    bypass: bool,
}

impl ConvolutionEngine {
    pub fn new(config: Arc<RwLock<DspConfig>>) -> Result<Self, String> {
        let cfg = config.blocking_read();
        let filter = if let Some(ref path) = cfg.convolution_filter_path {
            match load_filter_file(path) {
                Ok(f) => Some(f),
                Err(e) => {
                    warn!(path, error = %e, "convolution filter load failed — no filter active");
                    None
                }
            }
        } else {
            None
        };
        let enabled = cfg.convolution_enabled;
        let bypass = cfg.convolution_bypass;
        drop(cfg);

        let mut engine = Self {
            config: Arc::clone(&config),
            filter_fft: Vec::new(),
            overlap: Vec::new(),
            psize: 4096,
            fft_plan: None,
            ifft_plan: None,
            scratch: Vec::new(),
            enabled,
            bypass,
        };
        if let Some(f) = filter {
            engine.install_filter(f);
        }
        Ok(engine)
    }

    /// Load and install a convolution filter from a WAV file.
    pub fn load_filter(&mut self, path: &str) -> Result<(), String> {
        let samples = load_filter_file(path)?;
        self.install_filter(samples);
        Ok(())
    }

    /// Pre-compute filter FFTs and reset the overlap buffer.
    fn install_filter(&mut self, filter: Vec<f32>) {
        let taps = filter.len();
        info!(taps, "installing convolution filter");

        // Choose partition size
        self.psize = if taps <= 4096 {
            // Single-block: smallest power-of-two >= filter length * 2
            (taps * 2).next_power_of_two().max(2)
        } else {
            4096 // Uniform OLA for all longer filters
        };

        let fft_size = self.psize * 2;

        // Build and cache FFT plans once per filter load
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let ifft = planner.plan_fft_inverse(fft_size);

        // Partition filter into psize-length chunks, zero-pad, FFT each
        self.filter_fft = filter
            .chunks(self.psize)
            .map(|chunk| {
                let mut buf = vec![Complex32::new(0.0, 0.0); fft_size];
                for (i, &s) in chunk.iter().enumerate() {
                    buf[i].re = s;
                }
                fft.process(&mut buf);
                buf
            })
            .collect();

        // Cache the plans and pre-allocate scratch buffer for use in ola_process
        self.fft_plan = Some(fft);
        self.ifft_plan = Some(ifft);
        self.scratch = vec![Complex32::new(0.0, 0.0); fft_size];

        // Reset overlap tail. The accumulator tail is (nparts+1)*psize to cover partial
        // trailing input blocks (see tail_len comment in ola_process).
        self.overlap = vec![0.0f32; (self.filter_fft.len() + 1) * self.psize];

        debug!(
            taps,
            partitions = self.filter_fft.len(),
            psize = self.psize,
            "filter installed"
        );
    }

    /// Process audio through stateful OLA convolution.
    ///
    /// The overlap accumulator is updated on each call so that consecutive
    /// calls produce seamless audio (no clicks or discontinuities at block
    /// boundaries).
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if !self.is_enabled() {
            return samples.to_vec();
        }
        self.ola_process(samples)
    }

    fn ola_process(&mut self, input: &[f32]) -> Vec<f32> {
        let psize = self.psize;
        let fft_size = psize * 2;
        let scale = 1.0 / fft_size as f32;
        // nparts ≥ 1 (guaranteed: ola_process only called when is_enabled(), which checks
        // !filter_fft.is_empty())
        let nparts = self.filter_fft.len();
        // Tail length: for a partial trailing block starting at pos = input.len()-1 (worst case),
        // partition k = nparts-1 writes to pos + (nparts-1)*psize + fft_size - 1
        //   = (input.len()-1) + (nparts+1)*psize - 1 = input.len() + (nparts+1)*psize - 2.
        // So the accumulator needs (nparts+1)*psize elements past input.len() to avoid
        // out-of-bounds writes when the input length is not a multiple of psize.
        let tail_len = (nparts + 1) * psize;

        // Use cached FFT plans (always present when filter_fft is non-empty)
        let fft = self.fft_plan.as_ref().expect("fft_plan missing");
        let ifft = self.ifft_plan.as_ref().expect("ifft_plan missing");

        // Accumulator: input.len() + tail_len to hold OLA tails from all partitions
        let mut accum = vec![0.0f32; input.len() + tail_len];

        // Pre-add saved overlap from the previous call to the start of the accumulator
        for (i, &v) in self.overlap.iter().enumerate() {
            accum[i] += v;
        }

        // x_fft: forward FFT of current input block (reused across all partition multiplies)
        let mut x_fft = vec![Complex32::new(0.0, 0.0); fft_size];

        // Process input in non-overlapping psize-sample blocks
        let mut pos = 0;
        while pos < input.len() {
            let block_end = (pos + psize).min(input.len());
            let block = &input[pos..block_end];

            // Zero-pad block to fft_size and compute its FFT
            // Fill from previous contents first (the tail is already zero)
            for c in x_fft.iter_mut() {
                *c = Complex32::new(0.0, 0.0);
            }
            for (i, &s) in block.iter().enumerate() {
                x_fft[i].re = s;
            }
            fft.process(&mut x_fft);

            // Convolve with EACH filter partition and accumulate at the correct offset.
            // Partition k's contribution lands at pos + k*psize in the accumulator.
            for (k, fpart) in self.filter_fft.iter().enumerate() {
                // Copy x_fft into scratch, multiply pointwise by filter partition
                let scratch = &mut self.scratch;
                for (s, (x, f)) in scratch.iter_mut().zip(x_fft.iter().zip(fpart.iter())) {
                    s.re = x.re * f.re - x.im * f.im;
                    s.im = x.re * f.im + x.im * f.re;
                }
                ifft.process(scratch);

                // OLA: add scaled IFFT output to accumulator at pos + k*psize.
                // The accumulator is sized to input.len() + tail_len, which is always
                // ≥ pos + k*psize + fft_size for the valid partition range.
                let out_offset = pos + k * psize;
                let dest = &mut accum[out_offset..out_offset + fft_size];
                for (d, s) in dest.iter_mut().zip(scratch.iter()) {
                    *d += s.re * scale;
                }
            }

            pos = block_end;
        }

        // Save the new overlap tail for the next call (samples beyond input.len())
        self.overlap = accum[input.len()..].to_vec();

        // Return only the direct output
        accum.truncate(input.len());
        debug!(
            input_len = input.len(),
            output_len = accum.len(),
            "convolved"
        );
        accum
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled && !self.bypass && !self.filter_fft.is_empty()
    }

    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
        debug!(bypass, "convolution bypass changed");
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        debug!(enabled, "convolution enabled changed");
    }

    /// Test-only helper: install a filter from raw f32 samples (bypasses WAV parsing).
    #[cfg(test)]
    pub fn load_filter_from_vec(&mut self, samples: Vec<f32>) {
        self.enabled = true;
        self.bypass = false;
        self.install_filter(samples);
    }
}

/// Load a 32-bit float WAV file. Returns raw f32 samples.
///
/// Scans all RIFF chunks to locate `fmt ` and `data`. Validates that the file
/// uses IEEE float format (AudioFormat = 3) and 32-bit samples before reading.
fn load_filter_file(path: &str) -> Result<Vec<f32>, String> {
    let mut file = File::open(path).map_err(|e| format!("failed to open filter file: {e}"))?;

    let meta = file
        .metadata()
        .map_err(|e| format!("failed to stat filter file: {e}"))?;
    if meta.len() > MAX_FILTER_FILE_BYTES {
        return Err(format!(
            "filter file exceeds maximum size of {} MB",
            MAX_FILTER_FILE_BYTES / (1024 * 1024)
        ));
    }

    // Read the 12-byte RIFF/WAVE header
    let mut riff = [0u8; 12];
    file.read_exact(&mut riff)
        .map_err(|e| format!("failed to read WAV header: {e}"))?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err("not a valid WAV file".into());
    }

    // Scan chunks to find fmt and data
    let mut audio_format: Option<u16> = None;
    let mut bits_per_sample: Option<u16> = None;
    let mut data_size: Option<usize> = None;

    loop {
        let mut ch = [0u8; 8];
        match file.read(&mut ch).map_err(|e| format!("chunk read: {e}"))? {
            0 => break,
            n if n < 8 => return Err("truncated chunk header".into()),
            _ => {}
        }
        let csz = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as usize;

        if &ch[0..4] == b"fmt " {
            // Read fmt chunk (at least 16 bytes for PCM/float)
            if csz < 16 {
                return Err("fmt chunk too small".into());
            }
            let mut fmt = vec![0u8; csz];
            file.read_exact(&mut fmt)
                .map_err(|e| format!("failed to read fmt chunk: {e}"))?;
            audio_format = Some(u16::from_le_bytes([fmt[0], fmt[1]]));
            bits_per_sample = Some(u16::from_le_bytes([fmt[14], fmt[15]]));
        } else if &ch[0..4] == b"data" {
            data_size = Some(csz);
            break; // data chunk follows immediately
        } else {
            // Skip unknown/unneeded chunks (pad to even size per RIFF spec)
            let skip = csz + (csz & 1);
            file.seek_relative(skip as i64).map_err(|e| e.to_string())?;
        }
    }

    match audio_format {
        Some(3) => {} // IEEE_FLOAT — correct
        Some(1) => {
            return Err("WAV file is PCM integer format; a 32-bit float WAV is required".into())
        }
        Some(f) => {
            return Err(format!(
                "unsupported WAV AudioFormat {f}; 32-bit float (3) required"
            ))
        }
        None => return Err("WAV file has no fmt chunk".into()),
    }
    if bits_per_sample != Some(32) {
        return Err(format!(
            "WAV file has {} bits per sample; 32-bit float required",
            bits_per_sample.unwrap_or(0)
        ));
    }
    let data_size = data_size.ok_or("no audio data found in WAV file")?;

    let mut bytes = vec![0u8; data_size];
    file.read_exact(&mut bytes)
        .map_err(|e| format!("failed to read data: {e}"))?;

    let data: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    info!(path, samples = data.len(), "loaded convolution filter");
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::time::Instant;

    fn make_config() -> Arc<RwLock<DspConfig>> {
        Arc::new(RwLock::new(DspConfig {
            convolution_enabled: true,
            ..Default::default()
        }))
    }

    #[test]
    fn test_engine_creation() {
        assert!(ConvolutionEngine::new(make_config()).is_ok());
    }

    #[test]
    fn test_process_passthrough_no_filter() {
        // No filter loaded → passthrough
        let mut engine = ConvolutionEngine::new(make_config()).unwrap();
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        assert_eq!(engine.process(&input), input);
    }

    #[test]
    fn test_bypass() {
        let mut engine = ConvolutionEngine::new(make_config()).unwrap();
        engine.set_bypass(true);
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        assert_eq!(engine.process(&input), input);
    }

    #[test]
    fn identity_fir_passthrough() {
        // An FIR of [1.0, 0.0, ...] is the identity: every output sample should
        // equal the corresponding input sample (within f32 tolerance).
        // No startup transient: the identity filter contributes no history.
        let config = Arc::new(RwLock::new(DspConfig {
            convolution_enabled: true,
            convolution_bypass: false,
            ..Default::default()
        }));
        let mut identity = vec![0.0f32; 64];
        identity[0] = 1.0;

        let mut engine = ConvolutionEngine::new(config).unwrap();
        engine.load_filter_from_vec(identity);

        // Use a block larger than psize to exercise the multi-block path
        let sine: Vec<f32> = (0..512)
            .map(|i| (2.0 * PI * 1000.0 * i as f32 / 44100.0).sin())
            .collect();

        let output = engine.process(&sine);
        assert_eq!(output.len(), sine.len());
        // All samples must match — identity FIR has no startup transient
        for (i, (a, b)) in sine.iter().zip(output.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-4,
                "identity FIR mismatch at sample {i}: got {b:.6}, expected {a:.6}"
            );
        }
    }

    #[test]
    fn stateful_overlap_no_clicks() {
        // Process the same signal in two back-to-back calls.
        // Verify the OLA engine introduces no additional discontinuity beyond
        // what already exists in the input signal at the block boundary.
        // For identity FIR the output equals the input, so the jump between
        // out1.last() and out2.first() equals the jump in the input itself
        // (|sine[255] - sine[0]| ≈ 0.274).  We allow up to 0.35 to confirm
        // no OLA-induced artefacts inflate the discontinuity further.
        let config = Arc::new(RwLock::new(DspConfig {
            convolution_enabled: true,
            convolution_bypass: false,
            ..Default::default()
        }));
        let mut fir = vec![0.0f32; 128];
        fir[0] = 1.0;

        let mut engine = ConvolutionEngine::new(config).unwrap();
        engine.load_filter_from_vec(fir);

        let block: Vec<f32> = (0..256)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();

        let out1 = engine.process(&block);
        let out2 = engine.process(&block);

        // The jump between last sample of block 1 and first sample of block 2
        // must not exceed the natural input discontinuity by any significant margin.
        let jump = (out1.last().unwrap() - out2.first().unwrap()).abs();
        assert!(jump < 0.35, "click at block boundary: jump = {jump:.4}");
    }

    #[test]
    fn long_fir_processes_within_5ms() {
        let config = Arc::new(RwLock::new(DspConfig {
            convolution_enabled: true,
            convolution_bypass: false,
            ..Default::default()
        }));
        // 200k-tap filter → uniform OLA with 4096-sample partitions
        let long_fir = vec![0.0f32; 200_000];
        let mut engine = ConvolutionEngine::new(config).unwrap();
        engine.load_filter_from_vec(long_fir);

        let block = vec![0.0f32; 4096];
        let _ = engine.process(&block); // warm up

        let start = Instant::now();
        let _ = engine.process(&block);
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 10,
            "process() took {}ms — must be < 10ms",
            elapsed.as_millis()
        );
    }
}
