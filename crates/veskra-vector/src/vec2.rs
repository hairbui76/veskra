//! Minimal 2D vector arithmetic used internally (`PointF` is a foreign type, so operator
//! impls live on this local newtype-like struct).

use std::ops::{Add, Mul, Neg, Sub};

use veskra_core::PointF;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct V2 {
    pub x: f64,
    pub y: f64,
}

impl V2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dot(self, o: V2) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: V2) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn len2(self) -> f64 {
        self.dot(self)
    }
    /// Rotate by +90° (towards increasing angle): `(x, y) -> (-y, x)`.
    pub fn perp(self) -> V2 {
        V2::new(-self.y, self.x)
    }
    /// Unit vector, or `None` for (near-)zero / non-finite input.
    pub fn normalize(self) -> Option<V2> {
        let l = self.len();
        if l > 1e-300 && l.is_finite() { Some(V2::new(self.x / l, self.y / l)) } else { None }
    }
    pub fn rotate(self, angle: f64) -> V2 {
        let (s, c) = angle.sin_cos();
        V2::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    pub fn dist(self, o: V2) -> f64 {
        (self - o).len()
    }
    pub fn lerp(self, o: V2, t: f64) -> V2 {
        self + (o - self) * t
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
    pub fn pt(self) -> PointF {
        PointF::new(self.x, self.y)
    }
}

impl From<PointF> for V2 {
    fn from(p: PointF) -> Self {
        V2::new(p.x, p.y)
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for V2 {
    type Output = V2;
    fn mul(self, s: f64) -> V2 {
        V2::new(self.x * s, self.y * s)
    }
}

impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        V2::new(-self.x, -self.y)
    }
}

/// Signed angle from `a` to `b` in `(-π, π]`.
pub(crate) fn signed_angle(a: V2, b: V2) -> f64 {
    let ang = a.cross(b).atan2(a.dot(b));
    if ang <= -std::f64::consts::PI { ang + 2.0 * std::f64::consts::PI } else { ang }
}

/// Distance from `p` to the closed segment `[a, b]`.
pub(crate) fn dist_to_segment(p: V2, a: V2, b: V2) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    if l2.is_nan() || l2 <= 0.0 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}
