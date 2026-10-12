//! Brush-library commands on a [`Session`] (ADR-019, `docs/COMMANDS.md` "Brush library").
//!
//! Library changes are not document edits: no command here touches the document, its version or
//! the undo history. The library directory is owned by the app (`brush.library.open`).

use super::{EResult, Receipt, Session, internal};
use crate::brushlib::{AssetSpec, base64};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use veskra_brush::{BrushPreset, BrushSource, MaskChannel, StrokeContext, StrokeMode, StrokeRenderer};
use veskra_core::{ColorRgba, ErrorCode, RectI, SampleFlags, StrokeSample, VeskraError};
use veskra_raster::{Tile, TileGrid};

/// `brush.preview` bounds.
pub const MAX_PREVIEW_WIDTH: u32 = 1024;
pub const MAX_PREVIEW_HEIGHT: u32 = 256;
pub const MIN_PREVIEW_EDGE: u32 = 16;

/// One `brush.list` entry.
pub(super) fn list_entry(p: &BrushPreset, current: bool) -> Value {
    json!({
        "id": p.id, "name": p.name, "category": p.category.as_str(), "source": p.source.as_string(),
        "folder": p.folder, "vectorEligible": p.vector_eligible, "eraser": p.eraser, "version": p.version,
        "editable": p.source != BrushSource::Builtin, "current": current,
    })
}

/// Synthetic S-curve for previews: arithmetic only, pressure ramp 0.15 → 1 → 0.15, 8 ms apart.
/// The pressure is synthetic (`PRESSURE_REPORTED` cleared) and never stored anywhere.
fn preview_samples(w: u32, h: u32, size: f32) -> Vec<StrokeSample> {
    let n = 96;
    let (w, h) = (w as f64, h as f64);
    let mx = (size as f64 * 0.6 + 4.0).min(w * 0.25);
    let amp = (h * 0.5 - size as f64 * 0.55 - 2.0).max(0.0);
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let s = 10.392 * t * (1.0 - t) * (1.0 - 2.0 * t);
            let x = mx + (w - 2.0 * mx) * t;
            let y = h * 0.5 - amp * s;
            let p = 0.15 + 0.85 * 4.0 * t * (1.0 - t);
            let mut sm = StrokeSample::confirmed(x as f32, y as f32, i as u64 * 8_000_000, p as f32);
            sm.flags.remove(SampleFlags::PRESSURE_REPORTED);
            sm
        })
        .collect()
}

impl Session {
    pub(super) fn brush_library_open(&mut self, dir: &str) -> EResult<Receipt> {
        let (brushes, assets, warnings) = self.library.open(Path::new(dir))?;
        // The current brush follows its stored version if it is a library brush.
        if let Some(p) = self.library.get(&self.brush.id).filter(|_| self.library.is_user(&self.brush.id)) {
            self.brush = p;
        }
        Ok(self.receipt(json!({"dir": dir, "brushes": brushes, "assets": assets, "warnings": warnings})))
    }

    pub(super) fn brush_list(&self) -> Receipt {
        let list: Vec<Value> = self.library.list().iter().map(|p| list_entry(p, p.id == self.brush.id)).collect();
        self.receipt(json!({"presets": list, "current": self.brush_json()}))
    }

    pub(super) fn brush_get(&self, id: &str) -> EResult<Receipt> {
        let p = self
            .library
            .get(id)
            .ok_or_else(|| VeskraError::new(ErrorCode::NotFound, format!("unknown brush preset {id}")))?;
        let preset = serde_json::to_value(&p).map_err(internal)?;
        let assets: Vec<Value> = p
            .asset_refs()
            .iter()
            .map(|h| match self.library.find_asset(h) {
                Some(a) => json!({"hash": h, "width": a.width(), "height": a.height(), "available": true}),
                None => json!({"hash": h, "available": false}),
            })
            .collect();
        Ok(self.receipt(json!({"preset": preset, "editable": p.source != BrushSource::Builtin, "assets": assets})))
    }

    pub(super) fn brush_save(
        &mut self,
        preset: Value,
        assets: Option<BTreeMap<String, AssetSpec>>,
    ) -> EResult<Receipt> {
        let (p, warnings) = self.library.save(preset, assets.as_ref())?;
        if self.brush.id == p.id {
            self.brush = p.clone();
        }
        Ok(self.receipt(
            json!({"id": p.id, "version": p.version, "vectorEligible": p.vector_eligible, "warnings": warnings}),
        ))
    }

