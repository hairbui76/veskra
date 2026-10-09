//! # veskra-format — versioned `.veskra` native bundle, integrity, recovery (FMT, ADR-009)
//!
//! ## Container (schema 1)
//! A ZIP archive (own bounded implementation, [`zip`]) with, in this order:
//!
//! | entry | method | content |
//! |---|---|---|
//! | `mimetype` | STORE, first | `application/x-veskra` |
//! | `manifest.json` | DEFLATE | [`Manifest`]: format, schema, minReader, appVersion, created, documentId, tileSize, pixel, entries |
//! | `preview/composite.png`, `profiles/<sha256>.icc`, `project/document.json`, `raster/<layer>/<tx>_<ty>.tile`, `masks/<layer>/<tx>_<ty>.mask`, `selection/<tx>_<ty>.mask`, `strokes/<id>.bin` | sorted by name; PNG stored, rest DEFLATE | see below |
//! | `integrity.json` | DEFLATE, last | SHA-256 + uncompressed length of every other entry |
//!
//! * `.tile`: 64×64×4 `u16` little-endian premultiplied RGBA in the document working profile
//!   ([`veskra_core::PixelDescriptor::WORKING_V1`]); fully transparent tiles are omitted.
//! * `.mask`: 64×64 `u8` coverage; tiles uniformly equal to the mask default are omitted.
//! * `strokes/<id>.bin`: `"VSKS"`, version u16 = 1, count u32, then 34-byte samples
//!   (see [`encode_samples`]); only confirmed samples load (enforced by `StrokeRecord::new`).
//! * ZIP headers use a fixed timestamp, so identical documents encode to identical bytes.
//!
//! ## Compatibility policy
//! * `schema > 1` with `minReader <= 1`: opens with [`FormatWarning::NewerSchema`]; unknown JSON
//!   fields/entries are reported ([`FormatWarning::UnknownFields`] / [`FormatWarning::UnknownEntries`])
//!   and are **not** preserved if the document is re-saved.
//! * `minReader > 1`: [`FormatError::UnsupportedSchema`]; nothing is modified.
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

/// Schema version written by (and natively understood by) this reader.
pub const CURRENT_SCHEMA: u32 = 1;
/// `manifest.format` value.
pub const FORMAT_NAME: &str = "veskra";
/// Content of the `mimetype` entry.
pub const MIMETYPE: &str = "application/x-veskra";
/// `manifest.pixel` value for schema 1 tiles.
pub const PIXEL_V1: &str = "rgba16-premul-encoded";
/// Conventional file extension.
pub const FILE_EXTENSION: &str = "veskra";
