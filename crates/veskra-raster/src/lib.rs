//! veskra-raster: sparse copy-on-write tile store, masks, selections, fills, resampling (REN).
//!
//! Pixel conventions (CONTRACTS §1, ADR-005):
//! * Layer pixels are [`Px16`] = `[u16; 4]` RGBA, **premultiplied**, transfer-encoded in the
//!   document working profile, layout [`veskra_core::PixelDescriptor::WORKING_V1`].
//!   Invariant `r, g, b <= a` holds after every operation in this crate.
//! * Masks/selections/fill regions are `u8` coverage ([`veskra_core::PixelDescriptor::MASK_U8`]).
//! * Tiles are `TILE x TILE` (64) pixels; tile coordinates use floor division so negative
//!   document coordinates map to negative tiles (`x = -1` lives in tile `-1`, local `63`).
//!
//! Storage is sparse and copy-on-write: a [`TileGrid`] is a `BTreeMap<TileCoord, Arc<Tile>>`.
//! Cloning a grid clones `Arc`s only; mutation copies only the touched tiles
//! (`Arc::make_mut`), which is what undo snapshots rely on.

mod error;
mod mask;
mod tile;

pub mod fill;
pub mod ops;
pub mod select;
pub mod transform;

pub use error::RasterError;
pub use mask::{MASK_OP_LIMIT, MaskGrid, MaskOp, MaskTile};
pub use tile::{TILE_BYTES, TILE_LEN, TILE_PIXELS, Tile, TileCoord, TileGrid};

/// Premultiplied RGBA16 pixel (CONTRACTS §1).
pub type Px16 = [u16; 4];

/// Tile edge in pixels (re-export of [`veskra_core::TILE_SIZE`]).
pub const TILE: u32 = veskra_core::TILE_SIZE;

/// Tile edge as `i32`/`usize` helpers for internal index math.
pub(crate) const TILE_I: i32 = TILE as i32;
pub(crate) const TILE_U: usize = TILE as usize;

/// Maximum pixel count of a rectangle that `read_rect` will materialize and that
/// fill will scan ([`veskra_core::limits::MAX_DOCUMENT_PIXELS`], 8192²).
pub const MAX_RECT_PIXELS: u64 = veskra_core::limits::MAX_DOCUMENT_PIXELS;

/// Rectangle meaning "unbounded" (returned by [`MaskGrid::bounds`] for masks whose
/// default value outside materialized tiles is nonzero). Always intersect with a real clip.
pub const UNBOUNDED: veskra_core::RectI = veskra_core::RectI::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
