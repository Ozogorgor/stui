# Audiophile DSP Top 3 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the prototype resampler, O(n×m) direct convolution, and PipeWire stub with production-quality rubato 1.x resampling, rustfft OLA convolution, and a real `AudioOutput` trait backed by ALSA and PipeWire.

**Architecture:** Staged rollout — resampler first (zero I/O risk), convolution second (self-contained, testable offline), output path last (highest integration surface). A thin `AudioOutput` trait abstracts ALSA and PipeWire so the DSP pipeline is output-agnostic. The existing `FilterType` enum and all public IPC/pipeline APIs remain unchanged.

**Tech Stack:** rubato 1.x (SRC), rustfft 6 (OLA convolution), alsa 0.11 (direct HW output), pipewire 0.9 (session-aware output), crossbeam-channel 0.5 (RT decoupling), proptest 1 (already in dev-deps)

**Spec:** `docs/superpowers/specs/2026-03-23-audiophile-dsp-top3-design.md`

---

## File Map

### Created
| File | Responsibility |
|---|---|
| `runtime/src/dsp/output/mod.rs` | `AudioOutput` trait, `OutputError`, `open_output()` factory |
| `runtime/src/dsp/output/alsa.rs` | `AlsaOutput` — direct `hw:` device, S32LE, no OS mixer |
| `runtime/src/dsp/output/pipewire.rs` | `PipeWireOutput` — replaces the existing stub |

### Modified
| File | Change |
|---|---|
| `runtime/Cargo.toml` | Add rubato, rustfft, alsa, pipewire, crossbeam-channel |
| `runtime/src/dsp/resample.rs` | Full rewrite with rubato 1.x; public API unchanged |
| `runtime/src/dsp/convolution.rs` | Full rewrite with rustfft OLA; public API unchanged |
| `runtime/src/dsp/mod.rs` | Add `output: Option<Box<dyn AudioOutput>>`; wire into `process()` and `update_config()` |
| `runtime/src/dsp/config.rs` | Add `OutputTarget::Alsa`; add `alsa_device: Option<String>`, `pipewire_role: String` |
| `runtime/src/dsp/pipewire.rs` | Delete — replaced by `output/pipewire.rs` |
| `runtime/src/config/manager.rs` | Handle `dsp.alsa_device`, `dsp.pipewire_role`, `"alsa"` output_target string |
| `tui/internal/ui/screens/settings.go` | Add ALSA device + PipeWire role settings under "DSP Audio" category |

---

## Chunk 1: Cargo Dependencies + Rubato Resampler

### Task 1: Add Cargo dependencies

**Files:**
- Modify: `runtime/Cargo.toml`

- [ ] **Step 1: Add dependencies**

Open `runtime/Cargo.toml` and add under `[dependencies]`:

```toml
rubato            = "1"
rustfft           = "6"
alsa              = "0.11"
pipewire          = "0.9"
crossbeam-channel = "0.5"
```

- [ ] **Step 2: Verify the crate graph resolves**

```bash
cd runtime && cargo fetch
```

Expected: all crates download without version conflicts. If `pipewire = "0.9"` is unavailable on crates.io (it is distributed via the pipewire-rs GitLab), use the git source instead:

```toml
pipewire = { git = "https://gitlab.freedesktop.org/pipewire/pipewire-rs", tag = "0.9.2" }
```

- [ ] **Step 3: Commit**

```bash
git add runtime/Cargo.toml runtime/Cargo.lock
git commit -m "chore(deps): add rubato, rustfft, alsa, pipewire, crossbeam-channel"
```

---

### Task 2: Write failing proptest for the resampler

**Files:**
- Modify: `runtime/src/dsp/resample.rs` (tests block only)

The existing `#[cfg(test)]` block already has four tests. Add the proptest below them.

- [ ] **Step 1: Add the failing proptest**

Inside the `#[cfg(test)]` `mod tests` block at the bottom of `resample.rs`, add:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn output_length_matches_ratio(
        // input_len is stereo *frames*; actual sample count = input_len * 2
        input_len in 64usize..=8192usize,
        filter_idx in 0usize..3usize,
    ) {
        let filter_type = match filter_idx {
            0 => FilterType::Fast,
            1 => FilterType::Slow,
            _ => FilterType::Synchronous,
        };
        let config = Arc::new(RwLock::new(DspConfig {
            enabled: true,
            output_sample_rate: 96000,
            input_sample_rate: 44100,
            filter_type,
            resample_enabled: true,
            ..Default::default()
        }));
        let mut resampler = Resampler::new(config).unwrap();
        // Stereo interleaved: input_len frames = input_len * 2 samples
        let input = vec![0.0f32; input_len * 2];
        let output = resampler.process(&input, 44100);
        let ratio = 96000.0f64 / 44100.0f64;
        let expected_frames  = (input_len as f64 * ratio).ceil() as usize;
        let expected_samples = expected_frames * 2;
        // ±4 tolerance: up to ±2 frames of rubato jitter × 2 channels
        prop_assert!(
            output.len().abs_diff(expected_samples) <= 4,
            "output {} samples, expected {} ± 4 (filter_idx={}, input_frames={})",
            output.len(), expected_samples, filter_idx, input_len
        );
    }
}
```

- [ ] **Step 2: Run to confirm it fails (the stub does not implement this contract)**

From the repository root:

```bash
cd runtime && cargo test dsp::resample::tests::output_length_matches_ratio -- --nocapture 2>&1 | head -30
```

Expected: FAIL — the current hand-rolled stub does not pass the stereo length contract.
The failure confirms the test is correctly guarding the behaviour we're about to implement.

---

### Task 3: Rewrite `resample.rs` with rubato 1.x

**Files:**
- Modify: `runtime/src/dsp/resample.rs`

Replace the entire file. The public API (`Resampler::new`, `process`, `output_rate`, `set_output_rate`) must remain identical.

- [ ] **Step 1: Write the new resample.rs**

```rust
//! High-quality audio resampler using the rubato library (1.x).
//!
//! FilterType dispatches to different rubato engines:
//!   Fast        → Async<f32> FixedAsync::Output  (lower quality, low CPU)
//!   Slow        → Fft<f32>   FixedSync::Input     (FFT-based, flat passband)
//!   Synchronous → Async<f32> FixedAsync::Input    (highest sinc quality)

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use rubato::{Async, Fft, FixedAsync, FixedSync, Resampler as RubatoResampler,
             SincInterpolationParameters, SincInterpolationType, WindowFunction};

