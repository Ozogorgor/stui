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
pub fn encode(samples: &[f32]) -> Vec<f32> {
    let mut output = Vec::with_capacity(samples.len());

    // Process as interleaved stereo: [L0, R0, L1, R1, ...]
    let mut i = 0;
    while i + 1 < samples.len() {
        let left = samples[i];
        let right = samples[i + 1];

        // Mid = (L + R) / 2
        let mid = (left + right) * 0.5;
        // Side = (L - R) / 2
        let side = (left - right) * 0.5;

        output.push(mid);
        output.push(side);

        i += 2;
    }

    output
}

/// Process audio from Mid/Side back to L/R representation.
pub fn decode(samples: &[f32]) -> Vec<f32> {
    let mut output = Vec::with_capacity(samples.len());

    let mut i = 0;
    while i + 1 < samples.len() {
        let mid = samples[i];
        let side = samples[i + 1];

        // L = Mid + Side
        let left = mid + side;
        // R = Mid - Side
        let right = mid - side;

        output.push(left);
        output.push(right);

        i += 2;
    }

    output
}

/// Apply stereo width control.
/// width = 1.0: normal stereo
/// width > 1.0: wider stereo
/// width < 1.0: narrower stereo (0 = mono)
pub fn apply_width(samples: &[f32], width: f32) -> Vec<f32> {
    let mid_side = encode(samples);
    let mut output = Vec::with_capacity(mid_side.len());

    let mut i = 0;
    while i + 1 < mid_side.len() {
        let mid = mid_side[i];
        let side = mid_side[i + 1] * width;

        output.push(mid);
        output.push(side);

        i += 2;
    }

    decode(&output)
}

/// M/S processor with configurable width and balance.
pub struct MidSideProcessor {
    width: f32,
    mid_gain: f32,
    side_gain: f32,
    enabled: bool,
}

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

    /// Process stereo samples through M/S chain.
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if !self.enabled {
            return samples.to_vec();
        }

        // Encode L/R to M/S
        let mut mid_side = encode(samples);

        // Apply gains
        for i in (0..mid_side.len()).step_by(2) {
            mid_side[i] *= self.mid_gain; // Mid
            mid_side[i + 1] *= self.side_gain; // Side
        }

        // Apply width
        if (self.width - 1.0).abs() > 0.001 {
            for i in (1..mid_side.len()).step_by(2) {
                mid_side[i] *= self.width;
            }
        }

        // Decode back to L/R
        decode(&mid_side)
    }

    /// Set stereo width.
    pub fn set_width(&mut self, width: f32) {
        // Clamp to reasonable range: 0 (mono) to 2.0 (double stereo)
        self.width = width.clamp(0.0, 2.0);
    }

    /// Set mid (center) gain.
    pub fn set_mid_gain(&mut self, gain: f32) {
        self.mid_gain = gain.clamp(0.0, 2.0);
    }

    /// Set side (stereo) gain.
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

        // Normal width
        let normal = apply_width(&stereo, 1.0);
        assert!((normal[0] - 1.0).abs() < 1e-6);
        assert!((normal[1] - (-1.0)).abs() < 1e-6);

        // Double width
        // Input: L=1, R=-1 -> Mid=0, Side=1
        // After width=2: Side=2 -> L=2, R=-2
        let wide = apply_width(&stereo, 2.0);
        assert!((wide[0] - 2.0).abs() < 1e-6);
        assert!((wide[1] - (-2.0)).abs() < 1e-6);

        // Zero width (mono)
        // Input: L=1, R=-1 -> Mid=0, Side=1
        // After width=0: Side=0 -> L=0, R=0 (mono)
        let narrow = apply_width(&stereo, 0.0);
        assert!((narrow[0] - narrow[1]).abs() < 1e-6); // Both should be equal
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

        // With 0.5 width, side is halved
        // L = mid + side*0.5 = 0 + 1*0.5 = 0.5
        // R = mid - side*0.5 = 0 - 1*0.5 = -0.5
        assert!((output[0] - 0.5).abs() < 1e-6);
        assert!((output[1] - (-0.5)).abs() < 1e-6);
    }
}
