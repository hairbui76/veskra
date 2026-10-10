//! Boundary tracing of a coverage mask as closed pixel-edge polylines ("marching ants").
//!
//! A pixel is *inside* when its coverage is ≥ 128 (50 %). Every inside pixel contributes its
//! edges that border an outside pixel; edges are chained into closed loops with the inside on
//! the right-hand side of the travel direction (clockwise in y-down document coordinates for
//! outer boundaries, counter-clockwise for holes). Diagonally touching pixels are treated as
//! separate regions (4-connectivity). Only corners are emitted (collinear runs are merged) and a
//! loop is implicitly closed: the first point is not repeated.
//!
//! # Bounds
//! * Work is limited to `mask.bounds() ∩ clip`.
//! * At most [`MAX_UNIT_EDGES`] unit edges are traced; a busier mask is first reduced by an
//!   integer factor (a cell is inside when at least half of its pixels are), which sets
//!   `truncated`.
//! * The result holds at most `max_points` points: contours are simplified with
//!   Douglas–Peucker at growing tolerance, then the smallest contours are dropped. Either step
//!   sets `truncated`.
//!
//! Output is deterministic (loops start at their top-most, then left-most corner and are sorted
//! by that corner).

use veskra_core::RectI;

use crate::MaskGrid;

/// Upper bound on traced unit edges before the mask is down-sampled.
pub const MAX_UNIT_EDGES: usize = 1 << 20;
/// Coverage at or above which a pixel counts as selected.
pub const INSIDE_THRESHOLD: u8 = 128;

/// Result of [`trace`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outline {
    /// Closed polylines in document pixel-edge coordinates.
    pub contours: Vec<Vec<(i32, i32)>>,
    /// Detail was dropped (down-sampling, simplification or removed contours).
    pub truncated: bool,
}

impl Outline {
    pub fn point_count(&self) -> usize {
        self.contours.iter().map(Vec::len).sum()
    }
}

// Directions: 0 = +x, 1 = +y, 2 = −x, 3 = −y.
const DX: [i32; 4] = [1, 0, -1, 0];
const DY: [i32; 4] = [0, 1, 0, -1];

/// Binary rows of the (possibly down-sampled) mask over `area`; row/column 0 maps to `area` origin.
struct Cells<'a> {
    mask: &'a MaskGrid,
    area: RectI,
    step: i32,
    w: usize,
    h: usize,
}

impl Cells<'_> {
    fn row(&self, cy: usize, out: &mut Vec<bool>) {
        out.clear();
        out.resize(self.w, false);
        let s = self.step;
        let y0 = self.area.y0 + cy as i32 * s;
        let y1 = (y0 + s).min(self.area.y1);
        let px = self.mask.read_rect(RectI::new(self.area.x0, y0, self.area.x1, y1));
        let pw = self.area.width() as usize;
        if px.len() != pw * (y1 - y0) as usize {
            return;
        }
        if s == 1 {
            for (o, v) in out.iter_mut().zip(px.iter()) {
                *o = *v >= INSIDE_THRESHOLD;
            }
            return;
        }
        let mut count = vec![0u32; self.w];
        for r in px.chunks_exact(pw) {
            for (x, v) in r.iter().enumerate() {
                if *v >= INSIDE_THRESHOLD {
                    count[x / s as usize] += 1;
                }
            }
        }
        for (cx, o) in out.iter_mut().enumerate() {
            let cw = ((cx as i32 + 1) * s).min(self.area.width() as i32) - cx as i32 * s;
            let total = (cw * (y1 - y0)) as u32;
            *o = count[cx] * 2 >= total && count[cx] > 0;
        }
    }
}

