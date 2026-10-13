//! One editing session: the current document snapshot, history, tool state, input arbitration
//! and the live (uncommitted) stroke.
//!
//! Threading: a `Session` is a single-owner actor (`&mut self`). The FFI layer serializes access
//! with a mutex; long exports run on an `Arc<Document>` snapshot outside that lock
//! (see [`crate::export_snapshot`]).

use crate::command::{Command, LayerKindSpec, PlacementSpec, SampleSpec, SelectionOp};
use crate::dirty::DirtySet;
use crate::guide::ActiveGuide;
use crate::symmetry::Replica;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use veskra_brush::BrushPreset;
use veskra_core::export::{ExportFormat, ExportOptions};
use veskra_core::{
    BlendMode, BlendSpace, ColorRgba, DocumentId, ErrorCode, LayerId, PointF, RectF, RectI, SampleFlags, StrokeSample,
    VeskraError,
};
use veskra_doc::{
    DocColor, Document, GroupContent, History, HistoryConfig, LayerKind, LayerPatch, LayerProps, PixelContent,
    Placement, StrokeMode, VectorFidelity,
};
use veskra_input::{InputAction, InputArbiter, InputConfig, RawPointerEvent, ViewTransform};
use veskra_raster::{MaskGrid, MaskOp, TileGrid};

mod brushes;
mod color_tools;
mod raster_tools;
mod tools;

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
    /// What draws: one brush renderer per stroke of the gesture (the user's stroke plus its
    /// symmetry replicas), or the smudge engine. See `session/tools.rs`.
    ink: tools::LiveInk,
    /// The layer when the gesture began (after a filled shape's fill, if any).
    base: TileGrid,
    /// Base ⊕ confirmed stroke(s), shown instead of the layer's tiles while the stroke is live.
    live_tiles: TileGrid,
    /// Live tiles plus predicted dabs (display only, STY-007).
    predicted_tiles: Option<TileGrid>,
    predicted_rect: DirtySet,
    // The confirmed samples are owned by the renderers (`confirmed_samples()`): only samples a
    // renderer accepted (validated, monotonic time, in range) are rendered *and* recorded, so the
    // committed `StrokeRecord`s always reproduce the pixels.
    preset: BrushPreset,
    color: ColorRgba,
    seed: u64,
    mode: StrokeMode,
    alpha_locked: bool,
    selection_clipped: bool,
    opacity: f32,
    bounds: DirtySet,
    /// Symmetry transforms of the replicas (`ink` brush part `i + 1` / smudge head `i + 1`).
    replicas: Vec<Replica>,
    /// Ruler bound to this stroke; applied to samples before symmetry.
    guide: Option<ActiveGuide>,
    /// History label of the committed gesture.
    label: &'static str,
    /// Raster edit to record on the layer at commit (`fill` for filled shapes, `smudge`).
    raster_edit: Option<&'static str>,
    /// Path length fed so far and its last point (work budget of multi-stroke gestures).
    path_px: f64,
    last_pt: Option<PointF>,
}

