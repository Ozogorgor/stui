//! Spectral Amplitude Warping (SAW) noise-shaping / spectral-dynamics filter.
//!
//! Inspired by Lefebvre & Laflamme (1997) "Spectral amplitude warping for noise
//! spectrum shaping in audio coding".  Unlike classical error-feedback noise
//! shapers this filter operates directly on the audio-signal spectrum via STFT /
//! Overlap-Add (OLA), applying a frequency- and level-dependent magnitude warp:
//!
//! ```text
//!   warped_mag = (mag · w^(α−1))^α
//! ```
//!
//! where `w` is the per-bin IEC 61672-A weighting factor and `α ∈ (0,1)` is the
//! warp depth.  Because `α − 1 < 0`, bins where `w > 1` (i.e. the perceptually
//! sensitive 2–5 kHz region) receive *less* pre-warp magnitude boost, resulting
//! in reduced warping at those frequencies — the correct direction for
//! noise-shaping intent.
//!
//! `α` is adapted per-frame from three signals:
//!
//! - **Signal RMS**: quiet → deeper warp (`max_alpha`), loud → shallower (`min_alpha`)
//! - **Transient flag**: detected transients halve `α` to preserve attack character
//! - **Spectral entropy**: noise-like (flat) spectra use full `α`; tonal content
//!   reduces `α`, concentrating spectral dynamics where they are least audible
//!
//! # Algorithmic latency
//!
//! The OLA hop is [`HOP_SIZE`] (256) samples.  The first valid output sample
//! arrives after `FFT_SIZE − HOP_SIZE` = **256 samples** of input (one full
//! hop).  Query this via [`StftProcessor::latency_samples`].
//!
//! # Stereo use
//!
//! The rest of the DSP pipeline operates on interleaved stereo `&[f32]` slices.
//! Use [`StereoStftProcessor`] rather than driving a single [`StftProcessor`]
//! with interleaved data, which would corrupt the stereo image.

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

const FFT_SIZE: usize = 512;
const HOP_SIZE: usize = 256;

type C = Complex<f32>;

// ── SIMD detection ────────────────────────────────────────────────────────────

#[inline]
fn has_avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

// ── Spectral-node trait ───────────────────────────────────────────────────────

/// Trait for STFT-domain spectral processing nodes.
///
/// Named `SpectralNode` (not `DspNode`) to avoid collision with the
/// pipeline-level `DspNode` trait in `nodes/mod.rs`.
pub trait SpectralNode: Send + 'static {
    fn process_spectrum(&mut self, spectrum: &mut [C], frame_rms: f32, is_transient: bool);
}

// ── IEC 61672-A psychoacoustic weighting ─────────────────────────────────────

/// IEC 61672-A weighting in dB for `frequency_hz`.
///
/// This implements the IEC 61672 A-weighting curve.  It is *not* ISO 226
/// (equal-loudness contours, which are SPL-level-dependent).
fn a_weight_db(frequency_hz: f32) -> f32 {
    if frequency_hz < 20.0 {
        return -50.0;
    }
    let f2 = frequency_hz * frequency_hz;
    let ra = (12200.0_f32.powi(2) * f2 * f2)
        / ((f2 + 20.6_f32.powi(2))
            * (f2 + 12200.0_f32.powi(2))
            * (f2 + 107.7_f32.powi(2)).sqrt()
            * (f2 + 737.9_f32.powi(2)).sqrt());
    (20.0 * ra.log10() + 2.0).clamp(-50.0, 5.0)
}

#[inline]
fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Pre-compute IEC 61672-A linear weights for each FFT bin at `sample_rate`.
///
/// Weights are clamped to [0.2, 2.0] to prevent extreme modifications at the
/// band edges.
fn compute_a_weights(fft_size: usize, sample_rate: f32) -> Vec<f32> {
    let bin_width = sample_rate / fft_size as f32;
    (0..fft_size)
        .map(|k| db_to_linear(a_weight_db(k as f32 * bin_width)).clamp(0.2, 2.0))
        .collect()
}

// ── Spectral entropy ──────────────────────────────────────────────────────────

/// Normalised spectral entropy of the positive-frequency half-spectrum.
///
/// Returns `1.0` for flat (noise-like) spectra and approaches `0.0` for
/// strongly tonal content.  Silence is treated as maximum entropy so that a
/// silent frame does not suppress warping.
fn spectral_entropy(spectrum: &[C], half: usize) -> f32 {
    let total_power: f32 = spectrum[1..half].iter().map(|c| c.norm_sqr()).sum();
    if total_power < 1e-20 {
        return 1.0;
    }
    let entropy: f32 = spectrum[1..half]
        .iter()
        .map(|c| {
            let p = c.norm_sqr() / total_power;
            if p > 1e-15 {
                -p * p.ln()
            } else {
                0.0
            }
        })
        .sum();
    let max_entropy = ((half - 1) as f32).ln().max(1.0);
    (entropy / max_entropy).clamp(0.0, 1.0)
}