/// Collect directed unit edges `(y, x, dir)` (start vertex in cell coordinates). `None` if more
/// than `limit` edges exist.
fn collect_edges(cells: &Cells<'_>, limit: usize) -> Option<Vec<(i32, i32, u8)>> {
    let mut edges = Vec::new();
    let mut prev: Vec<bool> = vec![false; cells.w];
    let mut cur: Vec<bool> = Vec::new();
    let mut next: Vec<bool> = Vec::new();
    cells.row(0, &mut cur);
    for cy in 0..cells.h {
        if cy + 1 < cells.h {
            cells.row(cy + 1, &mut next);
        } else {
            next.clear();
            next.resize(cells.w, false);
        }
        let y = cy as i32;
        for cx in 0..cells.w {
            if !cur[cx] {
                continue;
            }
            let x = cx as i32;
            if !prev[cx] {
                edges.push((y, x, 0u8));
            }
            if cx + 1 >= cells.w || !cur[cx + 1] {
                edges.push((y, x + 1, 1));
            }
            if !next[cx] {
                edges.push((y + 1, x + 1, 2));
            }
            if cx == 0 || !cur[cx - 1] {
                edges.push((y + 1, x, 3));
            }
        }
        if edges.len() > limit {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
        std::mem::swap(&mut cur, &mut next);
    }
    Some(edges)
}

/// Chain unit edges into corner-only closed loops (cell coordinates).
fn chain(mut edges: Vec<(i32, i32, u8)>) -> Vec<Vec<(i32, i32)>> {
    edges.sort_unstable();
    let mut used = vec![false; edges.len()];
    // Index range of the edges starting at vertex (y, x).
    let at = |edges: &[(i32, i32, u8)], y: i32, x: i32| -> (usize, usize) {
        let lo = edges.partition_point(|e| (e.0, e.1) < (y, x));
        let hi = edges.partition_point(|e| (e.0, e.1) <= (y, x));
        (lo, hi)
    };
    let mut loops = Vec::new();
    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        let mut pts: Vec<(i32, i32)> = Vec::new();
        let mut i = start;
        let mut prev_dir = u8::MAX;
        loop {
            used[i] = true;
            let (y, x, d) = edges[i];
            if d != prev_dir {
                pts.push((x, y));
            }
            prev_dir = d;
            let (ny, nx) = (y + DY[d as usize], x + DX[d as usize]);
            let (lo, hi) = at(&edges, ny, nx);
            // Prefer the right turn so diagonally touching regions stay separate loops.
            let mut pick = None;
            for want in [(d + 1) % 4, d, (d + 3) % 4] {
                if let Some(k) = (lo..hi).find(|&k| !used[k] && edges[k].2 == want) {
                    pick = Some(k);
                    break;
                }
            }
            match pick {
                Some(k) => i = k,
                None => break,
            }
        }
        // Drop a redundant first point when the loop closes along its first direction.
        if pts.len() > 2 && edges[start].2 == prev_dir {
            pts.remove(0);
        }
        if pts.len() >= 4 {
            loops.push(pts);
        }
    }
    loops
}

fn seg_dist2(p: (i32, i32), a: (i32, i32), b: (i32, i32)) -> f64 {
    let (px, py) = (p.0 as f64, p.1 as f64);
    let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) };
    let (qx, qy) = (ax + t * dx - px, ay + t * dy - py);
    qx * qx + qy * qy
}

