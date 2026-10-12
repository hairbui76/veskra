//! Brush assets (ADR-019): 8-bit coverage masks used as image tips and image textures.
//!
//! * **Identity.** An asset is identified by its *content hash*:
//!   `SHA-256("VSKMASK1" ‖ width u32 LE ‖ height u32 LE ‖ mask bytes)` as 64 lowercase hex.
//!   It does not depend on how the PNG that carries the mask was compressed, so the same mask
//!   always has the same id and any carrier is verified by decoding and re-hashing it.
//! * **Storage.** Assets are stored (in `.vbrush` files and `.veskra` documents) as 8-bit
//!   grayscale PNGs whose gray value is the coverage (255 = full paint). [`BrushAsset::png_bytes`]
//!   is that canonical carrier.
//! * **Import.** [`BrushAsset::from_png`] accepts any PNG (gray, gray+alpha, RGB, RGBA, palette,
//!   1..16 bit, interlaced) and converts it with a [`MaskChannel`] (+ optional invert). Inputs are
//!   bounded: at most [`MAX_ASSET_PNG_BYTES`] encoded, at most [`MAX_ASSET_SOURCE_DIM`] per side.
//!   Sources wider or taller than [`MAX_ASSET_DIM`] are box-downscaled by an integer factor while
//!   rows stream in (reported by [`ImportedMask::downscaled_from`]); no full-size frame is ever
//!   allocated, so decoded memory stays within the mask itself (≤ 2048² bytes) plus row buffers.
//! * **Rendering.** A box-filtered mip pyramid is built once per asset; the dab rasterizer picks
//!   the level whose texels are not smaller than one document pixel and samples it bilinearly.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::BrushError;

/// Largest stored mask side (px).
pub const MAX_ASSET_DIM: u32 = 2048;
/// Largest accepted source PNG side (px); larger sides are rejected.
pub const MAX_ASSET_SOURCE_DIM: u32 = 8192;
/// Largest accepted encoded PNG (bytes).
pub const MAX_ASSET_PNG_BYTES: usize = 16 * 1024 * 1024;
/// Allocation budget handed to the `png` decoder (row buffers and ancillary chunks only).
const DECODER_LIMIT_BYTES: usize = 4 * 1024 * 1024 + 4 * (MAX_ASSET_SOURCE_DIM as usize * 8 + 1);
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const ADAM7: [(u32, u32, u32, u32); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// How a source image becomes coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskChannel {
    /// `alpha` if the image has any non-opaque pixel, else `darkness`.
    #[default]
    Auto,
    /// Coverage = alpha.
    Alpha,
    /// Coverage = luminance × alpha (white = paint).
    Luminance,
    /// Coverage = (1 − luminance) × alpha (black = paint).
    Darkness,
}

/// One mip level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaskLevel {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl MaskLevel {
    #[inline]
    fn at(&self, x: i64, y: i64) -> f64 {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            0.0
        } else {
            self.data[y as usize * self.w as usize + x as usize] as f64
        }
    }

    /// Bilinear sample in texel coordinates (texel centers at `i + 0.5`), zero outside, `0..=1`.
    #[inline]
    pub(crate) fn sample_clamped(&self, x: f64, y: f64) -> f64 {
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let (tx, ty) = (fx - x0, fy - y0);
        let (ix, iy) = (x0 as i64, y0 as i64);
        if ix < -1 || iy < -1 || ix >= self.w as i64 || iy >= self.h as i64 {
            return 0.0;
        }
        if ix >= 0 && iy >= 0 && ix + 1 < self.w as i64 && iy + 1 < self.h as i64 {
            // Interior fast path: all four taps exist.
            let w = self.w as usize;
            let i = iy as usize * w + ix as usize;
            let (a, b) = (self.data[i] as f64, self.data[i + 1] as f64);
            let (c, d) = (self.data[i + w] as f64, self.data[i + w + 1] as f64);
            let top = a + (b - a) * tx;
            let bot = c + (d - c) * tx;
            return (top + (bot - top) * ty) / 255.0;
        }
        let a = self.at(ix, iy);
        let b = self.at(ix + 1, iy);
        let c = self.at(ix, iy + 1);
        let d = self.at(ix + 1, iy + 1);
        let top = a + (b - a) * tx;
        let bot = c + (d - c) * tx;
        (top + (bot - top) * ty) / 255.0
    }

    /// Bilinear sample with wrap-around (tiled textures), `0..=1`.
    #[inline]
    pub(crate) fn sample_wrapped(&self, x: f64, y: f64) -> f64 {
        let (w, h) = (self.w as i64, self.h as i64);
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let (tx, ty) = (fx - x0, fy - y0);
        let ix = (x0 as i64).rem_euclid(w);
        let iy = (y0 as i64).rem_euclid(h);
        let ix1 = (ix + 1) % w;
        let iy1 = (iy + 1) % h;
        let px = |x: i64, y: i64| self.data[y as usize * self.w as usize + x as usize] as f64;
        let top = px(ix, iy) + (px(ix1, iy) - px(ix, iy)) * tx;
        let bot = px(ix, iy1) + (px(ix1, iy1) - px(ix, iy1)) * tx;
        (top + (bot - top) * ty) / 255.0
    }
}

