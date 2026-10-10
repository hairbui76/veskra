//! Canvas geometry changes: crop / extend (integer offset) and resampling resize.
//!
//! Both operations are document-wide and atomic: every pixel layer (including locked and hidden
//! ones), every layer mask, the selection and every stroke record (samples, fitted centerline,
//! outline, bounds) are mapped through the same transform, so the vector export of a stroke
//! still lines up with its pixels afterwards. Layer `raster_edits` are **not** touched: a
//! vector-representable layer stays vector-representable.
//!
//! * [`CanvasMap::Offset`]: content moves by whole pixels and is trimmed to the new canvas; pixel
//!   values, mask values and Bézier coordinates are exact. A stroke that is partly cropped keeps
//!   its full outline (the part outside the canvas is not visible in exports).
//! * [`CanvasMap::Resample`]: pixels and masks are resampled with
//!   [`veskra_raster::resample`] (box filter down, bilinear up, premultiplied); stroke geometry
//!   is scaled exactly, so outlines and resampled pixels agree to within the resampling blur.
//!   Stroke samples are scaled too but keep their recorded pressure/brush parameters, which are
//!   provenance only (committed pixels are authoritative).

use super::Document;
use std::sync::Arc;
use veskra_core::{Affine, SizeU, VeskraError, limits};
use veskra_raster::{resample, transform};

/// How document coordinates map onto the new canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasMap {
    /// `new = old + (dx, dy)` (crop / extend). Content outside the new canvas is discarded.
    Offset { dx: i32, dy: i32 },
    /// Scale the whole old canvas onto the whole new canvas.
    Resample,
}

impl Document {
    /// Replace the canvas size, mapping all content as described by `map`. On error the document
    /// is unchanged. Sizes are checked against [`limits::check_document_size`].
    pub fn remap_canvas(&mut self, new_size: SizeU, map: CanvasMap) -> Result<(), VeskraError> {
        limits::check_document_size(new_size.width, new_size.height)?;
        let old = self.size;
        let (src, dst) = ((old.width, old.height), (new_size.width, new_size.height));
        let affine = match map {
            CanvasMap::Offset { dx, dy } => {
                let lim = limits::MAX_DOCUMENT_DIM as i64;
                if (dx as i64).abs() > lim || (dy as i64).abs() > lim {
                    return Err(VeskraError::invalid("canvas offset out of range"));
                }
                Affine::translate(dx as f64, dy as f64)
            }
            CanvasMap::Resample => Affine::scale(dst.0 as f64 / src.0 as f64, dst.1 as f64 / src.1 as f64),
        };
        let mut next = self.clone();
        next.size = new_size;
        let clip = next.bounds();
        for layer in next.layers.values_mut() {
            if layer.pixels().is_none() {
                continue;
            }
            let Some(px) = Arc::make_mut(layer).pixels_mut() else { continue };
            match map {
                CanvasMap::Offset { .. } => {
                    px.tiles = transform::try_transform_pixels(&px.tiles, None, affine, clip)?;
                    if let Some(m) = &px.mask {
                        px.mask = Some(transform::try_transform_mask(m, affine, clip)?);
                    }
                }
                CanvasMap::Resample => {
                    px.tiles = resample::resample_pixels(&px.tiles, src, dst)?;
                    if let Some(m) = &px.mask {
                        px.mask = Some(resample::resample_mask(m, src, dst)?);
                    }
                }
            }
        }
        if let Some(sel) = next.selection.take() {
            let moved = match map {
                CanvasMap::Offset { .. } => transform::try_transform_mask(&sel, affine, clip)?,
                CanvasMap::Resample => resample::resample_mask(&sel, src, dst)?,
            };
            next.selection = if moved.is_empty() { None } else { Some(moved) };
        }
        for rec in next.strokes.values_mut() {
            *rec = Arc::new(rec.transformed(&affine)?);
        }
        next.validate()?;
        *self = next;
        Ok(())
    }
}
