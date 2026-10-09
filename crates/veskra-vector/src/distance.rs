//! Point-to-path distance (PNT-010 metric).

use veskra_core::{PointF, RectF};

use crate::cubic::{CubicBez, cubic_distance};
use crate::path::{BezPath, PathSeg};
use crate::vec2::{V2, dist_to_segment};

enum Prep {
    Point(V2),
    Line(V2, V2),
    Cubic(CubicBez, Vec<(f64, V2)>),
}

/// A path prepared for repeated distance queries.
pub struct PreparedPath {
    items: Vec<(Prep, RectF)>,
}

impl std::fmt::Debug for PreparedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedPath").field("items", &self.items.len()).finish()
    }
}

fn rect_dist(r: &RectF, p: V2) -> f64 {
    let dx = (r.x0 - p.x).max(0.0).max(p.x - r.x1);
    let dy = (r.y0 - p.y).max(0.0).max(p.y - r.y1);
    dx.hypot(dy)
}

impl PreparedPath {
    pub fn new(path: &BezPath) -> Self {
        let mut items = Vec::new();
        for s in path.subpaths() {
            if s.segs.is_empty() && s.closing_seg().is_none() {
                items.push((Prep::Point(s.start.into()), RectF::from_points([s.start])));
                continue;
            }
            for seg in s.geom_segs() {
                match seg {
                    PathSeg::Line(a, b) => items.push((Prep::Line(a.into(), b.into()), RectF::from_points([a, b]))),
                    PathSeg::Cubic(c) => {
                        let samples = c.distance_samples();
                        items.push((Prep::Cubic(c, samples), c.control_bounds()));
                    }
                }
            }
        }
        Self { items }
    }

    /// Distance from `p` to the path (`f64::INFINITY` for an empty path).
    pub fn distance(&self, p: PointF) -> f64 {
        self.distance_hint(p.into(), &mut 0)
    }

    fn distance_hint(&self, p: V2, hint: &mut usize) -> f64 {
        let mut best = f64::INFINITY;
        let mut best_i = *hint;
        let n = self.items.len();
        for k in 0..n {
            // Start at the previous winner (spatial coherence of consecutive queries).
            let i = (*hint + k) % n;
            let (prep, bb) = &self.items[i];
            if rect_dist(bb, p) >= best {
                continue;
            }
            let d = match prep {
                Prep::Point(q) => p.dist(*q),
                Prep::Line(a, b) => dist_to_segment(p, *a, *b),
                Prep::Cubic(c, s) => cubic_distance(c, s, p, best),
            };
            if d < best {
                best = d;
                best_i = i;
            }
        }
        *hint = best_i;
        best
    }
}

/// Maximum over `pts` of the distance from each point to `path` (PNT-010 fit metric).
///
/// Lines are exact. Cubics are sampled uniformly so that the chord polyline is provably
/// within `1e-3 × segment size` of the curve; every chord interval that could contain the
/// nearest point is then refined with clamped Newton iterations. The returned value is a
/// minimum over distances to **actual curve points**, so it never underestimates the true
/// distance (conservative for verification) and in practice equals it to ~1e-9 px.
///
/// Non-finite points are ignored. Returns `0.0` if `pts` has no finite points and
/// `f64::INFINITY` if the path has no geometry but points exist.
pub fn max_distance_to_path(path: &BezPath, pts: &[PointF]) -> f64 {
    let prepared = PreparedPath::new(path);
    let mut hint = 0;
    let mut worst: f64 = 0.0;
    for &p in pts {
        if !p.is_finite() {
            continue;
        }
        if prepared.items.is_empty() {
            return f64::INFINITY;
        }
        worst = worst.max(prepared.distance_hint(p.into(), &mut hint));
    }
    worst
}