// ── Transient detection ───────────────────────────────────────────────────────

pub struct TransientDetector {
    energy_history: Vec<f32>,
    history_idx: usize,
    threshold_db: f32,
    /// Attack time in milliseconds (stored for inspection / future reconfiguration).
    #[allow(dead_code)] // planned: exposed for transient detector UI/debug inspection
    pub attack_time_ms: f32,
    /// Release time in milliseconds.
    #[allow(dead_code)] // planned: exposed for transient detector UI/debug inspection
    pub release_time_ms: f32,
    /// One-pole attack coefficient, derived from `attack_time_ms` and the hop rate.
    attack_alpha: f32,
    /// One-pole release coefficient, derived from `release_time_ms` and the hop rate.
    release_alpha: f32,
    envelope: f32,
}

impl TransientDetector {
    /// Create a new detector.
    ///
    /// `sample_rate` is the audio sample rate in Hz.  Attack and release
    /// coefficients are computed from the physical time constants (5 ms attack,
    /// 50 ms release) and the hop rate (`sample_rate / HOP_SIZE` frames/s).
    pub fn new(sample_rate: f32) -> Self {
        let attack_time_ms = 5.0_f32;
        let release_time_ms = 50.0_f32;
        // Frames per second at which `process` is called.
        let hop_rate = sample_rate / HOP_SIZE as f32;
        // One-pole IIR coefficient: α = 1 − exp(−1 / (time_s · hop_rate))
        let attack_alpha = 1.0 - (-1000.0 / (attack_time_ms * hop_rate)).exp();
        let release_alpha = 1.0 - (-1000.0 / (release_time_ms * hop_rate)).exp();
        Self {
            energy_history: vec![0.0; 32],
            history_idx: 0,
            threshold_db: 12.0,
            attack_time_ms,
            release_time_ms,
            attack_alpha,
            release_alpha,
            envelope: 0.0,
        }
    }

    /// Feed one frame RMS; returns `true` if a transient is detected.
    pub fn process(&mut self, rms: f32) -> bool {
        self.energy_history[self.history_idx] = rms;
        self.history_idx = (self.history_idx + 1) % self.energy_history.len();

        let avg: f32 = self.energy_history.iter().sum::<f32>() / self.energy_history.len() as f32;

        let delta = rms - self.envelope;
        if delta > 0.0 {
            self.envelope += delta * self.attack_alpha;
        } else {
            self.envelope += delta * self.release_alpha;
        }

        let ratio = if avg > 1e-10 { rms / avg } else { 0.0 };
        let db_above = if ratio > 0.0 {
            20.0 * ratio.log10()
        } else {
            0.0
        };
        db_above > self.threshold_db
    }
}

// ── Adaptive SAW spectral node ────────────────────────────────────────────────

pub struct SawNode {
    min_alpha: f32,
    max_alpha: f32,
    rms_threshold_db: f32,
    use_simd: bool,
    /// Pre-computed IEC 61672-A linear weights, one per FFT bin.
    a_weights: Vec<f32>,
}

impl SawNode {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            min_alpha: 0.4,
            max_alpha: 0.8,
            rms_threshold_db: -40.0,
            use_simd: has_avx2(),
            a_weights: compute_a_weights(FFT_SIZE, sample_rate),
        }
    }

    /// Smooth RMS-to-alpha mapping: quiet → `max_alpha`, loud → `min_alpha`.
    fn adaptive_alpha(&self, rms: f32) -> f32 {
        let rms_db = if rms > 1e-10 {
            20.0 * rms.log10()
        } else {
            -100.0
        };
        let lo = self.rms_threshold_db - 30.0;
        let t = (rms_db - lo).clamp(0.0, 60.0) / 60.0;
        self.max_alpha - t * (self.max_alpha - self.min_alpha)
    }
}

