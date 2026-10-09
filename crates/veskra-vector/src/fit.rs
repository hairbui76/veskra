//! Error-bounded cubic Bézier fitting of digitized curves (VEC-01).
//!
//! Independent implementation of the approach described in P. J. Schneider, "An Algorithm
//! for Automatically Fitting Digitized Curves" (Graphics Gems, 1990): least-squares cubic
//! with fixed end tangents over a chord-length parameterization, Newton–Raphson
//! reparameterization, and recursive splitting at the worst point. Additions:
//!
//! * **Preprocessing:** non-finite points are dropped; consecutive points within
//!   `DEDUPE_FRAC * max_error` of the last kept point are merged.
//! * **Corners:** a point whose turning angle (measured between chords reaching at least
//!   `2 * max_error` back and forward, to be robust against sample jitter) is at least
//!   [`FitOptions::corner_angle`] splits the input; non-maximum suppression keeps one corner
//!   per neighborhood. Pieces between corners that are straight within tolerance become
//!   `LineTo`.
//! * **Closed loops:** detected when the last point is within the merge distance of the
//!   first (or forced via [`FitOptions::closed`]); emitted with `Close`.
//! * **No recursion:** an explicit work stack is used, so 10k+ point inputs cannot overflow
//!   the thread stack.
//!
//! ## Guarantee
//! For every finite input point `P`, `distance(P, output) <= max_error` (up to floating
//! point rounding of ~1e-12 relative). Proof sketch: each kept point `P_i` of a fitted cubic
//! satisfies `|B(u_i) - P_i| <= fit_tol` (the acceptance test uses the parametric error,
//! which bounds the true distance from above); straight pieces are accepted only if all
//! points are within `fit_tol` of the chord segment; two-point pieces interpolate exactly;
//! merged points are within `d = DEDUPE_FRAC * max_error` of a kept point; and
//! `fit_tol + d < max_error`. Termination is guaranteed because every split happens at an
//! interior index. The guarantee is verified in tests with [`crate::max_distance_to_path`].

use serde::{Deserialize, Serialize};
use veskra_core::PointF;

use crate::cubic::CubicBez;
use crate::path::{BezPath, PathEl};
use crate::vec2::{V2, dist_to_segment};

/// Default corner threshold: turning angle of 60° (interior angle ≤ 120°).
pub const DEFAULT_CORNER_ANGLE: f64 = std::f64::consts::FRAC_PI_3;

/// Fraction of `max_error` under which consecutive points are merged.
const DEDUPE_FRAC: f64 = 0.01;
/// Newton reparameterization is attempted when the error is below this multiple of the target.
const REPARAM_LIMIT: f64 = 16.0;
const MAX_REPARAM_ITERS: usize = 6;
/// Maximum points scanned when looking for a window-distance neighbor.
const MAX_WINDOW_SCAN: usize = 512;

/// Options for [`fit_curve_with`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FitOptions {
    /// Maximum allowed distance from any input point to the output path (document px).
    /// Non-positive or non-finite values produce the exact polyline through the input.
    pub max_error: f64,
    /// Turning angle (radians) at or above which a point is treated as a corner.
    pub corner_angle: f64,
    /// `None`: auto-detect closure (last point within the merge distance of the first);
    /// `Some(true)`: force a closed loop (needs ≥ 3 distinct points); `Some(false)`: open.
    pub closed: Option<bool>,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self { max_error: 0.5, corner_angle: DEFAULT_CORNER_ANGLE, closed: None }
    }
}

/// Fit an error-bounded path through `points` (an open centerline, unless it closes on
/// itself — see [`FitOptions::closed`]).
///
/// Degenerate inputs: no finite points → empty path; a single distinct point →
/// `MoveTo(p) LineTo(p)` (zero-length segment, renders as a dot with round caps);
/// two distinct points → a straight line.
pub fn fit_curve(points: &[PointF], max_error: f64) -> BezPath {
    fit_curve_with(points, &FitOptions { max_error, ..FitOptions::default() })
}

