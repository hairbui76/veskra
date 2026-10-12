//! PSD exporter (FMT-08/09/10, ADR-007, ADR-014): an independent minimal writer implemented from
//! Adobe's public *Photoshop File Formats Specification*. No third-party PSD code is used.
//!
//! # 16 bits per channel (`bit_depth = 16`)
//! The header depth is 16 and every color, transparency and user-mask sample is a big-endian `u16`
//! (straight values from `veskra_color::convert::premul16_to_straight16`; masks `v · 257`), still
//! PackBits-compressed per row. As the specification requires for 16-bit files, the *layer info*
//! length is written as 0 and the layer records + channel data live in an `Lr16` additional-layer-
//! information block after the (empty) global layer mask info. The merged image is matted over
//! white in 16 bits. Opacity, mask default color and flags stay 8-bit fields.
//!
//! # Target profile (`profile`)
//! See `png` module docs: with an explicit target the layer pixels **and** the merged image are
//! converted (relative colorimetric, `f64` on 16-bit data) and resource 1039 holds the target's
//! ICC bytes; out-of-gamut pixels are counted on the merged image only.
//!
//! # CMYK (`profile = cmyk`, ADR-018)
//! Color mode **4** (CMYK). Every layer's pixels and the merged image are converted from the
//! document RGB to CMYK with the chosen output profile (see [`crate::cmyk`]); layer records carry
//! channels `-1` (transparency), `0..=3` (C, M, Y, K) and `-2` (mask); the merged image has C, M,
//! Y, K plus the transparency channel when there are layers. Channel values follow Photoshop's
//! CMYK convention: **inverted**, 255 / 65535 = no ink, 0 = 100 % ink. The merged image is matted
//! over paper white in ink space (`ink · alpha`). Resource 1039 holds the CMYK profile bytes when
//! `embed_color_profile` (untagged CMYK is allowed and warned). Veskra composites in RGB, so
//! layers with non-Normal blend modes are reported (`BlendModeApproximated`): Photoshop
//! recomposites CMYK layers in CMYK. Blending ranges list gray + 5 channel pairs (48 bytes).
//!
//! # File layout written
//! * **Header**: `8BPS`, version 1, 8-bit, RGB (mode 3), ≤ 30 000 px per edge (larger documents
//!   are rejected with `PsdEncodeError`; PSB is not written). Channel count **4** (R, G, B + merged
//!   transparency) whenever there is at least one layer record, signalled by a **negative layer
//!   count** as the spec describes; 3 channels for a document without layers.
//! * **Color mode data**: empty.
//! * **Image resources**: 1005 `ResolutionInfo` (document DPI, pixels/inch, inches) and, when
//!   `embed_color_profile`, 1039 ICC profile with the document's exact ICC bytes (FMT-09, PSD-003).
//! * **Layer and mask information**: one record per layer/group, bottom → top (Photoshop order):
//!   rect, channels `-1` (transparency), `0`, `1`, `2` and `-2` (user mask) when the layer has a
//!   mask; `8BIM` + blend key (`BlendMode::psd_key`, `pass` for pass-through groups); opacity byte;
//!   clipping byte (`clip_to_below`); flags (bit 1 hidden, bit 0 alpha-locked, bit 3 set; group
//!   records also set bit 4); 20-byte layer mask data (rect, default color, flags = 0: absolute
//!   position, enabled); 40-byte "all pass" blending ranges; Pascal name (ASCII, `?` for other
//!   characters) padded to 4; tagged blocks `luni` (UTF-16 name), `lsct` (groups: bounding divider
//!   type 3 named `</Layer group>` below the children and an open (1) / closed (2) folder record
//!   above them, with the group's blend key) and `lspf` (alpha lock → 0x1, lock → 0x80000000).
//!   Channel data uses PackBits (compression 1) with per-row byte counts; empty channels use raw (0).
//! * **Clipping** follows `veskra-compose` (not psd-tools) so other apps recomposite like Veskra: the
//!   clipping byte is 1 only for a non-group `clip_to_below` layer with a non-clipper sibling below
//!   it; an orphan clipper (no base) and a group's clip flag are written as 0 with a
//!   `ClippingApproximated` warning; a pass-through group acting as a clip base is written as `pass`
//!   with a `ClippingApproximated` warning (Veskra composites it as an isolated Normal group).
//! * **Merged image data** (PackBits): the visible document composited by `veskra-compose` exactly as
//!   Veskra renders it, color channels **matted over white** and the 4th channel holding alpha —
//!   Photoshop's convention for transparent documents, which psd-tools and ImageMagick un-matte.
//!
//! Layer pixels are each layer's own pixels cropped to its content bounds, converted u16
//! premultiplied → 8-bit straight with `veskra_color::convert`. Vector-stroke layers are written as
//! their raster pixels (ADR-014, `LayerRasterized` warning). Hidden layers stay hidden. Documents in
//! `Linear` blend space get a `BlendSpaceMismatch` warning (layers are still written; the merged
//! image is Veskra's render). `raster_scale` must be 1.0 for PSD (preflight blocks otherwise).

mod packbits;
mod read;

pub use read::{PsdInfo, PsdLayerInfo, inspect_psd};

