//! `BezPath`: the vector path type shared with the document model and exporters.
//!
//! ## Path semantics
//! * A subpath starts with `MoveTo`. A `LineTo`/`CubicTo` with no current point (at the
//!   very start of the path) is treated as a `MoveTo` to its end point.
//! * `Close` ends the subpath; the implicit closing line from the current point back to the
//!   subpath start is part of the geometry (bounds, flattening, length, distance).
//! * A segment directly after `Close` starts a new subpath at the closed subpath's start
//!   point (SVG semantics). Writers ([`BezPath::to_svg_d`], [`BezPath::to_pdf_ops`]) make
//!   that explicit by emitting a move-to.

use serde::{Deserialize, Serialize};
use veskra_core::{Affine, PointF, RectF};

use crate::cubic::CubicBez;
use crate::num::push_num;

/// One path element (absolute coordinates, document pixels).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PathEl {
    MoveTo(PointF),
    LineTo(PointF),
    /// Control point 1, control point 2, end point.
    CubicTo(PointF, PointF, PointF),
    Close,
}

/// A sequence of path elements (see module docs for semantics).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BezPath {
    pub els: Vec<PathEl>,
}

/// A drawable segment with explicit start point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSeg {
    Line(PointF, PointF),
    Cubic(CubicBez),
}

impl PathSeg {
    pub fn start(&self) -> PointF {
        match self {
            PathSeg::Line(a, _) => *a,
            PathSeg::Cubic(c) => c.p0,
        }
    }
    pub fn end(&self) -> PointF {
        match self {
            PathSeg::Line(_, b) => *b,
            PathSeg::Cubic(c) => c.p3,
        }
    }
    pub fn reverse(&self) -> PathSeg {
        match self {
            PathSeg::Line(a, b) => PathSeg::Line(*b, *a),
            PathSeg::Cubic(c) => PathSeg::Cubic(c.reverse()),
        }
    }
}

/// A normalized subpath: explicit start, segments, closed flag. For closed subpaths
/// `segs` does **not** include the implicit closing line; see [`Subpath::closing_seg`].
#[derive(Debug, Clone, PartialEq)]
pub struct Subpath {
    pub start: PointF,
    pub segs: Vec<PathSeg>,
    pub closed: bool,
}

impl Subpath {
    pub fn end(&self) -> PointF {
        self.segs.last().map_or(self.start, PathSeg::end)
    }
    /// The implicit closing line, if the subpath is closed and does not already end at start.
    pub fn closing_seg(&self) -> Option<PathSeg> {
        let e = self.end();
        (self.closed && e != self.start).then_some(PathSeg::Line(e, self.start))
    }
    /// All geometric segments including the implicit closing line.
    pub fn geom_segs(&self) -> impl Iterator<Item = PathSeg> + '_ {
        self.segs.iter().copied().chain(self.closing_seg())
    }
}

impl BezPath {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_els(els: Vec<PathEl>) -> Self {
        Self { els }
    }

    pub fn is_empty(&self) -> bool {
        self.els.is_empty()
    }

    pub fn move_to(&mut self, p: PointF) {
        self.els.push(PathEl::MoveTo(p));
    }
    pub fn line_to(&mut self, p: PointF) {
        self.els.push(PathEl::LineTo(p));
    }
    pub fn cubic_to(&mut self, c1: PointF, c2: PointF, p: PointF) {
        self.els.push(PathEl::CubicTo(c1, c2, p));
    }
    pub fn close_path(&mut self) {
        self.els.push(PathEl::Close);
    }

    /// Normalized subpaths (see module docs).
    pub fn subpaths(&self) -> Vec<Subpath> {
        let mut out = Vec::new();
        let mut cur: Option<Subpath> = None;
        // Start point of the most recently closed subpath (current point after `Close`).
        let mut after_close: Option<PointF> = None;
        for el in &self.els {
            match *el {
                PathEl::MoveTo(p) => {
                    out.extend(cur.take());
                    after_close = None;
                    cur = Some(Subpath { start: p, segs: Vec::new(), closed: false });
                }
                PathEl::LineTo(_) | PathEl::CubicTo(..) => {
                    let end = match *el {
                        PathEl::LineTo(p) | PathEl::CubicTo(_, _, p) => p,
                        _ => continue,
                    };
                    if cur.is_none() {
                        match after_close.take() {
                            Some(s) => cur = Some(Subpath { start: s, segs: Vec::new(), closed: false }),
                            None => {
                                cur = Some(Subpath { start: end, segs: Vec::new(), closed: false });
                                continue;
                            }
                        }
                    }
                    if let Some(sp) = cur.as_mut() {
                        let from = sp.end();
                        sp.segs.push(match *el {
                            PathEl::CubicTo(c1, c2, p) => PathSeg::Cubic(CubicBez::new(from, c1, c2, p)),
                            _ => PathSeg::Line(from, end),
                        });
                    }
                }
                PathEl::Close => {
                    if let Some(mut sp) = cur.take() {
                        sp.closed = true;
                        after_close = Some(sp.start);
                        out.push(sp);
                    }
                }
            }
        }
        out.extend(cur);
        out
    }

