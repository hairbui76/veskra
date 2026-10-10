//! Matrix/TRC color transforms: source device RGB → PCS XYZ (D50) → destination device RGB.
//!
//! # Rendering intents
//! * **Relative colorimetric**: PCS XYZ passed through unchanged (media white → media white,
//!   because matrix/TRC colorants are already adapted to D50).
//! * **Perceptual / Saturation**: matrix/TRC profiles carry no perceptual or saturation
//!   rendering tables, so these intents use the colorimetric model and behave like **relative
//!   colorimetric**. This is what LittleCMS does for matrix-shaper profiles; LittleCMS
//!   additionally enables black-point compensation for v4 perceptual transforms, which is a
//!   no-op for profiles whose TRCs map 0 → 0 (all profiles Veskra generates). Profiles with a
//!   raised black point may differ from LittleCMS's perceptual result near black.
//! * **Absolute colorimetric**: PCS XYZ is scaled per component by
//!   `src_media_white / dst_media_white` (ICC v4 / LittleCMS with full adaptation), using
//!   [`IccProfile::effective_media_white`]. For v4 display profiles (media white = D50) this
//!   equals relative colorimetric.
//!
//! # Out-of-gamut handling
//! Destination linear RGB is **clipped per channel to `[0, 1]`** before re-encoding. No gamut
//! mapping or hue preservation is attempted (e.g. saturated Display P3 green → sRGB clips to the
//! sRGB gamut boundary along each channel independently). This matches LittleCMS's behaviour for
//! integer output of matrix-shaper transforms.
//!
//! # Identity detection
//! When source and destination are colorimetrically equivalent (colorants within
//! [`IDENTITY_MATRIX_TOL`], curves within [`IDENTITY_CURVE_TOL`] in the encoded domain, and for
//! absolute intent equal media whites), the transform is an **exact passthrough**: buffers are
//! untouched, so sRGB → sRGB at 8 or 16 bits is bit-exact (COL-002).
//!
//! # Precision
//! All math is `f64`. 8-bit input uses a 256-entry decode table per channel; 8-bit output uses
//! exact encoded-domain rounding (binary search over the 255 decision thresholds, i.e.
//! `round(enc · 255)` with ties rounded up).

use crate::error::ColorError;
use crate::icc::{IccProfile, ProfileColorSpace};
use crate::math::{self, Mat3};
use crate::transfer::ToneCurve;

/// Colorant tolerance (absolute XYZ) under which two profiles are treated as identical.
pub const IDENTITY_MATRIX_TOL: f64 = 5e-4;
/// Encoded-domain curve round-trip tolerance under which curves are treated as identical
/// (≈ 0.13 LSB at 8 bits).
pub const IDENTITY_CURVE_TOL: f64 = 5e-4;

/// A destination linear-light channel must leave `[0, 1]` by more than this before a color counts
/// as out of gamut. ICC colorants are stored as s15Fixed16 (steps of 1.5e-5), so two profiles
/// that share a primary (sRGB / Display P3 / Adobe RGB blue, sRGB / Adobe RGB red) map it to a
/// channel value of about ±3e-5 instead of exactly 0; the tolerance has to sit above that noise.
/// 1e-4 in linear light is below 0.1 ΔE00 and at most 1/3 of an 8-bit code.
pub const GAMUT_CLIP_EPS: f64 = 1e-4;

/// ICC rendering intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RenderingIntent {
    Perceptual,
    #[default]
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl RenderingIntent {
    /// Stable machine name (matches the color report JSON, e.g. `"relative-colorimetric"`).
    pub fn as_str(self) -> &'static str {
        match self {
            RenderingIntent::Perceptual => "perceptual",
            RenderingIntent::RelativeColorimetric => "relative-colorimetric",
            RenderingIntent::Saturation => "saturation",
            RenderingIntent::AbsoluteColorimetric => "absolute-colorimetric",
        }
    }

    /// ICC header/LittleCMS numeric code (0–3).
    pub fn icc_code(self) -> u32 {
        match self {
            RenderingIntent::Perceptual => 0,
            RenderingIntent::RelativeColorimetric => 1,
            RenderingIntent::Saturation => 2,
            RenderingIntent::AbsoluteColorimetric => 3,
        }
    }
}

