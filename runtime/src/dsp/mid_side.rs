//! Mid/Side (M/S) processing for stereo audio.
//!
//! Mid/Side processing converts left/right stereo to mid/side representation:
//! - Mid = (L + R) / 2  (mono center information)
//! - Side = (L - R) / 2  (stereo width information)
//!
//! This allows independent processing of center vs stereo width content.
//! Common uses:
//! - Stereo widening (increase Side)
//! - Stereo narrowing (decrease Side)
//! - Independent EQ for center vs ambience
//! - Bass management (route low Side to Mid)

/// Process audio from L/R to Mid/Side representation.
///
/// Input must be interleaved stereo: [L0, R0, L1, R1, ...]
#[allow(dead_code)] // Used by MidSideProcessor internally
pub fn encode(samples: &[f32]) -> Vec<f32> {
    debug_assert!(
        samples.len() % 2 == 0,
        "encode: input must be interleaved stereo"
    );
    let mut output = Vec::with_capacity(samples.len());
    for frame in samples.chunks_exact(2) {
        output.push((frame[0] + frame[1]) * 0.5); // Mid = (L + R) / 2
        output.push((frame[0] - frame[1]) * 0.5); // Side = (L - R) / 2
    }
    output
}

/// Process audio from Mid/Side back to L/R representation.
///
/// Input must be interleaved M/S: [M0, S0, M1, S1, ...]
#[allow(dead_code)] // Used by MidSideProcessor internally
pub fn decode(samples: &[f32]) -> Vec<f32> {
    debug_assert!(
        samples.len() % 2 == 0,
        "decode: input must be interleaved M/S"
    );
    let mut output = Vec::with_capacity(samples.len());
    for frame in samples.chunks_exact(2) {
        output.push(frame[0] + frame[1]); // L = Mid + Side
        output.push(frame[0] - frame[1]); // R = Mid - Side
    }
    output
}

/// Apply stereo width control directly to L/R samples.
///
/// - `width = 1.0`: unchanged
/// - `width > 1.0`: wider stereo image
/// - `width < 1.0`: narrower (0.0 = mono)
#[allow(dead_code)] // Available for future use
pub fn apply_width(samples: &[f32], width: f32) -> Vec<f32> {
    debug_assert!(
        samples.len() % 2 == 0,
        "apply_width: input must be interleaved stereo"
    );
    let mut output = Vec::with_capacity(samples.len());
    for frame in samples.chunks_exact(2) {
        let mid = (frame[0] + frame[1]) * 0.5;
        let side = (frame[0] - frame[1]) * 0.5 * width;
        output.push(mid + side);
        output.push(mid - side);
    }
    output
}

/// M/S processor with configurable width and independent mid/side gains.
#[allow(dead_code)] // planned: M/S stereo processor, wired in by DSP pipeline
pub struct MidSideProcessor {
    width: f32,
    mid_gain: f32,
    side_gain: f32,
    enabled: bool,
}

#[allow(dead_code)] // MidSideProcessor for future use
impl MidSideProcessor {
    /// Create a new M/S processor with default settings.
    pub fn new() -> Self {
        Self {
            width: 1.0,
            mid_gain: 1.0,
            side_gain: 1.0,
            enabled: false,
        }
    }

    /// Process stereo samples through the M/S chain in a single pass.
    #[allow(dead_code)] // planned: M/S stereo processor, wired in by DSP pipeline
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if !self.enabled {
            return samples.to_vec();
        }

        debug_assert!(
            samples.len() % 2 == 0,
            "process: input must be interleaved stereo"
        );

        // Fold width and side_gain into a single effective side multiplier so
        // the side channel is only scaled once.
        let effective_side = self.side_gain * self.width;
        let mid_gain = self.mid_gain;

        let mut output = Vec::with_capacity(samples.len());

        for frame in samples.chunks_exact(2) {
            let l = frame[0];
            let r = frame[1];

            // Encode, apply gains, decode — single pass, no intermediate Vec.
            let mid = (l + r) * 0.5 * mid_gain;
            let side = (l - r) * 0.5 * effective_side;

            output.push(mid + side); // L = Mid + Side
            output.push(mid - side); // R = Mid - Side
        }

