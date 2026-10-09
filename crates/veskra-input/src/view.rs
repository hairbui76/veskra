//! Canvas view transform (document ↔ view) and the finger navigation gesture solver (AND-04).
//!
//! `doc_to_view = translate(pan) · rotate(rotation) · scale(zoom) · mirror` where `mirror` is
//! `scale(-1, 1)` applied in document space. Equivalently `view = pan + R(rotation)·zoom·M·doc`.
//! `pan` is therefore the view position of the document origin. Units: view px, document px,
//! radians (positive rotation is clockwise on screen because +y points down).
//!
//! Fields are public for serialization; every method uses [`ViewTransform::sanitized`] values
//! (zoom clamped to [`MIN_ZOOM`]..=[`MAX_ZOOM`], non-finite values replaced), so the transform
//! is always invertible.

use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_2, PI};
use veskra_core::{Affine, PointF, SizeU};

pub const MIN_ZOOM: f64 = 0.02;
pub const MAX_ZOOM: f64 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewTransform {
    pub zoom: f64,
    pub rotation: f64,
    pub pan: PointF,
    pub mirror: bool,
    pub view_w: f64,
    pub view_h: f64,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self::identity(0.0, 0.0)
    }
}

/// Clamp zoom into the supported range; non-finite → 1.
pub fn clamp_zoom(z: f64) -> f64 {
    if z.is_finite() { z.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 }
}

/// Wrap an angle into `(-PI, PI]`; non-finite → 0.
pub fn normalize_angle(a: f64) -> f64 {
    if !a.is_finite() {
        return 0.0;
    }
    if a > -PI && a <= PI {
        return a; // exact for in-range values (pure pans keep rotation bit-identical)
    }
    let mut r = a.rem_euclid(2.0 * PI);
    if r > PI {
        r -= 2.0 * PI;
    }
    r
}

/// Snap to the nearest multiple of 90° when within `threshold` radians (`threshold <= 0` disables).
pub fn snap_rotation(a: f64, threshold: f64) -> f64 {
    if threshold.is_nan() || threshold <= 0.0 {
        return a;
    }
    let k = (a / FRAC_PI_2).round();
    let target = k * FRAC_PI_2;
    if (a - target).abs() <= threshold { normalize_angle(target) } else { a }
}

impl ViewTransform {
    pub fn identity(view_w: f64, view_h: f64) -> Self {
        Self { zoom: 1.0, rotation: 0.0, pan: PointF::new(0.0, 0.0), mirror: false, view_w, view_h }
    }

    /// Effective values used by every method.
    pub fn sanitized(&self) -> Self {
        let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
        let size = |v: f64| if v.is_finite() && v > 0.0 { v } else { 0.0 };
        Self {
            zoom: clamp_zoom(self.zoom),
            rotation: normalize_angle(self.rotation),
            pan: PointF::new(fin(self.pan.x), fin(self.pan.y)),
            mirror: self.mirror,
            view_w: size(self.view_w),
            view_h: size(self.view_h),
        }
    }

    /// Linear part (mirror, scale, rotate) without pan.
    fn linear(&self) -> Affine {
        let s = self.sanitized();
        let m = if s.mirror { Affine::scale(-1.0, 1.0) } else { Affine::IDENTITY };
        m.then(&Affine::scale(s.zoom, s.zoom)).then(&Affine::rotate(s.rotation))
    }

    pub fn doc_to_view(&self) -> Affine {
        let s = self.sanitized();
        self.linear().then(&Affine::translate(s.pan.x, s.pan.y))
    }

    /// Analytic inverse of [`Self::doc_to_view`] (always defined).
    pub fn view_to_doc(&self) -> Affine {
        let s = self.sanitized();
        let m = if s.mirror { Affine::scale(-1.0, 1.0) } else { Affine::IDENTITY };
        Affine::translate(-s.pan.x, -s.pan.y)
            .then(&Affine::rotate(-s.rotation))
            .then(&Affine::scale(1.0 / s.zoom, 1.0 / s.zoom))
            .then(&m)
    }

