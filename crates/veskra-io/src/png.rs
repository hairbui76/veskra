//! PNG exporter (FMT-07) and raster-export helpers shared with the PSD writer.
//!
//! # Output format (documented choices)
//! * Always **RGBA, straight alpha, non-interlaced** (color type 6), 8 bits per channel by default or
//!   16 bits with `bit_depth = 16` (big-endian samples, straight values from
//!   `veskra_color::convert::premul16_to_straight16`, so nothing is quantized to 8 bits on the way).
//!   We do not pre-scan the image to choose RGB for fully opaque documents: that would need a second
//!   full composite pass.
//! * Pixels come from the deterministic CPU compositor (`veskra-compose`), composited in row bands of
//!   [`BAND_ROWS`] (bounded memory, cancellation checked per band) and converted u16 premultiplied →
//!   8-bit straight exclusively by `veskra_color::convert::premul16_to_straight8` (no halos: alpha 0
//!   becomes `(0,0,0,0)`).
//! * zlib stream via `miniz_oxide` (streamed into ≤ 64 KiB `IDAT` chunks), CRC via `crc32fast`,
//!   per-row adaptive filter (minimum sum of absolute differences heuristic).
//!
//! # Color (COL-001/COL-002)
//! * `embed_color_profile = true` → `iCCP` with the document's exact ICC bytes (name from the profile
//!   description, Latin-1, 1–79 bytes). No `sRGB` chunk alongside (the PNG spec forbids both). For an
//!   sRGB-equivalent profile, `gAMA` + `cHRM` sRGB fallback values are written for readers that ignore
//!   `iCCP` (PNG spec recommendation).
//! * not embedding, sRGB document → `sRGB` chunk with rendering intent **1 (relative colorimetric,
//!   Veskra's default transform intent)** plus the spec-recommended `gAMA`/`cHRM` values.
//! * not embedding, non-sRGB document → pixels converted to sRGB with `RgbTransform` (relative
//!   colorimetric) and tagged `sRGB`; a `ProfileNotEmbedded` warning reports the conversion.
//! * `profile = srgb | display_p3 | adobe_rgb` (embedding on): pixels are converted from the document
//!   space to that target with the **relative colorimetric** intent, in `f64` on the 16-bit
//!   composite, and the target's ICC bytes are embedded (`iCCP`). When the document already is in the
//!   target space nothing is converted and the document's own profile bytes are embedded, so an sRGB
//!   document exported as `srgb` is byte-identical to the default export. Out-of-gamut pixels are
//!   clipped per channel and **counted** (`ExportReport::out_of_gamut_pixels`, `GamutClipped`
//!   warning). A wide-gamut target without embedding is rejected (`IncompatibleOutputProfile`).
//!
//! # `raster_scale`
//! Implemented: area-average (box) filter when downscaling, bilinear (pixel-centre aligned, edge
//! clamped) when upscaling, both in **premultiplied** u16 space so no dark/colored fringes appear.
//! Output size is `max(1, round(w · s))`; `pHYs` DPI is scaled so the physical size is unchanged.

use crate::{CancelToken, Exporter};
use miniz_oxide::deflate::core::CompressorOxide;
use miniz_oxide::inflate::stream::InflateState;
use miniz_oxide::{DataFormat, MZFlush, MZStatus};
use std::cell::Cell;
use std::io::Write;
use std::sync::Arc;
use veskra_color::{IccProfile, RenderingIntent, RgbTransform, convert};
use veskra_compose::ComposeOptions;
use veskra_core::export::{
    ExportFormat, ExportOptions, ExportPreflight, ExportProfile, ExportReport, ExportWarning, ExportWarningKind,
    ValidationStatus,
};
use veskra_core::{BlendSpace, ErrorCode, RectI, VeskraError};
use veskra_doc::{Document, LayerKind};

/// Rows composited per band (one tile row; bounds memory to `width · 64 · 8` bytes per band).
pub const BAND_ROWS: u32 = 64;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
const IDAT_CHUNK: usize = 64 * 1024;
/// sRGB `cHRM` values (×100000) from the PNG specification.
const SRGB_CHRM: [u32; 8] = [31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000];
/// `gAMA` for sRGB (1/2.2 × 100000) from the PNG specification.
const SRGB_GAMA: u32 = 45455;
/// `sRGB` chunk rendering intent: 1 = relative colorimetric.
const SRGB_INTENT: u8 = 1;

// ---------------------------------------------------------------------------------------------
// Shared helpers (PNG + PSD)
// ---------------------------------------------------------------------------------------------

/// The document's working profile, parsed. Fails with `InvalidProfile` if the document carries
/// bytes that cannot be parsed (a validated document never does).
pub(crate) fn document_profile(doc: &Document) -> Result<Arc<IccProfile>, VeskraError> {
    let srgb = IccProfile::srgb();
    if doc.color.icc_bytes.as_ref() == srgb.bytes() {
        return Ok(srgb);
    }
    IccProfile::parse(&doc.color.icc_bytes)
        .map(Arc::new)
        .map_err(|e| VeskraError::new(ErrorCode::InvalidProfile, format!("document ICC profile unusable: {e}")))
}

