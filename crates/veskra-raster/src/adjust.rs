//! Color adjustments on premultiplied RGBA16 tiles (EDIT: `edit.adjust`).
//!
//! All adjustments act on **straight (un-premultiplied), transfer-encoded** channel values, the
//! same numbers a color picker shows; alpha is never changed.
//!
//! Per pixel with alpha `a > 0`: `s = round(c · 65535 / a)` → adjustment → `c' = round(s' · a /
//! 65535)` (clamped to `a`, so `r,g,b ≤ a` holds). With partial region coverage `k` the result is
//! `c + round((c' − c) · k / 255)`. [`Adjustment::Invert`] skips the round trip and uses the exact
//! premultiplied identity `c' = a − c`, so inverting twice restores every pixel bit-exactly.
//! Fully transparent pixels are untouched. At very low alpha the stored premultiplied value has
//! only `a` distinct levels, which bounds the precision of any adjustment there.
//!
//! * **Brightness/contrast**: `v' = (v − 0.5) · k + 0.5 + brightness`, `k = (1 + c) / (1 − c)`
//!   (`c = −1` → flat mid-gray, `c = 1` → threshold), clamped to `[0, 1]`.
//! * **HSB**: RGB → HSV, `H' = H + hue`, `S' = clamp(S · (1 + saturation))`,
//!   `V' = V + (1 − V) · b` for `b ≥ 0` and `V · (1 + b)` for `b < 0`, → RGB.
//! * **Curves**: monotone cubic (Fritsch–Carlson) interpolation through the control points; the
//!   curve is constant before the first and after the last point. The `rgb` curve is applied
//!   first, then the per-channel curve.
//!
//! Only `+ − × ÷`, `sqrt`, `floor` and comparisons are used (all correctly rounded IEEE-754
//! operations, no transcendental functions), so results are bit-identical across runs and platforms.

use std::sync::Arc;

use veskra_core::RectI;

use crate::{MaskGrid, RasterError, TILE_PIXELS, TILE_U, Tile, TileCoord, TileGrid};

/// Maximum control points per curve.
pub const MAX_CURVE_POINTS: usize = 64;
const LUT_LEN: usize = 65536;

/// A validated adjustment. Build with the constructors.
#[derive(Clone, PartialEq)]
pub enum Adjustment {
    Invert,
    /// One 65536-entry straight-value lookup table per channel.
    Luts(Box<[Vec<u16>; 3]>),
    Hsb {
        /// Hue rotation as a fraction of a full turn, in `[-0.5, 0.5]`.
        hue_turns: f64,
        saturation: f64,
        brightness: f64,
    },
}

impl std::fmt::Debug for Adjustment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Adjustment::Invert => f.write_str("Invert"),
            Adjustment::Luts(_) => f.write_str("Luts"),
            Adjustment::Hsb { hue_turns, saturation, brightness } => {
                write!(f, "Hsb({hue_turns}, {saturation}, {brightness})")
            }
        }
    }
}

fn in_range(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && v >= lo && v <= hi
}

fn q16(v: f64) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5).floor() as u16
}

fn identity_lut() -> Vec<u16> {
    (0..LUT_LEN).map(|i| i as u16).collect()
}

/// Monotone cubic interpolation through sorted control points, sampled into a LUT.
fn curve_lut(points: &[[f64; 2]]) -> Result<Vec<u16>, RasterError> {
    if points.len() < 2 {
        return Err(RasterError::InvalidArgument("a curve needs at least 2 control points"));
    }
    if points.len() > MAX_CURVE_POINTS {
        return Err(RasterError::InvalidArgument("a curve accepts at most 64 control points"));
    }
    if points.iter().any(|p| !in_range(p[0], 0.0, 1.0) || !in_range(p[1], 0.0, 1.0)) {
        return Err(RasterError::InvalidArgument("curve control points must be finite and within [0, 1]"));
    }
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    if pts.windows(2).any(|w| w[1][0] - w[0][0] < 1e-6) {
        return Err(RasterError::InvalidArgument("curve control points need distinct input values"));
    }
    let n = pts.len();
    // Secant slopes and Fritsch–Carlson tangents (monotone on every monotone data interval).
    let d: Vec<f64> = pts.windows(2).map(|w| (w[1][1] - w[0][1]) / (w[1][0] - w[0][0])).collect();
    let mut m = vec![0.0f64; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) * 0.5 };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
        let s = a * a + b * b;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            m[i] = tau * a * d[i];
            m[i + 1] = tau * b * d[i];
        }
    }
    let mut lut = Vec::with_capacity(LUT_LEN);
    let mut seg = 0usize;
    for i in 0..LUT_LEN {
        let x = i as f64 / 65535.0;
        let y = if x <= pts[0][0] {
            pts[0][1]
        } else if x >= pts[n - 1][0] {
            pts[n - 1][1]
        } else {
            while seg + 2 < n && x > pts[seg + 1][0] {
                seg += 1;
            }
            let h = pts[seg + 1][0] - pts[seg][0];
            let t = (x - pts[seg][0]) / h;
            let (t2, t3) = (t * t, t * t * t);
            let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
            let h10 = t3 - 2.0 * t2 + t;
            let h01 = -2.0 * t3 + 3.0 * t2;
            let h11 = t3 - t2;
            h00 * pts[seg][1] + h10 * h * m[seg] + h01 * pts[seg + 1][1] + h11 * h * m[seg + 1]
        };
        lut.push(q16(y));
    }
    Ok(lut)
}