/// Transform construction options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransformOptions {
    /// Use exact passthrough for colorimetrically equivalent profiles (default `true`).
    /// Disabling it forces the full decode → matrix → encode pipeline (used by tests to measure
    /// the numeric pipeline itself).
    pub detect_identity: bool,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self { detect_identity: true }
    }
}

fn rgb_parts(p: &IccProfile) -> Result<(Mat3, [ToneCurve; 3]), ColorError> {
    if p.color_space() != ProfileColorSpace::Rgb {
        return Err(ColorError::WrongProfileKind("RgbTransform requires RGB profiles"));
    }
    let trc = [p.trc(0), p.trc(1), p.trc(2)];
    match trc {
        [Some(r), Some(g), Some(b)] => Ok((p.to_xyz_d50_matrix(), [r.clone(), g.clone(), b.clone()])),
        _ => Err(ColorError::WrongProfileKind("RGB profile without three curves")),
    }
}

fn absolute_scale(src: &IccProfile, dst: &IccProfile) -> Result<Mat3, ColorError> {
    let (ws, wd) = (src.effective_media_white(), dst.effective_media_white());
    if wd.iter().any(|v| v.is_nan() || *v <= 0.0) {
        return Err(ColorError::TransformFailure("destination media white has a non-positive component"));
    }
    Ok(math::diag([ws[0] / wd[0], ws[1] / wd[1], ws[2] / wd[2]]))
}

fn destination_inverse(dst: &IccProfile, curves: &[ToneCurve; 3]) -> Result<Mat3, ColorError> {
    if !curves.iter().all(ToneCurve::is_invertible) {
        return Err(ColorError::TransformFailure("destination tone curve is not invertible"));
    }
    math::invert(&dst.to_xyz_d50_matrix()).ok_or(ColorError::TransformFailure("destination matrix is singular"))
}