    /// Rebuild a path from normalized subpaths.
    pub fn from_subpaths(subs: &[Subpath]) -> BezPath {
        let mut p = BezPath::new();
        for s in subs {
            p.move_to(s.start);
            for seg in &s.segs {
                match seg {
                    PathSeg::Line(_, b) => p.line_to(*b),
                    PathSeg::Cubic(c) => p.cubic_to(c.p1, c.p2, c.p3),
                }
            }
            if s.closed {
                p.close_path();
            }
        }
        p
    }

    /// Iterate over all geometric segments (including implicit closing lines).
    pub fn segments(&self) -> Vec<PathSeg> {
        self.subpaths().iter().flat_map(|s| s.geom_segs().collect::<Vec<_>>()).collect()
    }

    /// Exact bounds: subpath start points, segment endpoints and true cubic extrema.
    /// An empty path returns `RectF::default()` (all zeros).
    pub fn bounds(&self) -> RectF {
        let mut r: Option<RectF> = None;
        let mut add = |b: RectF| {
            r = Some(match r {
                None => b,
                Some(a) => RectF::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)),
            });
        };
        for s in self.subpaths() {
            add(RectF::from_points([s.start]));
            for seg in &s.segs {
                match seg {
                    PathSeg::Line(_, b) => add(RectF::from_points([*b])),
                    PathSeg::Cubic(c) => add(c.bounds()),
                }
            }
        }
        r.unwrap_or_default()
    }

    /// Apply an affine transform (exact for Bézier geometry).
    pub fn transform(&self, m: &Affine) -> BezPath {
        let t = |p: PointF| m.apply(p);
        BezPath {
            els: self
                .els
                .iter()
                .map(|el| match *el {
                    PathEl::MoveTo(p) => PathEl::MoveTo(t(p)),
                    PathEl::LineTo(p) => PathEl::LineTo(t(p)),
                    PathEl::CubicTo(a, b, p) => PathEl::CubicTo(t(a), t(b), t(p)),
                    PathEl::Close => PathEl::Close,
                })
                .collect(),
        }
    }

    /// Flatten to one polyline per subpath. **Guarantee:** every point of each segment is
    /// within `tolerance` of the polyline and every polyline point is within `tolerance` of
    /// the curve (cubics are subdivided until `0.75 * max second difference <= tolerance`,
    /// a rigorous parametric bound). Closed subpaths end with a repeat of their start point.
    /// Non-positive/non-finite tolerances are clamped to `1e-9`.
    pub fn flatten(&self, tolerance: f64) -> Vec<Vec<PointF>> {
        self.subpaths()
            .iter()
            .map(|s| {
                let mut pts = vec![s.start];
                for seg in s.geom_segs() {
                    match seg {
                        PathSeg::Line(_, b) => pts.push(b),
                        PathSeg::Cubic(c) => c.flatten_into(tolerance, &mut pts),
                    }
                }
                pts
            })
            .collect()
    }

    /// Total length estimate (cubics accurate to ~1e-4 px each), including closing lines.
    pub fn length(&self) -> f64 {
        self.segments()
            .iter()
            .map(|s| match s {
                PathSeg::Line(a, b) => a.distance(*b),
                PathSeg::Cubic(c) => c.arc_length(1e-4),
            })
            .sum()
    }

    /// Reverse the direction of every subpath (subpath order is kept). A closed subpath
    /// whose implicit closing line has non-zero length gets it as an explicit `LineTo`.
    pub fn reverse(&self) -> BezPath {
        let subs: Vec<Subpath> = self
            .subpaths()
            .into_iter()
            .map(|s| {
                let mut segs: Vec<PathSeg> = s.segs.clone();
                segs.extend(s.closing_seg());
                let start = segs.last().map_or(s.start, PathSeg::end);
                Subpath { start, segs: segs.iter().rev().map(PathSeg::reverse).collect(), closed: s.closed }
            })
            .collect();
        BezPath::from_subpaths(&subs)
    }

    /// Number of anchor points as an editor (e.g. Illustrator) shows them: one per subpath
    /// start plus one per segment end, not counting a closed subpath's end anchor that
    /// coincides with its start.
    pub fn node_count(&self) -> usize {
        self.subpaths()
            .iter()
            .map(|s| {
                let n = 1 + s.segs.len();
                if s.closed && !s.segs.is_empty() && s.end() == s.start { n - 1 } else { n }
            })
            .sum()
    }

    /// Number of cubic segments.
    pub fn cubic_count(&self) -> usize {
        self.els.iter().filter(|e| matches!(e, PathEl::CubicTo(..))).count()
    }

    /// SVG path data (`d` attribute). Absolute commands `M L C Z`, numbers rounded to
    /// `decimals` places (≤ 12) with deterministic locale-independent formatting
    /// ([`crate::num`]): no exponent notation, no `-0`, trailing zeros stripped. Numbers are
    /// separated by a single space, omitted before a `-` sign. Every subpath starts with an
    /// explicit `M`. Rounding moves points by at most `0.5 * 10^-decimals` per axis.
    pub fn to_svg_d(&self, decimals: u8) -> String {
        let mut out = String::new();
        for s in self.subpaths() {
            push_cmd(&mut out, 'M', &[s.start], decimals);
            for seg in &s.segs {
                match seg {
                    PathSeg::Line(_, b) => push_cmd(&mut out, 'L', &[*b], decimals),
                    PathSeg::Cubic(c) => push_cmd(&mut out, 'C', &[c.p1, c.p2, c.p3], decimals),
                }
            }
            if s.closed {
                out.push('Z');
            }
        }
        out
    }

    /// PDF content-stream path construction operators (`m`, `l`, `c`, `h`), one operator per
    /// line, same number formatting as [`Self::to_svg_d`] (PDF forbids exponent notation).
    /// Coordinates are written as-is: the caller supplies the y-up page transform (e.g. via
    /// [`Self::transform`] or a `cm` operator). Painting operators (`f`, `S`) are not emitted.
    pub fn to_pdf_ops(&self, decimals: u8) -> String {
        let mut out = String::new();
        for s in self.subpaths() {
            push_pdf(&mut out, &[s.start], "m", decimals);
            for seg in &s.segs {
                match seg {
                    PathSeg::Line(_, b) => push_pdf(&mut out, &[*b], "l", decimals),
                    PathSeg::Cubic(c) => push_pdf(&mut out, &[c.p1, c.p2, c.p3], "c", decimals),
                }
            }
            if s.closed {
                out.push_str("h\n");
            }
        }
        out
    }
}