/// A validated coverage mask with its mip pyramid, content hash and canonical PNG carrier.
#[derive(Clone, PartialEq, Eq)]
pub struct BrushAsset {
    hash: String,
    levels: Vec<MaskLevel>,
    png: Arc<[u8]>,
}

impl std::fmt::Debug for BrushAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrushAsset")
            .field("hash", &self.hash)
            .field("width", &self.width())
            .field("height", &self.height())
            .field("png_bytes", &self.png.len())
            .finish()
    }
}

/// Result of [`BrushAsset::from_png`].
#[derive(Debug, Clone)]
pub struct ImportedMask {
    pub asset: BrushAsset,
    /// Source size when the image was downscaled to fit [`MAX_ASSET_DIM`].
    pub downscaled_from: Option<(u32, u32)>,
    /// Channel actually used (resolves [`MaskChannel::Auto`]).
    pub channel: MaskChannel,
}

/// Content hash of a mask (see the module docs).
pub fn mask_hash(width: u32, height: u32, data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"VSKMASK1");
    h.update(width.to_le_bytes());
    h.update(height.to_le_bytes());
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn invalid(m: impl Into<String>) -> BrushError {
    BrushError::InvalidAsset(m.into())
}

impl BrushAsset {
    /// Build from raw coverage (`width × height` bytes, row-major).
    pub fn from_mask(width: u32, height: u32, data: Vec<u8>) -> Result<BrushAsset, BrushError> {
        if width == 0 || height == 0 {
            return Err(invalid("mask has no pixels"));
        }
        if width > MAX_ASSET_DIM || height > MAX_ASSET_DIM {
            return Err(BrushError::AssetTooLarge(format!("mask {width}x{height} exceeds {MAX_ASSET_DIM} px")));
        }
        if data.len() != width as usize * height as usize {
            return Err(invalid("mask length does not match its size"));
        }
        let hash = mask_hash(width, height, &data);
        let png = encode_gray8(width, height, &data)?;
        let mut levels = vec![MaskLevel { w: width, h: height, data }];
        while let Some(last) = levels.last() {
            if last.w == 1 && last.h == 1 {
                break;
            }
            let next = downsample2(last);
            levels.push(next);
        }
        Ok(BrushAsset { hash, levels, png: Arc::from(png) })
    }

    /// Decode a stored asset (canonical grayscale PNG). The gray value is the coverage; any
    /// other PNG is accepted too (luminance × alpha), but it must already fit [`MAX_ASSET_DIM`].
    /// Callers verify the content hash against the reference they resolved.
    pub fn from_stored_png(bytes: &[u8]) -> Result<BrushAsset, BrushError> {
        let m = decode_mask(bytes, MaskChannel::Luminance, false, false)?;
        if m.downscaled_from.is_some() {
            return Err(BrushError::AssetTooLarge("stored brush asset exceeds 2048 px".into()));
        }
        Ok(m.asset)
    }

