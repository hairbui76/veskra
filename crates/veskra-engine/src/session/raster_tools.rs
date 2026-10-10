//! Raster-edit, selection and canvas commands (gradient, magic wand, adjustments, 4-corner
//! distort, canvas crop/resize) plus the read-only `selection.outline` / `layer.bounds` queries.
//!
//! Every mutating command here goes through [`Session::mutate`]: it works on a clone of the
//! document, so a failure changes nothing, and success records exactly one undo step.

use super::{EResult, Receipt, Session, editable_raster_layer};
use crate::command::{AdjustKindSpec, AnchorSpec, GradientKindSpec, GradientStopSpec, SelectionOp};
use serde_json::{Value, json};
use veskra_core::{BlendSpace, ColorRgba, ErrorCode, LayerId, PointF, RectF, RectI, SizeU, VeskraError};
use veskra_doc::{CanvasMap, Document, LayerKind};
use veskra_input::ViewTransform;
use veskra_raster::adjust::Adjustment;
use veskra_raster::gradient::{GradientGeometry, GradientKind};
use veskra_raster::warp::Homography;
use veskra_raster::{MaskGrid, MaskOp, Px16};

/// Maximum number of gradient stops accepted by `edit.gradient`.
pub const MAX_GRADIENT_STOPS: usize = 256;
/// Default and maximum `maxPoints` of `selection.outline`.
pub const DEFAULT_OUTLINE_POINTS: u32 = 4000;
pub const MAX_OUTLINE_POINTS: u32 = 100_000;

fn rect_json(r: RectI) -> Value {
    if r.is_empty() { Value::Null } else { json!([r.x0, r.y0, r.x1, r.y1]) }
}

/// Color ramp evaluated in the document's blend space on **premultiplied** values, the same
/// numbers the compositor interpolates when it blends two layers.
struct Ramp {
    /// `(offset, premultiplied color in blend space)`, sorted by offset (stable).
    stops: Vec<(f64, [f64; 4])>,
    linear: bool,
}

impl Ramp {
    fn new(colors: &[(f64, ColorRgba)], space: BlendSpace) -> Ramp {
        let linear = space == BlendSpace::Linear;
        let mut stops: Vec<(f64, [f64; 4])> = colors
            .iter()
            .map(|(o, c)| {
                let a = (c.a as f64).clamp(0.0, 1.0);
                let ch = |v: f32| {
                    let v = (v as f64).clamp(0.0, 1.0);
                    (if linear { veskra_color::transfer::srgb_to_linear_f64(v) } else { v }) * a
                };
                (*o, [ch(c.r), ch(c.g), ch(c.b), a])
            })
            .collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ramp { stops, linear }
    }

