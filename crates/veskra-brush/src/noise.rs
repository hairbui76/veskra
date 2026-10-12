//! Seeded procedural value noise (procedural texture / schema-2 "grain").
//! Pure integer hashing + `f64` interpolation: deterministic for identical inputs.

/// Procedural grain parameters as the legacy rasterizer consumes them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Grain {
    /// Feature size in document px.
    pub scale: f32,
    /// `0..=1`; dab alpha is multiplied by `1 − strength · noise'`.
    pub strength: f32,
}

#[inline]
pub(crate) fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Lattice value in `[0, 1)` for integer cell `(ix, iy)`.
#[inline]
fn lattice(ix: i64, iy: i64, seed: u64) -> f64 {
    let h = splitmix64(seed ^ splitmix64((ix as u64) ^ (iy as u64).rotate_left(32)));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

#[inline]
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear (smoothstep-weighted) value noise in `[0, 1)` with feature size `scale` px.
pub(crate) fn value_noise(x: f64, y: f64, scale: f64, seed: u64) -> f64 {
    let (fx, fy) = (x / scale, y / scale);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (smooth(fx - x0), smooth(fy - y0));
    let (ix, iy) = (x0 as i64, y0 as i64);
    let a = lattice(ix, iy, seed);
    let b = lattice(ix + 1, iy, seed);
    let c = lattice(ix, iy + 1, seed);
    let d = lattice(ix + 1, iy + 1, seed);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    top + (bot - top) * ty
}

/// Grain value `n` in `[0, 1]` at `(x, y)`: two octaves (`scale`, `scale/2`), contrast-stretched
/// around 0.5.
#[inline]
pub(crate) fn grain_noise(x: f64, y: f64, scale: f64, seed: u64) -> f64 {
    let n = 0.65 * value_noise(x, y, scale, seed) + 0.35 * value_noise(x, y, scale * 0.5, seed ^ 0xA5A5_5A5A_C3C3_3C3C);
    ((n - 0.5) * 1.8 + 0.5).clamp(0.0, 1.0)
}

/// Grain multiplier in `[1 − strength, 1]` at document pixel center `(x, y)`.
pub(crate) fn grain_factor(x: f64, y: f64, g: &Grain, seed: u64) -> f32 {
    let n = grain_noise(x, y, g.scale as f64, seed);
    (1.0 - g.strength as f64 * n) as f32
}

/// Deterministic uniform value in `[0, 1)` for (seed, dab index, channel).
#[inline]
pub(crate) fn rand01(seed: u64, index: u64, channel: u64) -> f64 {
    let h = splitmix64(seed ^ splitmix64(index.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ channel.rotate_left(17)));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_bounded_seeded() {
        let g = Grain { scale: 2.0, strength: 0.7 };
        let mut differs = false;
        for i in 0..500 {
            let (x, y) = (i as f64 * 0.73 - 40.0, i as f64 * 1.31 - 90.0);
            let a = grain_factor(x, y, &g, 7);
            assert_eq!(a, grain_factor(x, y, &g, 7));
            assert!((0.3 - 1e-6..=1.0).contains(&a), "{a}");
            differs |= a != grain_factor(x, y, &g, 8);
        }
        assert!(differs);
    }
}
