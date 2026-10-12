//! Stroke-time tools on a [`Session`]: the stroke pipeline (begin → samples / prediction →
//! commit) with symmetry, guides, the smudge tool and shape commands plugged in.
//!
//! Every gesture — a drawn stroke with any number of symmetry replicas, a smudge, a shape with
//! or without fill — is committed as **exactly one** undo step. All state here is session (UI)
//! state; none of it is written to `.veskra` files. What reaches the document are ordinary
//! stroke records (one per symmetry replica) and raster edits.
//!
//! Sample flow: confirmed/predicted samples → guide projection → symmetry replicas → renderers.
//! A gesture with a single brush renderer uses the renderer's own base ⊕ stroke compositing
//! (bit-identical to the pre-symmetry engine); several renderers are layered in commit order by
//! `veskra_brush::composite_layered`.

use super::{EResult, FALLBACK_PRESSURE, LiveStroke, Receipt, Session, editable_raster_layer, internal};
use crate::command::ToolKind;
use crate::dirty::DirtySet;
use crate::guide::{Guide, GuideKind};
use crate::shape::ShapePath;
use crate::symmetry::{Symmetry, SymmetryMode};
use serde_json::{Value, json};
use std::sync::Arc;
use veskra_brush::{
    FinishedStroke, LayeredPart, MAX_SMUDGE_SIZE_PX, SmudgeContext, SmudgeParams, SmudgeStroke, StrokeContext,
    StrokeMode as BrushMode, StrokeRenderer, composite_layered,
};
use veskra_core::{ErrorCode, PointF, RectI, StrokeId, StrokeSample, VeskraError};
use veskra_doc::{LayerKind, StrokeComposition, StrokeGeometry, StrokeMode, StrokeRecord, VectorFidelity};
use veskra_raster::{MaskOp, TileGrid};

/// Smudge strength of a new session.
pub(super) const DEFAULT_SMUDGE_STRENGTH: f32 = 0.5;

/// Work budget of one gesture: path length (document px) × strokes it drives. Dab placement
/// walks the whole path — also where it runs off the canvas — once per symmetry replica, so
/// without a bound a hostile path (or a huge shape under 32-fold symmetry) costs minutes.
/// It applies to shapes, smudge strokes, ruled strokes and strokes with symmetry replicas; a
/// plain brush stroke keeps the renderer's own per-stroke bounds. 2 000 000 px is still 62 500 px of pen
/// travel per stroke at the maximum of 32 replicas (500 000 px with quad symmetry).
pub(super) const MAX_GESTURE_PATH_PX: f64 = 2_000_000.0;
/// Same budget for one display-only prediction (path px × strokes); a longer predicted tail of a
/// symmetric or ruled stroke is simply not previewed.
pub(super) const MAX_PREDICTION_PATH_PX: f64 = 100_000.0;

/// Session tool state (never persisted in a document).
#[derive(Debug, Clone)]
pub(super) struct ToolState {
    pub(super) symmetry: Symmetry,
    pub(super) guide: Guide,
    pub(super) tool: ToolKind,
    pub(super) smudge_strength: f32,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            symmetry: Symmetry::default(),
            guide: Guide::Off,
            tool: ToolKind::Brush,
            smudge_strength: DEFAULT_SMUDGE_STRENGTH,
        }
    }
}

/// What a live gesture draws with.
pub(super) enum LiveInk {
    /// `[0]` is the user's stroke; the others are its symmetry replicas, in `LiveStroke::replicas`
    /// order. Never empty.
    Brush(Vec<StrokeRenderer>),
    /// Head 0 is the user's stroke, head `i + 1` follows replica `i`.
    Smudge(Box<SmudgeStroke>),
}

/// A shape command's contribution to the stroke it draws.
pub(super) struct ShapeBegin {
    /// The layer with the shape's fill already applied (`None` = no fill).
    pub(super) filled_base: Option<TileGrid>,
    pub(super) fill_dirty: RectI,
}

