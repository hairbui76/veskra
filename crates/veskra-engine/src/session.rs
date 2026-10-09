//! One editing session: the current document snapshot, history, tool state, input arbitration
//! and the live (uncommitted) stroke.
//!
//! Threading: a `Session` is a single-owner actor (`&mut self`). The FFI layer serializes access
//! with a mutex; long exports run on an `Arc<Document>` snapshot outside that lock
//! (see [`crate::export_snapshot`]).

use crate::command::{Command, LayerKindSpec, PlacementSpec, SampleSpec, SelectionOp};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use veskra_brush::{BrushPreset, StrokeContext, StrokeMode as BrushMode, StrokeRenderer};
use veskra_core::export::{ExportFormat, ExportOptions};
use veskra_core::{
    BlendMode, BlendSpace, ColorRgba, DocumentId, ErrorCode, LayerId, PointF, RectF, RectI, SampleFlags, StrokeId,
    StrokeSample, VeskraError,
};
use veskra_doc::{
    DocColor, Document, GroupContent, History, HistoryConfig, LayerKind, LayerPatch, LayerProps, PixelContent,
    Placement, StrokeComposition, StrokeGeometry, StrokeMode, StrokeRecord, VectorFidelity,
};
use veskra_input::{InputAction, InputArbiter, InputConfig, RawPointerEvent, ViewTransform};
use veskra_raster::{MaskGrid, MaskOp, TileGrid};

pub type EResult<T> = Result<T, VeskraError>;

/// Fallback pressure for samples whose device reported none (documented; flag cleared).
pub const FALLBACK_PRESSURE: f32 = 0.5;

/// Engine-wide configuration for a session.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// `Some(seed)` makes all IDs and brush seeds deterministic (tests, CLI replay).
    pub deterministic_seed: Option<u64>,
    pub history: HistoryConfig,
    pub app_version: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            deterministic_seed: None,
            history: HistoryConfig::default(),
            app_version: concat!("veskra-engine ", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }
}

/// Result of a successfully executed command.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    pub result: Value,
    pub document_version: u64,
    /// Document-space regions whose display must be refreshed.
    pub invalidated: Vec<RectI>,
    pub warnings: Vec<String>,
    pub history_changed: bool,
    pub view_changed: bool,
}

/// Flags returned to the UI after an input batch (CONTRACTS §8).
pub mod flags {
    pub const DIRTY: i32 = 1;
    pub const VIEW: i32 = 2;
    pub const COMMITTED: i32 = 4;
    pub const HISTORY: i32 = 8;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputOutcome {
    pub flags: i32,
    pub dirty: Vec<RectI>,
    pub hover: Option<PointF>,
    pub warnings: Vec<String>,
}

/// Cloneable so a failed command can restore it: failures never consume IDs, so replaying a
/// command log without its failing lines yields identical IDs (deterministic replay).
#[derive(Clone)]
enum IdSource {
    Random,
    Deterministic(veskra_core::ids::DeterministicIds),
}

impl IdSource {
    fn next(&mut self) -> u128 {
        match self {
            IdSource::Random => LayerId::new_random().as_u128(),
            IdSource::Deterministic(d) => d.next_u128(),
        }
    }
}

struct LiveStroke {
    layer: LayerId,
    renderer: StrokeRenderer,
    /// Base ⊕ confirmed stroke, shown instead of the layer's tiles while the stroke is live.
    live_tiles: TileGrid,
    /// Live tiles plus predicted dabs (display only, STY-007).
    predicted_tiles: Option<TileGrid>,
    predicted_rect: RectI,
    // The confirmed samples are owned by `renderer` (`confirmed_samples()`): only samples the
    // renderer accepted (validated, monotonic time, in range) are rendered *and* recorded, so the
    // committed `StrokeRecord` always reproduces the pixels.
    preset: BrushPreset,
    color: ColorRgba,
    seed: u64,
    mode: StrokeMode,
    alpha_locked: bool,
    selection_clipped: bool,
    opacity: f32,
    bounds: RectI,
}

pub struct Session {
    cfg: EngineConfig,
    ids: IdSource,
    doc: Option<Arc<Document>>,
    history: History,
    version: u64,
    saved_version: Option<u64>,
    presets: Vec<BrushPreset>,
    brush: BrushPreset,
    color: ColorRgba,
    recent_colors: Vec<ColorRgba>,
    arbiter: InputArbiter,
    view: ViewTransform,
    live: Option<LiveStroke>,
    /// Input stroke rejected at begin (e.g. locked layer): swallow its samples until it ends.
    swallow_stroke: bool,
    eraser_toggle: bool,
    hover: Option<PointF>,
    doc_path: Option<PathBuf>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("doc", &self.doc.as_ref().map(|d| d.id))
            .field("version", &self.version)
            .field("live_stroke", &self.live.is_some())
            .finish()
    }
}

fn no_doc() -> VeskraError {
    VeskraError::new(ErrorCode::InvalidState, "no document is open")
}

fn parse_layer(id: &str) -> EResult<LayerId> {
    LayerId::parse(id)
}

impl Session {
    pub fn new(cfg: EngineConfig) -> Self {
        let ids = match cfg.deterministic_seed {
            Some(s) => IdSource::Deterministic(veskra_core::ids::DeterministicIds::new(s)),
            None => IdSource::Random,
        };
        let presets = veskra_brush::builtin_presets();
        let brush = presets.iter().find(|p| p.id == "pencil.smooth").cloned().unwrap_or_else(|| presets[0].clone());
        Self {
            history: History::new(cfg.history),
            cfg,
            ids,
            doc: None,
            version: 0,
            saved_version: None,
            presets,
            brush,
            color: ColorRgba::BLACK,
            recent_colors: Vec::new(),
            arbiter: InputArbiter::new(InputConfig::default()),
            view: ViewTransform::default(),
            live: None,
            swallow_stroke: false,
            eraser_toggle: false,
            hover: None,
            doc_path: None,
        }
    }

    // ------------------------------------------------------------------ accessors

