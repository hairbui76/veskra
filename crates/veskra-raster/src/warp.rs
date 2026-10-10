//! Projective (4-corner "distort / perspective") resampling of pixels and masks.
//!
//! A [`Homography`] maps a source rectangle onto a destination quad given as
//! `[top-left, top-right, bottom-right, bottom-left]` images of the rectangle's corners. The quad
//! must be **strictly convex** (either winding, so mirrored quads are fine); degenerate,
//! self-intersecting or concave quads are rejected with `InvalidArgument` because the projective
//! map would fold the plane (its denominator changes sign inside the rectangle).
//!
//! Resampling follows the affine policy of [`crate::transform`]: inverse mapping of destination
//! pixel centres, source position quantized to 1/256 px, bilinear blend of four premultiplied
//! source pixels with integer weights and one round-half-up shift (so `r,g,b ≤ a` holds). Only
//! the quad's bounding box (± 1 px) ∩ `clip` is visited. Like the affine transform there is no
//! minification prefilter.

use std::sync::Arc;

use veskra_core::{PointF, RectF, RectI};

use crate::mask::MaskTile;
use crate::transform::{Cache, ONE, bilinear, bilinear1, sb_limits};
use crate::{MAX_RECT_PIXELS, MaskGrid, RasterError, TILE_U, Tile, TileGrid};

/// Largest accepted quad coordinate magnitude (document px).
pub const MAX_QUAD_COORD: f64 = 1e7;
/// Minimum |cross product| of adjacent quad edges (px²) at every corner.
pub const MIN_CORNER_CROSS: f64 = 1e-3;

type M3 = [[f64; 3]; 3];

fn mul3(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    o
}

fn invert3(m: &M3) -> Option<M3> {
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let adj = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
    if !det.is_finite() || det.abs() < 1e-300 {
        return None;
    }
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = adj[i][j] / det;
            if !o[i][j].is_finite() {
                return None;
            }
        }
    }
    Some(o)
}

/// Projective map from a source rectangle onto a convex quad, with its inverse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography {
    fwd: M3,
    inv: M3,
    quad: [PointF; 4],
}