use crate::cmyk::CmykPlan;
use crate::png::{
    BAND_ROWS, ColorPlan, color_plan, composite_band, raster_only_stroke_count, vector_layer_count, warning,
};
use crate::{CancelToken, Exporter};
use std::io::Write;
use veskra_color::convert;
use veskra_compose::ComposeOptions;
use veskra_core::export::{
    ExportFormat, ExportOptions, ExportPreflight, ExportReport, ExportWarning, ExportWarningKind, ValidationStatus,
};
use veskra_core::{BlendSpace, ErrorCode, LayerId, RectI, VeskraError};
use veskra_doc::{Document, Layer, LayerKind, MaskGrid, PixelContent};

/// Maximum PSD (version 1) edge length in pixels.
pub const PSD_MAX_DIM: u32 = 30_000;
/// Name Photoshop gives the bounding section divider record.
pub const GROUP_END_NAME: &str = "</Layer group>";

const FLAG_ALPHA_LOCK: u8 = 0x01;
const FLAG_HIDDEN: u8 = 0x02;
const FLAG_BIT4_USEFUL: u8 = 0x08;
const FLAG_IRRELEVANT_PIXELS: u8 = 0x10;

fn psd_err(m: impl Into<String>) -> VeskraError {
    VeskraError::new(ErrorCode::PsdEncodeError, m)
}

fn io_err(e: std::io::Error) -> VeskraError {
    let code = if e.kind() == std::io::ErrorKind::StorageFull { ErrorCode::StorageFull } else { ErrorCode::Io };
    VeskraError::new(code, format!("write failed: {}", e.kind()))
}

fn u32_len(n: usize, what: &str) -> Result<u32, VeskraError> {
    u32::try_from(n).map_err(|_| psd_err(format!("{what} exceeds the 4 GiB PSD section limit (PSB not supported)")))
}

/// Tight bounds of mask pixels that differ from the mask's default value.
fn mask_non_default_bounds(m: &MaskGrid) -> RectI {
    let d = m.default_value();
    let t = veskra_core::TILE_SIZE as usize;
    let mut b = RectI::EMPTY;
    for (c, tile) in m.iter_tiles() {
        let tr = c.rect();
        let (mut x0, mut y0, mut x1, mut y1) = (t, t, 0, 0);
        for (y, row) in tile.data().chunks_exact(t).enumerate() {
            if let Some(f) = row.iter().position(|&v| v != d) {
                let l = row.iter().rposition(|&v| v != d).unwrap_or(f);
                y0 = y0.min(y);
                y1 = y + 1;
                x0 = x0.min(f);
                x1 = x1.max(l + 1);
            }
        }
        if y1 > 0 {
            b = b.union(&RectI::new(tr.x0 + x0 as i32, tr.y0 + y0 as i32, tr.x0 + x1 as i32, tr.y0 + y1 as i32));
        }
    }
    b
}

/// Rect to write for some content bounds: unchanged unless an edge exceeds the PSD limit, in which
/// case it is cropped to the canvas (`true` = cropped).
fn fit_rect(r: RectI, canvas: RectI) -> (RectI, bool) {
    if r.width() > PSD_MAX_DIM || r.height() > PSD_MAX_DIM { (r.intersect(&canvas), true) } else { (r, false) }
}

/// What will be preserved, approximated or flattened (shared by preflight and export).
#[derive(Debug, Default)]
struct Analysis {
    warnings: Vec<ExportWarning>,
    flattened: Vec<String>,
    rasterized_layers: u32,
    records: u32,
    estimated_bytes: u64,
}

fn analyze(doc: &Document) -> Analysis {
    let mut a = Analysis::default();
    let canvas = doc.bounds();
    let vectors = vector_layer_count(doc);
    a.rasterized_layers = vectors;
    if vectors > 0 {
        a.warnings.push(warning(
            ExportWarningKind::LayerRasterized,
            format!(
                "{vectors} vector-stroke layer(s) are written as raster layers; editable geometry stays in .veskra."
            ),
            None,
            vectors,
        ));
        a.flattened.push(format!("vector_layers_as_raster:{vectors}"));
    }
    let hidden = doc.iter_layers().filter(|l| !l.props.visible).count() as u32;
    if hidden > 0 {
        a.warnings.push(warning(
            ExportWarningKind::HiddenLayer,
            format!("{hidden} hidden layer(s)/group(s) are kept as hidden layers (not lost)."),
            None,
            hidden,
        ));
    }
    if doc.color.blend_space == BlendSpace::Linear {
        a.warnings.push(warning(
            ExportWarningKind::BlendSpaceMismatch,
            "Document blends in linear light; Photoshop/Krita recomposite layers gamma-encoded, so their \
             rendering may differ. The merged image is Veskra's render.",
            None,
            1,
        ));
    }
    let mut est = 2048u64 + doc.width() as u64 * doc.height() as u64 * 4;
    for l in doc.iter_layers() {
        a.records += if l.is_group() { 2 } else { 1 };
        est += 256;
        if let Some(p) = l.pixels() {
            let (r, cropped) = fit_rect(p.tiles.content_bounds(), canvas);
            est += r.area() * 4;
            if cropped {
                a.warnings.push(warning(
                    ExportWarningKind::FeatureFlattened,
                    format!(
                        "Layer '{}' extends beyond PSD's 30000 px limit; off-canvas pixels are cropped.",
                        l.props.name
                    ),
                    Some(l.id.to_string()),
                    1,
                ));
                a.flattened.push(format!("offcanvas_cropped:{}", l.id));
            }
            if let Some(m) = &p.mask {
                let (mr, mc) = fit_rect(mask_non_default_bounds(m), canvas);
                est += mr.area();
                if mc {
                    a.warnings.push(warning(
                        ExportWarningKind::FeatureFlattened,
                        format!("Mask of layer '{}' is cropped to the canvas (PSD 30000 px limit).", l.props.name),
                        Some(l.id.to_string()),
                        1,
                    ));
                    a.flattened.push(format!("mask_cropped:{}", l.id));
                }
            }
        }
        if l.props.clip_to_below && l.is_group() {
            a.warnings.push(warning(
                ExportWarningKind::ClippingApproximated,
                format!(
                    "Group '{}' has the clipping flag, which Veskra ignores for groups; it is written unclipped.",
                    l.props.name
                ),
                Some(l.id.to_string()),
                1,
            ));
        } else if l.group().is_some_and(|g| g.pass_through) && is_clip_base(doc, l) {
            a.warnings.push(warning(
                ExportWarningKind::ClippingApproximated,
                format!(
                    "Pass-through group '{}' is a clipping base; Veskra composites it as an isolated Normal \
                     group, other apps may pass it through.",
                    l.props.name
                ),
                Some(l.id.to_string()),
                1,
            ));
        } else if l.props.clip_to_below && !has_clip_base(doc, l) {
            a.warnings.push(warning(
                ExportWarningKind::ClippingApproximated,
                format!(
                    "Layer '{}' is marked clipped but has no base layer below it; Veskra draws it unclipped, \
                     so it is written as a normal (unclipped) layer.",
                    l.props.name
                ),
                Some(l.id.to_string()),
                1,
            ));
        }
    }
    a.estimated_bytes = est;
    a
}