fn push_cmd(out: &mut String, cmd: char, pts: &[PointF], decimals: u8) {
    out.push(cmd);
    let mut first = true;
    for p in pts {
        for v in [p.x, p.y] {
            let mut s = String::new();
            push_num(&mut s, v, decimals);
            if !first && !s.starts_with('-') {
                out.push(' ');
            }
            out.push_str(&s);
            first = false;
        }
    }
}

fn push_pdf(out: &mut String, pts: &[PointF], op: &str, decimals: u8) {
    for p in pts {
        push_num(out, p.x, decimals);
        out.push(' ');
        push_num(out, p.y, decimals);
        out.push(' ');
    }
    out.push_str(op);
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> PointF {
        PointF::new(x, y)
    }

    fn sample() -> BezPath {
        let mut b = BezPath::new();
        b.move_to(p(10.0, -20.5));
        b.line_to(p(30.25, 40.0));
        b.cubic_to(p(-1.0, 2.0), p(3.0, -4.0), p(0.0001, -0.00001));
        b.close_path();
        b
    }

    #[test]
    fn svg_and_pdf_formatting() {
        let b = sample();
        assert_eq!(b.to_svg_d(2), "M10-20.5L30.25 40C-1 2 3-4 0 0Z");
        assert_eq!(b.to_pdf_ops(2), "10 -20.5 m\n30.25 40 l\n-1 2 3 -4 0 0 c\nh\n");
        // Segment after Close starts at the closed subpath's start, made explicit.
        let mut c = b.clone();
        c.line_to(p(5.0, 5.0));
        assert!(c.to_svg_d(1).ends_with("ZM10-20.5L5 5"));
        // Leading segment without MoveTo becomes a move.
        let d = BezPath::from_els(vec![PathEl::LineTo(p(1.0, 1.0)), PathEl::LineTo(p(2.0, 1.0))]);
        assert_eq!(d.to_svg_d(0), "M1 1L2 1");
    }

    #[test]
    fn reverse_roundtrip_and_closing() {
        let b = sample();
        let r = b.reverse();
        let rr = r.reverse();
        // Reversing twice yields the same geometry (closing line now explicit).
        let a: Vec<_> = b.flatten(1e-3);
        let c: Vec<_> = rr.flatten(1e-3);
        assert_eq!(a[0].first(), c[0].first());
        assert!((b.length() - r.length()).abs() < 1e-6);
        assert_eq!(r.subpaths()[0].start, p(10.0, -20.5));
    }

    #[test]
    fn transform_bounds_serde() {
        let b = sample();
        let t = b.transform(&Affine::translate(5.0, 5.0));
        let (b0, b1) = (b.bounds(), t.bounds());
        assert!((b1.x0 - b0.x0 - 5.0).abs() < 1e-9 && (b1.y1 - b0.y1 - 5.0).abs() < 1e-9);
        let json = serde_json::to_string(&b).unwrap();
        let back: BezPath = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
        assert_eq!(BezPath::new().bounds(), RectF::default());
        assert_eq!(b.node_count(), 3);
    }
}
