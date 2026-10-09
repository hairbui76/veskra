//! # veskra-core — frozen cross-crate contracts (ARC-01)
//!
//! This crate holds the small set of types every other Veskra crate agrees on.
//! It deliberately has no knowledge of tiles, documents, Android or file formats.
//!
//! Changing any public type here is a **contract change** and requires an ADR
//! (see `adr/ADR-016-core-contracts.md`). Additive changes (new enum variants
//! behind `#[non_exhaustive]`, new helper functions) are allowed with ORCH review.
//!
//! Conventions fixed by this crate:
//! * Document coordinates: `f32`/`f64` pixels, origin top-left, +x right, +y down.
//! * Pixel rectangles ([`RectI`]) are half-open: `x0..x1`, `y0..y1`.
//! * Colors handed between crates are [`ColorRgba`]: **straight (non-premultiplied)
//!   alpha, encoded in the document working space** (sRGB transfer for v1).
//! * Raster storage format is described by [`PixelDescriptor`]; the v1 authoritative
//!   layer format is [`PixelDescriptor::WORKING_V1`] (RGBA u16, premultiplied,
//!   sRGB-encoded) — see ADR-005.

pub mod blend;
pub mod color;
pub mod error;
pub mod export;
pub mod geom;
pub mod ids;
pub mod limits;
pub mod pixel;
pub mod sample;

pub use blend::BlendMode;
pub use color::ColorRgba;
pub use error::{ErrorCode, VeskraError};
pub use geom::{Affine, PointF, RectF, RectI, SizeU};
pub use ids::{DocumentId, JobId, LayerId, SessionId, StrokeId};
pub use pixel::{
    AlphaRepresentation, BlendSpace, ChannelModel, PixelDepth, PixelDescriptor, ProfileId, SampleEncoding, TILE_SIZE,
};
pub use sample::{SampleFlags, StrokeSample, ToolType};