    /// Convert any PNG to a coverage mask (see [`MaskChannel`]); `invert` flips the coverage.
    pub fn from_png(bytes: &[u8], channel: MaskChannel, invert: bool) -> Result<ImportedMask, BrushError> {
        decode_mask(bytes, channel, invert, true)
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }
    pub fn width(&self) -> u32 {
        self.levels[0].w
    }
    pub fn height(&self) -> u32 {
        self.levels[0].h
    }
    /// Full-resolution coverage, row-major.
    pub fn mask(&self) -> &[u8] {
        &self.levels[0].data
    }
    /// Canonical 8-bit grayscale PNG carrier.
    pub fn png_bytes(&self) -> &Arc<[u8]> {
        &self.png
    }

    /// Mip level for drawing at `px_per_texel` document pixels per level-0 texel (the smallest of
    /// both axes): the finest level whose texels are at least one pixel.
    pub(crate) fn level_for(&self, px_per_texel: f64) -> &MaskLevel {
        &self.levels[self.level_index_for(px_per_texel)]
    }

    /// Index of [`BrushAsset::level_for`].
    pub(crate) fn level_index_for(&self, px_per_texel: f64) -> usize {
        let mut i = 0usize;
        let mut s = px_per_texel;
        while s < 1.0 && i + 1 < self.levels.len() {
            s *= 2.0;
            i += 1;
        }
        i
    }

    pub(crate) fn levels(&self) -> &[MaskLevel] {
        &self.levels
    }

    #[cfg(test)]
    pub(crate) fn level0(&self) -> &MaskLevel {
        &self.levels[0]
    }

    /// Bytes held in memory (mask pyramid + PNG carrier).
    pub fn memory_bytes(&self) -> usize {
        self.levels.iter().map(|l| l.data.len()).sum::<usize>() + self.png.len()
    }
}

fn downsample2(l: &MaskLevel) -> MaskLevel {
    let (w, h) = (l.w.div_ceil(2), l.h.div_ceil(2));
    let mut data = vec![0u8; w as usize * h as usize];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            let mut n = 0u32;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = (2 * x + dx, 2 * y + dy);
                if sx < l.w && sy < l.h {
                    sum += l.data[(sy * l.w + sx) as usize] as u32;
                    n += 1;
                }
            }
            data[(y * w + x) as usize] = ((sum + n / 2) / n.max(1)) as u8;
        }
    }
    MaskLevel { w, h, data }
}

fn encode_gray8(w: u32, h: u32, data: &[u8]) -> Result<Vec<u8>, BrushError> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| invalid(format!("PNG encode: {e}")))?;
        wr.write_image_data(data).map_err(|e| invalid(format!("PNG encode: {e}")))?;
        wr.finish().map_err(|e| invalid(format!("PNG encode: {e}")))?;
    }
    Ok(out)
}

fn png_error(e: png::DecodingError) -> BrushError {
    match e {
        png::DecodingError::LimitsExceeded => BrushError::AssetTooLarge("PNG decoder memory limit exceeded".into()),
        png::DecodingError::IoError(io) => invalid(format!("PNG truncated or unreadable ({})", io.kind())),
        png::DecodingError::Format(f) => invalid(format!("PNG: {f}")),
        png::DecodingError::Parameter(p) => invalid(format!("PNG: {p}")),
    }
}

/// Per-pixel conversion of one output-format row into `(luminance, alpha)` pairs.
#[derive(Debug, Clone, Copy)]
struct Layout {
    channels: usize,
}

#[inline]
fn lum_alpha(row: &[u8], i: usize, l: Layout) -> (u32, u32) {
    let p = &row[i * l.channels..(i + 1) * l.channels];
    match l.channels {
        1 => (p[0] as u32, 255),
        2 => (p[0] as u32, p[1] as u32),
        3 => ((54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32 + 128) >> 8, 255),
        _ => ((54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32 + 128) >> 8, p[3] as u32),
    }
}

