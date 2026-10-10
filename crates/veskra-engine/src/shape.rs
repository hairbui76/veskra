//! Shape tools (`shape.line` / `shape.rect` / `shape.ellipse` / `shape.path`): geometry →
//! synthetic stroke samples.
//!
//! A shape is committed as an ordinary brush stroke along a dense polyline, so it uses the
//! current brush, size, opacity and color, honors selection / alpha lock exactly like a drawn
//! stroke, and — with a vector-eligible brush — keeps an editable vector form.
//!
//! The samples are **synthetic**: no hardware produced them, so they carry no pressure
//! (`PRESSURE_REPORTED` is clear; the engine's documented fallback pressure applies, exactly as
//! for a headless `stroke.samples` entry without `p`), no tilt and no azimuth. Timestamps are a
//! nominal 1 ms apart to keep the order explicit.

use crate::symmetry::MAX_TOOL_COORD;
use veskra_core::{PointF, SampleFlags, StrokeSample, VeskraError};

/// Most points accepted by `shape.path`.
pub const MAX_PATH_POINTS: usize = 10_000;
/// Most synthetic samples one shape may expand to.
pub const MAX_SHAPE_SAMPLES: usize = 200_000;
/// Chord length (document px) targeted when a curve is flattened.
const CURVE_CHORD_PX: f64 = 2.0;
/// Subdivision bounds per curve segment.
const MAX_SEGMENT_STEPS: usize = 512;

/// A polyline to stroke. For `closed` paths the last point repeats the first.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapePath {
    pub points: Vec<PointF>,
    pub closed: bool,
}

fn invalid(m: impl Into<String>) -> VeskraError {
    VeskraError::invalid(m)
}

fn check(v: f64) -> Result<f64, VeskraError> {
    if v.is_finite() && v.abs() <= MAX_TOOL_COORD {
        Ok(v)
    } else {
        Err(invalid(format!("shape coordinates must be finite and within ±{MAX_TOOL_COORD}")))
    }
}

fn corners(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<(f64, f64, f64, f64), VeskraError> {
    let (x0, y0, x1, y1) = (check(x0)?, check(y0)?, check(x1)?, check(y1)?);
    Ok((x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)))
}

/// Straight segment from `(x0, y0)` to `(x1, y1)` (a dot when both coincide).
pub fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<ShapePath, VeskraError> {
    let (a, b) = (PointF::new(check(x0)?, check(y0)?), PointF::new(check(x1)?, check(y1)?));
    Ok(ShapePath { points: if a == b { vec![a] } else { vec![a, b] }, closed: false })
}

/// Axis-aligned rectangle outline (corner order: top-left, clockwise on screen).
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<ShapePath, VeskraError> {
    let (l, t, r, b) = corners(x0, y0, x1, y1)?;
    let p = PointF::new;
    Ok(ShapePath { points: vec![p(l, t), p(r, t), p(r, b), p(l, b), p(l, t)], closed: true })
}

