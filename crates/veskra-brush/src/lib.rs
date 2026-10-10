//! # veskra-brush — brush presets and the deterministic dab stroke renderer (BRH, ADR-004)
//!
//! Pipeline for one stroke ([`StrokeRenderer`]):
//! 1. **Confirmed samples only** enter the stroke; predicted samples are counted and skipped
//!    (they can only be shown through [`StrokeRenderer::prediction_preview`]).
//! 2. **Stabilizer** (BRH-05, paint path only): distance-based "pulled rope" follower; the
//!    recorded samples are untouched. On flush the path catches up to the last sample.
//! 3. **Dab placement** at arc-length spacing (`spacing × current diameter`, ≥ 0.25 px) with
//!    distance carry-over across segments; a single sample produces one dab.
//! 4. **Rasterization** of round dabs (hardness falloff, analytic AA, area-conserving
//!    sub-pixel dabs, optional seeded document-space grain) into a sparse per-stroke `u16`
//!    coverage buffer with `Max` or `BuildUp` accumulation.
//! 5. **Composite** `base ⊕ stroke` with integer math from `veskra_raster::ops`
//!    (paint = src-over, alpha lock = src-atop, erase = destination-out, selection multiplies).
//!
//! Every stage is a sequential state machine, so output is bit-identical for identical
//! (samples, preset, color, seed, context) no matter how samples are batched (PNT-001).
//! Vector-eligible presets also yield a fitted centerline and a variable-width outline whose
//! width is the dab profile's 50 % iso-contour ([`GeometryOut`]).
//!
//! One gesture may drive several strokes (a stroke and its symmetry replicas): each has its own
//! [`StrokeRenderer`], and [`composite_layered`] composites them in order over the shared base so
//! the result equals committing the strokes one after another.
//!
//! [`smudge`] is the one tool here that is not a dab-coverage brush: it moves the layer's own
//! pixels and therefore has no vector form (see its module docs).

mod coverage;
mod curve;
mod dab;
mod error;
mod geometry;
mod noise;
mod path;
pub mod preset;
mod renderer;
pub mod smudge;

pub use curve::{MAX_CURVE_POINTS, ResponseCurve};
pub use error::BrushError;
pub use geometry::{CENTERLINE_TOLERANCE, GeometryOut, OUTLINE_TOLERANCE};
pub use preset::{
    Accumulation, BUILTIN_IDS, BrushKind, BrushPreset, Grain, PRESET_SCHEMA_VERSION, PresetLoad, TiltSize,
    builtin_preset, builtin_presets, load_preset_json,
};
pub use renderer::{
    FinishedStroke, LayeredPart, MAX_DABS_PER_STROKE, MAX_SAMPLE_COORD, MIN_DAB_STEP_PX, PredictedCoverage,
    StrokeContext, StrokeMode, StrokeRenderer, StrokeStats, composite_layered,
};
pub use smudge::{FinishedSmudge, MAX_SMUDGE_SIZE_PX, SmudgeContext, SmudgeParams, SmudgeStroke};