#[inline]
fn sanitize(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// RGB → RGB matrix/TRC transform.
#[derive(Debug, Clone)]
pub struct RgbTransform {
    intent: RenderingIntent,
    identity: bool,
    matrix: Mat3,
    src_trc: [ToneCurve; 3],
    dst_trc: [ToneCurve; 3],
    /// `src_lut8[c][v]` = linear value of 8-bit code `v` in channel `c`.
    src_lut8: Box<[[f64; 256]; 3]>,
    /// `thresholds8[c][k]` = linear value whose encoding is exactly `(k + 0.5) / 255`.
    thresholds8: Box<[[f64; 255]; 3]>,
}

impl RgbTransform {
    /// Build a transform with default options (identity detection on).
    pub fn new(src: &IccProfile, dst: &IccProfile, intent: RenderingIntent) -> Result<Self, ColorError> {
        Self::with_options(src, dst, intent, TransformOptions::default())
    }

    /// Build a transform with explicit options.
    pub fn with_options(
        src: &IccProfile,
        dst: &IccProfile,
        intent: RenderingIntent,
        options: TransformOptions,
    ) -> Result<Self, ColorError> {
        let (src_m, src_trc) = rgb_parts(src)?;
        let (_, dst_trc) = rgb_parts(dst)?;
        let dst_inv = destination_inverse(dst, &dst_trc)?;
        let pcs = match intent {
            RenderingIntent::AbsoluteColorimetric => absolute_scale(src, dst)?,
            _ => math::IDENTITY,
        };
        let matrix = math::mul(&dst_inv, &math::mul(&pcs, &src_m));
        if matrix.iter().flatten().any(|v| !v.is_finite()) {
            return Err(ColorError::TransformFailure("non-finite combined matrix"));
        }
        let whites_equal = intent != RenderingIntent::AbsoluteColorimetric
            || src
                .effective_media_white()
                .iter()
                .zip(dst.effective_media_white())
                .all(|(a, b)| (a - b).abs() <= IDENTITY_MATRIX_TOL);
        let identity = options.detect_identity
            && whites_equal
            && src.is_colorimetrically_equivalent(dst, IDENTITY_MATRIX_TOL, IDENTITY_CURVE_TOL);

        let src_lut8 = Box::new(std::array::from_fn(|c| std::array::from_fn(|v| src_trc[c].eval(v as f64 / 255.0))));
        let thresholds8 =
            Box::new(std::array::from_fn(|c| std::array::from_fn(|k| dst_trc[c].eval((k as f64 + 0.5) / 255.0))));
        Ok(Self { intent, identity, matrix, src_trc, dst_trc, src_lut8, thresholds8 })
    }

    /// Whether this transform is an exact passthrough.
    pub fn is_identity(&self) -> bool {
        self.identity
    }

    /// Intent requested at construction.
    pub fn intent(&self) -> RenderingIntent {
        self.intent
    }

    /// Combined linear matrix `dst⁻¹ · scale · src` (row-major).
    pub fn linear_matrix(&self) -> [[f64; 3]; 3] {
        self.matrix
    }

    #[inline]
    fn linear_to_dst_linear(&self, lin: [f64; 3]) -> [f64; 3] {
        let d = math::mul_vec(&self.matrix, lin);
        [d[0].clamp(0.0, 1.0), d[1].clamp(0.0, 1.0), d[2].clamp(0.0, 1.0)]
    }

    /// Transform one encoded RGB triple in `f64` (inputs clamped to `[0,1]`, NaN → 0).
    pub fn apply_f64(&self, rgb: [f64; 3]) -> [f64; 3] {
        let rgb = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        if self.identity {
            return rgb;
        }
        let lin = [self.src_trc[0].eval(rgb[0]), self.src_trc[1].eval(rgb[1]), self.src_trc[2].eval(rgb[2])];
        let d = self.linear_to_dst_linear(lin);
        [self.dst_trc[0].eval_inverse(d[0]), self.dst_trc[1].eval_inverse(d[1]), self.dst_trc[2].eval_inverse(d[2])]
    }

    /// [`apply_f64`](Self::apply_f64) that also reports whether the color was outside the
    /// destination gamut (a destination linear channel beyond `[0, 1]` by more than
    /// [`GAMUT_CLIP_EPS`]) and therefore clipped. Always `false` for identity transforms.
    pub fn apply_f64_checked(&self, rgb: [f64; 3]) -> ([f64; 3], bool) {
        let rgb = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        if self.identity {
            return (rgb, false);
        }
        let lin = [self.src_trc[0].eval(rgb[0]), self.src_trc[1].eval(rgb[1]), self.src_trc[2].eval(rgb[2])];
        let raw = math::mul_vec(&self.matrix, lin);
        let clipped = raw.iter().any(|v| *v < -GAMUT_CLIP_EPS || *v > 1.0 + GAMUT_CLIP_EPS);
        let d = [raw[0].clamp(0.0, 1.0), raw[1].clamp(0.0, 1.0), raw[2].clamp(0.0, 1.0)];
        (
            [
                self.dst_trc[0].eval_inverse(d[0]),
                self.dst_trc[1].eval_inverse(d[1]),
                self.dst_trc[2].eval_inverse(d[2]),
            ],
            clipped,
        )
    }

    /// Whether every source color is inside the destination gamut, i.e. no input can be clipped:
    /// all three source primaries map to non-negative destination linear RGB (white maps to
    /// white, so the whole source gamut then lies inside the destination's). Identity → `true`.
    pub fn source_gamut_fits(&self) -> bool {
        self.identity || self.matrix.iter().flatten().all(|v| *v >= -GAMUT_CLIP_EPS)
    }

    /// Transform one encoded RGB triple (encoded in → encoded out, inputs clamped to `[0,1]`).
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.identity {
            return rgb.map(sanitize);
        }
        self.apply_f64(rgb.map(|v| v as f64)).map(|v| v as f32)
    }

    #[inline]
    fn encode8(&self, c: usize, lin: f64) -> u8 {
        // Number of decision thresholds at or below `lin` = round-half-up of enc * 255.
        self.thresholds8[c].partition_point(|&t| t <= lin) as u8
    }

    /// In-place transform of RGBA8 **straight-alpha** pixels (`len % 4` trailing bytes are left
    /// untouched). Alpha is preserved; color is transformed even when alpha is 0.
    pub fn apply_rgba8_straight(&self, px: &mut [u8]) {
        if self.identity {
            return;
        }
        for p in px.chunks_exact_mut(4) {
            let lin =
                [self.src_lut8[0][p[0] as usize], self.src_lut8[1][p[1] as usize], self.src_lut8[2][p[2] as usize]];
            let d = self.linear_to_dst_linear(lin);
            p[0] = self.encode8(0, d[0]);
            p[1] = self.encode8(1, d[1]);
            p[2] = self.encode8(2, d[2]);
        }
    }

    /// In-place transform of RGBA16 **premultiplied** pixels (Veskra working layout).
    ///
    /// Per pixel: alpha 0 → `(0,0,0,0)`; otherwise unpremultiply in `f64` (clamped to 1, which
    /// also repairs `c > a` violations), transform, re-premultiply with round-half-up and clamp
    /// so `r,g,b ≤ a` holds. Alpha is never changed. `len % 4` trailing values are untouched.
    pub fn apply_rgba16_premul(&self, px: &mut [u16]) {
        if self.identity {
            return;
        }
        for p in px.chunks_exact_mut(4) {
            let a = p[3];
            if a == 0 {
                p[..3].fill(0);
                continue;
            }
            let af = a as f64;
            let straight = [(p[0] as f64 / af).min(1.0), (p[1] as f64 / af).min(1.0), (p[2] as f64 / af).min(1.0)];
            let enc = self.apply_f64(straight);
            for c in 0..3 {
                p[c] = ((enc[c] * af + 0.5).floor() as u16).min(a);
            }
        }
    }
}