impl Adjustment {
    /// `brightness` and `contrast` in `[-1, 1]` (see module docs for the formula).
    pub fn brightness_contrast(brightness: f64, contrast: f64) -> Result<Self, RasterError> {
        if !in_range(brightness, -1.0, 1.0) || !in_range(contrast, -1.0, 1.0) {
            return Err(RasterError::InvalidArgument("brightness and contrast must be within [-1, 1]"));
        }
        let k = (1.0 + contrast) / (1.0 - contrast).max(1e-9);
        let lut: Vec<u16> = (0..LUT_LEN).map(|i| q16((i as f64 / 65535.0 - 0.5) * k + 0.5 + brightness)).collect();
        Ok(Adjustment::Luts(Box::new([lut.clone(), lut.clone(), lut])))
    }

    /// Curves: `rgb` applies to all channels, then `r`/`g`/`b` to their channel. At least one
    /// curve is required; each needs 2..=64 points `[input, output]` in `[0, 1]` with distinct
    /// inputs (any order).
    pub fn curves(
        rgb: Option<&[[f64; 2]]>,
        r: Option<&[[f64; 2]]>,
        g: Option<&[[f64; 2]]>,
        b: Option<&[[f64; 2]]>,
    ) -> Result<Self, RasterError> {
        if rgb.is_none() && r.is_none() && g.is_none() && b.is_none() {
            return Err(RasterError::InvalidArgument("curves needs at least one of rgb, r, g, b"));
        }
        let master = rgb.map(curve_lut).transpose()?;
        let build = |ch: Option<&[[f64; 2]]>| -> Result<Vec<u16>, RasterError> {
            let own = ch.map(curve_lut).transpose()?;
            Ok(match (&master, own) {
                (None, None) => identity_lut(),
                (Some(m), None) => m.clone(),
                (None, Some(o)) => o,
                (Some(m), Some(o)) => m.iter().map(|&v| o[v as usize]).collect(),
            })
        };
        Ok(Adjustment::Luts(Box::new([build(r)?, build(g)?, build(b)?])))
    }

    /// `hue_degrees` in `[-180, 180]`, `saturation` and `brightness` in `[-1, 1]`.
    pub fn hsb(hue_degrees: f64, saturation: f64, brightness: f64) -> Result<Self, RasterError> {
        if !in_range(hue_degrees, -180.0, 180.0) {
            return Err(RasterError::InvalidArgument("hue must be within [-180, 180] degrees"));
        }
        if !in_range(saturation, -1.0, 1.0) || !in_range(brightness, -1.0, 1.0) {
            return Err(RasterError::InvalidArgument("saturation and brightness must be within [-1, 1]"));
        }
        Ok(Adjustment::Hsb { hue_turns: hue_degrees / 360.0, saturation, brightness })
    }

    /// Map one straight RGB16 triple.
    pub fn map(&self, s: [u16; 3]) -> [u16; 3] {
        match self {
            Adjustment::Invert => [65535 - s[0], 65535 - s[1], 65535 - s[2]],
            Adjustment::Luts(l) => [l[0][s[0] as usize], l[1][s[1] as usize], l[2][s[2] as usize]],
            Adjustment::Hsb { hue_turns, saturation, brightness } => hsb_map(s, *hue_turns, *saturation, *brightness),
        }
    }
}

