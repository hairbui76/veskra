//! The **only** place where alpha representation and bit-depth conversions are implemented
//! (ADR-005, CONTRACTS §1).
//!
//! # Rounding rules (normative)
//! * float → u16: `floor(v · 65535 + 0.5)` after clamping `v` to `[0, 1]` (NaN → 0).
//! * u16 → u8: `(v · 255 + 32767) / 65535` (integer; exact round-half-up of `v · 255 / 65535`).
//! * u8 → u16: `v · 257` (exact).
//! * Unpremultiply: single rounding of the exact rational `c / a` scaled to the output depth,
//!   clamped so channels never exceed full scale (repairs `c > a` violations).
//!
//! # No halos
//! Pixels whose (output) alpha is 0 become `(0, 0, 0, 0)`. A straight color is never derived
//! from a rounded-to-8-bit alpha; it always comes from the 16-bit premultiplied value, so small
//! alpha cannot amplify quantization noise into visible color shifts.
//!
//! # Slice handling
//! Buffer functions process `min(src_pixels, dst_pixels)` whole RGBA pixels; trailing partial
//! pixels and excess destination values are left untouched. Debug builds assert matching sizes.
//!
//! # Transfer for linear conversions
//! [`premul16_to_linear_premul_f32`] / [`linear_premul_f32_to_premul16`] use the sRGB curve: the v1
//! working profile is sRGB (ADR-006). Other working profiles need a profile-aware path.

use crate::transfer::{linear_to_srgb_f64, srgb_u16_to_linear_lut};
use veskra_core::ColorRgba;

#[inline]
fn unit32(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// float `[0,1]` → u16 with the contract rounding.
#[inline]
pub fn f32_to_u16(v: f32) -> u16 {
    (unit32(v) as f64 * 65535.0 + 0.5).floor() as u16
}

/// `f64` variant of [`f32_to_u16`].
#[inline]
pub fn f64_to_u16(v: f64) -> u16 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    (v * 65535.0 + 0.5).floor() as u16
}

/// u16 → u8 with exact round-half-up.
#[inline]
pub fn u16_to_u8(v: u16) -> u8 {
    ((v as u32 * 255 + 32767) / 65535) as u8
}

/// u8 → u16 (exact, `v · 257`).
#[inline]
pub fn u8_to_u16(v: u8) -> u16 {
    v as u16 * 257
}

/// `round(c · scale / a)` clamped to `scale`, for `a > 0`. Exact integer arithmetic.
#[inline]
fn unpremul(c: u16, a: u16, scale: u64) -> u64 {
    let (c, a) = (c as u64, a as u64);
    ((2 * c * scale + a) / (2 * a)).min(scale)
}

#[inline]
fn pixel_count(src_len: usize, dst_len: usize) -> usize {
    debug_assert_eq!(src_len / 4, dst_len / 4, "pixel buffer length mismatch");
    (src_len / 4).min(dst_len / 4)
}

/// RGBA16 premultiplied → RGBA8 straight (PNG/PSD interchange). Output alpha
/// `u16_to_u8(a)`; if that is 0 the pixel is `(0,0,0,0)`; otherwise each channel is
/// `round(c · 255 / a)` (≤ 255).
pub fn premul16_to_straight8(src: &[u16], dst: &mut [u8]) {
    let n = pixel_count(src.len(), dst.len());
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)).take(n) {
        let a8 = u16_to_u8(s[3]);
        if a8 == 0 {
            d.fill(0);
            continue;
        }
        for c in 0..3 {
            d[c] = unpremul(s[c], s[3], 255) as u8;
        }
        d[3] = a8;
    }
}

/// RGBA16 premultiplied → RGBA8 premultiplied (Android `ARGB_8888` memory layout: bytes
/// R, G, B, A). Each channel uses [`u16_to_u8`]; channels are clamped to alpha so `r,g,b ≤ a`
/// holds even for invalid input.
pub fn premul16_to_premul8(src: &[u16], dst: &mut [u8]) {
    let n = pixel_count(src.len(), dst.len());
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)).take(n) {
        let a = u16_to_u8(s[3]);
        d[0] = u16_to_u8(s[0]).min(a);
        d[1] = u16_to_u8(s[1]).min(a);
        d[2] = u16_to_u8(s[2]).min(a);
        d[3] = a;
    }
}

/// RGBA8 straight → RGBA16 premultiplied. `a16 = a · 257`, `c16 = round(c · a · 257 / 255)`.
/// Alpha 0 → `(0,0,0,0)`. Exactly invertible by [`premul16_to_straight8`] for every `a ≥ 1`.
pub fn straight8_to_premul16(src: &[u8], dst: &mut [u16]) {
    let n = pixel_count(src.len(), dst.len());
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)).take(n) {
        let a = s[3] as u32;
        for c in 0..3 {
            d[c] = ((2 * s[c] as u32 * a * 257 + 255) / 510) as u16;
        }
        d[3] = u8_to_u16(s[3]);
    }
}