/// [`fit_curve`] with explicit options.
pub fn fit_curve_with(points: &[PointF], opts: &FitOptions) -> BezPath {
    let marked: Vec<(V2, bool)> = points.iter().map(|&p| (V2::from(p), false)).collect();
    fit_marked(&marked, opts)
}

/// Fit with caller-supplied forced corners (`true` flags).
pub(crate) fn fit_marked(input: &[(V2, bool)], opts: &FitOptions) -> BezPath {
    let max_error = opts.max_error;
    let exact = !(max_error.is_finite() && max_error > 0.0);
    let merge_d = if exact { 0.0 } else { max_error * DEDUPE_FRAC };

    let mut pts: Vec<V2> = Vec::with_capacity(input.len());
    let mut forced: Vec<bool> = Vec::with_capacity(input.len());
    for &(p, c) in input {
        if !p.is_finite() {
            continue;
        }
        if let (Some(last), Some(lc)) = (pts.last(), forced.last_mut())
            && p.dist(*last) <= merge_d
        {
            *lc |= c;
            continue;
        }
        pts.push(p);
        forced.push(c);
    }

    let mut out = BezPath::new();
    match pts.len() {
        0 => return out,
        1 => {
            out.move_to(pts[0].pt());
            out.line_to(pts[0].pt());
            return out;
        }
        _ => {}
    }

    // Closure.
    let n0 = pts.len();
    let ends_meet = pts[0].dist(pts[n0 - 1]) <= merge_d;
    let mut closed = match opts.closed {
        Some(c) => c,
        None => ends_meet && n0 >= 4,
    };
    if closed && ends_meet {
        let c = forced.pop().unwrap_or(false);
        pts.pop();
        forced[0] |= c;
    }
    if pts.len() < 3 {
        closed = false;
    }
    let n = pts.len();

    out.move_to(pts[0].pt());
    if exact || (!closed && n == 2) {
        for p in &pts[1..] {
            out.line_to(p.pt());
        }
        if closed {
            out.close_path();
        }
        return out;
    }

    let fit_tol = max_error * (1.0 - DEDUPE_FRAC) * 0.999;
    let window = 2.0 * max_error;
    let corner_angle = if opts.corner_angle.is_finite() { opts.corner_angle } else { DEFAULT_CORNER_ANGLE };
    let detected = detect_corners(&pts, closed, window, corner_angle);
    let is_corner: Vec<bool> = forced.iter().zip(&detected).map(|(a, b)| *a || *b).collect();
    let fitter = Fitter { tol: fit_tol, window };

    if !closed {
        let mut idx: Vec<usize> = vec![0];
        idx.extend((1..n - 1).filter(|&i| is_corner[i]));
        idx.push(n - 1);
        for w in idx.windows(2) {
            fitter.fit_piece(&pts, w[0], w[1], &mut out.els);
        }
        return out;
    }

    let first_corner = is_corner.iter().position(|&c| c);
    match first_corner {
        None => {
            // Smooth loop: same tangent direction at the seam.
            let mut ext = pts.clone();
            ext.push(pts[0]);
            let back = window_neighbor(&pts, 0, -1, true, window);
            let fwd = window_neighbor(&pts, 0, 1, true, window);
            let t = (pts[fwd] - pts[back])
                .normalize()
                .or_else(|| (pts[1] - pts[0]).normalize())
                .unwrap_or(V2::new(1.0, 0.0));
            fitter.fit_range(&ext, 0, n, t, -t, &mut out.els);
        }
        Some(c0) => {
            let mut ext: Vec<V2> = pts[c0..].iter().chain(pts[..c0].iter()).copied().collect();
            ext.push(pts[c0]);
            out.els.clear();
            out.move_to(ext[0].pt());
            let mut idx: Vec<usize> = (0..n).filter(|&i| is_corner[(i + c0) % n]).collect();
            idx.push(n);
            for w in idx.windows(2) {
                fitter.fit_piece(&ext, w[0], w[1], &mut out.els);
            }
        }
    }
    out.close_path();
    out
}

