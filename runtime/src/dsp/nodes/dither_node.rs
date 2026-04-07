use super::DspNode;
use crate::dsp::config::DspConfig;
use crate::dsp::ns_filters::dither::{DitherFilter, NoiseShaping};

#[allow(dead_code)]
pub struct DitherNode {
    inner: Option<DitherFilter>,
    enabled: bool,
    bit_depth: u32,
    noise_shaping: NoiseShaping,
    sample_rate: u32,
}

#[allow(dead_code)]
impl DitherNode {
    pub fn new(config: &DspConfig) -> Self {
        let enabled = config.dither_enabled || config.dither_auto;
        let shaping =
            NoiseShaping::from_str(&config.dither_noise_shaping).unwrap_or(NoiseShaping::None);
        let inner = if enabled {
            Some(DitherFilter::new(config.dither_bit_depth, shaping.clone()))
        } else {
            None
        };

        Self {
            inner,
            enabled,
            bit_depth: config.dither_bit_depth,
            noise_shaping: shaping,
            sample_rate: config.input_sample_rate,
        }
    }
}

impl DspNode for DitherNode {
    fn name(&self) -> &str {
        "dither"
    }

    fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> Vec<f32> {
        self.sample_rate = sample_rate;
        if let Some(ref mut inner) = self.inner {
            inner.process(samples, sample_rate)
        } else {
            samples.to_vec()
        }
    }

    fn is_enabled(&self) -> bool {
        self.enabled && self.inner.is_some()
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled && self.inner.is_none() {
            self.inner = Some(DitherFilter::new(
                self.bit_depth,
                self.noise_shaping.clone(),
            ));
        } else if !enabled {
            self.inner = None;
        }
    }

    fn update_config(&mut self, config: &DspConfig) {
        self.bit_depth = config.dither_bit_depth;
        self.noise_shaping =
            NoiseShaping::from_str(&config.dither_noise_shaping).unwrap_or(NoiseShaping::None);
        if config.dither_enabled || config.dither_auto {
            if self.inner.is_none() {
                self.inner = Some(DitherFilter::new(
                    self.bit_depth,
                    self.noise_shaping.clone(),
                ));
                self.enabled = true;
            } else if let Some(ref mut inner) = self.inner {
                inner.set_params(self.bit_depth, self.noise_shaping.clone());
                self.enabled = true;
            }
        } else {
            self.enabled = false;
            self.inner = None;
        }
    }

    fn flush(&mut self) {
        if let Some(ref mut inner) = self.inner {
            inner.reset_state(self.sample_rate);
        }
    }
}
