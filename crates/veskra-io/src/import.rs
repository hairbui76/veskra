//! Color-managed raster image import (MASTER_PLAN §1.1 "managed path for imported artwork",
//! §6.2 step 1, §8.4; VERIFICATION_MATRIX COL-004, COL-005, SEC-001).
//!
//! Owned by FMT+CLR. v1 supports PNG via [`import_png`]; the result is a [`TileGrid`] of
//! premultiplied RGBA16 pixels **in the document working space**, placed at the origin (the caller
//! positions it and creates the layer/undo step).
//!
//! # Input handling (external files are hostile)
//! * A bounded pre-scan walks the chunk structure **before** any decoder allocation: signature,
//!   chunk bounds, `IHDR` dimensions checked against [`MAX_DOCUMENT_DIM`] /
//!   [`MAX_DOCUMENT_PIXELS`] (a tiny file declaring 60000×60000 is rejected without allocating).
//! * Decoding uses the `png` crate with explicit [`::png::Limits`] ([`DECODER_LIMIT_BYTES`]), text
//!   chunks ignored, rows streamed into tiles one tile-row band at a time (memory ≈ the result
//!   tiles plus one band; transparent areas allocate nothing).
//! * Supported: bit depth 1/2/4/8/16; gray, gray+alpha, RGB, RGBA, indexed (palette and `tRNS`
//!   expanded to alpha); Adam7 interlacing. APNG files import their default image (warning).
//!
//! # Color metadata resolution (first match wins)
//! 1. `cICP` (PNG 3rd edition; takes precedence over all others): BT.709 primaries + sRGB transfer
//!    → sRGB; P3-D65 primaries + sRGB transfer → Display P3; anything else (PQ/HLG/other
//!    primaries) is unsupported → falls back to `iCCP` if present, otherwise handled by
//!    [`InvalidProfilePolicy`].
//! 2. `iCCP`: decompressed here (bounded by [`MAX_ICC_BYTES`]) and parsed with
//!    [`IccProfile::parse`]. The `png` crate silently drops broken `iCCP` chunks, so Veskra extracts
//!    the chunk itself: a corrupt stream, a corrupt/unsupported profile (CMYK, Lab, LUT-only, …) or a
//!    profile whose color space does not match the image (RGB profile on a gray image, …) is
//!    **rejected** by default ([`InvalidProfilePolicy::Reject`], COL-004). Only an explicit
//!    [`InvalidProfilePolicy::AssumeSrgb`] imports such a file, with `profile_assumed = true`
//!    and a warning naming the error.
//! 3. `sRGB` chunk → sRGB IEC 61966-2.1.
//! 4. `gAMA` ≈ 1/2.2 (±[`GAMA_SRGB_TOL`]) **and** `cHRM` ≈ sRGB/D65 (±[`CHRM_SRGB_TOL`]) →
//!    interpreted as sRGB, with a warning (the PNG-spec fallback values for sRGB). For grayscale
//!    images, where chromaticities are meaningless, `gAMA` ≈ 1/2.2 without `cHRM` suffices.
//! 5. Otherwise (no color metadata, `gAMA` alone, or non-sRGB `gAMA`/`cHRM`, which Veskra does not
//!    model) → [`MissingProfilePolicy`]: by default sRGB is assumed with `profile_assumed = true`
//!    and a warning (MASTER_PLAN §6.2); [`MissingProfilePolicy::Reject`] refuses the file.
//!
//! # Profile handling (COL-005: explicit and visibly different)
//! * [`ProfileHandling::ConvertToDocument`] — source profile → document profile with
//!   [`RgbTransform`] / [`GrayToRgbTransform`], relative colorimetric ([`IMPORT_INTENT`]),
//!   per-channel clipping of out-of-gamut colors (veskra-color semantics). Equivalent profiles use
//!   the exact identity path (`converted = false`).
//! * [`ProfileHandling::AssignDocumentProfile`] — the encoded numbers are reinterpreted in the
//!   document profile with **no** transform; a warning says the appearance changes whenever the
//!   source profile differs from the document's.
//!
//! # Pixel math
//! Samples → straight RGBA16 (`u8 · 257` exact, 16-bit kept); conversion evaluates the
//! [`RgbTransform`] model in `f64` with lookup tables and 16-bit output rounding (see `FastRgb`;
//! gray sources use [`GrayToRgbTransform`] tabulated per code); then
//! [`convert::straight16_to_premul16`]. Alpha 0 → `(0,0,0,0)`. For 8-bit sRGB sources into an
//! sRGB document the stored values equal [`convert::straight8_to_premul16`], so a re-export
//! through `PngExporter` reproduces the source pixels exactly.

