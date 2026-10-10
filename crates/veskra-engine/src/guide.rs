//! Guides (rulers): constrain stroke samples to a line or an ellipse outline.
//!
//! Guides are **session (UI) state**, like symmetry. They only move sample positions; pressure,
//! tilt, azimuth, timestamps and flags pass through unchanged. A guide is applied to confirmed
//! and predicted samples alike, *before* symmetry, and never to `shape.*` commands.
//!
//! * `line` through two points: every sample is projected orthogonally onto the infinite line.
//! * `line` with an `angle`: the line has that direction and passes through the **first
//!   confirmed sample of each stroke** (a parallel ruler).
//! * `ellipse` (bounding box): every sample is projected onto the outline along the ray from the
//!   ellipse center through the sample (exact nearest point for circles). Central projection is
//!   used instead of the Euclidean nearest point because it is continuous everywhere except at
//!   the center itself, so the stroke never jumps while the pen moves inside the ellipse; a
//!   sample exactly at the center maps to the previous projected point (or the rightmost point).

use crate::symmetry::MAX_TOOL_COORD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use veskra_core::{PointF, StrokeSample, VeskraError};

/// Smallest line length / ellipse radius accepted (document px).
pub const MIN_GUIDE_EXTENT: f64 = 1e-3;

/// `guide.set` kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GuideKind {
    #[default]
    Off,
    Line,
    Ellipse,
}

/// A validated guide.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Guide {
    #[default]
    Off,
    /// Infinite line through `a` and `b`; `(ux, uy)` is its unit direction.
    Line { a: PointF, b: PointF, ux: f64, uy: f64 },
    /// Line with direction `angle` (radians, `(cos, sin)` in document space, y down) through the
    /// stroke's first confirmed sample.
    LineAngle { angle: f64, ux: f64, uy: f64 },
    /// Axis-aligned ellipse outline: center and radii.
    Ellipse { cx: f64, cy: f64, rx: f64, ry: f64 },
}

fn invalid(m: impl Into<String>) -> VeskraError {
    VeskraError::invalid(m)
}

fn coord(v: f64, what: &str) -> Result<f64, VeskraError> {
    if v.is_finite() && v.abs() <= MAX_TOOL_COORD {
        Ok(v)
    } else {
        Err(invalid(format!("guide {what} must be finite and within ±{MAX_TOOL_COORD}")))
    }
}

impl Guide {
    /// Validate `guide.set` fields.
    pub fn new(
        kind: GuideKind,
        x0: Option<f64>,
        y0: Option<f64>,
        x1: Option<f64>,
        y1: Option<f64>,
        angle: Option<f64>,
    ) -> Result<Guide, VeskraError> {
        let points = match (x0, y0, x1, y1) {
            (Some(x0), Some(y0), Some(x1), Some(y1)) => {
                Some((coord(x0, "x0")?, coord(y0, "y0")?, coord(x1, "x1")?, coord(y1, "y1")?))
            }
            (None, None, None, None) => None,
            _ => return Err(invalid("guide needs all of x0, y0, x1, y1")),
        };
        match kind {
            GuideKind::Off => Ok(Guide::Off),
            GuideKind::Line => match (points, angle) {
                (Some(_), Some(_)) => Err(invalid("line guide takes either x0,y0,x1,y1 or angle, not both")),
                (None, None) => Err(invalid("line guide needs x0,y0,x1,y1 or angle")),
                (Some((x0, y0, x1, y1)), None) => {
                    let (dx, dy) = (x1 - x0, y1 - y0);
                    let len = dx.hypot(dy);
                    if !(len.is_finite() && len >= MIN_GUIDE_EXTENT) {
                        return Err(invalid("line guide points must be distinct"));
                    }
                    Ok(Guide::Line { a: PointF::new(x0, y0), b: PointF::new(x1, y1), ux: dx / len, uy: dy / len })
                }
                (None, Some(a)) => {
                    if !(a.is_finite() && a.abs() <= 1000.0) {
                        return Err(invalid("guide angle must be finite radians within ±1000"));
                    }
                    let (uy, ux) = a.sin_cos();
                    Ok(Guide::LineAngle { angle: a, ux, uy })
                }
            },
            GuideKind::Ellipse => {
                if angle.is_some() {
                    return Err(invalid("ellipse guide does not take an angle"));
                }
                let (x0, y0, x1, y1) = points.ok_or_else(|| invalid("ellipse guide needs x0, y0, x1, y1"))?;
                let (rx, ry) = ((x1 - x0).abs() / 2.0, (y1 - y0).abs() / 2.0);
                if !(rx >= MIN_GUIDE_EXTENT && ry >= MIN_GUIDE_EXTENT) {
                    return Err(invalid("ellipse guide needs a non-empty bounding box"));
                }
                Ok(Guide::Ellipse { cx: (x0 + x1) / 2.0, cy: (y0 + y1) / 2.0, rx, ry })
            }
        }
    }