    pub(super) fn brush_delete(&mut self, id: &str) -> EResult<Receipt> {
        self.library.delete(id)?;
        // A deleted current brush stays usable for this session (its assets stay loaded).
        Ok(self.receipt(json!({"deleted": id})))
    }

    pub(super) fn brush_duplicate(&mut self, id: &str, name: Option<&str>) -> EResult<Receipt> {
        let p = self.library.duplicate(id, name)?;
        Ok(self.receipt(json!({"id": p.id, "name": p.name, "vectorEligible": p.vector_eligible})))
    }

    pub(super) fn brush_import(&mut self, path: &str) -> EResult<Receipt> {
        let r = self.library.import(Path::new(path))?;
        let imported: Vec<Value> = r
            .imported
            .iter()
            .map(|i| {
                json!({"id": i.id, "name": i.name, "folder": i.folder, "approximated": i.approximated,
                       "unsupported": i.unsupported})
            })
            .collect();
        let failed: Vec<Value> = r.failed.iter().map(|(n, why)| json!({"name": n, "reason": why})).collect();
        Ok(self.receipt(json!({"imported": imported, "failed": failed, "warnings": r.warnings})))
    }

    pub(super) fn brush_export(&self, id: &str, path: &str) -> EResult<Receipt> {
        let bytes = self.library.export(id, Path::new(path))?;
        Ok(self.receipt(json!({"path": path, "bytes": bytes})))
    }

    pub(super) fn brush_asset_add(
        &mut self,
        path: &str,
        channel: Option<MaskChannel>,
        invert: Option<bool>,
    ) -> EResult<Receipt> {
        let m = self.library.add_asset(Path::new(path), channel.unwrap_or_default(), invert.unwrap_or(false))?;
        let channel = serde_json::to_value(m.channel).map_err(internal)?;
        Ok(self.receipt(json!({
            "hash": m.asset.hash(), "width": m.asset.width(), "height": m.asset.height(),
            "channel": channel, "downscaledFrom": m.downscaled_from.map(|(w, h)| [w, h]),
        })))
    }

    /// `brush.preview`: a standard S-curve with a synthetic pressure ramp, current color, as a
    /// base64 PNG (straight-alpha sRGB RGBA8; PNG has no premultiplied mode).
    pub(super) fn brush_preview(&self, id: &str, width: u32, height: u32) -> EResult<Receipt> {
        if !(MIN_PREVIEW_EDGE..=MAX_PREVIEW_WIDTH).contains(&width)
            || !(MIN_PREVIEW_EDGE..=MAX_PREVIEW_HEIGHT).contains(&height)
        {
            return Err(VeskraError::invalid(format!(
                "preview size must be {MIN_PREVIEW_EDGE}..={MAX_PREVIEW_WIDTH} × {MIN_PREVIEW_EDGE}..={MAX_PREVIEW_HEIGHT}"
            )));
        }
        let p = self
            .library
            .get(id)
            .or_else(|| (self.brush.id == id).then(|| self.brush.clone()))
            .ok_or_else(|| VeskraError::new(ErrorCode::NotFound, format!("unknown brush preset {id}")))?;
        let clip = RectI::from_size(width, height);
        let size = p.size_px.min(height as f32 * 0.45).max(1.0);
        let mut base = TileGrid::new();
        if p.eraser {
            let gray = std::sync::Arc::new(Tile::new_filled(veskra_raster::ops::color_to_px16(ColorRgba::from_rgba8(
                200, 200, 200, 255,
            ))));
            for c in TileGrid::tiles_in_rect(clip) {
                base.set_tile(c, gray.clone());
            }
        }
        let ctx = StrokeContext {
            size_override: Some(size),
            ..StrokeContext::new(if p.eraser { StrokeMode::Erase } else { StrokeMode::Paint }, clip)
        };
        let assets = self.library.resolve(&p);
        let mut r = StrokeRenderer::begin_resolved(p, assets, self.color.with_alpha(1.0), 0x5EED, base, ctx);
        r.add_samples(&preview_samples(width, height, size));
        let fin = r.finish();
        let px = fin.tiles.read_rect(clip);
        let mut rgba = vec![0u8; px.len()];
        veskra_color::convert::premul16_to_straight8(&px, &mut rgba);
        let png = veskra_io::png::encode_rgba8_srgb(width, height, &rgba)?;
        Ok(self.receipt(json!({"png": base64(&png), "width": width, "height": height, "sizePx": size,
                               "synthetic": true})))
    }
}
