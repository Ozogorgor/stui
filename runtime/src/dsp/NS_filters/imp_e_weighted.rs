//! Improved-E-weighted noise shaping coefficients.
//!
//! Sourced from SoX src/dither.c (LGPL).

pub static COEFFS: &[f32] = &[
    2.847, -4.685, 6.214, -7.184, 6.639, -5.032, 3.263, -1.632, 0.4191,
];
