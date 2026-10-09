//! Cubic Bézier segment math: evaluation, subdivision, exact bounds, flattening with a
//! proven error bound, arc length and nearest-point queries.

use serde::{Deserialize, Serialize};
use veskra_core::{PointF, RectF};

use crate::vec2::V2;

/// Maximum adaptive subdivision depth for [`CubicBez::flatten_into`]. The flattening
/// guarantee holds whenever this depth is not reached, i.e. for any cubic whose second
/// differences are below `tolerance * 4^24 / 0.75` (≈ 3.7e14 × tolerance).
pub const MAX_FLATTEN_DEPTH: u32 = 24;

/// A cubic Bézier segment `p0 p1 p2 p3`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CubicBez {
    pub p0: PointF,
    pub p1: PointF,
    pub p2: PointF,
    pub p3: PointF,
}

impl CubicBez {
    pub const fn new(p0: PointF, p1: PointF, p2: PointF, p3: PointF) -> Self {
        Self { p0, p1, p2, p3 }
    }

    pub(crate) fn v(&self) -> [V2; 4] {
        [self.p0.into(), self.p1.into(), self.p2.into(), self.p3.into()]
    }

    pub(crate) fn from_v(v: [V2; 4]) -> Self {
        Self::new(v[0].pt(), v[1].pt(), v[2].pt(), v[3].pt())
    }

    /// Point at parameter `t` (`0..=1`).
    pub fn eval(&self, t: f64) -> PointF {
        self.eval_v(t).pt()
    }

    pub(crate) fn eval_v(&self, t: f64) -> V2 {
        let [p0, p1, p2, p3] = self.v();
        let mt = 1.0 - t;
        p0 * (mt * mt * mt) + p1 * (3.0 * mt * mt * t) + p2 * (3.0 * mt * t * t) + p3 * (t * t * t)
    }

    /// First derivative `B'(t)`.
    pub(crate) fn deriv_v(&self, t: f64) -> V2 {
        let [p0, p1, p2, p3] = self.v();
        let mt = 1.0 - t;
        ((p1 - p0) * (mt * mt) + (p2 - p1) * (2.0 * mt * t) + (p3 - p2) * (t * t)) * 3.0
    }

    /// Second derivative `B''(t)`.
    pub(crate) fn deriv2_v(&self, t: f64) -> V2 {
        let [p0, p1, p2, p3] = self.v();
        ((p2 - p1 * 2.0 + p0) * (1.0 - t) + (p3 - p2 * 2.0 + p1) * t) * 6.0
    }

    /// De Casteljau split at `t`.
    pub fn split(&self, t: f64) -> (CubicBez, CubicBez) {
        let [p0, p1, p2, p3] = self.v();
        let a = p0.lerp(p1, t);
        let b = p1.lerp(p2, t);
        let c = p2.lerp(p3, t);
        let d = a.lerp(b, t);
        let e = b.lerp(c, t);
        let f = d.lerp(e, t);
        (CubicBez::from_v([p0, a, d, f]), CubicBez::from_v([f, e, c, p3]))
    }

    /// The same curve traversed backwards.
    pub fn reverse(&self) -> CubicBez {
        CubicBez::new(self.p3, self.p2, self.p1, self.p0)
    }

    /// Parameters in `(0, 1)` where `B'(t)` has a zero x (`axis = 0`) or y (`axis = 1`) component.
    fn extrema_params(&self, axis: usize, out: &mut Vec<f64>) {
        let c = |p: PointF| if axis == 0 { p.x } else { p.y };
        let (p0, p1, p2, p3) = (c(self.p0), c(self.p1), c(self.p2), c(self.p3));
        // B'(t)/3 = a t^2 + b t + c
        let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
        let b = 2.0 * (p0 - 2.0 * p1 + p2);
        let cc = p1 - p0;
        solve_quadratic(a, b, cc, |t| {
            if t > 0.0 && t < 1.0 {
                out.push(t);
            }
        });
    }

    /// Exact axis-aligned bounds (endpoints plus true extrema, not control points).
    pub fn bounds(&self) -> RectF {
        let mut ts = Vec::with_capacity(4);
        self.extrema_params(0, &mut ts);
        self.extrema_params(1, &mut ts);
        RectF::from_points([self.p0, self.p3].into_iter().chain(ts.iter().map(|&t| self.eval(t))))
    }

    /// Bounds of the control polygon (a cheap conservative superset of [`Self::bounds`]).
    pub fn control_bounds(&self) -> RectF {
        RectF::from_points([self.p0, self.p1, self.p2, self.p3])
    }

    /// Upper bound on the distance between the curve and its chord `p0..p3` *parametrized
    /// linearly*: `max_t |B(t) - L(t)| <= (1/8) max|B''| <= 0.75 * max(|Δ²₀|, |Δ²₁|)`.
    pub fn flatness_bound(&self) -> f64 {
        let [p0, p1, p2, p3] = self.v();
        let m = (p0 - p1 * 2.0 + p2).len().max((p1 - p2 * 2.0 + p3).len());
        0.75 * m
    }

