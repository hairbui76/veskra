//! # veskra-doc — document model (CORE-03, DOC-01, DOC-04, BRH-08, VEC-03)
//!
//! A [`Document`] is a persistent (structurally shared) value:
//! * layers are `Arc<Layer>` and raster content is a copy-on-write [`TileGrid`], so cloning a
//!   document costs O(layers) pointer copies and mutating one tile copies only that tile;
//! * [`History`] stores whole-document snapshots (`Arc<Document>`). Undo/redo is therefore
//!   exact by construction (PNT-009) and memory cost is only the tiles that differ.
//!
//! Tree invariants (checked by [`Document::validate`]): every layer appears exactly once in the
//! tree, parent links agree with child lists, no cycles, group depth ≤ `MAX_GROUP_DEPTH`,
//! stroke references resolve, and names are within limits.
//!
//! Paint order: `root[0]` is the **bottom-most** node; children of a group likewise bottom→top.

mod document;
mod history;
mod layer;
mod stroke;

pub use document::{CanvasMap, DocColor, Document, DocumentError, DocumentParts, LayerPatch, Placement};
pub use history::{History, HistoryConfig, HistoryStats};
pub use layer::{GroupContent, Layer, LayerKind, LayerKindTag, LayerProps, PixelContent, VectorFidelity};
pub use stroke::{StrokeComposition, StrokeGeometry, StrokeMode, StrokeRecord};

pub use veskra_raster::{MaskGrid, TileGrid};