impl SpectralNode for SawNode {
    fn process_spectrum(&mut self, spectrum: &mut [C], frame_rms: f32, is_transient: bool) {
        let n = spectrum.len();
        let half = n / 2;

        // Base warp depth from RMS.
        let mut alpha = self.adaptive_alpha(frame_rms);

        // Transient: halve warp depth to preserve attack character.
        if is_transient {
            alpha *= 0.5;
        }

        // Entropy modulation: noise-like spectra receive full warp; tonal
        // content reduces warp depth, preserving harmonic structure.
        let entropy = spectral_entropy(spectrum, half);
        alpha *= 0.7 + 0.3 * entropy;

        let a_weights = &self.a_weights;

        // Dispatch: try SIMD on x86_64 (returns early), fall through to scalar.
        #[cfg(target_arch = "x86_64")]
        if self.use_simd {
            unsafe {
                saw_warp_simd(
                    spectrum,
                    alpha,
                    |k: usize| {
                        if k < half {
                            a_weights[k]
                        } else {
                            1.0
                        }
                    },
                );
            }
            return;
        }

        saw_warp_scalar(spectrum, half, alpha, |k: usize| {
            if k < half {
                a_weights[k]
            } else {
                1.0
            }
        });
    }
}

// ── Scalar SAW warp ───────────────────────────────────────────────────────────

/// Apply the SAW warp to the positive-frequency bins of `spectrum`.
///
/// The warp formula is:
/// ```text
///   warped_mag = (mag · w^(α−1))^α
/// ```
/// Since `α − 1 < 0`, bins where `w > 1` (perceptually sensitive frequencies)
/// get a *reduced* pre-warp magnitude, resulting in less spectral warping at
/// those frequencies.
fn saw_warp_scalar<F: Fn(usize) -> f32>(spectrum: &mut [C], half: usize, alpha: f32, weight_fn: F) {
    let n = spectrum.len();
    for k in 1..half {
        let c = spectrum[k];
        let mag = c.norm();
        if mag > 1e-10 {
            let w = weight_fn(k);
            let weighted_mag = mag * w.powf(alpha - 1.0);
            let warped = weighted_mag.powf(alpha);
            let new = C::from_polar(warped, c.arg());
            spectrum[k] = new;
            spectrum[n - k] = new.conj();
        }
    }
}

// ── SIMD SAW warp (AVX2) ──────────────────────────────────────────────────────
//
// AVX2 has no native `powf` instruction, so the per-bin warp computation is
// still scalar.  SIMD is used for magnitude calculation and the final
// phase-preserving scale-back, both of which are fully vectorisable.

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn saw_warp_simd<F>(spectrum: &mut [C], alpha: f32, weight_fn: F)
where
    F: Fn(usize) -> f32,
{
    use std::arch::x86_64::*;

    let n = spectrum.len();
    let half = n / 2;
    // Process AVX2 chunks of 8 bins. The last SIMD iteration at index i processes
    // bins i..i+7 (inclusive), so simd_limit must be half-7 to avoid including
    // bin (half-8) in both the SIMD loop and the scalar tail.
    let simd_limit = if half > 8 { half - 7 } else { 1 };

    for i in (1..simd_limit).step_by(8) {
        // Gather real and imaginary parts into contiguous arrays for SIMD load.
        let mut re = [0f32; 8];
        let mut im = [0f32; 8];
        for j in 0..8 {
            re[j] = spectrum[i + j].re;
            im[j] = spectrum[i + j].im;
        }

        let r = _mm256_loadu_ps(re.as_ptr());
        let imv = _mm256_loadu_ps(im.as_ptr());

        // SIMD magnitude: sqrt(re² + im²)
        let r2 = _mm256_mul_ps(r, r);
        let i2 = _mm256_mul_ps(imv, imv);
        let mag_v = _mm256_sqrt_ps(_mm256_add_ps(r2, i2));

        let mut mag_arr = [0f32; 8];
        _mm256_storeu_ps(mag_arr.as_mut_ptr(), mag_v);

        // Scalar powf for the warp — no AVX2 equivalent.
        let mut warped_arr = [0f32; 8];
        for j in 0..8 {
            let mag = mag_arr[j];
            if mag > 1e-10 {
                let w = weight_fn(i + j);
                let weighted = mag * w.powf(alpha - 1.0);
                warped_arr[j] = weighted.powf(alpha);
            }
            // else stays 0.0 — suppress near-silence bins
        }

        // SIMD scale-back: multiply (re, im) by warped/mag to preserve phase.
        let warped_v = _mm256_loadu_ps(warped_arr.as_ptr());
        let inv = _mm256_div_ps(warped_v, _mm256_add_ps(mag_v, _mm256_set1_ps(1e-12)));
        let new_r = _mm256_mul_ps(r, inv);
        let new_i = _mm256_mul_ps(imv, inv);

        let mut out_r = [0f32; 8];
        let mut out_i = [0f32; 8];
        _mm256_storeu_ps(out_r.as_mut_ptr(), new_r);
        _mm256_storeu_ps(out_i.as_mut_ptr(), new_i);

        for j in 0..8 {
            let c = C::new(out_r[j], out_i[j]);
            spectrum[i + j] = c;
            spectrum[n - (i + j)] = c.conj();
        }
    }

    // Scalar tail: bins simd_limit..half.
    for k in simd_limit..half {
        let c = spectrum[k];
        let mag = c.norm();
        if mag > 1e-10 {
            let w = weight_fn(k);
            let weighted = mag * w.powf(alpha - 1.0);
            let warped = weighted.powf(alpha);
            let new = C::from_polar(warped, c.arg());
            spectrum[k] = new;
            spectrum[n - k] = new.conj();
        }
    }
}

