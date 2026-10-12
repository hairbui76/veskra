//! Serde data-transfer objects for the JSON entries of a schema-1 bundle.
//!
//! These types are owned by the format (not by `veskra-doc`) so that the on-disk schema only
//! changes through a schema bump + migration. Field names are camelCase.

use serde::{Deserialize, Serialize};
use veskra_core::{DocumentId, LayerId, ProfileId, RectI, StrokeId};
use veskra_vector::BezPath;

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Always `"veskra"`.
    pub format: String,
    /// Schema version of this bundle.
    pub schema: u32,
    /// Oldest reader schema that can open this bundle without losing *meaning*
    /// (forward-compat policy: readers with `CURRENT_SCHEMA >= minReader` may open it).
    pub min_reader: u32,
    pub app_version: String,
    /// Caller-supplied creation timestamp (free-form, usually RFC 3339).
    pub created: String,
    pub document_id: DocumentId,
    /// Raster tile edge (64 in schema 1).
    pub tile_size: u32,
    /// Raster tile payload layout (`"rgba16-premul-encoded"` in schema 1).
    pub pixel: String,
    /// Every payload entry name except `mimetype`, `manifest.json` and `integrity.json`, sorted.
    pub entries: Vec<String>,
}

/// `integrity.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Integrity {
    pub algorithm: String,
    pub entries: Vec<IntegrityEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntegrityEntry {
    pub name: String,
    /// Uncompressed length in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the uncompressed bytes.
    pub sha256: String,
}

/// `project/document.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentDto {
    pub id: DocumentId,
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub color: ColorDto,
    /// Root node ids, bottom-most first.
    pub root: Vec<LayerId>,
    /// All layer nodes in id order.
    pub layers: Vec<LayerDto>,
    /// All stroke records in id order.
    pub strokes: Vec<StrokeDto>,
    pub selection: Option<MaskDto>,
    pub active_layer: Option<LayerId>,
    /// Schema 2: content hashes of the embedded brush assets (`brushes/<hash>.png`), sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brush_assets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColorDto {
    /// SHA-256 of the ICC bytes + semantic name.
    pub profile: ProfileId,
    /// `profiles/<sha256>.icc`.
    pub profile_entry: String,
    pub assumed: bool,
    /// `"gammaEncoded"` or `"linear"`.
    pub blend_space: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LayerDto {
    pub id: LayerId,
    pub parent: Option<LayerId>,
    /// `"raster"`, `"vector"` or `"group"`.
    pub kind: String,
    pub props: PropsDto,
    /// Present iff `kind == "group"`.
    pub group: Option<GroupDto>,
    /// Present iff `kind` is `"raster"` or `"vector"`.
    pub pixels: Option<PixelsDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PropsDto {
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    /// `BlendMode::as_str` name.
    pub blend: String,
    pub locked: bool,
    pub alpha_locked: bool,
    pub clip_to_below: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupDto {
    pub children: Vec<LayerId>,
    pub pass_through: bool,
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PixelsDto {
    /// Stroke ids in paint order.
    pub strokes: Vec<StrokeId>,
    pub raster_edits: Vec<String>,
    /// Stored (non-transparent) tiles as `[tx, ty]`; each has `raster/<layer>/<tx>_<ty>.tile`.
    pub tiles: Vec<[i32; 2]>,
    pub mask: Option<MaskDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MaskDto {
    /// Coverage outside stored tiles: 0 or 255.
    pub default: u8,
    /// Stored tiles as `[tx, ty]` (tiles uniformly equal to `default` are omitted).
    pub tiles: Vec<[i32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StrokeDto {
    pub id: StrokeId,
    pub layer: LayerId,
    pub brush: BrushDto,
    /// Straight-alpha encoded RGBA in `[0, 1]`.
    pub color: [f32; 4],
    pub seed: u64,
    pub profile: ProfileId,
    pub composition: CompositionDto,
    pub vector_eligible: bool,
    pub geometry: Option<GeometryDto>,
    pub bounds: RectI,
    /// `strokes/<id>.bin`.
    pub sample_entry: String,
    pub sample_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrushDto {
    pub id: String,
    pub version: u32,
    /// Opaque brush parameter string owned by veskra-brush.
    pub params: String,
    /// Schema 2: brush assets the parameters reference (content hashes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompositionDto {
    /// `"paint"` or `"erase"`.
    pub mode: String,
    pub opacity: f32,
    pub alpha_locked: bool,
    pub selection_clipped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeometryDto {
    pub centerline: BezPath,
    pub outline: BezPath,
    pub fit_error: f64,
}