pub struct Session {
    cfg: EngineConfig,
    ids: IdSource,
    doc: Option<Arc<Document>>,
    history: History,
    version: u64,
    saved_version: Option<u64>,
    /// Built-in + user brushes and their assets (ADR-019).
    library: crate::brushlib::BrushLibrary,
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
    /// Stroke-time tool state: symmetry, guide, tool, smudge strength (not saved in documents).
    tools: tools::ToolState,
    /// sRGB ↔ CMYK transforms for `color.convert` (ADR-018), cached per (profile, intent, BPC).
    cmyk_cache: color_tools::CmykCache,
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
        let library = crate::brushlib::BrushLibrary::new();
        let brush = library.get("pencil.smooth").unwrap_or_default();
        Self {
            history: History::new(cfg.history),
            cfg,
            ids,
            doc: None,
            version: 0,
            saved_version: None,
            library,
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
            tools: tools::ToolState::default(),
            cmyk_cache: color_tools::CmykCache::default(),
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
                let mut r = self.open(&latest.path)?;
                self.saved_version = None;
                self.doc_path = None;
                // Recompute: the info inside `r` was produced while the checkpoint looked saved.
                r.result = self.info()?;
                Ok(r)
            }
            Command::RecoveryOpen { path } => {
                let mut r = self.open(Path::new(&path))?;
                // A recovered checkpoint is not the user's file: never mark it saved.
                self.saved_version = None;
                self.doc_path = None;
                // Recompute: the info inside `r` was produced while the checkpoint looked saved.
                r.result = self.info()?;
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
            Command::LayerImportImage { path, handling, missing_profile, name, x, y } => {
                self.import_image(label, &path, handling.as_deref(), missing_profile.as_deref(), name, x, y)
            }
            Command::LayerMaskFromSelection { id } => {
                let id = parse_layer(&id)?;
                let r = self.mutate(label, |d, _| {
                    require_raster(d, id)?;
                    let b = d.bounds();
                    let mask = d.selection.clone().unwrap_or_else(MaskGrid::new_full);
                    d.pixels_mut_editable(id)?.mask = Some(mask);
                    Ok((json!({}), b))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerMaskRemove { id } => {
                let id = parse_layer(&id)?;
                let r = self.mutate(label, |d, _| {
                    require_raster(d, id)?;
                    let px = d.pixels_mut_editable(id)?;
                    if px.mask.take().is_none() {
                        return Err(VeskraError::new(ErrorCode::InvalidState, "layer has no mask"));
                    }
                    Ok((json!({}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
            }
            Command::LayerMaskApply { id } => {
                let id = parse_layer(&id)?;
                let r = self.mutate(label, |d, _| {
                    require_raster(d, id)?;
                    let px = d.pixels_mut_editable(id)?;
                    let mask =
                        px.mask.take().ok_or_else(|| VeskraError::new(ErrorCode::InvalidState, "layer has no mask"))?;
                    px.tiles = veskra_raster::ops::mask_pixels(&px.tiles, &mask);
                    d.mark_raster_edit(id, "mask_apply")?;
                    Ok((json!({}), d.bounds()))
                })?;
                Ok(self.finish_mutation(r))
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
            Command::BrushList => Ok(self.brush_list()),
            Command::BrushSelect { preset } => {
                let p = self
                    .library
                    .get(&preset)
                    .ok_or_else(|| VeskraError::new(ErrorCode::NotFound, format!("unknown brush preset {preset}")))?;
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
            Command::ColorConvert { hex, cmyk, to, cmyk_profile, intent, black_point_compensation } => {
                self.color_convert(hex, cmyk, to, cmyk_profile, intent, black_point_compensation)
            }
            Command::ProfileInspect { path } => self.profile_inspect(path),
            Command::StrokeBegin { eraser, tool } => {
                self.begin_stroke_with(tools::BeginOpts { eraser, tool, shape: None })?;
                Ok(self.receipt(json!({})))
            }
            Command::StrokeSamples { samples } => {
                let samples: Vec<StrokeSample> = samples.iter().map(sample_from_spec).collect::<EResult<_>>()?;
                let dirty = self.add_samples(&samples)?;
                let mut r = self.receipt(json!({}));
                r.invalidated = dirty.into_rects();
                Ok(r)
            }
            Command::StrokeCommit => {
                // On failure the error carries no rects: headless callers must refresh fully.
                let (v, dirty) = self.commit_stroke();
                let v = v?;
                let committed = v.get("committed").and_then(Value::as_bool) == Some(true);
                let mut r = self.receipt(v);
                r.history_changed = committed;
                r.invalidated = dirty.into_rects();
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
                r.invalidated = dirty.into_rects();
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
            Command::InputConfig { finger_draws, stylus_button, lock_view } => {
                let mut cfg = self.arbiter.config().clone();
                if let Some(f) = finger_draws {
                    cfg.finger_draws = f;
                }
                if let Some(l) = lock_view {
                    cfg.lock_view = l;
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
                let cfg = self.arbiter.config();
                let button = match cfg.stylus_button_action {
                    veskra_input::ButtonAction::Eyedropper => "eyedropper",
                    veskra_input::ButtonAction::ToggleEraser => "toggle_eraser",
                    veskra_input::ButtonAction::Undo => "undo",
                    _ => "none",
                };
                Ok(self.receipt(
                    json!({"fingerDraws": cfg.finger_draws, "stylusButton": button, "lockView": cfg.lock_view}),
                ))
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
            // ===== BEGIN raster-edit / canvas tools (session/raster_tools.rs) =====
            Command::EditGradient { x0, y0, x1, y1, kind, stops, opacity } => {
                self.edit_gradient(label, (x0, y0), (x1, y1), kind, stops, opacity)
            }
            Command::SelectionWand { x, y, tolerance, contiguous, sample_merged, op } => {
                self.selection_wand(label, x, y, tolerance, contiguous, sample_merged, op)
            }
            Command::EditAdjust { kind, hue, saturation, brightness, contrast, rgb, r, g, b } => {
                self.edit_adjust(label, kind, hue, saturation, brightness, contrast, [rgb, r, g, b])
            }
            Command::EditTransformQuad { quad } => self.edit_transform_quad(label, quad),
            Command::CanvasCrop { x0, y0, x1, y1 } => self.canvas_crop(label, x0, y0, x1, y1),
            Command::CanvasResize { width, height, resample, anchor } => {
                self.canvas_resize(label, width, height, resample, anchor)
            }
            Command::SelectionOutline { max_points } => self.selection_outline(max_points),
            Command::LayerBounds { id } => self.layer_bounds(id),
            // ===== END raster-edit / canvas tools =====
            // ===== stroke-time tools, to the end of this match (implemented in `session/tools.rs`) =====
            Command::SymmetrySet { mode, count, mirror, cx, cy } => self.symmetry_set(mode, count, mirror, cx, cy),
            Command::SymmetryGet => Ok(self.receipt(self.symmetry_json())),
            Command::GuideSet { kind, x0, y0, x1, y1, angle } => self.guide_set(kind, x0, y0, x1, y1, angle),
            Command::GuideGet => Ok(self.receipt(self.tools.guide.to_json())),
            Command::ToolSet { tool, strength } => self.tool_set(tool, strength),
            Command::ToolGet => Ok(self.receipt(self.tool_json())),
            Command::ShapeLine { x0, y0, x1, y1 } => self.shape_draw(crate::shape::line(x0, y0, x1, y1)?, false),
            Command::ShapeRect { x0, y0, x1, y1, fill } => self.shape_draw(crate::shape::rect(x0, y0, x1, y1)?, fill),
            Command::ShapeEllipse { x0, y0, x1, y1, fill } => {
                self.shape_draw(crate::shape::ellipse(x0, y0, x1, y1)?, fill)
            }
            Command::ShapePath { points, closed, smooth, fill } => {
                self.shape_draw(crate::shape::path(&points, closed, smooth)?, fill)
            }
            // ===== brush library (implemented in `session/brushes.rs`) =====
            Command::BrushLibraryOpen { dir } => self.brush_library_open(&dir),
            Command::BrushGet { id } => self.brush_get(&id),
            Command::BrushSave { preset, assets } => self.brush_save(preset, assets),
            Command::BrushDelete { id } => self.brush_delete(&id),
            Command::BrushDuplicate { id, name } => self.brush_duplicate(&id, name.as_deref()),
            Command::BrushImport { path } => self.brush_import(&path),
            Command::BrushExport { id, path } => self.brush_export(&id, &path),
            Command::BrushPreview { id, width, height } => self.brush_preview(&id, width, height),
            Command::BrushTipPreview { id, size, color } => self.brush_tip_preview(&id, size, color.as_deref()),
            Command::BrushAssetAdd { path, channel, invert } => self.brush_asset_add(&path, channel, invert),
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
        // Never write a native project under an interchange or Adobe extension (ADR-008: no
        // file may masquerade as `.ai`, and a `.psd`/`.pdf` name would mislabel the bundle).
        let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
        if matches!(ext.as_deref(), Some("ai" | "psd" | "psb" | "pdf" | "svg" | "png" | "jpg" | "jpeg")) {
            return Err(VeskraError::invalid("project files must not use an interchange extension; use .veskra"));
        }
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
        let mut asset_warnings = Vec::new();
        for (hash, png) in loaded.doc.iter_brush_assets() {
            match veskra_brush::BrushAsset::from_stored_png(png) {
                Ok(a) if a.hash() == hash => {}
                _ => asset_warnings.push(format!(
                    "Embedded brush image {}… is damaged: strokes keep their pixels, but re-rendering them falls back to a round tip.",
                    &hash[..12.min(hash.len())]
                )),
            }
        }
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
        r.warnings.extend(asset_warnings);
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
            "symmetry": self.symmetry_json(), "guide": self.tools.guide.to_json(), "tool": self.tool_json(),
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

    #[allow(clippy::too_many_arguments)]
    fn import_image(
        &mut self,
        label: &str,
        path: &str,
        handling: Option<&str>,
        missing: Option<&str>,
        name: Option<String>,
        x: i32,
        y: i32,
    ) -> EResult<Receipt> {
        use veskra_io::import::{ImportOptions, MissingProfilePolicy, ProfileHandling};
        let handling = match handling.unwrap_or("convert") {
            "convert" => ProfileHandling::ConvertToDocument,
            "assign" => ProfileHandling::AssignDocumentProfile,
            other => return Err(VeskraError::invalid(format!("unknown handling {other} (convert|assign)"))),
        };
        let missing_profile = match missing.unwrap_or("assume_srgb") {
            "assume_srgb" => MissingProfilePolicy::AssumeSrgb,
            "reject" => MissingProfilePolicy::Reject,
            other => return Err(VeskraError::invalid(format!("unknown missingProfile {other} (assume_srgb|reject)"))),
        };
        let meta = std::fs::metadata(path)
            .map_err(|e| VeskraError::new(ErrorCode::Io, format!("cannot read image: {}", e.kind())))?;
        if meta.len() > veskra_core::limits::MAX_BUNDLE_ENTRY_BYTES {
            return Err(VeskraError::alloc("image file too large"));
        }
        let bytes = std::fs::read(path)
            .map_err(|e| VeskraError::new(ErrorCode::Io, format!("cannot read image: {}", e.kind())))?;
        let doc_profile = veskra_color::icc::IccProfile::parse(&self.doc()?.color.icc_bytes)?;
        let opts = ImportOptions { handling, missing_profile, ..Default::default() };
        let img = veskra_io::import::import_png(&bytes, &doc_profile, &opts)?;
        let id = LayerId::from_u128(self.next_id());
        let warnings = img.warnings.clone();
        let r = self.mutate(label, |d, _| {
            let b = d.bounds();
            let placed = if x == 0 && y == 0 {
                img.tiles.clone()
            } else {
                let m = veskra_core::Affine::translate(x as f64, y as f64);
                veskra_raster::transform::try_transform_pixels(&img.tiles, None, m, b)?
            };
            let mut px = PixelContent::empty();
            px.tiles = placed;
            px.raster_edits.push("import".into());
            let p = resolve_placement(d, PlacementSpec::AboveActive)?;
            d.add_layer(
                id,
                LayerProps::named(name.unwrap_or_else(|| "Imported image".into())),
                LayerKind::Raster(px),
                p,
            )?;
            d.active_layer = Some(id);
            let dirty =
                RectI::new(x, y, x.saturating_add(img.width as i32), y.saturating_add(img.height as i32)).intersect(&b);
            Ok((
                json!({"layerId": id.to_string(), "width": img.width, "height": img.height,
                       "sourceProfile": img.source_profile_name, "profileAssumed": img.profile_assumed,
                       "converted": img.converted, "warnings": img.warnings}),
                dirty,
            ))
        })?;
        let mut receipt = self.finish_mutation(r);
        receipt.warnings.extend(warnings);
        Ok(receipt)
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
               "eraser": self.brush.eraser, "category": self.brush.category.as_str(),
               "source": self.brush.source.as_string(), "version": self.brush.version})
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

    // The stroke pipeline (begin → samples/prediction → commit) lives in `session/tools.rs`, where
    // symmetry, guides, smudge and shapes plug into it. These wrappers are the entry points of
    // both the headless commands and the packed input path.

    fn begin_stroke(&mut self, eraser_requested: bool) -> EResult<()> {
        self.begin_stroke_with(tools::BeginOpts { eraser: eraser_requested, tool: None, shape: None })
    }

    fn add_samples(&mut self, samples: &[StrokeSample]) -> EResult<DirtySet> {
        self.add_samples_impl(samples)
    }

    fn predict(&mut self, predicted: &[StrokeSample]) -> DirtySet {
        self.predict_impl(predicted)
    }

    /// End the live stroke and commit it as exactly one undo step (`{"committed": false}` and no
    /// history entry when no sample was accepted). The returned rects must be refreshed whether or
    /// not the commit succeeded: the live preview is gone either way. A failed commit changes
    /// neither document, version, history nor the ID stream. Failure is not expected in practice:
    /// samples are pre-filtered by the renderer, vector-layer policy is enforced at begin, and
    /// every document mutation is refused while a stroke is live.
    fn commit_stroke(&mut self) -> (EResult<Value>, DirtySet) {
        let Some(live) = self.live.take() else {
            return (Err(VeskraError::new(ErrorCode::InvalidState, "no stroke in progress")), DirtySet::default());
        };
        let mut dirty = live.bounds.clone();
        dirty.extend(&live.predicted_rect);
        let ids = self.ids.clone();
        let r = self.commit_live_impl(live, &mut dirty);
        if r.is_err() {
            self.ids = ids;
        }
        (r, dirty)
    }

    fn cancel_stroke(&mut self) -> DirtySet {
        match self.live.take() {
            Some(l) => {
                let mut d = l.bounds;
                d.extend(&l.predicted_rect);
                d
            }
            None => DirtySet::default(),
        }
    }

    fn undo(&mut self) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let cur = self.doc()?.clone();
        let old = cur.bounds();
        let (label, prev) =
            self.history.undo(cur).ok_or_else(|| VeskraError::new(ErrorCode::NothingToUndo, "nothing to undo"))?;
        let b = prev.bounds();
        self.doc = Some(prev);
        self.version += 1;
        let mut r = self.receipt(json!({"undone": label}));
        r.invalidated.push(b);
        r.history_changed = true;
        self.canvas_changed(old, &mut r); // canvas.crop / canvas.resize steps change the size
        Ok(r)
    }

    fn redo(&mut self) -> EResult<Receipt> {
        if self.live.is_some() {
            return Err(VeskraError::new(ErrorCode::InvalidState, "finish or cancel the current stroke first"));
        }
        let cur = self.doc()?.clone();
        let old = cur.bounds();
        let (label, next) =
            self.history.redo(cur).ok_or_else(|| VeskraError::new(ErrorCode::NothingToRedo, "nothing to redo"))?;
        let b = next.bounds();
        self.doc = Some(next);
        self.version += 1;
        let mut r = self.receipt(json!({"redone": label}));
        r.invalidated.push(b);
        r.history_changed = true;
        self.canvas_changed(old, &mut r);
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
        let mut dirty = DirtySet::default();
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
                        Ok(r) => dirty.extend(&r),
                        Err(e) => out.warnings.push(e.message),
                    }
                }
                InputAction::StrokePrediction(p) => {
                    if !self.swallow_stroke {
                        dirty.extend(&self.predict(&p));
                    }
                }
                InputAction::StrokeCommit => {
                    if self.swallow_stroke {
                        self.swallow_stroke = false;
                        continue;
                    }
                    let (res, r) = self.commit_stroke();
                    // Refresh even on failure: the live preview is gone.
                    dirty.extend(&r);
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
                    dirty.extend(&self.cancel_stroke());
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
                        dirty.add(*r.invalidated.first().unwrap_or(&RectI::EMPTY));
                        out.flags |= flags::HISTORY;
                    }
                }
                InputAction::ThreeFingerTap => {
                    if let Ok(r) = self.redo() {
                        dirty.add(*r.invalidated.first().unwrap_or(&RectI::EMPTY));
                        out.flags |= flags::HISTORY;
                    }
                }
                InputAction::StylusButton { pressed, action } => match action {
                    veskra_input::ButtonAction::ToggleEraser => self.eraser_toggle = pressed,
                    veskra_input::ButtonAction::Undo if pressed => {
                        if let Ok(r) = self.undo() {
                            dirty.add(*r.invalidated.first().unwrap_or(&RectI::EMPTY));
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
        // Disjoint areas (e.g. symmetry replicas) stay separate rects so the UI does not refresh
        // their whole bounding box on every pointer packet.
        let clip = self.doc.as_ref().map(|d| d.bounds());
        for r in dirty.into_rects() {
            let r = clip.map_or(r, |c| r.intersect(&c));
            if !r.is_empty() {
                out.dirty.push(r);
            }
        }
        if !out.dirty.is_empty() {
            out.flags |= flags::DIRTY;
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
                overrides_storage = [(l.layer, l.display_tiles())];
                &overrides_storage
            }
            None => &[],
        };
        let opts = veskra_compose::ComposeOptions { overrides, ..Default::default() };
        veskra_compose::render_display_rgba8(d, rect, &opts, out)
    }
}

/// Masks are raster-only: on a vector-stroke layer they would break exact vector fidelity.
fn require_raster(d: &Document, id: LayerId) -> EResult<()> {
    match d.layer(id).map(|l| &l.kind) {
        Some(LayerKind::Raster(_)) => Ok(()),
        Some(_) => Err(VeskraError::new(ErrorCode::UnsupportedLayerFeature, "layer masks need a raster layer")),
        None => Err(VeskraError::new(ErrorCode::NotFound, "layer not found")),
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