/// Whether a layer has a clipping base: a sibling below it that is not itself a clipper (a group,
/// or a layer without `clip_to_below`) — the `veskra-compose` rule.
fn has_clip_base(doc: &Document, l: &Layer) -> bool {
    let Ok(siblings) = doc.children_of(l.parent) else { return false };
    let Some(pos) = siblings.iter().position(|s| *s == l.id) else { return false };
    siblings[..pos].iter().any(|s| doc.layer(*s).is_some_and(|x| x.is_group() || !x.props.clip_to_below))
}

/// Whether the sibling directly above `l` is a (non-group) clipper, making `l` a clipping base.
fn is_clip_base(doc: &Document, l: &Layer) -> bool {
    let Ok(siblings) = doc.children_of(l.parent) else { return false };
    let Some(pos) = siblings.iter().position(|s| *s == l.id) else { return false };
    siblings.get(pos + 1).and_then(|s| doc.layer(*s)).is_some_and(|x| x.props.clip_to_below && !x.is_group())
}

/// PSD clipping byte: 1 only for a non-group clipper that has a base (exactly what Veskra renders).
fn clipping_byte(doc: &Document, l: &Layer) -> u8 {
    (l.props.clip_to_below && !l.is_group() && has_clip_base(doc, l)) as u8
}

/// One layer record plus its channel image data.
#[derive(Debug)]
struct Record {
    rect: RectI,
    /// `(channel id, data incl. the 2-byte compression field)`.
    channels: Vec<(i16, Vec<u8>)>,
    blend: [u8; 4],
    opacity: u8,
    clipping: u8,
    flags: u8,
    mask: Option<(RectI, u8)>,
    name: String,
    section: Option<(u32, [u8; 4])>,
    protection: u32,
    /// Blending-range entries (4 bytes each): 10 for RGB, 12 for CMYK.
    ranges: usize,
}

