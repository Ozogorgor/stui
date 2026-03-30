use super::DspNode;
use crate::dsp::config::DspConfig;
use crate::dsp::crossfeed::CrossfeedFilter;

pub struct CrossfeedNode {
    inner: Option<CrossfeedFilter>,
    enabled: bool,
    feed_level: f32,
    cutoff_hz: f32,
}

impl CrossfeedNode {
    pub fn new(config: &DspConfig) -> Self {
        let enabled = config.crossfeed_enabled || config.crossfeed_auto;
        let inner = if enabled {
            Some(CrossfeedFilter::new(
                config.crossfeed_feed_level,
                config.crossfeed_cutoff_hz,
            ))
        } else {
            None
        };

        Self {
            inner,
            enabled,
            feed_level: config.crossfeed_feed_level,
            cutoff_hz: config.crossfeed_cutoff_hz,
        }
    }
}

impl DspNode for CrossfeedNode {
    fn name(&self) -> &str {
        "crossfeed"
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
            self.inner = Some(CrossfeedFilter::new(self.feed_level, self.cutoff_hz));
        } else if !enabled {
            self.inner = None;
        }
    }

    fn update_config(&mut self, config: &DspConfig) {
        self.feed_level = config.crossfeed_feed_level;
        self.cutoff_hz = config.crossfeed_cutoff_hz;
        if config.crossfeed_enabled || config.crossfeed_auto {
            if self.inner.is_none() {
                self.inner = Some(CrossfeedFilter::new(
                    config.crossfeed_feed_level,
                    config.crossfeed_cutoff_hz,
                ));
                self.enabled = true;
            } else if let Some(ref mut inner) = self.inner {
                inner.set_params(config.crossfeed_feed_level, config.crossfeed_cutoff_hz);
                self.enabled = true;
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