/// RGBA16 premultiplied → RGBA16 straight (16-bit PSD channel data). Alpha 0 → `(0,0,0,0)`;
/// otherwise `round(c · 65535 / a)` (≤ 65535).
pub fn premul16_to_straight16(src: &[u16], dst: &mut [u16]) {
    let n = pixel_count(src.len(), dst.len());
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)).take(n) {
        if s[3] == 0 {
            d.fill(0);
            continue;
        }
        for c in 0..3 {
            d[c] = unpremul(s[c], s[3], 65535) as u16;
        }
        d[3] = s[3];
    }
}

/// RGBA16 straight → RGBA16 premultiplied: `round(c · a / 65535)`; alpha 0 → `(0,0,0,0)`.
pub fn straight16_to_premul16(src: &[u16], dst: &mut [u16]) {
    let n = pixel_count(src.len(), dst.len());
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)).take(n) {
        let a = s[3] as u64;
        for c in 0..3 {
            d[c] = ((2 * s[c] as u64 * a + 65535) / 131070) as u16;
        }
        d[3] = s[3];
    }
}

/// Straight encoded float color → premultiplied u16 pixel: `c16 = f32_to_u16(c · a)`,
/// `a16 = f32_to_u16(a)` (inputs clamped, NaN → 0). Premultiplication happens in the encoded
/// domain (ADR-005), and `c16 ≤ a16` always holds.
pub fn straight_f32_to_premul16(c: ColorRgba) -> [u16; 4] {
    let a = unit32(c.a) as f64;
    let q = |v: f32| f64_to_u16(unit32(v) as f64 * a);
    let a16 = f64_to_u16(a);
    [q(c.r).min(a16), q(c.g).min(a16), q(c.b).min(a16), a16]
}

/// Premultiplied encoded u16 pixel → premultiplied **linear-light** `f32` (sRGB curve), for
/// linear blend-space compositing. Alpha 0 → all zeros.
pub fn premul16_to_linear_premul_f32(p: [u16; 4]) -> [f32; 4] {
    if p[3] == 0 {
        return [0.0; 4];
    }
    let lut = srgb_u16_to_linear_lut();
    let a = p[3] as f32 / 65535.0;
    let lin = |c: u16| {
        // Straight encoded value at 16-bit precision, then exact table decode.
        let s = unpremul(c, p[3], 65535) as usize;
        lut.get(s).copied().unwrap_or(1.0) * a
    };
    [lin(p[0]), lin(p[1]), lin(p[2]), a]
}