/// How pixel values and profile tags are written for one export.
#[derive(Debug)]
pub(crate) struct ColorPlan {
    /// Profile whose bytes are embedded (`Some` only when embedding).
    pub embed: Option<Arc<IccProfile>>,
    /// Transform applied to premultiplied pixels before writing (document space → written space).
    pub convert: Option<RgbTransform>,
    /// Human-readable profile semantics for preflight/report.
    pub label: String,
    pub warnings: Vec<ExportWarning>,
    /// Name of the color space the written values are in.
    pub target_name: String,
    /// The conversion can clip (the document gamut is not contained in the target gamut).
    pub clip_possible: bool,
    /// Pixels clipped so far by [`ColorPlan::convert_counting`].
    clipped: Cell<u64>,
}

impl ColorPlan {
    /// Convert premultiplied RGBA16 pixels to the written color space (no-op without a transform).
    pub(crate) fn convert_px(&self, px: &mut [u16]) {
        if let Some(t) = &self.convert {
            t.apply_rgba16_premul(px);
        }
    }

    /// [`convert_px`](Self::convert_px) that also counts out-of-gamut (clipped) pixels. Used for
    /// the flattened image only, so layer data never counts a pixel twice.
    pub(crate) fn convert_counting(&self, px: &mut [u16]) {
        if let Some(t) = &self.convert {
            self.clipped.set(self.clipped.get().saturating_add(t.apply_rgba16_premul_counting(px)));
        }
    }

    pub(crate) fn clipped(&self) -> u64 {
        self.clipped.get()
    }

    /// `"relative-colorimetric"` when pixels are converted.
    pub(crate) fn intent(&self) -> Option<String> {
        self.convert.as_ref().map(|t| t.intent().as_str().to_string())
    }

    /// Warning for the pixels actually clipped during this export (`None` if there were none).
    pub(crate) fn clip_warning(&self) -> Option<ExportWarning> {
        gamut_clip_warning(self.clipped(), &self.target_name)
    }
}

/// `GamutClipped` warning for `n` clipped pixels (`None` for 0).
pub(crate) fn gamut_clip_warning(n: u64, target: &str) -> Option<ExportWarning> {
    (n > 0).then(|| {
        warning(
            ExportWarningKind::GamutClipped,
            format!(
                "{n} pixel(s) had colors outside '{target}' and were clipped to its gamut (relative colorimetric)."
            ),
            None,
            u32::try_from(n).unwrap_or(u32::MAX),
        )
    })
}

/// The ICC profile of an explicit export target (`None` for `document`).
pub(crate) fn target_profile(p: ExportProfile) -> Option<Arc<IccProfile>> {
    match p {
        ExportProfile::Srgb => Some(IccProfile::srgb()),
        ExportProfile::DisplayP3 => Some(IccProfile::display_p3()),
        ExportProfile::AdobeRgb => Some(IccProfile::adobe_rgb_compatible()),
        _ => None,
    }
}

pub(crate) fn color_plan(doc: &Document, opts: &ExportOptions, format: ExportFormat) -> Result<ColorPlan, VeskraError> {
    let profile = document_profile(doc)?;
    let name = profile.id().semantic_name;
    let assumed = if doc.color.assumed { ", assumed" } else { "" };
    let is_srgb = profile.is_srgb_equivalent();
    // Explicit target: convert unless the document already is in that space (then the plain
    // "embed the document profile" path below is taken, keeping its bytes identical).
    if opts.embed_color_profile
        && let Some(target) = target_profile(opts.profile)
    {
        let t = RgbTransform::new(&profile, &target, RenderingIntent::RelativeColorimetric).map_err(|e| {
            VeskraError::new(ErrorCode::ColorTransformFailure, format!("cannot convert to the target profile: {e}"))
        })?;
        if !t.is_identity() {
            let tname = target.id().semantic_name;
            let fits = t.source_gamut_fits();
            let mut warnings = vec![warning(
                ExportWarningKind::ColorConverted,
                format!(
                    "Pixels are converted from '{name}' to '{tname}' (relative colorimetric) and the '{tname}' profile is embedded."
                ),
                None,
                0,
            )];
            if !fits {
                warnings.push(warning(
                    ExportWarningKind::GamutClipped,
                    format!(
                        "'{name}' can hold colors outside '{tname}'; they are clipped to the target gamut and counted in the export report."
                    ),
                    None,
                    0,
                ));
            }
            return Ok(ColorPlan {
                label: format!("{tname} (embedded, converted from {name}{assumed}, relative colorimetric)"),
                embed: Some(target),
                convert: Some(t),
                warnings,
                target_name: tname,
                clip_possible: !fits,
                clipped: Cell::new(0),
            });
        }
    }
    if opts.embed_color_profile {
        return Ok(ColorPlan {
            label: format!("{name} (embedded{assumed})"),
            embed: Some(profile),
            convert: None,
            warnings: Vec::new(),
            target_name: name,
            clip_possible: false,
            clipped: Cell::new(0),
        });
    }
    let tag = match format {
        ExportFormat::Png => "sRGB chunk",
        _ => "untagged",
    };
    if is_srgb {
        let mut warnings = Vec::new();
        if format != ExportFormat::Png {
            warnings.push(warning(
                ExportWarningKind::ProfileNotEmbedded,
                "Color profile not embedded (option off); values are sRGB and the file is untagged.",
                None,
                1,
            ));
        }
        return Ok(ColorPlan {
            embed: None,
            convert: None,
            label: format!("sRGB IEC61966-2.1 values ({tag}, profile not embedded{assumed})"),
            warnings,
            target_name: veskra_color::icc::SRGB_SEMANTIC_NAME.to_string(),
            clip_possible: false,
            clipped: Cell::new(0),
        });
    }
    let t = RgbTransform::new(&profile, &IccProfile::srgb(), RenderingIntent::RelativeColorimetric)
        .map_err(|e| VeskraError::new(ErrorCode::ColorTransformFailure, format!("cannot convert to sRGB: {e}")))?;
    Ok(ColorPlan {
        embed: None,
        clip_possible: !t.source_gamut_fits(),
        convert: if t.is_identity() { None } else { Some(t) },
        target_name: veskra_color::icc::SRGB_SEMANTIC_NAME.to_string(),
        clipped: Cell::new(0),
        label: format!("sRGB IEC61966-2.1 values ({tag}, converted from {name}, profile not embedded)"),
        warnings: vec![warning(
            ExportWarningKind::ProfileNotEmbedded,
            format!(
                "Profile '{name}' not embedded (option off): pixels converted to sRGB (relative colorimetric); out-of-gamut colors are clipped."
            ),
            None,
            1,
        )],
    })
}