impl RgbTransform {
    /// [`apply_rgba16_premul`](Self::apply_rgba16_premul) that returns how many pixels with
    /// alpha > 0 were outside the destination gamut and clipped (see
    /// [`apply_f64_checked`](Self::apply_f64_checked)). Pixel results are identical.
    pub fn apply_rgba16_premul_counting(&self, px: &mut [u16]) -> u64 {
        if self.identity {
            return 0;
        }
        let mut clipped = 0u64;
        for p in px.chunks_exact_mut(4) {
            let a = p[3];
            if a == 0 {
                p[..3].fill(0);
                continue;
            }
            let af = a as f64;
            let straight = [(p[0] as f64 / af).min(1.0), (p[1] as f64 / af).min(1.0), (p[2] as f64 / af).min(1.0)];
            let (enc, c) = self.apply_f64_checked(straight);
            clipped += c as u64;
            for k in 0..3 {
                p[k] = ((enc[k] * af + 0.5).floor() as u16).min(a);
            }
        }
        clipped
    }
}

/// Gray TRC profile → RGB profile transform (e.g. importing a tagged grayscale PNG).
#[derive(Debug, Clone)]
pub struct GrayToRgbTransform {
    src_trc: ToneCurve,
    scale: Mat3,
    dst_inv: Mat3,
    dst_trc: [ToneCurve; 3],
}

impl GrayToRgbTransform {
    /// Gray source maps onto the neutral axis: PCS XYZ = `kTRC(g) · D50` (relative colorimetric;
    /// perceptual/saturation behave the same; absolute applies the media-white scaling).
    pub fn new(src: &IccProfile, dst: &IccProfile, intent: RenderingIntent) -> Result<Self, ColorError> {
        if src.color_space() != ProfileColorSpace::Gray {
            return Err(ColorError::WrongProfileKind("GrayToRgbTransform requires a Gray source profile"));
        }
        let src_trc = src.trc(0).cloned().ok_or(ColorError::WrongProfileKind("gray profile without kTRC"))?;
        let (_, dst_trc) = rgb_parts(dst)?;
        let dst_inv = destination_inverse(dst, &dst_trc)?;
        let scale = match intent {
            RenderingIntent::AbsoluteColorimetric => absolute_scale(src, dst)?,
            _ => math::IDENTITY,
        };
        Ok(Self { src_trc, scale, dst_inv, dst_trc })
    }

