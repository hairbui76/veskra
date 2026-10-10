//! Symmetry (mirror / radial) for stroke input.
//!
//! Symmetry is **session (UI) state**: it is not stored in the document or the `.veskra` file.
//! What *is* stored are its results: every replica of a gesture is committed as an ordinary
//! [`veskra_doc::StrokeRecord`] with its own (transformed) confirmed samples, so replicas stay
//! vector-eligible, export as editable paths and round-trip through save/open like any stroke.
//!
//! A replica is an isometry of the document plane about the symmetry center: an optional mirror
//! across the vertical axis through the center followed by a rotation. Only the sample position
//! (and the azimuth, which is a direction in the document plane) is transformed; pressure, tilt,
//! timestamps and flags pass through unchanged — nothing is invented.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use veskra_core::{PointF, StrokeSample, VeskraError};

/// Smallest/largest `count` accepted for radial symmetry.
pub const MIN_RADIAL_COUNT: u32 = 2;
pub const MAX_RADIAL_COUNT: u32 = 16;
/// `count` used when `symmetry.set` selects `radial` without one.
pub const DEFAULT_RADIAL_COUNT: u32 = 6;
/// Largest `|coordinate|` (document px) accepted for symmetry centers, guides and shapes.
pub const MAX_TOOL_COORD: f64 = 100_000.0;

/// `symmetry.set` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SymmetryMode {
    #[default]
    Off,
    /// Mirror across the **vertical axis** through the center (left ↔ right): 2 strokes.
    Vertical,
    /// Mirror across the **horizontal axis** through the center (top ↔ bottom): 2 strokes.
    Horizontal,
    /// Both axes: 4 strokes.
    Quad,
    /// `count` rotations about the center (`2·count` strokes with `mirror`).
    Radial,
}

impl SymmetryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SymmetryMode::Off => "off",
            SymmetryMode::Vertical => "vertical",
            SymmetryMode::Horizontal => "horizontal",
            SymmetryMode::Quad => "quad",
            SymmetryMode::Radial => "radial",
        }
    }
}

/// Validated symmetry settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Symmetry {
    pub mode: SymmetryMode,
    /// Radial sector count (`MIN_RADIAL_COUNT..=MAX_RADIAL_COUNT`); kept but unused by other modes.
    pub count: u32,
    /// Radial only: also mirror every sector (kaleidoscope).
    pub mirror: bool,
    /// Explicit center; `None` = the canvas center of whatever document a stroke begins on.
    pub center: Option<PointF>,
}

impl Default for Symmetry {
    fn default() -> Self {
        Self { mode: SymmetryMode::Off, count: DEFAULT_RADIAL_COUNT, mirror: false, center: None }
    }
}

/// One replica transform: `p' = c + R(θ)·F(p − c)` where `F` flips x when `flip` is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Replica {
    center: PointF,
    cos: f64,
    sin: f64,
    angle: f64,
    flip: bool,
}

/// `sin`/`cos` with values within 1e-12 of 0 or ±1 snapped, so quarter turns are exact.
fn snapped_sin_cos(angle: f64) -> (f64, f64) {
    let snap = |v: f64| {
        if v.abs() < 1e-12 {
            0.0
        } else if (v.abs() - 1.0).abs() < 1e-12 {
            v.signum()
        } else {
            v
        }
    };
    let (s, c) = angle.sin_cos();
    (snap(s), snap(c))
}

impl Replica {
    fn new(center: PointF, angle: f64, flip: bool) -> Self {
        let (sin, cos) = snapped_sin_cos(angle);
        Self { center, cos, sin, angle, flip }
    }

    /// Transform a document point.
    pub fn apply_point(&self, p: PointF) -> PointF {
        let mut dx = p.x - self.center.x;
        let dy = p.y - self.center.y;
        if self.flip {
            dx = -dx;
        }
        PointF::new(self.center.x + self.cos * dx - self.sin * dy, self.center.y + self.sin * dx + self.cos * dy)
    }

