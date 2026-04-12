use super::DspNode;
use crate::dsp::config::DspConfig;
use crate::dsp::dc_offset::DcOffsetFilter;

#[allow(dead_code)] // pub API: used by DSP chain
pub struct DcOffsetNode {
    inner: Option<DcOffsetFilter>,
    enabled: bool,
    last_cutoff_hz: f32,
}

#[allow(dead_code)] // pub API: used by DSP chain
impl DcOffsetNode {
    pub fn new(config: &DspConfig) -> Self {
        let enabled = config.dc_offset_enabled;
        let cutoff_hz = config.dc_offset_cutoff_hz;
        let inner = if enabled {
            Some(DcOffsetFilter::new(cutoff_hz))
        } else {
            None
        };

        Self {
            inner,
            enabled,
            last_cutoff_hz: cutoff_hz,
        }
    }
}

impl DspNode for DcOffsetNode {
    fn name(&self) -> &str {
        "dc_offset"
    }

    fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> Vec<f32> {
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
            self.inner = Some(DcOffsetFilter::new(self.last_cutoff_hz));
        } else if !enabled {
            self.inner = None;
        }
    }

    fn update_config(&mut self, config: &DspConfig) {
        self.last_cutoff_hz = config.dc_offset_cutoff_hz;
        if config.dc_offset_enabled {
            if self.inner.is_none() {
                self.inner = Some(DcOffsetFilter::new(config.dc_offset_cutoff_hz));
            }
            self.enabled = true;
            if let Some(ref mut inner) = self.inner {
                inner.set_cutoff(config.dc_offset_cutoff_hz);
            }
        } else {
            self.enabled = false;
            self.inner = None;
        }
    }

    fn flush(&mut self) {
        if let Some(ref mut inner) = self.inner {
            inner.reset();
        }
    }
}
