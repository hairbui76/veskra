//! # veskra-brush — brush presets and the deterministic dab stroke renderer (BRH, ADR-004, ADR-019)
//!
//! Pipeline for one stroke ([`StrokeRenderer`]):
//! 1. **Confirmed samples only** enter the stroke; predicted samples are counted and skipped
//!    (they can only be shown through [`StrokeRenderer::prediction_preview`]).
//! 2. **Stabilizer** (BRH-05, paint path only): distance-based "pulled rope" follower; the
//!    recorded samples are untouched. On flush the path catches up to the last sample.
//! 3. **Dab placement** at arc-length spacing (`spacing × current diameter`, ≥ 0.25 px) with
//!    distance carry-over across segments; a single sample produces one dab.
//! 4. **Dynamics and jitter** (schema 3): pressure / tilt / velocity drive size, opacity, flow,
//!    texture depth and scatter through response curves; seeded per-dab jitter of size, angle,
//!    opacity, flow, scatter and color.
//! 5. **Rasterization** of the dab — round (hardness falloff, analytic AA, area-conserving
//!    sub-pixel dabs, optional ellipse/rotation) or an image tip (mip-mapped grayscale mask) —
//!    with optional procedural or image texture, into a sparse per-stroke `u16` coverage buffer
//!    with `Max` or `BuildUp` accumulation (plus wet-edge depth / per-dab color buffers when the
//!    preset uses them).
//! 6. **Composite** `base ⊕ stroke` with integer math from `veskra_raster::ops`
//!    (paint = src-over, alpha lock = src-atop, erase = destination-out, selection multiplies).
//!
//! Every stage is a sequential state machine, so output is bit-identical for identical
//! (samples, preset, assets, color, seed, context) no matter how samples are batched (PNT-001).
//! Vector-eligible presets also yield a fitted centerline and a variable-width outline whose
//! width is the dab profile's 50 % iso-contour ([`GeometryOut`]).
//!
//! One gesture may drive several strokes (a stroke and its symmetry replicas): each has its own
//! [`StrokeRenderer`], and [`composite_layered`] composites them in order over the shared base so
//! the result equals committing the strokes one after another.
//!
//! [`smudge`] is the one tool here that is not a dab-coverage brush: it moves the layer's own
//! pixels and therefore has no vector form (see its module docs).
//!
//! Brush assets ([`asset`]) are content-hashed grayscale masks; the built-in ones are generated
//! procedurally ([`builtin_assets`]). [`interop::infinite_painter`] maps Infinite Painter
//! `.prbr` properties onto schema-3 presets (approximations are reported, never hidden).

pub mod asset;
mod builtin;
mod coverage;
mod curve;
mod dab;
mod error;
mod geometry;
pub mod interop;
mod noise;
mod path;
pub mod preset;
mod procedural;
mod renderer;
pub mod smudge;
mod stamp;

pub use asset::{AssetStore, BrushAsset, ImportedMask, MaskChannel, ResolvedAssets};
pub use builtin::{
    BUILTIN_ASSET_NAMES, BUILTIN_IDS, LEGACY_BUILTIN_IDS, builtin_asset_hash, builtin_assets, builtin_preset,
    builtin_presets,
};
pub use curve::{MAX_CURVE_POINTS, ResponseCurve};
pub use error::BrushError;
pub use geometry::{CENTERLINE_TOLERANCE, GeometryOut, OUTLINE_TOLERANCE};
pub use preset::{
    Accumulation, BrushCategory, BrushPreset, BrushSource, DabInput, Dynamics, DynamicsSource, DynamicsTarget, Jitter,
    PRESET_SCHEMA_VERSION, PresetLoad, Texture, TextureMode, TextureSource, Tip, TipRotation, TipShape, is_asset_hash,
    load_preset_json, load_preset_value,
};
pub use renderer::{
    FinishedStroke, LayeredPart, MAX_DABS_PER_STROKE, MAX_SAMPLE_COORD, MIN_DAB_STEP_PX, PredictedCoverage,
    StrokeContext, StrokeMode, StrokeRenderer, StrokeStats, composite_layered,
};
pub use smudge::{FinishedSmudge, MAX_SMUDGE_SIZE_PX, SmudgeContext, SmudgeParams, SmudgeStroke};