/// Index of the first point at distance ≥ `window` from `pts[i]` stepping by `dir`
/// (±1), or the farthest point reachable within the scan limits.
fn window_neighbor(pts: &[V2], i: usize, dir: isize, closed: bool, window: f64) -> usize {
    let n = pts.len() as isize;
    let limit = if closed { ((n - 1) / 2).max(1) } else { n };
    let mut last = i;
    let mut k: isize = 1;
    while k <= limit && (k as usize) <= MAX_WINDOW_SCAN {
        let j = i as isize + dir * k;
        let j = if closed {
            j.rem_euclid(n)
        } else if j < 0 || j >= n {
            break;
        } else {
            j
        } as usize;
        last = j;
        if pts[j].dist(pts[i]) >= window {
            break;
        }
        k += 1;
    }
    last
}

/// Corner detection with window-based turning angles and non-maximum suppression.
fn detect_corners(pts: &[V2], closed: bool, window: f64, angle: f64) -> Vec<bool> {
    let n = pts.len();
    let mut turn = vec![0.0f64; n];
    let mut range = vec![(0usize, 0usize); n];
    let mut cand = vec![false; n];
    for i in 0..n {
        if !closed && (i == 0 || i == n - 1) {
            continue;
        }
        let b = window_neighbor(pts, i, -1, closed, window);
        let f = window_neighbor(pts, i, 1, closed, window);
        if b == i || f == i {
            continue;
        }
        let vin = pts[i] - pts[b];
        let vout = pts[f] - pts[i];
        let t = vin.cross(vout).abs().atan2(vin.dot(vout));
        turn[i] = t;
        range[i] = (b, f);
        cand[i] = t >= angle;
    }
    let mut out = vec![false; n];
    for i in 0..n {
        if !cand[i] {
            continue;
        }
        // Compete only with candidates strictly inside the window (between the window
        // neighbors b and f, which are the first points at distance >= window), so two
        // genuine corners one sample apart (sparse input) both survive.
        let (b, f) = range[i];
        let next = |j: usize| if closed { (j + 1) % n } else { j + 1 };
        let mut keep = true;
        let mut j = next(b);
        let mut guard = 0;
        while j != f && guard <= n {
            if j != i && cand[j] && pts[j].dist(pts[i]) < window && (turn[j] > turn[i] || (turn[j] == turn[i] && j < i))
            {
                keep = false;
                break;
            }
            j = next(j);
            guard += 1;
        }
        out[i] = keep;
    }
    out
}

struct Fitter {
    tol: f64,
    window: f64,
}

impl Fitter {
    /// Fit a top-level piece between two corners (or path ends).
    fn fit_piece(&self, pts: &[V2], a: usize, b: usize, out: &mut Vec<PathEl>) {
        if b <= a {
            return;
        }
        if b - a == 1 || (a + 1..b).all(|i| dist_to_segment(pts[i], pts[a], pts[b]) <= self.tol) {
            out.push(PathEl::LineTo(pts[b].pt()));
            return;
        }
        let t1 = self.end_tangent(pts, a, b, true);
        let t2 = self.end_tangent(pts, a, b, false);
        self.fit_range(pts, a, b, t1, t2, out);
    }

    /// Unit tangent at the start (pointing forward) or end (pointing backward) of `a..=b`.
    fn end_tangent(&self, pts: &[V2], a: usize, b: usize, at_start: bool) -> V2 {
        let (o, mut j, step): (usize, usize, isize) = if at_start { (a, a + 1, 1) } else { (b, b - 1, -1) };
        let mut scanned = 0;
        loop {
            if pts[j].dist(pts[o]) >= self.window || j == if at_start { b } else { a } || scanned >= MAX_WINDOW_SCAN {
                break;
            }
            j = (j as isize + step) as usize;
            scanned += 1;
        }
        let near = if at_start { a + 1 } else { b - 1 };
        (pts[j] - pts[o]).normalize().or_else(|| (pts[near] - pts[o]).normalize()).unwrap_or(V2::new(1.0, 0.0))
    }

