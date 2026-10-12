//! # veskra-engine — the single command API (CORE-02, CORE-06, INT-01 host side)
//!
//! UI (via `veskra-ffi`), the CLI and tests all drive Veskra through a [`Session`]:
//! * typed [`Command`]s (also accepted as JSON, see `docs/COMMANDS.md`);
//! * a packed pointer-event path ([`Session::input_batch`]) that runs Rust input arbitration and
//!   paints the live stroke;
//! * [`Session::render_display`] for the platform presenter.
//!
//! Every successful mutating command is atomic and produces exactly one undo step; a stroke
//! becomes one undo step at commit (never per pointer packet). Failed commands leave the
//! document untouched.

pub mod brushlib;
pub mod command;
mod dirty;
pub mod guide;
mod session;
pub mod shape;
pub mod symmetry;

pub use command::{
    AdjustKindSpec, AnchorSpec, ColorTargetSpec, Command, GradientKindSpec, GradientStopSpec, LayerKindSpec,
    PlacementSpec, SampleSpec, SelectionOp, ToolKind,
};
pub use guide::GuideKind;
pub use session::{
    EResult, EngineConfig, FALLBACK_PRESSURE, InputOutcome, Receipt, Session, error_json, flags, response_json,
};
pub use symmetry::SymmetryMode;

pub use veskra_core;
pub use veskra_doc;
pub use veskra_input;
pub use veskra_io::CancelToken;

use std::path::Path;
use std::sync::Arc;
use veskra_core::VeskraError;
use veskra_core::export::{ExportFormat, ExportOptions, ExportReport};
use veskra_doc::Document;

/// Engine/FFI API version reported to the Android shell.
pub const API_VERSION: u32 = 1;

/// Re-render one committed stroke from its record alone (ADR-019): the preset stored in the
/// record's brush parameters, the brush assets embedded in `doc` (verified against their content
/// hashes) or built into this build, the record's color, seed and composition, over `base`.
/// Returns the layer tiles and the hashes that had to fall back to a round tip / procedural
/// grain because the document lacks them. No brush library is consulted. Selections that clipped
/// the stroke are not part of the record, so re-render a selection-clipped stroke with care.
pub fn rerender_stroke(
    doc: &Document,
    stroke: &veskra_doc::StrokeRecord,
    base: veskra_raster::TileGrid,
) -> Result<(veskra_raster::TileGrid, Vec<String>), VeskraError> {
    let preset = veskra_brush::load_preset_json(&stroke.brush_params).map_err(VeskraError::from)?.preset;
    let mut store = veskra_brush::AssetStore::new();
    for h in &stroke.brush_assets {
        if let Some(png) = doc.brush_asset(h)
            && let Ok(a) = veskra_brush::BrushAsset::from_stored_png(png)
            && a.hash() == h
        {
            store.insert(Arc::new(a));
        }
    }
    let assets = veskra_brush::ResolvedAssets::resolve(&preset, &[&store, veskra_brush::builtin_assets()]);
    let mode = match stroke.composition.mode {
        veskra_doc::StrokeMode::Paint => veskra_brush::StrokeMode::Paint,
        veskra_doc::StrokeMode::Erase => veskra_brush::StrokeMode::Erase,
    };
    let ctx = veskra_brush::StrokeContext {
        alpha_lock: stroke.composition.alpha_locked,
        ..veskra_brush::StrokeContext::new(mode, doc.bounds())
    };
    let mut r = veskra_brush::StrokeRenderer::begin_resolved(
        preset,
        assets,
        stroke.color,
        stroke.deterministic_seed,
        base,
        ctx,
    );
    let fallbacks = r.asset_fallbacks().to_vec();
    r.add_samples(stroke.samples());
    Ok((r.finish().tiles, fallbacks))
}

/// Export an immutable snapshot (safe to call off the session lock / on a background thread).
pub fn export_snapshot(
    doc: &Arc<Document>,
    format: ExportFormat,
    options: &ExportOptions,
    path: &Path,
    cancel: &CancelToken,
) -> Result<ExportReport, VeskraError> {
    veskra_io::export_to_path(doc, format, options, path, cancel)
}