pub(super) struct BeginOpts {
    /// Hardware eraser end / explicit `eraser: true`.
    pub(super) eraser: bool,
    /// Tool for this stroke only (`stroke.begin {tool}`); `None` = the session tool.
    pub(super) tool: Option<ToolKind>,
    /// Set by `shape.*`: draw with the brush, no guide, no stabilization.
    pub(super) shape: Option<ShapeBegin>,
}

impl LiveStroke {
    /// Tiles the display shows for the stroke's layer while it is live.
    pub(super) fn display_tiles(&self) -> &TileGrid {
        match (&self.predicted_tiles, &self.ink) {
            (Some(p), _) => p,
            (None, LiveInk::Smudge(sm)) => sm.tiles(),
            (None, LiveInk::Brush(_)) => &self.live_tiles,
        }
    }
}

fn canvas_center(b: RectI) -> PointF {
    PointF::new(f64::from(b.x1) / 2.0, f64::from(b.y1) / 2.0)
}

/// Length of the polyline `last → samples…` and its new end point.
fn path_length(mut last: Option<PointF>, samples: &[StrokeSample]) -> (f64, Option<PointF>) {
    let mut len = 0.0;
    for s in samples {
        let p = PointF::new(f64::from(s.x), f64::from(s.y));
        if let Some(l) = last {
            len += l.distance(p);
        }
        last = Some(p);
    }
    (len, last)
}

fn layered(parts: &[StrokeRenderer]) -> Vec<LayeredPart<'_>> {
    parts.iter().map(|p| LayeredPart { stroke: p, predicted: None }).collect()
}

impl Session {
    // ------------------------------------------------------------------ tool state commands

    pub(super) fn symmetry_json(&self) -> Value {
        self.tools.symmetry.to_json(self.doc.as_ref().map(|d| canvas_center(d.bounds())))
    }

    pub(super) fn tool_json(&self) -> Value {
        json!({"tool": self.tools.tool.as_str(), "strength": self.tools.smudge_strength,
               "smudgeMaxSize": MAX_SMUDGE_SIZE_PX})
    }

    /// `symmetry.set`: replaces the whole symmetry state. A stroke already in progress keeps the
    /// symmetry it began with.
    pub(super) fn symmetry_set(
        &mut self,
        mode: SymmetryMode,
        count: Option<u32>,
        mirror: Option<bool>,
        cx: Option<f64>,
        cy: Option<f64>,
    ) -> EResult<Receipt> {
        self.tools.symmetry = Symmetry::new(mode, count, mirror, cx, cy)?;
        Ok(self.receipt(self.symmetry_json()))
    }

    /// `guide.set`: replaces the guide. A stroke already in progress keeps its guide.
    pub(super) fn guide_set(
        &mut self,
        kind: GuideKind,
        x0: Option<f64>,
        y0: Option<f64>,
        x1: Option<f64>,
        y1: Option<f64>,
        angle: Option<f64>,
    ) -> EResult<Receipt> {
        self.tools.guide = Guide::new(kind, x0, y0, x1, y1, angle)?;
        Ok(self.receipt(self.tools.guide.to_json()))
    }

    /// `tool.set`: omitted fields keep their value; nothing changes when a field is invalid.
    pub(super) fn tool_set(&mut self, tool: Option<ToolKind>, strength: Option<f32>) -> EResult<Receipt> {
        if let Some(s) = strength
            && !(s.is_finite() && (0.0..=1.0).contains(&s))
        {
            return Err(VeskraError::invalid("strength out of range [0, 1]"));
        }
        if let Some(t) = tool {
            self.tools.tool = t;
        }
        if let Some(s) = strength {
            self.tools.smudge_strength = s;
        }
        Ok(self.receipt(self.tool_json()))
    }

    // ------------------------------------------------------------------ stroke pipeline