pub(crate) fn warning(
    kind: ExportWarningKind,
    message: impl Into<String>,
    layer: Option<String>,
    count: u32,
) -> ExportWarning {
    ExportWarning { kind, message: message.into(), layer, count }
}

/// Composite `rect` of the visible document (premultiplied RGBA16, working space), converted to
/// the written color space of `color` (out-of-gamut pixels are counted there).
pub(crate) fn composite_band(doc: &Document, rect: RectI, color: &ColorPlan) -> Vec<u16> {
    let mut px = veskra_compose::composite_rect(doc, rect, &ComposeOptions::default());
    color.convert_counting(&mut px);
    px
}

/// Number of pixel layers (raster+vector) that will be invisible in the composite.
pub(crate) fn hidden_layer_count(doc: &Document) -> u32 {
    doc.iter_layers().filter(|l| !l.props.visible).count() as u32
}

pub(crate) fn vector_layer_count(doc: &Document) -> u32 {
    doc.iter_layers().filter(|l| matches!(l.kind, LayerKind::Vector(_))).count() as u32
}

pub(crate) fn raster_only_stroke_count(doc: &Document) -> u32 {
    doc.iter_strokes().filter(|s| !s.has_vector_form()).count() as u32
}

fn png_err(msg: impl Into<String>) -> VeskraError {
    VeskraError::new(ErrorCode::PngEncodeError, msg)
}

fn io_err(e: std::io::Error) -> VeskraError {
    let code = if e.kind() == std::io::ErrorKind::StorageFull { ErrorCode::StorageFull } else { ErrorCode::Io };
    VeskraError::new(code, format!("write failed: {}", e.kind()))
}

// ---------------------------------------------------------------------------------------------
// Resampling (raster_scale)
// ---------------------------------------------------------------------------------------------

/// Output size for `raster_scale` (each edge `max(1, round(edge · s))`).
pub(crate) fn scaled_size(w: u32, h: u32, scale: f32) -> Result<(u32, u32), VeskraError> {
    if !(scale.is_finite() && scale > 0.0 && scale <= 4.0) {
        return Err(VeskraError::invalid("raster_scale must be in (0, 4]"));
    }
    let f = |v: u32| ((v as f64 * scale as f64).round() as u64).max(1);
    let (ow, oh) = (f(w), f(h));
    if ow > i32::MAX as u64 / 2 || oh > i32::MAX as u64 / 2 {
        return Err(VeskraError::alloc("scaled output too large"));
    }
    Ok((ow as u32, oh as u32))
}

/// Separable filter taps for one output coordinate.
#[derive(Debug, Clone)]
struct Taps {
    start: u32,
    weights: Vec<f32>,
}

/// Taps mapping `src` samples to `dst` samples (box when shrinking, bilinear when enlarging).
fn filter_taps(src: u32, dst: u32) -> Vec<Taps> {
    let s = dst as f64 / src as f64;
    (0..dst)
        .map(|o| {
            if dst < src {
                let (a, b) = (o as f64 / s, (o as f64 + 1.0) / s);
                let i0 = a.floor().max(0.0) as u32;
                let i1 = (b.ceil() as u32).min(src);
                let mut w: Vec<f32> =
                    (i0..i1).map(|i| ((b.min(i as f64 + 1.0) - a.max(i as f64)).max(0.0) * s) as f32).collect();
                let sum: f32 = w.iter().sum();
                if sum > 0.0 {
                    w.iter_mut().for_each(|v| *v /= sum);
                }
                Taps { start: i0, weights: w }
            } else {
                let c = (o as f64 + 0.5) / s - 0.5;
                let i0 = c.floor();
                let t = (c - i0) as f32;
                let lo = (i0.max(0.0) as u32).min(src - 1);
                let hi = ((i0 + 1.0).max(0.0) as u32).min(src - 1);
                if lo == hi {
                    Taps { start: lo, weights: vec![1.0] }
                } else {
                    Taps { start: lo, weights: vec![1.0 - t, t] }
                }
            }
        })
        .collect()
}

/// Produces output rows (premultiplied RGBA16) of the composite at the export scale, compositing
/// the source in bands and keeping only the current band in memory.
pub(crate) struct ScaledRows<'a> {
    doc: &'a Document,
    color: &'a ColorPlan,
    cancel: &'a CancelToken,
    src_w: u32,
    src_h: u32,
    pub out_w: u32,
    pub out_h: u32,
    band: Option<(u32, Vec<u16>)>,
    xt: Vec<Taps>,
    yt: Vec<Taps>,
    identity: bool,
}