    fn at(&self, t: f64) -> Px16 {
        let n = self.stops.len();
        // Index of the last stop at or before `t` (hard edges: a later stop at the same offset wins).
        let i = self.stops.partition_point(|s| s.0 <= t);
        let p = if i == 0 {
            self.stops[0].1
        } else if i >= n {
            self.stops[n - 1].1
        } else {
            let (a, b) = (&self.stops[i - 1], &self.stops[i]);
            let f = ((t - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
            std::array::from_fn(|k| a.1[k] + (b.1[k] - a.1[k]) * f)
        };
        let q = |v: f64| (v.clamp(0.0, 1.0) * 65535.0 + 0.5).floor() as u16;
        let a16 = q(p[3]);
        if a16 == 0 {
            return [0; 4];
        }
        let ch = |v: f64| {
            let v = if self.linear {
                veskra_color::transfer::linear_to_srgb_f64((v / p[3]).clamp(0.0, 1.0)) * p[3]
            } else {
                v
            };
            q(v).min(a16)
        };
        [ch(p[0]), ch(p[1]), ch(p[2]), a16]
    }
}

fn combine_selection(d: &mut Document, op: SelectionOp, new: MaskGrid) {
    // Same rules as `Session::selection_shape` (kept separate to leave that function untouched).
    let combined = match (op, d.selection.take()) {
        (SelectionOp::Replace, _) | (SelectionOp::Add, None) => new,
        (SelectionOp::Subtract, None) | (SelectionOp::Intersect, None) => MaskGrid::new_empty(),
        (op, Some(mut cur)) => {
            let mop = match op {
                SelectionOp::Replace => MaskOp::Replace,
                SelectionOp::Add => MaskOp::Add,
                SelectionOp::Subtract => MaskOp::Subtract,
                SelectionOp::Intersect => MaskOp::Intersect,
            };
            cur.combine(&new, mop);
            cur
        }
    };
    d.selection = if combined.is_empty() { None } else { Some(combined) };
}

impl Session {
    // ------------------------------------------------------------------ gradient

    #[allow(clippy::too_many_arguments)]
    pub(super) fn edit_gradient(
        &mut self,
        label: &str,
        p0: (f64, f64),
        p1: (f64, f64),
        kind: GradientKindSpec,
        stops: Option<Vec<GradientStopSpec>>,
        opacity: f32,
    ) -> EResult<Receipt> {
        if !(opacity.is_finite() && (0.0..=1.0).contains(&opacity)) {
            return Err(VeskraError::invalid("gradient opacity must be in [0, 1]"));
        }
        let kind = match kind {
            GradientKindSpec::Linear => GradientKind::Linear,
            GradientKindSpec::Radial => GradientKind::Radial,
        };
        let geom = GradientGeometry::new(kind, PointF::new(p0.0, p0.1), PointF::new(p1.0, p1.1))?;
        let colors: Vec<(f64, ColorRgba)> = match stops {
            None => vec![(0.0, self.color), (1.0, self.color.with_alpha(0.0))],
            Some(list) => {
                if list.len() < 2 || list.len() > MAX_GRADIENT_STOPS {
                    return Err(VeskraError::invalid(format!(
                        "a gradient needs between 2 and {MAX_GRADIENT_STOPS} stops (got {})",
                        list.len()
                    )));
                }
                let mut out = Vec::with_capacity(list.len());
                for s in &list {
                    if !(s.offset.is_finite() && (0.0..=1.0).contains(&s.offset)) {
                        return Err(VeskraError::invalid("gradient stop offset must be in [0, 1]"));
                    }
                    let c = ColorRgba::from_hex(&s.color)
                        .ok_or_else(|| VeskraError::invalid("gradient stop color must be #RRGGBB or #RRGGBBAA"))?;
                    out.push((s.offset, c));
                }
                out
            }
        };
        let opacity16 = (opacity as f64 * 65535.0 + 0.5).floor() as u16;
        let r = self.mutate(label, |d, _| {
            let layer = editable_raster_layer(d)?;
            let b = d.bounds();
            let ramp = Ramp::new(&colors, d.color.blend_space);
            let sel = d.selection.clone();
            let area = sel.as_ref().map_or(b, |s| s.bounds().intersect(&b));
            let alpha_lock = d.layer(layer).is_some_and(|l| l.props.alpha_locked);
            let px = d.pixels_mut_editable(layer)?;
            let dirty = veskra_raster::gradient::fill_gradient(
                &mut px.tiles,
                sel.as_ref(),
                area,
                &geom,
                &|t| ramp.at(t),
                opacity16,
                alpha_lock,
            )?;
            d.mark_raster_edit(layer, "gradient")?;
            Ok((json!({}), dirty))
        })?;
        Ok(self.finish_mutation(r))
    }

    // ------------------------------------------------------------------ magic wand

    #[allow(clippy::too_many_arguments)]
    pub(super) fn selection_wand(
        &mut self,
        label: &str,
        x: f32,
        y: f32,
        tolerance: f32,
        contiguous: bool,
        sample_merged: bool,
        op: SelectionOp,
    ) -> EResult<Receipt> {
        if !(tolerance.is_finite() && (0.0..=1.0).contains(&tolerance)) || !x.is_finite() || !y.is_finite() {
            return Err(VeskraError::invalid("wand tolerance must be in [0,1] and point finite"));
        }
        let r = self.mutate(label, |d, _| {
            let b = d.bounds();
            let (sx, sy) = (x.floor() as i32, y.floor() as i32);
            if !b.contains(sx, sy) {
                return Err(VeskraError::invalid("wand point outside the canvas"));
            }
            let reference = if sample_merged {
                veskra_compose::composite_grid(d, &veskra_compose::ComposeOptions::default())
            } else {
                let id = d.active_layer.ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "no active layer"))?;
                d.layer(id).and_then(|l| l.pixels()).map(|p| p.tiles.clone()).ok_or_else(|| {
                    VeskraError::new(
                        ErrorCode::UnsupportedLayerFeature,
                        "the magic wand needs a raster or vector layer (or sampleMerged)",
                    )
                })?
            };
            // Same region as the bucket fill (incl. its soft edge), so wand + fill_selection == fill.
            let opts = veskra_raster::fill::FillOptions { tolerance, contiguous, antialias: true };
            let region = veskra_raster::fill::region(&reference, (sx, sy), &opts, b, None)?;
            combine_selection(d, op, region);
            Ok((json!({"empty": d.selection.is_none()}), b))
        })?;
        Ok(self.finish_mutation(r))
    }

