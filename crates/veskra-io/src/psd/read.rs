//! Independent structural PSD parser used by [`super::PsdExporter::validate`] and tests.
//!
//! It walks every section with bounds checks (header, color mode data, image resources, layer and
//! mask information incl. every layer record, tagged block and channel image data, global layer
//! mask info, merged image data) and decodes every RLE row to its exact width. It shares no code
//! with the writer except the PackBits row decoder's counterpart (which has its own unit tests).
//! Scope: version-1 PSD, RGB (mode 3) or CMYK (mode 4), 8 or 16 bits per channel (16-bit layer
//! info in an `Lr16` block) — the subset the writer emits. For 16-bit files every decoded plane
//! holds two big-endian bytes per sample. Layer records must carry exactly the color channels of
//! the mode (`0..=2` or `0..=3`), at most one transparency (`-1`) and user mask (`-2`) channel.

use super::packbits;
use veskra_core::{ErrorCode, RectI, VeskraError};

fn bad(m: impl Into<String>) -> VeskraError {
    VeskraError::new(ErrorCode::PsdEncodeError, format!("invalid PSD: {}", m.into()))
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], VeskraError> {
        let end = self.p.checked_add(n).ok_or_else(|| bad(format!("{what}: length overflow")))?;
        let s = self.b.get(self.p..end).ok_or_else(|| bad(format!("{what}: truncated")))?;
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self, w: &str) -> Result<u8, VeskraError> {
        Ok(self.take(1, w)?[0])
    }
    fn u16(&mut self, w: &str) -> Result<u16, VeskraError> {
        let s = self.take(2, w)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self, w: &str) -> Result<u32, VeskraError> {
        let s = self.take(4, w)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self, w: &str) -> Result<i32, VeskraError> {
        Ok(self.u32(w)? as i32)
    }
    fn sig(&mut self, w: &str) -> Result<[u8; 4], VeskraError> {
        let s = self.take(4, w)?;
        Ok([s[0], s[1], s[2], s[3]])
    }
    fn rest(&self) -> usize {
        self.b.len() - self.p
    }
}

/// One parsed layer record (file order: bottom-most first).
#[derive(Debug, Clone, PartialEq)]
pub struct PsdLayerInfo {
    pub rect: RectI,
    /// Pascal (legacy, ASCII) name.
    pub name: String,
    /// `luni` Unicode name.
    pub unicode_name: Option<String>,
    pub blend_key: [u8; 4],
    pub opacity: u8,
    pub clipping: u8,
    pub flags: u8,
    /// `(channel id, compression)` in record order.
    pub channels: Vec<(i16, u16)>,
    /// `(rect, default color, flags)`.
    pub mask: Option<(RectI, u8, u8)>,
    /// `lsct` section divider type and optional blend key.
    pub section: Option<(u32, Option<[u8; 4]>)>,
    /// `lspf` protection flags.
    pub protection: Option<u32>,
    /// Decoded channel planes (only when requested): `(id, bytes)`.
    pub planes: Vec<(i16, Vec<u8>)>,
}

impl PsdLayerInfo {
    pub fn hidden(&self) -> bool {
        self.flags & 0x02 != 0
    }
    pub fn display_name(&self) -> &str {
        self.unicode_name.as_deref().unwrap_or(&self.name)
    }
}

/// Parsed file summary.
#[derive(Debug, Clone, PartialEq)]
pub struct PsdInfo {
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub depth: u16,
    pub color_mode: u16,
    /// Image resources `(id, data)` in file order.
    pub resources: Vec<(u16, Vec<u8>)>,
    /// Raw signed layer count (negative = merged image's first extra channel is transparency).
    pub layer_count_raw: i16,
    pub layers: Vec<PsdLayerInfo>,
    pub merged_compression: u16,
    /// Decoded merged image planes (only when requested), `channels` planes of `w·h` bytes.
    pub merged: Vec<Vec<u8>>,
}

impl PsdInfo {
    pub fn resource(&self, id: u16) -> Option<&[u8]> {
        self.resources.iter().find(|(i, _)| *i == id).map(|(_, d)| d.as_slice())
    }
}

fn read_rect(c: &mut Cur, w: &str) -> Result<RectI, VeskraError> {
    let (top, left, bottom, right) = (c.i32(w)?, c.i32(w)?, c.i32(w)?, c.i32(w)?);
    if bottom < top || right < left {
        return Err(bad(format!("{w}: inverted rectangle")));
    }
    if (bottom as i64 - top as i64) > 30_000 || (right as i64 - left as i64) > 30_000 {
        return Err(bad(format!("{w}: rectangle exceeds 30000 px")));
    }
    Ok(RectI::new(left, top, right, bottom))
}