#[inline]
fn coverage(lum: u32, alpha: u32, ch: MaskChannel, invert: bool) -> u32 {
    let c = match ch {
        MaskChannel::Alpha | MaskChannel::Auto => alpha,
        MaskChannel::Luminance => (lum * alpha + 127) / 255,
        MaskChannel::Darkness => ((255 - lum.min(255)) * alpha + 127) / 255,
    };
    if invert { 255 - c.min(255) } else { c.min(255) }
}

/// Streaming PNG → mask decode. `allow_downscale = false` rejects sources over 2048 px.
fn decode_mask(
    bytes: &[u8],
    channel: MaskChannel,
    invert: bool,
    allow_downscale: bool,
) -> Result<ImportedMask, BrushError> {
    if bytes.len() > MAX_ASSET_PNG_BYTES {
        return Err(BrushError::AssetTooLarge(format!("PNG larger than {MAX_ASSET_PNG_BYTES} bytes")));
    }
    if bytes.get(..8) != Some(&PNG_SIGNATURE[..]) {
        return Err(invalid("not a PNG file (bad signature)"));
    }
    let open = || -> Result<png::Reader<Cursor<&[u8]>>, BrushError> {
        let mut dec = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: DECODER_LIMIT_BYTES });
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        dec.set_ignore_text_chunk(true);
        dec.set_ignore_iccp_chunk(true);
        dec.read_info().map_err(png_error)
    };
    let reader = open()?;
    let info = reader.info();
    let (w, h, interlaced) = (info.width, info.height, info.interlaced);
    if w == 0 || h == 0 {
        return Err(invalid("PNG has no pixels"));
    }
    if w > MAX_ASSET_SOURCE_DIM || h > MAX_ASSET_SOURCE_DIM {
        return Err(BrushError::AssetTooLarge(format!("PNG {w}x{h} exceeds {MAX_ASSET_SOURCE_DIM} px")));
    }
    let f = w.max(h).div_ceil(MAX_ASSET_DIM).max(1);
    if f > 1 && !allow_downscale {
        return Err(BrushError::AssetTooLarge(format!("PNG {w}x{h} exceeds {MAX_ASSET_DIM} px")));
    }
    let channels = match reader.output_color_type().0 {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(invalid("PNG palette was not expanded")),
    };
    let layout = Layout { channels };
    let has_alpha_channel = channels == 2 || channels == 4;
    // Auto: a pre-pass looks for transparency (second decode, no extra memory).
    let ch = match channel {
        MaskChannel::Auto if has_alpha_channel => {
            let mut transparent = false;
            let mut r = open()?;
            visit_rows(&mut r, w, h, interlaced, channels, |_, _, row, n| {
                transparent |= (0..n).any(|i| lum_alpha(row, i, layout).1 < 255);
            })?;
            if transparent { MaskChannel::Alpha } else { MaskChannel::Darkness }
        }
        MaskChannel::Auto => MaskChannel::Darkness,
        c => c,
    };
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    let mut reader = reader;
    let data = if f == 1 {
        let mut mask = vec![0u8; ow as usize * oh as usize];
        visit_rows(&mut reader, w, h, interlaced, channels, |y, xs, row, n| {
            let (x0, dx) = xs;
            for i in 0..n {
                let (l, a) = lum_alpha(row, i, layout);
                let x = x0 + i as u32 * dx;
                mask[y as usize * ow as usize + x as usize] = coverage(l, a, ch, invert) as u8;
            }
        })?;
        mask
    } else {
        // f ≤ 4 (8192 / 2048): f² · 255 fits in u16.
        let mut sums = vec![0u16; ow as usize * oh as usize];
        visit_rows(&mut reader, w, h, interlaced, channels, |y, xs, row, n| {
            let (x0, dx) = xs;
            let oy = (y / f) as usize;
            for i in 0..n {
                let (l, a) = lum_alpha(row, i, layout);
                let x = x0 + i as u32 * dx;
                let s = &mut sums[oy * ow as usize + (x / f) as usize];
                *s = s.saturating_add(coverage(l, a, ch, invert) as u16);
            }
        })?;
        let mut mask = vec![0u8; sums.len()];
        for oy in 0..oh {
            let ch_ = f.min(h - oy * f);
            for ox in 0..ow {
                let cw = f.min(w - ox * f);
                let n = cw * ch_;
                let i = (oy * ow + ox) as usize;
                mask[i] = ((sums[i] as u32 + n / 2) / n) as u8;
            }
        }
        mask
    };
    let asset = BrushAsset::from_mask(ow, oh, data)?;
    Ok(ImportedMask { asset, downscaled_from: (f > 1).then_some((w, h)), channel: ch })
}

