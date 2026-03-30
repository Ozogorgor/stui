//! Modified-E-weighted noise shaping coefficients.
//!
//! Sourced from SoX src/dither.c (LGPL).

pub static COEFFS: &[f32] = &[
    1.662, -1.263, 0.4827, -0.2913, 0.1268, -0.1124, 0.03252, -0.01265, -0.03524,
];