    // ------------------------------------------------------------------ adjustments

    #[allow(clippy::too_many_arguments)]
    pub(super) fn edit_adjust(
        &mut self,
        label: &str,
        kind: AdjustKindSpec,
        hue: Option<f64>,
        saturation: Option<f64>,
        brightness: Option<f64>,
        contrast: Option<f64>,
        curves: [Option<Vec<[f64; 2]>>; 4],
    ) -> EResult<Receipt> {
        let [rgb, r, g, b] = curves;
        let has_curve = rgb.is_some() || r.is_some() || g.is_some() || b.is_some();
        let reject = |present: bool, field: &str| -> EResult<()> {
            if present {
                return Err(VeskraError::invalid(format!("field '{field}' does not apply to this adjustment kind")));
            }
            Ok(())
        };
        let adj = match kind {
            AdjustKindSpec::Hsb => {
                reject(contrast.is_some(), "contrast")?;
                reject(has_curve, "rgb/r/g/b")?;
                Adjustment::hsb(hue.unwrap_or(0.0), saturation.unwrap_or(0.0), brightness.unwrap_or(0.0))?
            }
            AdjustKindSpec::BrightnessContrast => {
                reject(hue.is_some(), "hue")?;
                reject(saturation.is_some(), "saturation")?;
                reject(has_curve, "rgb/r/g/b")?;
                Adjustment::brightness_contrast(brightness.unwrap_or(0.0), contrast.unwrap_or(0.0))?
            }
            AdjustKindSpec::Curves => {
                reject(hue.is_some(), "hue")?;
                reject(saturation.is_some(), "saturation")?;
                reject(brightness.is_some(), "brightness")?;
                reject(contrast.is_some(), "contrast")?;
                Adjustment::curves(rgb.as_deref(), r.as_deref(), g.as_deref(), b.as_deref())?
            }
            AdjustKindSpec::Invert => {
                reject(hue.is_some(), "hue")?;
                reject(saturation.is_some(), "saturation")?;
                reject(brightness.is_some(), "brightness")?;
                reject(contrast.is_some(), "contrast")?;
                reject(has_curve, "rgb/r/g/b")?;
                Adjustment::Invert
            }
        };
        let r = self.mutate(label, |d, _| {
            let layer = editable_raster_layer(d)?;
            let b = d.bounds();
            let sel = d.selection.clone();
            let px = d.pixels_mut_editable(layer)?;
            let dirty = veskra_raster::adjust::adjust_pixels(&mut px.tiles, sel.as_ref(), &adj).intersect(&b);
            d.mark_raster_edit(layer, "adjust")?;
            Ok((json!({}), dirty))
        })?;
        Ok(self.finish_mutation(r))
    }

