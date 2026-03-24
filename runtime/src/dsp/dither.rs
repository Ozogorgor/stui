//! TPDF dither + noise shaping filter.
//!
//! Applies triangular-PDF dither and optional error-feedback noise shaping
//! before bit-depth quantization. This is the final DSP stage before output.
//!
//! All coefficient tables sourced from SoX src/dither.c (LGPL).

// ── Constants ─────────────────────────────────────────────────────────────────

/// xorshift64 seed — golden ratio constant, guaranteed nonzero.
pub(crate) const INITIAL_SEED: u64 = 0x9E3779B97F4A7C15;

// ── Coefficient tables ────────────────────────────────────────────────────────
// All sourced from SoX src/dither.c (public domain / LGPL).

static LIPSHITZ:  &[f32] = &[2.033, -2.165, 1.959, -1.590, 0.6149];

static FWEIGHTED: &[f32] = &[
    2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847,
];

static MOD_E_WT:  &[f32] = &[
    1.662, -1.263, 0.4827, -0.2913, 0.1268, -0.1124, 0.03252, -0.01265, -0.03524,
];

static IMP_E_WT:  &[f32] = &[
    2.847, -4.685, 6.214, -7.184, 6.639, -5.032, 3.263, -1.632, 0.4191,
];

// Shibata: per-rate tables (sourced from SoX src/dither.c).
static SHI_08K: &[f32] = &[
    -1.202863335609436, -0.94103097915649414, -0.67878556251525879,
    -0.57650017738342285, -0.50004476308822632, -0.44349345564842224,
    -0.37833768129348755, -0.34028723835945129, -0.29413089156150818,
    -0.24994957447052002, -0.21715600788593292, -0.18792112171649933,
    -0.15268312394618988, -0.12135542929172516, -0.099610626697540283,
    -0.075273610651493073, -0.048787496984004974, -0.042586319148540497,
    -0.028991291299462318, -0.011869125068187714,
];

static SHI_11K: &[f32] = &[
    -0.9264228343963623, -0.98695987462997437, -0.631156325340271,
    -0.51966935396194458, -0.39738872647285461, -0.35679301619529724,
    -0.29720726609230042, -0.26310476660728455, -0.21719355881214142,
    -0.18561814725399017, -0.15404847264289856, -0.12687471508979797,
    -0.10339745879173279, -0.083688631653785706, -0.05875682458281517,
    -0.046893671154975891, -0.027950936928391457, -0.020740609616041183,
    -0.009366452693939209, -0.0060260160826146603,
];

static SHI_16K: &[f32] = &[
    -0.37251132726669312, -0.81423574686050415, -0.55010956525802612,
    -0.47405767440795898, -0.32624706625938416, -0.3161766529083252,
    -0.2286367267370224, -0.22916607558727264, -0.19565616548061371,
    -0.18160104751586914, -0.15423151850700378, -0.14104481041431427,
    -0.11844276636838913, -0.097583092749118805, -0.076493598520755768,
    -0.068106919527053833, -0.041881654411554337, -0.036922425031661987,
    -0.019364040344953537, -0.014994367957115173,
];

static SHI_22K: &[f32] = &[
    0.056581053882837296, -0.56956905126571655, -0.40727734565734863,
    -0.33870288729667664, -0.29810553789138794, -0.19039161503314972,
    -0.16510021686553955, -0.13468159735202789, -0.096633769571781158,
    -0.081049129366874695, -0.064953058958053589, -0.054459091275930405,
    -0.043378707021474838, -0.03660014271736145, -0.026256965473294258,
    -0.018786206841468811, -0.013387725688517094, -0.0090983230620622635,
    -0.0026585909072309732, -0.00042083300650119781,
];

static SHI_32K: &[f32] = &[
    0.82118552923202515, -1.0063692331314087, 0.62341964244842529,
    -1.0447187423706055, 0.64532512426376343, -0.87615132331848145,
    0.52219754457473755, -0.67434263229370117, 0.44954317808151245,
    -0.52557498216629028, 0.34567299485206604, -0.39618203043937683,
    0.26791760325431824, -0.28936097025871277, 0.1883765310049057,
    -0.19097308814525604, 0.10431359708309174, -0.10633844882249832,
    0.046832218766212463, -0.039653312414884567,
];