    fn center_tangent(&self, pts: &[V2], f: usize, l: usize, s: usize) -> V2 {
        let mut b = s - 1;
        let mut scanned = 0;
        while b > f && pts[b].dist(pts[s]) < self.window && scanned < MAX_WINDOW_SCAN {
            b -= 1;
            scanned += 1;
        }
        let mut fw = s + 1;
        scanned = 0;
        while fw < l && pts[fw].dist(pts[s]) < self.window && scanned < MAX_WINDOW_SCAN {
            fw += 1;
            scanned += 1;
        }
        (pts[b] - pts[fw])
            .normalize()
            .or_else(|| (pts[s - 1] - pts[s + 1]).normalize())
            .or_else(|| (pts[s - 1] - pts[s]).normalize())
            .unwrap_or(V2::new(-1.0, 0.0))
    }

    /// Schneider fit of `pts[f..=l]` with unit tangents `t1` (forward at start) and `t2`
    /// (backward at end). Appends `CubicTo`s in order. Uses an explicit stack.
    fn fit_range(&self, pts: &[V2], f: usize, l: usize, t1: V2, t2: V2, out: &mut Vec<PathEl>) {
        let mut stack: Vec<(usize, usize, V2, V2)> = vec![(f, l, t1, t2)];
        while let Some((f, l, t1, t2)) = stack.pop() {
            let p0 = pts[f];
            let p3 = pts[l];
            if l - f == 1 {
                let d = p0.dist(p3) / 3.0;
                emit(out, [p0, p0 + t1 * d, p3 + t2 * d, p3]);
                continue;
            }
            let mut u = chord_params(pts, f, l);
            let mut bez = generate(pts, f, l, &u, t1, t2);
            let (mut err, mut split) = max_error(pts, f, l, &u, &bez);
            if err > self.tol && err <= self.tol * REPARAM_LIMIT {
                for _ in 0..MAX_REPARAM_ITERS {
                    reparameterize(pts, f, &mut u, &bez);
                    let cand = generate(pts, f, l, &u, t1, t2);
                    let (e2, s2) = max_error(pts, f, l, &u, &cand);
                    if e2 < err {
                        bez = cand;
                        err = e2;
                        split = s2;
                    }
                    if err <= self.tol {
                        break;
                    }
                }
            }
            if err <= self.tol {
                emit(out, bez);
                continue;
            }
            let tc = self.center_tangent(pts, f, l, split);
            // Push right half first so the left half is emitted first.
            stack.push((split, l, -tc, t2));
            stack.push((f, split, t1, tc));
        }
    }
}

fn emit(out: &mut Vec<PathEl>, b: [V2; 4]) {
    out.push(PathEl::CubicTo(b[1].pt(), b[2].pt(), b[3].pt()));
}

fn chord_params(pts: &[V2], f: usize, l: usize) -> Vec<f64> {
    let mut u = Vec::with_capacity(l - f + 1);
    u.push(0.0);
    let mut acc = 0.0;
    for i in f + 1..=l {
        acc += pts[i].dist(pts[i - 1]);
        u.push(acc);
    }
    if acc > 0.0 {
        for v in &mut u {
            *v /= acc;
        }
    }
    u
}

fn bernstein(t: f64) -> [f64; 4] {
    let mt = 1.0 - t;
    [mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t]
}