/// Douglas–Peucker on a closed loop (split at index 0 and the point farthest from it).
/// `budget` bounds the number of distance evaluations; when it runs out the remaining spans
/// keep only their end points.
fn simplify_closed(pts: &[(i32, i32)], eps: f64, budget: &mut u64) -> Vec<(i32, i32)> {
    let n = pts.len();
    if n <= 4 {
        return pts.to_vec();
    }
    let far = (1..n)
        .max_by(|&a, &b| {
            let d = |i: usize| {
                let (dx, dy) = ((pts[i].0 - pts[0].0) as f64, (pts[i].1 - pts[0].1) as f64);
                dx * dx + dy * dy
            };
            d(a).total_cmp(&d(b)).then(b.cmp(&a))
        })
        .unwrap_or(n / 2);
    let mut keep = vec![false; n + 1];
    keep[0] = true;
    keep[far] = true;
    keep[n] = true; // virtual closing point == pts[0]
    let get = |i: usize| pts[i % n];
    let mut stack = vec![(0usize, far), (far, n)];
    let e2 = eps * eps;
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let span = (b - a) as u64;
        if *budget < span {
            continue;
        }
        *budget -= span;
        let (mut best, mut best_d) = (a, -1.0f64);
        for i in a + 1..b {
            let d = seg_dist2(get(i), get(a), get(b));
            if d > best_d {
                best_d = d;
                best = i;
            }
        }
        if best_d > e2 {
            keep[best] = true;
            stack.push((a, best));
            stack.push((best, b));
        }
    }
    (0..n).filter(|&i| keep[i]).map(|i| pts[i]).collect()
}

fn loop_area2(p: &[(i32, i32)]) -> i64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 as i64 * p[(i + 1) % n].1 as i64 - p[(i + 1) % n].0 as i64 * p[i].1 as i64).sum::<i64>().abs()
}