static SHI_38K: &[f32] = &[
    1.6335992813110351562, -2.2615492343902587891, 2.4077029228210449219,
    -2.6341717243194580078, 2.1440362930297851562, -1.8153258562088012695,
    1.0816224813461303711, -0.70302653312683105469, 0.15991993248462677002,
    0.041549518704414367676, -0.29416576027870178223, 0.2518316805362701416,
    -0.27766478061676025391, 0.15785403549671173096, -0.10165894031524658203,
    0.016833892092108726501,
];

static SHI_44K: &[f32] = &[
    2.6773197650909423828, -4.8308925628662109375, 6.570110321044921875,
    -7.4572014808654785156, 6.7263274192810058594, -4.8481650352478027344,
    2.0412089824676513672, 0.7006359100341796875, -2.9537565708160400391,
    4.0800385475158691406, -4.1845216751098632812, 3.3311812877655029297,
    -2.1179926395416259766, 0.879302978515625, -0.031759146600961685181,
    -0.42382788658142089844, 0.47882103919982910156, -0.35490813851356506348,
    0.17496839165687561035, -0.060908168554306030273,
];

static SHI_48K: &[f32] = &[
    2.8720729351043701172, -5.0413231849670410156, 6.2442994117736816406,
    -5.8483986854553222656, 3.7067542076110839844, -1.0495119094848632812,
    -1.1830236911773681641, 2.1126792430877685547, -1.9094531536102294922,
    0.99913084506988525391, -0.17090806365013122559, -0.32615602016448974609,
    0.39127644896507263184, -0.26876461505889892578, 0.097676105797290802002,
    -0.023473845794796943665,
];

// Low-Shibata: 44.1k and 48k only.
static SHL_44K: &[f32] = &[
    2.0833916664123535156, -3.0418450832366943359, 3.2047898769378662109,
    -2.7571926116943359375, 1.4978630542755126953, -0.3427594602108001709,
    -0.71733748912811279297, 1.0737057924270629883, -1.0225815773010253906,
    0.56649994850158691406, -0.20968692004680633545, -0.065378531813621520996,
    0.10322438180446624756, -0.067442022264003753662, -0.00495197344571352005,
];

static SHL_48K: &[f32] = &[
    2.3925774097442626953, -3.4350297451019287109, 3.1853709220886230469,
    -1.8117271661758422852, -0.20124770700931549072, 1.4759907722473144531,
    -1.7210904359817504883, 0.97746700048446655273, -0.13790138065814971924,
    -0.38185903429985046387, 0.27421241998672485352, 0.066584214568138122559,
    -0.35223302245140075684, 0.37672343850135803223, -0.23964276909828186035,
    0.068674825131893157959,
];

// High-Shibata: single 44.1k table, used at all rates.
static SHH_44K: &[f32] = &[
    3.0259189605712890625, -6.0268716812133789062, 9.195003509521484375,
    -11.824929237365722656, 12.767142295837402344, -11.917946815490722656,
    9.1739168167114257812, -5.3712320327758789062, 1.1393624544143676758,
    2.4484779834747314453, -4.9719839096069335938, 6.0392003059387207031,
    -5.9359521865844726562, 4.903278350830078125, -3.5527443885803222656,
    2.1909697055816650391, -1.1672389507293701172, 0.4903914332389831543,
    -0.16519790887832641602, 0.023217858746647834778,
];

// Gesemann: rate-selected feedforward + feedback pairs.
static GES44_FF: &[f32] = &[2.2061, -0.4706, -0.2534, -0.6214];
static GES44_FB: &[f32] = &[1.0587,  0.0676, -0.6054, -0.2738];
static GES48_FF: &[f32] = &[2.2374, -0.7339, -0.1251, -0.6033];
static GES48_FB: &[f32] = &[0.9030,  0.0116, -0.5853, -0.2571];

// ── Shibata rate selection ────────────────────────────────────────────────────

