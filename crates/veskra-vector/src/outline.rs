//! Pressure-width stroke outlines (VEC-02).
//!
//! ## Model
//! A stroke is a polyline of centers `c_i` with radii `r_i = width_i / 2` (widths clamped
//! to `min_width`), linearly interpolated between samples. The painted region is the union
//! of all interpolated discs, which equals the union over segments of the convex hulls of
//! consecutive end discs ("tapered capsules"; if one end disc contains the other, the hull is
//! that disc — the `|dr/ds| > 1` case where the classical offset curve does not exist).
//!
//! ## Construction (single closed contour, nonzero fill)
//! For each segment the two external tangent lines have unit normals
//! `n± = cosψ·u ± sinψ·v` with `cosψ = (r_i − r_{i+1}) / |c_{i+1} − c_i|` clamped to
//! `[−1, 1]` (clamping yields exactly the "one disc contains the other" case). The contour
//! runs along the `n−` lines forward, around the end cap, along the `n+` lines backward and
//! around the start cap. At each joint `j` the two sides are connected by arcs of circle `j`
//! with signed sweeps `a_j` (n−) and `b_j` (n+), chosen in `(−π, π]`.
//!
//! **Coverage argument.** As 1-chains, the contour equals `Σ ∂capsule_i + Σ k_j ∂disc_j`,
//! where `k_j = (a_j + b_j − 2ψ_{j−1} − (2π − 2ψ_j)) / 2π ∈ {−1, 0, 1}` (we add 2π to an arc
//! if it would be −2). Every capsule boundary is positively oriented, so the winding number
//! is `Σ 1[capsule_i] + Σ k_j 1[disc_j]`. A point inside `m` discs with `k = −1` lies in at
//! least `m + 1` distinct capsules (`disc_j ⊂ capsule_{j−1} ∩ capsule_j`), hence the winding is
//! ≥ 1 everywhere inside the union and 0 outside: **no notches and no spikes**, regardless of
//! sharp turns, cusps, hairpins or width jumps. Self-overlap of the contour is expected and
//! harmless under the nonzero rule (do not render with even-odd).
//!
//! **Inner-join shortcut.** Where both tangent lines adjacent to a backward (inner) arc
//! intersect at a point `X` lying on both segments, the overshoot `X → f → arc → f' → X` is
//! removed. That loop bounds a region inside `conv({X} ∪ disc_j) ⊂ capsule_{j−1} ∩ capsule_j`
//! with winding ±1, so the argument above still holds (at most one shortcut per joint unless the
//! two arcs' angular sectors are disjoint). This keeps the inner side of smooth curves clean
//! so that fitting produces few nodes.
//!
//! ## Approximation budget (`tolerance = τ`)
//! * centerline (x, y, r) Douglas–Peucker simplification: union moves ≤ 0.2 τ;
//! * arc flattening (chords inside the discs): ≤ 0.05 τ;
//! * cubic fitting of the contour polyline with [`crate::fit_curve`] semantics: ≤ 0.7 τ at
//!   contour vertices (vertices are spaced ≤ 3 τ along straight runs).
//!
//! ## What is and is NOT preserved (MASTER_PLAN §2.3)
//! The output is an **editable filled path** (anchor points + handles) whose filled area
//! matches the painted stroke within `τ`. It is **not** an Illustrator pressure-aware brush
//! stroke or variable-width profile: the centerline, the per-sample pressure/width and the
//! brush identity are not represented in the outline. Re-editing width in Illustrator means
//! editing the outline's anchors. The centerline can be exported separately with
//! [`crate::fit_curve`] (e.g. as project metadata) but must not be presented as editable
//! pressure dynamics.

use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use veskra_core::PointF;

use crate::fit::{DEFAULT_CORNER_ANGLE, FitOptions, fit_marked};
use crate::path::BezPath;
use crate::vec2::{V2, signed_angle};

const SIMPLIFY_FRAC: f64 = 0.2;
const FLATTEN_FRAC: f64 = 0.05;
const FIT_FRAC: f64 = 0.7;
const DENSIFY_FRAC: f64 = 3.0;
/// Default outline tolerance in document px.
pub const DEFAULT_OUTLINE_TOLERANCE: f64 = 0.25;

/// A centerline sample with its full stroke width (diameter, document px).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WidthPoint {
    pub p: PointF,
    pub width: f64,
}

impl WidthPoint {
    pub const fn new(x: f64, y: f64, width: f64) -> Self {
        Self { p: PointF::new(x, y), width }
    }
}