impl Homography {
    /// Map `src`'s corners `(x0,y0) (x1,y0) (x1,y1) (x0,y1)` onto `quad[0..4]`.
    pub fn rect_to_quad(src: RectF, quad: [PointF; 4]) -> Result<Self, RasterError> {
        if quad
            .iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite() || p.x.abs() > MAX_QUAD_COORD || p.y.abs() > MAX_QUAD_COORD)
        {
            return Err(RasterError::InvalidArgument("quad corners must be finite and within ±1e7"));
        }
        let (rw, rh) = (src.x1 - src.x0, src.y1 - src.y0);
        if !(rw.is_finite() && rh.is_finite() && rw > 0.0 && rh > 0.0) {
            return Err(RasterError::InvalidArgument("source rectangle for the quad transform is empty"));
        }
        // Strict convexity: all corner cross products share one sign and are not ~0.
        let mut sign = 0.0f64;
        for i in 0..4 {
            let (a, b, c) = (quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]);
            let z = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
            if !z.is_finite() || z.abs() < MIN_CORNER_CROSS || (sign != 0.0 && z.signum() != sign) {
                return Err(RasterError::InvalidArgument(
                    "quad must be convex and non-degenerate (corners in order: top-left, top-right, bottom-right, bottom-left)",
                ));
            }
            sign = z.signum();
        }
        // Unit square → quad (Heckbert).
        let [p0, p1, p2, p3] = quad;
        let (dx1, dx2, sx) = (p1.x - p2.x, p3.x - p2.x, p0.x - p1.x + p2.x - p3.x);
        let (dy1, dy2, sy) = (p1.y - p2.y, p3.y - p2.y, p0.y - p1.y + p2.y - p3.y);
        let den = dx1 * dy2 - dx2 * dy1;
        if !den.is_finite() || den.abs() < 1e-12 {
            return Err(RasterError::InvalidArgument("quad is degenerate"));
        }
        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;
        // The denominator g·u + h·v + 1 must stay positive on the whole unit square.
        if ![1.0, 1.0 + g, 1.0 + g + h, 1.0 + h].iter().all(|w| w.is_finite() && *w > 1e-9) {
            return Err(RasterError::InvalidArgument("quad is too extreme for a perspective transform"));
        }
        let sq: M3 = [
            [p1.x - p0.x + g * p1.x, p3.x - p0.x + h * p3.x, p0.x],
            [p1.y - p0.y + g * p1.y, p3.y - p0.y + h * p3.y, p0.y],
            [g, h, 1.0],
        ];
        let norm: M3 = [[1.0 / rw, 0.0, -src.x0 / rw], [0.0, 1.0 / rh, -src.y0 / rh], [0.0, 0.0, 1.0]];
        let fwd = mul3(&sq, &norm);
        let inv = invert3(&fwd).ok_or(RasterError::InvalidArgument("quad transform is singular"))?;
        if fwd.iter().flatten().any(|v| !v.is_finite()) {
            return Err(RasterError::InvalidArgument("quad transform is not finite"));
        }
        Ok(Self { fwd, inv, quad })
    }

    fn project(m: &M3, p: PointF) -> Option<(PointF, f64)> {
        let w = m[2][0] * p.x + m[2][1] * p.y + m[2][2];
        if !w.is_finite() || w.abs() < 1e-12 {
            return None;
        }
        let x = (m[0][0] * p.x + m[0][1] * p.y + m[0][2]) / w;
        let y = (m[1][0] * p.x + m[1][1] * p.y + m[1][2]) / w;
        (x.is_finite() && y.is_finite()).then_some((PointF::new(x, y), w))
    }

    /// Source → destination (`None` on or beyond the horizon line).
    pub fn apply(&self, p: PointF) -> Option<PointF> {
        Self::project(&self.fwd, p).filter(|(_, w)| *w > 0.0).map(|(q, _)| q)
    }

    /// Destination → source (`None` where the destination has no finite pre-image on the
    /// visible side of the horizon).
    pub fn apply_inverse(&self, p: PointF) -> Option<PointF> {
        let (s, _) = Self::project(&self.inv, p)?;
        // The pre-image must lie on the positive side of the forward denominator.
        let w = self.fwd[2][0] * s.x + self.fwd[2][1] * s.y + self.fwd[2][2];
        (w > 1e-9).then_some(s)
    }

    /// Bounding box of the destination quad.
    pub fn quad_bounds(&self) -> RectF {
        RectF::from_points(self.quad)
    }
}

fn dest_box(h: &Homography, clip: RectI) -> Result<RectI, RasterError> {
    let d = RectI::covering(&h.quad_bounds().inflate(1.0)).intersect(&clip);
    if d.area() > MAX_RECT_PIXELS {
        return Err(RasterError::TooLarge { pixels: d.area(), limit: MAX_RECT_PIXELS });
    }
    Ok(d)
}

/// Quantized (1/256 px) sample position for destination pixel `(x, y)`.
fn sample_pos(h: &Homography, x: i32, y: i32) -> Option<(f64, f64, i64, i64)> {
    let s = h.apply_inverse(PointF::new(x as f64 + 0.5, y as f64 + 0.5))?;
    let (sx, sy) = (s.x - 0.5, s.y - 0.5);
    if sx.abs() > 1e12 || sy.abs() > 1e12 {
        return None;
    }
    let q = |v: f64| (v * ONE as f64).round() as i64;
    Some((sx, sy, q(sx), q(sy)))
}

/// Warp `src` by `h`, producing pixels only inside `clip`. `TooLarge` if the destination box
/// exceeds [`MAX_RECT_PIXELS`].
pub fn try_warp_pixels(src: &TileGrid, h: &Homography, clip: RectI) -> Result<TileGrid, RasterError> {
    let mut out = TileGrid::new();
    let sb = src.content_bounds();
    let dbox = dest_box(h, clip)?;
    if sb.is_empty() || dbox.is_empty() {
        return Ok(out);
    }
    let lim = sb_limits(sb);
    let mut cache = Cache { grid: src, key: None, tile: None };
    for c in TileGrid::tiles_in_rect(dbox) {
        let tr = c.rect();
        let ir = tr.intersect(&dbox);
        let mut tile: Option<Tile> = None;
        for y in ir.y0..ir.y1 {
            for x in ir.x0..ir.x1 {
                let Some((sx, sy, qx, qy)) = sample_pos(h, x, y) else { continue };
                if !(sx > lim.x0 && sx < lim.x1 && sy > lim.y0 && sy < lim.y1) {
                    continue;
                }
                let p = bilinear(qx, qy, |x, y| cache.get(x, y));
                if p != [0; 4] {
                    tile.get_or_insert_with(Tile::new_transparent).set((x - tr.x0) as u32, (y - tr.y0) as u32, p);
                }
            }
        }
        if let Some(t) = tile {
            out.set_tile(c, Arc::new(t));
        }
    }
    Ok(out)
}