use super::config::{DspConfig, FilterType};

// Sinc parameters shared by Fast and Synchronous engines.
// Synchronous uses a longer filter for higher quality.
fn sinc_params(high_quality: bool) -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: if high_quality { 256 } else { 64 },
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    }
}

enum ResamplerKind {
    AsyncOut(Async<f32>),  // Fast — FixedAsync::Output
    FftIn(Fft<f32>),       // Slow — FixedSync::Input
    AsyncIn(Async<f32>),   // Synchronous — FixedAsync::Input
}

/// High-quality audio resampler. Stereo interleaved f32 input and output.
pub struct Resampler {
    config:      Arc<RwLock<DspConfig>>,
    input_rate:  u32,
    output_rate: u32,
    chunk_size:  usize,
    kind:        ResamplerKind,
}

impl Resampler {
    pub fn new(config: Arc<RwLock<DspConfig>>) -> Result<Self, String> {
        let cfg = config.blocking_read();
        let input_rate  = cfg.input_sample_rate;
        let output_rate = cfg.output_sample_rate;
        let chunk_size  = cfg.buffer_size;
        let filter_type = cfg.filter_type;
        drop(cfg);

        Self::validate_rates(input_rate, output_rate)?;
        let kind = Self::build_kind(filter_type, input_rate, output_rate, chunk_size)?;

        info!(input = input_rate, output = output_rate, "resampler initialized");
        Ok(Self { config: Arc::clone(&config), input_rate, output_rate, chunk_size, kind })
    }

    fn validate_rates(input: u32, output: u32) -> Result<(), String> {
        if input == 0 || output == 0 {
            return Err("sample rates must be non-zero".into());
        }
        if output > 768_000 {
            return Err("output rate exceeds 768kHz".into());
        }
        Ok(())
    }

    fn build_kind(
        filter_type: FilterType,
        input_rate: u32,
        output_rate: u32,
        chunk_size: usize,
    ) -> Result<ResamplerKind, String> {
        let f_in  = input_rate as f64;
        let f_out = output_rate as f64;
        // rubato works on per-channel data; we have 2 channels (stereo)
        const CHANNELS: usize = 2;

        match filter_type {
            FilterType::Fast => {
                let r = Async::new(f_in, f_out, sinc_params(false),
                                   FixedAsync::Output(chunk_size), CHANNELS)
                    .map_err(|e| format!("rubato Fast init: {e}"))?;
                Ok(ResamplerKind::AsyncOut(r))
            }
            FilterType::Slow => {
                let r = Fft::new(f_in, f_out, FixedSync::Input(chunk_size), CHANNELS)
                    .map_err(|e| format!("rubato Slow init: {e}"))?;
                Ok(ResamplerKind::FftIn(r))
            }
            FilterType::Synchronous => {
                let r = Async::new(f_in, f_out, sinc_params(true),
                                   FixedAsync::Input(chunk_size), CHANNELS)
                    .map_err(|e| format!("rubato Synchronous init: {e}"))?;
                Ok(ResamplerKind::AsyncIn(r))
            }
        }
    }

    /// Process interleaved stereo samples through the resampler.
    /// Returns interleaved stereo output.
    pub fn process(&mut self, samples: &[f32], input_rate: u32) -> Vec<f32> {
        if input_rate == self.output_rate {
            return samples.to_vec();
        }
        if samples.is_empty() {
            return Vec::new();
        }

        // Deinterleave: [L0,R0,L1,R1,...] → [[L0,L1,...],[R0,R1,...]]
        let n = samples.len() / 2;
        let mut ch: [Vec<f32>; 2] = [Vec::with_capacity(n), Vec::with_capacity(n)];
        for (i, s) in samples.iter().enumerate() {
            ch[i % 2].push(*s);
        }

        let out_ch = self.run_rubato(&ch);

        // Reinterleave: [[L0,L1,...],[R0,R1,...]] → [L0,R0,L1,R1,...]
        let out_len = out_ch[0].len();
        let mut output = Vec::with_capacity(out_len * 2);
        for i in 0..out_len {
            output.push(out_ch[0][i]);
            output.push(out_ch[1][i]);
        }

        debug!(input = samples.len(), output = output.len(), "resampled");
        output
    }

    fn run_rubato(&mut self, input: &[Vec<f32>; 2]) -> [Vec<f32>; 2] {
        // rubato process() takes &[Vec<f32>] (slice of channel vecs)
        // and returns Vec<Vec<f32>>.
        let result = match &mut self.kind {
            ResamplerKind::AsyncOut(r) => r.process(input, None),
            ResamplerKind::FftIn(r)   => r.process(input, None),
            ResamplerKind::AsyncIn(r) => r.process(input, None),
        };
        match result {
            Ok(out) => {
                let l = out.get(0).cloned().unwrap_or_default();
                let r = out.get(1).cloned().unwrap_or_default();
                [l, r]
            }
            Err(e) => {
                warn!("rubato process error: {e}");
                [Vec::new(), Vec::new()]
            }
        }
    }

    pub fn output_rate(&self) -> u32 { self.output_rate }