// ── Mono STFT processor ───────────────────────────────────────────────────────

/// Single-channel STFT/OLA processor.
///
/// For stereo operation use [`StereoStftProcessor`] rather than feeding
/// interleaved samples here.
pub struct StftProcessor {
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,

    input_ring: Vec<f32>,
    spectrum: Vec<C>,
    ola: Vec<f32>,
    window: Vec<f32>,

    write_pos: usize,
    ola_read_idx: usize,
    hop_counter: usize,

    dsp: Vec<Box<dyn SpectralNode>>,
    transient_detector: TransientDetector,

    use_simd: bool,
}

impl StftProcessor {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let ifft = planner.plan_fft_inverse(FFT_SIZE);

        // Sine window — satisfies the Princen-Bradley perfect-reconstruction
        // condition when the OLA step equals FFT_SIZE / 2.
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| (std::f32::consts::PI * i as f32 / FFT_SIZE as f32).sin())
            .collect();

        Self {
            fft,
            ifft,
            input_ring: vec![0.0; FFT_SIZE],
            spectrum: vec![C::new(0.0, 0.0); FFT_SIZE],
            ola: vec![0.0; FFT_SIZE],
            window,
            write_pos: 0,
            ola_read_idx: 0,
            hop_counter: 0,
            dsp: Vec::new(),
            transient_detector: TransientDetector::new(sample_rate),
            use_simd: has_avx2(),
        }
    }

    pub fn add_dsp(&mut self, node: Box<dyn SpectralNode>) {
        self.dsp.push(node);
    }

    /// Algorithmic latency introduced by the OLA processing, in samples.
    ///
    /// The pipeline should compensate for this delay when synchronising audio
    /// with video frames, lyrics, or other timed events.
    #[allow(dead_code)] // planned: used for A/V sync compensation in pipeline
    pub fn latency_samples() -> usize {
        FFT_SIZE - HOP_SIZE
    }

    fn calculate_rms(&self) -> f32 {
        let sum: f32 = (0..FFT_SIZE)
            .map(|i| {
                let s = self.input_ring[(self.write_pos + i) % FFT_SIZE];
                s * s
            })
            .sum();
        (sum / FFT_SIZE as f32).sqrt()
    }

    /// Process one input sample; returns the corresponding output sample.
    ///
    /// The first [`Self::latency_samples`] output samples will be zero while
    /// the OLA buffer fills.
    pub fn process(&mut self, input: f32) -> f32 {
        self.input_ring[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) % FFT_SIZE;

        let out = self.ola[self.ola_read_idx];
        self.ola[self.ola_read_idx] = 0.0;
        self.ola_read_idx = (self.ola_read_idx + 1) % FFT_SIZE;

        self.hop_counter += 1;

        if self.hop_counter >= HOP_SIZE {
            self.hop_counter = 0;

            let rms = self.calculate_rms();
            let is_transient = self.transient_detector.process(rms);

            // Windowing
            if self.use_simd {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    window_gather_simd(
                        &self.input_ring,
                        &self.window,
                        &mut self.spectrum,
                        self.write_pos,
                    );
                }
                #[cfg(not(target_arch = "x86_64"))]
                self.apply_window_scalar();
            } else {
                self.apply_window_scalar();
            }

            self.fft.process(&mut self.spectrum);

            for node in &mut self.dsp {
                node.process_spectrum(&mut self.spectrum, rms, is_transient);
            }

            self.ifft.process(&mut self.spectrum);

            let scale = 1.0 / FFT_SIZE as f32;

            // OLA accumulation
            if self.use_simd {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    ola_simd_offset(
                        &mut self.ola,
                        &self.spectrum,
                        &self.window,
                        scale,
                        self.ola_read_idx,
                    );
                }
                #[cfg(not(target_arch = "x86_64"))]
                self.ola_accumulate_scalar(scale);
            } else {
                self.ola_accumulate_scalar(scale);
            }
        }

        out
    }

    fn apply_window_scalar(&mut self) {
        for i in 0..FFT_SIZE {
            let idx = (self.write_pos + i) % FFT_SIZE;
            self.spectrum[i] = C::new(self.input_ring[idx] * self.window[i], 0.0);
        }
    }

    fn ola_accumulate_scalar(&mut self, scale: f32) {
        for i in 0..FFT_SIZE {
            let idx = (self.ola_read_idx + i) % FFT_SIZE;
            self.ola[idx] += self.spectrum[i].re * scale * self.window[i];
        }
    }
}

