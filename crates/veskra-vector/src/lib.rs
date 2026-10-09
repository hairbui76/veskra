//! # veskra-vector — stroke geometry for editable vector export (BRH)
//!
//! Tickets: VEC-01 (error-bounded fitting), VEC-02 (pressure-width outline), PNT-010
//! (fit-error metric); groundwork for FMT-12 (SVG editable paths).
//!
//! * [`BezPath`] / [`PathEl`] — the shared path type (document px, y down): exact bounds,
//!   affine transform, flattening with a rigorous tolerance, deterministic SVG `d` and PDF
//!   path-operator serialization, reverse, length, serde.
//! * [`fit_curve`] — error-bounded cubic fit of a sampled centerline: every input point is
//!   within `max_error` of the output (see [`fit`] docs for the proof sketch).
//! * [`variable_width_outline`] — a single closed outline (nonzero fill) covering the union
//!   of the swept pressure discs within `tolerance` (see [`outline`] docs).
//! * [`max_distance_to_path`] — true point-to-path distance metric used to verify fits.
//!
//! ## What is and is NOT preserved in Illustrator handoff (MASTER_PLAN §2.3, §7.3)
//! * Preserved: the stroke's **filled appearance** as editable anchors/handles (SVG
//!   `<path>` / PDF `m l c h` + fill), colorable and editable in Illustrator; plain
//!   centerlines as editable open paths.
//! * NOT preserved: pressure/tilt dynamics, Illustrator variable-width profiles or
//!   pressure-aware brush objects, brush textures. A pressure stroke becomes an outline
//!   whose width can only be changed by editing its anchors. Exporters must say so and
//!   must not label outlines as live brush strokes. Textured/smudge strokes are not handled
//!   by this crate (raster fallback, VEC-03).
//! * Output paths rely on the **nonzero** fill rule (`fill-rule="nonzero"` in SVG, `f` not
//!   `f*` in PDF); the outline contour may overlap itself.

mod cubic;
mod distance;
pub mod fit;
pub mod num;
pub mod outline;
mod path;
mod vec2;

pub use cubic::{CubicBez, MAX_FLATTEN_DEPTH};
pub use distance::{PreparedPath, max_distance_to_path};
pub use fit::{DEFAULT_CORNER_ANGLE, FitOptions, fit_curve, fit_curve_with};
pub use outline::{
    Cap, DEFAULT_OUTLINE_TOLERANCE, OutlineOptions, OutlineReport, WidthPoint, outline_contour, variable_width_outline,
    variable_width_outline_report,
};
pub use path::{BezPath, PathEl, PathSeg, Subpath};