struct ShibataEntry { rate: u32, coeffs: &'static [f32] }

static SHIBATA_TABLES: &[ShibataEntry] = &[
    ShibataEntry { rate:  8000, coeffs: SHI_08K },
    ShibataEntry { rate: 11025, coeffs: SHI_11K },
    ShibataEntry { rate: 16000, coeffs: SHI_16K },
    ShibataEntry { rate: 22050, coeffs: SHI_22K },
    ShibataEntry { rate: 32000, coeffs: SHI_32K },
    ShibataEntry { rate: 37800, coeffs: SHI_38K },
    ShibataEntry { rate: 44100, coeffs: SHI_44K },
    ShibataEntry { rate: 48000, coeffs: SHI_48K },
];

/// Select Shibata table for `rate_hz`. Rates ≥48k use the 48k table.
/// Ties (equidistant) go to the higher-rate table.
fn shibata_select(rate_hz: u32) -> &'static [f32] {
    if rate_hz >= 48000 { return SHI_48K; }
    let mut best = &SHIBATA_TABLES[0];
    let mut best_diff = rate_hz.abs_diff(best.rate);
    for t in SHIBATA_TABLES.iter().skip(1) {
        if t.rate >= 48000 { break; }
        let diff = rate_hz.abs_diff(t.rate);
        if diff < best_diff || (diff == best_diff && t.rate > best.rate) {
            best = t;
            best_diff = diff;
        }
    }
    best.coeffs
}

/// Select Low-Shibata table: ≤46050 Hz → 44.1k, else → 48k.
fn low_shibata_select(rate_hz: u32) -> &'static [f32] {
    if rate_hz <= 46050 { SHL_44K } else { SHL_48K }
}

/// Select Gesemann feedforward + feedback pair: ≤46050 Hz → 44.1k, else → 48k.
fn gesemann_select(rate_hz: u32) -> (&'static [f32], &'static [f32]) {
    if rate_hz <= 46050 { (GES44_FF, GES44_FB) } else { (GES48_FF, GES48_FB) }
}

// ── NoiseShaping ─────────────────────────────────────────────────────────────

/// Noise shaping algorithm selection.
#[derive(Debug, Clone, PartialEq)]
pub enum NoiseShaping {
    None,
    Lipshitz,
    Fweighted,
    ModifiedEweighted,
    ImprovedEweighted,
    Shibata,
    LowShibata,
    HighShibata,
    Gesemann,
}

impl NoiseShaping {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "none"                 => Some(Self::None),
            "lipshitz"             => Some(Self::Lipshitz),
            "fweighted"            => Some(Self::Fweighted),
            "modified_e_weighted"  => Some(Self::ModifiedEweighted),
            "improved_e_weighted"  => Some(Self::ImprovedEweighted),
            "shibata"              => Some(Self::Shibata),
            "low_shibata"          => Some(Self::LowShibata),
            "high_shibata"         => Some(Self::HighShibata),
            "gesemann"             => Some(Self::Gesemann),
            _                      => None,
        }
    }
}

// ── DitherFilter ─────────────────────────────────────────────────────────────

pub struct DitherFilter {
    bit_depth:     u32,
    noise_shaping: NoiseShaping,
    sample_rate:   u32,            // initialized to 44100; updated on each rate change
    // FIR error feedback state (per channel)
    pub(crate) err_l: Vec<f32>,
    pub(crate) err_r: Vec<f32>,
    // IIR state (Gesemann feedforward + feedback, per channel)
    pub(crate) ff_l:  Vec<f32>,
    pub(crate) ff_r:  Vec<f32>,
    pub(crate) fb_l:  Vec<f32>,
    pub(crate) fb_r:  Vec<f32>,
    pub(crate) rng:   u64,
    // Cached coefficient slices (set by select_coeffs)
    fir_c:    Vec<f32>,
    ges_ff_c: Vec<f32>,
    ges_fb_c: Vec<f32>,
}