// ── Stereo STFT processor ─────────────────────────────────────────────────────

/// Stereo wrapper: two independent [`StftProcessor`] instances, one per channel.
///
/// Use this when integrating with the DSP pipeline, which operates on
/// interleaved stereo `&[f32]` slices.
#[allow(dead_code)] // planned: stereo noise-shaping via SAW, integrated into DitherNode
pub struct StereoStftProcessor {
    left: StftProcessor,
    right: StftProcessor,
}

#[allow(dead_code)] // planned: stereo noise-shaping via SAW, integrated into DitherNode
impl StereoStftProcessor {
    /// Create a stereo processor with a default [`SawNode`] (α = 0.6) on each channel.
    pub fn new(sample_rate: f32) -> Self {
        let mut left = StftProcessor::new(sample_rate);
        let mut right = StftProcessor::new(sample_rate);
        left.add_dsp(Box::new(SawNode::new(sample_rate)));
        right.add_dsp(Box::new(SawNode::new(sample_rate)));
        Self { left, right }
    }

    /// Process one stereo sample pair; returns `(left_out, right_out)`.
    pub fn process_frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.left.process(left), self.right.process(right))
    }

    /// Process an interleaved stereo buffer in-place (`L R L R …`).
    ///
    /// `samples` must have even length.
    pub fn process_interleaved(&mut self, samples: &mut [f32]) {
        debug_assert!(
            samples.len() % 2 == 0,
            "process_interleaved requires even-length buffer, got {}",
            samples.len()
        );
        for chunk in samples.chunks_exact_mut(2) {
            let (l, r) = self.process_frame(chunk[0], chunk[1]);
            chunk[0] = l;
            chunk[1] = r;
        }
    }

    /// Algorithmic latency in samples (same as mono).
    pub fn latency_samples() -> usize {
        StftProcessor::latency_samples()
    }
}

// ── SIMD helpers ──────────────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn window_gather_simd(input: &[f32], window: &[f32], output: &mut [C], write_pos: usize) {
    use std::arch::x86_64::*;

    for i in (0..FFT_SIZE).step_by(8) {
        let mut tmp = [0f32; 8];
        for j in 0..8 {
            tmp[j] = input[(write_pos + i + j) % FFT_SIZE];
        }
        let x = _mm256_loadu_ps(tmp.as_ptr());
        let w = _mm256_loadu_ps(window.as_ptr().add(i));
        let y = _mm256_mul_ps(x, w);
        let mut out = [0f32; 8];
        _mm256_storeu_ps(out.as_mut_ptr(), y);
        for j in 0..8 {
            output[i + j] = C::new(out[j], 0.0);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn ola_simd_offset(
    ola: &mut [f32],
    spectrum: &[C],
    window: &[f32],
    scale: f32,
    offset: usize,
) {
    use std::arch::x86_64::*;

    let scale_v = _mm256_set1_ps(scale);
    let n = spectrum.len();
    let fft_size = FFT_SIZE;
    let limit = n.saturating_sub(1);

    for i in (0..=limit).step_by(8) {
        let idx = (offset + i) % fft_size;
        if idx + 8 <= fft_size && i + 8 <= n {
            // No wrap-around in either buffer for this 8-element chunk.
            let mut real = [0f32; 8];
            let mut win = [0f32; 8];
            for j in 0..8 {
                real[j] = spectrum[i + j].re;
                win[j] = window[i + j];
            }
            let x = _mm256_loadu_ps(real.as_ptr());
            let w = _mm256_loadu_ps(win.as_ptr());
            let y = _mm256_mul_ps(_mm256_mul_ps(x, w), scale_v);
            let dst = _mm256_loadu_ps(ola.as_ptr().add(idx));
            _mm256_storeu_ps(ola.as_mut_ptr().add(idx), _mm256_add_ps(dst, y));
        } else {
            // Scalar fallback handles buffer wrap-around correctly.
            let remaining = std::cmp::min(8, n.saturating_sub(i));
            for j in 0..remaining {
                let idx = (offset + i + j) % fft_size;
                ola[idx] += spectrum[i + j].re * window[i + j] * scale;
            }
        }
    }
}