    pub(super) fn begin_stroke_with(&mut self, opts: BeginOpts) -> EResult<()> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "a stroke is already in progress"));
        }
        let d = self.doc()?.clone();
        let layer_id = d.active_layer.ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "no active layer"))?;
        let layer = d.layer(layer_id).ok_or_else(|| VeskraError::new(ErrorCode::NotFound, "active layer missing"))?;
        if d.is_effectively_locked(layer_id) {
            return Err(VeskraError::new(ErrorCode::LayerLocked, "the active layer is locked"));
        }
        let px = layer
            .pixels()
            .ok_or_else(|| VeskraError::new(ErrorCode::UnsupportedLayerFeature, "cannot paint on a group"))?;
        let is_shape = opts.shape.is_some();
        // Shapes always draw with the brush. Otherwise an explicit eraser request (flag, stylus
        // eraser end, stylus button) wins over the selected tool.
        let tool = if is_shape {
            ToolKind::Brush
        } else if opts.eraser || self.eraser_toggle {
            ToolKind::Eraser
        } else {
            opts.tool.unwrap_or(self.tools.tool)
        };
        let is_vector_layer = matches!(layer.kind, LayerKind::Vector(_));
        let alpha_locked = layer.props.alpha_locked;
        let selection = d.selection.clone();
        let selection_clipped = selection.is_some();
        let bounds = d.bounds();
        let replicas = self.tools.symmetry.replicas(canvas_center(bounds));
        let guide = if is_shape { None } else { self.tools.guide.activate() };
        let (base, fill_dirty, filled) = match opts.shape {
            Some(ShapeBegin { filled_base: Some(t), fill_dirty }) => (t, fill_dirty, true),
            _ => (px.tiles.clone(), RectI::EMPTY, false),
        };

        if tool == ToolKind::Smudge {
            if is_vector_layer {
                return Err(VeskraError::new(
                    ErrorCode::UnsupportedLayerFeature,
                    "the smudge tool needs a raster layer (vector layers keep only editable strokes)",
                ));
            }
            let params = SmudgeParams {
                size_px: self.brush.size_px,
                strength: self.tools.smudge_strength,
                // A ruler already constrains the path; the rope would only pull it off the guide.
                stabilization: if guide.is_some() { 0.0 } else { self.brush.stabilization },
            };
            let ctx = SmudgeContext { alpha_lock: alpha_locked, selection, clip: bounds };
            let smudge = SmudgeStroke::begin(base.clone(), params, ctx, 1 + replicas.len())?;
            self.live = Some(LiveStroke {
                layer: layer_id,
                ink: LiveInk::Smudge(Box::new(smudge)),
                live_tiles: base.clone(),
                base,
                predicted_tiles: None,
                predicted_rect: DirtySet::default(),
                preset: self.brush.clone(),
                color: self.color,
                seed: 0,
                mode: StrokeMode::Paint,
                alpha_locked,
                selection_clipped,
                opacity: 1.0,
                bounds: DirtySet::default(),
                replicas,
                guide,
                label: "Smudge",
                raster_edit: Some("smudge"),
                path_px: 0.0,
                last_pt: None,
            });
            return Ok(());
        }

        let eraser = tool == ToolKind::Eraser || self.brush.eraser;
        let mut preset = if eraser && !self.brush.eraser {
            veskra_brush::builtin_preset("eraser.hard").unwrap_or_else(|| self.brush.clone())
        } else {
            self.brush.clone()
        };
        if is_vector_layer && (eraser || !preset.vector_eligible) {
            return Err(VeskraError::new(
                ErrorCode::UnsupportedLayerFeature,
                "vector layers accept only vector-eligible brushes (use a raster layer for texture or pixel erasing)",
            ));
        }
        let mode = if eraser { StrokeMode::Erase } else { StrokeMode::Paint };
        // `Document::add_stroke` only accepts vector-representable strokes on vector layers; refuse
        // here, before anything is drawn, instead of discarding the finished stroke at commit.
        if is_vector_layer && (alpha_locked || selection_clipped) {
            return Err(VeskraError::new(
                ErrorCode::UnsupportedLayerFeature,
                "vector layers cannot be painted with alpha lock or through a selection (clear the selection or use a raster layer)",
            ));
        }
        if is_shape || guide.is_some() {
            // Shapes and ruled strokes follow their geometry exactly: the rope stabilizer would
            // round a rectangle's corners and cut inside an ellipse. Stored in the stroke's own
            // brush parameters, so the record stays a faithful description of what was drawn.
            preset.stabilization = 0.0;
        }
        let seed = (self.next_id() & u64::MAX as u128) as u64;
        let ctx = StrokeContext {
            mode: if eraser { BrushMode::Erase } else { BrushMode::Paint },
            alpha_lock: alpha_locked,
            selection,
            clip: bounds,
            ..Default::default()
        };
        // One renderer per stroke of the gesture. Replicas share the seed, so document-space
        // grain (and jitter) is the same field for all of them. Image tips / textures come from
        // the library or the built-ins (ADR-019).
        let assets = self.library.resolve(&preset);
        let parts: Vec<StrokeRenderer> = (0..=replicas.len())
            .map(|_| {
                StrokeRenderer::begin_resolved(
                    preset.clone(),
                    assets.clone(),
                    self.color,
                    seed,
                    base.clone(),
                    ctx.clone(),
                )
            })
            .collect();
        self.live = Some(LiveStroke {
            layer: layer_id,
            ink: LiveInk::Brush(parts),
            live_tiles: base.clone(),
            base,
            predicted_tiles: None,
            predicted_rect: DirtySet::default(),
            opacity: preset.opacity,
            preset,
            color: self.color,
            seed,
            mode,
            alpha_locked,
            selection_clipped,
            // A fill is visible from the first frame and must be refreshed (or restored on cancel).
            bounds: DirtySet::from(fill_dirty),
            replicas,
            guide,
            label: if is_shape { "Shape" } else { "Stroke" },
            raster_edit: filled.then_some("fill"),
            path_px: 0.0,
            last_pt: None,
        });
        Ok(())
    }

    pub(super) fn add_samples_impl(&mut self, samples: &[StrokeSample]) -> EResult<DirtySet> {
        let live =
            self.live.as_mut().ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "no stroke in progress"))?;
        let mut confirmed: Vec<StrokeSample> = samples.iter().copied().filter(|s| s.is_confirmed()).collect();
        for s in &confirmed {
            s.validate()?;
        }
        if let LiveInk::Brush(parts) = &live.ink
            && parts[0].confirmed_samples().len() + confirmed.len() > veskra_core::limits::MAX_STROKE_SAMPLES
        {
            return Err(VeskraError::alloc("stroke exceeds the sample limit"));
        }
        // Guide first (on a copy of its per-stroke state: nothing is kept if the batch is refused).
        let mut guide = live.guide;
        if let Some(g) = &mut guide {
            for s in &mut confirmed {
                *s = g.project_confirmed(s);
            }
        }
        // Work budget (see `MAX_GESTURE_PATH_PX`), measured on the path that will actually be
        // walked: an ellipse guide can turn a tiny pen movement into a long arc. A refused batch
        // leaves the stroke exactly as it was.
        let strokes = match &live.ink {
            LiveInk::Brush(parts) => parts.len(),
            LiveInk::Smudge(sm) => sm.head_count(),
        };
        if strokes > 1 || guide.is_some() || matches!(live.ink, LiveInk::Smudge(_)) {
            let (added, last) = path_length(live.last_pt, &confirmed);
            let total = live.path_px + added;
            if !total.is_finite() || total * strokes as f64 > MAX_GESTURE_PATH_PX {
                return Err(VeskraError::alloc("stroke is too long for the active tool, guide and symmetry"));
            }
            live.path_px = total;
            live.last_pt = last;
        }
        live.guide = guide;
        let mut dirty = DirtySet::default();
        match &mut live.ink {
            // The renderer skips (and counts) samples it cannot use, e.g. timestamps going
            // backwards or coordinates beyond its range; only accepted samples end up in a record.
            LiveInk::Brush(parts) if parts.len() == 1 => {
                let d = parts[0].add_samples(&confirmed);
                if !d.is_empty() {
                    parts[0].render_into(&mut live.live_tiles, d);
                }
                dirty.add(d);
            }
            LiveInk::Brush(parts) => {
                dirty.add(parts[0].add_samples(&confirmed));
                for (rep, part) in live.replicas.iter().zip(parts.iter_mut().skip(1)) {
                    let mapped: Vec<StrokeSample> = confirmed.iter().map(|s| rep.apply(s)).collect();
                    dirty.add(part.add_samples(&mapped));
                }
                let refs = layered(parts);
                for r in dirty.rects() {
                    composite_layered(&live.base, &refs, &mut live.live_tiles, *r);
                }
            }
            LiveInk::Smudge(sm) => {
                // Heads are interleaved per sample so the result is independent of batching.
                for s in &confirmed {
                    dirty.add(sm.add_sample(0, s));
                    for (i, rep) in live.replicas.iter().enumerate() {
                        dirty.add(sm.add_sample(i + 1, &rep.apply(s)));
                    }
                }
            }
        }
        live.bounds.extend(&dirty);
        // A new confirmed batch supersedes any prediction.
        if live.predicted_tiles.take().is_some() {
            dirty.extend(&std::mem::take(&mut live.predicted_rect));
        }
        Ok(dirty)
    }

    pub(super) fn predict_impl(&mut self, predicted: &[StrokeSample]) -> DirtySet {
        let Some(live) = self.live.as_mut() else { return DirtySet::default() };
        // The previous preview disappears either way.
        let mut dirty = std::mem::take(&mut live.predicted_rect);
        live.predicted_tiles = None;
        // Smudge has no prediction preview: its dabs change the pixels the next dab reads.
        let LiveInk::Brush(parts) = &live.ink else { return dirty };
        if predicted.is_empty() {
            return dirty;
        }
        let projected: Vec<StrokeSample> = match &live.guide {
            Some(g) => predicted.iter().map(|s| g.project_predicted(s)).collect(),
            None => predicted.to_vec(),
        };
        if parts.len() > 1 || live.guide.is_some() {
            let (len, _) = path_length(live.last_pt, &projected);
            if !len.is_finite() || len * parts.len() as f64 > MAX_PREDICTION_PATH_PX {
                return dirty;
            }
        }
        let mut tiles = live.live_tiles.clone();
        let mut now = DirtySet::default();
        if parts.len() == 1 {
            now.add(parts[0].prediction_preview(&projected, &mut tiles));
        } else {
            let mut branches = Vec::with_capacity(parts.len());
            branches.push(parts[0].predicted_coverage(&projected));
            for (rep, part) in live.replicas.iter().zip(parts.iter().skip(1)) {
                let mapped: Vec<StrokeSample> = projected.iter().map(|s| rep.apply(s)).collect();
                branches.push(part.predicted_coverage(&mapped));
            }
            for b in &branches {
                now.add(b.dirty());
            }
            let refs: Vec<LayeredPart<'_>> =
                parts.iter().zip(&branches).map(|(s, b)| LayeredPart { stroke: s, predicted: Some(b) }).collect();
            for r in now.rects() {
                composite_layered(&live.base, &refs, &mut tiles, *r);
            }
        }
        dirty.extend(&now);
        live.predicted_tiles = Some(tiles);
        live.predicted_rect = now;
        dirty
    }

    pub(super) fn commit_live_impl(&mut self, live: LiveStroke, dirty: &mut DirtySet) -> EResult<Value> {
        let LiveStroke {
            layer,
            ink,
            base,
            preset,
            color,
            seed,
            mode,
            alpha_locked,
            selection_clipped,
            opacity,
            label,
            raster_edit,
            ..
        } = live;
        let mut parts = match ink {
            LiveInk::Smudge(sm) => {
                let finished = sm.finish();
                dirty.add(finished.bounds);
                if finished.bounds.is_empty() {
                    // No pixel changed (blank or uniform area, strength 0): no undo step.
                    return Ok(json!({"committed": false}));
                }
                let before = self.doc()?.clone();
                let mut next = (*before).clone();
                next.pixels_mut_editable(layer)?.tiles = finished.tiles;
                next.mark_raster_edit(layer, raster_edit.unwrap_or("smudge"))?;
                let trimmed = self.history.record(label, before, &next);
                self.doc = Some(Arc::new(next));
                self.version += 1;
                return Ok(json!({"committed": true, "tool": "smudge", "strokeIds": [], "historyTrimmed": trimmed}));
            }
            LiveInk::Brush(parts) => parts,
        };
        // `finish` flushes the stabilizer and the final dab; the finished tiles (base ⊕ full
        // stroke) are authoritative, not the live preview tiles.
        let (tiles, finished): (TileGrid, Vec<FinishedStroke>) = if parts.len() == 1 {
            let f = parts.remove(0).finish();
            (f.tiles.clone(), vec![f])
        } else {
            let mut all = DirtySet::default();
            for p in &mut parts {
                p.flush();
                all.add(p.bounds());
            }
            let mut tiles = base.clone();
            let refs = layered(&parts);
            for r in all.rects() {
                composite_layered(&base, &refs, &mut tiles, *r);
            }
            (tiles, parts.into_iter().map(StrokeRenderer::finish_layered).collect())
        };
        for f in &finished {
            dirty.add(f.bounds);
        }
        if raster_edit.is_none() && finished.iter().all(|f| f.confirmed_samples.is_empty()) {
            return Ok(json!({"committed": false}));
        }
        let before = self.doc()?.clone();
        let mut next = (*before).clone();
        let profile = next.color.profile.clone();
        let brush_params = serde_json::to_string(&preset).map_err(internal)?;
        // Embed the images this brush uses so the strokes re-render without the library
        // (ADR-019). An image the session could not resolve stays a reference (the stroke was
        // drawn with the documented fallback, which is also what a re-render will use).
        let asset_refs = preset.asset_refs();
        for h in &asset_refs {
            if let Some(a) = self.library.find_asset(h) {
                next.insert_brush_asset(h, a.png_bytes().clone())?;
            }
        }
        next.pixels_mut_editable(layer)?.tiles = tiles;
        if let Some(reason) = raster_edit {
            next.mark_raster_edit(layer, reason)?;
        }
        // One real stroke record per replica that accepted samples, in compositing order.
        let vector_paint = preset.vector_eligible && mode == StrokeMode::Paint;
        let mut ids: Vec<String> = Vec::with_capacity(finished.len());
        let mut skipped = 0u32;
        for (i, f) in finished.into_iter().enumerate() {
            if f.confirmed_samples.is_empty() {
                continue;
            }
            // A replica that lies entirely outside the canvas *and* beyond the renderer's vector
            // reach changed no pixel and has no vector form. Recording it would only turn an
            // otherwise exact vector layer into a partially-raster one (and a vector layer could
            // not hold it at all), so it is dropped and counted. The user's own stroke (`i == 0`)
            // is never dropped.
            if i > 0 && vector_paint && f.geometry.is_none() && f.bounds.is_empty() {
                skipped += 1;
                continue;
            }
            let geometry = f.geometry.map(|g| StrokeGeometry {
                centerline: g.centerline,
                outline: g.outline,
                fit_error: g.fit_error,
            });
            let stroke_id = StrokeId::from_u128(self.next_id());
            let rec = StrokeRecord::new(
                stroke_id,
                layer,
                f.confirmed_samples,
                preset.id.clone(),
                preset.version,
                brush_params.clone(),
                color,
                seed,
                profile.clone(),
                StrokeComposition { mode, opacity, alpha_locked, selection_clipped },
                preset.vector_eligible && mode == StrokeMode::Paint,
                geometry,
                f.bounds,
            )?
            .with_brush_assets(asset_refs.clone())?;
            next.add_stroke(rec)?;
            ids.push(stroke_id.to_string());
        }
        let trimmed = self.history.record(label, before, &next);
        self.doc = Some(Arc::new(next));
        self.version += 1;
        Ok(json!({"committed": true, "strokeId": ids.first(), "strokeIds": ids, "historyTrimmed": trimmed,
                  "skippedReplicas": skipped}))
    }

    // ------------------------------------------------------------------ shapes

    /// `shape.*`: stroke `path` with the current brush as one undo step, optionally filling the
    /// interior first. Symmetry applies (outline and fill); guides and stabilization do not.
    pub(super) fn shape_draw(&mut self, path: ShapePath, fill: bool) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let d = self.doc()?.clone();
        let samples = crate::shape::samples(&path, FALLBACK_PRESSURE);
        let (perimeter, _) = path_length(None, &samples);
        if perimeter * self.tools.symmetry.stroke_count() as f64 > MAX_GESTURE_PATH_PX {
            return Err(VeskraError::alloc("shape is too large for the active symmetry"));
        }
        let mut shape = ShapeBegin { filled_base: None, fill_dirty: RectI::EMPTY };
        if fill {
            if !path.closed {
                return Err(VeskraError::invalid("fill needs a closed shape (rect, ellipse or a closed path)"));
            }
            if self.brush.eraser {
                return Err(VeskraError::invalid("fill is not available with an eraser brush"));
            }
            let layer = editable_raster_layer(&d)?;
            if d.is_effectively_locked(layer) {
                return Err(VeskraError::new(ErrorCode::LayerLocked, "the active layer is locked"));
            }
            let b = d.bounds();
            // Union of the shape and its symmetry replicas (max, so overlaps are filled once),
            // clipped by the selection like every fill.
            let mut region = veskra_raster::select::polygon(&path.points, b);
            for rep in self.tools.symmetry.replicas(canvas_center(b)) {
                let pts: Vec<PointF> = path.points.iter().map(|p| rep.apply_point(*p)).collect();
                region.combine(&veskra_raster::select::polygon(&pts, b), MaskOp::Add);
            }
            if let Some(sel) = &d.selection {
                region.combine(sel, MaskOp::Intersect);
            }
            let alpha_lock = d.layer(layer).is_some_and(|l| l.props.alpha_locked);
            let mut tiles = d.layer(layer).and_then(|l| l.pixels()).map(|p| p.tiles.clone()).unwrap_or_default();
            // Same compositing as `edit.fill_selection`.
            shape.fill_dirty = veskra_raster::fill::apply(&mut tiles, &region, self.color, alpha_lock);
            shape.filled_base = Some(tiles);
        }
        self.begin_stroke_with(BeginOpts { eraser: false, tool: None, shape: Some(shape) })?;
        if let Err(e) = self.add_samples_impl(&samples) {
            self.live = None;
            return Err(e);
        }
        let (res, dirty) = self.commit_stroke();
        let mut v = res?;
        let committed = v.get("committed").and_then(Value::as_bool) == Some(true);
        // Does the shape survive as editable paths on SVG/PDF export?
        let doc = self.doc()?;
        let ids: Vec<StrokeId> = v
            .get("strokeIds")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| s.as_str().and_then(|s| StrokeId::parse(s).ok())).collect())
            .unwrap_or_default();
        let strokes_vector = !ids.is_empty() && ids.iter().all(|i| doc.stroke(*i).is_some_and(|s| s.has_vector_form()));
        let fidelity = doc.active_layer.and_then(|l| doc.vector_fidelity(l));
        let vector = strokes_vector && !matches!(fidelity, Some(VectorFidelity::RasterAuthoritative { .. }));
        if let Value::Object(m) = &mut v {
            m.insert("filled".into(), json!(fill));
            m.insert("vector".into(), json!(vector));
            m.insert("samples".into(), json!(samples.len()));
        }
        let mut r = self.receipt(v);
        r.history_changed = committed;
        r.invalidated = dirty.into_rects();
        if fill {
            r.warnings.push(
                "The shape's fill is raster content: this layer now exports as raster, not as editable paths.".into(),
            );
        }
        if let Some(t) = r.result.get("historyTrimmed").and_then(Value::as_u64).filter(|t| *t > 0) {
            r.warnings
                .push(format!("Undo history was trimmed by {t} oldest step(s) to stay within the memory budget."));
        }
        Ok(r)
    }
}