use crate::CancelToken;
use std::io::Cursor;
use std::sync::Arc;
use veskra_color::icc::{IccProfile, ProfileColorSpace};
use veskra_color::transform::{GrayToRgbTransform, IDENTITY_CURVE_TOL, IDENTITY_MATRIX_TOL};
use veskra_color::{ColorError, RenderingIntent, RgbTransform, convert};
use veskra_core::limits::{MAX_DOCUMENT_DIM, MAX_DOCUMENT_PIXELS, MAX_ICC_BYTES};
use veskra_core::{ErrorCode, ProfileId, RectI, VeskraError};
use veskra_raster::{TILE, TileGrid};

/// Rendering intent of every import conversion (Veskra's default transform intent).
pub const IMPORT_INTENT: RenderingIntent = RenderingIntent::RelativeColorimetric;

/// Allocation budget handed to the `png` decoder ([`::png::Limits::bytes`]): an ICC-sized allowance
/// for ancillary chunks plus four row buffers at the widest legal 16-bit RGBA row. Pixel storage
/// is allocated by Veskra (bounded by [`MAX_DOCUMENT_PIXELS`]), not by the decoder.
pub const DECODER_LIMIT_BYTES: usize = MAX_ICC_BYTES + 4 * (MAX_DOCUMENT_DIM as usize * 8 + 1);

/// `gAMA` tolerance (units of 1/100000) around 45455 for the sRGB interpretation.
pub const GAMA_SRGB_TOL: u32 = 500;
/// `cHRM` tolerance (units of 1/100000, per coordinate) around the sRGB/D65 values.
pub const CHRM_SRGB_TOL: u32 = 500;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
const SRGB_GAMA: u32 = 45455;
/// White x,y, red x,y, green x,y, blue x,y (×100000), PNG specification sRGB values.
const SRGB_CHRM: [u32; 8] = [31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000];
/// Adam7 passes: (x0, y0, dx, dy).
const ADAM7: [(u32, u32, u32, u32); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// How the source's color profile relates to the document (COL-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProfileHandling {
    /// Transform colors from the source profile into the document profile (appearance kept).
    #[default]
    ConvertToDocument,
    /// Keep the numbers, reinterpret them in the document profile (appearance may change).
    AssignDocumentProfile,
}

/// What to do when the file declares no usable color space at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MissingProfilePolicy {
    /// Assume sRGB IEC 61966-2.1, set `profile_assumed = true` and warn (MASTER_PLAN §6.2).
    #[default]
    AssumeSrgb,
    /// Refuse the import (`ErrorCode::InvalidProfile`).
    Reject,
}

/// What to do when the file embeds a profile that is corrupt, unsupported or mismatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InvalidProfilePolicy {
    /// Refuse the import with the typed profile error (COL-004 default).
    #[default]
    Reject,
    /// Explicit opt-in: ignore the broken profile, assume sRGB, `profile_assumed = true`, warn.
    AssumeSrgb,
}

/// Import options. `Default` = convert, assume sRGB when untagged, reject broken profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ImportOptions {
    pub handling: ProfileHandling,
    pub missing_profile: MissingProfilePolicy,
    pub invalid_profile: InvalidProfilePolicy,
}

/// A decoded, color-managed image ready to become a raster layer.
#[derive(Debug, Clone)]
pub struct ImportedImage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA16 in the **document** working space, covering `(0,0)..(width,height)`.
    pub tiles: TileGrid,
    /// Profile declared by the file (`iCCP`, `sRGB`, `cICP`, or sRGB-equivalent `gAMA`/`cHRM`);
    /// `None` when nothing usable was declared and sRGB was assumed.
    pub source_profile: Option<ProfileId>,
    /// Semantic name of the profile used to interpret the source values.
    pub source_profile_name: String,
    /// `true` when the source profile was assumed rather than read from the file.
    pub profile_assumed: bool,
    /// `true` when a non-identity color transform was applied to the pixels.
    pub converted: bool,
    /// Sample bit depth of the file (1, 2, 4, 8 or 16).
    pub source_bit_depth: u8,
    /// Human-readable notes for the import summary (assumptions, conversions, ignored data).
    pub warnings: Vec<String>,
}

/// Decode `bytes` as PNG and convert it into `doc_profile` according to `opts`.
pub fn import_png(bytes: &[u8], doc_profile: &IccProfile, opts: &ImportOptions) -> Result<ImportedImage, VeskraError> {
    import_png_cancellable(bytes, doc_profile, opts, &CancelToken::new())
}