    /// Append a polyline approximation (excluding `p0`, including `p3`). Every point of the
    /// curve is within `tolerance` of the polyline and vice versa (parametric Hausdorff
    /// bound from [`Self::flatness_bound`]), provided [`MAX_FLATTEN_DEPTH`] is not hit.
    pub fn flatten_into(&self, tolerance: f64, out: &mut Vec<PointF>) {
        self.flatten_params(tolerance, |_, p| out.push(p));
    }

    /// Like [`Self::flatten_into`] but reports `(t, point)` pairs (excluding `t = 0`).
    pub fn flatten_params(&self, tolerance: f64, mut sink: impl FnMut(f64, PointF)) {
        let tol = sanitize_tolerance(tolerance);
        // Stack of (sub-curve, t0, t1, depth); pop order = curve order.
        let mut stack: Vec<(CubicBez, f64, f64, u32)> = vec![(*self, 0.0, 1.0, 0)];
        while let Some((c, t0, t1, depth)) = stack.pop() {
            let fb = c.flatness_bound();
            if fb <= tol || depth >= MAX_FLATTEN_DEPTH || !fb.is_finite() {
                sink(t1, c.p3);
            } else {
                let (a, b) = c.split(0.5);
                let tm = 0.5 * (t0 + t1);
                stack.push((b, tm, t1, depth + 1));
                stack.push((a, t0, tm, depth + 1));
            }
        }
    }

    /// Arc length, accurate to roughly `accuracy` (absolute, document px).
    pub fn arc_length(&self, accuracy: f64) -> f64 {
        let acc = sanitize_tolerance(accuracy);
        let mut total = 0.0;
        let mut stack: Vec<(CubicBez, u32)> = vec![(*self, 0)];
        while let Some((c, depth)) = stack.pop() {
            let [p0, p1, p2, p3] = c.v();
            let chord = p0.dist(p3);
            let poly = p0.dist(p1) + p1.dist(p2) + p2.dist(p3);
            if poly - chord <= acc || depth >= 20 || !poly.is_finite() {
                // Gravesen's estimate for cubics: (2*chord + 2*poly) / 4.
                total += 0.5 * (chord + poly);
            } else {
                let (a, b) = c.split(0.5);
                stack.push((a, depth + 1));
                stack.push((b, depth + 1));
            }
        }
        total
    }

    /// Distance from `p` to the curve (true point-to-curve distance; see
    /// [`crate::max_distance_to_path`] for the method and accuracy statement).
    pub fn distance_to(&self, p: PointF) -> f64 {
        let samples = self.distance_samples();
        cubic_distance(self, &samples, p.into(), f64::INFINITY)
    }

    /// Uniform parameter samples dense enough that the chord polyline is within
    /// `DIST_REL_TOL * size` of the curve (rigorous bound, uniform `h = 1/n`).
    pub(crate) fn distance_samples(&self) -> Vec<(f64, V2)> {
        let size = {
            let r = self.control_bounds();
            (r.x1 - r.x0).max(r.y1 - r.y0)
        };
        let tol = (size * DIST_REL_TOL).max(1e-12);
        // deviation <= 0.75 M h^2 with h = 1/n
        let m = self.flatness_bound() / 0.75;
        let n = if m.is_finite() && m > 0.0 { (0.75 * m / tol).sqrt().ceil().clamp(1.0, 4096.0) as usize } else { 1 };
        (0..=n)
            .map(|i| {
                let t = i as f64 / n as f64;
                (t, self.eval_v(t))
            })
            .collect()
    }
}

/// Relative accuracy of the dense sampling used in distance queries (see
/// [`crate::max_distance_to_path`]).
pub(crate) const DIST_REL_TOL: f64 = 1e-3;

/// Distance from `p` to cubic `c` given its dense samples. Candidate chord intervals whose
/// chord distance could still beat `best` are refined with clamped Newton iterations on
/// `f(t) = (B(t) - p)·B'(t)`. Returns the minimum of distances to *actual curve points*,
/// so the result is never below the true distance; with the Newton refinement it is
/// typically exact to ~1e-12 relative.
pub(crate) fn cubic_distance(c: &CubicBez, samples: &[(f64, V2)], p: V2, best_in: f64) -> f64 {
    let mut best = best_in;
    let size = {
        let r = c.control_bounds();
        (r.x1 - r.x0).max(r.y1 - r.y0)
    };
    let slack = 2.0 * (size * DIST_REL_TOL).max(1e-12);
    // First pass: chord distances.
    let mut chord_d: Vec<f64> = Vec::with_capacity(samples.len().saturating_sub(1));
    let mut chord_best = f64::INFINITY;
    for w in samples.windows(2) {
        let d = crate::vec2::dist_to_segment(p, w[0].1, w[1].1);
        chord_best = chord_best.min(d);
        chord_d.push(d);
    }
    if samples.len() == 1 {
        return best.min(p.dist(samples[0].1));
    }
    // The curve point nearest p lies within `slack/2` of its chord interval, hence that
    // interval's chord distance is <= true_dist + slack/2 <= chord_best + slack.
    let limit = chord_best + slack;
    for (i, w) in samples.windows(2).enumerate() {
        let d = chord_d[i];
        if d > limit || d - slack > best {
            continue;
        }
        let (t0, a) = w[0];
        let (t1, b) = w[1];
        best = best.min(p.dist(a)).min(p.dist(b));
        let ab = b - a;
        let l2 = ab.len2();
        let s = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let mut t = t0 + (t1 - t0) * s;
        for _ in 0..12 {
            let q = c.eval_v(t) - p;
            let d1 = c.deriv_v(t);
            let d2 = c.deriv2_v(t);
            let num = q.dot(d1);
            let den = d1.len2() + q.dot(d2);
            if den.is_nan() || den.abs() <= 1e-300 {
                break;
            }
            let nt = (t - num / den).clamp(t0, t1);
            let done = (nt - t).abs() < 1e-14;
            t = nt;
            if done {
                break;
            }
        }
        best = best.min(p.dist(c.eval_v(t)));
    }
    best
}