/// Feed every decoded row to `f(y, (x0, dx), row, pixel_count)`; handles Adam7 passes. Rows
/// shorter than their pixel count are rejected (never indexed out of bounds).
fn visit_rows(
    reader: &mut png::Reader<Cursor<&[u8]>>,
    w: u32,
    h: u32,
    interlaced: bool,
    channels: usize,
    mut f: impl FnMut(u32, (u32, u32), &[u8], usize),
) -> Result<(), BrushError> {
    let passes: Vec<(u32, u32, u32, u32)> = if interlaced { ADAM7.to_vec() } else { vec![(0, 0, 1, 1)] };
    for (x0, y0, dx, dy) in passes {
        let pw = w.saturating_sub(x0).div_ceil(dx);
        let ph = h.saturating_sub(y0).div_ceil(dy);
        if pw == 0 || ph == 0 {
            continue;
        }
        for line in 0..ph {
            let Some(row) = reader.next_row().map_err(png_error)? else {
                return Err(invalid("PNG image data ended early"));
            };
            let data = row.data();
            if data.len() < pw as usize * channels {
                return Err(invalid("PNG row shorter than its width"));
            }
            f(y0 + line * dy, (x0, dx), data, pw as usize);
        }
    }
    Ok(())
}

/// Assets by content hash. Cloning shares the assets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetStore {
    map: BTreeMap<String, Arc<BrushAsset>>,
}

impl AssetStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, a: Arc<BrushAsset>) {
        self.map.insert(a.hash.clone(), a);
    }
    pub fn get(&self, hash: &str) -> Option<&Arc<BrushAsset>> {
        self.map.get(hash)
    }
    pub fn contains(&self, hash: &str) -> bool {
        self.map.contains_key(hash)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<BrushAsset>)> {
        self.map.iter()
    }
    pub fn remove(&mut self, hash: &str) -> Option<Arc<BrushAsset>> {
        self.map.remove(hash)
    }
}

/// The assets one stroke needs, resolved by the caller (library, document, built-ins).
#[derive(Debug, Clone, Default)]
pub struct ResolvedAssets {
    pub tip: Option<Arc<BrushAsset>>,
    pub texture: Option<Arc<BrushAsset>>,
}

impl ResolvedAssets {
    /// Look up the preset's tip/texture images in `stores` (first hit wins). Missing ones stay
    /// `None`; the renderer then uses the documented fallback (round tip / procedural grain).
    pub fn resolve(preset: &crate::BrushPreset, stores: &[&AssetStore]) -> ResolvedAssets {
        let find = |h: &Option<String>| h.as_deref().and_then(|h| stores.iter().find_map(|s| s.get(h)).cloned());
        let tip = (preset.tip.shape == crate::TipShape::Image).then(|| find(&preset.tip.image)).flatten();
        let texture =
            preset.texture.as_ref().filter(|t| t.source == crate::TextureSource::Image).and_then(|t| find(&t.image));
        ResolvedAssets { tip, texture }
    }