    // ------------------------------------------------------------------ distort / perspective

    pub(super) fn edit_transform_quad(&mut self, label: &str, quad: [[f64; 2]; 4]) -> EResult<Receipt> {
        let q = quad.map(|p| PointF::new(p[0], p[1]));
        let r = self.mutate(label, |d, _| {
            let layer = editable_raster_layer(d)?;
            let b = d.bounds();
            let sel = d.selection.clone();
            let src = match &sel {
                Some(s) => s.bounds().intersect(&b),
                None => {
                    d.layer(layer).and_then(|l| l.pixels()).map(|p| p.tiles.content_bounds()).unwrap_or(RectI::EMPTY)
                }
            };
            if src.is_empty() {
                return Err(VeskraError::new(
                    ErrorCode::InvalidState,
                    "nothing to transform (empty layer or selection)",
                ));
            }
            let h =
                Homography::rect_to_quad(RectF::new(src.x0 as f64, src.y0 as f64, src.x1 as f64, src.y1 as f64), q)?;
            let px = d.pixels_mut_editable(layer)?;
            match &sel {
                Some(sel) => {
                    let moved = veskra_raster::ops::cut(&mut px.tiles, sel);
                    let t = veskra_raster::warp::try_warp_pixels(&moved, &h, b)?;
                    veskra_raster::ops::paste_over(&mut px.tiles, &t);
                }
                None => px.tiles = veskra_raster::warp::try_warp_pixels(&px.tiles, &h, b)?,
            }
            if let Some(sel) = sel {
                let warped = veskra_raster::warp::try_warp_mask(&sel, src, &h, b)?;
                d.selection = if warped.is_empty() { None } else { Some(warped) };
            }
            d.mark_raster_edit(layer, "transform")?;
            let dst = RectI::covering(&h.quad_bounds().inflate(1.0));
            Ok((json!({"sourceBounds": rect_json(src)}), src.union(&dst).intersect(&b)))
        })?;
        Ok(self.finish_mutation(r))
    }

    // ------------------------------------------------------------------ canvas

    /// After a canvas-size change (command, undo or redo): everything inside the old and the new
    /// canvas must be redrawn, and the view is re-fitted like on document open.
    pub(super) fn canvas_changed(&mut self, old: RectI, r: &mut Receipt) {
        let Some(d) = &self.doc else { return };
        let new = d.bounds();
        if new == old {
            return;
        }
        r.invalidated = vec![old.union(&new)];
        if self.view.view_w > 0.0 {
            self.view = ViewTransform::fit(d.size(), self.view.view_w, self.view.view_h);
        }
        r.view_changed = true;
    }

    fn remap_canvas(&mut self, label: &str, size: SizeU, map: CanvasMap, result: Value) -> EResult<Receipt> {
        let old = self.doc()?.bounds();
        let r = self.mutate(label, |d, _| {
            d.remap_canvas(size, map)?;
            Ok((result, old.union(&d.bounds())))
        })?;
        let mut receipt = self.finish_mutation(r);
        self.canvas_changed(old, &mut receipt);
        Ok(receipt)
    }