/// Warp the part of `src` inside `src_rect` by `h` (coverage outside `src_rect` counts as 0).
/// The result always has default value 0.
pub fn try_warp_mask(src: &MaskGrid, src_rect: RectI, h: &Homography, clip: RectI) -> Result<MaskGrid, RasterError> {
    let mut out = MaskGrid::new_empty();
    let dbox = dest_box(h, clip)?;
    if src_rect.is_empty() || dbox.is_empty() {
        return Ok(out);
    }
    let lim = sb_limits(src_rect);
    let fetch = |x: i64, y: i64| -> u32 {
        if x < src_rect.x0 as i64 || x >= src_rect.x1 as i64 || y < src_rect.y0 as i64 || y >= src_rect.y1 as i64 {
            return 0;
        }
        src.get(x as i32, y as i32) as u32
    };
    for c in TileGrid::tiles_in_rect(dbox) {
        let tr = c.rect();
        let ir = tr.intersect(&dbox);
        let mut t: Option<MaskTile> = None;
        for y in ir.y0..ir.y1 {
            for x in ir.x0..ir.x1 {
                let Some((sx, sy, qx, qy)) = sample_pos(h, x, y) else { continue };
                if !(sx > lim.x0 && sx < lim.x1 && sy > lim.y0 && sy < lim.y1) {
                    continue;
                }
                let v = bilinear1(qx, qy, fetch) as u8;
                if v != 0 {
                    t.get_or_insert_with(|| MaskTile::new_filled(0)).data_mut()
                        [(y - tr.y0) as usize * TILE_U + (x - tr.x0) as usize] = v;
                }
            }
        }
        if let Some(t) = t {
            out.set_tile(c, Arc::new(t));
        }
    }
    out.compact();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use veskra_core::Affine;

    const CLIP: RectI = RectI::new(-256, -256, 512, 512);

    fn q(p: [(f64, f64); 4]) -> [PointF; 4] {
        p.map(|(x, y)| PointF::new(x, y))
    }

    fn pattern(r: RectI) -> TileGrid {
        let mut g = TileGrid::new();
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let a = 20000 + ((x * 37 + y * 101) & 0x7fff) as u16;
                g.put_pixel(x, y, [(a / 3) + (x as u16 & 7), a / 2, (y as u16 * 13) % (a / 4), a]);
            }
        }
        g
    }

    #[test]
    fn identity_quad_is_bit_exact() {
        let r = RectI::new(3, 5, 90, 70);
        let g = pattern(r);
        let h = Homography::rect_to_quad(
            RectF::new(3.0, 5.0, 90.0, 70.0),
            q([(3.0, 5.0), (90.0, 5.0), (90.0, 70.0), (3.0, 70.0)]),
        )
        .unwrap();
        assert_eq!(try_warp_pixels(&g, &h, CLIP).unwrap(), g);
        let m = MaskGrid::new_rect(r);
        assert_eq!(try_warp_mask(&m, r, &h, CLIP).unwrap(), m);
    }

    #[test]
    fn parallelogram_quads_match_the_affine_transform() {
        let r = RectI::new(0, 0, 40, 30);
        let g = pattern(r);
        let src = RectF::new(0.0, 0.0, 40.0, 30.0);
        for m in [
            Affine::translate(17.0, -9.0),
            Affine::scale(-1.0, 1.0).then(&Affine::translate(40.0, 0.0)),
            Affine::rotate(std::f64::consts::FRAC_PI_2).then(&Affine::translate(30.0, 0.0)),
            Affine::scale(2.0, 0.5).then(&Affine::translate(3.25, 1.5)),
        ] {
            let c = |x: f64, y: f64| m.apply(PointF::new(x, y));
            let h = Homography::rect_to_quad(src, [c(0.0, 0.0), c(40.0, 0.0), c(40.0, 30.0), c(0.0, 30.0)]).unwrap();
            let warped = try_warp_pixels(&g, &h, CLIP).unwrap();
            let affine = crate::transform::try_transform_pixels(&g, None, m, CLIP).unwrap();
            assert_eq!(warped, affine, "{m:?}");
        }
    }

    #[test]
    fn perspective_maps_corners_and_centre() {
        let src = RectF::new(0.0, 0.0, 100.0, 100.0);
        let quad = q([(20.0, 10.0), (80.0, 10.0), (100.0, 90.0), (0.0, 90.0)]);
        let h = Homography::rect_to_quad(src, quad).unwrap();
        for (s, d) in [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)].into_iter().zip(quad) {
            let p = h.apply(PointF::new(s.0, s.1)).unwrap();
            assert!((p.x - d.x).abs() < 1e-9 && (p.y - d.y).abs() < 1e-9, "{s:?} -> {p:?}");
            let back = h.apply_inverse(d).unwrap();
            assert!((back.x - s.0).abs() < 1e-7 && (back.y - s.1).abs() < 1e-7);
        }
        // The rectangle's centre maps to the intersection of the quad's diagonals (projective invariant).
        let c = h.apply(PointF::new(50.0, 50.0)).unwrap();
        assert!((c.x - 50.0).abs() < 1e-9 && (c.y - 40.0).abs() < 1e-9, "{c:?}");
        // A solid opaque square warps to exactly the trapezoid's interior.
        let mut g = TileGrid::new();
        for y in 0..100 {
            for x in 0..100 {
                g.put_pixel(x, y, [0, 65535, 0, 65535]);
            }
        }
        let w = try_warp_pixels(&g, &h, CLIP).unwrap();
        assert_eq!(w.get_pixel(50, 50), [0, 65535, 0, 65535]);
        assert_eq!(w.get_pixel(22, 12), [0, 65535, 0, 65535]);
        assert_eq!(w.get_pixel(5, 12), [0; 4], "outside the slanted left edge");
        assert_eq!(w.get_pixel(3, 88), [0, 65535, 0, 65535]);
        let cb = w.content_bounds();
        assert_eq!(cb.intersect(&RectI::new(-1, 9, 101, 91)), cb);
        assert!(w.iter_tiles().all(|(_, t)| t.pixels().chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3])));
    }

    #[test]
    fn bad_quads_are_typed_errors() {
        let src = RectF::new(0.0, 0.0, 10.0, 10.0);
        let bad = [
            q([(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (10.0, 10.0)]), // repeated corner
            q([(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)]),  // bow-tie
            q([(0.0, 0.0), (10.0, 0.0), (2.0, 2.0), (0.0, 10.0)]),    // concave
            q([(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (15.0, 0.0)]),    // collinear
            q([(0.0, 0.0), (f64::NAN, 0.0), (10.0, 10.0), (0.0, 10.0)]),
            q([(0.0, 0.0), (1e9, 0.0), (1e9, 10.0), (0.0, 10.0)]),
        ];
        for b in bad {
            assert!(matches!(Homography::rect_to_quad(src, b), Err(RasterError::InvalidArgument(_))), "{b:?}");
        }
        assert!(
            Homography::rect_to_quad(
                RectF::new(0.0, 0.0, 0.0, 10.0),
                q([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
            )
            .is_err()
        );
        // Huge but valid quad: bounded by the clip / pixel limit, never a panic.
        let big = q([(-9e6, -9e6), (9e6, -9e6), (9e6, 9e6), (-9e6, 9e6)]);
        let h = Homography::rect_to_quad(src, big).unwrap();
        let g = pattern(RectI::new(0, 0, 10, 10));
        assert!(try_warp_pixels(&g, &h, RectI::new(0, 0, 64, 64)).is_ok());
        let huge_clip = RectI::new(-100_000, -100_000, 100_000, 100_000);
        assert!(matches!(try_warp_pixels(&g, &h, huge_clip), Err(RasterError::TooLarge { .. })));
    }
}