/// Axis-aligned ellipse outline inscribed in the box, flattened so that the chord error stays
/// below 0.05 px.
pub fn ellipse(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<ShapePath, VeskraError> {
    let (l, t, r, b) = corners(x0, y0, x1, y1)?;
    let (cx, cy, rx, ry) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
    let rmax = rx.max(ry);
    let n = if rmax <= 0.05 {
        24
    } else {
        let half_angle = (1.0 - 0.05 / rmax).clamp(-1.0, 1.0).acos();
        ((std::f64::consts::PI / half_angle.max(1e-6)).ceil() as usize).clamp(24, 2048)
    };
    let mut points = Vec::with_capacity(n + 1);
    for i in 0..n {
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        points.push(PointF::new(cx + rx * a.cos(), cy + ry * a.sin()));
    }
    points.push(points[0]);
    Ok(ShapePath { points, closed: true })
}

/// Polyline / pen path through `pts`. `closed` joins the last point to the first; `smooth` fits
/// a uniform Catmull-Rom spline through the points (it passes through every point).
pub fn path(pts: &[[f64; 2]], closed: bool, smooth: bool) -> Result<ShapePath, VeskraError> {
    if pts.is_empty() {
        return Err(invalid("shape.path needs at least one point"));
    }
    if pts.len() > MAX_PATH_POINTS {
        return Err(VeskraError::alloc(format!("shape.path accepts at most {MAX_PATH_POINTS} points")));
    }
    let mut v: Vec<PointF> = Vec::with_capacity(pts.len() + 1);
    for p in pts {
        let q = PointF::new(check(p[0])?, check(p[1])?);
        // Consecutive duplicates add nothing and would give the spline zero-length tangents.
        if v.last() != Some(&q) {
            v.push(q);
        }
    }
    if closed && v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    if closed && v.len() < 3 {
        return Err(invalid("a closed path needs at least three distinct points"));
    }
    let points = if smooth && v.len() >= 3 { catmull_rom(&v, closed)? } else { polyline(v, closed) };
    Ok(ShapePath { points, closed })
}

fn polyline(mut v: Vec<PointF>, closed: bool) -> Vec<PointF> {
    if closed {
        v.push(v[0]);
    }
    v
}

fn catmull_rom(v: &[PointF], closed: bool) -> Result<Vec<PointF>, VeskraError> {
    let n = v.len();
    let segs = if closed { n } else { n - 1 };
    // Neighbor lookup: wrap for closed paths, clamp (duplicate the end point) for open ones.
    let at = |i: isize| -> PointF {
        if closed { v[i.rem_euclid(n as isize) as usize] } else { v[i.clamp(0, n as isize - 1) as usize] }
    };
    let mut out = vec![v[0]];
    for s in 0..segs {
        let i = s as isize;
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        // Catmull-Rom → cubic Bezier control points.
        let c1 = PointF::new(p1.x + (p2.x - p0.x) / 6.0, p1.y + (p2.y - p0.y) / 6.0);
        let c2 = PointF::new(p2.x - (p3.x - p1.x) / 6.0, p2.y - (p3.y - p1.y) / 6.0);
        let hull = p1.distance(c1) + c1.distance(c2) + c2.distance(p2);
        let steps = ((hull / CURVE_CHORD_PX).ceil() as usize).clamp(1, MAX_SEGMENT_STEPS);
        if out.len() + steps > MAX_SHAPE_SAMPLES {
            return Err(VeskraError::alloc("shape is too long to smooth; use fewer or closer points"));
        }
        for k in 1..=steps {
            if k == steps {
                out.push(p2); // exact: the curve passes through every input point
                continue;
            }
            let t = k as f64 / steps as f64;
            let u = 1.0 - t;
            let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            out.push(PointF::new(
                b0 * p1.x + b1 * c1.x + b2 * c2.x + b3 * p2.x,
                b0 * p1.y + b1 * c1.y + b2 * c2.y + b3 * p2.y,
            ));
        }
    }
    Ok(out)
}

/// Synthetic confirmed samples along the path (see the module docs for what they carry).
pub fn samples(path: &ShapePath, fallback_pressure: f32) -> Vec<StrokeSample> {
    path.points
        .iter()
        .enumerate()
        .map(|(i, p)| StrokeSample {
            x: p.x as f32,
            y: p.y as f32,
            t_ns: i as u64 * 1_000_000,
            pressure: fallback_pressure,
            tilt: None,
            azimuth: None,
            rotation: None,
            flags: SampleFlags::CONFIRMED,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_and_line_points() {
        let r = rect(30.0, 40.0, 10.0, 20.0).unwrap();
        assert!(r.closed);
        assert_eq!(r.points.len(), 5);
        assert_eq!(r.points[0], PointF::new(10.0, 20.0));
        assert_eq!(r.points[0], r.points[4]);
        assert_eq!(line(1.0, 2.0, 3.0, 4.0).unwrap().points.len(), 2);
        assert_eq!(line(1.0, 2.0, 1.0, 2.0).unwrap().points.len(), 1);
    }

    #[test]
    fn ellipse_points_lie_on_the_outline() {
        let e = ellipse(0.0, 0.0, 200.0, 100.0).unwrap();
        assert!(e.closed && e.points.len() >= 25);
        assert_eq!(e.points.first(), e.points.last());
        for p in &e.points {
            let v = ((p.x - 100.0) / 100.0).powi(2) + ((p.y - 50.0) / 50.0).powi(2);
            assert!((v - 1.0).abs() < 1e-9);
        }
        // Chord sagitta bound.
        let (a, b) = (e.points[0], e.points[1]);
        let mid = PointF::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let v = (((mid.x - 100.0) / 100.0).powi(2) + ((mid.y - 50.0) / 50.0).powi(2)).sqrt();
        assert!((1.0 - v) * 100.0 < 0.06, "{v}");
    }

    #[test]
    fn smooth_path_interpolates_its_points() {
        let pts = [[10.0, 10.0], [60.0, 80.0], [120.0, 20.0], [180.0, 90.0]];
        let p = path(&pts, false, true).unwrap();
        assert!(p.points.len() > pts.len() * 4);
        for q in pts {
            assert!(p.points.contains(&PointF::new(q[0], q[1])), "{q:?} missing");
        }
        let c = path(&pts, true, true).unwrap();
        assert_eq!(c.points.first(), c.points.last());
        // Not smooth: exactly the input (plus the closing point).
        assert_eq!(path(&pts, false, false).unwrap().points.len(), 4);
        assert_eq!(path(&pts, true, false).unwrap().points.len(), 5);
    }

    #[test]
    fn hostile_input_is_a_typed_error() {
        assert!(path(&[], false, false).is_err());
        assert!(path(&[[f64::NAN, 0.0]], false, false).is_err());
        assert!(path(&[[0.0, f64::INFINITY], [1.0, 1.0]], false, true).is_err());
        assert!(path(&[[0.0, 0.0], [1.0, 1.0]], true, false).is_err(), "closed needs 3 points");
        assert!(path(&[[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]], true, true).is_err());
        assert!(path(&vec![[1.0, 1.0]; MAX_PATH_POINTS + 1], false, false).is_err());
        assert!(line(0.0, 0.0, 1e9, 0.0).is_err());
        assert!(rect(f64::NEG_INFINITY, 0.0, 1.0, 1.0).is_err());
        assert!(ellipse(0.0, f64::NAN, 1.0, 1.0).is_err());
        // Far-apart smooth points hit the sample bound instead of allocating without limit.
        let far: Vec<[f64; 2]> = (0..2000).map(|i| [if i % 2 == 0 { -99_000.0 } else { 99_000.0 }, i as f64]).collect();
        assert!(path(&far, false, true).is_err());
    }

    #[test]
    fn samples_carry_no_hardware_data() {
        let s = samples(&rect(0.0, 0.0, 10.0, 10.0).unwrap(), 0.5);
        assert_eq!(s.len(), 5);
        for (i, q) in s.iter().enumerate() {
            assert!(q.is_confirmed());
            assert!(!q.flags.contains(SampleFlags::PRESSURE_REPORTED));
            assert_eq!((q.tilt, q.azimuth, q.rotation), (None, None, None));
            assert_eq!(q.t_ns, i as u64 * 1_000_000);
            q.validate().unwrap();
        }
    }
}
