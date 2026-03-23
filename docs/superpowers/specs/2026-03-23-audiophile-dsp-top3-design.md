# Audiophile DSP — Top 3 Features Design

**Date:** 2026-03-23
**Status:** Approved
**Scope:** STUI runtime DSP pipeline — rubato resampler, partitioned FFT convolution, bit-perfect audio output (ALSA + PipeWire)

---

## Background

The existing DSP pipeline has three significant gaps that limit audiophile use:

1. **Resampler** — uses a hand-rolled linear interpolation / windowed-sinc prototype. The `FilterType` setting in the UI has no effect because all paths share the same stub code.
2. **Convolution** — uses O(n×m) direct convolution. Unusable for real room correction FIRs (65k–1M taps).
3. **Output path** — the PipeWire backend is a stub that logs but does nothing. ALSA direct output does not exist. The OS mixer resamples all audio to 48kHz, defeating the upsampling pipeline upstream.

This spec covers replacing all three with production-quality implementations using a staged rollout strategy.

---

## Goals

- Replace the resampler with `rubato`, wiring `FilterType::Fast/Slow/Synchronous` to distinct rubato engines
- Replace direct convolution with partitioned overlap-add FFT convolution via `rustfft`, with automatic partition strategy based on filter length
- Implement a thin `AudioOutput` trait with real ALSA and PipeWire backends, enabling bit-perfect delivery
- Keep all public-facing APIs (IPC, DSP pipeline interface, settings keys) unchanged
- PipeWire is the primary output path; ALSA is a fallback and an explicit user choice

---

## Non-Goals

- macOS CoreAudio support
- JACK output
- Sample format conversion beyond f32 internal / S32LE or F32LE output
- MQA unfolding
- Parametric EQ, crossfeed, dither (separate features)

---

## New Dependencies

```toml
rubato    = "0.15"
rustfft   = "6"
alsa      = "0.9"
pipewire  = "0.8"
crossbeam-channel = "0.5"
```

---

## Architecture

### Approach

Unified `AudioOutput` trait with staged rollout:

1. **Resampler** (lowest risk, self-contained, no I/O)
2. **Convolution** (self-contained, verifiable against old implementation)
3. **AudioOutput trait + backends** (highest integration surface, built last)

### Module Layout

```
runtime/src/dsp/
  output/
    mod.rs        ← AudioOutput trait + open_output() factory
    alsa.rs       ← AlsaOutput
    pipewire.rs   ← PipeWireOutput (replaces current stub)
  resample.rs     ← rewritten with rubato
  convolution.rs  ← rewritten with rustfft OLA
  mod.rs          ← pipeline wiring (adds output field)
  config.rs       ← two new fields: alsa_device, pipewire_role
```

---

## Section 1: Rubato Resampler

### FilterType Mapping

| `FilterType` | Rubato engine     | Characteristics                                      |
|--------------|-------------------|------------------------------------------------------|
| `Fast`       | `SincFixedOut`    | Lower sinc quality, ~4ms latency, minimal CPU        |
| `Slow`       | `FftFixedIn`      | FFT-based, flat passband to Nyquist, ~10ms latency   |
| `Synchronous`| `SincFixedIn`     | Highest quality sinc, variable output length         |

### Internal Design

```rust
enum ResamplerKind {
    Fft(FftFixedIn<f32>),
    SincOut(SincFixedOut<f32>),
    SincIn(SincFixedIn<f32>),
}

pub struct Resampler {
    config:      Arc<RwLock<DspConfig>>,
    input_rate:  u32,
    output_rate: u32,
    chunk_size:  usize,
    kind:        ResamplerKind,
}
```

- `process()` dispatches on `ResamplerKind`, handles rubato's chunk-size contract (padding/draining as needed), and always returns `ceil(input_len × ratio) ± 2` samples
- `FilterType` changes at runtime reconstruct only the inner `ResamplerKind`; the `Resampler` struct is not replaced
- Public API (`process`, `output_rate`, `set_output_rate`) is unchanged

### Validation

- Existing unit tests remain valid
- New `proptest` property test: for any input length 64–8192 and all three filter types, output length is within ±2 of `ceil(input_len × ratio)`

---

## Section 2: Partitioned FFT Convolution

### Partition Strategy (automatic, based on filter length at load time)

| Filter length       | Strategy                  | Partition size                         |
|---------------------|---------------------------|----------------------------------------|
| ≤ 4,096 taps        | Single-block OLA          | `next_power_of_two(filter_len × 2)`    |
| 4,097 – 65,536 taps | Uniformly partitioned OLA | 4,096 samples per partition            |
| > 65,536 taps       | Non-uniform partitioned OLA | First 512, doubling up to 65,536      |

Non-uniform partitioning keeps first-partition latency low (~512 samples / ~10ms at 48kHz) while amortising the cost of the long filter tail across larger, less frequent FFTs.

### Internal Design

```rust
enum ConvStrategy { SingleBlock, Uniform, NonUniform }

pub struct ConvolutionEngine {
    config:         Arc<RwLock<DspConfig>>,
    filter_fft:     Vec<Vec<Complex32>>,   // pre-computed per-partition filter FFTs
    overlap:        Vec<f32>,              // OLA accumulator
    partition_size: usize,
    strategy:       ConvStrategy,
    enabled:        bool,
    bypass:         bool,
}
```