fn opacity_byte(o: f32) -> u8 {
    (o.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Empty channel: raw compression and no data.
fn empty_channel() -> Vec<u8> {
    vec![0, 0]
}

fn empty_channels(color_channels: i16) -> Vec<(i16, Vec<u8>)> {
    (-1i16..color_channels).map(|id| (id, empty_channel())).collect()
}

/// Accumulates PackBits rows for one channel.
#[derive(Debug, Default)]
struct RlePlane {
    counts: Vec<u16>,
    data: Vec<u8>,
}

impl RlePlane {
    fn push_row(&mut self, row: &[u8]) -> Result<(), VeskraError> {
        let n = packbits::encode_row(row, &mut self.data);
        self.counts.push(u16::try_from(n).map_err(|_| psd_err("RLE row too long"))?);
        Ok(())
    }

    /// Layer channel data: compression 1, row counts, rows.
    fn into_channel(self) -> Vec<u8> {
        if self.counts.is_empty() {
            return empty_channel();
        }
        let mut v = Vec::with_capacity(2 + self.counts.len() * 2 + self.data.len());
        v.extend_from_slice(&1u16.to_be_bytes());
        for c in &self.counts {
            v.extend_from_slice(&c.to_be_bytes());
        }
        v.extend_from_slice(&self.data);
        v
    }
}

/// How pixel colors are written: RGB (optionally converted to an RGB target) or CMYK.
#[derive(Clone, Copy)]
enum Pixels<'a> {
    Rgb(&'a ColorPlan),
    Cmyk(&'a CmykPlan),
}

impl Pixels<'_> {
    /// Color channels per pixel (3 or 4).
    fn color_channels(self) -> i16 {
        match self {
            Pixels::Rgb(_) => 3,
            Pixels::Cmyk(_) => 4,
        }
    }

    /// Blending-range entries: gray + one source/destination pair per color channel + 1.
    fn ranges(self) -> usize {
        2 * (self.color_channels() as usize + 2)
    }
}

struct Encoder<'a> {
    doc: &'a Document,
    color: Pixels<'a>,
    cancel: &'a CancelToken,
    canvas: RectI,
    /// 16 bits per channel (big-endian samples) instead of 8.
    deep: bool,
}

impl Encoder<'_> {
    fn walk(&self, ids: &[LayerId], out: &mut Vec<Record>) -> Result<(), VeskraError> {
        for id in ids {
            self.cancel.check()?;
            let l = self
                .doc
                .layer(*id)
                .ok_or_else(|| VeskraError::new(ErrorCode::InvalidDocument, "dangling layer reference"))?;
            match &l.kind {
                LayerKind::Group(g) => {
                    out.push(Record {
                        rect: RectI::EMPTY,
                        channels: empty_channels(self.color.color_channels()),
                        blend: *b"norm",
                        opacity: 255,
                        clipping: 0,
                        flags: FLAG_BIT4_USEFUL | FLAG_IRRELEVANT_PIXELS,
                        mask: None,
                        name: GROUP_END_NAME.to_string(),
                        section: Some((3, *b"norm")),
                        protection: 0,
                        ranges: self.color.ranges(),
                    });
                    self.walk(&g.children, out)?;
                    let key = if g.pass_through { *b"pass" } else { *l.props.blend.psd_key() };
                    out.push(Record {
                        rect: RectI::EMPTY,
                        channels: empty_channels(self.color.color_channels()),
                        blend: key,
                        opacity: opacity_byte(l.props.opacity),
                        clipping: 0, // Veskra ignores clipping on groups
                        flags: self.flags(l) | FLAG_IRRELEVANT_PIXELS,
                        mask: None,
                        name: l.props.name.clone(),
                        section: Some((if g.collapsed { 2 } else { 1 }, key)),
                        protection: protection(l),
                        ranges: self.color.ranges(),
                    });
                }
                LayerKind::Raster(p) | LayerKind::Vector(p) => out.push(self.pixel_record(l, p)?),
            }
        }
        Ok(())
    }

    fn flags(&self, l: &Layer) -> u8 {
        let mut f = FLAG_BIT4_USEFUL;
        if !l.props.visible {
            f |= FLAG_HIDDEN;
        }
        if l.props.alpha_locked {
            f |= FLAG_ALPHA_LOCK;
        }
        f
    }

    /// CMYK layer channels: transparency, then C, M, Y, K inverted (Photoshop: max = no ink).
    fn cmyk_channels(&self, rect: RectI, p: &PixelContent, cm: &CmykPlan) -> Result<Vec<(i16, Vec<u8>)>, VeskraError> {
        let w = rect.width() as usize;
        let bps = if self.deep { 2 } else { 1 };
        let mut planes: [RlePlane; 5] = Default::default(); // A, C, M, Y, K
        let mut cmyk = Vec::new();
        let mut row = vec![0u8; w * bps];
        let mut y = rect.y0;
        while y < rect.y1 {
            self.cancel.check()?;
            let y1 = (y + BAND_ROWS as i32).min(rect.y1);
            let px = p.tiles.try_read_rect(RectI::new(rect.x0, y, rect.x1, y1)).map_err(VeskraError::from)?;
            cm.convert(&px, &mut cmyk, false);
            for r in cmyk.chunks_exact(w * 5) {
                for (pi, ch) in [4usize, 0, 1, 2, 3].into_iter().enumerate() {
                    if self.deep {
                        for (d, s) in row.chunks_exact_mut(2).zip(r.chunks_exact(5)) {
                            let v = if ch == 4 { s[4] } else { 65535 - s[ch] };
                            d.copy_from_slice(&v.to_be_bytes());
                        }
                    } else {
                        for (d, s) in row.iter_mut().zip(r.chunks_exact(5)) {
                            let a8 = convert::u16_to_u8(s[4]);
                            *d = match (ch, a8) {
                                (4, _) => a8,
                                (_, 0) => 255, // transparent at 8 bits: no ink
                                _ => convert::u16_to_u8(65535 - s[ch]),
                            };
                        }
                    }
                    planes[pi].push_row(&row)?;
                }
            }
            y = y1;
        }
        let [a, c, m, yy, k] = planes;
        Ok(vec![
            (-1i16, a.into_channel()),
            (0, c.into_channel()),
            (1, m.into_channel()),
            (2, yy.into_channel()),
            (3, k.into_channel()),
        ])
    }

    fn pixel_record(&self, l: &Layer, p: &PixelContent) -> Result<Record, VeskraError> {
        let (rect, _) = fit_rect(p.tiles.content_bounds(), self.canvas);
        let color = match self.color {
            Pixels::Rgb(c) => c,
            Pixels::Cmyk(cm) => {
                let channels = self.cmyk_channels(rect, p, cm)?;
                return self.finish_record(l, p, rect, channels);
            }
        };
        let w = rect.width() as usize;
        let bps = if self.deep { 2 } else { 1 };
        let mut planes: [RlePlane; 4] = Default::default(); // A, R, G, B
        let mut straight = Vec::new();
        let mut straight16 = Vec::new();
        let mut row = vec![0u8; w * bps];
        let mut y = rect.y0;
        while y < rect.y1 {
            self.cancel.check()?;
            let y1 = (y + BAND_ROWS as i32).min(rect.y1);
            let band = RectI::new(rect.x0, y, rect.x1, y1);
            let mut px = p.tiles.try_read_rect(band).map_err(VeskraError::from)?;
            color.convert_px(&mut px);
            if self.deep {
                straight16.resize(px.len(), 0);
                convert::premul16_to_straight16(&px, &mut straight16);
                for r in straight16.chunks_exact(w * 4) {
                    for (pi, ch) in [3usize, 0, 1, 2].into_iter().enumerate() {
                        for (d, s) in row.chunks_exact_mut(2).zip(r.chunks_exact(4)) {
                            d.copy_from_slice(&s[ch].to_be_bytes());
                        }
                        planes[pi].push_row(&row)?;
                    }
                }
            } else {
                straight.resize(px.len(), 0);
                convert::premul16_to_straight8(&px, &mut straight);
                for r in straight.chunks_exact(w * 4) {
                    for (pi, ch) in [3usize, 0, 1, 2].into_iter().enumerate() {
                        for (d, s) in row.iter_mut().zip(r.chunks_exact(4)) {
                            *d = s[ch];
                        }
                        planes[pi].push_row(&row)?;
                    }
                }
            }
            y = y1;
        }
        let [a, r, g, b] = planes;
        let channels =
            vec![(-1i16, a.into_channel()), (0, r.into_channel()), (1, g.into_channel()), (2, b.into_channel())];
        self.finish_record(l, p, rect, channels)
    }

    /// Append the user mask channel (if any) and build the record.
    fn finish_record(
        &self,
        l: &Layer,
        p: &PixelContent,
        rect: RectI,
        mut channels: Vec<(i16, Vec<u8>)>,
    ) -> Result<Record, VeskraError> {
        let mut mask = None;
        if let Some(m) = &p.mask {
            let (mr, _) = fit_rect(mask_non_default_bounds(m), self.canvas);
            let mut plane = RlePlane::default();
            let mut y = mr.y0;
            while y < mr.y1 {
                self.cancel.check()?;
                let y1 = (y + BAND_ROWS as i32).min(mr.y1);
                let v = m.read_rect(RectI::new(mr.x0, y, mr.x1, y1));
                if v.len() != mr.width() as usize * (y1 - y) as usize {
                    return Err(psd_err("mask read failed"));
                }
                let mut wide = Vec::new();
                for r in v.chunks_exact(mr.width() as usize) {
                    if self.deep {
                        wide.clear();
                        wide.extend(r.iter().flat_map(|&m| convert::u8_to_u16(m).to_be_bytes()));
                        plane.push_row(&wide)?;
                    } else {
                        plane.push_row(r)?;
                    }
                }
                y = y1;
            }
            channels.push((-2, plane.into_channel()));
            mask = Some((mr, m.default_value()));
        }
        Ok(Record {
            rect,
            channels,
            blend: *l.props.blend.psd_key(),
            opacity: opacity_byte(l.props.opacity),
            clipping: clipping_byte(self.doc, l),
            flags: self.flags(l),
            mask,
            name: l.props.name.clone(),
            section: None,
            protection: protection(l),
            ranges: self.color.ranges(),
        })
    }
}

