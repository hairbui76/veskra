//! Gradient fill geometry and compositing (EDIT: `edit.gradient`).
//!
//! The gradient parameter `t` is evaluated at **pixel centres** `(x + 0.5, y + 0.5)`:
//! * linear: projection of the pixel onto the axis `p0 → p1`, `t = (p − p0)·(p1 − p0) / |p1 − p0|²`;
//! * radial: `t = |p − p0| / |p1 − p0|` (`p0` = centre, `p1` = a point on the outer circle).
//!
//! `t` is clamped to `[0, 1]` (pad extension: pixels before `p0` take the first color, pixels
//! beyond `p1` the last). The caller supplies `color_at(t)` returning a **premultiplied** RGBA16
//! pixel, so the interpolation space (gamma-encoded or linear light) is the caller's choice and
//! this module stays color-agnostic. The color is scaled by `coverage × opacity` and composited
//! with [`src_over`] (or [`src_atop`] under alpha lock), exactly like [`crate::fill::apply`].

use veskra_core::{PointF, RectI};

use crate::ops::{cov8_to_16, mul65535, scale, src_atop, src_over};
use crate::{MAX_RECT_PIXELS, MaskGrid, Px16, RasterError, TILE_U, Tile, TileGrid};

/// Largest accepted coordinate magnitude for gradient end points.
pub const MAX_COORD: f64 = 1e9;
/// Smallest accepted distance between the two gradient points (document px).
pub const MIN_LENGTH: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GradientKind {
    Linear,
    Radial,
}

/// Validated gradient geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientGeometry {
    kind: GradientKind,
    p0: PointF,
    d: PointF,
    len2: f64,
}

impl GradientGeometry {
    /// `InvalidArgument` for non-finite or out-of-range points and for `p0 == p1`.
    pub fn new(kind: GradientKind, p0: PointF, p1: PointF) -> Result<Self, RasterError> {
        if ![p0.x, p0.y, p1.x, p1.y].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) {
            return Err(RasterError::InvalidArgument("gradient points must be finite and within ±1e9"));
        }
        let d = PointF::new(p1.x - p0.x, p1.y - p0.y);
        let len2 = d.x * d.x + d.y * d.y;
        if len2.sqrt() < MIN_LENGTH {
            return Err(RasterError::InvalidArgument("gradient start and end points must differ"));
        }
        Ok(Self { kind, p0, d, len2 })
    }

    pub fn kind(&self) -> GradientKind {
        self.kind
    }

    /// Gradient parameter of pixel `(x, y)` (centre sampled), clamped to `[0, 1]`.
    pub fn t_at(&self, x: i32, y: i32) -> f64 {
        let (px, py) = (x as f64 + 0.5 - self.p0.x, y as f64 + 0.5 - self.p0.y);
        let t = match self.kind {
            GradientKind::Linear => (px * self.d.x + py * self.d.y) / self.len2,
            GradientKind::Radial => ((px * px + py * py) / self.len2).sqrt(),
        };
        if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) }
    }
}