/// Stroke end style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Cap {
    /// Half-disc caps: the outline covers exactly the union of the swept discs.
    #[default]
    Round,
    /// Flat ends: the cap is the chord between the two tangent points of the end disc
    /// (perpendicular to the end segment; through the center for constant width).
    Butt,
}

/// Options for [`variable_width_outline`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OutlineOptions {
    /// Maximum geometric deviation of the outline (document px). Non-finite or
    /// non-positive → [`DEFAULT_OUTLINE_TOLERANCE`]; values below 1e-4 are raised to 1e-4.
    pub tolerance: f64,
    pub cap: Cap,
    /// Widths below this are raised to it (document px).
    pub min_width: f64,
}

impl Default for OutlineOptions {
    fn default() -> Self {
        Self { tolerance: DEFAULT_OUTLINE_TOLERANCE, cap: Cap::Round, min_width: 0.0 }
    }
}

/// Diagnostics for an outline (node counts are what an editor will show).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OutlineReport {
    /// Finite input samples.
    pub input_points: usize,
    /// Centerline nodes after duplicate merging and simplification.
    pub centerline_nodes: usize,
    /// Vertices of the exact (unfitted) contour polyline.
    pub contour_points: usize,
    /// Inner joins resolved by the tangent-intersection shortcut.
    pub shortcut_joins: usize,
    /// Anchor points of the final path ([`BezPath::node_count`]).
    pub anchors: usize,
}

#[derive(Debug, Clone, Copy)]
struct Node {
    c: V2,
    r: f64,
}

enum Built {
    Empty,
    Circle(V2, f64),
    Contour(Vec<(V2, bool)>),
}

fn sanitize_tol(t: f64) -> f64 {
    if t.is_finite() && t > 0.0 { t.max(1e-4) } else { DEFAULT_OUTLINE_TOLERANCE }
}

/// Closed filled outline (nonzero fill rule) of a pressure-varying stroke.
///
/// Degenerate cases: no finite samples or all widths zero → empty path; all samples at
/// one location (or a single sample) → a circle of the largest width (for either cap).
pub fn variable_width_outline(pts: &[WidthPoint], opts: &OutlineOptions) -> BezPath {
    variable_width_outline_report(pts, opts).0
}

/// [`variable_width_outline`] plus diagnostics.
pub fn variable_width_outline_report(pts: &[WidthPoint], opts: &OutlineOptions) -> (BezPath, OutlineReport) {
    let tol = sanitize_tol(opts.tolerance);
    let (built, mut report) = build(pts, opts);
    let path = match built {
        Built::Empty => BezPath::new(),
        Built::Circle(c, r) => circle_path(c, r, tol),
        Built::Contour(contour) => fit_marked(
            &contour,
            &FitOptions { max_error: FIT_FRAC * tol, corner_angle: DEFAULT_CORNER_ANGLE, closed: Some(true) },
        ),
    };
    report.anchors = path.node_count();
    (path, report)
}

/// The exact contour before cubic fitting, as a closed polyline (first point not repeated).
/// Useful for diagnostics and for verifying the fitting step.
pub fn outline_contour(pts: &[WidthPoint], opts: &OutlineOptions) -> Vec<PointF> {
    let tol = sanitize_tol(opts.tolerance);
    match build(pts, opts).0 {
        Built::Empty => Vec::new(),
        Built::Circle(c, r) => {
            let mut e = Emitter::new(tol);
            e.start(c + V2::new(r, 0.0), false);
            e.arc(c, r, V2::new(1.0, 0.0), TAU);
            e.pts.pop();
            e.pts.iter().map(|(p, _)| p.pt()).collect()
        }
        Built::Contour(c) => c.iter().map(|(p, _)| p.pt()).collect(),
    }
}