fn protection(l: &Layer) -> u32 {
    let mut v = 0;
    if l.props.alpha_locked {
        v |= 0x01;
    }
    if l.props.locked {
        v |= 0x8000_0000;
    }
    v
}

fn put_rect(v: &mut Vec<u8>, r: RectI) {
    for c in [r.y0, r.x0, r.y1, r.x1] {
        v.extend_from_slice(&c.to_be_bytes());
    }
}

fn tagged(v: &mut Vec<u8>, key: &[u8; 4], mut data: Vec<u8>) -> Result<(), VeskraError> {
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    v.extend_from_slice(b"8BIM");
    v.extend_from_slice(key);
    v.extend_from_slice(&u32_len(data.len(), "tagged block")?.to_be_bytes());
    v.extend_from_slice(&data);
    Ok(())
}

/// Legacy Pascal layer name: ASCII printable (others → `?`), ≤ 255 bytes, padded to 4 incl. length.
fn pascal_name(name: &str) -> Vec<u8> {
    let bytes: Vec<u8> =
        name.chars().map(|c| if (' '..='~').contains(&c) { c as u8 } else { b'?' }).take(255).collect();
    let mut v = Vec::with_capacity(bytes.len() + 4);
    v.push(bytes.len() as u8);
    v.extend_from_slice(&bytes);
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
    v
}

fn record_bytes(r: &Record) -> Result<Vec<u8>, VeskraError> {
    let mut v = Vec::with_capacity(128);
    put_rect(&mut v, r.rect);
    v.extend_from_slice(&(r.channels.len() as u16).to_be_bytes());
    for (id, data) in &r.channels {
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(&u32_len(data.len(), "channel data")?.to_be_bytes());
    }
    v.extend_from_slice(b"8BIM");
    v.extend_from_slice(&r.blend);
    v.extend_from_slice(&[r.opacity, r.clipping, r.flags, 0]);
    let mut extra = Vec::with_capacity(96);
    match r.mask {
        Some((mr, def)) => {
            extra.extend_from_slice(&20u32.to_be_bytes());
            put_rect(&mut extra, mr);
            extra.extend_from_slice(&[def, 0, 0, 0]); // default color, flags (absolute, enabled), padding
        }
        None => extra.extend_from_slice(&0u32.to_be_bytes()),
    }
    // Blending ranges: gray + one pair per channel, all "pass everything" (0..255 → 0..255).
    extra.extend_from_slice(&(4 * r.ranges as u32).to_be_bytes());
    for _ in 0..r.ranges {
        extra.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);
    }
    extra.extend_from_slice(&pascal_name(&r.name));
    let units: Vec<u16> = r.name.encode_utf16().collect();
    let mut luni = Vec::with_capacity(4 + units.len() * 2);
    luni.extend_from_slice(&u32_len(units.len(), "layer name")?.to_be_bytes());
    for u in units {
        luni.extend_from_slice(&u.to_be_bytes());
    }
    tagged(&mut extra, b"luni", luni)?;
    if let Some((t, key)) = r.section {
        let mut d = t.to_be_bytes().to_vec();
        d.extend_from_slice(b"8BIM");
        d.extend_from_slice(&key);
        tagged(&mut extra, b"lsct", d)?;
    }
    if r.protection != 0 {
        tagged(&mut extra, b"lspf", r.protection.to_be_bytes().to_vec())?;
    }
    v.extend_from_slice(&u32_len(extra.len(), "layer extra data")?.to_be_bytes());
    v.extend_from_slice(&extra);
    Ok(v)
}

