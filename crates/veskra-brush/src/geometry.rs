//! Vector geometry for vector-eligible strokes (geometry half of VEC-01/02).
//!
//! The centerline is fitted through the **stabilized paint path** (the same points the dabs
//! were placed along) and the outline uses the same pressure → diameter mapping as the dabs,
//! scaled to the dab profile's 50 % coverage iso-contour, so the vector silhouette matches the
//! raster silhouette (measured by the IoU test in `tests/geometry_iou.rs`).

use veskra_core::PointF;
use veskra_vector::{
    BezPath, Cap, OutlineOptions, WidthPoint, fit_curve, max_distance_to_path, variable_width_outline,
};

use crate::dab::{MIN_RADIUS, profile};
use crate::path::PathPt;

/// Centerline fit tolerance (document px).
pub const CENTERLINE_TOLERANCE: f64 = 0.5;
/// Outline geometric tolerance (document px).
pub const OUTLINE_TOLERANCE: f64 = 0.25;

/// Editable vector interpretation of a stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct GeometryOut {
    /// Fitted open centerline (document px).
    pub centerline: BezPath,
    /// Closed variable-width outline, nonzero fill rule (document px).
    pub outline: BezPath,
    /// Max distance from the stabilized paint-path points to `centerline` (document px).
    pub fit_error: f64,
}

/// Diameter of the 50 % coverage iso-contour of a single dab with nominal diameter `diam`
/// (solved numerically on [`crate::dab::profile`], including sub-pixel fading). For a hard dab
/// this equals `diam`; for softer dabs it lies between `h·diam` and `diam`.
pub(crate) fn iso50_diameter(diam: f64, hardness: f64) -> f64 {
    let radius = diam / 2.0;
    if !(radius.is_finite() && radius > 0.0) {
        return 0.0;
    }
    let r = radius.max(MIN_RADIUS);
    let scale = (radius / r).powi(2);
    let f = |d: f64| profile(d, r, hardness) * scale;
    if f(0.0) < 0.5 {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0, r + 0.5);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if f(mid) >= 0.5 { lo = mid } else { hi = mid }
    }
    2.0 * lo
}

pub(crate) fn build(path: &[PathPt], diameter: impl Fn(&PathPt) -> f64, hardness: f64) -> Option<GeometryOut> {
    // Collapse consecutive duplicates (keep the widest so taps keep their peak size).
    let mut pts: Vec<(PointF, f64)> = Vec::with_capacity(path.len());
    for p in path {
        let w = iso50_diameter(diameter(p), hardness);
        let q = PointF::new(p.x, p.y);
        match pts.last_mut() {
            Some((lp, lw)) if *lp == q => *lw = lw.max(w),
            _ => pts.push((q, w)),
        }
    }
    if pts.is_empty() {
        return None;
    }
    let centers: Vec<PointF> = pts.iter().map(|(p, _)| *p).collect();
    let centerline = fit_curve(&centers, CENTERLINE_TOLERANCE);
    let widths: Vec<WidthPoint> = pts.iter().map(|(p, w)| WidthPoint { p: *p, width: *w }).collect();
    let outline = variable_width_outline(
        &widths,
        &OutlineOptions { tolerance: OUTLINE_TOLERANCE, cap: Cap::Round, min_width: 0.0 },
    );
    let fit_error = max_distance_to_path(&centerline, &centers);
    Some(GeometryOut { centerline, outline, fit_error })
}