    /// Encoded gray → encoded destination RGB (clipped).
    pub fn apply(&self, gray: f32) -> [f32; 3] {
        let y = self.src_trc.eval(if gray.is_nan() { 0.0 } else { gray as f64 });
        let xyz = math::mul_vec(&self.scale, [math::D50_XYZ[0] * y, y, math::D50_XYZ[2] * y]);
        let d = math::mul_vec(&self.dst_inv, xyz);
        std::array::from_fn(|c| self.dst_trc[c].eval_inverse(d[c].clamp(0.0, 1.0)) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icc::IccProfile;

    fn full(src: &IccProfile, dst: &IccProfile) -> RgbTransform {
        RgbTransform::with_options(
            src,
            dst,
            RenderingIntent::RelativeColorimetric,
            TransformOptions { detect_identity: false },
        )
        .unwrap()
    }

    #[test]
    fn srgb_identity_is_bit_exact() {
        let s = IccProfile::srgb();
        for intent in [
            RenderingIntent::Perceptual,
            RenderingIntent::RelativeColorimetric,
            RenderingIntent::Saturation,
            RenderingIntent::AbsoluteColorimetric,
        ] {
            let t = RgbTransform::new(&s, &s, intent).unwrap();
            assert!(t.is_identity());
            let mut px: Vec<u8> = (0..=255u8).flat_map(|v| [v, 255 - v, v / 2, v]).collect();
            let before = px.clone();
            t.apply_rgba8_straight(&mut px);
            assert_eq!(px, before);
            let mut p16: Vec<u16> = vec![100, 200, 300, 400, 0, 0, 0, 0, 65535, 0, 1, 65535];
            let b16 = p16.clone();
            t.apply_rgba16_premul(&mut p16);
            assert_eq!(p16, b16);
            assert_eq!(t.apply([0.25, f32::NAN, 2.0]), [0.25, 0.0, 1.0]);
        }
    }

    #[test]
    fn full_pipeline_srgb_to_srgb_is_exact_for_all_8bit_codes() {
        let s = IccProfile::srgb();
        let t = full(&s, &s);
        assert!(!t.is_identity());
        let mut px: Vec<u8> = (0..=255u8).flat_map(|v| [v, v, v, 255]).collect();
        let before = px.clone();
        t.apply_rgba8_straight(&mut px);
        assert_eq!(px, before);
    }

    #[test]
    fn srgb_p3_roundtrip_in_gamut() {
        let (s, p) = (IccProfile::srgb(), IccProfile::display_p3());
        let to = RgbTransform::new(&s, &p, RenderingIntent::RelativeColorimetric).unwrap();
        let back = RgbTransform::new(&p, &s, RenderingIntent::RelativeColorimetric).unwrap();
        assert!(!to.is_identity());
        for c in [[1.0, 0.0, 0.0], [0.2, 0.5, 0.8], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]] {
            let r = back.apply_f64(to.apply_f64(c));
            for i in 0..3 {
                assert!((r[i] - c[i]).abs() < 1e-9, "{c:?} -> {r:?}");
            }
        }
        // sRGB red is inside P3: published value ≈ (0.9175, 0.2003, 0.1386).
        let red = to.apply_f64([1.0, 0.0, 0.0]);
        assert!((red[0] - 0.9175).abs() < 1e-3 && (red[1] - 0.2003).abs() < 1e-3 && (red[2] - 0.1386).abs() < 1e-3);
        // P3 green is outside sRGB and clips.
        let g = back.apply_f64([0.0, 1.0, 0.0]);
        assert_eq!(g[1], 1.0);
        assert_eq!(g[0], 0.0);
        // White maps to white exactly in 8 bit.
        let mut w = [255u8, 255, 255, 255, 0, 0, 0, 0];
        to.apply_rgba8_straight(&mut w);
        assert_eq!(w, [255, 255, 255, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn rgba16_premul_keeps_invariants() {
        let to =
            RgbTransform::new(&IccProfile::srgb(), &IccProfile::display_p3(), RenderingIntent::Perceptual).unwrap();
        let mut px = vec![
            0, 0, 0, 0, // transparent
            123, 4, 9, 0, // garbage under alpha 0 → cleared
            65535, 0, 0, 65535, // opaque red
            300, 0, 0, 300, // faint red
            1, 1, 1, 1, // tiny alpha
            5000, 6000, 7000, 4000, // violates c ≤ a → clamped
        ];
        to.apply_rgba16_premul(&mut px);
        for p in px.chunks_exact(4) {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
        }
        assert_eq!(&px[0..8], &[0; 8]);
        assert_eq!(px[11], 65535);
        assert_eq!(px[15], 300);
        // Opaque red equals the f64 transform.
        let r = to.apply_f64([1.0, 0.0, 0.0]);
        assert_eq!(px[8], (r[0] * 65535.0 + 0.5).floor() as u16);
        // Faint red keeps hue: the red channel stays the largest.
        assert!(px[12] > px[13] && px[12] > px[14]);
    }

    #[test]
    fn eight_bit_path_matches_f64_rounding() {
        let to =
            RgbTransform::new(&IccProfile::srgb(), &IccProfile::display_p3(), RenderingIntent::RelativeColorimetric)
                .unwrap();
        let mut worst = 0i32;
        for r in (0..=255).step_by(5) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(17) {
                    let mut px = [r as u8, g as u8, b as u8, 255];
                    to.apply_rgba8_straight(&mut px);
                    let f = to.apply_f64([r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0]);
                    for c in 0..3 {
                        let q = (f[c] * 255.0 + 0.5).floor() as i32;
                        worst = worst.max((q - px[c] as i32).abs());
                    }
                }
            }
        }
        assert_eq!(worst, 0);
    }

    #[test]
    fn absolute_intent_scales_by_media_white() {
        let s = IccProfile::srgb();
        let t = RgbTransform::new(&s, &s, RenderingIntent::AbsoluteColorimetric).unwrap();
        assert!(t.is_identity(), "v4 display profiles have D50 media white");
        assert_eq!(RenderingIntent::AbsoluteColorimetric.as_str(), "absolute-colorimetric");
        assert_eq!(RenderingIntent::Saturation.icc_code(), 2);
    }

    #[test]
    fn gamut_containment_and_clip_counting() {
        let (s, p, a) = (IccProfile::srgb(), IccProfile::display_p3(), IccProfile::adobe_rgb_compatible());
        let rel = RenderingIntent::RelativeColorimetric;
        // sRGB is inside both wide gamuts; the reverse directions (and P3 <-> Adobe) can clip.
        assert!(RgbTransform::new(&s, &p, rel).unwrap().source_gamut_fits());
        assert!(RgbTransform::new(&s, &a, rel).unwrap().source_gamut_fits());
        assert!(RgbTransform::new(&s, &s, rel).unwrap().source_gamut_fits());
        for (src, dst) in [(&p, &s), (&a, &s), (&p, &a), (&a, &p)] {
            assert!(!RgbTransform::new(src, dst, rel).unwrap().source_gamut_fits());
        }
        // Counting variant: identical pixels, exact count of clipped (alpha > 0) pixels.
        let back = RgbTransform::new(&p, &s, rel).unwrap();
        let mut px = vec![
            0, 65535, 0, 65535, // P3 green: outside sRGB
            0, 20000, 0, 20000, // the same color at 30 % alpha: still outside
            30000, 30000, 30000, 65535, // gray: inside
            65535, 65535, 65535, 65535, // white: inside (maps to white)
            0, 0, 0, 0, // transparent: never counted
            65535, 0, 0, 65535, // P3 red: outside
        ];
        let mut plain = px.clone();
        back.apply_rgba16_premul(&mut plain);
        assert_eq!(back.apply_rgba16_premul_counting(&mut px), 3);
        assert_eq!(px, plain);
        // No sRGB color is reported as clipped on the way into a wider gamut, including the
        // primaries the profiles share (their ICC colorants differ only by s15Fixed16 rounding).
        for dst in [&p, &a] {
            let t = RgbTransform::new(&s, dst, rel).unwrap();
            for r in (0..=255).step_by(51) {
                for g in (0..=255).step_by(51) {
                    for b in (0..=255).step_by(51) {
                        let (_, clipped) = t.apply_f64_checked([r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0]);
                        assert!(!clipped, "({r},{g},{b})");
                    }
                }
            }
        }
        let id = RgbTransform::new(&s, &s, rel).unwrap();
        assert_eq!(id.apply_rgba16_premul_counting(&mut px), 0);
        assert_eq!(id.apply_f64_checked([0.25, 0.5, 2.0]), ([0.25, 0.5, 1.0], false));
    }

    #[test]
    fn adobe_rgb_compatible_profile_is_deterministic_and_round_trips() {
        let a = IccProfile::adobe_rgb_compatible();
        let again = crate::icc::build_rgb_profile_bytes(&crate::icc::RgbProfileSpec::adobe_rgb_compatible()).unwrap();
        assert_eq!(a.bytes(), again.as_slice());
        let parsed = IccProfile::parse(a.bytes()).unwrap();
        assert_eq!(parsed.id(), a.id());
        // Pure gamma 563/256 survives the s15Fixed16 encoding exactly.
        assert!(
            matches!(parsed.trc(0), Some(crate::transfer::ToneCurve::Gamma(g)) if *g == 563.0 / 256.0)
                || parsed.trc(0).is_some_and(|c| (c.eval(0.5) - 0.5f64.powf(563.0 / 256.0)).abs() < 1e-12)
        );
        let (to, back) = (full(&IccProfile::srgb(), &a), full(&a, &IccProfile::srgb()));
        for c in [[0.2, 0.5, 0.8], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [0.9, 0.1, 0.4]] {
            let r = back.apply_f64(to.apply_f64(c));
            for i in 0..3 {
                assert!((r[i] - c[i]).abs() < 1e-9, "{c:?} -> {r:?}");
            }
        }
    }

    #[test]
    fn gray_transform() {
        let gray = {
            let mut b = vec![0u8; 128];
            b[8] = 4;
            b[12..16].copy_from_slice(b"mntr");
            b[16..20].copy_from_slice(b"GRAY");
            b[20..24].copy_from_slice(b"XYZ ");
            b[36..40].copy_from_slice(b"acsp");
            b.extend_from_slice(&1u32.to_be_bytes());
            b.extend_from_slice(b"kTRC");
            b.extend_from_slice(&144u32.to_be_bytes());
            b.extend_from_slice(&32u32.to_be_bytes());
            // para type 3 sRGB curve
            b.extend_from_slice(b"para\0\0\0\0\0\x03\0\0");
            for v in crate::transfer::ParametricCurve::SRGB.params() {
                b.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
            }
            let n = b.len() as u32;
            b[0..4].copy_from_slice(&n.to_be_bytes());
            IccProfile::parse(&b).unwrap()
        };
        let t = GrayToRgbTransform::new(&gray, &IccProfile::srgb(), RenderingIntent::RelativeColorimetric).unwrap();
        for g in [0.0f32, 0.2, 0.5, 1.0] {
            let rgb = t.apply(g);
            for c in rgb {
                assert!((c - g).abs() < 2e-4, "{g} -> {rgb:?}");
            }
        }
        assert!(matches!(
            RgbTransform::new(&gray, &IccProfile::srgb(), RenderingIntent::Perceptual),
            Err(ColorError::WrongProfileKind(_))
        ));
        assert!(
            GrayToRgbTransform::new(&IccProfile::srgb(), &IccProfile::srgb(), RenderingIntent::Perceptual).is_err()
        );
    }
}