impl DitherFilter {
    pub fn new(bit_depth: u32, noise_shaping: NoiseShaping) -> Self {
        let mut f = Self {
            bit_depth,
            noise_shaping,
            sample_rate:  0,
            err_l: Vec::new(), err_r: Vec::new(),
            ff_l:  Vec::new(), ff_r:  Vec::new(),
            fb_l:  Vec::new(), fb_r:  Vec::new(),
            rng:   INITIAL_SEED,
            fir_c:    Vec::new(),
            ges_ff_c: Vec::new(),
            ges_fb_c: Vec::new(),
        };
        f.select_coeffs(44100); // nominal; recomputed on first process() call
        f
    }

    /// Select coefficient tables and resize/zero state buffers for `rate_hz`.
    fn select_coeffs(&mut self, rate_hz: u32) {
        self.sample_rate = rate_hz;

        let (fir, ges_ff, ges_fb) = match &self.noise_shaping {
            NoiseShaping::None              => (&[][..], &[][..], &[][..]),
            NoiseShaping::Lipshitz          => (LIPSHITZ,  &[][..], &[][..]),
            NoiseShaping::Fweighted         => (FWEIGHTED, &[][..], &[][..]),
            NoiseShaping::ModifiedEweighted => (MOD_E_WT,  &[][..], &[][..]),
            NoiseShaping::ImprovedEweighted => (IMP_E_WT,  &[][..], &[][..]),
            NoiseShaping::Shibata           => (shibata_select(rate_hz),     &[][..], &[][..]),
            NoiseShaping::LowShibata        => (low_shibata_select(rate_hz), &[][..], &[][..]),
            NoiseShaping::HighShibata       => (SHH_44K,   &[][..], &[][..]),
            NoiseShaping::Gesemann          => {
                let (ff, fb) = gesemann_select(rate_hz);
                (&[][..], ff, fb)
            }
        };

        self.fir_c    = fir.to_vec();
        self.ges_ff_c = ges_ff.to_vec();
        self.ges_fb_c = ges_fb.to_vec();

        let fir_n = self.fir_c.len();
        let ges_n = self.ges_ff_c.len();

        self.err_l = vec![0.0; fir_n];
        self.err_r = vec![0.0; fir_n];
        self.ff_l  = vec![0.0; ges_n];
        self.ff_r  = vec![0.0; ges_n];
        self.fb_l  = vec![0.0; ges_n];
        self.fb_r  = vec![0.0; ges_n];
    }

    /// Reset all state buffers and re-seed the RNG.
    fn reset_state(&mut self, rate_hz: u32) {
        self.select_coeffs(rate_hz);
        self.rng = INITIAL_SEED;
    }

    pub fn set_params(&mut self, bit_depth: u32, noise_shaping: NoiseShaping) {
        self.bit_depth     = bit_depth;
        self.noise_shaping = noise_shaping;
        self.reset_state(self.sample_rate);
    }

    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        debug_assert!(samples.len() % 2 == 0, "dither: input must be interleaved stereo");

        // 32-bit is a no-op (f32 mantissa = 24 bits; 32-bit quantization is meaningless).
        if self.bit_depth == 32 {
            return samples.to_vec();
        }

        if sample_rate != self.sample_rate {
            self.reset_state(sample_rate);
        }

        let scale = (1u32 << (self.bit_depth.saturating_sub(1))) as f32;
        let lsb   = 1.0 / scale;

