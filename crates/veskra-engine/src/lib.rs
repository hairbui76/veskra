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