/// Decode `rows` rows of `width` bytes of one channel (`compression` already read) from `c`,
/// consuming exactly the channel's bytes.
fn read_plane(data: &[u8], compression: u16, width: usize, rows: usize, keep: bool) -> Result<Vec<u8>, VeskraError> {
    let mut out = Vec::new();
    match compression {
        0 => {
            if data.len() as u64 != width as u64 * rows as u64 {
                return Err(bad("raw channel length mismatch"));
            }
            if keep {
                out.extend_from_slice(data);
            }
        }
        1 => {
            let mut c = Cur { b: data, p: 0 };
            let mut counts = Vec::with_capacity(rows);
            for _ in 0..rows {
                counts.push(c.u16("RLE row count")? as usize);
            }
            let mut row = Vec::with_capacity(width);
            for n in counts {
                row.clear();
                packbits::decode_row(c.take(n, "RLE row")?, width, &mut row).ok_or_else(|| bad("RLE row decode"))?;
                if keep {
                    out.extend_from_slice(&row);
                }
            }
            if c.rest() != 0 {
                return Err(bad("RLE channel has trailing bytes"));
            }
        }
        k => return Err(bad(format!("unsupported compression {k}"))),
    }
    Ok(out)
}

fn read_pascal(c: &mut Cur, pad: usize, w: &str) -> Result<String, VeskraError> {
    let n = c.u8(w)? as usize;
    let s = c.take(n, w)?;
    let total = 1 + n;
    let padded = total.div_ceil(pad) * pad;
    c.take(padded - total, w)?;
    Ok(s.iter().map(|&b| b as char).collect())
}

