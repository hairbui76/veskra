//! Pressure response curve: monotone cubic Hermite interpolation (Fritsch–Carlson) through
//! user control points, mapping `[0,1] → [0,1]`. Applied only to hardware-reported pressure.

use crate::InputError;
use serde::{Deserialize, Serialize};

/// Maximum control points accepted (bounded allocation for imported presets).
pub const MAX_CURVE_POINTS: usize = 32;

/// Monotone non-decreasing curve through control points `(x, y)`.
/// Invariants: 2..=32 points, `x[0] == 0`, `x[last] == 1`, `x` strictly increasing,
/// `y` in `[0,1]` and non-decreasing. Default is the identity.
/// Serialized as its control points `[[x, y], ...]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<[f32; 2]>", into = "Vec<[f32; 2]>")]
pub struct PressureCurve {
    points: Vec<[f32; 2]>,
    xs: Vec<f64>,
    ys: Vec<f64>,
    tangents: Vec<f64>,
    identity: bool,
}

impl Default for PressureCurve {
    fn default() -> Self {
        Self::identity()
    }
}

impl PressureCurve {
    pub fn identity() -> Self {
        Self {
            points: vec![[0.0, 0.0], [1.0, 1.0]],
            xs: vec![0.0, 1.0],
            ys: vec![0.0, 1.0],
            tangents: vec![1.0, 1.0],
            identity: true,
        }
    }

    pub fn new(points: &[[f32; 2]]) -> Result<Self, InputError> {
        let bad = |m: &'static str| Err(InputError::InvalidCurve(m));
        if points.len() < 2 || points.len() > MAX_CURVE_POINTS {
            return bad("needs 2..=32 control points");
        }
        if points.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return bad("non-finite control point");
        }
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return bad("empty");
        };
        if first[0] != 0.0 || last[0] != 1.0 {
            return bad("x must start at 0 and end at 1");
        }
        if points.iter().any(|p| !(0.0..=1.0).contains(&p[1])) {
            return bad("y outside [0,1]");
        }
        if points.windows(2).any(|w| w[1][0] <= w[0][0]) {
            return bad("x must be strictly increasing");
        }
        if points.windows(2).any(|w| w[1][1] < w[0][1]) {
            return bad("y must be non-decreasing");
        }
        let xs: Vec<f64> = points.iter().map(|p| f64::from(p[0])).collect();
        let ys: Vec<f64> = points.iter().map(|p| f64::from(p[1])).collect();
        let tangents = fritsch_carlson(&xs, &ys);
        let identity = points.iter().all(|p| p[0] == p[1]);
        Ok(Self { points: points.to_vec(), xs, ys, tangents, identity })
    }

    pub fn points(&self) -> &[[f32; 2]] {
        &self.points
    }

    pub fn is_identity(&self) -> bool {
        self.identity
    }

    /// Evaluate at `p` (clamped to `[0,1]`; NaN → 0). Output is in `[0,1]`.
    pub fn eval(&self, p: f32) -> f32 {
        if p.is_nan() {
            return 0.0;
        }
        let p = p.clamp(0.0, 1.0);
        if self.identity {
            return p;
        }
        let x = f64::from(p);
        // Index k of the interval [x_k, x_{k+1}] containing x.
        let k = self.xs.partition_point(|&xi| xi <= x).saturating_sub(1).min(self.xs.len().saturating_sub(2));
        let (Some(&x0), Some(&x1), Some(&y0), Some(&y1), Some(&m0), Some(&m1)) = (
            self.xs.get(k),
            self.xs.get(k + 1),
            self.ys.get(k),
            self.ys.get(k + 1),
            self.tangents.get(k),
            self.tangents.get(k + 1),
        ) else {
            return p;
        };
        let h = x1 - x0;
        let t = (x - x0) / h;
        let (t2, t3) = (t * t, t * t * t);
        let v = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * h * m0
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * h * m1;
        (v.clamp(0.0, 1.0)) as f32
    }
}

