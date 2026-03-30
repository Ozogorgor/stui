//! F-weighted noise shaping coefficients.
//!
//! Sourced from SoX src/dither.c (LGPL).

pub static COEFFS: &[f32] = &[
    2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847,
];
