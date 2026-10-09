//! Layer node schema (MASTER_PLAN §5.4): Raster, Vector-stroke and Group variants.

use serde::{Deserialize, Serialize};
use veskra_core::{BlendMode, LayerId, StrokeId};
use veskra_raster::{MaskGrid, TileGrid};

/// User-visible layer properties shared by all node kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerProps {
    pub name: String,
    pub visible: bool,
    /// `[0, 1]`.
    pub opacity: f32,
    pub blend: BlendMode,
    /// Locked layers reject all pixel and property edits except unlocking/visibility.
    pub locked: bool,
    /// Alpha lock: painting only changes color where alpha already exists.
    pub alpha_locked: bool,
    /// Clipping: this layer is clipped to the alpha of the nearest non-clipped sibling below it.
    pub clip_to_below: bool,
}

impl LayerProps {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            locked: false,
            alpha_locked: false,
            clip_to_below: false,
        }
    }
}

/// Whether a pixel layer's tiles are exactly the rendering of its recorded strokes, which
/// decides if its strokes may be exported as editable vectors (VEC-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorFidelity {
    /// Pixels are exactly the strokes; every stroke has a vector form.
    Exact,
    /// Pixels are exactly the strokes, but some strokes are raster-only (texture, airbrush, eraser).
    PartiallyVector,
    /// Raster operations (fill, transform, imported pixels, ...) changed pixels independently of
    /// the stroke list; vectors would misrepresent the layer. Reasons are user-facing keys.
    RasterAuthoritative { reasons: Vec<String> },
}

/// Pixel-bearing layer content (used by both Raster and Vector-stroke layers).
#[derive(Debug, Clone, PartialEq)]
pub struct PixelContent {
    /// Authoritative committed pixels (RGBA16 premultiplied, working profile).
    pub tiles: TileGrid,
    /// Optional layer mask (255 = visible). `None` = no mask.
    pub mask: Option<MaskGrid>,
    /// Strokes committed to this layer, in paint order.
    pub strokes: Vec<StrokeId>,
    /// Raster edits that are not strokes (fill/transform/import/clear), as reason keys.
    pub raster_edits: Vec<String>,
}

impl PixelContent {
    pub fn empty() -> Self {
        Self { tiles: TileGrid::new(), mask: None, strokes: Vec::new(), raster_edits: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupContent {
    /// Child nodes, bottom-most first.
    pub children: Vec<LayerId>,
    /// Pass-through groups composite children directly into the parent backdrop; isolated
    /// groups (default, PSD-compatible) composite children into a transparent buffer first.
    pub pass_through: bool,
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayerKind {
    /// General raster layer: any brush, fill, selection edit.
    Raster(PixelContent),
    /// Vector-stroke layer: accepts only vector-eligible strokes; never receives non-stroke
    /// raster edits, so its strokes are always faithfully exportable as vectors.
    Vector(PixelContent),
    Group(GroupContent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKindTag {
    Raster,
    Vector,
    Group,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub id: LayerId,
    pub parent: Option<LayerId>,
    pub props: LayerProps,
    pub kind: LayerKind,
}

impl Layer {
    pub fn tag(&self) -> LayerKindTag {
        match self.kind {
            LayerKind::Raster(_) => LayerKindTag::Raster,
            LayerKind::Vector(_) => LayerKindTag::Vector,
            LayerKind::Group(_) => LayerKindTag::Group,
        }
    }

    pub fn pixels(&self) -> Option<&PixelContent> {
        match &self.kind {
            LayerKind::Raster(p) | LayerKind::Vector(p) => Some(p),
            LayerKind::Group(_) => None,
        }
    }

    pub fn pixels_mut(&mut self) -> Option<&mut PixelContent> {
        match &mut self.kind {
            LayerKind::Raster(p) | LayerKind::Vector(p) => Some(p),
            LayerKind::Group(_) => None,
        }
    }

    pub fn group(&self) -> Option<&GroupContent> {
        match &self.kind {
            LayerKind::Group(g) => Some(g),
            _ => None,
        }
    }

    pub fn group_mut(&mut self) -> Option<&mut GroupContent> {
        match &mut self.kind {
            LayerKind::Group(g) => Some(g),
            _ => None,
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self.kind, LayerKind::Group(_))
    }
}
