//! Command schema (ARCHITECTURE §C). The same typed commands serve the Android UI (as JSON over
//! JNI), the CLI and tests. Serialized names (`"cmd": "layer.add"`, camelCase fields) are the
//! stable FFI contract documented in `docs/COMMANDS.md`.
//!
//! **Unknown fields are rejected** (`deny_unknown_fields`, review decision): a misspelled optional
//! field (e.g. `"opactiy"`) would otherwise be ignored and the command would report success —
//! for `layer.set` even recording an undo step that changed nothing. UI and engine ship together
//! in one APK, so there is no version skew to tolerate. Explicit `null` for an optional field is
//! accepted (the Kotlin builders send `"background": null`). Limits: field-less commands (serde
//! unit variants, e.g. `document.info`) ignore extra keys — they have no argument to misspell —
//! and `ExportOptions` (veskra-core) still ignores unknown fields.

use serde::{Deserialize, Serialize};
use veskra_core::export::{ExportFormat, ExportOptions};

/// Where to place a layer, mirroring `veskra_doc::Placement` in JSON form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", tag = "at", deny_unknown_fields)]
pub enum PlacementSpec {
    /// Directly above the active layer (or top of the stack when none).
    #[default]
    AboveActive,
    Top,
    Above {
        id: String,
    },
    Below {
        id: String,
    },
    Into {
        group: String,
        #[serde(default)]
        index: Option<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKindSpec {
    Raster,
    Vector,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SelectionOp {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// One input sample for headless `stroke.samples` (document coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleSpec {
    pub x: f32,
    pub y: f32,
    /// Timestamp in nanoseconds.
    #[serde(default)]
    pub t: u64,
    /// Hardware pressure; omitted = not reported (fallback pressure, flag cleared).
    #[serde(default)]
    pub p: Option<f32>,
    #[serde(default)]
    pub tilt: Option<f32>,
    #[serde(default)]
    pub azimuth: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum Command {
    // ----- document -----
    #[serde(rename = "document.create")]
    DocumentCreate {
        width: u32,
        height: u32,
        #[serde(default = "default_dpi")]
        dpi: f32,
        /// `#RRGGBB[AA]` fill for an initial "Background" raster layer; `None` = transparent canvas.
        #[serde(default)]
        background: Option<String>,
    },
    #[serde(rename = "document.info")]
    DocumentInfo,
    #[serde(rename = "document.save")]
    DocumentSave { path: String },
    #[serde(rename = "document.open")]
    DocumentOpen { path: String },
    #[serde(rename = "document.autosave")]
    DocumentAutosave { dir: String },
    /// Fully decode and validate a `.veskra` file without opening it (staged SAF save check).
    #[serde(rename = "document.validate")]
    DocumentValidate { path: String },
    /// Open the newest valid checkpoint in an autosave directory (never marked as saved).
    #[serde(rename = "document.recover")]
    DocumentRecover { dir: String },
    #[serde(rename = "recovery.list")]
    RecoveryList { dir: String },
    #[serde(rename = "recovery.open")]
    RecoveryOpen { path: String },

    // ----- layers -----
    #[serde(rename = "layer.add")]
    LayerAdd {
        #[serde(default = "default_kind")]
        kind: LayerKindSpec,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        placement: PlacementSpec,
    },
    #[serde(rename = "layer.remove")]
    LayerRemove { id: String },
    #[serde(rename = "layer.duplicate")]
    LayerDuplicate { id: String },
    #[serde(rename = "layer.move")]
    LayerMove { id: String, placement: PlacementSpec },
    #[serde(rename = "layer.set")]
    LayerSet {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        visible: Option<bool>,
        #[serde(default)]
        opacity: Option<f32>,
        #[serde(default)]
        blend: Option<String>,
        #[serde(default)]
        locked: Option<bool>,
        #[serde(default)]
        alpha_locked: Option<bool>,
        #[serde(default)]
        clip: Option<bool>,
        #[serde(default)]
        pass_through: Option<bool>,
    },
    #[serde(rename = "layer.rename")]
    LayerRename { id: String, name: String },
    #[serde(rename = "layer.select")]
    LayerSelect { id: String },
    #[serde(rename = "layer.clear")]
    LayerClear { id: String },
    /// Import a PNG as a new raster layer above the active layer (color-managed, COL-004/005).
    #[serde(rename = "layer.import_image")]
    LayerImportImage {
        path: String,
        /// `convert` (default, keep appearance) or `assign` (keep numbers).
        #[serde(default)]
        handling: Option<String>,
        /// `assume_srgb` (default) or `reject` for files without a color profile.
        #[serde(default)]
        missing_profile: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
    },
    #[serde(rename = "layer.list")]
    LayerList,

    // ----- brush / color -----
    #[serde(rename = "brush.list")]
    BrushList,
    #[serde(rename = "brush.select")]
    BrushSelect { preset: String },
    #[serde(rename = "brush.set")]
    BrushSet {
        #[serde(default)]
        size: Option<f32>,
        #[serde(default)]
        opacity: Option<f32>,
        #[serde(default)]
        flow: Option<f32>,
        #[serde(default)]
        stabilization: Option<f32>,
    },
    #[serde(rename = "color.set")]
    ColorSet { hex: String },
    #[serde(rename = "color.get")]
    ColorGet,
    /// Sample the composite (or the active layer with `layer: true`) at a document point.
    #[serde(rename = "color.pick")]
    ColorPick {
        x: f32,
        y: f32,
        #[serde(default)]
        layer: bool,
    },

    // ----- strokes (headless path; Android uses the packed input batch) -----
    #[serde(rename = "stroke.begin")]
    StrokeBegin {
        #[serde(default)]
        eraser: bool,
    },
    #[serde(rename = "stroke.samples")]
    StrokeSamples { samples: Vec<SampleSpec> },
    #[serde(rename = "stroke.commit")]
    StrokeCommit,
    #[serde(rename = "stroke.cancel")]
    StrokeCancel,

    // ----- history -----
    #[serde(rename = "history.undo")]
    HistoryUndo,
    #[serde(rename = "history.redo")]
    HistoryRedo,

    // ----- selection / edit -----
    #[serde(rename = "selection.rect")]
    SelectionRect {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        #[serde(default)]
        op: SelectionOp,
    },
    #[serde(rename = "selection.ellipse")]
    SelectionEllipse {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        #[serde(default)]
        op: SelectionOp,
    },
    #[serde(rename = "selection.lasso")]
    SelectionLasso {
        points: Vec<[f64; 2]>,
        #[serde(default)]
        op: SelectionOp,
    },
    #[serde(rename = "selection.all")]
    SelectionAll,
    #[serde(rename = "selection.clear")]
    SelectionClear,
    #[serde(rename = "selection.invert")]
    SelectionInvert,
    /// Bucket fill at a document point on the active layer.
    #[serde(rename = "edit.fill")]
    EditFill {
        x: f32,
        y: f32,
        #[serde(default = "default_tolerance")]
        tolerance: f32,
        #[serde(default = "default_true")]
        contiguous: bool,
        /// Use the visible composite as the region reference instead of the active layer.
        #[serde(default)]
        sample_merged: bool,
    },
    /// Fill the current selection (or whole layer) with the foreground color.
    #[serde(rename = "edit.fill_selection")]
    EditFillSelection,
    /// Erase the selected pixels of the active layer.
    #[serde(rename = "edit.delete_selection")]
    EditDeleteSelection,
    /// Affine-transform the selected pixels (or the whole active layer) by `[a,b,c,d,e,f]`.
    #[serde(rename = "edit.transform")]
    EditTransform { matrix: [f64; 6] },

    // ----- view / input -----
    #[serde(rename = "view.size")]
    ViewSize { width: f64, height: f64 },
    #[serde(rename = "view.fit")]
    ViewFit,
    #[serde(rename = "view.set")]
    ViewSet {
        #[serde(default)]
        zoom: Option<f64>,
        #[serde(default)]
        rotation: Option<f64>,
        #[serde(default)]
        pan_x: Option<f64>,
        #[serde(default)]
        pan_y: Option<f64>,
        #[serde(default)]
        mirror: Option<bool>,
    },
    #[serde(rename = "view.mirror")]
    ViewMirror { enabled: bool },
    #[serde(rename = "input.config")]
    InputConfig {
        #[serde(default)]
        finger_draws: Option<bool>,
        #[serde(default)]
        stylus_button: Option<String>,
    },
    /// Lifecycle pause: pending stroke handled per `veskra-input` reset policy.
    #[serde(rename = "input.reset")]
    InputReset,

    // ----- export -----
    #[serde(rename = "export.preflight")]
    ExportPreflight {
        format: ExportFormat,
        #[serde(default)]
        options: ExportOptions,
    },
    #[serde(rename = "export.run")]
    ExportRun {
        format: ExportFormat,
        path: String,
        #[serde(default)]
        options: ExportOptions,
    },
}

fn default_dpi() -> f32 {
    300.0
}
fn default_kind() -> LayerKindSpec {
    LayerKindSpec::Raster
}
fn default_tolerance() -> f32 {
    0.1
}
fn default_true() -> bool {
    true
}

impl Command {
    /// Parse a JSON command with a typed error.
    pub fn from_json(s: &str) -> Result<Command, veskra_core::VeskraError> {
        serde_json::from_str(s).map_err(|e| veskra_core::VeskraError::invalid(format!("bad command: {e}")))
    }

    /// Commands that mutate the document and therefore create one undo step when they succeed.
    pub fn label(&self) -> &'static str {
        match self {
            Command::LayerAdd { .. } => "Add layer",
            Command::LayerRemove { .. } => "Delete layer",
            Command::LayerDuplicate { .. } => "Duplicate layer",
            Command::LayerMove { .. } => "Move layer",
            Command::LayerSet { .. } => "Layer properties",
            Command::LayerRename { .. } => "Rename layer",
            Command::LayerClear { .. } => "Clear layer",
            Command::LayerImportImage { .. } => "Import image",
            Command::StrokeCommit => "Stroke",
            Command::SelectionRect { .. }
            | Command::SelectionEllipse { .. }
            | Command::SelectionLasso { .. }
            | Command::SelectionAll
            | Command::SelectionClear
            | Command::SelectionInvert => "Selection",
            Command::EditFill { .. } | Command::EditFillSelection => "Fill",
            Command::EditDeleteSelection => "Delete",
            Command::EditTransform { .. } => "Transform",
            _ => "",
        }
    }
}