    /// Fit the whole document centered in the view, rotation 0, no mirror.
    /// Degenerate sizes give the identity transform.
    pub fn fit(doc: SizeU, view_w: f64, view_h: f64) -> Self {
        let base = Self::identity(view_w, view_h).sanitized();
        if doc.width == 0 || doc.height == 0 || base.view_w <= 0.0 || base.view_h <= 0.0 {
            return base;
        }
        let (w, h) = (f64::from(doc.width), f64::from(doc.height));
        let zoom = clamp_zoom((base.view_w / w).min(base.view_h / h));
        Self { zoom, pan: PointF::new((base.view_w - w * zoom) / 2.0, (base.view_h - h * zoom) / 2.0), ..base }
    }

    /// Same zoom/rotation/mirror with `pan` chosen so `doc_pt` maps exactly to `view_pt`.
    pub fn with_anchor(&self, doc_pt: PointF, view_pt: PointF) -> Self {
        let s = self.sanitized();
        let l = self.linear().apply(doc_pt);
        let pan = PointF::new(view_pt.x - l.x, view_pt.y - l.y);
        if !pan.is_finite() {
            return s;
        }
        Self { pan, ..s }
    }

    pub fn zoomed_about(&self, view_pt: PointF, zoom: f64) -> Self {
        let d = self.view_to_doc().apply(view_pt);
        Self { zoom: clamp_zoom(zoom), ..self.sanitized() }.with_anchor(d, view_pt)
    }

    pub fn rotated_about(&self, view_pt: PointF, delta: f64) -> Self {
        let d = self.view_to_doc().apply(view_pt);
        let s = self.sanitized();
        Self { rotation: normalize_angle(s.rotation + delta), ..s }.with_anchor(d, view_pt)
    }

    pub fn mirrored_about(&self, view_pt: PointF, mirror: bool) -> Self {
        let d = self.view_to_doc().apply(view_pt);
        Self { mirror, ..self.sanitized() }.with_anchor(d, view_pt)
    }

    /// New view size (device rotation / window resize) keeping the document point at the old
    /// view center at the new view center; zoom and rotation are preserved.
    pub fn resized(&self, view_w: f64, view_h: f64) -> Self {
        let s = self.sanitized();
        let d = self.view_to_doc().apply(PointF::new(s.view_w / 2.0, s.view_h / 2.0));
        let n = Self { view_w, view_h, ..s }.sanitized();
        n.with_anchor(d, PointF::new(n.view_w / 2.0, n.view_h / 2.0))
    }
}

/// Navigation gesture tuning.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GestureConfig {
    pub zoom: bool,
    pub rotate: bool,
    /// Snap the resulting rotation to 0/90/180/270° when within this many radians (0 disables).
    pub rotation_snap_rad: f64,
    /// Below this initial finger span (view px) a two-finger gesture only pans.
    pub min_span_px: f64,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self { zoom: true, rotate: true, rotation_snap_rad: 5f64.to_radians(), min_span_px: 8.0 }
    }
}

/// Similarity-transform solver for one- or two-finger navigation.
///
/// Built from the view and finger positions at the moment the finger set last changed. The
/// result maps the start centroid's document point exactly onto the current centroid (stable
/// anchor), whatever zoom clamping or rotation snapping is applied. Without clamping/snapping
/// both fingers stay over the document points they first touched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewGesture {
    start: ViewTransform,
    p0: PointF,
    q0: Option<PointF>,
    anchor_doc: PointF,
}

fn mid(a: PointF, b: PointF) -> PointF {
    a.lerp(b, 0.5)
}

impl ViewGesture {
    pub fn one_finger(start: &ViewTransform, p0: PointF) -> Self {
        let start = start.sanitized();
        Self { start, p0, q0: None, anchor_doc: start.view_to_doc().apply(p0) }
    }

    pub fn two_finger(start: &ViewTransform, p0: PointF, q0: PointF) -> Self {
        let start = start.sanitized();
        Self { start, p0, q0: Some(q0), anchor_doc: start.view_to_doc().apply(mid(p0, q0)) }
    }

    pub fn start_view(&self) -> &ViewTransform {
        &self.start
    }

