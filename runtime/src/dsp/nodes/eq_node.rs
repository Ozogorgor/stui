use super::DspNode;
use crate::dsp::command::EqPreset;
use crate::dsp::config::DspConfig;

pub struct EqNode {
    enabled: bool,
    bands: Vec<EqBandState>,
    sample_rate: u32,
    current_preset: String,
}

// NOTE: field naming here intentionally deviates from DSP convention.
// Standard DSP: b coefficients = feedforward (numerator), a = feedback (denominator).
// Here: a0/a1/a2 store the *normalized feedforward* (b0'/b1'/b2' ÷ a0') and
//       b1/b2 store the *normalized feedback* (a1'/a2' ÷ a0').
// The filter difference equation in process() uses them correctly; don't rename
// without updating all coefficient assignments in make_band and process().
struct EqBandState {
    frequency: f32,
    gain: f32,
    q: f32,
    a0: f32, // normalized feedforward: b0 / a0_denom
    a1: f32, // normalized feedforward: b1 / a0_denom
    a2: f32, // normalized feedforward: b2 / a0_denom
    b1: f32, // normalized feedback:    a1 / a0_denom
    b2: f32, // normalized feedback:    a2 / a0_denom
    // Separate state for left and right channels
    xl: [f32; 2],
    yl: [f32; 2],
    xr: [f32; 2],
    yr: [f32; 2],
}

impl EqNode {
    pub fn new() -> Self {
        Self {
            enabled: true,
            bands: Vec::new(),
            sample_rate: 48000,
            current_preset: "flat".to_string(),
        }
    }

    pub fn set_preset(&mut self, preset: EqPreset) {
        let sr = self.sample_rate;
        self.bands = match preset {
            EqPreset::Flat => Vec::new(),
            EqPreset::BassBoost => vec![
                Self::make_band(60.0, 6.0, 1.0, sr),
                Self::make_band(200.0, 3.0, 1.0, sr),
            ],
            EqPreset::TrebleBoost => vec![
                Self::make_band(3000.0, 3.0, 1.0, sr),
                Self::make_band(8000.0, 6.0, 1.0, sr),
            ],
            EqPreset::Vocal => vec![
                Self::make_band(200.0, -2.0, 1.0, sr),
                Self::make_band(1000.0, 4.0, 1.0, sr),
                Self::make_band(3000.0, 2.0, 1.0, sr),
                Self::make_band(6000.0, -1.0, 1.0, sr),
            ],
            EqPreset::Loudness => vec![
                Self::make_band(60.0, 6.0, 1.0, sr),
                Self::make_band(250.0, 3.0, 1.0, sr),
                Self::make_band(1000.0, 0.0, 1.0, sr),
                Self::make_band(4000.0, -2.0, 1.0, sr),
            ],
            EqPreset::Custom(bands) => bands
                .iter()
                .map(|b| Self::make_band(b.frequency_hz as f32, b.gain_db, b.q, sr))
                .collect(),
        };
    }

    fn make_band(frequency: f32, gain_db: f32, q: f32, sample_rate: u32) -> EqBandState {
        let sr = sample_rate as f32;
        // Guard against frequencies at or above Nyquist (sample_rate/2)
        let nyquist = sr / 2.0;
        let safe_freq = if frequency >= nyquist {
            // Clamp to 99% of Nyquist to avoid filter instability
            nyquist * 0.99
        } else {
            frequency
        };

        let a = 10.0_f32.powf(gain_db / 40.0);
        let omega = 2.0 * std::f32::consts::PI * safe_freq / sr;
        let sin_omega = omega.sin();
        let cos_omega = omega.cos();
        // Prevent division by zero - minimum q of 0.01, also handle negative Q
        let safe_q = if q.abs() < 0.01 { 0.01 } else { q.abs() };
        let alpha = sin_omega / (2.0 * safe_q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_omega;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_omega;
        let a2 = 1.0 - alpha / a;

        EqBandState {
            frequency: safe_freq,
            gain: gain_db,
            q,
            a0: b0 / a0,
            a1: b1 / a0,
            a2: b2 / a0,
            b1: a1 / a0,
            b2: a2 / a0,
            xl: [0.0, 0.0],
            yl: [0.0, 0.0],
            xr: [0.0, 0.0],
            yr: [0.0, 0.0],
        }
    }
}

impl Default for EqNode {
    fn default() -> Self {
        Self::new()
    }
}

impl DspNode for EqNode {
    fn name(&self) -> &str {
        "eq"
    }

    fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> Vec<f32> {
        if !self.enabled || self.bands.is_empty() {
            return samples.to_vec();
        }

        // Recalculate coefficients if sample rate changed, preserving filter state
        // This avoids audible clicks that would occur from resetting filter history
        if sample_rate != self.sample_rate {
            let sr = sample_rate as f32;
            let nyquist = sr / 2.0;

            // Recalculate coefficients for each band
            for band in self.bands.iter_mut() {
                // Clamp frequency to Nyquist to prevent filter instability
                // when sample rate decreases below the original frequency
                let safe_freq = band.frequency.min(nyquist * 0.99);
                band.frequency = safe_freq;
                let a = 10.0_f32.powf(band.gain / 40.0);
                let omega = 2.0 * std::f32::consts::PI * safe_freq / sr;
                let sin_omega = omega.sin();
                let cos_omega = omega.cos();
                let safe_q = if band.q.abs() < 0.01 {
                    0.01
                } else {
                    band.q.abs()
                };
                let alpha = sin_omega / (2.0 * safe_q);

                let b0 = 1.0 + alpha * a;
                let b1 = -2.0 * cos_omega;
                let b2 = 1.0 - alpha * a;
                let a0 = 1.0 + alpha / a;
                let a1 = -2.0 * cos_omega;
                let a2 = 1.0 - alpha / a;

                band.a0 = b0 / a0;
                band.a1 = b1 / a0;
                band.a2 = b2 / a0;
                band.b1 = a1 / a0;
                band.b2 = a2 / a0;
            }
            self.sample_rate = sample_rate;
        }

        let len = samples.len();
        if len % 2 != 0 {
            tracing::warn!(
                "EQ expects stereo interleaved samples, got {} samples — \
                 trailing sample dropped to preserve L/R filter alignment",
                len
            );
        }
        let mut output = samples.to_vec();
        let stereo_frames = len / 2;

        // Process complete stereo frames
        for i in 0..stereo_frames {
            let idx = i * 2;
            let mut l = samples[idx];
            let mut r = samples[idx + 1];

            for band in self.bands.iter_mut() {
                // Left channel
                let l_out = band.a0 * l + band.a1 * band.xl[0] + band.a2 * band.xl[1]
                    - band.b1 * band.yl[0]
                    - band.b2 * band.yl[1];
                band.xl[1] = band.xl[0];
                band.xl[0] = l;
                band.yl[1] = band.yl[0];
                band.yl[0] = l_out;
                l = l_out;

                // Right channel
                let r_out = band.a0 * r + band.a1 * band.xr[0] + band.a2 * band.xr[1]
                    - band.b1 * band.yr[0]
                    - band.b2 * band.yr[1];
                band.xr[1] = band.xr[0];
                band.xr[0] = r;
                band.yr[1] = band.yr[0];
                band.yr[0] = r_out;
                r = r_out;
            }

            output[idx] = l;
            output[idx + 1] = r;
        }

        // Drop the trailing odd sample rather than pushing it through the left-channel
        // filter state: processing it would advance xl/yl out of phase with the next
        // call's properly-paired L/R frames, corrupting subsequent stereo separation.
        if len % 2 != 0 {
            output.truncate(len - 1);
        }

        output
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn update_config(&mut self, config: &DspConfig) {
        let name = &config.eq_preset;
        // "custom" bands are set directly via set_preset(EqPreset::Custom(..)) and
        // cannot be reconstructed from DspConfig alone — skip to avoid overwriting them.
        if name == "custom" || name == self.current_preset {
            return;
        }
        let preset = match name.as_str() {
            "bass_boost"    => EqPreset::BassBoost,
            "treble_boost"  => EqPreset::TrebleBoost,
            "vocal"         => EqPreset::Vocal,
            "loudness"      => EqPreset::Loudness,
            _               => EqPreset::Flat, // "flat" and unknown values
        };
        self.set_preset(preset);
        self.current_preset = name.clone();
    }

    fn flush(&mut self) {
        for band in self.bands.iter_mut() {
            band.xl = [0.0, 0.0];
            band.yl = [0.0, 0.0];
            band.xr = [0.0, 0.0];
            band.yr = [0.0, 0.0];
        }
    }
}
