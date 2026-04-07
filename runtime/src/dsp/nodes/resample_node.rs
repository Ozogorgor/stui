use super::DspNode;
use crate::dsp::config::DspConfig;
use crate::dsp::resample::Resampler;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

#[allow(dead_code)]
pub struct ResampleNode {
    inner: Option<Resampler>,
    enabled: bool,
    last_config: Option<DspConfig>,
}

#[allow(dead_code)]
impl ResampleNode {
    pub fn new(config: DspConfig) -> Self {
        let mut enabled = config.resample_enabled;
        let last_config = Some(config.clone());
        let config_arc = Arc::new(RwLock::new(config));
        let inner = match Resampler::new(config_arc) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!("Failed to initialize resampler: {}", e);
                enabled = false;
                None
            }
        };

        Self {
            inner,
            enabled,
            last_config,
        }
    }

    pub fn output_rate(&self) -> Option<u32> {
        self.inner.as_ref().map(|r| r.output_rate())
    }
}

impl DspNode for ResampleNode {
    fn name(&self) -> &str {
        "resample"
    }

    fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> Vec<f32> {
        if let Some(ref mut inner) = self.inner {
            if self.enabled {
                inner.process(samples, sample_rate)
            } else {
                samples.to_vec()
            }
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
            if let Some(cfg) = &self.last_config {
                let config_arc = Arc::new(RwLock::new(cfg.clone()));
                match Resampler::new(config_arc) {
                    Ok(r) => {
                        self.inner = Some(r);
                    }
                    Err(e) => {
                        warn!(
                            "Failed to lazily initialize resampler in set_enabled: {}",
                            e
                        );
                        self.enabled = false;
                    }
                }
            }
        } else if !enabled {
            self.inner = None;
        }
    }

    fn update_config(&mut self, config: &DspConfig) {
        self.last_config = Some(config.clone());
        self.enabled = config.resample_enabled;
        if config.resample_enabled {
            if self.inner.is_none() {
                // Lazily construct the resampler when enabled but not yet initialized
                // (mirrors CrossfeedNode/DitherNode lazy re-enable pattern).
                let config_arc = Arc::new(RwLock::new(config.clone()));
                match Resampler::new(config_arc) {
                    Ok(r) => {
                        self.inner = Some(r);
                    }
                    Err(e) => {
                        warn!("Failed to lazily initialize resampler: {}", e);
                        self.enabled = false;
                    }
                }
            }
        } else {
            self.inner = None;
        }
    }

    fn flush(&mut self) {
        if let Some(ref mut inner) = self.inner {
            inner.reset();
        }
    }
}