**At `load_filter()` time:**
1. Load WAV (existing path, with 64MB cap)
2. Determine strategy from filter length
3. Partition into chunks, zero-pad to `2 × partition_size`, compute FFT, store as `Vec<Complex32>`

**At `process()` time:**
1. FFT input partitions
2. Pointwise multiply against stored filter FFTs
3. IFFT, OLA accumulate, trim to input length

The filter's FFTs are pre-computed at load time — `process()` pays only input FFT + multiply + IFFT per block.

Public API (`process`, `load_filter`, `set_bypass`, `is_enabled`) is unchanged.

### Validation

- Impulse response round-trip: convolve a sine wave through an identity FIR `[1.0, 0.0, ...]`, assert output matches input within f32 tolerance
- Performance test: 200k-tap FIR, 4,096-sample block must complete in < 5ms (`std::time::Instant`)

---

## Section 3: AudioOutput Trait + ALSA + PipeWire

### Trait Definition (`output/mod.rs`)

```rust
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("device not found: {0}")]  DeviceNotFound(String),
    #[error("write error: {0}")]       WriteError(String),
    #[error("config error: {0}")]      ConfigError(String),
}

pub trait AudioOutput: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn write(&mut self, samples: &[f32]) -> Result<(), OutputError>;
    fn close(self: Box<Self>);
}

pub fn open_output(
    target: OutputTarget,
    config: &DspConfig,
) -> Result<Box<dyn AudioOutput>, OutputError>
```

`open_output` is the single construction point. It tries PipeWire first when `target == OutputTarget::PipeWire`; if the PipeWire socket is unavailable, it automatically falls back to ALSA and logs a warning.

### ALSA Backend (`output/alsa.rs`)

- Opens `hw:{alsa_device}` (from `DspConfig.alsa_device`, defaulting to `"hw:0,0"`)
- Hardware params: `output_sample_rate`, S32LE format (fallback F32LE), 2 channels, period = `buffer_size`
- `write()`: converts `&[f32]` → interleaved S32LE bytes, calls `snd_pcm_writei`
- On `EPIPE` (underrun): calls `snd_pcm_prepare`, retries once, then returns `WriteError`
- `close()`: `snd_pcm_drain` → `snd_pcm_close`
- No OS mixer in the signal path — bit-perfect by construction

### PipeWire Backend (`output/pipewire.rs`)

- Creates a `pw::stream::Stream` with `MediaRole::Music` (default) or `MediaRole::ProAudio` (exclusive, controlled by `DspConfig.pipewire_role`)
- Negotiates F32LE at `output_sample_rate`, 2 channels
- PipeWire's realtime callback thread is decoupled from the tokio runtime via a bounded `crossbeam_channel` (capacity = 4 periods)
- `write()` enqueues to the channel; the PipeWire callback drains it
- Pro Audio role bypasses the PipeWire session manager's resampler, enabling bit-perfect output at the negotiated rate

### Pipeline Wiring (`mod.rs`)

```rust
pub struct DspPipeline {
    config:      Arc<RwLock<DspConfig>>,
    resampler:   Option<Resampler>,
    dsd:         Option<DsdConverter>,
    convolution: Option<ConvolutionEngine>,
    output:      Option<Box<dyn AudioOutput>>,  // new
}
```

`process()` ends with `out.write(&processed_samples)`.

`update_config()` checks if `output_target`, `alsa_device`, or `pipewire_role` changed — if so, closes the current output and opens a new one via `open_output`.

### New Config Fields

```rust
// DspConfig additions
pub alsa_device:    Option<String>,  // None → "hw:0,0"
pub pipewire_role:  String,          // "Music" | "Pro Audio"
```

### New Settings Entries (DSP Audio category in settings.go)

| Label | Key | Kind | Default |
|---|---|---|---|
| ALSA device | `dsp.alsa_device` | `settingPath` | `""` (hw:0,0) |
| PipeWire role | `dsp.pipewire_role` | `settingChoice` (Music / Pro Audio) | `Music` |

### Validation

- `cfg(test)` opens a `plug:null` ALSA device / dummy PipeWire context — no real hardware required in CI
- Integration test: `open_output` → write 4,096 silence samples → `close`, assert no error for both backends

---

## Error Handling

| Scenario | Behaviour |
|---|---|
| PipeWire socket missing | `open_output` falls back to ALSA, logs `warn!` |
| ALSA device not found | Returns `OutputError::DeviceNotFound`, DSP pipeline disables output and sets status msg |
| ALSA underrun | Retry once after `snd_pcm_prepare`; on second failure returns `WriteError` |
| Convolution filter load fails | `ConvolutionEngine` sets `enabled = false`, surfaces error via existing IPC error response |
| Rubato chunk size mismatch | Handled internally by padding/draining; never propagates to caller |

---

## Implementation Order

1. Add new Cargo dependencies
2. Rewrite `resample.rs` with rubato — all existing tests pass + new proptest
3. Rewrite `convolution.rs` with rustfft OLA — all existing tests pass + new perf test
4. Add `output/mod.rs` trait + `output/alsa.rs`
5. Replace `output/pipewire.rs` stub with real implementation
6. Wire `output` field into `DspPipeline` in `mod.rs`
7. Add new `DspConfig` fields + `apply_dsp_key` cases in `manager.rs`
8. Add new settings entries in `settings.go`

---

## Open Questions

None — all design decisions resolved during brainstorming.