    /// Document point held under the finger centroid.
    pub fn anchor_doc(&self) -> PointF {
        self.anchor_doc
    }

    /// View for the current finger positions. `q1` is ignored for one-finger gestures.
    pub fn solve(&self, p1: PointF, q1: Option<PointF>, cfg: &GestureConfig) -> ViewTransform {
        let s = self.start;
        let (Some(q0), Some(q1)) = (self.q0, q1) else {
            return if p1.is_finite() { s.with_anchor(self.anchor_doc, p1) } else { s };
        };
        if !p1.is_finite() || !q1.is_finite() {
            return s;
        }
        let (v0, v1) = (PointF::new(q0.x - self.p0.x, q0.y - self.p0.y), PointF::new(q1.x - p1.x, q1.y - p1.y));
        let span0 = v0.x.hypot(v0.y);
        let span1 = v1.x.hypot(v1.y);
        let (mut scale, mut delta) = (1.0, 0.0);
        if span0 >= cfg.min_span_px && span0 > 0.0 && span1 > 0.0 {
            scale = span1 / span0;
            delta = v1.y.atan2(v1.x) - v0.y.atan2(v0.x);
        }
        let mut out = s;
        if cfg.zoom {
            out.zoom = clamp_zoom(s.zoom * scale);
        }
        if cfg.rotate {
            out.rotation = snap_rotation(normalize_angle(s.rotation + delta), cfg.rotation_snap_rad);
        }
        out.with_anchor(self.anchor_doc, mid(p1, q1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: PointF, b: PointF, eps: f64) -> bool {
        (a.x - b.x).abs() <= eps && (a.y - b.y).abs() <= eps
    }

    #[test]
    fn inverse_roundtrip_with_mirror_rotation() {
        for mirror in [false, true] {
            let v = ViewTransform {
                zoom: 2.5,
                rotation: 0.7,
                pan: PointF::new(31.0, -12.0),
                mirror,
                view_w: 800.0,
                view_h: 600.0,
            };
            let p = PointF::new(123.25, -45.5);
            let q = v.view_to_doc().apply(v.doc_to_view().apply(p));
            assert!(close(p, q, 1e-9));
            let m = v.doc_to_view().then(&v.view_to_doc());
            assert!(close(m.apply(PointF::new(1.0, 2.0)), PointF::new(1.0, 2.0), 1e-9));
        }
    }

    #[test]
    fn fit_centers_and_clamps() {
        let v = ViewTransform::fit(SizeU::new(2000, 1000), 1000.0, 1000.0);
        assert_eq!(v.zoom, 0.5);
        let c = v.doc_to_view().apply(PointF::new(1000.0, 500.0));
        assert!(close(c, PointF::new(500.0, 500.0), 1e-9));
        let tiny = ViewTransform::fit(SizeU::new(1, 1), 1000.0, 1000.0);
        assert_eq!(tiny.zoom, MAX_ZOOM);
        assert_eq!(ViewTransform::fit(SizeU::new(0, 10), 100.0, 100.0).zoom, 1.0);
        assert_eq!(ViewTransform::fit(SizeU::new(10, 10), f64::NAN, 100.0).zoom, 1.0);
    }

    #[test]
    fn sanitized_clamps() {
        let v = ViewTransform { zoom: 1e9, rotation: 7.0, ..ViewTransform::identity(10.0, 10.0) };
        let s = v.sanitized();
        assert_eq!(s.zoom, MAX_ZOOM);
        assert!(s.rotation > -PI && s.rotation <= PI);
        assert_eq!(ViewTransform { zoom: f64::NAN, ..v }.sanitized().zoom, 1.0);
        assert_eq!(ViewTransform { zoom: 0.0, ..v }.sanitized().zoom, MIN_ZOOM);
        assert!(ViewTransform { zoom: 0.0, ..v }.view_to_doc().is_finite());
    }

    #[test]
    fn about_operations_keep_anchor() {
        let v = ViewTransform { zoom: 1.3, rotation: 0.2, ..ViewTransform::identity(800.0, 600.0) };
        let a = PointF::new(300.0, 200.0);
        let d = v.view_to_doc().apply(a);
        for n in [v.zoomed_about(a, 4.0), v.rotated_about(a, 1.1), v.mirrored_about(a, true)] {
            assert!(close(n.doc_to_view().apply(d), a, 1e-9));
        }
        let center = v.view_to_doc().apply(PointF::new(400.0, 300.0));
        let r = v.resized(600.0, 800.0);
        assert!(close(r.doc_to_view().apply(center), PointF::new(300.0, 400.0), 1e-9));
        assert_eq!((r.zoom, r.rotation), (v.zoom, v.rotation));
    }

    #[test]
    fn rotation_snapping() {
        let t = 5f64.to_radians();
        assert_eq!(snap_rotation(0.05, t), 0.0);
        assert!((snap_rotation(FRAC_PI_2 + 0.04, t) - FRAC_PI_2).abs() < 1e-12);
        assert!((snap_rotation(PI - 0.03, t) - PI).abs() < 1e-12);
        assert!((snap_rotation(-FRAC_PI_2 - 0.02, t) + FRAC_PI_2).abs() < 1e-12);
        assert_eq!(snap_rotation(0.5, t), 0.5);
        assert_eq!(snap_rotation(0.05, 0.0), 0.05);
    }

    #[test]
    fn two_finger_similarity_pins_both_fingers() {
        let start =
            ViewTransform { zoom: 0.8, rotation: -0.3, mirror: true, ..ViewTransform::identity(1600.0, 1000.0) };
        let (p0, q0) = (PointF::new(700.0, 500.0), PointF::new(900.0, 520.0));
        let g = ViewGesture::two_finger(&start, p0, q0);
        let cfg = GestureConfig { rotation_snap_rad: 0.0, ..GestureConfig::default() };
        let (p1, q1) = (PointF::new(650.0, 430.0), PointF::new(1010.0, 610.0));
        let v = g.solve(p1, Some(q1), &cfg);
        let inv0 = start.view_to_doc();
        assert!(close(v.doc_to_view().apply(inv0.apply(p0)), p1, 1e-9));
        assert!(close(v.doc_to_view().apply(inv0.apply(q0)), q1, 1e-9));
        assert!(close(v.view_to_doc().apply(mid(p1, q1)), g.anchor_doc(), 1e-9));
        // pure pan keeps zoom & rotation bit-exact
        let pan = g.solve(PointF::new(p0.x + 40.0, p0.y - 7.0), Some(PointF::new(q0.x + 40.0, q0.y - 7.0)), &cfg);
        assert_eq!((pan.zoom, pan.rotation), (start.zoom, start.rotation));
        // one-finger pan
        let g1 = ViewGesture::one_finger(&start, p0);
        let v1 = g1.solve(p1, None, &cfg);
        assert!(close(v1.doc_to_view().apply(inv0.apply(p0)), p1, 1e-9));
        // non-finite input leaves the view unchanged
        assert_eq!(g.solve(PointF::new(f64::NAN, 0.0), Some(q1), &cfg), start.sanitized());
    }

    #[test]
    fn clamped_zoom_still_anchors() {
        let start = ViewTransform { zoom: 60.0, ..ViewTransform::identity(1000.0, 1000.0) };
        let g = ViewGesture::two_finger(&start, PointF::new(400.0, 400.0), PointF::new(500.0, 400.0));
        let v = g.solve(PointF::new(100.0, 400.0), Some(PointF::new(900.0, 420.0)), &GestureConfig::default());
        assert_eq!(v.zoom, MAX_ZOOM);
        assert!(close(v.view_to_doc().apply(PointF::new(500.0, 410.0)), g.anchor_doc(), 1e-9));
        // tiny initial span: pan only
        let g = ViewGesture::two_finger(&start, PointF::new(400.0, 400.0), PointF::new(401.0, 400.0));
        let v = g.solve(PointF::new(300.0, 400.0), Some(PointF::new(600.0, 400.0)), &GestureConfig::default());
        assert_eq!(v.zoom, 60.0);
    }
}
