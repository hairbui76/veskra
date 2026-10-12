//! # veskra-format — versioned `.veskra` native bundle, integrity, recovery (FMT, ADR-009)
//!
//! ## Container (schema 1)
//! A ZIP archive (own bounded implementation, [`zip`]) with, in this order:
//!
//! | entry | method | content |
//! |---|---|---|
//! | `mimetype` | STORE, first | `application/x-veskra` |
//! | `manifest.json` | DEFLATE | [`Manifest`]: format, schema, minReader, appVersion, created, documentId, tileSize, pixel, entries |
//! | `brushes/<hash>.png` (schema 2), `preview/composite.png`, `profiles/<sha256>.icc`, `project/document.json`, `raster/<layer>/<tx>_<ty>.tile`, `masks/<layer>/<tx>_<ty>.mask`, `selection/<tx>_<ty>.mask`, `strokes/<id>.bin` | sorted by name; PNG stored, rest DEFLATE | see below |
//! | `integrity.json` | DEFLATE, last | SHA-256 + uncompressed length of every other entry |
//!
//! * `.tile`: 64×64×4 `u16` little-endian premultiplied RGBA in the document working profile
//!   ([`veskra_core::PixelDescriptor::WORKING_V1`]); fully transparent tiles are omitted.
//! * `.mask`: 64×64 `u8` coverage; tiles uniformly equal to the mask default are omitted.
//! * `strokes/<id>.bin`: `"VSKS"`, version u16 = 1, count u32, then 34-byte samples
//!   (see [`encode_samples`]); only confirmed samples load (enforced by `StrokeRecord::new`).
//! * ZIP headers use a fixed timestamp, so identical documents encode to identical bytes.
//!
//! ## Schema 2 (brush system v2, ADR-019)
//! Schema 2 is a strict superset of schema 1: `project/document.json` gains `brushAssets` (the
//! content hashes of embedded brush assets) and each stroke's `brush` gains `assets` (the hashes
//! its preset references); every embedded asset is a `brushes/<hash>.png` entry (STORE, canonical
//! 8-bit grayscale PNG, at most `veskra_doc::MAX_BRUSH_ASSET_BYTES`). The writer emits schema 2
//! (with `minReader` 1) only when the document embeds brush assets; otherwise it writes exactly
//! the schema-1 bytes it always wrote. A schema-1 reader opens a schema-2 file with
//! `NewerSchema` / unknown-field / unknown-entry warnings: the pixels are complete, only the
//! embedded assets (needed to *re-render* image-tip strokes) are not preserved by its re-save.
//! This reader treats a stroke reference with no embedded asset as
//! [`FormatWarning::MissingBrushAssets`] (pixels stay authoritative; re-rendering falls back to a
//! round tip), and a listed asset whose entry is missing, oversized or not a PNG as a typed error.
//!
//! ## Compatibility policy
//! * `schema` 1 and 2 are read natively (no migration step: 2 only adds optional data).
//! * `schema > 2` with `minReader <= 2`: opens with [`FormatWarning::NewerSchema`]; unknown JSON
//!   fields/entries are reported ([`FormatWarning::UnknownFields`] / [`FormatWarning::UnknownEntries`])
//!   and are **not** preserved if the document is re-saved.
//! * `minReader > 2`: [`FormatError::UnsupportedSchema`]; nothing is modified.
//! * `schema < 1`: migrated through [`migrate::MIGRATIONS`] (`vN -> vN+1`), or `UnsupportedSchema`.
//!
//! ## Saving
//! [`save_atomic`] stages, fsyncs, re-reads and validates, then renames (see [`save`] docs);
//! [`recovery::AutosaveStore`] keeps bounded validated checkpoints.

pub mod codec;
mod dto;
pub mod error;
pub mod migrate;
pub mod recovery;
pub mod save;
pub mod zip;

pub use codec::{
    BundleInfo, EntryInfo, LoadedDocument, MAX_JSON_BYTES, MAX_TILE_COORD, SaveMeta, decode, decode_samples,
    decode_tile, encode, encode_samples, encode_tile, encode_to, inspect, read_preview,
};
pub use dto::Manifest;
pub use error::{FormatError, FormatWarning};
pub use recovery::{AutosaveStore, CheckpointInfo};
pub use save::{SaveReport, SaveSink, load, remove_stale_temps, save_atomic, save_atomic_with};

/// Newest schema this reader understands natively (and writes when a document embeds brush
/// assets).
pub const CURRENT_SCHEMA: u32 = 2;
/// Oldest schema read without a migration step (schemas `MIN_NATIVE_SCHEMA..=CURRENT_SCHEMA`).
pub const MIN_NATIVE_SCHEMA: u32 = 1;
/// `manifest.format` value.
pub const FORMAT_NAME: &str = "veskra";
/// Content of the `mimetype` entry.
pub const MIMETYPE: &str = "application/x-veskra";
/// `manifest.pixel` value for schema 1 tiles.
pub const PIXEL_V1: &str = "rgba16-premul-encoded";
/// Conventional file extension.
pub const FILE_EXTENSION: &str = "veskra";
