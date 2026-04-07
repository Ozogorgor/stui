//! Noise shaping filters.
//!
//! Contains dithering and noise shaping algorithms:
//! - DitherFilter: dithering with various noise shaping algorithms
//! - SawNode: Spectral Amplitude Weighting (SAW) noise shaping
//!
//! Individual noise shaping coefficient files:
//! - lipshitz.rs: Lipshitz noise shaping
//! - fweighted.rs: F-weighted noise shaping
//! - mod_e_weighted.rs: Modified-E-weighted noise shaping
//! - imp_e_weighted.rs: Improved-E-weighted noise shaping
//! - shibata.rs: Shibata, Low-Shibata, High-Shibata
//! - gesemann.rs: Gesemann noise shaping

pub mod dither;
pub mod saw;
pub mod tns;

// Individual NS coefficient modules
pub mod fweighted;
pub mod gesemann;
pub mod imp_e_weighted;
pub mod lipshitz;
pub mod mod_e_weighted;
pub mod shibata;

#[allow(unused_imports)]
pub use dither::{DitherFilter, NoiseShaping};
#[allow(unused_imports)]
pub use saw::{SawNode, StereoStftProcessor, StftProcessor};
#[allow(unused_imports)]
pub use tns::{StereoTnsProcessor, TnsNode};