        output
    }

    /// Set stereo width. Clamped to [0.0, 2.0].
    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(0.0, 2.0);
    }

    /// Set mid (center) gain. Clamped to [0.0, 2.0].
    pub fn set_mid_gain(&mut self, gain: f32) {
        self.mid_gain = gain.clamp(0.0, 2.0);
    }

    /// Set side (stereo) gain. Clamped to [0.0, 2.0].
    pub fn set_side_gain(&mut self, gain: f32) {
        self.side_gain = gain.clamp(0.0, 2.0);
    }

    /// Enable/disable M/S processing.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if processing is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get current width.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Get current mid gain.
    pub fn mid_gain(&self) -> f32 {
        self.mid_gain
    }

    /// Get current side gain.
    pub fn side_gain(&self) -> f32 {
        self.side_gain
    }
}

impl Default for MidSideProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_is_identity() {
        let input: Vec<f32> = vec![0.5, -0.5, 0.25, -0.25, 0.0, 1.0];
        let encoded = encode(&input);
        let decoded = decode(&encoded);

        for (a, b) in input.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-6, "{} != {}", a, b);
        }
    }

    #[test]
    fn mono_becomes_dual_mid() {
        // Mono: L = R = 0.5
        let mono = vec![0.5, 0.5];
        let encoded = encode(&mono);

        // Mid should be 0.5, Side should be 0
        assert!((encoded[0] - 0.5).abs() < 1e-6);
        assert!(encoded[1].abs() < 1e-6);
    }

    #[test]
    fn width_processing() {
        // Pure stereo: L = 1, R = -1
        let stereo = vec![1.0, -1.0];

        // Normal width — signal unchanged
        let normal = apply_width(&stereo, 1.0);
        assert!((normal[0] - 1.0).abs() < 1e-6);
        assert!((normal[1] - (-1.0)).abs() < 1e-6);

        // Double width: L=1, R=-1 → Mid=0, Side=1 → Side*2=2 → L=2, R=-2
        let wide = apply_width(&stereo, 2.0);
        assert!((wide[0] - 2.0).abs() < 1e-6);
        assert!((wide[1] - (-2.0)).abs() < 1e-6);

        // Zero width (mono): Side=0 → L=R
        let narrow = apply_width(&stereo, 0.0);
        assert!((narrow[0] - narrow[1]).abs() < 1e-6);
    }

    #[test]
    fn processor_default_disabled() {
        let mut proc = MidSideProcessor::new();
        assert!(!proc.is_enabled());

        let input = vec![0.5, -0.5];
        let output = proc.process(&input);
        assert_eq!(input, output);
    }

    #[test]
    fn processor_applies_width() {
        let mut proc = MidSideProcessor::new();
        proc.set_enabled(true);
        proc.set_width(0.5);

        let input = vec![1.0, -1.0];
        let output = proc.process(&input);

        // Mid=0, Side=1 → Side*0.5=0.5 → L=0.5, R=-0.5
        assert!((output[0] - 0.5).abs() < 1e-6);
        assert!((output[1] - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn processor_width_and_side_gain_are_independent() {
        // With both controls, effective side = side_gain * width.
        // width=2, side_gain=0.5 → effective_side=1.0 → signal unchanged.
        let mut proc = MidSideProcessor::new();
        proc.set_enabled(true);
        proc.set_width(2.0);
        proc.set_side_gain(0.5);

        let input = vec![1.0, -1.0];
        let output = proc.process(&input);
        // effective_side = 2.0 * 0.5 = 1.0 → no change
        assert!(
            (output[0] - 1.0).abs() < 1e-6,
            "L should be unchanged, got {}",
            output[0]
        );
        assert!(
            (output[1] - (-1.0)).abs() < 1e-6,
            "R should be unchanged, got {}",
            output[1]
        );
    }

    #[test]
    fn processor_mid_gain_attenuates_center() {
        let mut proc = MidSideProcessor::new();
        proc.set_enabled(true);
        proc.set_mid_gain(0.0); // kill center

        // Mono signal → only mid → should be silenced
        let input = vec![0.5, 0.5];
        let output = proc.process(&input);
        assert!(output[0].abs() < 1e-6);
        assert!(output[1].abs() < 1e-6);
    }

    #[test]
    fn unity_gain_is_identity() {
        // width=1, mid_gain=1, side_gain=1 → no change
        let mut proc = MidSideProcessor::new();
        proc.set_enabled(true);

        let input: Vec<f32> = (0..64)
            .flat_map(|i| {
                let l = (i as f32 * 0.1).sin() * 0.5;
                let r = (i as f32 * 0.07).cos() * 0.3;
                [l, r]
            })
            .collect();

        let output = proc.process(&input);

        for (a, b) in input.iter().zip(output.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "unity should be identity: {} != {}",
                a,
                b
            );
        }
    }
}
