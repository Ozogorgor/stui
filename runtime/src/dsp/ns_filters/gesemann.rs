//! Gesemann noise shaping coefficients.
//!
//! Rate-selected feedforward + feedback pairs.
//! Sourced from SoX src/dither.c (LGPL).

pub static GES44_FF: &[f32] = &[2.2061, -0.4706, -0.2534, -0.6214];
pub static GES44_FB: &[f32] = &[1.0587, 0.0676, -0.6054, -0.2738];
pub static GES48_FF: &[f32] = &[2.2374, -0.7339, -0.1251, -0.6033];
pub static GES48_FB: &[f32] = &[0.9030, 0.0116, -0.5853, -0.2571];

/// Select Gesemann coefficients based on sample rate.
#[inline]
pub fn select(rate_hz: u32) -> (&'static [f32], &'static [f32]) {
    if rate_hz <= 46050 {
        (GES44_FF, GES44_FB)
    } else {
        (GES48_FF, GES48_FB)
    }
}