fn build(pts: &[WidthPoint], opts: &OutlineOptions) -> (Built, OutlineReport) {
    let tol = sanitize_tol(opts.tolerance);
    let min_w = if opts.min_width.is_finite() && opts.min_width > 0.0 { opts.min_width } else { 0.0 };
    let mut report = OutlineReport::default();
    let merge = tol * 1e-3;
    let mut nodes: Vec<Node> = Vec::with_capacity(pts.len());
    for wp in pts {
        let c = V2::from(wp.p);
        if !c.is_finite() || wp.width.is_nan() {
            continue;
        }
        report.input_points += 1;
        let w = wp.width.max(min_w).max(0.0);
        let r = if w.is_finite() { 0.5 * w } else { f64::MAX.sqrt() };
        if let Some(last) = nodes.last_mut()
            && last.c.dist(c) <= merge
        {
            last.r = last.r.max(r);
            continue;
        }
        nodes.push(Node { c, r });
    }
    if nodes.is_empty() || nodes.iter().all(|n| n.r.is_nan() || n.r <= 0.0) {
        return (Built::Empty, report);
    }
    let nodes = simplify(&nodes, SIMPLIFY_FRAC * tol / std::f64::consts::SQRT_2);
    report.centerline_nodes = nodes.len();
    if nodes.len() == 1 {
        return (Built::Circle(nodes[0].c, nodes[0].r), report);
    }
    let (contour, shortcuts) = contour(&nodes, opts.cap, tol);
    report.contour_points = contour.len();
    report.shortcut_joins = shortcuts;
    (Built::Contour(contour), report)
}