/// Parse layer records and their channel image data (`bps` = bytes per sample). Returns the raw
/// signed layer count and the layers in file order.
fn parse_layer_info(
    li: &mut Cur,
    bps: usize,
    color_channels: i16,
    keep_pixels: bool,
) -> Result<(i16, Vec<PsdLayerInfo>), VeskraError> {
    let mut layers: Vec<PsdLayerInfo> = Vec::new();
    let layer_count_raw = li.u16("layer count")? as i16;
    let n = layer_count_raw.unsigned_abs() as usize;
    let mut chan_lens: Vec<Vec<(i16, u32)>> = Vec::with_capacity(n);
    for _ in 0..n {
        let rect = read_rect(li, "layer rect")?;
        let nch = li.u16("channel count")? as usize;
        if nch > 56 {
            return Err(bad("layer channel count"));
        }
        let mut ch: Vec<(i16, u32)> = Vec::with_capacity(nch);
        for _ in 0..nch {
            let id = li.u16("channel id")? as i16;
            if !(-2..color_channels).contains(&id) {
                return Err(bad("channel id does not belong to the color mode"));
            }
            if ch.iter().any(|(c, _)| *c == id) {
                return Err(bad("duplicate channel id"));
            }
            ch.push((id, li.u32("channel length")?));
        }
        if (0..color_channels).any(|c| !ch.iter().any(|(id, _)| *id == c)) {
            return Err(bad("layer record lacks a color channel of the mode"));
        }
        if &li.sig("blend signature")? != b"8BIM" {
            return Err(bad("blend mode signature"));
        }
        let blend_key = li.sig("blend key")?;
        let opacity = li.u8("opacity")?;
        let clipping = li.u8("clipping")?;
        if clipping > 1 {
            return Err(bad("clipping byte"));
        }
        let flags = li.u8("flags")?;
        li.u8("filler")?;
        let extra_len = li.u32("extra data length")? as usize;
        let mut ex = Cur { b: li.take(extra_len, "layer extra data")?, p: 0 };
        let mask_len = ex.u32("mask data length")? as usize;
        let mut mk = Cur { b: ex.take(mask_len, "mask data")?, p: 0 };
        let mask = if mask_len == 0 {
            None
        } else {
            if !matches!(mask_len, 20 | 36) {
                return Err(bad("mask data length"));
            }
            let r = read_rect(&mut mk, "mask rect")?;
            let def = mk.u8("mask default")?;
            if def != 0 && def != 255 {
                return Err(bad("mask default color"));
            }
            Some((r, def, mk.u8("mask flags")?))
        };
        let br_len = ex.u32("blending ranges length")? as usize;
        if !br_len.is_multiple_of(8) {
            return Err(bad("blending ranges length"));
        }
        ex.take(br_len, "blending ranges")?;
        let name = read_pascal(&mut ex, 4, "layer name")?;
        let mut info = PsdLayerInfo {
            rect,
            name,
            unicode_name: None,
            blend_key,
            opacity,
            clipping,
            flags,
            channels: Vec::new(),
            mask,
            section: None,
            protection: None,
            planes: Vec::new(),
        };
        while ex.rest() > 0 {
            let sig = ex.sig("tagged block signature")?;
            if &sig != b"8BIM" && &sig != b"8B64" {
                return Err(bad("tagged block signature"));
            }
            let key = ex.sig("tagged block key")?;
            let len = ex.u32("tagged block length")? as usize;
            let mut d = Cur { b: ex.take(len, "tagged block data")?, p: 0 };
            match &key {
                b"luni" => {
                    let n = d.u32("luni length")? as usize;
                    let units: Vec<u16> = (0..n).map(|_| d.u16("luni char")).collect::<Result<_, _>>()?;
                    info.unicode_name = Some(String::from_utf16(&units).map_err(|_| bad("luni is not valid UTF-16"))?);
                }
                b"lsct" => {
                    let t = d.u32("lsct type")?;
                    if t > 3 {
                        return Err(bad("lsct type"));
                    }
                    let key = if len >= 12 {
                        if &d.sig("lsct signature")? != b"8BIM" {
                            return Err(bad("lsct signature"));
                        }
                        Some(d.sig("lsct key")?)
                    } else {
                        None
                    };
                    info.section = Some((t, key));
                }
                b"lspf" => info.protection = Some(d.u32("lspf")?),
                _ => {}
            }
        }
        chan_lens.push(ch);
        layers.push(info);
    }
    // Channel image data, same order as the records.
    for (info, ch) in layers.iter_mut().zip(&chan_lens) {
        for &(id, len) in ch {
            let data = li.take(len as usize, "channel image data")?;
            let mut cd = Cur { b: data, p: 0 };
            let comp = cd.u16("channel compression")?;
            let r = if id == -2 {
                info.mask.map(|m| m.0).ok_or_else(|| bad("mask channel without mask data"))?
            } else {
                info.rect
            };
            let plane = read_plane(&data[2..], comp, r.width() as usize * bps, r.height() as usize, keep_pixels)?;
            info.channels.push((id, comp));
            if keep_pixels {
                info.planes.push((id, plane));
            }
        }
    }
    if li.b[li.p..].iter().any(|&b| b != 0) {
        return Err(bad("unexpected bytes after channel image data"));
    }
    // Group nesting: bounding divider (3) opens, folder record (1/2) closes (bottom → top).
    let mut depth = 0usize;
    for l in &layers {
        match l.section {
            Some((3, _)) => depth += 1,
            Some((1 | 2, _)) => depth = depth.checked_sub(1).ok_or_else(|| bad("unbalanced group"))?,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(bad("unbalanced group dividers"));
    }
    Ok((layer_count_raw, layers))
}

/// Parse and structurally validate a PSD. With `keep_pixels`, decoded planes are returned.
pub fn inspect_psd(bytes: &[u8], keep_pixels: bool) -> Result<PsdInfo, VeskraError> {
    let mut c = Cur { b: bytes, p: 0 };
    if &c.sig("header")? != b"8BPS" {
        return Err(bad("signature"));
    }
    if c.u16("version")? != 1 {
        return Err(bad("version (only PSD v1, no PSB)"));
    }
    if c.take(6, "reserved")?.iter().any(|&b| b != 0) {
        return Err(bad("reserved bytes"));
    }
    let channels = c.u16("channels")?;
    let height = c.u32("height")?;
    let width = c.u32("width")?;
    let depth = c.u16("depth")?;
    let color_mode = c.u16("mode")?;
    if !(1..=56).contains(&channels) {
        return Err(bad("channel count"));
    }
    if !(1..=30_000).contains(&width) || !(1..=30_000).contains(&height) {
        return Err(bad("dimensions"));
    }
    // Color modes Veskra writes: 3 = RGB (≥ 3 channels), 4 = CMYK (≥ 4 channels, ADR-018).
    let min_channels = match color_mode {
        3 => 3,
        4 => 4,
        _ => return Err(bad("only RGB (mode 3) and CMYK (mode 4) are produced by Veskra")),
    };
    if !matches!(depth, 8 | 16) || channels < min_channels {
        return Err(bad("only 8-bit and 16-bit RGB or CMYK are produced by Veskra"));
    }
    let bps = depth as usize / 8;
    let cm_len = c.u32("color mode data")? as usize;
    c.take(cm_len, "color mode data")?;

    // Image resources.
    let res_len = c.u32("resources length")? as usize;
    let mut rc = Cur { b: c.take(res_len, "resources")?, p: 0 };
    let mut resources = Vec::new();
    while rc.rest() > 0 {
        if &rc.sig("resource signature")? != b"8BIM" {
            return Err(bad("resource signature"));
        }
        let id = rc.u16("resource id")?;
        read_pascal(&mut rc, 2, "resource name")?;
        let n = rc.u32("resource size")? as usize;
        let d = rc.take(n, "resource data")?;
        rc.take(n % 2, "resource padding")?;
        if id == 1005 && n != 16 {
            return Err(bad("ResolutionInfo size"));
        }
        resources.push((id, d.to_vec()));
    }

    // Layer and mask information.
    let lm_len = c.u32("layer and mask length")? as usize;
    let mut lm = Cur { b: c.take(lm_len, "layer and mask section")?, p: 0 };
    let mut layers = Vec::new();
    let mut layer_count_raw = 0i16;
    if lm_len > 0 {
        let li_len = lm.u32("layer info length")? as usize;
        let mut li = Cur { b: lm.take(li_len, "layer info")?, p: 0 };
        if li_len > 0 {
            if depth != 8 {
                return Err(bad("16-bit layer info must be stored in an Lr16 block"));
            }
            (layer_count_raw, layers) = parse_layer_info(&mut li, 1, min_channels as i16, keep_pixels)?;
        }
        if lm.rest() > 0 {
            let g = lm.u32("global mask length")? as usize;
            lm.take(g, "global mask info")?;
        }
        while lm.rest() > 0 {
            let sig = lm.sig("global tagged block")?;
            if &sig != b"8BIM" && &sig != b"8B64" {
                return Err(bad("global tagged block signature"));
            }
            let key = lm.sig("global tagged key")?;
            let n = lm.u32("global tagged length")? as usize;
            let data = lm.take(n, "global tagged data")?;
            if &key == b"Lr16" {
                if depth != 16 || !layers.is_empty() {
                    return Err(bad("unexpected Lr16 block"));
                }
                let mut li = Cur { b: data, p: 0 };
                (layer_count_raw, layers) = parse_layer_info(&mut li, 2, min_channels as i16, keep_pixels)?;
            }
        }
    }

    // Merged image data: everything that remains.
    let merged_compression = c.u16("merged compression")?;
    let rest = c.take(c.rest(), "merged image")?;
    // `w` is the row length in bytes (16-bit samples are two big-endian bytes).
    let (w, h, nch) = (width as usize * bps, height as usize, channels as usize);
    let mut merged = Vec::new();
    match merged_compression {
        0 => {
            if rest.len() as u64 != (w * h) as u64 * nch as u64 {
                return Err(bad("raw merged image length"));
            }
            if keep_pixels {
                merged = rest.chunks_exact(w * h).map(|p| p.to_vec()).collect();
            }
        }
        1 => {
            let mut mc = Cur { b: rest, p: 0 };
            let mut counts = Vec::with_capacity(nch * h);
            for _ in 0..nch * h {
                counts.push(mc.u16("merged RLE count")? as usize);
            }
            let mut row = Vec::with_capacity(w);
            for ch in 0..nch {
                let mut plane = Vec::new();
                for n in &counts[ch * h..(ch + 1) * h] {
                    row.clear();
                    packbits::decode_row(mc.take(*n, "merged RLE row")?, w, &mut row)
                        .ok_or_else(|| bad("merged RLE row decode"))?;
                    if keep_pixels {
                        plane.extend_from_slice(&row);
                    }
                }
                if keep_pixels {
                    merged.push(plane);
                }
            }
            if mc.rest() != 0 {
                return Err(bad("trailing bytes after merged image"));
            }
        }
        k => return Err(bad(format!("unsupported merged compression {k}"))),
    }
    Ok(PsdInfo {
        width,
        height,
        channels,
        depth,
        color_mode,
        resources,
        layer_count_raw,
        layers,
        merged_compression,
        merged,
    })
}