impl std::fmt::Debug for ScaledRows<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScaledRows").field("out_w", &self.out_w).field("out_h", &self.out_h).finish()
    }
}

impl<'a> ScaledRows<'a> {
    pub fn new(
        doc: &'a Document,
        scale: f32,
        color: &'a ColorPlan,
        cancel: &'a CancelToken,
    ) -> Result<Self, VeskraError> {
        let (src_w, src_h) = (doc.width(), doc.height());
        let (out_w, out_h) = scaled_size(src_w, src_h, scale)?;
        let identity = out_w == src_w && out_h == src_h;
        let (xt, yt) =
            if identity { (Vec::new(), Vec::new()) } else { (filter_taps(src_w, out_w), filter_taps(src_h, out_h)) };
        Ok(Self { doc, color, cancel, src_w, src_h, out_w, out_h, band: None, xt, yt, identity })
    }

    fn src_row(&mut self, y: u32) -> Result<&[u16], VeskraError> {
        let b = y / BAND_ROWS;
        if self.band.as_ref().is_none_or(|(i, _)| *i != b) {
            self.cancel.check()?;
            let y0 = b * BAND_ROWS;
            let y1 = (y0 + BAND_ROWS).min(self.src_h);
            let r = RectI::new(0, y0 as i32, self.src_w as i32, y1 as i32);
            self.band = Some((b, composite_band(self.doc, r, self.color)));
        }
        let stride = self.src_w as usize * 4;
        let off = (y % BAND_ROWS) as usize * stride;
        self.band
            .as_ref()
            .and_then(|(_, px)| px.get(off..off + stride))
            .ok_or_else(|| VeskraError::new(ErrorCode::Internal, "compositor returned a short band"))
    }