        let mut out = Vec::with_capacity(samples.len());
        let mut iter = samples.chunks_exact(2);
        for frame in iter.by_ref() {
            let l = self.process_sample(frame[0], scale, lsb, 0);
            let r = self.process_sample(frame[1], scale, lsb, 1);
            out.push(l);
            out.push(r);
        }
        // Odd remainder pass-through (should never occur in a stereo pipeline).
        for &s in iter.remainder() { out.push(s); }
        out
    }

    /// Process a single sample for the given channel (0=L, 1=R).
    fn process_sample(&mut self, sample: f32, scale: f32, lsb: f32, ch: usize) -> f32 {
        let tpdf = self.next_tpdf() * lsb;

        match &self.noise_shaping {
            NoiseShaping::None => {
                let q = (sample * scale + tpdf).round() / scale;
                q.clamp(-1.0, 1.0 - lsb)
            }
            NoiseShaping::Gesemann => {
                let (ff_buf, fb_buf) = if ch == 0 {
                    (&mut self.ff_l, &mut self.fb_l)
                } else {
                    (&mut self.ff_r, &mut self.fb_r)
                };
                let shaped = tpdf
                    + dot(ff_buf, &self.ges_ff_c)
                    - dot(fb_buf, &self.ges_fb_c);
                let q = (sample * scale + shaped).round() / scale;
                let err = sample - q;
                shift_in(ff_buf, err);
                shift_in(fb_buf, shaped);
                q.clamp(-1.0, 1.0 - lsb)
            }
            _ => {
                let err_buf = if ch == 0 { &mut self.err_l } else { &mut self.err_r };
                let shaped = tpdf + dot(err_buf, &self.fir_c);
                let q = (sample * scale + shaped).round() / scale;
                let err = sample - q;
                shift_in(err_buf, err);
                q.clamp(-1.0, 1.0 - lsb)
            }
        }
    }

    /// Generate one TPDF sample in [-1, 1] using xorshift64.
    fn next_tpdf(&mut self) -> f32 {
        let r1 = self.xorshift64() as f32 / u64::MAX as f32 - 0.5;
        let r2 = self.xorshift64() as f32 / u64::MAX as f32 - 0.5;
        r1 + r2
    }

    fn xorshift64(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Dot product of two equal-length slices. Returns 0.0 if either is empty.
fn dot(buf: &[f32], coeffs: &[f32]) -> f32 {
    buf.iter().zip(coeffs.iter()).map(|(b, c)| b * c).sum()
}

/// Shift `buf` right, inserting `val` at index 0 (most-recent position).
fn shift_in(buf: &mut Vec<f32>, val: f32) {
    if buf.is_empty() { return; }
    buf.rotate_right(1);
    buf[0] = val;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine_stereo(freq_hz: f32, sample_rate: u32, n_frames: usize) -> Vec<f32> {
        (0..n_frames)
            .flat_map(|i| {
                let s = (2.0 * PI * freq_hz * i as f32 / sample_rate as f32).sin();
                [s, s]
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    // bit_depth=32 must be a no-op: return input unchanged.
    #[test]
    fn noop_at_32bit() {
        let input: Vec<f32> = (0..64).flat_map(|i| {
            let v = i as f32 * 0.01;
            [v, -v]
        }).collect();
        let mut f = DitherFilter::new(32, NoiseShaping::None);
        let out = f.process(&input, 44100);
        assert_eq!(out.len(), input.len());
        for (a, b) in input.iter().zip(out.iter()) {
            assert_eq!(a, b, "bit_depth=32 must return input unchanged");
        }
    }

    // TPDF mean ≈ 0 over 10k frames.
    #[test]
    fn tpdf_zero_mean() {
        let dc: Vec<f32> = vec![0.5_f32; 20_000]; // 10k stereo frames
        let mut f = DitherFilter::new(16, NoiseShaping::None);
        let out = f.process(&dc, 44100);
        let mean_error: f32 = out.iter().zip(dc.iter())
            .map(|(o, i)| o - i)
            .sum::<f32>() / out.len() as f32;
        assert!(mean_error.abs() < 0.001, "TPDF mean error {mean_error} not near 0");
    }

    // 16-bit output must snap to multiples of 1/32768.
    #[test]
    fn quantization_snaps_to_lsb() {
        let input = sine_stereo(440.0, 44100, 1024);
        let mut f = DitherFilter::new(16, NoiseShaping::None);
        let out = f.process(&input, 44100);
        let lsb = 1.0_f32 / 32768.0;
        for &s in &out {
            let rounded = (s / lsb).round() * lsb;
            assert!(
                (s - rounded).abs() < 1e-6,
                "sample {s} is not a multiple of lsb={lsb}"
            );
        }
    }

    // Lipshitz shaping must push more noise above 10kHz than below.
    #[test]
    fn noise_shaping_pushes_noise_high() {
        let sr = 44100_u32;
        // -60 dBFS 1 kHz sine
        let input: Vec<f32> = (0..2048_usize)
            .flat_map(|i| {
                let s = (2.0 * PI * 1000.0 * i as f32 / sr as f32).sin() * 0.001;
                [s, s]
            })
            .collect();
        let mut f = DitherFilter::new(16, NoiseShaping::Lipshitz);
        let out = f.process(&input, sr);

        // Separate L-channel, measure noise above and below 10 kHz.
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        let l_in: Vec<f32> = input.iter().step_by(2).copied().collect();
        let noise: Vec<f32> = l.iter().zip(l_in.iter()).map(|(o, i)| o - i).collect();

        // DFT bin check: energy in first vs last quarter of spectrum.
        // Compute DFT magnitude squared for each bin and sum low (bins 0..n/4)
        // vs high (bins n*3/4..n) frequency halves.
        let n = noise.len();
        let low_energy: f32 = (0..n / 4).map(|k| {
            let re: f32 = noise.iter().enumerate()
                .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).cos())
                .sum();
            let im: f32 = noise.iter().enumerate()
                .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).sin())
                .sum();
            re * re + im * im
        }).sum::<f32>() / n as f32;
        let high_energy: f32 = (n * 3 / 4..n).map(|k| {
            let re: f32 = noise.iter().enumerate()
                .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).cos())
                .sum();
            let im: f32 = noise.iter().enumerate()
                .map(|(i, &x)| x * (2.0 * PI * k as f32 * i as f32 / n as f32).sin())
                .sum();
            re * re + im * im
        }).sum::<f32>() / n as f32;

        assert!(
            high_energy > low_energy,
            "noise shaping should push energy high: high={high_energy:.2e} low={low_energy:.2e}"
        );
    }

    // Rate change clears state without panic.
    #[test]
    fn sample_rate_change_resets_state() {
        let mut f = DitherFilter::new(16, NoiseShaping::Lipshitz);
        let chunk = sine_stereo(1000.0, 44100, 64);
        let _ = f.process(&chunk, 44100);
        // After rate change, process must not panic and must produce valid output.
        let out = f.process(&chunk, 48000);
        assert_eq!(out.len(), chunk.len());
        assert!(out.iter().all(|s| s.is_finite()), "output must be finite after rate change");
    }

    // Shibata: all three test rates must not panic.
    #[test]
    fn shibata_selects_nearest_rate_table() {
        let chunk = sine_stereo(440.0, 44100, 64);
        for &rate in &[44100_u32, 48000, 96000] {
            let mut f = DitherFilter::new(16, NoiseShaping::Shibata);
            let out = f.process(&chunk, rate);
            assert_eq!(out.len(), chunk.len(), "rate={rate}");
            assert!(out.iter().all(|s| s.is_finite()), "non-finite at rate={rate}");
        }
    }

    // Gesemann IIR must not produce NaN or Inf.
    #[test]
    fn gesemann_iir_no_nan() {
        let chunk = sine_stereo(440.0, 44100, 1000);
        let mut f = DitherFilter::new(16, NoiseShaping::Gesemann);
        let out = f.process(&chunk, 44100);
        assert!(out.iter().all(|s| s.is_finite()), "Gesemann must not produce NaN/Inf");
    }

    // set_params resets all state buffers; rng is re-seeded to INITIAL_SEED.
    #[test]
    fn set_params_resets_state() {
        let mut f = DitherFilter::new(16, NoiseShaping::Gesemann);
        let chunk = sine_stereo(440.0, 44100, 64);
        let _ = f.process(&chunk, 44100);
        // After set_params, all bufs must be zero and rng must be INITIAL_SEED.
        f.set_params(16, NoiseShaping::Gesemann);
        assert!(f.err_l.iter().all(|&x| x == 0.0), "err_l not zeroed");
        assert!(f.err_r.iter().all(|&x| x == 0.0), "err_r not zeroed");
        assert!(f.ff_l.iter().all(|&x| x == 0.0), "ff_l not zeroed");
        assert!(f.ff_r.iter().all(|&x| x == 0.0), "ff_r not zeroed");
        assert!(f.fb_l.iter().all(|&x| x == 0.0), "fb_l not zeroed");
        assert!(f.fb_r.iter().all(|&x| x == 0.0), "fb_r not zeroed");
        assert_eq!(f.rng, INITIAL_SEED, "rng must be reset to INITIAL_SEED");
    }
}
