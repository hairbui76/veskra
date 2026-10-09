//! Editable monotone response curves (pressure → size/opacity/flow).

use serde::{Deserialize, Serialize};

use crate::BrushError;

/// Maximum control points in one curve (bounded input).
pub const MAX_CURVE_POINTS: usize = 32;

/// Monotone non-decreasing curve `[0,1] → [0,1]` through control points `[x, y]`.
///
/// Rules (checked by [`ResponseCurve::validate`]): 2..=[`MAX_CURVE_POINTS`] finite points,
/// first `x = 0`, last `x = 1`, `x` strictly increasing, `y` in `[0,1]` and non-decreasing.
/// Evaluation is a monotone cubic Hermite interpolant (Fritsch–Butland tangents), so the
/// curve never overshoots its control points. Serialized as `[[x, y], ...]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResponseCurve {
    points: Vec<[f32; 2]>,
}

impl Default for ResponseCurve {
    fn default() -> Self {
        Self::identity()
    }
}

impl ResponseCurve {
    /// `y = x`.
    pub fn identity() -> Self {
        Self { points: vec![[0.0, 0.0], [1.0, 1.0]] }
    }

    /// `y = v` for every input (disables the pressure dependency).
    pub fn constant(v: f32) -> Self {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        Self { points: vec![[0.0, v], [1.0, v]] }
    }

    /// Validated curve from control points.
    pub fn new(points: Vec<[f32; 2]>) -> Result<Self, BrushError> {
        let c = Self { points };
        c.validate()?;
        Ok(c)
    }

    /// `y = x^gamma` sampled at `n` (≥ 2) evenly spaced points (used by built-in presets).
    pub fn power(gamma: f32, n: usize) -> Self {
        let n = n.clamp(2, MAX_CURVE_POINTS);
        let points = (0..n)
            .map(|i| {
                let x = i as f32 / (n - 1) as f32;
                [x, x.powf(gamma).clamp(0.0, 1.0)]
            })
            .collect();
        Self { points }
    }

    pub fn points(&self) -> &[[f32; 2]] {
        &self.points
    }

    pub fn validate(&self) -> Result<(), BrushError> {
        let bad = |m: &str| Err(BrushError::InvalidPreset(format!("curve: {m}")));
        let p = &self.points;
        if p.len() < 2 || p.len() > MAX_CURVE_POINTS {
            return bad("needs 2..=32 control points");
        }
        if p.iter().any(|[x, y]| !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(y)) {
            return bad("points must be finite with y in [0,1]");
        }
        if p[0][0] != 0.0 || p[p.len() - 1][0] != 1.0 {
            return bad("first x must be 0 and last x must be 1");
        }
        if p.windows(2).any(|w| w[1][0] <= w[0][0]) {
            return bad("x must be strictly increasing");
        }
        if p.windows(2).any(|w| w[1][1] < w[0][1]) {
            return bad("y must be non-decreasing (monotone)");
        }
        Ok(())
    }

    /// True if every control point has the same `y` (output independent of input).
    pub fn is_constant(&self) -> bool {
        self.points.windows(2).all(|w| w[0][1] == w[1][1])
    }

    /// Constant curve with value `v`.
    pub fn is_constant_value(&self, v: f32) -> bool {
        self.is_constant() && self.points.first().is_some_and(|p| p[1] == v)
    }

    /// Evaluate at `x` (clamped to `[0,1]`; NaN → 0). An invalid (unvalidated) curve falls
    /// back to identity rather than panicking.
    pub fn eval(&self, x: f32) -> f32 {
        let x = if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 } as f64;
        let p = &self.points;
        if p.len() < 2 {
            return x as f32;
        }
        // Segment i with p[i].x <= x <= p[i+1].x.
        let i = match p.iter().rposition(|q| (q[0] as f64) <= x) {
            Some(i) => i.min(p.len() - 2),
            None => 0,
        };
        let (x0, y0) = (p[i][0] as f64, p[i][1] as f64);
        let (x1, y1) = (p[i + 1][0] as f64, p[i + 1][1] as f64);
        let h = x1 - x0;
        if h <= 0.0 {
            return y0.clamp(0.0, 1.0) as f32;
        }
        let t = ((x - x0) / h).clamp(0.0, 1.0);
        let (m0, m1) = (self.tangent(i) * h, self.tangent(i + 1) * h);
        let t2 = t * t;
        let t3 = t2 * t;
        let y =
            (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * m1;
        let (lo, hi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        y.clamp(lo, hi).clamp(0.0, 1.0) as f32
    }

    fn secant(&self, k: usize) -> (f64, f64) {
        let p = &self.points;
        let h = p[k + 1][0] as f64 - p[k][0] as f64;
        let d = if h > 0.0 { (p[k + 1][1] as f64 - p[k][1] as f64) / h } else { 0.0 };
        (h, d)
    }

    /// Fritsch–Butland tangent: interior tangents are a weighted harmonic mean of the adjacent
    /// secants (0 when either is 0), which keeps `m ≤ 3·min(d0, d1)` and therefore monotone.
    fn tangent(&self, i: usize) -> f64 {
        let n = self.points.len();
        if i == 0 {
            return self.secant(0).1;
        }
        if i >= n - 1 {
            return self.secant(n - 2).1;
        }
        let (h0, d0) = self.secant(i - 1);
        let (h1, d1) = self.secant(i);
        if d0 * d1 <= 0.0 {
            return 0.0;
        }
        3.0 * (h0 + h1) / ((2.0 * h1 + h0) / d0 + (h1 + 2.0 * h0) / d1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_constant() {
        let c = ResponseCurve::identity();
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            assert!((c.eval(x) - x).abs() < 1e-6);
        }
        assert_eq!(ResponseCurve::constant(0.4).eval(0.9), 0.4);
        assert!(ResponseCurve::constant(1.0).is_constant_value(1.0));
        assert_eq!(c.eval(f32::NAN), 0.0);
        assert_eq!(c.eval(7.0), 1.0);
    }

    #[test]
    fn monotone_no_overshoot() {
        let c = ResponseCurve::new(vec![[0.0, 0.0], [0.1, 0.6], [0.2, 0.62], [0.7, 0.63], [1.0, 1.0]]).unwrap();
        let mut prev = -1.0f32;
        for i in 0..=1000 {
            let y = c.eval(i as f32 / 1000.0);
            assert!(y >= prev, "not monotone at {i}");
            assert!((0.0..=1.0).contains(&y));
            prev = y;
        }
        // Passes through control points.
        for p in c.points() {
            assert!((c.eval(p[0]) - p[1]).abs() < 1e-6);
        }
        let g = ResponseCurve::power(2.0, 9);
        g.validate().unwrap();
        assert!((g.eval(0.5) - 0.25).abs() < 0.01);
    }

    #[test]
    fn rejects_invalid() {
        assert!(ResponseCurve::new(vec![[0.0, 0.0]]).is_err());
        assert!(ResponseCurve::new(vec![[0.0, 0.5], [1.0, 0.2]]).is_err());
        assert!(ResponseCurve::new(vec![[0.0, 0.0], [0.5, 0.5], [0.5, 0.6], [1.0, 1.0]]).is_err());
        assert!(ResponseCurve::new(vec![[0.1, 0.0], [1.0, 1.0]]).is_err());
        assert!(ResponseCurve::new(vec![[0.0, 0.0], [1.0, f32::NAN]]).is_err());
        assert!(ResponseCurve::new(vec![[0.0, 0.0]; 40]).is_err());
    }
}