    pub(super) fn canvas_crop(&mut self, label: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> EResult<Receipt> {
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite() && v.abs() <= 1e9) {
            return Err(VeskraError::invalid("crop rectangle must be finite"));
        }
        let old = self.doc()?.bounds();
        let rd = |v: f64| v.round() as i64;
        let (ax, bx) = (rd(x0.min(x1)), rd(x0.max(x1)));
        let (ay, by) = (rd(y0.min(y1)), rd(y0.max(y1)));
        let (w, h) = (bx - ax, by - ay);
        if w <= 0 || h <= 0 {
            return Err(VeskraError::invalid("crop rectangle is empty (needs at least 1 x 1 px after rounding)"));
        }
        if ax >= old.x1 as i64 || bx <= 0 || ay >= old.y1 as i64 || by <= 0 {
            return Err(VeskraError::invalid("crop rectangle does not intersect the canvas"));
        }
        if w > u32::MAX as i64 || h > u32::MAX as i64 {
            return Err(VeskraError::alloc("crop rectangle too large"));
        }
        veskra_core::limits::check_document_size(w as u32, h as u32)?;
        let rect = RectI::new(ax as i32, ay as i32, bx as i32, by as i32);
        if rect == old {
            return Err(VeskraError::invalid("crop rectangle equals the canvas (nothing to do)"));
        }
        let result = json!({"width": w, "height": h, "offsetX": -ax, "offsetY": -ay});
        self.remap_canvas(
            label,
            SizeU::new(w as u32, h as u32),
            CanvasMap::Offset { dx: -(ax as i32), dy: -(ay as i32) },
            result,
        )
    }

    pub(super) fn canvas_resize(
        &mut self,
        label: &str,
        width: u32,
        height: u32,
        resample: bool,
        anchor: Option<AnchorSpec>,
    ) -> EResult<Receipt> {
        veskra_core::limits::check_document_size(width, height)?;
        let old = self.doc()?.size();
        if (old.width, old.height) == (width, height) {
            return Err(VeskraError::invalid("canvas already has this size (nothing to do)"));
        }
        if resample {
            if anchor.is_some() {
                return Err(VeskraError::invalid("'anchor' applies only when 'resample' is false"));
            }
            let (sx, sy) = (width as f64 / old.width as f64, height as f64 / old.height as f64);
            let result = json!({"width": width, "height": height, "resampled": true, "scaleX": sx, "scaleY": sy,
                                "offsetX": 0, "offsetY": 0});
            return self.remap_canvas(label, SizeU::new(width, height), CanvasMap::Resample, result);
        }
        let (fx, fy) = anchor.unwrap_or_default().factors();
        let off = |new: u32, old: u32, f: u8| -> i32 {
            let d = new as i64 - old as i64;
            (match f {
                0 => 0,
                1 => d.div_euclid(2),
                _ => d,
            }) as i32
        };
        let (dx, dy) = (off(width, old.width, fx), off(height, old.height, fy));
        let result = json!({"width": width, "height": height, "resampled": false, "scaleX": 1.0, "scaleY": 1.0,
                            "offsetX": dx, "offsetY": dy});
        self.remap_canvas(label, SizeU::new(width, height), CanvasMap::Offset { dx, dy }, result)
    }

    // ------------------------------------------------------------------ read-only queries

    pub(super) fn selection_outline(&self, max_points: Option<u32>) -> EResult<Receipt> {
        let max = max_points.unwrap_or(DEFAULT_OUTLINE_POINTS);
        if !(8..=MAX_OUTLINE_POINTS).contains(&max) {
            return Err(VeskraError::invalid(format!("maxPoints must be in 8..={MAX_OUTLINE_POINTS}")));
        }
        let d = self.doc()?;
        let b = d.bounds();
        let Some(sel) = d.selection.as_ref().filter(|s| !s.bounds().intersect(&b).is_empty()) else {
            return Ok(self.receipt(json!({"empty": true, "bounds": null, "contours": [], "truncated": false,
                                          "pointCount": 0})));
        };
        let o = veskra_raster::outline::trace(sel, b, max as usize);
        let contours: Vec<Vec<[i32; 2]>> =
            o.contours.iter().map(|c| c.iter().map(|&(x, y)| [x, y]).collect()).collect();
        Ok(self.receipt(json!({
            "empty": false,
            "bounds": rect_json(sel.bounds().intersect(&b)),
            "contours": contours,
            "truncated": o.truncated,
            "pointCount": o.point_count(),
        })))
    }