/// Premultiplied linear-light `f32` → premultiplied encoded u16 (sRGB curve). Alpha ≤ 0 or
/// NaN → zeros; straight linear values are clamped to `[0,1]` before encoding.
pub fn linear_premul_f32_to_premul16(p: [f32; 4]) -> [u16; 4] {
    let a = unit32(p[3]) as f64;
    let a16 = f64_to_u16(a);
    if a16 == 0 {
        return [0; 4];
    }
    let enc = |c: f32| {
        let straight = if c.is_nan() { 0.0 } else { (c as f64 / a).clamp(0.0, 1.0) };
        f64_to_u16(linear_to_srgb_f64(straight) * a).min(a16)
    };
    [enc(p[0]), enc(p[1]), enc(p[2]), a16]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_rounding_rules() {
        assert_eq!(f32_to_u16(0.0), 0);
        assert_eq!(f32_to_u16(1.0), 65535);
        assert_eq!(f32_to_u16(0.5), 32768);
        assert_eq!(f32_to_u16(f32::NAN), 0);
        assert_eq!(f32_to_u16(-3.0), 0);
        assert_eq!(f32_to_u16(7.0), 65535);
        for v in 0..=65535u32 {
            let exact = (v as f64 * 255.0 / 65535.0 + 0.5).floor() as u8;
            assert_eq!(u16_to_u8(v as u16), exact, "{v}");
        }
        for v in 0..=255u8 {
            assert_eq!(u16_to_u8(u8_to_u16(v)), v);
        }
    }

    #[test]
    fn straight8_premul16_roundtrip_exhaustive() {
        // Every (channel, alpha) pair; rgb channels share the value so 256*256 pixels.
        let src: Vec<u8> = (0..=255u8).flat_map(|a| (0..=255u8).flat_map(move |c| [c, 255 - c, c / 3, a])).collect();
        let mut p16 = vec![0u16; src.len()];
        straight8_to_premul16(&src, &mut p16);
        let mut back = vec![0u8; src.len()];
        premul16_to_straight8(&p16, &mut back);
        for ((s, p), b) in src.chunks_exact(4).zip(p16.chunks_exact(4)).zip(back.chunks_exact(4)) {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
            if s[3] == 0 {
                assert_eq!(p, [0, 0, 0, 0]);
                assert_eq!(b, [0, 0, 0, 0]);
            } else {
                assert_eq!(s, b, "via {p:?}");
            }
        }
    }

    #[test]
    fn premul16_to_straight8_no_halos_dense() {
        // All 16-bit alphas with representative channel values: never above 255, alpha-0 output
        // is (0,0,0,0), and error vs exact c/a is ≤ 0.5 LSB.
        let mut worst: f64 = 0.0;
        for a in 0..=65535u16 {
            for c in [0, 1, a / 3, a / 2, a.saturating_sub(1), a, a.saturating_add(1)] {
                let src = [c, c / 2, 0, a];
                let mut d = [0u8; 4];
                premul16_to_straight8(&src, &mut d);
                let a8 = u16_to_u8(a);
                if a8 == 0 {
                    assert_eq!(d, [0; 4]);
                    continue;
                }
                assert_eq!(d[3], a8);
                assert_eq!(d[2], 0);
                let exact = (c as f64 / a as f64).min(1.0) * 255.0;
                worst = worst.max((d[0] as f64 - exact).abs());
            }
        }
        assert!(worst <= 0.5 + 1e-9, "{worst}");
    }

    #[test]
    fn premul8_layout() {
        let src = [65535, 32768, 0, 65535, 100, 100, 100, 100, 200, 0, 0, 128, 0, 0, 0, 0];
        let mut d = [9u8; 16];
        premul16_to_premul8(&src, &mut d);
        assert_eq!(&d[0..4], &[255, 128, 0, 255]);
        assert_eq!(&d[4..8], &[0, 0, 0, 0]);
        assert_eq!(&d[8..12], &[0, 0, 0, 0]);
        assert_eq!(&d[12..16], &[0, 0, 0, 0]);
        // c > a violation is clamped.
        let mut e = [0u8; 4];
        premul16_to_premul8(&[65535, 0, 0, 32768], &mut e);
        assert_eq!(e, [128, 0, 0, 128]);
    }

    #[test]
    fn sixteen_bit_straight_roundtrip() {
        let mut worst = 0i64;
        for a in [1u16, 2, 3, 255, 256, 1000, 32767, 32768, 65534, 65535] {
            for c in (0..=65535u16).step_by(97).chain([65535]) {
                let s = [c, 65535 - c, c / 2, a];
                let mut p = [0u16; 4];
                straight16_to_premul16(&s, &mut p);
                assert!(p[0] <= a && p[1] <= a && p[2] <= a);
                let mut b = [0u16; 4];
                premul16_to_straight16(&p, &mut b);
                assert_eq!(b[3], a);
                if a == 65535 {
                    assert_eq!(b, s);
                }
                // Expected precision loss is bounded by the premultiplied quantum 65535/a.
                let tol = (65535.0 / a as f64 / 2.0).ceil() as i64;
                for i in 0..3 {
                    let err = (b[i] as i64 - s[i] as i64).abs();
                    assert!(err <= tol, "a={a} c={c} {s:?} -> {p:?} -> {b:?}");
                    worst = worst.max(err - tol);
                }
            }
        }
        let mut z = [5u16; 4];
        premul16_to_straight16(&[3, 2, 1, 0], &mut z);
        assert_eq!(z, [0; 4]);
        assert!(worst <= 0);
    }

    #[test]
    fn color_rgba_to_premul16() {
        assert_eq!(straight_f32_to_premul16(ColorRgba::WHITE), [65535; 4]);
        assert_eq!(straight_f32_to_premul16(ColorRgba::TRANSPARENT), [0; 4]);
        assert_eq!(straight_f32_to_premul16(ColorRgba::new(1.0, 0.5, 0.0, 0.5)), [32768, 16384, 0, 32768]);
        assert_eq!(straight_f32_to_premul16(ColorRgba::new(f32::NAN, 2.0, -1.0, 1.0)), [0, 65535, 0, 65535]);
        let p = straight_f32_to_premul16(ColorRgba::new(1.0, 1.0, 1.0, 0.01));
        assert!(p[0] <= p[3] && p[3] == 655);
    }

    #[test]
    fn linear_roundtrip() {
        assert_eq!(premul16_to_linear_premul_f32([10, 10, 10, 0]), [0.0; 4]);
        assert_eq!(linear_premul_f32_to_premul16([0.5, 0.5, 0.5, 0.0]), [0; 4]);
        let mut worst = 0i32;
        for a in [65535u16, 32768, 655, 257] {
            for c in (0..=a).step_by((a as usize / 200).max(1)) {
                let p = [c, a / 2, 0, a];
                let lin = premul16_to_linear_premul_f32(p);
                assert!(lin.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0));
                let back = linear_premul_f32_to_premul16(lin);
                assert_eq!(back[3], a);
                for i in 0..3 {
                    worst = worst.max((back[i] as i32 - p[i] as i32).abs());
                }
            }
        }
        // f32 linear storage + premultiplied quantization: ≤ 2 codes at 16 bit.
        assert!(worst <= 2, "{worst}");
        // Mid-gray: encoded 0.5 ≈ linear 0.214.
        let l = premul16_to_linear_premul_f32([32768, 32768, 32768, 65535]);
        assert!((l[0] - 0.2140).abs() < 1e-3);
    }

    #[test]
    fn mismatched_lengths_process_common_prefix_in_release() {
        // Equal pixel counts but trailing partial pixel: the partial pixel is ignored.
        let src = [65535u16, 0, 0, 65535, 1, 2];
        let mut dst = [7u8; 6];
        premul16_to_straight8(&src, &mut dst);
        assert_eq!(dst, [255, 0, 0, 255, 7, 7]);
    }
}