    /// Write output row `y` (premultiplied RGBA16, `out_w · 4` values) into `dst`.
    pub fn row(&mut self, y: u32, dst: &mut [u16]) -> Result<(), VeskraError> {
        if self.identity {
            let r = self.src_row(y)?;
            dst.copy_from_slice(r);
            return Ok(());
        }
        let yt = self.yt.get(y as usize).cloned().ok_or_else(|| VeskraError::invalid("row out of range"))?;
        let mut acc = vec![0f32; self.out_w as usize * 4];
        let xt = std::mem::take(&mut self.xt);
        let res: Result<(), VeskraError> = (|| {
            for (k, wy) in yt.weights.iter().enumerate() {
                if *wy == 0.0 {
                    continue;
                }
                let src = self.src_row(yt.start + k as u32)?;
                for (ox, t) in xt.iter().enumerate() {
                    let mut p = [0f32; 4];
                    for (j, wx) in t.weights.iter().enumerate() {
                        let i = (t.start as usize + j) * 4;
                        if let Some(s) = src.get(i..i + 4) {
                            for c in 0..4 {
                                p[c] += *wx * s[c] as f32;
                            }
                        }
                    }
                    for c in 0..4 {
                        acc[ox * 4 + c] += *wy * p[c];
                    }
                }
            }
            Ok(())
        })();
        self.xt = xt;
        res?;
        for (d, a) in dst.chunks_exact_mut(4).zip(acc.chunks_exact(4)) {
            let q = |v: f32| (v + 0.5).floor().clamp(0.0, 65535.0) as u16;
            let al = q(a[3]);
            d[0] = q(a[0]).min(al);
            d[1] = q(a[1]).min(al);
            d[2] = q(a[2]).min(al);
            d[3] = al;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// PNG writer
// ---------------------------------------------------------------------------------------------

fn write_chunk(out: &mut dyn Write, ty: &[u8; 4], data: &[u8]) -> Result<(), VeskraError> {
    let len = u32::try_from(data.len()).map_err(|_| png_err("chunk too large"))?;
    let mut crc = crc32fast::Hasher::new();
    crc.update(ty);
    crc.update(data);
    out.write_all(&len.to_be_bytes()).map_err(io_err)?;
    out.write_all(ty).map_err(io_err)?;
    out.write_all(data).map_err(io_err)?;
    out.write_all(&crc.finalize().to_be_bytes()).map_err(io_err)?;
    Ok(())
}

/// Streaming zlib → `IDAT` chunk emitter.
struct IdatStream<'a> {
    out: &'a mut dyn Write,
    comp: Box<CompressorOxide>,
    buf: Vec<u8>,
    len: usize,
    bytes: u64,
}

impl<'a> IdatStream<'a> {
    fn new(out: &'a mut dyn Write) -> Self {
        let comp = Box::new(CompressorOxide::with_format_and_level(
            DataFormat::Zlib,
            miniz_oxide::deflate::CompressionLevel::DefaultLevel,
        ));
        Self { out, comp, buf: vec![0; IDAT_CHUNK], len: 0, bytes: 0 }
    }

    fn emit(&mut self) -> Result<(), VeskraError> {
        if self.len > 0 {
            write_chunk(self.out, b"IDAT", &self.buf[..self.len])?;
            self.bytes += self.len as u64 + 12;
            self.len = 0;
        }
        Ok(())
    }

    fn feed(&mut self, mut input: &[u8], flush: MZFlush) -> Result<(), VeskraError> {
        if input.is_empty() && flush != MZFlush::Finish {
            return Ok(());
        }
        loop {
            if self.len == self.buf.len() {
                self.emit()?;
            }
            let r = miniz_oxide::deflate::stream::deflate(&mut self.comp, input, &mut self.buf[self.len..], flush);
            let st = r.status.map_err(|e| png_err(format!("deflate failed: {e:?}")))?;
            input = input.get(r.bytes_consumed..).unwrap_or_default();
            self.len += r.bytes_written;
            if flush == MZFlush::Finish {
                if st == MZStatus::StreamEnd {
                    return self.emit();
                }
            } else if input.is_empty() {
                return Ok(());
            }
        }
    }
}

/// Filter one row with the minimum-sum-of-absolute-differences heuristic. `out` is `1 + row.len()`.
fn filter_row(row: &[u8], prev: &[u8], out: &mut Vec<u8>, scratch: &mut [Vec<u8>; 5], bpp: usize) {
    let n = row.len();
    for (f, s) in scratch.iter_mut().enumerate() {
        s.clear();
        s.reserve(n);
        for i in 0..n {
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let x = row[i];
            let v = match f {
                0 => x,
                1 => x.wrapping_sub(a),
                2 => x.wrapping_sub(b),
                3 => x.wrapping_sub(((a as u16 + b as u16) / 2) as u8),
                _ => x.wrapping_sub(paeth(a, b, c)),
            };
            s.push(v);
        }
    }
    let score = |s: &Vec<u8>| s.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum::<u64>();
    let mut best = 0;
    let mut best_score = u64::MAX;
    for (f, s) in scratch.iter().enumerate() {
        let sc = score(s);
        if sc < best_score {
            best = f;
            best_score = sc;
        }
    }
    out.clear();
    out.push(best as u8);
    out.extend_from_slice(&scratch[best]);
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// `iCCP` profile name: Latin-1 printable, no leading/trailing/double spaces, 1–79 bytes.
pub fn icc_profile_name(description: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for ch in description.chars() {
        let c = ch as u32;
        let b = if (32..=126).contains(&c) || (161..=255).contains(&c) { c as u8 } else { b' ' };
        if b == b' ' && (out.is_empty() || out.last() == Some(&b' ')) {
            continue;
        }
        if out.len() >= 79 {
            break;
        }
        out.push(b);
    }
    while out.last() == Some(&b' ') {
        out.pop();
    }
    if out.is_empty() {
        out.extend_from_slice(b"ICC profile");
    }
    out
}

fn ppm_from_dpi(dpi: f64) -> u32 {
    (dpi / 0.0254).round().clamp(1.0, u32::MAX as f64 / 2.0) as u32
}

/// Row producer: fills row `y` (`w · 4` straight RGBA8 bytes, or `w · 8` big-endian RGBA16 bytes).
type RowFn<'a> = dyn FnMut(u32, &mut [u8]) -> Result<(), VeskraError> + 'a;

/// Write a complete straight-alpha RGBA PNG with `depth` (8 or 16) bits per channel. `icc`: embed
/// this profile (`iCCP`); `None` → `sRGB` chunk. `rows(y, buf)` fills row `y` (`w · depth / 2` bytes).
#[allow(clippy::too_many_arguments)]
fn write_png(
    out: &mut dyn Write,
    w: u32,
    h: u32,
    depth: u8,
    icc: Option<&IccProfile>,
    dpi: Option<f64>,
    cancel: &CancelToken,
    rows: &mut RowFn<'_>,
) -> Result<(), VeskraError> {
    if depth != 8 && depth != 16 {
        return Err(png_err("PNG bit depth must be 8 or 16"));
    }
    let bpp = depth as usize / 2;
    if w == 0 || h == 0 || w > i32::MAX as u32 || h > i32::MAX as u32 {
        return Err(png_err("PNG dimensions must be in 1..2^31"));
    }
    out.write_all(&PNG_SIGNATURE).map_err(io_err)?;
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[depth, 6, 0, 0, 0]); // depth, RGBA, deflate, adaptive filter, no interlace
    write_chunk(out, b"IHDR", &ihdr)?;
    let srgb_fallback = |out: &mut dyn Write| -> Result<(), VeskraError> {
        let mut chrm = Vec::with_capacity(32);
        for v in SRGB_CHRM {
            chrm.extend_from_slice(&v.to_be_bytes());
        }
        write_chunk(out, b"cHRM", &chrm)?;
        write_chunk(out, b"gAMA", &SRGB_GAMA.to_be_bytes())
    };
    match icc {
        Some(profile) => {
            if profile.is_srgb_equivalent() {
                srgb_fallback(out)?;
            }
            let mut d = icc_profile_name(profile.description());
            d.push(0);
            d.push(0); // compression method 0 (zlib)
            d.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(profile.bytes(), 9));
            write_chunk(out, b"iCCP", &d)?;
        }
        None => {
            srgb_fallback(out)?;
            write_chunk(out, b"sRGB", &[SRGB_INTENT])?;
        }
    }
    if let Some(dpi) = dpi {
        let ppm = ppm_from_dpi(dpi);
        let mut phys = Vec::with_capacity(9);
        phys.extend_from_slice(&ppm.to_be_bytes());
        phys.extend_from_slice(&ppm.to_be_bytes());
        phys.push(1);
        write_chunk(out, b"pHYs", &phys)?;
    }
    let stride = w as usize * bpp;
    let mut row8 = vec![0u8; stride];
    let mut prev = vec![0u8; stride];
    let mut filtered = Vec::with_capacity(stride + 1);
    let mut scratch: [Vec<u8>; 5] = Default::default();
    {
        let mut idat = IdatStream::new(out);
        for y in 0..h {
            if y % BAND_ROWS == 0 {
                cancel.check()?;
            }
            rows(y, &mut row8)?;
            filter_row(&row8, &prev, &mut filtered, &mut scratch, bpp);
            idat.feed(&filtered, MZFlush::None)?;
            std::mem::swap(&mut prev, &mut row8);
        }
        idat.feed(&[], MZFlush::Finish)?;
    }
    cancel.check()?;
    write_chunk(out, b"IEND", &[])?;
    out.flush().map_err(io_err)
}

/// Encode straight-alpha RGBA8 **sRGB** pixels as an sRGB-tagged PNG (`sRGB` chunk, relative
/// colorimetric, plus `gAMA`/`cHRM` fallback; no `pHYs`). Used for `.veskra` preview thumbnails.
/// The caller guarantees the values are sRGB-encoded. Fails if `rgba_straight.len() != w · h · 4`.
pub fn encode_rgba8_srgb(width: u32, height: u32, rgba_straight: &[u8]) -> Result<Vec<u8>, VeskraError> {
    let stride = (width as usize).checked_mul(4).ok_or_else(|| VeskraError::alloc("PNG too large"))?;
    let len = stride.checked_mul(height as usize).ok_or_else(|| VeskraError::alloc("PNG too large"))?;
    if rgba_straight.len() != len {
        return Err(VeskraError::invalid("pixel buffer length must be width * height * 4"));
    }
    let mut out = Vec::with_capacity(len / 2 + 256);
    write_png(&mut out, width, height, 8, None, None, &CancelToken::new(), &mut |y, row| {
        let o = y as usize * stride;
        row.copy_from_slice(&rgba_straight[o..o + stride]);
        Ok(())
    })?;
    Ok(out)
}

/// PNG exporter (FMT-07). Stateless.
#[derive(Debug, Default, Clone)]
pub struct PngExporter;

impl PngExporter {
    fn common_warnings(doc: &Document) -> (Vec<ExportWarning>, Vec<String>, u32) {
        let mut w = Vec::new();
        let mut flat = Vec::new();
        let hidden = hidden_layer_count(doc);
        if hidden > 0 {
            w.push(warning(
                ExportWarningKind::HiddenLayer,
                format!("{hidden} hidden layer(s)/group(s) are not included in the flat PNG image."),
                None,
                hidden,
            ));
        }
        let vectors = vector_layer_count(doc);
        if vectors > 0 {
            w.push(warning(
                ExportWarningKind::LayerRasterized,
                format!(
                    "{vectors} vector-stroke layer(s) are flattened into pixels; editable geometry stays in .veskra."
                ),
                None,
                vectors,
            ));
            flat.push(format!("vector_layers_rasterized:{vectors}"));
        }
        let layers = doc.layer_count() as u32;
        if layers > 1 {
            w.push(warning(
                ExportWarningKind::FeatureFlattened,
                format!("{layers} layer node(s) are merged into one image (PNG has no layers)."),
                None,
                layers,
            ));
            flat.push(format!("layers_merged:{layers}"));
        }
        (w, flat, vectors)
    }
}

impl Exporter for PngExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Png
    }

    fn preflight(&self, doc: &Document, opts: &ExportOptions) -> ExportPreflight {
        let mut pf = ExportPreflight {
            format: self.format(),
            options: *opts,
            blocked_reason: None,
            vector_path_count: 0,
            raster_only_stroke_count: raster_only_stroke_count(doc),
            rasterized_layer_count: 0,
            profile: String::new(),
            width_px: doc.width(),
            height_px: doc.height(),
            estimated_bytes: 0,
            warnings: Vec::new(),
            target_profile: String::new(),
            bit_depth: opts.bit_depth,
            rendering_intent: None,
            gamut_clipping_possible: false,
        };
        if let Err(e) = opts.validate(self.format()) {
            pf.blocked_reason = Some(e.message);
            return pf;
        }
        match scaled_size(doc.width(), doc.height(), opts.raster_scale) {
            Ok((w, h)) => {
                pf.width_px = w;
                pf.height_px = h;
                // Upper-bound-ish estimate: raw RGBA + filter bytes (real files are usually smaller).
                pf.estimated_bytes = (w as u64 * (opts.bit_depth as u64 / 2) + 1) * h as u64 + 1024;
            }
            Err(e) => {
                pf.blocked_reason = Some(e.message);
                return pf;
            }
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
            Err(e) => pf.blocked_reason = Some(e.message),
        }
        let (w, _, vectors) = Self::common_warnings(doc);
        pf.rasterized_layer_count = vectors;
        pf.warnings.extend(w);
        if doc.color.blend_space == BlendSpace::Linear {
            pf.warnings.push(warning(
                ExportWarningKind::Other,
                "Document blends in linear light; the PNG contains Veskra's linear-light composite.",
                None,
                1,
            ));
        }
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
        opts.validate(self.format())?;
        cancel.check()?;
        let cp = color_plan(doc, opts, self.format())?;
        let mut rows = ScaledRows::new(doc, opts.raster_scale, &cp, cancel)?;
        let (w, h) = (rows.out_w, rows.out_h);
        let dpi = doc.dpi as f64 * (w as f64 / doc.width() as f64);
        let mut counter = CountingWriter { inner: out, n: 0 };
        let mut row16 = vec![0u16; w as usize * 4];
        let deep = opts.bit_depth == 16;
        let mut straight16 = vec![0u16; if deep { w as usize * 4 } else { 0 }];
        write_png(&mut counter, w, h, opts.bit_depth, cp.embed.as_deref(), Some(dpi), cancel, &mut |y, row| {
            rows.row(y, &mut row16)?;
            if deep {
                convert::premul16_to_straight16(&row16, &mut straight16);
                for (d, v) in row.chunks_exact_mut(2).zip(straight16.iter()) {
                    d.copy_from_slice(&v.to_be_bytes());
                }
            } else {
                convert::premul16_to_straight8(&row16, row);
            }
            Ok(())
        })?;
        drop(rows);

        let (mut warnings, flattened, vectors) = Self::common_warnings(doc);
        let mut all = cp.warnings.clone();
        all.extend(cp.clip_warning());
        all.append(&mut warnings);
        Ok(ExportReport {
            success: false, // set by export_to_path after flush + validation (MASTER_PLAN §7.6)
            file_uri: None,
            format: self.format(),
            target_profile: cp.target_name.clone(),
            bit_depth: opts.bit_depth,
            rendering_intent: cp.intent(),
            out_of_gamut_pixels: cp.clipped(),
            profile: cp.label,
            profile_embedded: cp.embed.is_some(),
            vector_object_count: 0,
            rasterized_object_count: vectors,
            flattened_feature_list: flattened,
            warnings: all,
            duration_ms: start.elapsed().as_millis() as u64,
            file_bytes: counter.n,
            width_px: w,
            height_px: h,
            validation: ValidationStatus::NotValidated,
        })
    }

    fn validate(&self, bytes: &[u8]) -> Result<(), VeskraError> {
        inspect_png(bytes).map(|_| ())
    }
}