/// Least-squares handle lengths for fixed tangents (Schneider's `GenerateBezier`), with the
/// Wu/Barsky fallback when the solution is degenerate, negative or implausibly long.
fn generate(pts: &[V2], f: usize, l: usize, u: &[f64], t1: V2, t2: V2) -> [V2; 4] {
    let p0 = pts[f];
    let p3 = pts[l];
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (k, &t) in u.iter().enumerate() {
        let b = bernstein(t);
        let a1 = t1 * b[1];
        let a2 = t2 * b[2];
        c00 += a1.dot(a1);
        c01 += a1.dot(a2);
        c11 += a2.dot(a2);
        let tmp = pts[f + k] - (p0 * (b[0] + b[1]) + p3 * (b[2] + b[3]));
        x0 += a1.dot(tmp);
        x1 += a2.dot(tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let chord = p0.dist(p3);
    let arc: f64 = (f + 1..=l).map(|i| pts[i].dist(pts[i - 1])).sum();
    let eps = 1e-6 * arc.max(chord);
    let (mut al, mut ar) = (f64::NAN, f64::NAN);
    if det.abs() > 1e-12 * (c00 * c11).abs() && det != 0.0 {
        al = (x0 * c11 - x1 * c01) / det;
        ar = (c00 * x1 - c01 * x0) / det;
    }
    let ok = |a: f64| a.is_finite() && a > eps && a <= arc;
    if !ok(al) || !ok(ar) {
        let d = if chord > 0.1 * arc { chord } else { arc } / 3.0;
        al = d;
        ar = d;
    }
    [p0, p0 + t1 * al, p3 + t2 * ar, p3]
}

/// Maximum parametric error `|B(u_i) - P_i|` over interior points and its index.
fn max_error(pts: &[V2], f: usize, l: usize, u: &[f64], bez: &[V2; 4]) -> (f64, usize) {
    let c = CubicBez::from_v(*bez);
    let mut best = (0.0, (f + l) / 2);
    for i in f + 1..l {
        let d = c.eval_v(u[i - f]).dist(pts[i]);
        if d > best.0 || !d.is_finite() {
            best = (if d.is_finite() { d } else { f64::INFINITY }, i);
        }
    }
    best
}

/// One Newton–Raphson step per point on `(B(u) - P)·B'(u) = 0`.
fn reparameterize(pts: &[V2], f: usize, u: &mut [f64], bez: &[V2; 4]) {
    let c = CubicBez::from_v(*bez);
    for (k, t) in u.iter_mut().enumerate() {
        let q = c.eval_v(*t) - pts[f + k];
        let d1 = c.deriv_v(*t);
        let d2 = c.deriv2_v(*t);
        let den = d1.len2() + q.dot(d2);
        if den.abs() > 1e-300 {
            let nt = *t - q.dot(d1) / den;
            if nt.is_finite() {
                *t = nt.clamp(0.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::max_distance_to_path;

    fn p(x: f64, y: f64) -> PointF {
        PointF::new(x, y)
    }

    #[test]
    fn degenerate_inputs() {
        assert!(fit_curve(&[], 0.5).is_empty());
        let one = fit_curve(&[p(1.0, 2.0), p(1.0, 2.0)], 0.5);
        assert_eq!(one.els, vec![PathEl::MoveTo(p(1.0, 2.0)), PathEl::LineTo(p(1.0, 2.0))]);
        let two = fit_curve(&[p(0.0, 0.0), p(f64::NAN, 1.0), p(10.0, 0.0)], 0.5);
        assert_eq!(two.els, vec![PathEl::MoveTo(p(0.0, 0.0)), PathEl::LineTo(p(10.0, 0.0))]);
        let exact = fit_curve(&[p(0.0, 0.0), p(1.0, 1.0), p(2.0, 0.0)], 0.0);
        assert_eq!(exact.els.len(), 3);
    }

    #[test]
    fn collinear_becomes_line_and_corner_is_kept() {
        let mut pts: Vec<PointF> = (0..=50).map(|i| p(i as f64, 0.0)).collect();
        pts.extend((1..=50).map(|i| p(50.0, i as f64)));
        let path = fit_curve(&pts, 0.25);
        assert_eq!(path.els.len(), 3, "{:?}", path.els);
        assert_eq!(path.els[1], PathEl::LineTo(p(50.0, 0.0)));
        assert!(max_distance_to_path(&path, &pts) <= 0.25);
    }

    #[test]
    fn closed_circle_detected() {
        let pts: Vec<PointF> = (0..=200)
            .map(|i| {
                let a = i as f64 / 200.0 * std::f64::consts::TAU;
                p(100.0 + 50.0 * a.cos(), 100.0 + 50.0 * a.sin())
            })
            .collect();
        let path = fit_curve(&pts, 0.1);
        assert_eq!(path.els.last(), Some(&PathEl::Close));
        assert!(max_distance_to_path(&path, &pts) <= 0.1);
        assert!(path.cubic_count() <= 8, "{}", path.cubic_count());
    }
}