    pub(super) fn layer_bounds(&self, id: Option<String>) -> EResult<Receipt> {
        let d = self.doc()?;
        let id = match id {
            Some(s) => LayerId::parse(&s)?,
            None => d.active_layer.ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "no active layer"))?,
        };
        fn content(d: &Document, id: LayerId, depth: usize) -> RectI {
            let Some(l) = d.layer(id) else { return RectI::EMPTY };
            match &l.kind {
                LayerKind::Raster(p) | LayerKind::Vector(p) => p.tiles.content_bounds(),
                LayerKind::Group(g) if depth <= veskra_core::limits::MAX_GROUP_DEPTH => {
                    g.children.iter().fold(RectI::EMPTY, |acc, c| acc.union(&content(d, *c, depth + 1)))
                }
                LayerKind::Group(_) => RectI::EMPTY,
            }
        }
        if d.layer(id).is_none() {
            return Err(VeskraError::new(ErrorCode::NotFound, "layer not found"));
        }
        Ok(self.receipt(json!({"layerId": id.to_string(), "bounds": rect_json(content(d, id, 0))})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hex: &str) -> ColorRgba {
        ColorRgba::from_hex(hex).unwrap_or(ColorRgba::BLACK)
    }

    #[test]
    fn ramp_hits_stops_and_interpolates_premultiplied() {
        let r = Ramp::new(&[(0.0, c("#FF0000")), (1.0, c("#FF000000"))], BlendSpace::GammaEncoded);
        assert_eq!(r.at(0.0), [65535, 0, 0, 65535]);
        assert_eq!(r.at(1.0), [0; 4]);
        // Foreground → transparent keeps the hue: premultiplied red at half alpha.
        assert_eq!(r.at(0.5), [32768, 0, 0, 32768]);
        // Unsorted stops are ordered; before the first / after the last stop the end colors pad.
        let r = Ramp::new(&[(0.75, c("#FFFFFF")), (0.25, c("#000000"))], BlendSpace::GammaEncoded);
        assert_eq!(r.at(0.0), [0, 0, 0, 65535]);
        assert_eq!(r.at(0.5), [32768, 32768, 32768, 65535]);
        assert_eq!(r.at(0.9), [65535; 4]);
        // Hard edge: two stops at the same offset.
        let r = Ramp::new(
            &[(0.0, c("#000000")), (0.5, c("#000000")), (0.5, c("#FFFFFF")), (1.0, c("#FFFFFF"))],
            BlendSpace::GammaEncoded,
        );
        assert_eq!(r.at(0.4999), [0, 0, 0, 65535]);
        assert_eq!(r.at(0.5), [65535; 4]);
    }

    #[test]
    fn linear_blend_space_interpolates_in_linear_light() {
        let stops = [(0.0, c("#000000")), (1.0, c("#FFFFFF"))];
        let lin = Ramp::new(&stops, BlendSpace::Linear).at(0.5);
        let enc = Ramp::new(&stops, BlendSpace::GammaEncoded).at(0.5);
        // 50 % linear light is sRGB-encoded ≈ 0.7354 (188/255), brighter than the encoded midpoint.
        let expected = (veskra_color::transfer::linear_to_srgb_f64(0.5) * 65535.0 + 0.5).floor() as u16;
        assert_eq!(lin, [expected, expected, expected, 65535]);
        assert_eq!(enc[0], 32768);
        assert!((lin[0] as f64 / 65535.0 - 0.7354).abs() < 1e-3);
        // End points are exact in both spaces.
        assert_eq!(Ramp::new(&stops, BlendSpace::Linear).at(1.0), [65535; 4]);
        assert_eq!(Ramp::new(&stops, BlendSpace::Linear).at(0.0), [0, 0, 0, 65535]);
    }
}
