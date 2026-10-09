//! Geometry primitives shared by raster, vector, input and rendering crates.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PointF {
    pub x: f64,
    pub y: f64,
}

impl PointF {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn distance(self, o: PointF) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn lerp(self, o: PointF, t: f64) -> PointF {
        PointF::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct SizeU {
    pub width: u32,
    pub height: u32,
}

impl SizeU {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// Half-open integer pixel rectangle `[x0, x1) x [y0, y1)`. Empty if `x1 <= x0 || y1 <= y0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct RectI {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl RectI {
    pub const EMPTY: RectI = RectI { x0: 0, y0: 0, x1: 0, y1: 0 };

    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }
    pub const fn from_size(w: u32, h: u32) -> Self {
        Self { x0: 0, y0: 0, x1: w as i32, y1: h as i32 }
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn width(&self) -> u32 {
        if self.is_empty() { 0 } else { (self.x1 - self.x0) as u32 }
    }
    pub fn height(&self) -> u32 {
        if self.is_empty() { 0 } else { (self.y1 - self.y0) as u32 }
    }
    pub fn area(&self) -> u64 {
        self.width() as u64 * self.height() as u64
    }
    pub fn intersect(&self, o: &RectI) -> RectI {
        let r = RectI::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1));
        if r.is_empty() { RectI::EMPTY } else { r }
    }
    pub fn union(&self, o: &RectI) -> RectI {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        RectI::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1))
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
    /// Smallest integer rect covering a float rect (clamped to i32 range).
    pub fn covering(r: &RectF) -> RectI {
        if r.is_empty() || !r.x0.is_finite() || !r.y0.is_finite() || !r.x1.is_finite() || !r.y1.is_finite() {
            return RectI::EMPTY;
        }
        let c = |v: f64| v.clamp(i32::MIN as f64 / 2.0, i32::MAX as f64 / 2.0);
        RectI::new(c(r.x0.floor()) as i32, c(r.y0.floor()) as i32, c(r.x1.ceil()) as i32, c(r.y1.ceil()) as i32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RectF {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl RectF {
    pub const fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }
    pub fn is_empty(&self) -> bool {
        !(self.x1 > self.x0 && self.y1 > self.y0)
    }
    pub fn from_points(pts: impl IntoIterator<Item = PointF>) -> RectF {
        let mut r = RectF::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in pts {
            r.x0 = r.x0.min(p.x);
            r.y0 = r.y0.min(p.y);
            r.x1 = r.x1.max(p.x);
            r.y1 = r.y1.max(p.y);
        }
        r
    }
    pub fn inflate(&self, d: f64) -> RectF {
        RectF::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }
    pub fn union(&self, o: &RectF) -> RectF {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        RectF::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1))
    }
}

/// 2D affine transform mapping (x, y) -> (a*x + c*y + e, b*x + d*y + f) (same layout as SVG/PDF `matrix(a b c d e f)`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(tx: f64, ty: f64) -> Self {
        Affine { e: tx, f: ty, ..Self::IDENTITY }
    }
    pub fn scale(sx: f64, sy: f64) -> Self {
        Affine { a: sx, d: sy, ..Self::IDENTITY }
    }
    pub fn rotate(rad: f64) -> Self {
        let (s, c) = rad.sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }
    /// `self.then(o)` applies `self` first and `o` second: `o * self`.
    pub fn then(&self, o: &Affine) -> Affine {
        Affine {
            a: o.a * self.a + o.c * self.b,
            b: o.b * self.a + o.d * self.b,
            c: o.a * self.c + o.c * self.d,
            d: o.b * self.c + o.d * self.d,
            e: o.a * self.e + o.c * self.f + o.e,
            f: o.b * self.e + o.d * self.f + o.f,
        }
    }
    pub fn apply(&self, p: PointF) -> PointF {
        PointF::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }
    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }
    /// Inverse, or `None` if (near-)singular or non-finite.
    pub fn invert(&self) -> Option<Affine> {
        let det = self.determinant();
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        })
    }
    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f].iter().all(|v| v.is_finite())
    }
    /// Uniform scale magnitude (sqrt of |det|).
    pub fn scale_factor(&self) -> f64 {
        self.determinant().abs().sqrt()
    }
    pub fn transform_rect_bounds(&self, r: &RectF) -> RectF {
        RectF::from_points([
            self.apply(PointF::new(r.x0, r.y0)),
            self.apply(PointF::new(r.x1, r.y0)),
            self.apply(PointF::new(r.x0, r.y1)),
            self.apply(PointF::new(r.x1, r.y1)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = RectI::new(0, 0, 10, 10);
        let b = RectI::new(5, 5, 20, 20);
        assert_eq!(a.intersect(&b), RectI::new(5, 5, 10, 10));
        assert_eq!(a.union(&b), RectI::new(0, 0, 20, 20));
        assert!(a.intersect(&RectI::new(10, 0, 20, 10)).is_empty());
        assert_eq!(RectI::EMPTY.union(&a), a);
        assert_eq!(RectI::covering(&RectF::new(0.5, 0.5, 2.1, 3.0)), RectI::new(0, 0, 3, 3));
        assert!(RectI::covering(&RectF::new(f64::NAN, 0.0, 1.0, 1.0)).is_empty());
    }

    #[test]
    fn affine_inverse_roundtrip() {
        let t = Affine::translate(10.0, -3.0).then(&Affine::rotate(0.7)).then(&Affine::scale(2.0, 0.5));
        let inv = t.invert().unwrap();
        let p = PointF::new(3.5, -7.25);
        let q = inv.apply(t.apply(p));
        assert!((p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9);
        assert!(Affine::scale(0.0, 1.0).invert().is_none());
    }

    #[test]
    fn affine_then_order() {
        // translate then scale: (1,0) -> (11,0) -> (22,0)
        let t = Affine::translate(10.0, 0.0).then(&Affine::scale(2.0, 2.0));
        assert_eq!(t.apply(PointF::new(1.0, 0.0)), PointF::new(22.0, 0.0));
    }
}