fn hsb_map(s: [u16; 3], hue_turns: f64, sat: f64, bri: f64) -> [u16; 3] {
    let (r, g, b) = (s[0] as f64 / 65535.0, s[1] as f64 / 65535.0, s[2] as f64 / 65535.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    // Hue in sixths of a turn, [0, 6).
    let mut h6 = if delta <= 0.0 {
        0.0
    } else if max == r {
        let v = (g - b) / delta;
        if v < 0.0 { v + 6.0 } else { v }
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    let sv = if max <= 0.0 { 0.0 } else { delta / max };
    h6 += hue_turns * 6.0;
    h6 -= (h6 / 6.0).floor() * 6.0;
    if !(0.0..6.0).contains(&h6) {
        h6 = 0.0;
    }
    let s2 = (sv * (1.0 + sat)).clamp(0.0, 1.0);
    let v2 = if bri >= 0.0 { max + (1.0 - max) * bri } else { max * (1.0 + bri) }.clamp(0.0, 1.0);
    let i = h6.floor();
    let f = h6 - i;
    let p = v2 * (1.0 - s2);
    let q = v2 * (1.0 - s2 * f);
    let t = v2 * (1.0 - s2 * (1.0 - f));
    let (r2, g2, b2) = match i as u8 {
        0 => (v2, t, p),
        1 => (q, v2, p),
        2 => (p, v2, t),
        3 => (p, q, v2),
        4 => (t, p, v2),
        _ => (v2, p, q),
    };
    [q16(r2), q16(g2), q16(b2)]
}

/// Apply `adj` to `target`, weighted by `region` coverage (`None` = every pixel). Returns the
/// bounds of the pixels that actually changed.
pub fn adjust_pixels(target: &mut TileGrid, region: Option<&MaskGrid>, adj: &Adjustment) -> RectI {
    let mut dirty = RectI::EMPTY;
    let rdef = region.map_or(255, |m| m.default_value());
    let coords: Vec<TileCoord> = target.coords().collect();
    for c in coords {
        let mt = region.and_then(|m| m.tile(c));
        if mt.is_none() && rdef == 0 {
            continue;
        }
        let Some(src) = target.tile(c).cloned() else { continue };
        let sp = src.pixels();
        let mut work: Option<Tile> = None;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for i in 0..TILE_PIXELS {
            let a = sp[i * 4 + 3];
            if a == 0 {
                continue;
            }
            let cov = mt.map_or(rdef, |t| t.data()[i]) as i64;
            if cov == 0 {
                continue;
            }
            let p = [sp[i * 4].min(a), sp[i * 4 + 1].min(a), sp[i * 4 + 2].min(a)];
            let af = a as u64;
            let full: [u16; 3] = match adj {
                Adjustment::Invert => [a - p[0], a - p[1], a - p[2]],
                _ => {
                    let un = |v: u16| ((v as u64 * 65535 + af / 2) / af).min(65535) as u16;
                    let m = adj.map([un(p[0]), un(p[1]), un(p[2])]);
                    let pm = |v: u16| (((v as u64 * af) + 32767) / 65535).min(af) as u16;
                    [pm(m[0]), pm(m[1]), pm(m[2])]
                }
            };
            let mut out = [0u16; 3];
            for ch in 0..3 {
                let (o, n) = (p[ch] as i64, full[ch] as i64);
                // Round-half-away blend toward the adjusted value by coverage.
                let diff = (n - o) * cov;
                let step = if diff >= 0 { (diff + 127) / 255 } else { -((-diff + 127) / 255) };
                out[ch] = (o + step).clamp(0, a as i64) as u16;
            }
            if out == [sp[i * 4], sp[i * 4 + 1], sp[i * 4 + 2]] {
                continue;
            }
            let t = work.get_or_insert_with(|| (*src).clone());
            t.pixels_mut()[i * 4..i * 4 + 3].copy_from_slice(&out);
            let (lx, ly) = (i % TILE_U, i / TILE_U);
            x0 = x0.min(lx);
            x1 = x1.max(lx + 1);
            y0 = y0.min(ly);
            y1 = y1.max(ly + 1);
        }
        if let Some(t) = work {
            target.set_tile(c, Arc::new(t));
            let tr = c.rect();
            dirty =
                dirty.union(&RectI::new(tr.x0 + x0 as i32, tr.y0 + y0 as i32, tr.x0 + x1 as i32, tr.y0 + y1 as i32));
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_grid() -> TileGrid {
        let mut g = TileGrid::new();
        let mut s = 0x1234_5678_9abc_def0u64;
        for y in -5..70 {
            for x in -5..70 {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let a = (s >> 20) as u16;
                let c = |k: u32| if a == 0 { 0 } else { ((s >> k) % (a as u64 + 1)) as u16 };
                g.put_pixel(x, y, [c(3), c(23), c(41), a]);
            }
        }
        g
    }

    fn premul_ok(g: &TileGrid) -> bool {
        g.iter_tiles().all(|(_, t)| t.pixels().chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]))
    }

    #[test]
    fn invert_is_exact_and_self_inverse() {
        let g = sample_grid();
        let mut h = g.clone();
        let d = adjust_pixels(&mut h, None, &Adjustment::Invert);
        assert_eq!(d, RectI::new(-5, -5, 70, 70));
        assert!(premul_ok(&h));
        let (p, q) = (g.get_pixel(3, 3), h.get_pixel(3, 3));
        assert_eq!(q, [p[3] - p[0], p[3] - p[1], p[3] - p[2], p[3]]);
        adjust_pixels(&mut h, None, &Adjustment::Invert);
        assert_eq!(h, g);
    }

    #[test]
    fn identity_adjustments_change_nothing() {
        let g = sample_grid();
        for adj in [
            Adjustment::hsb(0.0, 0.0, 0.0).unwrap(),
            Adjustment::brightness_contrast(0.0, 0.0).unwrap(),
            Adjustment::curves(Some(&[[0.0, 0.0], [1.0, 1.0]]), None, None, None).unwrap(),
            Adjustment::curves(None, Some(&[[0.0, 0.0], [0.3, 0.3], [1.0, 1.0]]), None, None).unwrap(),
        ] {
            // The straight round trip is exact for opaque pixels; check those bit-exactly.
            let mut opaque = TileGrid::new();
            for (i, v) in [0u16, 1, 257, 32768, 65534, 65535].into_iter().enumerate() {
                opaque.put_pixel(i as i32, 0, [v, 65535 - v, v / 2, 65535]);
            }
            let before = opaque.clone();
            assert!(adjust_pixels(&mut opaque, None, &adj).is_empty(), "{adj:?}");
            assert_eq!(opaque, before);
            let mut h = g.clone();
            adjust_pixels(&mut h, None, &adj);
            assert!(premul_ok(&h));
            // Alpha never changes.
            for y in -5..70 {
                for x in -5..70 {
                    assert_eq!(h.get_pixel(x, y)[3], g.get_pixel(x, y)[3]);
                }
            }
        }
    }

    #[test]
    fn hsb_known_values() {
        let red = [65535u16, 0, 0];
        assert_eq!(Adjustment::hsb(120.0, 0.0, 0.0).unwrap().map(red), [0, 65535, 0]);
        assert_eq!(Adjustment::hsb(-120.0, 0.0, 0.0).unwrap().map(red), [0, 0, 65535]);
        assert_eq!(Adjustment::hsb(180.0, 0.0, 0.0).unwrap().map(red), [0, 65535, 65535]);
        assert_eq!(Adjustment::hsb(0.0, -1.0, 0.0).unwrap().map(red), [65535; 3]);
        assert_eq!(Adjustment::hsb(0.0, 0.0, -1.0).unwrap().map(red), [0; 3]);
        assert_eq!(Adjustment::hsb(0.0, 0.0, -0.5).unwrap().map(red), [32768, 0, 0]);
        // Grays stay gray under hue/saturation changes.
        let gray = [30000u16; 3];
        assert_eq!(Adjustment::hsb(77.0, 1.0, 0.0).unwrap().map(gray), gray);
        assert_eq!(Adjustment::hsb(0.0, 0.0, 1.0).unwrap().map(gray), [65535; 3]);
        // Half saturation of pure orange keeps value and hue.
        assert_eq!(Adjustment::hsb(0.0, -0.5, 0.0).unwrap().map([65535, 32767, 0]), [65535, 49151, 32768]);
    }

    #[test]
    fn brightness_contrast_known_values() {
        let m = |b: f64, c: f64, v: u16| Adjustment::brightness_contrast(b, c).unwrap().map([v; 3])[0];
        assert_eq!(m(0.25, 0.0, 0), 16384);
        assert_eq!(m(1.0, 0.0, 0), 65535);
        assert_eq!(m(-1.0, 0.0, 65535), 0);
        assert_eq!(m(0.0, -1.0, 0), 32768);
        assert_eq!(m(0.0, 1.0, 20000), 0);
        assert_eq!(m(0.0, 1.0, 40000), 65535);
        // contrast 0.5 → k = 3: v' · 65535 = 3 · v16 − 65535.
        assert_eq!(m(0.0, 0.5, 30000), 24465);
        assert_eq!(m(0.0, 0.5, 10000), 0);
        assert_eq!(m(0.0, 0.5, 50000), 65535);
    }

    #[test]
    fn curves_pass_through_points_monotonically() {
        let pts = [[0.0, 0.0], [0.25, 0.5], [0.75, 0.6], [1.0, 1.0]];
        let Adjustment::Luts(l) = Adjustment::curves(Some(&pts), None, None, None).unwrap() else { panic!() };
        for p in pts {
            let i = (p[0] * 65535.0f64).round() as usize;
            assert!((l[0][i] as f64 / 65535.0 - p[1]).abs() < 2e-5, "{p:?} -> {}", l[0][i]);
        }
        assert!(l[0].windows(2).all(|w| w[1] >= w[0]), "monotone data gives a monotone curve");
        assert_eq!(l[0], l[1]);
        // Per-channel curve composes after the master curve; flat ends outside the points.
        let Adjustment::Luts(l) =
            Adjustment::curves(Some(&[[0.0, 1.0], [1.0, 0.0]]), Some(&[[0.2, 0.3], [0.6, 0.9]]), None, None).unwrap()
        else {
            panic!()
        };
        assert_eq!(l[1][0], 65535);
        assert_eq!(l[1][65535], 0);
        assert_eq!(l[0][65535], q16(0.3));
        assert_eq!(l[0][0], q16(0.9));
        // Order of points does not matter.
        assert_eq!(
            Adjustment::curves(Some(&[[1.0, 1.0], [0.0, 0.0], [0.5, 0.2]]), None, None, None).unwrap(),
            Adjustment::curves(Some(&[[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]]), None, None, None).unwrap()
        );
    }

    #[test]
    fn hostile_parameters_are_typed_errors() {
        assert!(Adjustment::hsb(f64::NAN, 0.0, 0.0).is_err());
        assert!(Adjustment::hsb(181.0, 0.0, 0.0).is_err());
        assert!(Adjustment::hsb(0.0, 1.5, 0.0).is_err());
        assert!(Adjustment::brightness_contrast(f64::INFINITY, 0.0).is_err());
        assert!(Adjustment::brightness_contrast(0.0, -2.0).is_err());
        assert!(Adjustment::curves(None, None, None, None).is_err());
        assert!(Adjustment::curves(Some(&[]), None, None, None).is_err());
        assert!(Adjustment::curves(Some(&[[0.5, 0.5]]), None, None, None).is_err());
        assert!(Adjustment::curves(Some(&[[0.5, 0.1], [0.5, 0.9]]), None, None, None).is_err());
        assert!(Adjustment::curves(Some(&[[0.0, 0.0], [1.0, f64::NAN]]), None, None, None).is_err());
        assert!(Adjustment::curves(Some(&[[0.0, 0.0], [1.5, 1.0]]), None, None, None).is_err());
        let many: Vec<[f64; 2]> = (0..1000).map(|i| [i as f64 / 999.0, 0.5]).collect();
        assert!(Adjustment::curves(Some(&many), None, None, None).is_err());
    }

    #[test]
    fn region_limits_and_blends() {
        let mut g = TileGrid::new();
        for x in 0..4 {
            g.put_pixel(x, 0, [65535, 0, 0, 65535]);
        }
        let mut m = MaskGrid::new_empty();
        m.set(1, 0, 255);
        m.set(2, 0, 128);
        let d = adjust_pixels(&mut g, Some(&m), &Adjustment::Invert);
        assert_eq!(d, RectI::new(1, 0, 3, 1));
        assert_eq!(g.get_pixel(0, 0), [65535, 0, 0, 65535]);
        assert_eq!(g.get_pixel(1, 0), [0, 65535, 65535, 65535]);
        assert_eq!(g.get_pixel(2, 0), [32639, 32896, 32896, 65535]);
        assert_eq!(g.get_pixel(3, 0), [65535, 0, 0, 65535]);
    }
}