struct CountingWriter<'a> {
    inner: &'a mut dyn Write,
    n: u64,
}

impl Write for CountingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let k = self.inner.write(buf)?;
        self.n += k as u64;
        Ok(k)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

// ---------------------------------------------------------------------------------------------
// Structural validation
// ---------------------------------------------------------------------------------------------

/// What [`inspect_png`] found in a structurally valid file.
#[derive(Debug, Clone, PartialEq)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub color_type: u8,
    /// Bits per channel (8 or 16).
    pub bit_depth: u8,
    /// Chunk types in file order.
    pub chunks: Vec<[u8; 4]>,
    /// Decompressed `iCCP` profile (name, bytes).
    pub icc: Option<(Vec<u8>, Vec<u8>)>,
    pub srgb_intent: Option<u8>,
    /// Pixels per metre (x, y) when the unit is metres.
    pub ppm: Option<(u32, u32)>,
}

/// Re-read a PNG produced by [`PngExporter`]: signature, chunk lengths and CRCs, IHDR sanity,
/// chunk ordering, iCCP/sRGB exclusivity, and the IDAT zlib stream inflating to exactly
/// `height · (1 + width · bpp)` bytes with valid filter types. Bounded memory (streaming inflate).
pub fn inspect_png(bytes: &[u8]) -> Result<PngInfo, VeskraError> {
    let bad = |m: &str| png_err(format!("invalid PNG: {m}"));
    if bytes.get(..8) != Some(&PNG_SIGNATURE[..]) {
        return Err(bad("signature"));
    }
    let mut pos = 8usize;
    let mut info = PngInfo {
        width: 0,
        height: 0,
        color_type: 0,
        bit_depth: 0,
        chunks: Vec::new(),
        icc: None,
        srgb_intent: None,
        ppm: None,
    };
    let mut idat: Vec<&[u8]> = Vec::new();
    let mut idat_done = false;
    let mut seen_iend = false;
    let mut bpp = 4usize;
    while pos < bytes.len() {
        if seen_iend {
            return Err(bad("data after IEND"));
        }
        let hdr = bytes.get(pos..pos + 8).ok_or_else(|| bad("truncated chunk header"))?;
        let len = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
        if len > i32::MAX as usize {
            return Err(bad("chunk length"));
        }
        let ty: [u8; 4] = [hdr[4], hdr[5], hdr[6], hdr[7]];
        if !ty.iter().all(|c| c.is_ascii_alphabetic()) {
            return Err(bad("chunk type"));
        }
        let data = bytes.get(pos + 8..pos + 8 + len).ok_or_else(|| bad("truncated chunk data"))?;
        let crc_b = bytes.get(pos + 8 + len..pos + 12 + len).ok_or_else(|| bad("truncated CRC"))?;
        let mut crc = crc32fast::Hasher::new();
        crc.update(&ty);
        crc.update(data);
        if crc.finalize() != u32::from_be_bytes([crc_b[0], crc_b[1], crc_b[2], crc_b[3]]) {
            return Err(bad("CRC mismatch"));
        }
        pos += 12 + len;
        if info.chunks.is_empty() && &ty != b"IHDR" {
            return Err(bad("IHDR must be first"));
        }
        if !idat.is_empty() && &ty != b"IDAT" {
            idat_done = true;
        }
        match &ty {
            b"IHDR" => {
                if !info.chunks.is_empty() || len != 13 {
                    return Err(bad("IHDR"));
                }
                info.width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                info.height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                if info.width == 0 || info.height == 0 || info.width > i32::MAX as u32 || info.height > i32::MAX as u32
                {
                    return Err(bad("dimensions"));
                }
                info.color_type = data[9];
                info.bit_depth = data[8];
                bpp = match (data[8], data[9]) {
                    (8, 6) => 4,
                    (8, 2) => 3,
                    (16, 6) => 8,
                    _ => return Err(bad("unsupported depth/color type (writer emits 8-bit RGB/RGBA or 16-bit RGBA)")),
                };
                if data[10] != 0 || data[11] != 0 || data[12] != 0 {
                    return Err(bad("compression/filter/interlace method"));
                }
            }
            b"IDAT" => {
                if idat_done {
                    return Err(bad("non-consecutive IDAT"));
                }
                idat.push(data);
            }
            b"IEND" => {
                if len != 0 {
                    return Err(bad("IEND length"));
                }
                seen_iend = true;
            }
            b"iCCP" | b"sRGB" | b"gAMA" | b"cHRM" | b"pHYs" if !idat.is_empty() => {
                return Err(bad("ancillary color/physical chunk after IDAT"));
            }
            b"iCCP" => {
                let nul = data.iter().position(|&b| b == 0).ok_or_else(|| bad("iCCP name"))?;
                if !(1..=79).contains(&nul) || data.get(nul + 1) != Some(&0) {
                    return Err(bad("iCCP name/method"));
                }
                let prof = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
                    &data[nul + 2..],
                    veskra_core::limits::MAX_ICC_BYTES,
                )
                .map_err(|_| bad("iCCP zlib"))?;
                info.icc = Some((data[..nul].to_vec(), prof));
            }
            b"sRGB" => {
                if len != 1 || data[0] > 3 {
                    return Err(bad("sRGB"));
                }
                info.srgb_intent = Some(data[0]);
            }
            b"gAMA" if len != 4 => return Err(bad("gAMA")),
            b"cHRM" if len != 32 => return Err(bad("cHRM")),
            b"pHYs" => {
                if len != 9 {
                    return Err(bad("pHYs"));
                }
                if data[8] == 1 {
                    info.ppm = Some((
                        u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
                        u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
                    ));
                }
            }
            _ => {
                if ty[0].is_ascii_uppercase() && !matches!(&ty, b"PLTE") {
                    return Err(bad("unknown critical chunk"));
                }
            }
        }
        if info.chunks.iter().any(|c| c == &ty) && !matches!(&ty, b"IDAT" | b"tEXt" | b"iTXt" | b"zTXt") {
            return Err(bad("duplicate chunk"));
        }
        info.chunks.push(ty);
    }
    if !seen_iend {
        return Err(bad("missing IEND"));
    }
    if info.icc.is_some() && info.srgb_intent.is_some() {
        return Err(bad("both iCCP and sRGB present"));
    }
    if idat.is_empty() {
        return Err(bad("no IDAT"));
    }
    let row = (info.width as u64) * bpp as u64 + 1;
    let expected = row.checked_mul(info.height as u64).ok_or_else(|| bad("size overflow"))?;
    // Compressed data is bounded by the file size; the inflated stream is checked in 64 KiB steps.
    let data: Vec<u8> = idat.concat();
    let mut inp: &[u8] = &data;
    let mut st = InflateState::new_boxed(DataFormat::Zlib);
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let r = miniz_oxide::inflate::stream::inflate(&mut st, inp, &mut buf, MZFlush::None);
        for (k, b) in buf[..r.bytes_written].iter().enumerate() {
            if (total + k as u64).is_multiple_of(row) && *b > 4 {
                return Err(bad("invalid filter type"));
            }
        }
        total += r.bytes_written as u64;
        if total > expected {
            return Err(bad("image data longer than IHDR implies"));
        }
        inp = inp.get(r.bytes_consumed..).unwrap_or_default();
        match r.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) if r.bytes_consumed == 0 && r.bytes_written == 0 => return Err(bad("zlib stream truncated")),
            Ok(_) => {}
            Err(_) => return Err(bad("zlib stream corrupt or truncated")),
        }
    }
    if !inp.is_empty() {
        return Err(bad("trailing data after zlib stream"));
    }
    if total != expected {
        return Err(bad("image data size mismatch"));
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_are_latin1_and_bounded() {
        assert_eq!(icc_profile_name("sRGB IEC61966-2.1"), b"sRGB IEC61966-2.1".to_vec());
        assert_eq!(icc_profile_name("  a  b  "), b"a b".to_vec());
        assert_eq!(icc_profile_name(""), b"ICC profile".to_vec());
        assert_eq!(icc_profile_name("Ünï\u{4e00}x"), vec![0xdc, b'n', 0xef, b' ', b'x']);
        assert_eq!(icc_profile_name(&"x".repeat(200)).len(), 79);
    }

    #[test]
    fn filter_taps_sum_to_one() {
        for (s, d) in [(10u32, 3u32), (3, 10), (7, 7), (1, 4), (4, 1), (1000, 333)] {
            for t in filter_taps(s, d) {
                let sum: f32 = t.weights.iter().sum();
                assert!((sum - 1.0).abs() < 1e-4, "{s}->{d}: {sum}");
                assert!(t.start as usize + t.weights.len() <= s as usize);
            }
        }
    }

    #[test]
    fn paeth_matches_spec() {
        assert_eq!(paeth(10, 20, 10), 20);
        assert_eq!(paeth(20, 10, 10), 20);
        assert_eq!(paeth(0, 0, 0), 0);
    }

    #[test]
    fn validator_rejects_garbage() {
        assert!(inspect_png(b"").is_err());
        assert!(inspect_png(&PNG_SIGNATURE).is_err());
    }
}