/// Fritsch–Carlson tangents for monotone data (y non-decreasing).
fn fritsch_carlson(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = xs.len();
    let d: Vec<f64> = xs.windows(2).zip(ys.windows(2)).map(|(x, y)| (y[1] - y[0]) / (x[1] - x[0])).collect();
    let mut m: Vec<f64> = (0..n)
        .map(|i| {
            let left = i.checked_sub(1).and_then(|j| d.get(j).copied());
            match (left, d.get(i).copied()) {
                (None, Some(r)) => r,
                (Some(l), None) => l,
                (Some(l), Some(r)) if l * r > 0.0 => (l + r) / 2.0,
                _ => 0.0,
            }
        })
        .collect();
    for (k, &dk) in d.iter().enumerate() {
        let (Some(&mk), Some(&mk1)) = (m.get(k), m.get(k + 1)) else { continue };
        let (nk, nk1) = if dk == 0.0 {
            (0.0, 0.0)
        } else {
            let (a, b) = (mk / dk, mk1 / dk);
            let s = a * a + b * b;
            if s > 9.0 {
                let tau = 3.0 / s.sqrt();
                (tau * a * dk, tau * b * dk)
            } else {
                (mk, mk1)
            }
        };
        if let Some(v) = m.get_mut(k) {
            *v = nk;
        }
        if let Some(v) = m.get_mut(k + 1) {
            *v = nk1;
        }
    }
    m
}

impl TryFrom<Vec<[f32; 2]>> for PressureCurve {
    type Error = InputError;
    fn try_from(v: Vec<[f32; 2]>) -> Result<Self, InputError> {
        PressureCurve::new(&v)
    }
}

impl From<PressureCurve> for Vec<[f32; 2]> {
    fn from(c: PressureCurve) -> Self {
        c.points
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_exact() {
        let c = PressureCurve::default();
        for i in 0..=1000 {
            let p = i as f32 / 1000.0;
            assert_eq!(c.eval(p), p);
        }
        assert_eq!(c.eval(-1.0), 0.0);
        assert_eq!(c.eval(2.0), 1.0);
        assert_eq!(c.eval(f32::NAN), 0.0);
        // explicit identity points are detected
        assert!(PressureCurve::new(&[[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]).unwrap().is_identity());
    }

    #[test]
    fn monotone_through_points() {
        let pts = [[0.0, 0.0], [0.2, 0.05], [0.4, 0.6], [0.5, 0.62], [0.8, 0.62], [1.0, 1.0]];
        let c = PressureCurve::new(&pts).unwrap();
        for p in pts {
            assert!((c.eval(p[0]) - p[1]).abs() < 1e-6, "passes through {p:?}");
        }
        let mut prev = -1.0f32;
        for i in 0..=10_000 {
            let v = c.eval(i as f32 / 10_000.0);
            assert!((0.0..=1.0).contains(&v));
            assert!(v >= prev, "monotone at {i}");
            prev = v;
        }
        // flat segment stays flat (no overshoot)
        for i in 50..=80 {
            assert!((c.eval(i as f32 / 100.0) - 0.62).abs() < 1e-6);
        }
    }

    #[test]
    fn rejects_invalid() {
        assert!(PressureCurve::new(&[[0.0, 0.0]]).is_err());
        assert!(PressureCurve::new(&[[0.1, 0.0], [1.0, 1.0]]).is_err());
        assert!(PressureCurve::new(&[[0.0, 0.0], [0.9, 1.0]]).is_err());
        assert!(PressureCurve::new(&[[0.0, 0.0], [0.5, 0.6], [0.5, 0.7], [1.0, 1.0]]).is_err());
        assert!(PressureCurve::new(&[[0.0, 0.5], [1.0, 0.2]]).is_err());
        assert!(PressureCurve::new(&[[0.0, 0.0], [1.0, 1.5]]).is_err());
        assert!(PressureCurve::new(&[[0.0, f32::NAN], [1.0, 1.0]]).is_err());
        assert!(PressureCurve::new(&vec![[0.0, 0.0]; 40]).is_err());
    }

    #[test]
    fn serde_roundtrip_validates() {
        let c = PressureCurve::new(&[[0.0, 0.1], [0.5, 0.3], [1.0, 0.9]]).unwrap();
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, "[[0.0,0.1],[0.5,0.3],[1.0,0.9]]");
        assert_eq!(serde_json::from_str::<PressureCurve>(&s).unwrap(), c);
        assert!(serde_json::from_str::<PressureCurve>("[[0.0,0.5],[1.0,0.1]]").is_err());
    }
}