fn resource(v: &mut Vec<u8>, id: u16, data: &[u8]) -> Result<(), VeskraError> {
    v.extend_from_slice(b"8BIM");
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&[0, 0]); // empty Pascal name padded to even
    v.extend_from_slice(&u32_len(data.len(), "image resource")?.to_be_bytes());
    v.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        v.push(0);
    }
    Ok(())
}

/// PSD exporter. Stateless.
#[derive(Debug, Default, Clone)]
pub struct PsdExporter;

impl PsdExporter {
    fn blocked(doc: &Document, opts: &ExportOptions) -> Option<String> {
        if let Err(e) = opts.validate(ExportFormat::Psd) {
            return Some(e.message);
        }
        if opts.raster_scale != 1.0 {
            return Some("PSD export writes document pixels 1:1; set raster_scale to 1.0.".into());
        }
        if doc.width() > PSD_MAX_DIM || doc.height() > PSD_MAX_DIM {
            return Some(format!(
                "Document exceeds the PSD limit of {PSD_MAX_DIM} px per edge (PSB is not supported)."
            ));
        }
        None
    }
}

impl Exporter for PsdExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Psd
    }

    fn preflight(&self, doc: &Document, opts: &ExportOptions) -> ExportPreflight {
        let a = analyze(doc);
        let mut pf = ExportPreflight {
            format: self.format(),
            options: opts.clone(),
            blocked_reason: Self::blocked(doc, opts),
            vector_path_count: 0,
            raster_only_stroke_count: raster_only_stroke_count(doc),
            rasterized_layer_count: a.rasterized_layers,
            profile: String::new(),
            width_px: doc.width(),
            height_px: doc.height(),
            estimated_bytes: a.estimated_bytes * if opts.bit_depth == 16 { 2 } else { 1 },
            warnings: Vec::new(),
            target_profile: String::new(),
            bit_depth: opts.bit_depth,
            rendering_intent: None,
            gamut_clipping_possible: false,
            total_area_coverage_max: None,
            cmyk_profile: None,
        };
        if opts.is_cmyk() {
            // Five channels instead of four.
            pf.estimated_bytes = pf.estimated_bytes / 4 * 5;
            match CmykPlan::new(doc, opts, self.format()) {
                Ok(cp) => {
                    pf.target_profile = cp.target_name.clone();
                    pf.rendering_intent = Some(cp.intent_name.clone());
                    pf.gamut_clipping_possible = true;
                    if cp.embed {
                        pf.estimated_bytes += cp.profile_bytes().len() as u64;
                    }
                    pf.cmyk_profile = Some(cp.info.clone());
                    pf.warnings.extend(cp.warnings.iter().cloned());
                    if pf.blocked_reason.is_none() {
                        let (tac, w) = cp.preflight_warnings(doc);
                        pf.total_area_coverage_max = tac;
                        pf.warnings.extend(w);
                    }
                    pf.profile = cp.label;
                }
                Err(e) => {
                    if pf.blocked_reason.is_none() {
                        pf.blocked_reason = Some(e.message);
                    }
                }
            }
            pf.warnings.extend(a.warnings);
            return pf;
        }
        match color_plan(doc, opts, self.format()) {
            Ok(cp) => {
                pf.target_profile = cp.target_name.clone();
                pf.rendering_intent = cp.intent();
                pf.gamut_clipping_possible = cp.clip_possible;
                if let Some(p) = &cp.embed {
                    pf.estimated_bytes += p.bytes().len() as u64;
                }
                pf.profile = cp.label;
                pf.warnings.extend(cp.warnings);
            }
            Err(e) => {
                if pf.blocked_reason.is_none() {
                    pf.blocked_reason = Some(e.message);
                }
            }
        }
        pf.warnings.extend(a.warnings);
        pf
    }

    fn export(
        &self,
        doc: &Document,
        opts: &ExportOptions,
        out: &mut dyn Write,
        cancel: &CancelToken,
    ) -> Result<ExportReport, VeskraError> {
        let start = std::time::Instant::now();
        if let Some(reason) = Self::blocked(doc, opts) {
            let code = if opts.raster_scale != 1.0 || opts.validate(self.format()).is_err() {
                ErrorCode::InvalidArgument
            } else {
                ErrorCode::PsdEncodeError
            };
            return Err(VeskraError::new(code, reason));
        }
        cancel.check()?;
        let cmyk = if opts.is_cmyk() { Some(CmykPlan::new(doc, opts, self.format())?) } else { None };
        let cp = if cmyk.is_none() { Some(color_plan(doc, opts, self.format())?) } else { None };
        let pixels = match (&cp, &cmyk) {
            (Some(c), _) => Pixels::Rgb(c),
            (None, Some(k)) => Pixels::Cmyk(k),
            (None, None) => return Err(VeskraError::new(ErrorCode::Internal, "no color plan")),
        };
        let analysis = analyze(doc);
        let (w, h) = (doc.width(), doc.height());
        let mut written = 0u64;
        let mut emit = |out: &mut dyn Write, b: &[u8]| -> Result<(), VeskraError> {
            out.write_all(b).map_err(io_err)?;
            written += b.len() as u64;
            Ok(())
        };

        // Layer records first (in memory) so the header channel count is known.
        let deep = opts.bit_depth == 16;
        let bps: usize = if deep { 2 } else { 1 };
        let enc = Encoder { doc, color: pixels, cancel, canvas: doc.bounds(), deep };
        let mut records = Vec::new();
        enc.walk(doc.root(), &mut records)?;
        let n_layers = i16::try_from(records.len()).map_err(|_| psd_err("too many layer records for PSD"))?;
        let color_channels = pixels.color_channels() as u16;
        let channels: u16 = if records.is_empty() { color_channels } else { color_channels + 1 };
        // PSD color modes: 3 = RGB, 4 = CMYK.
        let mode: u16 = if cmyk.is_some() { 4 } else { 3 };
        let embed: Option<&[u8]> = match pixels {
            Pixels::Rgb(c) => c.embed.as_deref().map(|p| p.bytes()),
            Pixels::Cmyk(k) => k.embed.then(|| k.profile_bytes()),
        };

        // Header, color mode data, image resources.
        let mut head = Vec::with_capacity(64);
        head.extend_from_slice(b"8BPS");
        head.extend_from_slice(&1u16.to_be_bytes());
        head.extend_from_slice(&[0; 6]);
        head.extend_from_slice(&channels.to_be_bytes());
        head.extend_from_slice(&h.to_be_bytes());
        head.extend_from_slice(&w.to_be_bytes());
        head.extend_from_slice(&(opts.bit_depth as u16).to_be_bytes());
        head.extend_from_slice(&mode.to_be_bytes());
        head.extend_from_slice(&0u32.to_be_bytes());
        let mut res = Vec::new();
        let fixed = ((doc.dpi as f64) * 65536.0).round().clamp(65536.0, u32::MAX as f64) as u32;
        let mut ri = Vec::with_capacity(16);
        for _ in 0..2 {
            ri.extend_from_slice(&fixed.to_be_bytes());
            ri.extend_from_slice(&1u16.to_be_bytes()); // pixels per inch
            ri.extend_from_slice(&1u16.to_be_bytes()); // display unit: inches
        }
        resource(&mut res, 1005, &ri)?;
        if let Some(p) = embed {
            resource(&mut res, 1039, p)?;
        }
        head.extend_from_slice(&u32_len(res.len(), "image resources")?.to_be_bytes());
        head.extend_from_slice(&res);
        emit(out, &head)?;

        // Layer and mask information.
        let mut li = Vec::new();
        if !records.is_empty() {
            li.extend_from_slice(&(-n_layers).to_be_bytes());
            for r in &records {
                li.extend_from_slice(&record_bytes(r)?);
            }
            for r in &records {
                for (_, d) in &r.channels {
                    li.extend_from_slice(d);
                }
            }
            while !li.len().is_multiple_of(4) {
                li.push(0);
            }
        }
        drop(records);
        cancel.check()?;
        let mut lm = Vec::with_capacity(li.len() + 24);
        if deep && !li.is_empty() {
            // 16-bit files: empty layer info, then the records in an `Lr16` tagged block.
            lm.extend_from_slice(&0u32.to_be_bytes()); // layer info length
            lm.extend_from_slice(&0u32.to_be_bytes()); // global layer mask info: none
            lm.extend_from_slice(b"8BIM");
            lm.extend_from_slice(b"Lr16");
            lm.extend_from_slice(&u32_len(li.len(), "Lr16 layer info")?.to_be_bytes());
            lm.extend_from_slice(&li);
        } else {
            lm.extend_from_slice(&u32_len(li.len(), "layer info")?.to_be_bytes());
            lm.extend_from_slice(&li);
            lm.extend_from_slice(&0u32.to_be_bytes()); // global layer mask info: none
        }
        drop(li);
        emit(out, &u32_len(lm.len(), "layer and mask section")?.to_be_bytes())?;
        emit(out, &lm)?;
        drop(lm);

        // Merged composite (R, G, B matted over white; A) — or C, M, Y, K matted over paper; A.
        let mut planes: [RlePlane; 5] = Default::default();
        let mut row: Vec<Vec<u8>> = vec![vec![0u8; w as usize * bps]; 5];
        let mut y = 0u32;
        let mut cmyk_buf = Vec::new();
        while y < h {
            cancel.check()?;
            let y1 = (y + BAND_ROWS).min(h);
            let band = RectI::new(0, y as i32, w as i32, y1 as i32);
            let color = match pixels {
                Pixels::Rgb(c) => c,
                Pixels::Cmyk(k) => {
                    let px = veskra_compose::composite_rect(doc, band, &ComposeOptions::default());
                    if px.len() != w as usize * (y1 - y) as usize * 4 {
                        return Err(VeskraError::new(ErrorCode::Internal, "compositor returned a short band"));
                    }
                    k.convert(&px, &mut cmyk_buf, true);
                    for r in cmyk_buf.chunks_exact(w as usize * 5) {
                        for (x, s) in r.chunks_exact(5).enumerate() {
                            let a = s[4];
                            for c in 0..4 {
                                // Ink over paper white (ink · alpha), stored inverted (max = no ink).
                                let m = 65535 - ((s[c] as u32 * a as u32 + 32767) / 65535) as u16;
                                if deep {
                                    row[c][x * 2..x * 2 + 2].copy_from_slice(&m.to_be_bytes());
                                } else {
                                    row[c][x] = convert::u16_to_u8(m);
                                }
                            }
                            if deep {
                                row[4][x * 2..x * 2 + 2].copy_from_slice(&a.to_be_bytes());
                            } else {
                                row[4][x] = convert::u16_to_u8(a);
                            }
                        }
                        for c in 0..channels as usize {
                            planes[c].push_row(&row[c])?;
                        }
                    }
                    y = y1;
                    continue;
                }
            };
            let px = composite_band(doc, band, color);
            if px.len() != w as usize * (y1 - y) as usize * 4 {
                return Err(VeskraError::new(ErrorCode::Internal, "compositor returned a short band"));
            }
            for r in px.chunks_exact(w as usize * 4) {
                for (x, p) in r.chunks_exact(4).enumerate() {
                    let a = p[3];
                    for c in 0..3 {
                        // Premultiplied color over white: c + (1 - a), then the contract u16 → u8 rounding.
                        let m = (p[c].min(a) as u32 + (65535 - a as u32)) as u16;
                        if deep {
                            row[c][x * 2..x * 2 + 2].copy_from_slice(&m.to_be_bytes());
                        } else {
                            row[c][x] = convert::u16_to_u8(m);
                        }
                    }
                    if deep {
                        row[3][x * 2..x * 2 + 2].copy_from_slice(&a.to_be_bytes());
                    } else {
                        row[3][x] = convert::u16_to_u8(a);
                    }
                }
                for c in 0..channels as usize {
                    planes[c].push_row(&row[c])?;
                }
            }
            y = y1;
        }
        let mut merged = Vec::new();
        merged.extend_from_slice(&1u16.to_be_bytes());
        for p in planes.iter().take(channels as usize) {
            for c in &p.counts {
                merged.extend_from_slice(&c.to_be_bytes());
            }
        }
        for p in planes.iter().take(channels as usize) {
            merged.extend_from_slice(&p.data);
        }
        cancel.check()?;
        emit(out, &merged)?;
        out.flush().map_err(io_err)?;

        let embedded = embed.is_some();
        let (mut warnings, target_profile, rendering_intent, out_of_gamut_pixels, profile, tac, cmyk_profile) =
            match pixels {
                Pixels::Rgb(cp) => {
                    let mut w = cp.warnings.clone();
                    w.extend(cp.clip_warning());
                    (w, cp.target_name.clone(), cp.intent(), cp.clipped(), cp.label.clone(), None, None)
                }
                Pixels::Cmyk(k) => {
                    let mut w = k.warnings.clone();
                    w.extend(k.gamut_warning());
                    let s = k.stats();
                    (
                        w,
                        k.target_name.clone(),
                        Some(k.intent_name.clone()),
                        s.out_of_gamut,
                        k.label.clone(),
                        Some(k.tac()),
                        Some(k.info.clone()),
                    )
                }
            };
        warnings.extend(analysis.warnings);
        Ok(ExportReport {
            success: false, // set by export_to_path after flush + validation (MASTER_PLAN §7.6)
            file_uri: None,
            format: self.format(),
            target_profile,
            bit_depth: opts.bit_depth,
            rendering_intent,
            out_of_gamut_pixels,
            total_area_coverage_max: tac,
            cmyk_profile,
            profile,
            profile_embedded: embedded,
            vector_object_count: 0,
            rasterized_object_count: analysis.rasterized_layers,
            flattened_feature_list: analysis.flattened,
            warnings,
            duration_ms: start.elapsed().as_millis() as u64,
            file_bytes: written,
            width_px: w,
            height_px: h,
            validation: ValidationStatus::NotValidated,
        })
    }

    fn validate(&self, bytes: &[u8]) -> Result<(), VeskraError> {
        inspect_psd(bytes, false).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_names_are_padded() {
        assert_eq!(pascal_name(""), vec![0, 0, 0, 0]);
        assert_eq!(pascal_name("abc"), vec![3, b'a', b'b', b'c']);
        assert_eq!(pascal_name("abcd").len(), 8);
        assert_eq!(pascal_name("Ä"), vec![1, b'?', 0, 0]);
        assert_eq!(pascal_name(&"x".repeat(300))[0], 255);
    }

    #[test]
    fn opacity_rounding() {
        assert_eq!(opacity_byte(0.0), 0);
        assert_eq!(opacity_byte(1.0), 255);
        assert_eq!(opacity_byte(0.5), 128);
    }

    #[test]
    fn rect_fitting() {
        let canvas = RectI::from_size(100, 100);
        assert_eq!(fit_rect(RectI::new(-10, -10, 50, 50), canvas), (RectI::new(-10, -10, 50, 50), false));
        assert_eq!(fit_rect(RectI::new(-40_000, 0, 50, 50), canvas), (RectI::new(0, 0, 50, 50), true));
    }
}