/// Trace the boundary of `mask` inside `clip` with at most `max_points` points (minimum 8).
pub fn trace(mask: &MaskGrid, clip: RectI, max_points: usize) -> Outline {
    let max_points = max_points.max(8);
    let area = mask.bounds().intersect(&clip);
    if area.is_empty() || area.area() > crate::MAX_RECT_PIXELS {
        return Outline { contours: Vec::new(), truncated: !area.is_empty() };
    }
    let mut step = 1i32;
    let mut truncated = false;
    let edges = loop {
        let cells = Cells {
            mask,
            area,
            step,
            w: (area.width() as usize).div_ceil(step as usize),
            h: (area.height() as usize).div_ceil(step as usize),
        };
        match collect_edges(&cells, MAX_UNIT_EDGES) {
            Some(e) => break e,
            None => {
                truncated = true;
                step *= 2;
                if step > 4096 {
                    return Outline { contours: Vec::new(), truncated: true };
                }
            }
        }
    };
    let mut loops = chain(edges);
    let total = |l: &[Vec<(i32, i32)>]| l.iter().map(Vec::len).sum::<usize>();
    if total(&loops) > max_points {
        truncated = true;
        let mut eps = 0.75f64;
        let mut budget = 200_000_000u64;
        let mut simplified = loops.clone();
        for _ in 0..16 {
            simplified = loops.iter().map(|l| simplify_closed(l, eps, &mut budget)).filter(|l| l.len() >= 3).collect();
            if total(&simplified) <= max_points {
                break;
            }
            eps *= 2.0;
        }
        loops = simplified;
        if total(&loops) > max_points {
            // Keep the largest contours that fit.
            let mut order: Vec<usize> = (0..loops.len()).collect();
            order.sort_by(|&a, &b| loop_area2(&loops[b]).cmp(&loop_area2(&loops[a])).then(a.cmp(&b)));
            let mut keep = vec![false; loops.len()];
            let mut used = 0usize;
            for i in order {
                if used + loops[i].len() <= max_points {
                    used += loops[i].len();
                    keep[i] = true;
                }
            }
            let mut k = 0;
            loops.retain(|_| {
                k += 1;
                keep[k - 1]
            });
        }
    }
    let contours = loops
        .into_iter()
        .map(|l| {
            l.into_iter().map(|(x, y)| ((area.x0 + x * step).min(area.x1), (area.y0 + y * step).min(area.y1))).collect()
        })
        .collect();
    Outline { contours, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veskra_core::{PointF, RectF};

    const CLIP: RectI = RectI::new(0, 0, 512, 512);

    #[test]
    fn rectangle_is_four_clockwise_corners() {
        let m = MaskGrid::new_rect(RectI::new(10, 20, 100, 60));
        let o = trace(&m, CLIP, 4000);
        assert_eq!(o, Outline { contours: vec![vec![(10, 20), (100, 20), (100, 60), (10, 60)]], truncated: false });
    }

    #[test]
    fn hole_and_diagonal_neighbours() {
        let mut m = MaskGrid::new_rect(RectI::new(0, 0, 10, 10));
        m.fill_rect(RectI::new(3, 3, 6, 6), 0);
        let o = trace(&m, CLIP, 4000);
        assert_eq!(o.contours.len(), 2);
        assert_eq!(o.contours[0], vec![(0, 0), (10, 0), (10, 10), (0, 10)]);
        // The hole runs counter-clockwise.
        assert_eq!(o.contours[1], vec![(3, 3), (3, 6), (6, 6), (6, 3)]);
        // Two pixels touching only at a corner are two separate loops.
        let mut d = MaskGrid::new_empty();
        d.set(5, 5, 255);
        d.set(6, 6, 255);
        let o = trace(&d, CLIP, 4000);
        assert_eq!(o.contours, vec![vec![(5, 5), (6, 5), (6, 6), (5, 6)], vec![(6, 6), (7, 6), (7, 7), (6, 7)]]);
    }

    #[test]
    fn threshold_and_clip() {
        let mut m = MaskGrid::new_empty();
        m.set(1, 1, 127);
        m.set(2, 1, 128);
        assert_eq!(trace(&m, CLIP, 4000).contours, vec![vec![(2, 1), (3, 1), (3, 2), (2, 2)]]);
        assert!(trace(&MaskGrid::new_empty(), CLIP, 4000).contours.is_empty());
        // Inverted (default 255) masks are traced within the clip.
        let mut full = MaskGrid::new_full();
        full.fill_rect(RectI::new(0, 0, 4, 4), 0);
        let o = trace(&full, RectI::new(0, 0, 8, 8), 4000);
        assert_eq!(o.contours, vec![vec![(4, 0), (8, 0), (8, 8), (0, 8), (0, 4), (4, 4)]]);
        assert!(!o.truncated);
    }

    #[test]
    fn ellipse_is_simplified_to_the_point_budget() {
        let m = crate::select::ellipse(RectF::new(6.0, 6.0, 506.0, 406.0), CLIP);
        let exact = trace(&m, CLIP, 1_000_000);
        assert!(!exact.truncated);
        assert_eq!(exact.contours.len(), 1);
        assert!(exact.point_count() > 800, "{}", exact.point_count());
        let o = trace(&m, CLIP, 200);
        assert!(o.truncated);
        assert_eq!(o.contours.len(), 1);
        assert!(o.point_count() <= 200 && o.point_count() >= 16, "{}", o.point_count());
        // Every kept point lies on the ellipse within a few pixels.
        for &(x, y) in &o.contours[0] {
            let (nx, ny) = ((x as f64 - 256.0) / 250.0, (y as f64 - 206.0) / 200.0);
            let r = (nx * nx + ny * ny).sqrt();
            assert!((r - 1.0).abs() < 0.02, "({x},{y}) r={r}");
        }
        // Deterministic.
        assert_eq!(o, trace(&m, CLIP, 200));
    }

    #[test]
    fn noisy_masks_stay_bounded() {
        // Checkerboard: every pixel is its own contour (worst case for point counts).
        let mut m = MaskGrid::new_empty();
        for y in 0..300 {
            for x in 0..300 {
                if (x + y) % 2 == 0 {
                    m.set(x, y, 255);
                }
            }
        }
        let o = trace(&m, CLIP, 500);
        assert!(o.truncated);
        assert!(o.point_count() <= 500);
        let lasso = crate::select::polygon(
            &[PointF::new(10.0, 10.0), PointF::new(200.0, 30.0), PointF::new(120.0, 220.0)],
            CLIP,
        );
        let o = trace(&lasso, CLIP, 4000);
        assert_eq!(o.contours.len(), 1);
        assert!(o.point_count() <= 4000);
    }
}
