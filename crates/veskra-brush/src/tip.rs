//! Brush tip preview (ADR-019 amendment 1, `brush.tip_preview`): the footprint of **one dab** of
//! a preset, centered and scaled to fill a square image.
//!
//! What is drawn: the tip (round with its hardness, or the image mask) at the preset's default
//! `tip.angle` and `tip.roundness` (a `direction` tip is shown moving right, a `pen` tip without
//! azimuth), at full strength. A texture whose `mode` is `follow` is applied at its own `depth`,
//! scaled with the tip (its size relative to the dab is the one at the preset's `size_px`).
//! Not drawn: `fixed` (paper) textures, flow, opacity, dynamics, jitter, wet edges and color
//! jitter. A tip image the caller could not resolve is drawn as the documented round fallback
//! ([`TipPreview::tip_fallback`]). The same dab rasterizer as strokes is used
//! ([`crate::stamp`]), so the footprint is exactly what one dab of a stroke covers.

use veskra_core::RectI;

use crate::asset::ResolvedAssets;
use crate::coverage::CoverageGrid;
use crate::preset::{BrushPreset, TextureMode, TextureSource, TipShape};
use crate::stamp::{self, Buffers, Dab, StampStyle, TexKind, TexStyle};
use crate::{Accumulation, BrushError};

/// Largest tip preview side (px).
pub const MAX_TIP_PREVIEW_PX: u32 = 2048;
/// Seed of a dab-space texture in tip previews (any fixed value; previews are deterministic).
const TIP_TEXTURE_SEED: u64 = 0x5EED;
/// Free margin (px) around the footprint.
const MARGIN_PX: f64 = 1.0;

/// Result of [`render_tip`].
#[derive(Debug, Clone, PartialEq)]
pub struct TipPreview {
    /// Side of the square (px).
    pub size: u32,
    /// Coverage `0..=255`, row-major, `size × size`.
    pub coverage: Vec<u8>,
    /// Dab radius used (px).
    pub radius: f64,
    /// A dab-space (`follow`) texture was applied.
    pub texture_applied: bool,
    /// The preset's tip image was not resolved; the round fallback was drawn.
    pub tip_fallback: bool,
}

/// Render the tip footprint of `preset` (see the module docs) into a `size × size` coverage
/// mask, `1..=MAX_TIP_PREVIEW_PX`. `assets` are the preset's resolved images. Invalid presets are
/// [`BrushPreset::sanitized`] like strokes; nothing panics.
pub fn render_tip(preset: &BrushPreset, assets: &ResolvedAssets, size: u32) -> Result<TipPreview, BrushError> {
    if size == 0 || size > MAX_TIP_PREVIEW_PX {
        return Err(BrushError::InvalidArgument("tip preview size must be 1..=2048 px"));
    }
    let p = preset.sanitized();
    let tip = assets.tip.clone().filter(|_| p.tip.shape == TipShape::Image);
    let tip_fallback = p.tip.shape == TipShape::Image && tip.is_none();
    let rho = f64::from(p.tip.roundness).clamp(0.05, 1.0);
    let (sin, cos) = f64::from(p.tip.angle).to_radians().sin_cos();
    let half = f64::from(size) / 2.0 - MARGIN_PX;
    // Half extents of the rotated footprint per unit radius (rotation of the tip frame, see
    // `stamp`): the image rectangle, or the ellipse plus the 0.5-px antialiasing rim.
    let radius = match &tip {
        Some(a) => {
            let m = f64::from(a.width().max(a.height()));
            let (hw, hh) = (f64::from(a.width()) / m, f64::from(a.height()) / m * rho);
            let ext = (hw * cos.abs() + hh * sin.abs()).max(hw * sin.abs() + hh * cos.abs());
            half / ext
        }
        None => {
            let ext = (cos * cos + rho * rho * sin * sin).sqrt().max((sin * sin + rho * rho * cos * cos).sqrt());
            half / ext - 0.5
        }
    }
    .max(0.5);
    let k = 2.0 * radius / f64::from(p.size_px);
    let texture = p.texture.as_ref().filter(|t| t.mode == TextureMode::Follow).map(|t| {
        let scale = f64::from(t.scale) * k;
        let kind = match (t.source, &assets.texture) {
            (TextureSource::Image, Some(a)) => {
                let level = a.level_index_for(scale);
                let lw = f64::from(a.levels()[level].w);
                TexKind::Image { asset: a.clone(), level, texel: scale * f64::from(a.width()) / lw }
            }
            (TextureSource::Image, None) => TexKind::Procedural { scale: f64::from(t.scale).clamp(0.5, 256.0) * k },
            (TextureSource::Procedural, _) => TexKind::Procedural { scale },
        };
        (TexStyle { kind, mode: TextureMode::Follow, invert: t.invert, seed: TIP_TEXTURE_SEED }, f64::from(t.depth))
    });
    let texture_applied = texture.is_some();
    let (texture, tex_depth) = match texture {
        Some((t, d)) => (Some(t), d),
        None => (None, 0.0),
    };
    let style = StampStyle {
        tip,
        hardness: f64::from(p.tip.hardness),
        roundness: rho,
        accumulation: Accumulation::Max,
        texture,
    };
    let c = f64::from(size) / 2.0;
    let dab = Dab { cx: c, cy: c, radius, alpha: 1.0, cos, sin, tex_depth, color: None };
    let mut cov = CoverageGrid::default();
    let clip = RectI::from_size(size, size);
    stamp::stamp(&mut Buffers { cov: &mut cov, depth: None, color: None }, &style, &dab, clip);
    let n = size as i32;
    let mut coverage = Vec::with_capacity(size as usize * size as usize);
    for y in 0..n {
        for x in 0..n {
            coverage.push(((u32::from(cov.get(x, y)) * 255 + 32_767) / 65_535) as u8);
        }
    }
    Ok(TipPreview { size, coverage, radius, texture_applied, tip_fallback })
}