    /// Transform a sample: position and azimuth move with the isometry; everything else
    /// (pressure, tilt, time, flags) is copied.
    pub fn apply(&self, s: &StrokeSample) -> StrokeSample {
        let p = self.apply_point(PointF::new(f64::from(s.x), f64::from(s.y)));
        // Azimuth convention (veskra-input): direction `(sin a, −cos a)`, so a flip of x negates
        // it and a rotation adds its angle.
        let azimuth = s.azimuth.map(|a| {
            let a = f64::from(a);
            wrap_pi(self.angle + if self.flip { -a } else { a }) as f32
        });
        StrokeSample { x: p.x as f32, y: p.y as f32, azimuth, ..*s }
    }
}

/// Wrap an angle into `[-π, π]`.
fn wrap_pi(a: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    if !a.is_finite() {
        return 0.0;
    }
    let mut r = a % TAU;
    if r > PI {
        r -= TAU;
    } else if r < -PI {
        r += TAU;
    }
    r
}

fn invalid(m: impl Into<String>) -> VeskraError {
    VeskraError::invalid(m)
}

impl Symmetry {
    /// Validate `symmetry.set` fields. `count` is checked for every mode (a bad value is never
    /// silently stored) but only used by `radial`; `cx`/`cy` must be given together.
    pub fn new(
        mode: SymmetryMode,
        count: Option<u32>,
        mirror: Option<bool>,
        cx: Option<f64>,
        cy: Option<f64>,
    ) -> Result<Self, VeskraError> {
        let count = count.unwrap_or(DEFAULT_RADIAL_COUNT);
        if !(MIN_RADIAL_COUNT..=MAX_RADIAL_COUNT).contains(&count) {
            return Err(invalid(format!("symmetry count must be in [{MIN_RADIAL_COUNT}, {MAX_RADIAL_COUNT}]")));
        }
        let center = match (cx, cy) {
            (None, None) => None,
            (Some(x), Some(y)) => {
                if !(x.is_finite() && y.is_finite() && x.abs() <= MAX_TOOL_COORD && y.abs() <= MAX_TOOL_COORD) {
                    return Err(invalid(format!("symmetry center must be finite and within ±{MAX_TOOL_COORD}")));
                }
                Some(PointF::new(x, y))
            }
            _ => return Err(invalid("symmetry center needs both cx and cy")),
        };
        Ok(Self { mode, count, mirror: mirror.unwrap_or(false), center })
    }

    /// Strokes one gesture produces (1 when off).
    pub fn stroke_count(&self) -> usize {
        match self.mode {
            SymmetryMode::Off => 1,
            SymmetryMode::Vertical | SymmetryMode::Horizontal => 2,
            SymmetryMode::Quad => 4,
            SymmetryMode::Radial => self.count as usize * if self.mirror { 2 } else { 1 },
        }
    }

    /// The replica transforms **excluding** the identity (the user's own stroke), in commit order.
    pub fn replicas(&self, canvas_center: PointF) -> Vec<Replica> {
        use std::f64::consts::{PI, TAU};
        let c = self.center.unwrap_or(canvas_center);
        match self.mode {
            SymmetryMode::Off => Vec::new(),
            SymmetryMode::Vertical => vec![Replica::new(c, 0.0, true)],
            // Mirror across the horizontal axis = flip x, then rotate by π.
            SymmetryMode::Horizontal => vec![Replica::new(c, PI, true)],
            SymmetryMode::Quad => {
                vec![Replica::new(c, 0.0, true), Replica::new(c, PI, true), Replica::new(c, PI, false)]
            }
            SymmetryMode::Radial => {
                let n = self.count.clamp(MIN_RADIAL_COUNT, MAX_RADIAL_COUNT);
                let step = TAU / f64::from(n);
                let mut out = Vec::with_capacity(self.stroke_count().saturating_sub(1));
                for k in 1..n {
                    out.push(Replica::new(c, step * f64::from(k), false));
                }
                if self.mirror {
                    for k in 0..n {
                        out.push(Replica::new(c, step * f64::from(k), true));
                    }
                }
                out
            }
        }
    }