    pub fn document(&self) -> Option<&Arc<Document>> {
        self.doc.as_ref()
    }
    /// Immutable snapshot for background jobs (export/save).
    pub fn snapshot(&self) -> EResult<Arc<Document>> {
        self.doc.clone().ok_or_else(no_doc)
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    /// Unsaved changes exist (SAV-002). `saved_version` is the version of the snapshot that was
    /// written, so edits made after it keep the document dirty. Versions are monotonic: undoing
    /// back to the saved content still reports dirty (conservative — never a false "saved").
    /// Recovered checkpoints are never marked saved.
    pub fn is_dirty(&self) -> bool {
        self.doc.is_some() && self.saved_version != Some(self.version)
    }
    pub fn view(&self) -> ViewTransform {
        self.view
    }
    pub fn has_live_stroke(&self) -> bool {
        self.live.is_some()
    }
    pub fn app_version(&self) -> &str {
        &self.cfg.app_version
    }

    fn doc(&self) -> EResult<&Arc<Document>> {
        self.doc.as_ref().ok_or_else(no_doc)
    }

    fn next_id(&mut self) -> u128 {
        self.ids.next()
    }

    fn receipt(&self, result: Value) -> Receipt {
        Receipt {
            result,
            document_version: self.version,
            invalidated: vec![],
            warnings: vec![],
            history_changed: false,
            view_changed: false,
        }
    }

    /// Apply a mutation to a clone of the current document as one atomic, undoable command.
    /// On error nothing changes (the clone is dropped).
    fn mutate<F>(&mut self, label: &str, f: F) -> EResult<(Value, RectI)>
    where
        F: FnOnce(&mut Document, &mut Self) -> EResult<(Value, RectI)>,
    {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let before = self.doc()?.clone();
        let mut next = (*before).clone();
        let (v, dirty) = f(&mut next, self)?;
        next.validate()?;
        let trimmed = self.history.record(label, before, &next);
        self.doc = Some(Arc::new(next));
        self.version += 1;
        let mut v = v;
        if trimmed > 0
            && let Value::Object(m) = &mut v
        {
            m.insert("historyTrimmed".into(), json!(trimmed));
        }
        Ok((v, dirty))
    }

    fn finish_mutation(&self, (v, dirty): (Value, RectI)) -> Receipt {
        let mut r = self.receipt(v);
        r.history_changed = true;
        if !dirty.is_empty() {
            r.invalidated.push(dirty);
        }
        if let Some(t) = r.result.get("historyTrimmed").and_then(|t| t.as_u64()) {
            r.warnings
                .push(format!("Undo history was trimmed by {t} oldest step(s) to stay within the memory budget."));
        }
        r
    }

    // ------------------------------------------------------------------ JSON boundary

    /// Execute a JSON command and return `{"ok": {...}, "version": N, "invalidated": [...], ...}`
    /// or `{"error": {"code", "key", "message", "recoverable"}}`. Never panics on bad input.
    pub fn execute_json(&mut self, json_cmd: &str) -> String {
        response_json(Command::from_json(json_cmd).and_then(|c| self.execute(c))).to_string()
    }

    // ------------------------------------------------------------------ commands

    /// Execute one typed command. On error nothing observable changes — document, version,
    /// history, live stroke and the ID stream are as before (IDs are restored on failure).
    pub fn execute(&mut self, cmd: Command) -> EResult<Receipt> {
        let ids = self.ids.clone();
        let r = self.execute_inner(cmd);
        if r.is_err() {
            self.ids = ids;
        }
        r
    }

    fn execute_inner(&mut self, cmd: Command) -> EResult<Receipt> {
        let label = cmd.label();
        match cmd {
            Command::DocumentCreate { width, height, dpi, background } => {
                self.create_document(width, height, dpi, background)
            }
            Command::DocumentInfo => Ok(self.receipt(self.info()?)),
            Command::DocumentSave { path } => self.save(Path::new(&path)),
            Command::DocumentOpen { path } => self.open(Path::new(&path)),
            Command::DocumentAutosave { dir } => self.autosave(Path::new(&dir)),
            Command::RecoveryList { dir } => {
                let store = veskra_format::recovery::AutosaveStore::new(PathBuf::from(dir), 3)?;
                let list: Vec<Value> = store
                    .list()?
                    .into_iter()
                    .map(|c| {
                        json!({"path": c.path.to_string_lossy(), "documentId": c.doc_id.to_string(), "error": c.error,
                               "seq": c.seq, "bytes": c.bytes, "valid": c.valid})
                    })
                    .collect();
                Ok(self.receipt(json!({ "checkpoints": list })))
            }
            Command::DocumentValidate { path } => {
                let bytes = std::fs::read(&path)
                    .map_err(|e| VeskraError::new(ErrorCode::Io, format!("cannot read file: {}", e.kind())))?;
                let loaded = veskra_format::decode(&bytes)?;
                Ok(self.receipt(json!({"valid": true, "bytes": bytes.len(), "documentId": loaded.doc.id.to_string(),
                                       "warnings": loaded.warnings.iter().map(|w| w.to_string()).collect::<Vec<_>>()})))
            }
            Command::DocumentRecover { dir } => {
                let store = veskra_format::recovery::AutosaveStore::new(PathBuf::from(dir), 3)?;
                let latest = store
                    .list()?
                    .into_iter()
                    .filter(|c| c.valid)
                    .max_by_key(|c| c.seq)
                    .ok_or_else(|| VeskraError::new(ErrorCode::NotFound, "no valid recovery checkpoint"))?;
                let r = self.open(&latest.path)?;
                self.saved_version = None;
                self.doc_path = None;
                Ok(r)
            }
            Command::RecoveryOpen { path } => {
                let r = self.open(Path::new(&path))?;
                // A recovered checkpoint is not the user's file: never mark it saved.
                self.saved_version = None;
                self.doc_path = None;
                Ok(r)
            }
            Command::LayerAdd { kind, name, placement } => self.layer_add(kind, name, placement, label),
            Command::LayerRemove { id } => {
                let id = parse_layer(&id)?;
                let r = self.mutate(label, |d, _| {
                    let removed = d.remove_layer(id)?;
                    if d.active_layer.is_none() {
                        d.active_layer =
                            d.paint_order().into_iter().rev().find(|l| d.layer(*l).is_some_and(|x| !x.is_group()));
                    }
                    Ok((json!({"removed": removed.iter().map(|x| x.to_string()).collect::<Vec<_>>()}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerDuplicate { id } => {
                let id = parse_layer(&id)?;
                let r = self.mutate(label, |d, s| {
                    let mut gen_id = || s.next_id();
                    let new_id = d.duplicate_layer(id, &mut gen_id)?;
                    // Like layer.add/layer.select: a group is never the active (paint) layer.
                    if d.layer(new_id).is_some_and(|l| !l.is_group()) {
                        d.active_layer = Some(new_id);
                    }
                    Ok((json!({"layerId": new_id.to_string()}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerMove { id, placement } => {
                let id = parse_layer(&id)?;
                let r = self.mutate(label, |d, _| {
                    let p = resolve_placement(d, placement)?;
                    d.move_layer(id, p)?;
                    Ok((json!({}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerSet { id, name, visible, opacity, blend, locked, alpha_locked, clip, pass_through } => {
                let id = parse_layer(&id)?;
                let blend = match blend {
                    Some(b) => Some(
                        BlendMode::parse(&b).ok_or_else(|| VeskraError::invalid(format!("unknown blend mode {b}")))?,
                    ),
                    None => None,
                };
                let patch = LayerPatch {
                    name,
                    visible,
                    opacity,
                    blend,
                    locked,
                    alpha_locked,
                    clip_to_below: clip,
                    pass_through,
                };
                let r = self.mutate(label, |d, _| {
                    d.apply_patch(id, &patch)?;
                    Ok((json!({}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerRename { id, name } => {
                let id = parse_layer(&id)?;
                let patch = LayerPatch { name: Some(name), ..Default::default() };
                let r = self.mutate(label, |d, _| {
                    d.apply_patch(id, &patch)?;
                    Ok((json!({}), RectI::EMPTY))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerSelect { id } => {
                let id = parse_layer(&id)?;
                let d = self.doc()?;
                let l = d.layer(id).ok_or_else(|| VeskraError::new(ErrorCode::NotFound, "layer not found"))?;
                if l.is_group() {
                    return Err(VeskraError::new(
                        ErrorCode::UnsupportedLayerFeature,
                        "select a raster or vector layer",
                    ));
                }
                // Selecting the active layer is UI state, not an undoable edit.
                let mut next = (**d).clone();
                next.active_layer = Some(id);
                self.doc = Some(Arc::new(next));
                Ok(self.receipt(json!({"activeLayer": id.to_string()})))
            }
            Command::LayerClear { id } => {
                let id = parse_layer(&id)?;
                let r = self.mutate(label, |d, _| {
                    d.clear_pixel_layer(id)?;
                    Ok((json!({}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerList => Ok(self.receipt(self.layer_list()?)),
            Command::BrushList => {
                let list: Vec<Value> = self
                    .presets
                    .iter()
                    .map(|p| json!({"id": p.id, "name": p.name, "vectorEligible": p.vector_eligible, "eraser": p.eraser}))
                    .collect();
                Ok(self.receipt(json!({"presets": list, "current": self.brush_json()})))
            }
            Command::BrushSelect { preset } => {
                let p =
                    self.presets.iter().find(|p| p.id == preset).cloned().ok_or_else(|| {
                        VeskraError::new(ErrorCode::NotFound, format!("unknown brush preset {preset}"))
                    })?;
                self.brush = p;
                Ok(self.receipt(json!({"current": self.brush_json()})))
            }
            Command::BrushSet { size, opacity, flow, stabilization } => {
                let check = |v: f32, lo: f32, hi: f32, what: &str| -> EResult<f32> {
                    if v.is_finite() && (lo..=hi).contains(&v) {
                        Ok(v)
                    } else {
                        Err(VeskraError::invalid(format!("{what} out of range [{lo}, {hi}]")))
                    }
                };
                if let Some(s) = size {
                    self.brush.size_px = check(s, 0.5, 1000.0, "size")?;
                }
                if let Some(o) = opacity {
                    self.brush.opacity = check(o, 0.0, 1.0, "opacity")?;
                }
                if let Some(f) = flow {
                    self.brush.flow = check(f, 0.0, 1.0, "flow")?;
                }
                if let Some(s) = stabilization {
                    self.brush.stabilization = check(s, 0.0, 1.0, "stabilization")?;
                }
                Ok(self.receipt(json!({"current": self.brush_json()})))
            }
            Command::ColorSet { hex } => {
                let c =
                    ColorRgba::from_hex(&hex).ok_or_else(|| VeskraError::invalid("expected #RRGGBB or #RRGGBBAA"))?;
                self.set_color(c);
                Ok(self.receipt(self.color_json()))
            }
            Command::ColorGet => Ok(self.receipt(self.color_json())),
            Command::ColorPick { x, y, layer } => {
                let c = self.pick(x, y, layer)?;
                if c.a > 0.0 {
                    self.set_color(c.with_alpha(1.0));
                }
                Ok(self.receipt(json!({"picked": c.to_hex(), "color": self.color.to_hex()})))
            }
            Command::StrokeBegin { eraser } => {
                self.begin_stroke(eraser)?;
                Ok(self.receipt(json!({})))
            }
            Command::StrokeSamples { samples } => {
                let samples: Vec<StrokeSample> = samples.iter().map(sample_from_spec).collect::<EResult<_>>()?;
                let dirty = self.add_samples(&samples)?;
                let mut r = self.receipt(json!({}));
                if !dirty.is_empty() {
                    r.invalidated.push(dirty);
                }
                Ok(r)
            }
            Command::StrokeCommit => {
                // On failure the error carries no rects: headless callers must refresh fully.
                let (v, dirty) = self.commit_stroke();
                let v = v?;
                let committed = v.get("committed").and_then(Value::as_bool) == Some(true);
                let mut r = self.receipt(v);
                r.history_changed = committed;
                if !dirty.is_empty() {
                    r.invalidated.push(dirty);
                }
                if let Some(t) = r.result.get("historyTrimmed").and_then(Value::as_u64).filter(|t| *t > 0) {
                    r.warnings.push(format!(
                        "Undo history was trimmed by {t} oldest step(s) to stay within the memory budget."
                    ));
                }
                Ok(r)
            }
            Command::StrokeCancel => {
                let dirty = self.cancel_stroke();
                let mut r = self.receipt(json!({}));
                if !dirty.is_empty() {
                    r.invalidated.push(dirty);
                }
                Ok(r)
            }
            Command::HistoryUndo => self.undo(),
            Command::HistoryRedo => self.redo(),
            Command::SelectionRect { x0, y0, x1, y1, op } => self
                .selection_shape(label, op, move |clip| veskra_raster::select::rect(RectF::new(x0, y0, x1, y1), clip)),
            Command::SelectionEllipse { x0, y0, x1, y1, op } => self.selection_shape(label, op, move |clip| {
                veskra_raster::select::ellipse(RectF::new(x0, y0, x1, y1), clip)
            }),
            Command::SelectionLasso { points, op } => {
                let pts: Vec<PointF> = points.iter().map(|p| PointF::new(p[0], p[1])).collect();
                self.selection_shape(label, op, move |clip| veskra_raster::select::polygon(&pts, clip))
            }
            Command::SelectionAll => {
                let r = self.mutate(label, |d, _| {
                    let b = d.bounds();
                    d.selection = Some(veskra_raster::select::rect(RectF::new(0.0, 0.0, b.x1 as f64, b.y1 as f64), b));
                    Ok((json!({}), b))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::SelectionClear => {
                let r = self.mutate(label, |d, _| {
                    d.selection = None;
                    Ok((json!({}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::SelectionInvert => {
                let r = self.mutate(label, |d, _| {
                    let b = d.bounds();
                    let mut m = d.selection.take().unwrap_or_else(MaskGrid::new_empty);
                    m.invert(b);
                    d.selection = Some(m);
                    Ok((json!({}), b))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::EditFill { x, y, tolerance, contiguous, sample_merged } => {
                self.edit_fill(label, x, y, tolerance, contiguous, sample_merged)
            }
            Command::EditFillSelection => {
                let color = self.color;
                let r = self.mutate(label, |d, _| {
                    let layer = editable_raster_layer(d)?;
                    let b = d.bounds();
                    let region = d.selection.clone().unwrap_or_else(|| {
                        veskra_raster::select::rect(RectF::new(0.0, 0.0, b.x1 as f64, b.y1 as f64), b)
                    });
                    let alpha_lock = d.layer(layer).is_some_and(|l| l.props.alpha_locked);
                    let px = d.pixels_mut_editable(layer)?;
                    let dirty = veskra_raster::fill::apply(&mut px.tiles, &region, color, alpha_lock);
                    d.mark_raster_edit(layer, "fill")?;
                    Ok((json!({}), dirty))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::EditDeleteSelection => {
                let r = self.mutate(label, |d, _| {
                    let layer = editable_raster_layer(d)?;
                    let sel = d
                        .selection
                        .clone()
                        .ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "nothing is selected"))?;
                    let px = d.pixels_mut_editable(layer)?;
                    let _removed = veskra_raster::ops::cut(&mut px.tiles, &sel);
                    d.mark_raster_edit(layer, "erase")?;
                    Ok((json!({}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::EditTransform { matrix } => {
                let m = veskra_core::Affine {
                    a: matrix[0],
                    b: matrix[1],
                    c: matrix[2],
                    d: matrix[3],
                    e: matrix[4],
                    f: matrix[5],
                };
                if !m.is_finite() || m.invert().is_none() {
                    return Err(VeskraError::invalid("transform matrix must be finite and invertible"));
                }
                let r = self.mutate(label, |d, _| {
                    let layer = editable_raster_layer(d)?;
                    let b = d.bounds();
                    let sel = d.selection.clone();
                    let px = d.pixels_mut_editable(layer)?;
                    match &sel {
                        Some(sel) => {
                            let moved = veskra_raster::ops::cut(&mut px.tiles, sel);
                            let t = veskra_raster::transform::try_transform_pixels(&moved, None, m, b)?;
                            veskra_raster::ops::paste_over(&mut px.tiles, &t);
                        }
                        None => {
                            px.tiles = veskra_raster::transform::try_transform_pixels(&px.tiles, None, m, b)?;
                        }
                    }
                    if let Some(sel) = sel {
                        d.selection = Some(veskra_raster::transform::try_transform_mask(&sel, m, b)?);
                    }
                    d.mark_raster_edit(layer, "transform")?;
                    Ok((json!({}), b))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::ViewSize { width, height } => {
                if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
                    return Err(VeskraError::invalid("view size must be positive"));
                }
                let first = self.view.view_w <= 0.0;
                self.view = self.view.resized(width, height);
                if first && let Some(d) = &self.doc {
                    self.view = ViewTransform::fit(d.size(), width, height);
                }
                let mut r = self.receipt(self.view_json());
                r.view_changed = true;
                Ok(r)
            }
            Command::ViewFit => {
                let d = self.doc()?;
                self.view = ViewTransform::fit(d.size(), self.view.view_w.max(1.0), self.view.view_h.max(1.0));
                let mut r = self.receipt(self.view_json());
                r.view_changed = true;
                Ok(r)
            }
            Command::ViewSet { zoom, rotation, pan_x, pan_y, mirror } => {
                let mut v = self.view;
                if let Some(z) = zoom {
                    v.zoom = z;
                }
                if let Some(rot) = rotation {
                    v.rotation = rot;
                }
                if let Some(x) = pan_x {
                    v.pan.x = x;
                }
                if let Some(y) = pan_y {
                    v.pan.y = y;
                }
                if let Some(m) = mirror {
                    v.mirror = m;
                }
                self.view = v.sanitized();
                let mut r = self.receipt(self.view_json());
                r.view_changed = true;
                Ok(r)
            }
            Command::ViewMirror { enabled } => {
                let c = PointF::new(self.view.view_w / 2.0, self.view.view_h / 2.0);
                self.view = self.view.mirrored_about(c, enabled);
                let mut r = self.receipt(self.view_json());
                r.view_changed = true;
                Ok(r)
            }
            Command::InputConfig { finger_draws, stylus_button } => {
                let mut cfg = self.arbiter.config().clone();
                if let Some(f) = finger_draws {
                    cfg.finger_draws = f;
                }
                if let Some(b) = stylus_button {
                    cfg.stylus_button_action = match b.as_str() {
                        "none" => veskra_input::ButtonAction::None,
                        "eyedropper" => veskra_input::ButtonAction::Eyedropper,
                        "toggle_eraser" => veskra_input::ButtonAction::ToggleEraser,
                        "undo" => veskra_input::ButtonAction::Undo,
                        other => return Err(VeskraError::invalid(format!("unknown stylus button action {other}"))),
                    };
                }
                self.arbiter.set_config(cfg);
                Ok(self.receipt(json!({})))
            }
            Command::InputReset => {
                let actions = self.arbiter.reset();
                let out = self.apply_actions(actions);
                let mut r = self.receipt(json!({"flags": out.flags}));
                r.invalidated = out.dirty;
                r.warnings = out.warnings;
                r.history_changed = out.flags & flags::HISTORY != 0;
                Ok(r)
            }
            Command::ExportPreflight { format, options } => {
                let d = self.doc()?;
                let pf = veskra_io::exporter_for(format).preflight(d, &options);
                Ok(self.receipt(serde_json::to_value(pf).map_err(internal)?))
            }
            Command::ExportRun { format, path, options } => {
                let snap = self.snapshot()?;
                let report =
                    crate::export_snapshot(&snap, format, &options, Path::new(&path), &veskra_io::CancelToken::new())?;
                Ok(self.receipt(serde_json::to_value(report).map_err(internal)?))
            }
        }
    }

    // ------------------------------------------------------------------ document lifecycle

    fn create_document(&mut self, width: u32, height: u32, dpi: f32, background: Option<String>) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let srgb = veskra_color::icc::IccProfile::srgb();
        let color = DocColor {
            profile: srgb.id(),
            icc_bytes: Arc::from(srgb.bytes()),
            assumed: false,
            blend_space: BlendSpace::GammaEncoded,
        };
        let mut doc = Document::new(DocumentId::from_u128(self.next_id()), width, height, dpi, color)?;
        let bg_id = LayerId::from_u128(self.next_id());
        let mut bg_px = PixelContent::empty();
        if let Some(hex) = &background {
            let c = ColorRgba::from_hex(hex).ok_or_else(|| VeskraError::invalid("background must be #RRGGBB[AA]"))?;
            let b = doc.bounds();
            let full = veskra_raster::select::rect(RectF::new(0.0, 0.0, b.x1 as f64, b.y1 as f64), b);
            veskra_raster::fill::apply(&mut bg_px.tiles, &full, c, false);
            bg_px.raster_edits.push("background_fill".into());
        }
        doc.add_layer(bg_id, LayerProps::named("Background"), LayerKind::Raster(bg_px), Placement::Top)?;
        let ink = LayerId::from_u128(self.next_id());
        doc.add_layer(ink, LayerProps::named("Layer 1"), LayerKind::Raster(PixelContent::empty()), Placement::Top)?;
        doc.active_layer = Some(ink);
        doc.validate()?;
        let size = doc.size();
        self.doc = Some(Arc::new(doc));
        self.history.clear();
        self.version += 1;
        self.saved_version = None;
        self.doc_path = None;
        if self.view.view_w > 0.0 {
            self.view = ViewTransform::fit(size, self.view.view_w, self.view.view_h);
        }
        let mut r = self.receipt(self.info()?);
        r.invalidated.push(RectI::from_size(width, height));
        r.history_changed = true;
        r.view_changed = true;
        Ok(r)
    }

    fn save_meta(&self) -> veskra_format::SaveMeta {
        veskra_format::SaveMeta { app_version: self.cfg.app_version.clone(), created: String::from("unspecified") }
    }

    fn preview_png(doc: &Document) -> Option<Vec<u8>> {
        let (w, h, px) = veskra_compose::thumbnail_rgba8(doc, 256);
        veskra_io::png::encode_rgba8_srgb(w, h, &px).ok()
    }

    fn save(&mut self, path: &Path) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let snap = self.snapshot()?;
        let version = self.version;
        let preview = Self::preview_png(&snap);
        let report = veskra_format::save_atomic(&snap, path, &self.save_meta(), preview.as_deref())?;
        // Only after validated rename: mark this snapshot version saved (SAV-002).
        self.saved_version = Some(version);
        self.doc_path = Some(path.to_path_buf());
        Ok(self.receipt(json!({"bytes": report.bytes, "savedVersion": version, "sha256": report.sha256})))
    }

    fn autosave(&mut self, dir: &Path) -> EResult<Receipt> {
        let snap = self.snapshot()?;
        std::fs::create_dir_all(dir)
            .map_err(|e| VeskraError::new(ErrorCode::Io, format!("autosave dir: {}", e.kind())))?;
        let store = veskra_format::recovery::AutosaveStore::new(dir.to_path_buf(), 3)?;
        let info = store.checkpoint(&snap, &self.save_meta())?;
        Ok(self.receipt(json!({"path": info.path.to_string_lossy(), "seq": info.seq, "bytes": info.bytes})))
    }

    fn open(&mut self, path: &Path) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let loaded = veskra_format::load(path)?;
        let mut doc = loaded.doc;
        if doc.active_layer.is_none() {
            doc.active_layer =
                doc.paint_order().into_iter().rev().find(|l| doc.layer(*l).is_some_and(|x| !x.is_group()));
        }
        let size = doc.size();
        self.doc = Some(Arc::new(doc));
        self.history.clear();
        self.version += 1;
        self.saved_version = Some(self.version);
        self.doc_path = Some(path.to_path_buf());
        if self.view.view_w > 0.0 {
            self.view = ViewTransform::fit(size, self.view.view_w, self.view.view_h);
        }
        let mut r = self.receipt(self.info()?);
        r.invalidated.push(RectI::from_size(size.width, size.height));
        r.warnings = loaded.warnings.iter().map(|w| w.to_string()).collect();
        r.history_changed = true;
        r.view_changed = true;
        Ok(r)
    }

    fn info(&self) -> EResult<Value> {
        let d = self.doc()?;
        let st = self.history.stats();
        Ok(json!({
            "documentId": d.id.to_string(),
            "width": d.width(), "height": d.height(), "dpi": d.dpi,
            "profile": d.color.profile.semantic_name, "profileAssumed": d.color.assumed,
            "blendSpace": match d.color.blend_space { BlendSpace::GammaEncoded => "gamma_encoded", BlendSpace::Linear => "linear" },
            "version": self.version, "savedVersion": self.saved_version, "dirty": self.is_dirty(),
            "layerCount": d.layer_count(), "strokeCount": d.stroke_count(),
            "activeLayer": d.active_layer.map(|l| l.to_string()),
            "hasSelection": d.selection.is_some(),
            "canUndo": self.history.can_undo(), "canRedo": self.history.can_redo(),
            "undoLabel": self.history.undo_label(), "redoLabel": self.history.redo_label(),
            "historyBytes": st.bytes, "historyTrimmed": st.trimmed_entries,
        }))
    }

    fn layer_list(&self) -> EResult<Value> {
        let d = self.doc()?;
        let mut out = Vec::new();
        for id in d.paint_order() {
            let Some(l) = d.layer(id) else { continue };
            let fidelity = d.vector_fidelity(id).map(|f| match f {
                VectorFidelity::Exact => "exact".to_string(),
                VectorFidelity::PartiallyVector => "partially_vector".to_string(),
                VectorFidelity::RasterAuthoritative { .. } => "raster".to_string(),
            });
            out.push(json!({
                "id": id.to_string(), "parent": l.parent.map(|p| p.to_string()), "depth": d.depth(id),
                "kind": match l.kind { LayerKind::Raster(_) => "raster", LayerKind::Vector(_) => "vector", LayerKind::Group(_) => "group" },
                "name": l.props.name, "visible": l.props.visible, "opacity": l.props.opacity,
                "blend": l.props.blend.as_str(), "locked": l.props.locked, "alphaLocked": l.props.alpha_locked,
                "clip": l.props.clip_to_below,
                "passThrough": l.group().map(|g| g.pass_through),
                "strokes": l.pixels().map(|p| p.strokes.len()),
                "hasMask": l.pixels().map(|p| p.mask.is_some()),
                "vectorFidelity": fidelity,
                "active": d.active_layer == Some(id),
            }));
        }
        // UI lists layers top-first.
        out.reverse();
        Ok(json!({ "layers": out }))
    }

    fn layer_add(
        &mut self,
        kind: LayerKindSpec,
        name: Option<String>,
        placement: PlacementSpec,
        label: &str,
    ) -> EResult<Receipt> {
        let id = LayerId::from_u128(self.next_id());
        let r = self.mutate(label, |d, _| {
            let n = d.layer_count() + 1;
            let (k, default_name) = match kind {
                LayerKindSpec::Raster => (LayerKind::Raster(PixelContent::empty()), format!("Layer {n}")),
                LayerKindSpec::Vector => (LayerKind::Vector(PixelContent::empty()), format!("Vector {n}")),
                LayerKindSpec::Group => (
                    LayerKind::Group(GroupContent { children: vec![], pass_through: false, collapsed: false }),
                    format!("Group {n}"),
                ),
            };
            let p = resolve_placement(d, placement)?;
            d.add_layer(id, LayerProps::named(name.unwrap_or(default_name)), k, p)?;
            if kind != LayerKindSpec::Group {
                d.active_layer = Some(id);
            }
            // Not a no-op visually: an empty layer inserted under a clipping layer (`clip`)
            // becomes its clip base and hides it.
            Ok((json!({"layerId": id.to_string()}), d.bounds()))
        })?;
        Ok(self.finish_mutation(r))
    }

    fn selection_shape<F>(&mut self, label: &str, op: SelectionOp, shape: F) -> EResult<Receipt>
    where
        F: FnOnce(RectI) -> MaskGrid,
    {
        let r = self.mutate(label, |d, _| {
            let b = d.bounds();
            let new = shape(b);
            let combined = match (op, d.selection.take()) {
                (SelectionOp::Replace, _) | (_, None)
                    if op != SelectionOp::Intersect && op != SelectionOp::Subtract =>
                {
                    new
                }
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
                (_, None) => new,
            };
            d.selection = if combined.is_empty() { None } else { Some(combined) };
            Ok((json!({"empty": d.selection.is_none()}), b))
        })?;
        Ok(self.finish_mutation(r))
    }

    fn edit_fill(
        &mut self,
        label: &str,
        x: f32,
        y: f32,
        tolerance: f32,
        contiguous: bool,
        sample_merged: bool,
    ) -> EResult<Receipt> {
        if !(tolerance.is_finite() && (0.0..=1.0).contains(&tolerance)) || !x.is_finite() || !y.is_finite() {
            return Err(VeskraError::invalid("fill tolerance must be in [0,1] and point finite"));
        }
        let color = self.color;
        let r = self.mutate(label, |d, _| {
            let layer = editable_raster_layer(d)?;
            let b = d.bounds();
            let (sx, sy) = (x.floor() as i32, y.floor() as i32);
            if !b.contains(sx, sy) {
                return Err(VeskraError::invalid("fill point outside the canvas"));
            }
            let reference = if sample_merged {
                veskra_compose::composite_grid(d, &veskra_compose::ComposeOptions::default())
            } else {
                d.layer(layer).and_then(|l| l.pixels()).map(|p| p.tiles.clone()).unwrap_or_default()
            };
            let opts = veskra_raster::fill::FillOptions { tolerance, contiguous, antialias: true };
            let region = veskra_raster::fill::region(&reference, (sx, sy), &opts, b, d.selection.as_ref())?;
            let alpha_lock = d.layer(layer).is_some_and(|l| l.props.alpha_locked);
            let px = d.pixels_mut_editable(layer)?;
            let dirty = veskra_raster::fill::apply(&mut px.tiles, &region, color, alpha_lock);
            d.mark_raster_edit(layer, "fill")?;
            Ok((json!({}), dirty))
        })?;
        Ok(self.finish_mutation(r))
    }

    // ------------------------------------------------------------------ color / brush

    fn set_color(&mut self, c: ColorRgba) {
        self.color = c;
        self.recent_colors.retain(|r| r.to_hex() != c.to_hex());
        self.recent_colors.insert(0, c);
        self.recent_colors.truncate(12);
    }

    fn color_json(&self) -> Value {
        json!({"color": self.color.to_hex(), "recent": self.recent_colors.iter().map(|c| c.to_hex()).collect::<Vec<_>>(), "profile": "sRGB"})
    }

    fn brush_json(&self) -> Value {
        json!({"id": self.brush.id, "name": self.brush.name, "size": self.brush.size_px, "opacity": self.brush.opacity,
               "flow": self.brush.flow, "stabilization": self.brush.stabilization, "vectorEligible": self.brush.vector_eligible,
               "eraser": self.brush.eraser})
    }

    fn view_json(&self) -> Value {
        let m = self.view.doc_to_view();
        json!({"zoom": self.view.zoom, "rotation": self.view.rotation, "panX": self.view.pan.x, "panY": self.view.pan.y,
               "mirror": self.view.mirror, "matrix": [m.a, m.b, m.c, m.d, m.e, m.f]})
    }

    fn pick(&self, x: f32, y: f32, layer_only: bool) -> EResult<ColorRgba> {
        let d = self.doc()?;
        if !x.is_finite() || !y.is_finite() {
            return Err(VeskraError::invalid("pick point not finite"));
        }
        let (xi, yi) = (x.floor() as i32, y.floor() as i32);
        if !d.bounds().contains(xi, yi) {
            return Ok(ColorRgba::TRANSPARENT);
        }
        if layer_only {
            let l = d.active_layer.ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "no active layer"))?;
            let p = d.layer(l).and_then(|l| l.pixels()).map(|p| p.tiles.get_pixel(xi, yi)).unwrap_or([0; 4]);
            let mut out = [0u8; 4];
            veskra_color::convert::premul16_to_straight8(&p, &mut out);
            Ok(ColorRgba::from_rgba8(out[0], out[1], out[2], out[3]))
        } else {
            Ok(veskra_compose::sample_composite(d, xi, yi))
        }
    }

    // ------------------------------------------------------------------ strokes

    fn begin_stroke(&mut self, eraser_requested: bool) -> EResult<()> {
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
        let eraser = eraser_requested || self.eraser_toggle || self.brush.eraser;
        let preset = if eraser && !self.brush.eraser {
            self.presets.iter().find(|p| p.id == "eraser.hard").cloned().unwrap_or_else(|| self.brush.clone())
        } else {
            self.brush.clone()
        };
        let is_vector_layer = matches!(layer.kind, LayerKind::Vector(_));
        if is_vector_layer && (eraser || !preset.vector_eligible) {
            return Err(VeskraError::new(
                ErrorCode::UnsupportedLayerFeature,
                "vector layers accept only vector-eligible brushes (use a raster layer for texture or pixel erasing)",
            ));
        }
        let mode = if eraser { StrokeMode::Erase } else { StrokeMode::Paint };
        let alpha_locked = layer.props.alpha_locked;
        let selection = d.selection.clone();
        let selection_clipped = selection.is_some();
        // `Document::add_stroke` only accepts vector-representable strokes on vector layers; refuse
        // here, before anything is drawn, instead of discarding the finished stroke at commit.
        if is_vector_layer && (alpha_locked || selection_clipped) {
            return Err(VeskraError::new(
                ErrorCode::UnsupportedLayerFeature,
                "vector layers cannot be painted with alpha lock or through a selection (clear the selection or use a raster layer)",
            ));
        }
        let seed = (self.next_id() & u64::MAX as u128) as u64;
        let ctx = StrokeContext {
            mode: if eraser { BrushMode::Erase } else { BrushMode::Paint },
            alpha_lock: alpha_locked,
            selection,
            clip: d.bounds(),
            ..Default::default()
        };
        let base = px.tiles.clone();
        let renderer = StrokeRenderer::begin(preset.clone(), self.color, seed, base.clone(), ctx);
        self.live = Some(LiveStroke {
            layer: layer_id,
            renderer,
            live_tiles: base,
            predicted_tiles: None,
            predicted_rect: RectI::EMPTY,
            opacity: preset.opacity,
            preset,
            color: self.color,
            seed,
            mode,
            alpha_locked,
            selection_clipped,
            bounds: RectI::EMPTY,
        });
        Ok(())
    }

    fn add_samples(&mut self, samples: &[StrokeSample]) -> EResult<RectI> {
        let live =
            self.live.as_mut().ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "no stroke in progress"))?;
        let confirmed: Vec<StrokeSample> = samples.iter().copied().filter(|s| s.is_confirmed()).collect();
        for s in &confirmed {
            s.validate()?;
        }
        if live.renderer.confirmed_samples().len() + confirmed.len() > veskra_core::limits::MAX_STROKE_SAMPLES {
            return Err(VeskraError::alloc("stroke exceeds the sample limit"));
        }
        // The renderer skips (and counts) samples it cannot use, e.g. timestamps going backwards
        // or coordinates beyond its range; only accepted samples end up in the record.
        let mut dirty = live.renderer.add_samples(&confirmed);
        if !dirty.is_empty() {
            live.renderer.render_into(&mut live.live_tiles, dirty);
        }
        live.bounds = live.bounds.union(&dirty);
        // A new confirmed batch supersedes any prediction.
        if live.predicted_tiles.take().is_some() {
            dirty = dirty.union(&live.predicted_rect);
            live.predicted_rect = RectI::EMPTY;
        }
        Ok(dirty)
    }

    fn predict(&mut self, predicted: &[StrokeSample]) -> RectI {
        let Some(live) = self.live.as_mut() else { return RectI::EMPTY };
        let old = live.predicted_rect;
        if predicted.is_empty() {
            live.predicted_tiles = None;
            live.predicted_rect = RectI::EMPTY;
            return old;
        }
        let mut tiles = live.live_tiles.clone();
        let r = live.renderer.prediction_preview(predicted, &mut tiles);
        live.predicted_tiles = Some(tiles);
        live.predicted_rect = r;
        old.union(&r)
    }

    /// End the live stroke and commit it as exactly one undo step (`{"committed": false}` and no
    /// history entry when no sample was accepted). The returned rect must be refreshed whether or
    /// not the commit succeeded: the live preview is gone either way. A failed commit changes
    /// neither document, version, history nor the ID stream. Failure is not expected in practice:
    /// samples are pre-filtered by the renderer, vector-layer policy is enforced at begin, and
    /// every document mutation is refused while a stroke is live.
    fn commit_stroke(&mut self) -> (EResult<Value>, RectI) {
        let Some(live) = self.live.take() else {
            return (Err(VeskraError::new(ErrorCode::InvalidState, "no stroke in progress")), RectI::EMPTY);
        };
        let mut dirty = live.bounds.union(&live.predicted_rect);
        let ids = self.ids.clone();
        let r = self.commit_live(live, &mut dirty);
        if r.is_err() {
            self.ids = ids;
        }
        (r, dirty)
    }

    fn commit_live(&mut self, live: LiveStroke, dirty: &mut RectI) -> EResult<Value> {
        let LiveStroke { layer, renderer, preset, color, seed, mode, alpha_locked, selection_clipped, opacity, .. } =
            live;
        // `finish` flushes the stabilizer and the final dab; its tiles (base ⊕ full stroke) are
        // authoritative, not the live preview tiles.
        let finished = renderer.finish();
        *dirty = dirty.union(&finished.bounds);
        if finished.confirmed_samples.is_empty() {
            return Ok(json!({"committed": false}));
        }
        let before = self.doc()?.clone();
        let mut next = (*before).clone();
        let profile = next.color.profile.clone();
        let geometry = finished.geometry.map(|g| StrokeGeometry {
            centerline: g.centerline,
            outline: g.outline,
            fit_error: g.fit_error,
        });
        let stroke_id = StrokeId::from_u128(self.next_id());
        let rec = StrokeRecord::new(
            stroke_id,
            layer,
            finished.confirmed_samples,
            preset.id.clone(),
            preset.version,
            serde_json::to_string(&preset).map_err(internal)?,
            color,
            seed,
            profile,
            StrokeComposition { mode, opacity, alpha_locked, selection_clipped },
            preset.vector_eligible && mode == StrokeMode::Paint,
            geometry,
            finished.bounds,
        )?;
        next.pixels_mut_editable(layer)?.tiles = finished.tiles;
        next.add_stroke(rec)?;
        let trimmed = self.history.record("Stroke", before, &next);
        self.doc = Some(Arc::new(next));
        self.version += 1;
        Ok(json!({"committed": true, "strokeId": stroke_id.to_string(), "historyTrimmed": trimmed}))
    }

    fn cancel_stroke(&mut self) -> RectI {
        match self.live.take() {
            Some(l) => l.bounds.union(&l.predicted_rect),
            None => RectI::EMPTY,
        }
    }

    fn undo(&mut self) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let cur = self.doc()?.clone();
        let (label, prev) =
            self.history.undo(cur).ok_or_else(|| VeskraError::new(ErrorCode::NothingToUndo, "nothing to undo"))?;
        let b = prev.bounds();
        self.doc = Some(prev);
        self.version += 1;
        let mut r = self.receipt(json!({"undone": label}));
        r.invalidated.push(b);
        r.history_changed = true;
        Ok(r)
    }

    fn redo(&mut self) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let cur = self.doc()?.clone();
        let (label, next) =
            self.history.redo(cur).ok_or_else(|| VeskraError::new(ErrorCode::NothingToRedo, "nothing to redo"))?;
        let b = next.bounds();
        self.doc = Some(next);
        self.version += 1;
        let mut r = self.receipt(json!({"redone": label}));
        r.invalidated.push(b);
        r.history_changed = true;
        Ok(r)
    }

    // ------------------------------------------------------------------ input pipeline

    /// Process a batch of raw pointer events (VIEW coordinates) through the Rust arbiter.
    pub fn input_batch(&mut self, events: &[RawPointerEvent]) -> InputOutcome {
        let mut actions = Vec::new();
        self.arbiter.handle_batch_into(events, &self.view.clone(), &mut actions);
        self.apply_actions(actions)
    }

    fn apply_actions(&mut self, actions: Vec<InputAction>) -> InputOutcome {
        let mut out = InputOutcome::default();
        let mut dirty = RectI::EMPTY;
        for a in actions {
            match a {
                InputAction::StrokeBegin { eraser, .. } => {
                    self.swallow_stroke = false;
                    if self.doc.is_none() {
                        self.swallow_stroke = true;
                        continue;
                    }
                    if let Err(e) = self.begin_stroke(eraser) {
                        self.swallow_stroke = true;
                        out.warnings.push(e.message);
                    }
                }
                InputAction::StrokeSamples(s) => {
                    if self.swallow_stroke {
                        continue;
                    }
                    match self.add_samples(&s) {
                        Ok(r) => dirty = dirty.union(&r),
                        Err(e) => out.warnings.push(e.message),
                    }
                }
                InputAction::StrokePrediction(p) => {
                    if !self.swallow_stroke {
                        dirty = dirty.union(&self.predict(&p));
                    }
                }
                InputAction::StrokeCommit => {
                    if self.swallow_stroke {
                        self.swallow_stroke = false;
                        continue;
                    }
                    let (res, r) = self.commit_stroke();
                    // Refresh even on failure: the live preview is gone.
                    dirty = dirty.union(&r);
                    match res {
                        Ok(v) => {
                            if v.get("committed").and_then(|c| c.as_bool()) == Some(true) {
                                out.flags |= flags::COMMITTED | flags::HISTORY;
                            }
                        }
                        Err(e) => out.warnings.push(e.message),
                    }
                }
                InputAction::StrokeCancel => {
                    // A swallowed input stroke never owned `self.live` (which may be a headless
                    // stroke that made this one's begin fail): only end the swallow.
                    if std::mem::take(&mut self.swallow_stroke) {
                        continue;
                    }
                    dirty = dirty.union(&self.cancel_stroke());
                }
                InputAction::Hover(p) => {
                    out.hover = p;
                    // Latest hover (or exit) is what an eyedropper press in this batch samples.
                    self.hover = p;
                }
                InputAction::ViewChanged(v) => {
                    self.view = v;
                    out.flags |= flags::VIEW;
                }
                InputAction::GestureEnd => {}
                InputAction::TwoFingerTap => {
                    if let Ok(r) = self.undo() {
                        dirty = dirty.union(r.invalidated.first().unwrap_or(&RectI::EMPTY));
                        out.flags |= flags::HISTORY;
                    }
                }
                InputAction::ThreeFingerTap => {
                    if let Ok(r) = self.redo() {
                        dirty = dirty.union(r.invalidated.first().unwrap_or(&RectI::EMPTY));
                        out.flags |= flags::HISTORY;
                    }
                }
                InputAction::StylusButton { pressed, action } => match action {
                    veskra_input::ButtonAction::ToggleEraser => self.eraser_toggle = pressed,
                    veskra_input::ButtonAction::Undo if pressed => {
                        if let Ok(r) = self.undo() {
                            dirty = dirty.union(r.invalidated.first().unwrap_or(&RectI::EMPTY));
                            out.flags |= flags::HISTORY;
                        }
                    }
                    veskra_input::ButtonAction::Eyedropper if pressed => {
                        if let Some(h) = self.hover
                            && let Ok(c) = self.pick(h.x as f32, h.y as f32, false)
                            && c.a > 0.0
                        {
                            self.set_color(c.with_alpha(1.0));
                        }
                    }
                    _ => {}
                },
            }
        }
        if let Some(d) = &self.doc {
            dirty = dirty.intersect(&d.bounds());
        }
        if !dirty.is_empty() {
            out.flags |= flags::DIRTY;
            out.dirty.push(dirty);
        }
        out
    }

    // ------------------------------------------------------------------ display

    /// Render the current composite (including the live stroke and any prediction) as RGBA8
    /// premultiplied sRGB for the display (`out.len() == rect.w * rect.h * 4`).
    pub fn render_display(&self, rect: RectI, out: &mut [u8]) -> EResult<()> {
        let d = self.doc()?;
        let overrides_storage;
        let overrides: &[(LayerId, &TileGrid)] = match &self.live {
            Some(l) => {
                overrides_storage = [(l.layer, l.predicted_tiles.as_ref().unwrap_or(&l.live_tiles))];
                &overrides_storage
            }
            None => &[],
        };
        let opts = veskra_compose::ComposeOptions { overrides, ..Default::default() };
        veskra_compose::render_display_rgba8(d, rect, &opts, out)
    }
}

fn editable_raster_layer(d: &Document) -> EResult<LayerId> {
    let id = d.active_layer.ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "no active layer"))?;
    match d.layer(id).map(|l| &l.kind) {
        Some(LayerKind::Raster(_)) => Ok(id),
        Some(LayerKind::Vector(_)) => Err(VeskraError::new(
            ErrorCode::UnsupportedLayerFeature,
            "fills, selection edits and transforms need a raster layer (vector layers keep only editable strokes)",
        )),
        Some(LayerKind::Group(_)) => Err(VeskraError::new(ErrorCode::UnsupportedLayerFeature, "select a raster layer")),
        None => Err(VeskraError::new(ErrorCode::NotFound, "active layer missing")),
    }
}

fn resolve_placement(d: &Document, p: PlacementSpec) -> EResult<Placement> {
    Ok(match p {
        PlacementSpec::AboveActive => match d.active_layer {
            Some(a) if d.layer(a).is_some() => Placement::Above(a),
            _ => Placement::Top,
        },
        PlacementSpec::Top => Placement::Top,
        PlacementSpec::Above { id } => Placement::Above(parse_layer(&id)?),
        PlacementSpec::Below { id } => Placement::Below(parse_layer(&id)?),
        PlacementSpec::Into { group, index } => Placement::IntoGroup { group: parse_layer(&group)?, index },
    })
}

fn sample_from_spec(s: &SampleSpec) -> EResult<StrokeSample> {
    let (pressure, mut flags) = match s.p {
        Some(p) => (p, SampleFlags::CONFIRMED | SampleFlags::PRESSURE_REPORTED),
        None => (FALLBACK_PRESSURE, SampleFlags::CONFIRMED),
    };
    flags.insert(SampleFlags::empty());
    let sample =
        StrokeSample { x: s.x, y: s.y, t_ns: s.t, pressure, tilt: s.tilt, azimuth: s.azimuth, rotation: None, flags };
    sample.validate()?;
    Ok(sample)
}

fn internal(e: serde_json::Error) -> VeskraError {
    VeskraError::new(ErrorCode::Internal, format!("serialization: {e}"))
}

/// The response envelope of `execute_json` (docs/COMMANDS.md): `{"ok", "version", "invalidated",
/// "warnings", "historyChanged", "viewChanged"}` or `{"error": {...}}`.
pub fn response_json(r: EResult<Receipt>) -> Value {
    match r {
        Ok(r) => json!({
            "ok": r.result,
            "version": r.document_version,
            "invalidated": r.invalidated.iter().map(|x| [x.x0, x.y0, x.x1, x.y1]).collect::<Vec<_>>(),
            "warnings": r.warnings,
            "historyChanged": r.history_changed,
            "viewChanged": r.view_changed,
        }),
        Err(e) => error_json(&e),
    }
}

pub fn error_json(e: &VeskraError) -> Value {
    json!({"error": {"code": e.code, "key": e.code.key(), "message": e.message, "recoverable": e.recoverable}})
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send>() {}
    is_send::<Session>();
    let _ = ExportFormat::Png;
    let _ = ExportOptions::default();
}