    /// Hashes the preset references but this resolution lacks.
    pub fn missing(&self, preset: &crate::BrushPreset) -> Vec<String> {
        let mut out = Vec::new();
        if preset.tip.shape == crate::TipShape::Image && self.tip.is_none() {
            out.extend(preset.tip.image.clone());
        }
        if let Some(t) = &preset.texture
            && t.source == crate::TextureSource::Image
            && self.texture.is_none()
        {
            out.extend(t.image.clone());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&f(x, y));
            }
        }
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&px).unwrap();
        wr.finish().unwrap();
        out
    }

    #[test]
    fn hash_is_content_not_carrier_and_round_trips() {
        let data: Vec<u8> = (0..64u32 * 32).map(|i| (i * 7 % 256) as u8).collect();
        let a = BrushAsset::from_mask(64, 32, data.clone()).unwrap();
        assert_eq!(a.hash(), mask_hash(64, 32, &data));
        let b = BrushAsset::from_stored_png(a.png_bytes()).unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_eq!(b.mask(), &data[..]);
        // Mip chain ends at 1x1.
        assert_eq!(a.levels.last().map(|l| (l.w, l.h)), Some((1, 1)));
    }

    #[test]
    fn channels_and_invert() {
        // Left half opaque black, right half transparent white.
        let png = rgba_png(4, 2, |x, _| if x < 2 { [0, 0, 0, 255] } else { [255, 255, 255, 0] });
        let auto = BrushAsset::from_png(&png, MaskChannel::Auto, false).unwrap();
        assert_eq!(auto.channel, MaskChannel::Alpha);
        assert_eq!(auto.asset.mask(), &[255, 255, 0, 0, 255, 255, 0, 0]);
        let dark = BrushAsset::from_png(&png, MaskChannel::Darkness, false).unwrap();
        assert_eq!(dark.asset.mask(), &[255, 255, 0, 0, 255, 255, 0, 0]);
        let lum = BrushAsset::from_png(&png, MaskChannel::Luminance, true).unwrap();
        assert_eq!(lum.asset.mask(), &[255, 255, 255, 255, 255, 255, 255, 255]);
        // Opaque image: auto = darkness (black = paint).
        let opaque = rgba_png(2, 1, |x, _| if x == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        let m = BrushAsset::from_png(&opaque, MaskChannel::Auto, false).unwrap();
        assert_eq!((m.channel, m.asset.mask()), (MaskChannel::Darkness, &[255u8, 0][..]));
    }

    #[test]
    fn oversized_sources_downscale_or_fail() {
        let png = rgba_png(2050, 3, |_, _| [0, 0, 0, 255]);
        let m = BrushAsset::from_png(&png, MaskChannel::Darkness, false).unwrap();
        assert_eq!((m.asset.width(), m.asset.height()), (1025, 2));
        assert_eq!(m.downscaled_from, Some((2050, 3)));
        assert!(m.asset.mask().iter().all(|&v| v == 255));
        assert!(matches!(BrushAsset::from_stored_png(&png), Err(BrushError::AssetTooLarge(_))));
        assert!(matches!(
            BrushAsset::from_png(b"not a png", MaskChannel::Auto, false),
            Err(BrushError::InvalidAsset(_))
        ));
        let mut trunc = rgba_png(16, 16, |x, y| [(x * 16) as u8, (y * 16) as u8, 0, 255]);
        trunc.truncate(trunc.len() / 2);
        assert!(BrushAsset::from_png(&trunc, MaskChannel::Auto, false).is_err());
    }

    #[test]
    fn gray_png_round_trips_exactly() {
        let (w, h) = (13u32, 9u32);
        let px: Vec<u8> = (0..w * h).map(|i| (i * 37 % 251) as u8).collect();
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Grayscale);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(&px).unwrap();
        wr.finish().unwrap();
        let m = BrushAsset::from_png(&out, MaskChannel::Luminance, false).unwrap();
        assert_eq!(m.asset.mask(), &px[..]);
    }

    #[test]
    fn sampling_is_bounded() {
        let a = BrushAsset::from_mask(2, 2, vec![255, 0, 0, 255]).unwrap();
        let l = a.level0();
        assert_eq!(l.sample_clamped(0.5, 0.5), 1.0);
        assert_eq!(l.sample_clamped(-5.0, 0.5), 0.0);
        assert!((l.sample_clamped(1.0, 0.5) - 0.5).abs() < 1e-12);
        assert_eq!(l.sample_wrapped(2.5, 2.5), 1.0);
        assert_eq!(l.sample_wrapped(-0.5, 0.5), 0.0);
    }
}