    pub fn is_off(&self) -> bool {
        matches!(self, Guide::Off)
    }

    /// Per-stroke projector, or `None` when the guide is off.
    pub fn activate(&self) -> Option<ActiveGuide> {
        (!self.is_off()).then_some(ActiveGuide { guide: *self, anchor: None, last: None })
    }

    /// State as reported by `guide.get` / `document.info`.
    pub fn to_json(&self) -> Value {
        match *self {
            Guide::Off => json!({"kind": "off"}),
            Guide::Line { a, b, .. } => json!({"kind": "line", "x0": a.x, "y0": a.y, "x1": b.x, "y1": b.y}),
            Guide::LineAngle { angle, .. } => json!({"kind": "line", "angle": angle}),
            Guide::Ellipse { cx, cy, rx, ry } => {
                json!({"kind": "ellipse", "x0": cx - rx, "y0": cy - ry, "x1": cx + rx, "y1": cy + ry})
            }
        }
    }
}

/// A guide bound to one stroke (holds the anchor of an angle line and the last ellipse point).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveGuide {
    guide: Guide,
    anchor: Option<PointF>,
    last: Option<PointF>,
}

impl ActiveGuide {
    /// Project a confirmed sample (updates the per-stroke anchor state).
    pub fn project_confirmed(&mut self, s: &StrokeSample) -> StrokeSample {
        let p = PointF::new(f64::from(s.x), f64::from(s.y));
        if !p.is_finite() {
            return *s; // rejected later by sample validation; never becomes the anchor
        }
        let anchor = *self.anchor.get_or_insert(p);
        let q = self.project_point(p, anchor);
        self.last = Some(q);
        StrokeSample { x: q.x as f32, y: q.y as f32, ..*s }
    }

    /// Project a predicted (display-only) sample without changing the stroke's anchor state.
    pub fn project_predicted(&self, s: &StrokeSample) -> StrokeSample {
        let p = PointF::new(f64::from(s.x), f64::from(s.y));
        if !p.is_finite() {
            return *s;
        }
        let q = self.project_point(p, self.anchor.unwrap_or(p));
        StrokeSample { x: q.x as f32, y: q.y as f32, ..*s }
    }

    fn project_point(&self, p: PointF, anchor: PointF) -> PointF {
        match self.guide {
            Guide::Off => p,
            Guide::Line { a, ux, uy, .. } => on_line(p, a, ux, uy),
            Guide::LineAngle { ux, uy, .. } => on_line(p, anchor, ux, uy),
            Guide::Ellipse { cx, cy, rx, ry } => {
                // Normalize to the unit circle, push to radius 1, scale back.
                let (nx, ny) = ((p.x - cx) / rx, (p.y - cy) / ry);
                let n = nx.hypot(ny);
                if n.is_finite() && n > 1e-9 {
                    PointF::new(cx + rx * nx / n, cy + ry * ny / n)
                } else {
                    self.last.unwrap_or(PointF::new(cx + rx, cy))
                }
            }
        }
    }
}