impl TipPreview {
    /// Straight-alpha RGBA8 pixels: `rgb` with alpha = coverage; fully transparent pixels are
    /// `0, 0, 0, 0`.
    pub fn rgba(&self, rgb: [u8; 3]) -> Vec<u8> {
        self.coverage.iter().flat_map(|&a| if a == 0 { [0; 4] } else { [rgb[0], rgb[1], rgb[2], a] }).collect()
    }

    /// [`TipPreview::rgba`] as an sRGB-tagged 8-bit RGBA PNG (straight alpha). Uses the fast
    /// deflate setting: previews are transient, many are made at once (a brush picker), and
    /// compression is ~10× faster than the export writer's while the files stay small.
    pub fn png(&self, rgb: [u8; 3]) -> Result<Vec<u8>, BrushError> {
        let err = |e: png::EncodingError| BrushError::InvalidAsset(format!("PNG encode: {e}"));
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.size, self.size);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
            enc.set_compression(png::Compression::Fast);
            let mut wr = enc.write_header().map_err(err)?;
            wr.write_image_data(&self.rgba(rgb)).map_err(err)?;
            wr.finish().map_err(err)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TipRotation, builtin_assets, builtin_preset, builtin_presets};

    fn bbox(t: &TipPreview) -> (i32, i32, i32, i32) {
        let n = t.size as i32;
        let (mut x0, mut y0, mut x1, mut y1) = (n, n, -1, -1);
        for y in 0..n {
            for x in 0..n {
                if t.coverage[(y * n + x) as usize] > 0 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
        }
        (x0, y0, x1, y1)
    }

    #[test]
    fn every_builtin_fills_the_square_without_clipping() {
        for p in builtin_presets() {
            let assets = ResolvedAssets::resolve(&p, &[builtin_assets()]);
            let t = render_tip(&p, &assets, 128).unwrap();
            assert!(!t.tip_fallback, "{}", p.id);
            let (x0, y0, x1, y1) = bbox(&t);
            assert!(x1 >= x0 && y1 >= y0, "{}: empty tip", p.id);
            // Inside the image with the margin kept, and filling it along one axis at least
            // (image masks may leave transparent texels at their own border).
            assert!(x0 >= 0 && y0 >= 0 && x1 <= 127 && y1 <= 127, "{}: {:?}", p.id, (x0, y0, x1, y1));
            let painted = t.coverage.iter().filter(|v| **v > 0).count();
            assert!(painted > 60, "{}: {painted}", p.id);
            if p.tip.shape == TipShape::Round {
                assert!(x1 - x0 >= 120 || y1 - y0 >= 120, "{}: {:?}", p.id, (x0, y0, x1, y1));
            }
            assert_eq!(t.texture_applied, p.texture.as_ref().is_some_and(|t| t.mode == TextureMode::Follow));
            assert_eq!(render_tip(&p, &assets, 128).unwrap(), t, "{}: not deterministic", p.id);
        }
    }

    #[test]
    fn angle_roundness_and_fallback() {
        // Flat nib: a thin ellipse at 40° ccw → the footprint runs from bottom-left to top-right.
        let p = builtin_preset("calligraphy.flat_nib").unwrap();
        let t = render_tip(&p, &ResolvedAssets::default(), 64).unwrap();
        let at = |x: usize, y: usize| t.coverage[y * 64 + x];
        assert!(at(52, 13) > 200 && at(12, 51) > 200, "ends of the nib");
        assert_eq!(at(12, 12), 0);
        assert_eq!(at(52, 51), 0);
        // Hard round: full coverage at the center, nothing in the corners.
        let r = render_tip(&builtin_preset("round.hard").unwrap(), &ResolvedAssets::default(), 32).unwrap();
        assert_eq!(r.coverage[16 * 32 + 16], 255);
        assert_eq!(r.coverage[0], 0);
        // A missing tip image falls back to the round tip, flagged.
        let mut q = builtin_preset("nature.grass").unwrap();
        q.tip.rotation = TipRotation::Fixed;
        let f = render_tip(&q, &ResolvedAssets::default(), 32).unwrap();
        assert!(f.tip_fallback);
        assert!(matches!(render_tip(&q, &ResolvedAssets::default(), 0), Err(BrushError::InvalidArgument(_))));
        assert!(matches!(render_tip(&q, &ResolvedAssets::default(), 4096), Err(BrushError::InvalidArgument(_))));
    }
}