/// [`import_png`] with cooperative cancellation (checked once per decoded row band).
pub fn import_png_cancellable(
    bytes: &[u8],
    doc_profile: &IccProfile,
    opts: &ImportOptions,
    cancel: &CancelToken,
) -> Result<ImportedImage, VeskraError> {
    if doc_profile.color_space() != ProfileColorSpace::Rgb {
        return Err(VeskraError::new(ErrorCode::UnsupportedColorMode, "document profile must be an RGB profile"));
    }
    let scan = prescan(bytes)?;
    let mut warnings = scan.warnings.clone();

    let limits = ::png::Limits { bytes: DECODER_LIMIT_BYTES };
    let mut dec = ::png::Decoder::new_with_limits(Cursor::new(bytes), limits);
    dec.set_transformations(::png::Transformations::EXPAND);
    dec.set_ignore_text_chunk(true);
    // iCCP is extracted and validated by `prescan` (the png crate drops corrupt ones silently).
    dec.set_ignore_iccp_chunk(true);
    let mut reader = dec.read_info().map_err(png_error)?;

    let info = reader.info();
    let (width, height) = (info.width, info.height);
    if (width, height) != (scan.width, scan.height) {
        return Err(corrupt("IHDR differs between structure scan and decoder"));
    }
    let interlaced = info.interlaced;
    let source_bit_depth = info.bit_depth as u8;
    let is_gray = matches!(info.color_type, ::png::ColorType::Grayscale | ::png::ColorType::GrayscaleAlpha);
    if info.animation_control.is_some() {
        warnings.push("Animated PNG: only the default image was imported.".to_string());
    }
    let meta = ColorMeta {
        srgb: info.srgb.is_some(),
        gama: info.gama_chunk.map(|g| g.into_scaled()),
        chrm: info.chrm_chunk.map(|c| {
            [c.white.0, c.white.1, c.red.0, c.red.1, c.green.0, c.green.1, c.blue.0, c.blue.1].map(|v| v.into_scaled())
        }),
        cicp: info
            .coding_independent_code_points
            .map(|c| (c.color_primaries, c.transfer_function, c.matrix_coefficients, c.is_video_full_range_image)),
    };

    let source = resolve_source(&scan, &meta, is_gray, opts, &mut warnings)?;

    let (color, depth) = reader.output_color_type();
    let channels = match color {
        ::png::ColorType::Grayscale => 1,
        ::png::ColorType::GrayscaleAlpha => 2,
        ::png::ColorType::Rgb => 3,
        ::png::ColorType::Rgba => 4,
        ::png::ColorType::Indexed => return Err(internal("palette not expanded")),
    };
    let depth16 = match depth {
        ::png::BitDepth::Eight => false,
        ::png::BitDepth::Sixteen => true,
        _ => return Err(internal("sub-byte samples not expanded")),
    };
    let layout = RowLayout { channels, depth16 };
    let mapper = build_mapper(&source, doc_profile, depth16, opts, &mut warnings)?;

    let mut tiles = TileGrid::new();
    if interlaced {
        decode_interlaced(&mut reader, width, height, layout, &mapper, &mut tiles, cancel)?;
    } else {
        decode_progressive(&mut reader, width, height, layout, &mapper, &mut tiles, cancel)?;
    }
    if reader.next_row().map_err(png_error)?.is_some() {
        return Err(corrupt("more image rows than IHDR declares"));
    }
    reader.finish().map_err(png_error)?;

    Ok(ImportedImage {
        width,
        height,
        tiles,
        source_profile: source.declared.then(|| source.profile.id()),
        source_profile_name: source.profile.id().semantic_name,
        profile_assumed: !source.declared,
        converted: mapper.converted(),
        source_bit_depth,
        warnings,
    })
}

// ---------------------------------------------------------------------------------------------
// Pre-scan (bounded, no pixel allocation)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Scan<'a> {
    width: u32,
    height: u32,
    /// Raw `iCCP` chunk payload (CRC already verified).
    iccp: Option<&'a [u8]>,
    warnings: Vec<String>,
}

fn corrupt(msg: impl std::fmt::Display) -> VeskraError {
    VeskraError::new(ErrorCode::CorruptData, format!("invalid PNG: {msg}"))
}