/// Paint a gradient into `target` over `area`, weighted by `coverage` (`None` = full) and
/// `opacity16`. Returns the bounds of the pixels that actually changed.
///
/// `TooLarge` if `area` exceeds [`MAX_RECT_PIXELS`]. With `alpha_lock` only existing pixels are
/// recolored (destination alpha is preserved).
pub fn fill_gradient(
    target: &mut TileGrid,
    coverage: Option<&MaskGrid>,
    area: RectI,
    geom: &GradientGeometry,
    color_at: &dyn Fn(f64) -> Px16,
    opacity16: u16,
    alpha_lock: bool,
) -> Result<RectI, RasterError> {
    if area.area() > MAX_RECT_PIXELS {
        return Err(RasterError::TooLarge { pixels: area.area(), limit: MAX_RECT_PIXELS });
    }
    let mut dirty = RectI::EMPTY;
    if area.is_empty() || opacity16 == 0 {
        return Ok(dirty);
    }
    let cdef = coverage.map_or(255, |m| m.default_value());
    for c in TileGrid::tiles_in_rect(area) {
        let tr = c.rect();
        let ir = tr.intersect(&area);
        let mt = coverage.and_then(|m| m.tile(c));
        if mt.is_none() && cdef == 0 {
            continue;
        }
        let existing = target.tile(c).cloned();
        if alpha_lock && existing.is_none() {
            continue;
        }
        let mut work: Option<Tile> = None;
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in ir.y0..ir.y1 {
            let ly = (y - tr.y0) as usize;
            for x in ir.x0..ir.x1 {
                let lx = (x - tr.x0) as usize;
                let cov = mt.map_or(cdef, |t| t.data()[ly * TILE_U + lx]);
                if cov == 0 {
                    continue;
                }
                let src = scale(color_at(geom.t_at(x, y)), mul65535(cov8_to_16(cov), opacity16));
                if src == [0; 4] {
                    continue;
                }
                let d = match (&work, &existing) {
                    (Some(t), _) => t.get(lx as u32, ly as u32),
                    (None, Some(t)) => t.get(lx as u32, ly as u32),
                    (None, None) => [0; 4],
                };
                let o = if alpha_lock { src_atop(d, src) } else { src_over(d, src) };
                if o == d {
                    continue;
                }
                work.get_or_insert_with(|| existing.as_deref().cloned().unwrap_or_else(Tile::new_transparent))
                    .set(lx as u32, ly as u32, o);
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
            }
        }
        if let Some(t) = work {
            target.set_tile(c, std::sync::Arc::new(t));
            dirty = dirty.union(&RectI::new(x0, y0, x1, y1));
        }
    }
    Ok(dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(t: f64) -> Px16 {
        let v = (t * 65535.0 + 0.5).floor() as u16;
        [v, v, v, 65535]
    }

    #[test]
    fn linear_is_exact_at_pixel_centres_and_pads() {
        let g = GradientGeometry::new(GradientKind::Linear, PointF::new(10.0, 0.0), PointF::new(20.0, 0.0)).unwrap();
        assert_eq!(g.t_at(0, 5), 0.0);
        assert_eq!(g.t_at(10, 0), 0.05);
        assert_eq!(g.t_at(14, 99), 0.45);
        assert_eq!(g.t_at(25, 0), 1.0);
        let mut grid = TileGrid::new();
        let d = fill_gradient(&mut grid, None, RectI::new(0, 0, 30, 2), &g, &gray, 65535, false).unwrap();
        assert_eq!(d, RectI::new(0, 0, 30, 2));
        assert_eq!(grid.get_pixel(0, 0), [0, 0, 0, 65535]);
        assert_eq!(grid.get_pixel(15, 1), gray(0.55));
        assert_eq!(grid.get_pixel(29, 1), [65535; 4]);
    }

    #[test]
    fn radial_distance_and_degenerate_rejected() {
        let g = GradientGeometry::new(GradientKind::Radial, PointF::new(8.0, 8.0), PointF::new(8.0, 18.0)).unwrap();
        assert!((g.t_at(8, 12) - (0.5f64 * 0.5 + 4.5 * 4.5).sqrt() / 10.0).abs() < 1e-12);
        assert_eq!(g.t_at(40, 40), 1.0);
        for (a, b) in [((1.0, 1.0), (1.0, 1.0)), ((f64::NAN, 0.0), (1.0, 1.0)), ((0.0, 0.0), (f64::INFINITY, 0.0))] {
            let r = GradientGeometry::new(GradientKind::Linear, PointF::new(a.0, a.1), PointF::new(b.0, b.1));
            assert!(matches!(r, Err(RasterError::InvalidArgument(_))));
        }
    }

    #[test]
    fn coverage_alpha_lock_and_limits() {
        let g = GradientGeometry::new(GradientKind::Linear, PointF::new(0.0, 0.0), PointF::new(8.0, 0.0)).unwrap();
        let white = |_t: f64| -> Px16 { [65535; 4] };
        let mut grid = TileGrid::new();
        grid.put_pixel(1, 0, [0, 0, 0, 32768]);
        // Alpha lock: only the existing pixel changes and keeps its alpha.
        let d = fill_gradient(&mut grid, None, RectI::new(0, 0, 8, 1), &g, &white, 65535, true).unwrap();
        assert_eq!(d, RectI::new(1, 0, 2, 1));
        assert_eq!(grid.get_pixel(1, 0), [32768; 4]);
        assert_eq!(grid.get_pixel(0, 0), [0; 4]);
        // Coverage limits the painted pixels; half coverage gives half alpha.
        let mut m = MaskGrid::new_empty();
        m.set(3, 0, 255);
        m.set(4, 0, 128);
        let mut grid = TileGrid::new();
        let d = fill_gradient(&mut grid, Some(&m), RectI::new(0, 0, 8, 1), &g, &white, 65535, false).unwrap();
        assert_eq!(d, RectI::new(3, 0, 5, 1));
        assert_eq!(grid.get_pixel(3, 0), [65535; 4]);
        assert_eq!(grid.get_pixel(4, 0)[3], 32896);
        assert_eq!(grid.tile_count(), 1);
        let huge = RectI::new(0, 0, 100_000, 100_000);
        assert!(matches!(
            fill_gradient(&mut grid, None, huge, &g, &white, 65535, false),
            Err(RasterError::TooLarge { .. })
        ));
    }
}