fn on_line(p: PointF, a: PointF, ux: f64, uy: f64) -> PointF {
    let t = (p.x - a.x) * ux + (p.y - a.y) * uy;
    PointF::new(a.x + t * ux, a.y + t * uy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f32, y: f32) -> StrokeSample {
        let mut s = StrokeSample::confirmed(x, y, 9, 0.7);
        s.tilt = Some(0.3);
        s
    }

    #[test]
    fn two_point_line_projects_orthogonally_and_keeps_pressure() {
        let g = Guide::new(GuideKind::Line, Some(0.0), Some(10.0), Some(100.0), Some(10.0), None).unwrap();
        let mut a = g.activate().unwrap();
        let q = a.project_confirmed(&s(40.0, 33.0));
        assert_eq!((q.x, q.y), (40.0, 10.0));
        assert_eq!((q.pressure, q.tilt, q.t_ns, q.flags), (0.7, Some(0.3), 9, s(0.0, 0.0).flags));
        // Beyond the two points: the line is infinite.
        let q = a.project_confirmed(&s(250.0, -5.0));
        assert_eq!((q.x, q.y), (250.0, 10.0));
    }

    #[test]
    fn angle_line_passes_through_first_sample() {
        let g = Guide::new(GuideKind::Line, None, None, None, None, Some(std::f64::consts::FRAC_PI_2)).unwrap();
        let mut a = g.activate().unwrap();
        let first = a.project_confirmed(&s(20.0, 30.0));
        assert_eq!((first.x, first.y), (20.0, 30.0));
        let q = a.project_confirmed(&s(55.0, 80.0));
        assert!((q.x - 20.0).abs() < 1e-4 && (q.y - 80.0).abs() < 1e-4, "{q:?}");
        // Prediction uses the same anchor and does not move it.
        let p = a.project_predicted(&s(90.0, 95.0));
        assert!((p.x - 20.0).abs() < 1e-4 && (p.y - 95.0).abs() < 1e-4);
    }

    #[test]
    fn ellipse_projects_onto_outline() {
        let g = Guide::new(GuideKind::Ellipse, Some(10.0), Some(20.0), Some(110.0), Some(60.0), None).unwrap();
        let mut a = g.activate().unwrap();
        for (x, y) in [(60.0, 40.5), (200.0, 40.0), (61.0, -300.0), (12.0, 22.0), (60.0, 40.0)] {
            let q = a.project_confirmed(&s(x, y));
            let e = ((f64::from(q.x) - 60.0) / 50.0).powi(2) + ((f64::from(q.y) - 40.0) / 20.0).powi(2);
            assert!((e - 1.0).abs() < 1e-4, "({x},{y}) -> {q:?} e={e}");
        }
    }

    #[test]
    fn hostile_values_are_typed_errors() {
        let n = f64::NAN;
        assert!(Guide::new(GuideKind::Line, Some(n), Some(0.0), Some(1.0), Some(1.0), None).is_err());
        assert!(Guide::new(GuideKind::Line, Some(1.0), Some(1.0), Some(1.0), Some(1.0), None).is_err());
        assert!(Guide::new(GuideKind::Line, Some(1.0), None, Some(1.0), Some(1.0), None).is_err());
        assert!(Guide::new(GuideKind::Line, None, None, None, None, None).is_err());
        assert!(Guide::new(GuideKind::Line, None, None, None, None, Some(f64::INFINITY)).is_err());
        assert!(Guide::new(GuideKind::Line, Some(0.0), Some(0.0), Some(1.0), Some(1.0), Some(0.0)).is_err());
        assert!(Guide::new(GuideKind::Ellipse, Some(0.0), Some(0.0), Some(0.0), Some(9.0), None).is_err());
        assert!(Guide::new(GuideKind::Ellipse, Some(0.0), Some(0.0), Some(1e300), Some(9.0), None).is_err());
        assert!(Guide::new(GuideKind::Ellipse, None, None, None, None, None).is_err());
        assert_eq!(Guide::new(GuideKind::Off, None, None, None, None, None).unwrap(), Guide::Off);
    }
}