fn internal(msg: &str) -> VeskraError {
    VeskraError::new(ErrorCode::Internal, format!("PNG import: {msg}"))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn chunk_name(ty: &[u8]) -> String {
    ty.iter().map(|&c| if c.is_ascii_alphabetic() { c as char } else { '?' }).collect()
}

/// Check declared dimensions against the import limits (before anything is allocated).
pub fn check_import_dimensions(width: u32, height: u32) -> Result<(), VeskraError> {
    if width == 0 || height == 0 {
        return Err(corrupt("zero image dimension"));
    }
    if width > MAX_DOCUMENT_DIM || height > MAX_DOCUMENT_DIM || width as u64 * height as u64 > MAX_DOCUMENT_PIXELS {
        return Err(VeskraError::new(
            ErrorCode::AllocationLimit,
            format!(
                "image is {width}x{height} px; import limit is {MAX_DOCUMENT_DIM} px per edge and {MAX_DOCUMENT_PIXELS} px total"
            ),
        ));
    }
    Ok(())
}

fn prescan(bytes: &[u8]) -> Result<Scan<'_>, VeskraError> {
    if bytes.get(..8) != Some(&PNG_SIGNATURE[..]) {
        return Err(corrupt("not a PNG file (bad signature)"));
    }
    let mut scan = Scan { width: 0, height: 0, iccp: None, warnings: Vec::new() };
    let mut pos = 8usize;
    let mut first = true;
    let mut seen_idat = false;
    loop {
        let len = be32(bytes, pos).ok_or_else(|| corrupt("file truncated (missing IEND)"))? as usize;
        if len > i32::MAX as usize {
            return Err(corrupt("chunk length out of range"));
        }
        let ty = bytes.get(pos + 4..pos + 8).ok_or_else(|| corrupt("file truncated in chunk header"))?;
        let data_end = (pos + 8).checked_add(len).ok_or_else(|| corrupt("chunk length overflow"))?;
        let data = bytes
            .get(pos + 8..data_end)
            .ok_or_else(|| corrupt(format!("file truncated inside chunk '{}'", chunk_name(ty))))?;
        let crc = be32(bytes, data_end).ok_or_else(|| corrupt("file truncated before chunk CRC"))?;
        let next = data_end + 4;
        if first {
            if ty != b"IHDR" || len != 13 {
                return Err(corrupt("IHDR must be the first chunk"));
            }
            scan.width = be32(data, 0).unwrap_or(0);
            scan.height = be32(data, 4).unwrap_or(0);
            if scan.width > i32::MAX as u32 || scan.height > i32::MAX as u32 {
                return Err(corrupt("dimensions out of range"));
            }
            check_import_dimensions(scan.width, scan.height)?;
            first = false;
        }
        match ty {
            b"IDAT" => seen_idat = true,
            b"iCCP" => {
                if seen_idat {
                    return Err(corrupt("iCCP chunk after image data"));
                }
                if scan.iccp.is_some() {
                    return Err(corrupt("duplicate iCCP chunk"));
                }
                let mut h = crc32fast::Hasher::new();
                h.update(ty);
                h.update(data);
                if h.finalize() != crc {
                    return Err(corrupt("iCCP chunk CRC mismatch"));
                }
                scan.iccp = Some(data);
            }
            b"IEND" => {
                if next < bytes.len() {
                    scan.warnings
                        .push(format!("{} byte(s) after the PNG end marker were ignored.", bytes.len() - next));
                }
                return Ok(scan);
            }
            _ => {}
        }
        pos = next;
    }
}

/// Decompress an `iCCP` payload (keyword, method 0, zlib) to at most [`MAX_ICC_BYTES`].
fn inflate_iccp(data: &[u8]) -> Result<Vec<u8>, ColorError> {
    let nul = data.iter().position(|&b| b == 0).ok_or(ColorError::InvalidProfile("iCCP chunk has no profile name"))?;
    if !(1..=79).contains(&nul) {
        return Err(ColorError::InvalidProfile("iCCP profile name must be 1-79 bytes"));
    }
    if data.get(nul + 1) != Some(&0) {
        return Err(ColorError::InvalidProfile("iCCP compression method is not zlib"));
    }
    let stream = data.get(nul + 2..).unwrap_or_default();
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(stream, MAX_ICC_BYTES).map_err(|e| match e.status {
        miniz_oxide::inflate::TINFLStatus::HasMoreOutput => {
            ColorError::TooLarge { size: MAX_ICC_BYTES + 1, limit: MAX_ICC_BYTES }
        }
        _ => ColorError::InvalidProfile("iCCP zlib stream is corrupt"),
    })
}