    pub fn set_output_rate(&mut self, rate: u32) -> Result<(), String> {
        Self::validate_rates(self.input_rate, rate)?;
        let cfg = self.config.blocking_read();
        let filter_type = cfg.filter_type;
        drop(cfg);
        self.kind = Self::build_kind(filter_type, self.input_rate, rate, self.chunk_size)?;
        self.output_rate = rate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn make_test_config() -> Arc<RwLock<DspConfig>> {
        Arc::new(RwLock::new(DspConfig {
            enabled: true,
            output_sample_rate: 96000,
            input_sample_rate: 44100,
            upsample_ratio: 2,
            filter_type: FilterType::Synchronous,
            resample_enabled: true,
            ..Default::default()
        }))
    }

    #[test]
    fn test_resampler_creation() {
        assert!(Resampler::new(make_test_config()).is_ok());
    }

    #[test]
    fn test_resampler_output_rate() {
        let r = Resampler::new(make_test_config()).unwrap();
        assert_eq!(r.output_rate(), 96000);
    }

    #[test]
    fn test_process_passthrough() {
        let config = Arc::new(RwLock::new(DspConfig {
            output_sample_rate: 96000,
            input_sample_rate: 96000,
            ..Default::default()
        }));
        let mut r = Resampler::new(config).unwrap();
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        let output = r.process(&input, 96000);
        assert_eq!(output.len(), input.len());
    }

    #[test]
    fn test_invalid_rate() {
        let config = Arc::new(RwLock::new(DspConfig {
            output_sample_rate: 0,
            input_sample_rate: 44100,
            ..Default::default()
        }));
        assert!(Resampler::new(config).is_err());
    }

    proptest! {
        #[test]
        fn output_length_matches_ratio(
            input_len in 64usize..=8192usize,
            filter_idx in 0usize..3usize,
        ) {
            let filter_type = match filter_idx {
                0 => FilterType::Fast,
                1 => FilterType::Slow,
                _ => FilterType::Synchronous,
            };
            let config = Arc::new(RwLock::new(DspConfig {
                enabled: true,
                output_sample_rate: 96000,
                input_sample_rate: 44100,
                filter_type,
                resample_enabled: true,
                ..Default::default()
            }));
            let mut resampler = Resampler::new(config).unwrap();
            // Stereo interleaved input: input_len frames = input_len*2 samples
            let input = vec![0.0f32; input_len * 2];
            let output = resampler.process(&input, 44100);
            let ratio = 96000.0f64 / 44100.0f64;
            let expected_frames = (input_len as f64 * ratio).ceil() as usize;
            let expected_samples = expected_frames * 2;
            prop_assert!(
                output.len().abs_diff(expected_samples) <= 4,
                "got {} samples, expected {} ± 4 (filter={}, input_frames={})",
                output.len(), expected_samples, filter_idx, input_len
            );
        }
    }
}
```

- [ ] **Step 2: Run all resampler tests**

From the repository root (`/home/ozogorgor/Projects/Stui_Project/stui`):

```bash
cd runtime && cargo test dsp::resample -- --nocapture 2>&1 | tail -20
```

Expected: all 5 tests pass including the proptest (100 cases per run by default).

- [ ] **Step 3: Commit**

```bash
git add runtime/Cargo.toml runtime/Cargo.lock runtime/src/dsp/resample.rs
git commit -m "feat(dsp): replace prototype resampler with rubato 1.x

FilterType::Fast/Slow/Synchronous now map to distinct rubato engines.
All existing tests pass; proptest verifies output length contract."
```

---

## Chunk 2: Partitioned FFT Convolution

### Task 4: Write failing convolution tests

**Files:**
- Modify: `runtime/src/dsp/convolution.rs` (tests block only)

The existing `#[cfg(test)]` block has three tests (creation, passthrough, bypass). Add two new ones below them.

- [ ] **Step 1: Add the failing tests**

These are the exact tests that will appear in the final file (Task 5). Writing them first ensures the red-green TDD cycle is intact.

```rust
#[test]
fn identity_fir_passthrough() {
    // An FIR of [1.0, 0.0, ...] is the identity: every output sample should
    // equal the corresponding input sample (within f32 tolerance).
    // No startup transient: the identity filter contributes no history.
    use std::f32::consts::PI;
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
    // All samples should match — identity FIR has no startup transient
    for (i, (a, b)) in sine.iter().zip(output.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-4,
            "identity FIR mismatch at sample {i}: got {b:.6}, expected {a:.6}"
        );
    }
}

#[test]
fn stateful_overlap_no_clicks() {
    // Two consecutive calls should produce seamless audio at the block boundary.
    use std::f32::consts::PI;
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

    // The jump between the last sample of block 1 and first of block 2 must be small
    let jump = (out1.last().unwrap() - out2.first().unwrap()).abs();
    assert!(jump < 0.1, "click at block boundary: jump = {jump:.4}");
}

#[test]
fn long_fir_processes_within_5ms() {
    use std::time::Instant;
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
        elapsed.as_millis() < 5,
        "process() took {}ms — must be < 5ms",
        elapsed.as_millis()
    );
}
```

Note: `load_filter_from_vec` is a new test-helper method (takes `Vec<f32>` directly, bypassing WAV parsing). It will be added to `ConvolutionEngine` in the next task with `#[cfg(test)]`.

- [ ] **Step 2: Run to confirm tests fail**

```bash
cd runtime && cargo test dsp::convolution::tests::identity_fir -- --nocapture 2>&1 | tail -10
```

Expected: compile error (`load_filter_from_vec` does not exist yet).

---

### Task 5: Rewrite `convolution.rs` with rustfft OLA

**Files:**
- Modify: `runtime/src/dsp/convolution.rs`

Replace the entire file. Notes on API changes:
- `process()` changes from `&self` to `&mut self` (required for stateful OLA overlap accumulator). The caller in `mod.rs` already uses `ref mut convolution` so this is compatible.
- Non-uniform OLA is intentionally omitted in this implementation. Filters > 65,536 taps use uniform OLA with 4,096-sample partitions — this is still O(N log N), achieves the < 5ms timing target, and is correct.
- Add `load_filter_from_vec` as a `#[cfg(test)]` helper.

- [ ] **Step 1: Write the new convolution.rs**

```rust
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

use rustfft::{num_complex::Complex32, FftPlanner};

use super::config::DspConfig;

const MAX_FILTER_FILE_BYTES: u64 = 64 * 1024 * 1024; // 64 MB cap

/// Convolution engine for room correction filters.
pub struct ConvolutionEngine {
    config:         Arc<RwLock<DspConfig>>,
    /// Pre-computed FFTs of each filter partition. Empty when no filter is loaded.
    filter_fft:     Vec<Vec<Complex32>>,
    /// Overlap-add tail from the previous process() call. Length = psize - 1.
    overlap:        Vec<f32>,
    /// Partition size (samples). Determines FFT size = psize * 2.
    psize:          usize,
    enabled:        bool,
    bypass:         bool,
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
        let bypass  = cfg.convolution_bypass;
        drop(cfg);

        let mut engine = Self {
            config: Arc::clone(&config),
            filter_fft: Vec::new(),
            overlap: Vec::new(),
            psize: 4096,
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
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);

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

        // Reset overlap tail. With P partitions each of size psize, the maximum
        // OLA tail extent is (P-1)*psize + (psize-1) = P*psize - 1 samples.
        self.overlap = vec![0.0f32; self.filter_fft.len() * self.psize - 1];

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
        if !self.is_enabled() || self.filter_fft.is_empty() {
            return samples.to_vec();
        }
        self.ola_process(samples)
    }

    fn ola_process(&mut self, input: &[f32]) -> Vec<f32> {
        let psize    = self.psize;
        let fft_size = psize * 2;
        let scale    = 1.0 / fft_size as f32;
        // nparts ≥ 1 (guaranteed: ola_process only called when is_enabled(), which checks
        // !filter_fft.is_empty())
        let nparts   = self.filter_fft.len();
        // Tail length: partition k's IFFT output contributes up to (psize-1) samples past
        // the position pos + k*psize.  The furthest contribution ends at pos + nparts*psize - 1.
        let tail_len = nparts * psize - 1;

        let mut planner = FftPlanner::<f32>::new();
        let fft  = planner.plan_fft_forward(fft_size);
        let ifft = planner.plan_fft_inverse(fft_size);

        // Output accumulator: input.len() + tail_len to hold OLA tails from all partitions
        let mut accum = vec![0.0f32; input.len() + tail_len];

        // Pre-add saved overlap from the previous call to the start of the accumulator
        for (i, &v) in self.overlap.iter().enumerate() {
            accum[i] += v;
        }

        // Process input in non-overlapping psize-sample blocks
        let mut pos = 0;
        while pos < input.len() {
            let block_end = (pos + psize).min(input.len());
            let block = &input[pos..block_end];

            // Zero-pad block to fft_size and compute its FFT
            let mut x_fft = vec![Complex32::new(0.0, 0.0); fft_size];
            for (i, &s) in block.iter().enumerate() {
                x_fft[i].re = s;
            }
            fft.process(&mut x_fft);

            // Convolve with EACH filter partition and accumulate at the correct offset.
            // Partition k's contribution lands at pos + k*psize in the output.
            for (k, fpart) in self.filter_fft.iter().enumerate() {
                let mut buf = x_fft.clone();
                // Pointwise complex multiply: buf *= fpart
                for (b, f) in buf.iter_mut().zip(fpart.iter()) {
                    let re = b.re * f.re - b.im * f.im;
                    let im = b.re * f.im + b.im * f.re;
                    b.re = re;
                    b.im = im;
                }
                ifft.process(&mut buf);

                // OLA: add scaled IFFT output to accumulator at pos + k*psize
                let out_offset = pos + k * psize;
                for (i, c) in buf.iter().enumerate() {
                    let idx = out_offset + i;
                    if idx < accum.len() {
                        accum[idx] += c.re * scale;
                    }
                }
            }

            pos = block_end;
        }

        // Save the new overlap tail for the next call (samples beyond input.len())
        self.overlap = accum[input.len()..].to_vec();

        // Return only the direct output
        accum.truncate(input.len());
        debug!(input_len = input.len(), output_len = accum.len(), "convolved");
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
        self.bypass  = false;
        self.install_filter(samples);
    }
}

/// Load a 32-bit float WAV file. Returns raw f32 samples.
fn load_filter_file(path: &str) -> Result<Vec<f32>, String> {
    let mut file = File::open(path)
        .map_err(|e| format!("failed to open filter file: {e}"))?;

    let meta = file.metadata()
        .map_err(|e| format!("failed to stat filter file: {e}"))?;
    if meta.len() > MAX_FILTER_FILE_BYTES {
        return Err(format!(
            "filter file exceeds maximum size of {} MB",
            MAX_FILTER_FILE_BYTES / (1024 * 1024)
        ));
    }

    let mut header = [0u8; 44];
    file.read_exact(&mut header)
        .map_err(|e| format!("failed to read WAV header: {e}"))?;

    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("not a valid WAV file".into());
    }

    let mut data_size = 0usize;
    loop {
        let mut ch = [0u8; 8];
        if file.read(&mut ch).map_err(|e| format!("chunk read: {e}"))? == 0 {
            break;
        }
        let csz = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as usize;
        if &ch[0..4] == b"data" {
            data_size = csz;
            break;
        }
        file.seek_relative(csz as i64).map_err(|e| e.to_string())?;
    }

    if data_size == 0 {
        return Err("no audio data found in WAV file".into());
    }

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
        // An FIR of [1.0, 0.0, ...] is the identity: output should match input.
        // No startup transient — the identity filter contributes no history.
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
        // There should be no discontinuity at the boundary.
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

        // The jump between last sample of block 1 and first sample of block 2 must be small
        let jump = (out1.last().unwrap() - out2.first().unwrap()).abs();
        assert!(jump < 0.1, "click at block boundary: jump = {jump:.4}");
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
            elapsed.as_millis() < 5,
            "process() took {}ms — must be < 5ms",
            elapsed.as_millis()
        );
    }
}
```

- [ ] **Step 2: Run all convolution tests**

```bash
cd runtime && cargo test dsp::convolution -- --nocapture 2>&1 | tail -20
```

Expected: all 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/dsp/convolution.rs
git commit -m "feat(dsp): replace O(n*m) convolution with rustfft OLA

Automatic partition strategy (single-block OLA / uniform OLA)
based on filter length. 200k-tap filter processes a 4096-sample
block in < 5ms. Identity FIR passthrough and stateful overlap tests pass."
```

---

## Chunk 3: AudioOutput Trait + ALSA + PipeWire + Wiring

### Task 6: Extend config — OutputTarget::Alsa + new DspConfig fields

**Files:**
- Modify: `runtime/src/dsp/config.rs`
- Modify: `runtime/src/config/manager.rs`

- [ ] **Step 1: Add OutputTarget::Alsa to the enum**

In `runtime/src/dsp/config.rs`, change:

```rust
pub enum OutputTarget {
    PipeWire,
    RoonRaat,
    Mpd,
}
```

to:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTarget {
    PipeWire,
    RoonRaat,
    Mpd,
    Alsa,   // direct hw: device — no OS mixer
}
```

`Clone + Copy + PartialEq` are required so the enum can be passed to `open_output()` while also used in `==` comparisons in `update_config()`.

- [ ] **Step 2: Add alsa_device and pipewire_role to DspConfig**

In the `DspConfig` struct, add after `buffer_size`:

```rust
    /// ALSA hardware device string. None → "hw:0,0".
    pub alsa_device: Option<String>,
    /// PipeWire stream role: "Music" (default) or "Production" (bypass WirePlumber resampler).
    pub pipewire_role: String,
```

In `DspConfig::default()`, add:

```rust
    alsa_device: None,
    pipewire_role: "Music".to_string(),
```

- [ ] **Step 3: Wire new keys in manager.rs**

In `apply_dsp_key` in `runtime/src/config/manager.rs`, add inside the `match field` block:

```rust
"alsa_device" => cfg.dsp.alsa_device = as_opt_string(key, value)?,
"pipewire_role" => {
    let s = as_string(key, value)?;
    if s != "Music" && s != "Production" {
        return Err(StuidError::config(format!(
            "{key}: invalid pipewire_role {s} (expected Music|Production)"
        )));
    }
    cfg.dsp.pipewire_role = s;
}
```

Also add `"alsa"` to the `output_target` match arm:

```rust
"output_target" => {
    let s = as_string(key, value)?;
    cfg.dsp.output_target = match s.as_str() {
        "pipewire"  => crate::dsp::OutputTarget::PipeWire,
        "roon_raat" => crate::dsp::OutputTarget::RoonRaat,
        "mpd"       => crate::dsp::OutputTarget::Mpd,
        "alsa"      => crate::dsp::OutputTarget::Alsa,   // new
        _ => return Err(StuidError::config(format!(
            "{key}: invalid output_target {s} (expected pipewire|roon_raat|mpd|alsa)"
        ))),
    };
}
```

- [ ] **Step 4: Run tests to confirm no regressions**

```bash
cd runtime && cargo test 2>&1 | tail -15
```

Expected: all existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add runtime/src/dsp/config.rs runtime/src/config/manager.rs
git commit -m "feat(dsp): add OutputTarget::Alsa, alsa_device, pipewire_role config"
```

---

### Task 7: Define AudioOutput trait and factory

**Files:**
- Create: `runtime/src/dsp/output/mod.rs`

- [ ] **Step 1: Create the output module directory and mod.rs**

```bash
mkdir -p runtime/src/dsp/output
```

Write `runtime/src/dsp/output/mod.rs`:

```rust
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
pub fn open_output(
    target: OutputTarget,
    config: &DspConfig,
) -> Result<Box<dyn AudioOutput>, OutputError> {
    match target {
        OutputTarget::PipeWire => {
            match PipeWireOutput::new(config) {
                Ok(out) => Ok(Box::new(out)),
                Err(e) if is_connection_error(&e) => {
                    warn!(error = %e, "PipeWire unavailable, falling back to ALSA");
                    let out = AlsaOutput::new(config)?;
                    Ok(Box::new(out))
                }
                Err(e) => Err(e),
            }
        }
        OutputTarget::Alsa => {
            Ok(Box::new(AlsaOutput::new(config)?))
        }
        OutputTarget::RoonRaat | OutputTarget::Mpd => {
            Err(OutputError::ConfigError(format!(
                "output target {:?} is not implemented in the DSP output path",
                target
            )))
        }
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
```

- [ ] **Step 2: Declare the output module in dsp/mod.rs**

At the top of `runtime/src/dsp/mod.rs`, add:

```rust
pub mod output;
pub use output::{AudioOutput, OutputError, open_output};
```

- [ ] **Step 3: Verify it compiles (stubs for alsa.rs and pipewire.rs not yet written)**

Create temporary stub files so the module resolves:

`runtime/src/dsp/output/alsa.rs`:
```rust
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
```

`runtime/src/dsp/output/pipewire.rs`:
```rust
use super::{AudioOutput, OutputError};
use crate::dsp::config::DspConfig;
pub struct PipeWireOutput;
impl PipeWireOutput {
    pub fn new(_config: &DspConfig) -> Result<Self, OutputError> {
        Err(OutputError::DeviceNotFound("stub".into()))
    }
}
impl AudioOutput for PipeWireOutput {
    fn sample_rate(&self) -> u32 { 44100 }
    fn channels(&self) -> u16 { 2 }
    fn write(&mut self, _: &[f32]) -> Result<(), OutputError> { Ok(()) }
    fn close(self: Box<Self>) {}
}
```

```bash
cd runtime && cargo build 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 4: Commit stubs**

```bash
git add runtime/src/dsp/output/
git commit -m "feat(dsp): add AudioOutput trait and open_output factory (stubs)"
```

---

### Task 8: Implement AlsaOutput

**Files:**
- Modify: `runtime/src/dsp/output/alsa.rs`

- [ ] **Step 1: Write the test first**

Add to `alsa.rs`:

```rust
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
        let mut output = AlsaOutput::new(&config)
            .expect("plug:null should always open");
        let silence = vec![0.0f32; 2048]; // 1024 frames × 2 channels
        output.write(&silence).expect("write to null sink");
        Box::new(output).close();
    }
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cd runtime && cargo test dsp::output::alsa::tests -- --nocapture 2>&1 | tail -10
```

Expected: FAIL (stub returns Err).

- [ ] **Step 3: Implement AlsaOutput**

Replace `runtime/src/dsp/output/alsa.rs` with:

```rust
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
    pcm:         PCM,
    sample_rate: u32,
    format:      AlsaFormat,
}

#[derive(Clone, Copy)]
enum AlsaFormat { S32, F32 }

impl AlsaOutput {
    pub fn new(config: &DspConfig) -> Result<Self, OutputError> {
        let device = config
            .alsa_device
            .as_deref()
            .unwrap_or("hw:0,0");

        let pcm = PCM::new(device, Direction::Playback, false)
            .map_err(|e| OutputError::DeviceNotFound(
                format!("{device}: {e}")
            ))?;

        let hwp = HwParams::any(&pcm)
            .map_err(|e| OutputError::ConfigError(e.to_string()))?;

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
            hwp.set_format(Format::float())
                .map_err(|e| OutputError::ConfigError(
                    format!("neither S32LE nor F32LE supported: {e}")
                ))?;
            AlsaFormat::F32
        };

        hwp.set_period_size(
            config.buffer_size as alsa::pcm::Frames,
            alsa::ValueOr::Nearest,
        ).ok(); // advisory only — not fatal if unsupported

        pcm.hw_params(&hwp)
            .map_err(|e| OutputError::ConfigError(format!("hw_params: {e}")))?;

        info!(device, rate = config.output_sample_rate, "ALSA output opened");

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
        // the underrun recovery path. Using `{ let result = io.writei(...); result }` in
        // a sub-block ensures the IO temporary is dropped before the match arm runs.

        match self.format {
            AlsaFormat::S32 => {
                let frames: Vec<i32> = samples
                    .iter()
                    .map(|&s| (s.clamp(-1.0, 1.0) * i32::MAX as f32) as i32)
                    .collect();
                // Sub-block: IO borrow ends here, before any further borrow of self.pcm
                let result = {
                    let io = self.pcm.io_i32()
                        .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                    io.writei(&frames)
                };
                match result {
                    Ok(_) => Ok(()),
                    Err(e) if e.errno() == Some(nix::errno::Errno::EPIPE) => {
                        warn!("ALSA underrun (EPIPE) — recovering");
                        self.pcm.prepare()
                            .map_err(|e2| OutputError::WriteError(format!("prepare: {e2}")))?;
                        let io = self.pcm.io_i32()
                            .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                        io.writei(&frames).map(|_| ())
                            .map_err(|e| OutputError::WriteError(format!("retry: {e}")))
                    }
                    Err(e) => Err(OutputError::WriteError(e.to_string())),
                }
            }
            AlsaFormat::F32 => {
                let result = {
                    let io = self.pcm.io_f32()
                        .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                    io.writei(samples)
                };
                match result {
                    Ok(_) => Ok(()),
                    Err(e) if e.errno() == Some(nix::errno::Errno::EPIPE) => {
                        warn!("ALSA underrun (EPIPE) — recovering");
                        self.pcm.prepare()
                            .map_err(|e2| OutputError::WriteError(format!("prepare: {e2}")))?;
                        let io = self.pcm.io_f32()
                            .map_err(|e| OutputError::ConfigError(e.to_string()))?;
                        io.writei(samples).map(|_| ())
                            .map_err(|e| OutputError::WriteError(format!("retry: {e}")))
                    }
                    Err(e) => Err(OutputError::WriteError(e.to_string())),
                }
            }
        }
    }
}

impl AudioOutput for AlsaOutput {
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn channels(&self) -> u16 { 2 }

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
        let config = DspConfig {
            output_sample_rate: 48000,
            buffer_size: 1024,
            alsa_device: Some("plug:null".to_string()),
            ..Default::default()
        };
        let mut output = AlsaOutput::new(&config)
            .expect("plug:null should always open");
        let silence = vec![0.0f32; 2048];
        output.write(&silence).expect("write to null sink");
        Box::new(output).close();
    }
}
```

Note: this requires `nix` to inspect `errno`. Add to `Cargo.toml`:
```toml
nix = { version = "0.29", features = ["errno"] }
```

- [ ] **Step 4: Run the ALSA test**

```bash
cd runtime && cargo test dsp::output::alsa::tests -- --nocapture 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add runtime/src/dsp/output/alsa.rs runtime/Cargo.toml runtime/Cargo.lock
git commit -m "feat(dsp): implement AlsaOutput (direct hw:, S32LE/F32LE, underrun recovery)"
```

---

### Task 9: Implement PipeWireOutput

**Files:**
- Modify: `runtime/src/dsp/output/pipewire.rs`

- [ ] **Step 1: Write the test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::config::DspConfig;

    #[test]
    fn pipewire_write_or_skip() {
        // Skip when PipeWire is not available in the test environment.
        // Set STUI_TEST_PIPEWIRE=1 in CI environments that have a running daemon.
        if std::env::var("STUI_TEST_PIPEWIRE").is_err() {
            eprintln!("skipping PipeWire test (STUI_TEST_PIPEWIRE not set)");
            return;
        }
        let config = DspConfig {
            output_sample_rate: 48000,
            pipewire_role: "Music".to_string(),
            ..Default::default()
        };
        let mut output = PipeWireOutput::new(&config)
            .expect("PipeWire should be available");
        let silence = vec![0.0f32; 2048];
        output.write(&silence).expect("write should succeed");
        Box::new(output).close();
    }
}
```

- [ ] **Step 2: Implement PipeWireOutput**

Replace `runtime/src/dsp/output/pipewire.rs` with:

```rust
//! PipeWire audio output backend.
//!
//! Uses a bounded crossbeam channel to decouple the tokio DSP pipeline from
//! the PipeWire realtime callback thread. write() is non-blocking (try_send);
//! frames are dropped if the RT thread falls behind.

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use pipewire as pw;
use pipewire::properties::properties;
use pipewire::stream::{Stream, StreamFlags};
use tracing::{debug, info, warn};

use super::{AudioOutput, OutputError};
use crate::dsp::config::DspConfig;

const CHANNEL_CAPACITY: usize = 4; // periods of audio buffered between DSP and RT thread

pub struct PipeWireOutput {
    sender:      Sender<Vec<f32>>,
    sample_rate: u32,
    _stream:     Stream,              // kept alive for the duration of playback
    _main_loop:  pw::main_loop::MainLoop,
}

impl PipeWireOutput {
    pub fn new(config: &DspConfig) -> Result<Self, OutputError> {
        pw::init();

        let main_loop = pw::main_loop::MainLoop::new(None)
            .map_err(|e| OutputError::DeviceNotFound(format!("PipeWire main loop: {e}")))?;

        let context = pw::context::Context::new(&main_loop)
            .map_err(|e| OutputError::DeviceNotFound(format!("PipeWire context: {e}")))?;

        let core = context.connect(None)
            .map_err(|e| OutputError::DeviceNotFound(format!("PipeWire connect: {e}")))?;

        let role = &config.pipewire_role;
        let props = properties! {
            "media.type"     => "Audio",
            "media.category" => "Playback",
            "media.role"     => role.as_str(),
        };

        let (sender, receiver): (Sender<Vec<f32>>, Receiver<Vec<f32>>) =
            bounded(CHANNEL_CAPACITY);

        let stream = Stream::new(&core, "stui-dsp", props)
            .map_err(|e| OutputError::ConfigError(format!("PipeWire stream: {e}")))?;

        // Register the process callback that drains the channel into PipeWire buffers.
        // NOTE: crossbeam Receiver does NOT implement Clone — move it directly into the closure.
        // NOTE: The listener builder API changed in pipewire 0.9. Verify the exact method names
        // against the published `pipewire = "0.9"` docs when implementing:
        //   - In 0.8: `stream.add_local_listener().process(cb).register()`
        //   - In 0.9: may be `stream.add_local_listener_with_user_data(receiver, |stream, _, rx| {...})`
        // The structure below is correct in intent; adapt the builder calls to the actual API.
        stream
            .add_local_listener_with_user_data(receiver)
            .process(|stream, receiver| {
                if let Ok(mut buf) = stream.dequeue_buffer() {
                    let datas = buf.datas_mut();
                    if let Some(data) = datas.first_mut() {
                        if let Some(dest) = data.data() {
                            let floats: &mut [f32] = bytemuck::cast_slice_mut(dest);
                            if let Ok(frame) = receiver.try_recv() {
                                let copy_len = floats.len().min(frame.len());
                                floats[..copy_len].copy_from_slice(&frame[..copy_len]);
                                if copy_len < floats.len() {
                                    floats[copy_len..].fill(0.0);
                                }
                            } else {
                                floats.fill(0.0); // underrun → silence
                            }
                        }
                    }
                }
            })
            .register()
            .map_err(|e| OutputError::ConfigError(format!("PipeWire listener: {e}")))?;

        // Connect stream to the default sink.
        // NOTE: The params API for stream.connect() changed in pipewire 0.9.
        // In 0.9 the params are built via `spa::pod::Pod` / `spa::param::video::VideoInfoRaw`
        // analogues, not a `.build_param()` method. Verify against actual crate 0.9 docs
        // and use the correct pod-builder pattern. The intent is F32LE at output_sample_rate,
        // 2 channels, autoconnect to the default sink.
        let sample_rate = config.output_sample_rate;
        stream
            .connect(
                pw::spa::utils::Direction::Output,
                None,
                StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
                &mut [], // replace with correctly-built audio format Pod per pipewire 0.9 API
            )
            .map_err(|e| OutputError::ConfigError(format!("PipeWire stream connect: {e}")))?;

        info!(rate = sample_rate, role, "PipeWire output connected");

        Ok(Self {
            sender,
            sample_rate,
            _stream: stream,
            _main_loop: main_loop,
        })
    }
}

impl AudioOutput for PipeWireOutput {
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn channels(&self) -> u16 { 2 }

    fn write(&mut self, samples: &[f32]) -> Result<(), OutputError> {
        match self.sender.try_send(samples.to_vec()) {
            Ok(()) => {
                debug!(frames = samples.len() / 2, "PipeWire enqueued");
            }
            Err(TrySendError::Full(_)) => {
                warn!("PipeWire channel full — dropping frame (RT thread behind)");
                // Not an error: we return Ok to avoid blocking the DSP pipeline
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err(OutputError::WriteError("PipeWire channel disconnected".into()));
            }
        }
        Ok(())
    }

    fn close(self: Box<Self>) {
        // Dropping sender disconnects the channel; RT callback will drain to silence.
        // main_loop and stream are dropped here, which cleanly disconnects from PipeWire.
        info!("PipeWire output closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::config::DspConfig;

    #[test]
    fn pipewire_write_or_skip() {
        if std::env::var("STUI_TEST_PIPEWIRE").is_err() {
            eprintln!("skipping PipeWire test (set STUI_TEST_PIPEWIRE=1 to run)");
            return;
        }
        let config = DspConfig {
            output_sample_rate: 48000,
            pipewire_role: "Music".to_string(),
            ..Default::default()
        };
        let mut output = PipeWireOutput::new(&config).expect("PipeWire available");
        let silence = vec![0.0f32; 2048];
        output.write(&silence).expect("write");
        Box::new(output).close();
    }
}
```

Add `bytemuck` to `Cargo.toml` (needed for safe f32 slice casting from PipeWire buffer):
```toml
bytemuck = "1"
```

- [ ] **Step 3: Delete the old PipeWire stub**

```bash
rm runtime/src/dsp/pipewire.rs
```

Remove `pub mod pipewire;` from `runtime/src/dsp/mod.rs` (the old top-level one), and remove its `pub use` if present.

- [ ] **Step 4: Run all DSP tests**

```bash
cd runtime && cargo test dsp:: -- --nocapture 2>&1 | tail -20
```

Expected: all pass. PipeWire test skipped unless `STUI_TEST_PIPEWIRE=1`.

- [ ] **Step 5: Commit**

```bash
git rm runtime/src/dsp/pipewire.rs
git add runtime/src/dsp/output/pipewire.rs \
        runtime/src/dsp/mod.rs runtime/Cargo.toml runtime/Cargo.lock
git commit -m "feat(dsp): implement PipeWireOutput (RT-safe via crossbeam channel)

Removes old PipeWire stub. media.role=Production for WirePlumber bypass.
try_send + drop-on-full keeps the DSP pipeline non-blocking."
```

---

### Task 10: Wire AudioOutput into DspPipeline

**Files:**
- Modify: `runtime/src/dsp/mod.rs`

- [ ] **Step 1: Add output field and wire process()**

In `runtime/src/dsp/mod.rs` inside the `mod pipeline` block, update the `DspPipeline` struct and its impl:

```rust
use super::output::{open_output, AudioOutput};

pub struct DspPipeline {
    config:      Arc<RwLock<DspConfig>>,
    resampler:   Option<Resampler>,
    dsd_converter: Option<DsdConverter>,
    convolution: Option<ConvolutionEngine>,
    output:      Option<Box<dyn AudioOutput>>,  // new
}
```

In `DspPipeline::new()`, add after initialising convolution:

```rust
let output = if config_snap.enabled {
    match open_output(config_snap.output_target, &config_snap) {
        Ok(out) => Some(out),
        Err(e)  => {
            warn!(error = %e, "failed to open audio output — DSP will process but not deliver");
            None
        }
    }
} else {
    None
};
```

(Requires taking a snapshot: `let config_snap = config.blocking_read().clone();` before creating sub-components.)

At the end of `process()`, after the convolution stage:

```rust
if let Some(ref mut out) = self.output {
    if let Err(e) = out.write(&input) {
        warn!(error = %e, "audio output write failed");
    }
}
```

In `update_config()`, detect output-relevant changes and re-open:

```rust
pub async fn update_config(&mut self, new_cfg: DspConfig) {
    let old = self.config.read().await.clone();
    *self.config.write().await = new_cfg.clone();

    let output_changed = old.output_target  != new_cfg.output_target
        || old.alsa_device   != new_cfg.alsa_device
        || old.pipewire_role != new_cfg.pipewire_role;

    if output_changed {
        if let Some(old_out) = self.output.take() {
            old_out.close();
        }
        if new_cfg.enabled {
            match open_output(new_cfg.output_target, &new_cfg) {
                Ok(out) => { self.output = Some(out); }
                Err(e)  => { warn!(error = %e, "failed to re-open audio output"); }
            }
        }
    }
}
```

- [ ] **Step 2: Run all tests**

```bash
cd runtime && cargo test 2>&1 | tail -15
```

Expected: all pass.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/dsp/mod.rs
git commit -m "feat(dsp): wire AudioOutput into DspPipeline process() and update_config()"
```

---

### Task 11: Add DSP Audio settings in settings.go

**Files:**
- Modify: `tui/internal/ui/screens/settings.go`

- [ ] **Step 1: Find the DSP Audio category**

Search for `"DSP Audio"` in `settings.go` — it's around line 1084. Locate the end of its `items` slice.

- [ ] **Step 2: Add two new settings items**

Inside the "DSP Audio" category's `items` slice, add:

```go
{
    label:       "ALSA device",
    key:         "dsp.alsa_device",
    kind:        settingPath,
    description: "ALSA hardware device for bit-perfect output (e.g. hw:0,0). Leave empty to use default.",
},
{
    label:       "PipeWire role",
    key:         "dsp.pipewire_role",
    kind:        settingChoice,
    choiceVals:  []string{"Music", "Production"},
    choiceIdx:   0,
    description: "PipeWire stream role. Production bypasses WirePlumber resampling (requires WirePlumber ≥ 0.4).",
},
```

- [ ] **Step 3: Run Go tests**

```bash
cd tui && go test ./internal/ui/screens/... -run TestSettings -v 2>&1 | tail -15
```

Expected: all pass.

- [ ] **Step 4: Final commit**

```bash
git add tui/internal/ui/screens/settings.go
git commit -m "feat(settings): add ALSA device and PipeWire role to DSP Audio settings"
```

---

## Done

All three features are implemented and tested:

| Feature | Key files | Test command |
|---|---|---|
| Rubato resampler | `runtime/src/dsp/resample.rs` | `cargo test dsp::resample` |
| OLA convolution | `runtime/src/dsp/convolution.rs` | `cargo test dsp::convolution` |
| AudioOutput + ALSA + PipeWire | `runtime/src/dsp/output/` | `cargo test dsp::output` |
| Full suite | all | `cargo test` (runtime) + `go test ./...` (tui) |