/// Douglas–Peucker in (x, y, r) space: every dropped node is within `eps` (3D Euclidean,
/// hence within `√2·eps` in `|Δc| + |Δr|`) of the kept chord, which bounds the Hausdorff
/// distance between the original and simplified disc unions.
fn simplify(nodes: &[Node], eps: f64) -> Vec<Node> {
    let n = nodes.len();
    if n <= 2 {
        return nodes.to_vec();
    }
    let p3 = |k: &Node| [k.c.x, k.c.y, k.r];
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (p3(&nodes[a]), p3(&nodes[b]));
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let l2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let mut worst = (0.0, a);
        for (k, node) in nodes.iter().enumerate().take(b).skip(a + 1) {
            let q = p3(node);
            let aq = [q[0] - pa[0], q[1] - pa[1], q[2] - pa[2]];
            let t = if l2 > 0.0 { ((aq[0] * ab[0] + aq[1] * ab[1] + aq[2] * ab[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d = ((aq[0] - ab[0] * t).powi(2) + (aq[1] - ab[1] * t).powi(2) + (aq[2] - ab[2] * t).powi(2)).sqrt();
            if d > worst.0 {
                worst = (d, k);
            }
        }
        if worst.0 > eps {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    nodes.iter().zip(keep).filter(|(_, k)| *k).map(|(n, _)| *n).collect()
}

struct Emitter {
    pts: Vec<(V2, bool)>,
    smax: f64,
    tol_f: f64,
}

impl Emitter {
    fn new(tol: f64) -> Self {
        Self { pts: Vec::new(), smax: DENSIFY_FRAC * tol, tol_f: FLATTEN_FRAC * tol }
    }
    fn start(&mut self, p: V2, corner: bool) {
        self.pts.push((p, corner));
    }
    fn line_to(&mut self, p: V2, corner: bool) {
        let Some(&(from, _)) = self.pts.last() else {
            self.pts.push((p, corner));
            return;
        };
        let len = from.dist(p);
        if len.is_nan() || len <= 0.0 {
            if let Some(last) = self.pts.last_mut() {
                last.1 |= corner;
            }
            return;
        }
        let steps = (len / self.smax).ceil().clamp(1.0, 1e6) as usize;
        for i in 1..steps {
            self.pts.push((from.lerp(p, i as f64 / steps as f64), false));
        }
        self.pts.push((p, corner));
    }
    /// Arc on circle (c, r) from direction `n0` with signed `sweep` (excluding the start).
    fn arc(&mut self, c: V2, r: f64, n0: V2, sweep: f64) {
        if r.is_nan() || r <= 0.0 || sweep == 0.0 {
            return;
        }
        let dmax = if r <= self.tol_f { FRAC_PI_2 } else { (2.0 * (1.0 - self.tol_f / r).acos()).min(FRAC_PI_2) };
        let steps = (sweep.abs() / dmax).ceil().clamp(1.0, 1e6) as usize;
        for i in 1..=steps {
            self.pts.push((c + n0.rotate(sweep * i as f64 / steps as f64) * r, false));
        }
    }
}

struct SegGeom {
    np: V2,
    nm: V2,
    psi: f64,
    degen: bool,
}

/// Intersection of segments `p + t(q - p)` and `p2 + s(q2 - p2)` with `t, s ∈ [0, 1]`.
fn seg_intersect(p: V2, q: V2, p2: V2, q2: V2) -> Option<(V2, f64, f64)> {
    let d1 = q - p;
    let d2 = q2 - p2;
    let den = d1.cross(d2);
    if den.is_nan() || den.abs() <= 1e-12 * d1.len() * d2.len() {
        return None;
    }
    let w = p2 - p;
    let t = w.cross(d2) / den;
    let s = w.cross(d1) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&s)).then(|| (p + d1 * t, t, s))
}

fn arcs_overlap(s1: V2, sw1: f64, s2: V2, sw2: f64) -> bool {
    let iv = |s: V2, sw: f64| (s.y.atan2(s.x) + sw.min(0.0), sw.abs());
    let inside = |x: f64, (lo, len): (f64, f64)| (x - lo).rem_euclid(TAU) < len;
    let (i1, i2) = (iv(s1, sw1), iv(s2, sw2));
    inside(i1.0, i2) || inside(i2.0, i1)
}

fn contour(nodes: &[Node], cap: Cap, tol: f64) -> (Vec<(V2, bool)>, usize) {
    let n = nodes.len();
    let m = n - 1;
    let c = |i: usize| nodes[i].c;
    let r = |i: usize| nodes[i].r;
    let geo: Vec<SegGeom> = (0..m)
        .map(|i| {
            let d = c(i + 1) - c(i);
            let dl = d.len();
            let u = d.normalize().unwrap_or(V2::new(1.0, 0.0));
            let v = u.perp();
            let cp = ((r(i) - r(i + 1)) / dl).clamp(-1.0, 1.0);
            let cp = if cp.is_finite() { cp } else { 0.0 };
            let sp = (1.0 - cp * cp).max(0.0).sqrt();
            SegGeom { np: u * cp + v * sp, nm: u * cp - v * sp, psi: sp.atan2(cp), degen: cp.abs() >= 1.0 - 1e-12 }
        })
        .collect();
    // Tangent segment endpoints.
    let fm = |i: usize| (c(i) + geo[i].nm * r(i), c(i + 1) + geo[i].nm * r(i + 1));
    let fp = |i: usize| (c(i) + geo[i].np * r(i), c(i + 1) + geo[i].np * r(i + 1));

    // Joint sweeps.
    let mut a = vec![0.0; n];
    let mut b = vec![0.0; n];
    for j in 1..m {
        let mut aj = signed_angle(geo[j - 1].nm, geo[j].nm);
        let mut bj = signed_angle(geo[j].np, geo[j - 1].np);
        let s1 = 2.0 * geo[j - 1].psi;
        let s2 = TAU - 2.0 * geo[j].psi;
        let mut k = ((aj + bj - s1 - s2) / TAU).round();
        while k < -1.0 {
            if aj <= bj {
                aj += TAU;
            } else {
                bj += TAU;
            }
            k += 1.0;
        }
        a[j] = aj;
        b[j] = bj;
    }

    // Inner-join shortcuts.
    let mut sm: Vec<Option<V2>> = vec![None; n];
    let mut spl: Vec<Option<V2>> = vec![None; n];
    let mut last = 0.0;
    for j in 1..m {
        let ok_geo = !geo[j - 1].degen && !geo[j].degen;
        let cand = if ok_geo && a[j] < -1e-9 && a[j] > -PI {
            let (p1, q1) = fm(j - 1);
            let (p2, q2) = fm(j);
            seg_intersect(p1, q1, p2, q2)
        } else {
            None
        };
        match cand {
            Some((x, t, s)) if t >= last => {
                sm[j] = Some(x);
                last = s;
            }
            _ => last = 0.0,
        }
    }
    let mut last = 1.0;
    for j in (1..m).rev() {
        let ok_geo = !geo[j - 1].degen && !geo[j].degen;
        let cand = if ok_geo && b[j] < -1e-9 && b[j] > -PI {
            let (p1, q1) = fp(j);
            let (p2, q2) = fp(j - 1);
            seg_intersect(p1, q1, p2, q2)
        } else {
            None
        };
        let clash = sm[j].is_some() && arcs_overlap(geo[j - 1].nm, a[j], geo[j].np, b[j]);
        match cand {
            Some((x, t_on_j, t_on_prev)) if t_on_j <= last && !clash => {
                spl[j] = Some(x);
                last = t_on_prev;
            }
            _ => last = 1.0,
        }
    }
    let shortcuts = sm.iter().chain(&spl).filter(|x| x.is_some()).count();

    let butt = cap == Cap::Butt;
    let mut e = Emitter::new(tol);
    let s0 = c(0) + geo[0].nm * r(0);
    e.start(s0, butt);
    for j in 1..m {
        if let Some(x) = sm[j] {
            e.line_to(x, false);
        } else {
            e.line_to(c(j) + geo[j - 1].nm * r(j), false);
            e.arc(c(j), r(j), geo[j - 1].nm, a[j]);
        }
    }
    let ls = m - 1;
    e.line_to(c(m) + geo[ls].nm * r(m), butt);
    if butt {
        e.line_to(c(m) + geo[ls].np * r(m), true);
    } else {
        e.arc(c(m), r(m), geo[ls].nm, 2.0 * geo[ls].psi);
    }
    for j in (1..m).rev() {
        if let Some(x) = spl[j] {
            e.line_to(x, false);
        } else {
            e.line_to(c(j) + geo[j].np * r(j), false);
            e.arc(c(j), r(j), geo[j].np, b[j]);
        }
    }
    e.line_to(c(0) + geo[0].np * r(0), butt);
    if butt {
        e.line_to(s0, true);
    } else {
        e.arc(c(0), r(0), geo[0].np, TAU - 2.0 * geo[0].psi);
    }
    // The loop ends on (numerically near) its start; drop the duplicate.
    if e.pts.len() > 1
        && let (Some(&(lp, lc)), Some(&(fp0, _))) = (e.pts.last(), e.pts.first())
        && lp.dist(fp0) <= tol * 1e-6
    {
        e.pts.pop();
        e.pts[0].1 |= lc;
    }
    (e.pts, shortcuts)
}

/// Circle as cubic arcs; enough segments that the cubic approximation error
/// (≈ 2.7e-4·r for a quarter circle, scaling with the 6th power of the sweep) is ≤ τ/2.
fn circle_path(c: V2, r: f64, tol: f64) -> BezPath {
    let mut segs = 4usize;
    let mut err = 2.7e-4 * r;
    while err > 0.5 * tol && segs < 256 {
        segs *= 2;
        err /= 64.0;
    }
    let sweep = TAU / segs as f64;
    let k = 4.0 / 3.0 * (sweep / 4.0).tan() * r;
    let mut p = BezPath::new();
    let at = |ang: f64| V2::new(ang.cos(), ang.sin());
    p.move_to((c + at(0.0) * r).pt());
    for i in 0..segs {
        let a0 = sweep * i as f64;
        let a1 = a0 + sweep;
        let (d0, d1) = (at(a0), at(a1));
        let p0 = c + d0 * r;
        let p3 = c + d1 * r;
        p.cubic_to((p0 + d0.perp() * k).pt(), (p3 - d1.perp() * k).pt(), p3.pt());
    }
    p.close_path();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degenerate_strokes() {
        let o = OutlineOptions::default();
        assert!(variable_width_outline(&[], &o).is_empty());
        assert!(variable_width_outline(&[WidthPoint::new(1.0, 1.0, 0.0)], &o).is_empty());
        let dot = variable_width_outline(&[WidthPoint::new(10.0, 10.0, 20.0), WidthPoint::new(10.0, 10.0, 8.0)], &o);
        let b = dot.bounds();
        assert!((b.x0 - 0.0).abs() < 1e-9 && (b.x1 - 20.0).abs() < 1e-9 && (b.y1 - 20.0).abs() < 1e-9, "{b:?}");
        // min_width clamp turns a zero-width stroke into a visible one.
        let mw = OutlineOptions { min_width: 2.0, ..o };
        let line = variable_width_outline(&[WidthPoint::new(0.0, 0.0, 0.0), WidthPoint::new(50.0, 0.0, 0.0)], &mw);
        let lb = line.bounds();
        assert!((lb.y1 - 1.0).abs() < 0.2 && (lb.x0 + 1.0).abs() < 0.2, "{lb:?}");
    }

    #[test]
    fn butt_cap_rectangle() {
        let o = OutlineOptions { cap: Cap::Butt, ..OutlineOptions::default() };
        let path = variable_width_outline(&[WidthPoint::new(0.0, 0.0, 10.0), WidthPoint::new(100.0, 0.0, 10.0)], &o);
        let b = path.bounds();
        assert!((b.x0).abs() < 1e-9 && (b.x1 - 100.0).abs() < 1e-9 && (b.y0 + 5.0).abs() < 1e-9);
        assert_eq!(path.node_count(), 4, "{:?}", path.els);
    }
}