fn png_error(e: ::png::DecodingError) -> VeskraError {
    match e {
        ::png::DecodingError::IoError(io) => corrupt(format!("data truncated or unreadable ({})", io.kind())),
        ::png::DecodingError::Format(f) => corrupt(f),
        ::png::DecodingError::LimitsExceeded => {
            VeskraError::new(ErrorCode::AllocationLimit, "PNG decoder memory limit exceeded")
        }
        ::png::DecodingError::Parameter(p) => internal(&p.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// Source color resolution
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct ColorMeta {
    srgb: bool,
    gama: Option<u32>,
    chrm: Option<[u32; 8]>,
    /// (primaries, transfer, matrix, full range)
    cicp: Option<(u8, u8, u8, bool)>,
}

#[derive(Debug, Clone)]
struct Source {
    profile: Arc<IccProfile>,
    /// Declared by the file (`false` = assumed).
    declared: bool,
}

fn profile_error(e: ColorError, what: &str) -> VeskraError {
    VeskraError::new(e.code(), format!("{what}: {e}"))
}

/// Parse the embedded profile and check it fits the image's color type.
fn embedded_profile(iccp: &[u8], is_gray: bool) -> Result<Arc<IccProfile>, ColorError> {
    let bytes = inflate_iccp(iccp)?;
    let srgb = IccProfile::srgb();
    let p = if bytes.as_slice() == srgb.bytes() { srgb } else { Arc::new(IccProfile::parse(&bytes)?) };
    match (p.color_space(), is_gray) {
        (ProfileColorSpace::Gray, false) => Err(ColorError::InvalidProfile("Gray ICC profile on an RGB PNG")),
        (ProfileColorSpace::Rgb, true) => Err(ColorError::InvalidProfile("RGB ICC profile on a grayscale PNG")),
        _ => Ok(p),
    }
}

fn cicp_profile(c: (u8, u8, u8, bool)) -> Result<Arc<IccProfile>, String> {
    match c {
        (1, 13, 0, true) => Ok(IccProfile::srgb()),
        (12, 13, 0, true) => Ok(IccProfile::display_p3()),
        (p, t, m, r) => Err(format!(
            "cICP primaries {p}, transfer {t}, matrix {m}, full-range {r} is not supported (only sRGB and Display P3)"
        )),
    }
}

fn near(a: u32, b: u32, tol: u32) -> bool {
    a.abs_diff(b) <= tol
}

fn resolve_source(
    scan: &Scan<'_>,
    meta: &ColorMeta,
    is_gray: bool,
    opts: &ImportOptions,
    warnings: &mut Vec<String>,
) -> Result<Source, VeskraError> {
    let declared = |profile| Ok(Source { profile, declared: true });
    let unusable = |err: VeskraError, warnings: &mut Vec<String>| match opts.invalid_profile {
        InvalidProfilePolicy::Reject => Err(err),
        InvalidProfilePolicy::AssumeSrgb => {
            warnings.push(format!(
                "{} — ignored by explicit policy; sRGB IEC61966-2.1 assumed, colors may be wrong.",
                err.message
            ));
            Ok(Source { profile: IccProfile::srgb(), declared: false })
        }
    };

    if let Some(c) = meta.cicp {
        match cicp_profile(c) {
            Ok(p) => {
                if scan.iccp.is_some() {
                    warnings.push("iCCP profile ignored: the cICP chunk takes precedence (PNG 3rd edition).".into());
                }
                return declared(p);
            }
            Err(msg) if scan.iccp.is_some() => {
                warnings.push(format!("{msg}; using the embedded ICC profile instead."));
            }
            Err(msg) => return unusable(VeskraError::new(ErrorCode::UnsupportedColorMode, msg), warnings),
        }
    }
    if let Some(iccp) = scan.iccp {
        return match embedded_profile(iccp, is_gray) {
            Ok(p) => {
                if meta.srgb {
                    warnings.push("File has both iCCP and sRGB chunks; the embedded ICC profile was used.".into());
                }
                if p.has_lut_tags() {
                    warnings.push(format!(
                        "Profile '{}' also contains LUT tags; Veskra uses its matrix/TRC model.",
                        p.id().semantic_name
                    ));
                }
                declared(p)
            }
            Err(e) => unusable(profile_error(e, "embedded ICC profile is unusable"), warnings),
        };
    }
    if meta.srgb {
        return declared(IccProfile::srgb());
    }
    let gama_srgb = meta.gama.is_some_and(|g| near(g, SRGB_GAMA, GAMA_SRGB_TOL));
    let chrm_srgb = meta.chrm.is_some_and(|c| c.iter().zip(SRGB_CHRM).all(|(&v, s)| near(v, s, CHRM_SRGB_TOL)));
    if gama_srgb && (chrm_srgb || (is_gray && meta.chrm.is_none())) {
        warnings.push("No ICC profile: gAMA 1/2.2 with sRGB chromaticities interpreted as sRGB IEC61966-2.1.".into());
        return declared(IccProfile::srgb());
    }
    let mut detail = String::new();
    if let Some(g) = meta.gama {
        detail.push_str(&format!(" gAMA {:.5} was not modelled;", g as f64 / 100000.0));
    }
    if meta.chrm.is_some() && !chrm_srgb {
        detail.push_str(" non-sRGB cHRM chromaticities were not modelled;");
    }
    match opts.missing_profile {
        MissingProfilePolicy::Reject => Err(VeskraError::new(
            ErrorCode::InvalidProfile,
            format!("PNG has no usable color profile and the import policy rejects untagged images.{detail}"),
        )),
        MissingProfilePolicy::AssumeSrgb => {
            warnings.push(format!("No color profile in file;{detail} sRGB IEC61966-2.1 assumed."));
            Ok(Source { profile: IccProfile::srgb(), declared: false })
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pixel mapping
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
enum Mapper {
    /// Numbers kept (identity conversion or assign).
    Keep,
    /// RGB conversion (matrix/TRC, see [`FastRgb`]).
    Rgb(Box<FastRgb>),
    /// Gray conversion through a per-sample lookup table (256 or 65536 entries).
    Gray(Vec<[u16; 3]>),
}

impl Mapper {
    fn converted(&self) -> bool {
        !matches!(self, Mapper::Keep)
    }
}

fn build_mapper(
    source: &Source,
    doc: &IccProfile,
    depth16: bool,
    opts: &ImportOptions,
    warnings: &mut Vec<String>,
) -> Result<Mapper, VeskraError> {
    let src = &source.profile;
    let (src_name, doc_name) = (src.id().semantic_name, doc.id().semantic_name);
    let same = src.is_colorimetrically_equivalent(doc, IDENTITY_MATRIX_TOL, IDENTITY_CURVE_TOL);
    match opts.handling {
        ProfileHandling::AssignDocumentProfile => {
            if !same {
                warnings.push(format!(
                    "Assigned document profile '{doc_name}' to values encoded for '{src_name}' without conversion: \
                     colors will look different from the source."
                ));
            }
            Ok(Mapper::Keep)
        }
        ProfileHandling::ConvertToDocument if src.color_space() == ProfileColorSpace::Gray => {
            let t = GrayToRgbTransform::new(src, doc, IMPORT_INTENT)
                .map_err(|e| profile_error(e, "cannot convert gray image"))?;
            let n = if depth16 { 65536 } else { 256 };
            let max = (n - 1) as f32;
            let lut = (0..n).map(|v| t.apply(v as f32 / max).map(|c| convert::f64_to_u16(c as f64))).collect();
            warnings.push(format!("Converted from '{src_name}' to '{doc_name}' (relative colorimetric)."));
            Ok(Mapper::Gray(lut))
        }
        ProfileHandling::ConvertToDocument => {
            // RGB source profile; a gray image under an RGB profile (sRGB chunk, cICP, assumed)
            // is converted as R = G = B.
            let t = RgbTransform::new(src, doc, IMPORT_INTENT)
                .map_err(|e| profile_error(e, "cannot convert to the document profile"))?;
            if t.is_identity() {
                return Ok(Mapper::Keep);
            }
            warnings.push(format!(
                "Converted from '{src_name}' to '{doc_name}' (relative colorimetric); colors outside the document \
                 gamut are clipped per channel."
            ));
            Ok(Mapper::Rgb(Box::new(FastRgb::new(&t, src, doc, depth16)?)))
        }
    }
}

/// Table-driven evaluation of an [`RgbTransform`]'s relative-colorimetric model with 16-bit output:
/// source curve per input code (exact `ToneCurve::eval` values), the transform's own combined
/// matrix ([`RgbTransform::linear_matrix`]) in `f64`, per-channel clipping to `[0, 1]`, and the
/// destination encoding by binary search over the 65535 linear decision thresholds — i.e.
/// `round_half_up(encoded · 65535)` without a per-pixel `pow`. This is the same technique as
/// `RgbTransform::apply_rgba8_straight` at 8 bits; results equal `apply_f64` + `f64_to_u16`
/// except for floating-point ties (≤ 1 code at 16 bits, measured in tests).
#[derive(Debug)]
struct FastRgb {
    matrix: [[f64; 3]; 3],
    /// Linear value per source code (256 or 65536 entries) and channel.
    src_lin: [Vec<f64>; 3],
    /// `thresholds[c][k]` = linear value whose destination encoding is `(k + 0.5) / 65535`.
    thresholds: [Vec<f64>; 3],
    /// `bins[c][b]` = number of thresholds `<= b / ENCODE_BINS` (narrows each search to one bin).
    bins: [Vec<u16>; 3],
}

/// Uniform linear-domain bins indexing [`FastRgb::thresholds`].
const ENCODE_BINS: usize = 1 << 16;

impl FastRgb {
    fn new(t: &RgbTransform, src: &IccProfile, dst: &IccProfile, depth16: bool) -> Result<Self, VeskraError> {
        let n: usize = if depth16 { 65536 } else { 256 };
        let max = (n - 1) as f64;
        let curve = |p: &IccProfile, c: usize| {
            p.trc(c).cloned().ok_or_else(|| internal("RGB profile without three tone curves"))
        };
        let (s, d) =
            ([curve(src, 0)?, curve(src, 1)?, curve(src, 2)?], [curve(dst, 0)?, curve(dst, 1)?, curve(dst, 2)?]);
        let thresholds: [Vec<f64>; 3] =
            std::array::from_fn(|c| (0..65535).map(|k| d[c].eval((k as f64 + 0.5) / 65535.0)).collect());
        let bins = std::array::from_fn(|c| {
            let th = &thresholds[c];
            let mut k = 0usize;
            (0..=ENCODE_BINS)
                .map(|b| {
                    let edge = b as f64 / ENCODE_BINS as f64;
                    while k < th.len() && th[k] <= edge {
                        k += 1;
                    }
                    k as u16
                })
                .collect()
        });
        Ok(Self {
            matrix: t.linear_matrix(),
            src_lin: std::array::from_fn(|c| (0..n).map(|v| s[c].eval(v as f64 / max)).collect()),
            thresholds,
            bins,
        })
    }

    /// Source codes (8- or 16-bit, matching construction) → destination encoded u16.
    fn apply(&self, idx: [usize; 3]) -> [u16; 3] {
        let lin: [f64; 3] = std::array::from_fn(|c| self.src_lin[c].get(idx[c]).copied().unwrap_or(0.0));
        let d = veskra_color::math::mul_vec(&self.matrix, lin);
        std::array::from_fn(|c| {
            let v = d[c];
            if v.is_nan() || v <= 0.0 {
                0
            } else if v >= 1.0 {
                65535
            } else {
                // Thresholds <= b/BINS are all <= v; those > (b+1)/BINS are all > v.
                let b = ((v * ENCODE_BINS as f64) as usize).min(ENCODE_BINS - 1);
                let (bins, th) = (&self.bins[c], &self.thresholds[c]);
                let lo = bins.get(b).copied().unwrap_or(0) as usize;
                let hi = (bins.get(b + 1).copied().unwrap_or(65535) as usize).max(lo);
                (lo + th.get(lo..hi).unwrap_or_default().partition_point(|&t| t <= v)) as u16
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct RowLayout {
    channels: usize,
    depth16: bool,
}

impl RowLayout {
    fn bytes_per_pixel(self) -> usize {
        self.channels * if self.depth16 { 2 } else { 1 }
    }
}

/// One decoded row (`pixels` pixels) → premultiplied RGBA16 in document space.
fn convert_row(
    data: &[u8],
    pixels: usize,
    layout: RowLayout,
    mapper: &Mapper,
    out: &mut [u16],
) -> Result<(), VeskraError> {
    let bpp = layout.bytes_per_pixel();
    if data.len() != pixels * bpp || out.len() != pixels * 4 {
        return Err(corrupt("decoded row length does not match IHDR"));
    }
    let sample = |px: &[u8], i: usize| -> (u16, usize) {
        if layout.depth16 {
            let v = u16::from_be_bytes([px[2 * i], px[2 * i + 1]]);
            (v, v as usize)
        } else {
            (px[i] as u16 * 257, px[i] as usize)
        }
    };
    for (px, o) in data.chunks_exact(bpp).zip(out.chunks_exact_mut(4)) {
        let (rgb, idx, alpha) = match layout.channels {
            1 | 2 => {
                let (g, gi) = sample(px, 0);
                let a = if layout.channels == 2 { sample(px, 1).0 } else { 65535 };
                ([g; 3], [gi; 3], a)
            }
            _ => {
                let a = if layout.channels == 4 { sample(px, 3).0 } else { 65535 };
                let (r, g, b) = (sample(px, 0), sample(px, 1), sample(px, 2));
                ([r.0, g.0, b.0], [r.1, g.1, b.1], a)
            }
        };
        let rgb = if alpha == 0 {
            [0; 3]
        } else {
            match mapper {
                // Gray LUT only exists for gray images (one sample per pixel).
                Mapper::Gray(lut) => lut.get(idx[0]).copied().unwrap_or(rgb),
                Mapper::Rgb(t) => t.apply(idx),
                Mapper::Keep => rgb,
            }
        };
        convert::straight16_to_premul16(&[rgb[0], rgb[1], rgb[2], alpha], o);
    }
    Ok(())
}

fn cancelled(cancel: &CancelToken) -> Result<(), VeskraError> {
    if cancel.is_cancelled() { Err(VeskraError::new(ErrorCode::Cancelled, "import cancelled")) } else { Ok(()) }
}

fn next_row_data<R: std::io::BufRead + std::io::Seek>(reader: &mut ::png::Reader<R>) -> Result<&[u8], VeskraError> {
    match reader.next_row().map_err(png_error)? {
        Some(row) => Ok(row.data()),
        None => Err(corrupt("image data ended early")),
    }
}

/// Non-interlaced: rows streamed into one tile-row band at a time.
fn decode_progressive<R: std::io::BufRead + std::io::Seek>(
    reader: &mut ::png::Reader<R>,
    width: u32,
    height: u32,
    layout: RowLayout,
    mapper: &Mapper,
    tiles: &mut TileGrid,
    cancel: &CancelToken,
) -> Result<(), VeskraError> {
    let w = width as usize;
    let mut band = vec![0u16; w * 4 * TILE as usize];
    let mut y0 = 0u32;
    while y0 < height {
        cancelled(cancel)?;
        let rows = TILE.min(height - y0);
        for r in 0..rows as usize {
            let data = next_row_data(reader)?;
            convert_row(data, w, layout, mapper, &mut band[r * w * 4..(r + 1) * w * 4])?;
        }
        let rect = RectI::new(0, y0 as i32, width as i32, (y0 + rows) as i32);
        tiles.write_rect(rect, &band[..rows as usize * w * 4])?;
        y0 += rows;
    }
    Ok(())
}

/// Adam7: each reduced row is converted and scattered into the tiles (no full-frame buffer).
fn decode_interlaced<R: std::io::BufRead + std::io::Seek>(
    reader: &mut ::png::Reader<R>,
    width: u32,
    height: u32,
    layout: RowLayout,
    mapper: &Mapper,
    tiles: &mut TileGrid,
    cancel: &CancelToken,
) -> Result<(), VeskraError> {
    let mut row = vec![0u16; width as usize * 4];
    for (x0, y0, dx, dy) in ADAM7 {
        let pw = width.saturating_sub(x0).div_ceil(dx);
        let ph = height.saturating_sub(y0).div_ceil(dy);
        if pw == 0 || ph == 0 {
            continue;
        }
        for line in 0..ph {
            if line % TILE == 0 {
                cancelled(cancel)?;
            }
            let data = next_row_data(reader)?;
            let out = &mut row[..pw as usize * 4];
            convert_row(data, pw as usize, layout, mapper, out)?;
            let y = (y0 + line * dy) as i32;
            for (i, p) in out.chunks_exact(4).enumerate() {
                let x = (x0 + i as u32 * dx) as i32;
                tiles.put_pixel(x, y, [p[0], p[1], p[2], p[3]]);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_limit_covers_widest_row() {
        const { assert!(DECODER_LIMIT_BYTES > MAX_DOCUMENT_DIM as usize * 8) };
        assert!(check_import_dimensions(MAX_DOCUMENT_DIM, 1).is_ok());
        assert_eq!(check_import_dimensions(MAX_DOCUMENT_DIM + 1, 1).unwrap_err().code, ErrorCode::AllocationLimit);
        assert_eq!(check_import_dimensions(8193, 8193).unwrap_err().code, ErrorCode::AllocationLimit);
        assert_eq!(check_import_dimensions(0, 5).unwrap_err().code, ErrorCode::CorruptData);
    }

    #[test]
    fn cicp_mapping() {
        assert!(cicp_profile((1, 13, 0, true)).unwrap().is_srgb_equivalent());
        assert!(cicp_profile((12, 13, 0, true)).unwrap().is_display_p3_equivalent());
        assert!(cicp_profile((9, 16, 0, true)).is_err());
        assert!(cicp_profile((1, 13, 0, false)).is_err());
    }

    #[test]
    fn iccp_payload_errors_are_typed() {
        assert!(matches!(inflate_iccp(b"no-nul"), Err(ColorError::InvalidProfile(_))));
        assert!(matches!(inflate_iccp(b"name\0\x01xx"), Err(ColorError::InvalidProfile(_))));
        assert!(matches!(inflate_iccp(b"name\0\0garbage"), Err(ColorError::InvalidProfile(_))));
        assert!(matches!(inflate_iccp(b"\0\0"), Err(ColorError::InvalidProfile(_))));
        let big = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; MAX_ICC_BYTES + 10], 6);
        let mut d = b"p\0\0".to_vec();
        d.extend_from_slice(&big);
        assert!(matches!(inflate_iccp(&d), Err(ColorError::TooLarge { .. })));
    }
}
