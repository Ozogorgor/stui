pub mod crossfeed_node;
pub mod dc_offset_node;
pub mod dither_node;
pub mod eq_node;
pub mod gain_node;
pub mod resample_node;

#[allow(unused_imports)]
pub use crossfeed_node::CrossfeedNode;
#[allow(unused_imports)]
pub use dc_offset_node::DcOffsetNode;
#[allow(unused_imports)]
pub use dither_node::DitherNode;
pub use eq_node::EqNode;
pub use gain_node::GainNode;
#[allow(unused_imports)]
pub use resample_node::ResampleNode;

use crate::dsp::config::DspConfig;

#[allow(dead_code)] // planned: DSP node trait, implemented by all DSP node types
pub trait DspNode: Send {
    fn name(&self) -> &str;
    fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> Vec<f32>;
    fn is_enabled(&self) -> bool;
    fn set_enabled(&mut self, enabled: bool);
    fn update_config(&mut self, config: &DspConfig);
    fn flush(&mut self);
}

#[allow(dead_code)]
pub struct DspChain {
    nodes: Vec<Box<dyn DspNode>>,
}

#[allow(dead_code)] // planned: DSP chain pub API, wired in by DspPipeline
impl DspChain {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn add(&mut self, node: Box<dyn DspNode>) {
        self.nodes.push(node);
    }

    pub fn process(&mut self, samples: &mut [f32], sample_rate: u32) -> Vec<f32> {
        // Note: Per-call allocation here can cause real-time audio issues under load.
        // For production, use a pre-allocated buffer or process in-place where possible.
        // Since resample can change output size, full optimization requires buffer pooling.
        let mut result = samples.to_vec();
        for node in self.nodes.iter_mut() {
            if node.is_enabled() {
                result = node.process(&mut result, sample_rate);
            }
        }
        result
    }

    pub fn flush(&mut self) {
        for node in self.nodes.iter_mut() {
            node.flush();
        }
    }

    pub fn node_mut(&mut self, name: &str) -> Option<&mut Box<dyn DspNode>> {
        self.nodes.iter_mut().find(|n| n.name() == name)
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        if let Some(node) = self.node_mut(name) {
            node.set_enabled(enabled);
        }
    }

    pub fn describe(&self) -> String {
        self.nodes
            .iter()
            .filter(|n| n.is_enabled())
            .map(|n| n.name().to_string())
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

impl Default for DspChain {
    fn default() -> Self {
        Self::new()
    }
}