pub(crate) fn sanitize_tolerance(tol: f64) -> f64 {
    if tol.is_finite() && tol > 1e-9 { tol } else { 1e-9 }
}

/// Real roots of `a t^2 + b t + c = 0` (numerically stable form).
pub(crate) fn solve_quadratic(a: f64, b: f64, c: f64, mut f: impl FnMut(f64)) {
    let scale = a.abs().max(b.abs()).max(c.abs());
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }
    let (a, b, c) = (a / scale, b / scale, c / scale);
    if a.abs() < 1e-12 {
        if b.abs() > 1e-300 {
            f(-c / b);
        }
        return;
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return;
    }
    let sq = disc.sqrt();
    let q = -0.5 * (b + if b >= 0.0 { sq } else { -sq });
    f(q / a);
    if q.abs() > 1e-300 {
        let r2 = c / q;
        f(r2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> PointF {
        PointF::new(x, y)
    }

    #[test]
    fn bounds_use_true_extrema() {
        // Symmetric arch: control points reach y = 100, curve max is 75.
        let c = CubicBez::new(p(0.0, 0.0), p(0.0, 100.0), p(100.0, 100.0), p(100.0, 0.0));
        let b = c.bounds();
        assert!((b.y1 - 75.0).abs() < 1e-12, "{b:?}");
        assert_eq!((b.x0, b.x1, b.y0), (0.0, 100.0, 0.0));
        // S-shape with x extrema beyond endpoints.
        let s = CubicBez::new(p(0.0, 0.0), p(-50.0, 30.0), p(150.0, 60.0), p(100.0, 90.0));
        let sb = s.bounds();
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..=100_000 {
            let q = s.eval(i as f64 / 100_000.0);
            lo = lo.min(q.x);
            hi = hi.max(q.x);
        }
        assert!((sb.x0 - lo).abs() < 1e-6 && (sb.x1 - hi).abs() < 1e-6);
        assert!(sb.x0 > -50.0 && sb.x1 < 150.0);
    }

    #[test]
    fn flatten_respects_tolerance() {
        let c = CubicBez::new(p(0.0, 0.0), p(300.0, -200.0), p(-100.0, 400.0), p(250.0, 120.0));
        for &tol in &[1.0, 0.25, 0.01] {
            let mut pts = vec![c.p0];
            let mut ts = vec![0.0];
            c.flatten_params(tol, |t, q| {
                ts.push(t);
                pts.push(q);
            });
            // Check parametric deviation at many points inside each interval.
            for k in 0..ts.len() - 1 {
                for j in 1..8 {
                    let s = j as f64 / 8.0;
                    let t = ts[k] + (ts[k + 1] - ts[k]) * s;
                    let on_curve: V2 = c.eval_v(t);
                    let on_chord = V2::from(pts[k]).lerp(pts[k + 1].into(), s);
                    assert!(on_curve.dist(on_chord) <= tol * (1.0 + 1e-9));
                }
            }
        }
    }

    #[test]
    fn arc_length_of_quarter_circle() {
        let k = 0.552_284_749_830_793_4;
        let c = CubicBez::new(p(1.0, 0.0), p(1.0, k), p(k, 1.0), p(0.0, 1.0));
        let l = c.arc_length(1e-9) * 100.0;
        assert!((l - std::f64::consts::FRAC_PI_2 * 100.0).abs() < 0.03, "{l}");
    }

    #[test]
    fn distance_matches_brute_force() {
        let c = CubicBez::new(p(0.0, 0.0), p(40.0, 80.0), p(90.0, -60.0), p(120.0, 30.0));
        for q in [p(50.0, 50.0), p(-10.0, 3.0), p(60.0, 0.0), p(200.0, 200.0)] {
            let d = c.distance_to(q);
            let mut bf = f64::INFINITY;
            for i in 0..=200_000 {
                bf = bf.min(c.eval(i as f64 / 200_000.0).distance(q));
            }
            assert!(d <= bf + 1e-9 && d >= bf - 1e-3, "{d} vs {bf}");
        }
    }
}