    /// State as reported by `symmetry.get` / `document.info`. `canvas_center` is the open
    /// document's center, if any (the center actually used when none was set explicitly).
    pub fn to_json(&self, canvas_center: Option<PointF>) -> Value {
        let c = self.center.or(canvas_center);
        json!({
            "mode": self.mode.as_str(),
            "count": self.count,
            "mirror": self.mirror,
            "cx": c.map(|p| p.x),
            "cy": c.map(|p| p.y),
            "autoCenter": self.center.is_none(),
            "strokes": self.stroke_count(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(mode: SymmetryMode, count: u32, mirror: bool) -> Symmetry {
        Symmetry { mode, count, mirror, center: None }
    }

    #[test]
    fn replica_counts_match_stroke_count() {
        let c = PointF::new(50.0, 40.0);
        for (s, n) in [
            (sym(SymmetryMode::Off, 6, false), 1),
            (sym(SymmetryMode::Vertical, 6, false), 2),
            (sym(SymmetryMode::Horizontal, 6, true), 2),
            (sym(SymmetryMode::Quad, 6, false), 4),
            (sym(SymmetryMode::Radial, 2, false), 2),
            (sym(SymmetryMode::Radial, 16, false), 16),
            (sym(SymmetryMode::Radial, 16, true), 32),
        ] {
            assert_eq!(s.stroke_count(), n);
            assert_eq!(s.replicas(c).len(), n - 1, "{s:?}");
        }
    }

    #[test]
    fn mirrors_and_quarter_turns_are_exact() {
        let c = PointF::new(100.0, 60.0);
        let p = PointF::new(30.25, 10.5);
        let v = sym(SymmetryMode::Vertical, 6, false).replicas(c);
        assert_eq!(v[0].apply_point(p), PointF::new(169.75, 10.5));
        let h = sym(SymmetryMode::Horizontal, 6, false).replicas(c);
        assert_eq!(h[0].apply_point(p), PointF::new(30.25, 109.5));
        let q = sym(SymmetryMode::Quad, 6, false).replicas(c);
        assert_eq!(q[2].apply_point(p), PointF::new(169.75, 109.5));
        let r = sym(SymmetryMode::Radial, 4, false).replicas(c);
        // Quarter turn (clockwise on screen, y down): (dx, dy) -> (-dy, dx).
        assert_eq!(r[0].apply_point(p), PointF::new(100.0 + 49.5, 60.0 - 69.75));
    }

    #[test]
    fn samples_keep_pressure_tilt_and_flags() {
        let c = PointF::new(0.0, 0.0);
        let rep = sym(SymmetryMode::Vertical, 6, false).replicas(c)[0];
        let mut s = StrokeSample::confirmed(3.0, 4.0, 77, 0.3);
        s.tilt = Some(0.2);
        s.azimuth = Some(0.5);
        let m = rep.apply(&s);
        assert_eq!((m.x, m.y), (-3.0, 4.0));
        assert_eq!((m.pressure, m.tilt, m.t_ns, m.flags), (s.pressure, s.tilt, s.t_ns, s.flags));
        assert_eq!(m.azimuth, Some(-0.5));
        m.validate().unwrap();
    }

    #[test]
    fn hostile_values_are_typed_errors() {
        assert!(Symmetry::new(SymmetryMode::Radial, Some(0), None, None, None).is_err());
        assert!(Symmetry::new(SymmetryMode::Radial, Some(u32::MAX), None, None, None).is_err());
        assert!(Symmetry::new(SymmetryMode::Vertical, None, None, Some(f64::NAN), Some(1.0)).is_err());
        assert!(Symmetry::new(SymmetryMode::Vertical, None, None, Some(1e300), Some(1.0)).is_err());
        assert!(Symmetry::new(SymmetryMode::Vertical, None, None, Some(1.0), None).is_err());
        assert!(Symmetry::new(SymmetryMode::Off, None, None, None, None).is_ok());
    }
}
