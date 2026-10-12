//! Bounded, panic-free ICC profile parsing (RGB matrix/TRC and Gray TRC, ICC v2 and v4) and
//! Veskra's generated working profiles.
//!
//! # Supported subset
//! * Header: size, version (2.x or 4.x), class (`mntr`, `scnr`, `spac`, `prtr`), data color
//!   space `RGB ` or `GRAY`, PCS `XYZ `, `acsp` magic.
//! * Tags: `desc` (`desc` or `mluc`), `cprt` (`text`, `desc` or `mluc`), `wtpt`, `chad`,
//!   `rXYZ`/`gXYZ`/`bXYZ`, `rTRC`/`gTRC`/`bTRC`/`kTRC` (`curv` identity/gamma/table or `para`).
//!   Other tags are bounds-checked but ignored.
//! * LUT-only profiles (A2Bx/B2Ax/D2Bx without matrix/TRC) are rejected with
//!   [`ColorError::LutOnlyProfile`]. When a profile has both, the matrix/TRC model is used and
//!   [`IccProfile::has_lut_tags`] reports it (callers can warn: LittleCMS would prefer the LUT).
//!
//! # Robustness
//! Every offset/length from the file is bounds-checked with checked arithmetic, all allocations
//! are bounded by the input size (≤ [`MAX_ICC_BYTES`]), and the parser never panics (fuzzed by
//! `tests/icc_robustness.rs`). Informational text tags that are malformed are ignored rather
//! than rejected; structural problems and malformed colorimetric tags are typed errors.

pub mod lut;
mod md5;
mod writer;

pub use writer::{RgbProfileSpec, build_rgb_profile_bytes};

use crate::error::{ColorError, sig_str};
use crate::math::{self, D50_XYZ, Mat3, Vec3};
use crate::transfer::{ParametricCurve, TableCurve, ToneCurve};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::{Arc, OnceLock};
use veskra_core::ProfileId;
use veskra_core::limits::MAX_ICC_BYTES;

/// Upper bound on the tag count accepted (real profiles have < 100 tags).
pub const MAX_TAGS: usize = 1024;
/// Upper bound on `curv` table entries accepted.
pub const MAX_CURVE_ENTRIES: usize = 1 << 20;
/// Maximum characters kept from description/copyright text.
pub const MAX_TEXT_CHARS: usize = 256;

/// Colorant XYZ tolerance (absolute, PCS units) for [`IccProfile::is_srgb_equivalent`].
pub const SRGB_EQUIVALENCE_MATRIX_TOL: f64 = 2e-3;
/// Curve tolerance (encoded-domain round-trip error) for [`IccProfile::is_srgb_equivalent`].
/// 2e-3 ≈ 0.5 LSB at 8 bits; a pure 2.2 gamma differs from sRGB by > 4e-3 and is rejected.
pub const SRGB_EQUIVALENCE_CURVE_TOL: f64 = 2e-3;

/// Canonical semantic names used in [`ProfileId::semantic_name`].
pub const SRGB_SEMANTIC_NAME: &str = "sRGB IEC61966-2.1";
/// Semantic name for Display P3 equivalents.
pub const DISPLAY_P3_SEMANTIC_NAME: &str = "Display P3";

/// Data color space of a supported profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileColorSpace {
    Rgb,
    Gray,
}

/// ICC profile/device class of a supported profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileClass {
    /// `scnr`
    Input,
    /// `mntr`
    Display,
    /// `prtr`
    Output,
    /// `spac`
    ColorSpace,
}

/// ICC version from header bytes 8–9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IccVersion {
    pub major: u8,
    pub minor: u8,
    pub bugfix: u8,
}

#[derive(Clone, PartialEq)]
enum Model {
    Rgb { matrix: Mat3, trc: Box<[ToneCurve; 3]> },
    Gray { trc: ToneCurve },
}

/// A parsed RGB matrix/TRC or Gray TRC ICC profile. The original bytes are kept verbatim so the
/// profile can be embedded on save/export; [`IccProfile::id`] is the SHA-256 of those bytes.
#[derive(Clone)]
pub struct IccProfile {
    bytes: Arc<[u8]>,
    id: ProfileId,
    version: IccVersion,
    class: ProfileClass,
    rendering_intent: u32,
    description: String,
    copyright: Option<String>,
    media_white: Option<Vec3>,
    chad: Option<Mat3>,
    has_lut_tags: bool,
    model: Model,
}

impl fmt::Debug for IccProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IccProfile")
            .field("description", &self.description)
            .field("version", &self.version)
            .field("class", &self.class)
            .field("color_space", &self.color_space())
            .field("bytes", &self.bytes.len())
            .field("id", &self.id)
            .finish()
    }
}

// ---------------------------------------------------------------------------------------------
// Bounded readers. All return typed errors instead of panicking.

pub(crate) fn slice(data: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    data.get(off..off.checked_add(len)?)
}

pub(crate) fn be_u16(data: &[u8], off: usize) -> Option<u16> {
    let s = slice(data, off, 2)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

pub(crate) fn be_u32(data: &[u8], off: usize) -> Option<u32> {
    let s = slice(data, off, 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

pub(crate) fn sig(data: &[u8], off: usize) -> Option<[u8; 4]> {
    let s = slice(data, off, 4)?;
    Some([s[0], s[1], s[2], s[3]])
}

pub(crate) fn s15f16(data: &[u8], off: usize) -> Option<f64> {
    Some(be_u32(data, off)? as i32 as f64 / 65536.0)
}

fn xyz_at(data: &[u8], off: usize) -> Option<Vec3> {
    Some([s15f16(data, off)?, s15f16(data, off + 4)?, s15f16(data, off + 8)?])
}

pub(crate) fn tag_err(tag: [u8; 4], reason: &'static str) -> ColorError {
    ColorError::InvalidTag { tag: sig_str(tag), reason }
}

const LUT_TAGS: [&[u8; 4]; 9] = [b"A2B0", b"A2B1", b"A2B2", b"B2A0", b"B2A1", b"B2A2", b"D2B0", b"D2B1", b"D2B2"];

pub(crate) struct TagTable<'a> {
    data: &'a [u8],
    tags: Vec<([u8; 4], usize, usize)>,
}

impl<'a> TagTable<'a> {
    pub(crate) fn get(&self, s: &[u8; 4]) -> Option<&'a [u8]> {
        self.tags.iter().find(|(t, _, _)| t == s).and_then(|&(_, off, len)| slice(self.data, off, len))
    }
    pub(crate) fn has(&self, s: &[u8; 4]) -> bool {
        self.tags.iter().any(|(t, _, _)| t == s)
    }
    /// `(offset, size)` of a tag inside the profile bytes (validated by [`parse_tag_table`]).
    pub(crate) fn range(&self, s: &[u8; 4]) -> Option<(usize, usize)> {
        self.tags.iter().find(|(t, _, _)| t == s).map(|&(_, off, len)| (off, len))
    }
}

pub(crate) fn parse_xyz_tag(tag: [u8; 4], d: &[u8]) -> Result<Vec3, ColorError> {
    if sig(d, 0) != Some(*b"XYZ ") {
        return Err(tag_err(tag, "expected XYZType"));
    }
    xyz_at(d, 8).ok_or_else(|| tag_err(tag, "XYZType too short"))
}

pub(crate) fn parse_curve_tag(tag: [u8; 4], d: &[u8]) -> Result<ToneCurve, ColorError> {
    match sig(d, 0) {
        Some(t) if &t == b"curv" => {
            let count = be_u32(d, 8).ok_or_else(|| tag_err(tag, "curv too short"))? as usize;
            if count > MAX_CURVE_ENTRIES {
                return Err(tag_err(tag, "curv table too large"));
            }
            let needed =
                count.checked_mul(2).and_then(|n| n.checked_add(12)).ok_or_else(|| tag_err(tag, "overflow"))?;
            if needed > d.len() {
                return Err(tag_err(tag, "curv entries exceed tag size"));
            }
            match count {
                0 => Ok(ToneCurve::Identity),
                1 => {
                    let g = be_u16(d, 12).ok_or_else(|| tag_err(tag, "curv too short"))? as f64 / 256.0;
                    if g <= 0.0 {
                        return Err(tag_err(tag, "zero gamma"));
                    }
                    Ok(if g == 1.0 { ToneCurve::Identity } else { ToneCurve::Gamma(g) })
                }
                _ => {
                    let mut entries = Vec::with_capacity(count);
                    for i in 0..count {
                        entries.push(be_u16(d, 12 + 2 * i).ok_or_else(|| tag_err(tag, "curv too short"))?);
                    }
                    Ok(ToneCurve::Table(TableCurve::new(entries)?))
                }
            }
        }
        Some(t) if &t == b"para" => {
            let ft = be_u16(d, 8).ok_or_else(|| tag_err(tag, "para too short"))?;
            let n = ParametricCurve::param_count(ft).ok_or_else(|| tag_err(tag, "unknown para function type"))?;
            let mut params = [0.0; 7];
            for (i, p) in params.iter_mut().take(n).enumerate() {
                *p = s15f16(d, 12 + 4 * i).ok_or_else(|| tag_err(tag, "para too short"))?;
            }
            let pc = ParametricCurve::new(ft, &params[..n]).map_err(|e| match e {
                ColorError::InvalidTag { reason, .. } => tag_err(tag, reason),
                other => other,
            })?;
            Ok(ToneCurve::Parametric(pc))
        }
        _ => Err(tag_err(tag, "TRC must be curv or para")),
    }
}

fn sanitize_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_TEXT_CHARS).collect::<String>().trim().to_string()
}

fn parse_ascii(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    sanitize_text(&String::from_utf8_lossy(&bytes[..end]))
}

/// `mluc`: prefer en-US, then any English record, then the first record.
fn parse_mluc(d: &[u8]) -> Option<String> {
    let n = be_u32(d, 8)? as usize;
    let rec = be_u32(d, 12)? as usize;
    if rec < 12 {
        return None;
    }
    let max_records = d.len().saturating_sub(16) / rec;
    let mut best: Option<(u8, usize, usize)> = None;
    for i in 0..n.min(max_records) {
        let base = 16 + i * rec;
        let lang = slice(d, base, 2)?;
        let country = slice(d, base + 2, 2)?;
        let len = be_u32(d, base + 4)? as usize;
        let off = be_u32(d, base + 8)? as usize;
        let score = match (lang, country) {
            (b"en", b"US") => 3,
            (b"en", _) => 2,
            _ => 1,
        };
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, off, len));
        }
    }
    let (_, off, len) = best?;
    let raw = slice(d, off, len)?;
    let units = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
    let s: String = char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
    Some(sanitize_text(&s))
}

pub(crate) fn parse_text_tag(d: &[u8]) -> Option<String> {
    match &sig(d, 0)? {
        b"mluc" => parse_mluc(d),
        b"desc" => {
            let n = be_u32(d, 8)? as usize;
            let n = n.min(d.len().saturating_sub(12));
            Some(parse_ascii(slice(d, 12, n)?))
        }
        b"text" => Some(parse_ascii(d.get(8..)?)),
        _ => None,
    }
}

/// Header fields shared by every profile kind (see [`parse_header`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header {
    pub version: IccVersion,
    pub class: [u8; 4],
    pub color_space: [u8; 4],
    pub pcs: [u8; 4],
    pub rendering_intent: u32,
}

/// Bounded header checks shared by every parser: size ≤ `limit`, ≥ 132 bytes, declared size equal
/// to the data length, `acsp` magic and ICC major version 2 or 4.
pub(crate) fn parse_header(bytes: &[u8], limit: usize) -> Result<Header, ColorError> {
    let len = bytes.len();
    if len > limit {
        return Err(ColorError::TooLarge { size: len, limit });
    }
    if len < 132 {
        return Err(ColorError::Truncated { what: "header and tag count", needed: 132, available: len as u64 });
    }
    let declared = be_u32(bytes, 0).unwrap_or(0) as u64;
    if declared > len as u64 {
        return Err(ColorError::Truncated { what: "declared profile size", needed: declared, available: len as u64 });
    }
    if declared < len as u64 {
        return Err(ColorError::SizeMismatch { declared, actual: len as u64 });
    }
    if sig(bytes, 36) != Some(*b"acsp") {
        return Err(ColorError::InvalidProfile("missing 'acsp' file signature"));
    }
    let version = IccVersion { major: bytes[8], minor: bytes[9] >> 4, bugfix: bytes[9] & 0x0f };
    if version.major != 2 && version.major != 4 {
        return Err(ColorError::UnsupportedVersion { major: version.major, minor: version.minor });
    }
    Ok(Header {
        version,
        class: sig(bytes, 12).unwrap_or([0; 4]),
        color_space: sig(bytes, 16).unwrap_or([0; 4]),
        pcs: sig(bytes, 20).unwrap_or([0; 4]),
        rendering_intent: be_u32(bytes, 64).unwrap_or(0) & 0xffff,
    })
}

/// Bounded tag-table parse: count ≤ [`MAX_TAGS`], every entry inside the data and after the table,
/// ≥ 8 bytes, unique signatures, no partial overlaps (identical ranges may be shared).
pub(crate) fn parse_tag_table(bytes: &[u8]) -> Result<TagTable<'_>, ColorError> {
    let len = bytes.len();
    let count = be_u32(bytes, 128).unwrap_or(0) as usize;
    if count > MAX_TAGS {
        return Err(ColorError::InvalidProfile("absurd tag count"));
    }
    let table_end = 132 + 12 * count;
    if table_end > len {
        return Err(ColorError::Truncated { what: "tag table", needed: table_end as u64, available: len as u64 });
    }
    let mut tags = Vec::with_capacity(count);
    for i in 0..count {
        let base = 132 + 12 * i;
        let s = sig(bytes, base).unwrap_or([0; 4]);
        let off = be_u32(bytes, base + 4).unwrap_or(0) as usize;
        let size = be_u32(bytes, base + 8).unwrap_or(0) as usize;
        if off < table_end {
            return Err(tag_err(s, "tag data overlaps header or tag table"));
        }
        match off.checked_add(size) {
            Some(end) if end <= len => {}
            _ => return Err(tag_err(s, "tag data out of bounds")),
        }
        if size < 8 {
            return Err(tag_err(s, "tag smaller than its type header"));
        }
        if tags.iter().any(|(t, _, _)| *t == s) {
            return Err(tag_err(s, "duplicate tag signature"));
        }
        tags.push((s, off, size));
    }
    // Identical (offset, size) pairs may be shared (e.g. rTRC = gTRC = bTRC); partial overlaps
    // are rejected.
    let mut ranges: Vec<(usize, usize)> = tags.iter().map(|&(_, o, s)| (o, o + s)).collect();
    ranges.sort_unstable();
    ranges.dedup();
    if ranges.windows(2).any(|w| w[1].0 < w[0].1) {
        return Err(ColorError::InvalidProfile("overlapping tag data"));
    }
    Ok(TagTable { data: bytes, tags })
}

/// Header-level facts about any ICC profile (also unsupported kinds), for `profile.inspect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSummary {
    pub version: IccVersion,
    /// Device class signature, e.g. `"prtr"`, `"mntr"` (non-printable bytes → `?`).
    pub class: String,
    /// Data color space signature without padding, e.g. `"CMYK"`, `"RGB"`.
    pub color_space: String,
    /// PCS signature without padding, `"Lab"` or `"XYZ"`.
    pub pcs: String,
    pub description: String,
    pub copyright: Option<String>,
    /// Tag signatures in table order.
    pub tags: Vec<String>,
    pub bytes: usize,
}

impl ProfileSummary {
    /// Whether the tag table lists `sig`.
    pub fn has_tag(&self, sig: &str) -> bool {
        self.tags.iter().any(|t| t == sig)
    }

    /// `"2.1"`, `"4.3"`, … (major.minor).
    pub fn version_string(&self) -> String {
        format!("{}.{}", self.version.major, self.version.minor)
    }
}

/// Structural summary of any ICC profile up to `limit` bytes (header + tag table + text tags).
/// Fails only for data that is not a structurally valid ICC v2/v4 profile; unsupported classes or
/// color spaces are reported, not rejected.
pub fn summarize(bytes: &[u8], limit: usize) -> Result<ProfileSummary, ColorError> {
    let h = parse_header(bytes, limit)?;
    let table = parse_tag_table(bytes)?;
    let trim = |s: [u8; 4]| sig_str(s).trim_end().to_string();
    Ok(ProfileSummary {
        version: h.version,
        class: sig_str(h.class),
        color_space: trim(h.color_space),
        pcs: trim(h.pcs),
        description: table.get(b"desc").and_then(parse_text_tag).unwrap_or_default(),
        copyright: table.get(b"cprt").and_then(parse_text_tag),
        tags: table.tags.iter().map(|(t, _, _)| sig_str(*t)).collect(),
        bytes: bytes.len(),
    })
}

impl IccProfile {
    /// Parse and validate an ICC profile (see module docs for the supported subset).
    pub fn parse(bytes: &[u8]) -> Result<IccProfile, ColorError> {
        let h = parse_header(bytes, MAX_ICC_BYTES)?;
        let version = h.version;
        let class = match &h.class {
            b"scnr" => ProfileClass::Input,
            b"mntr" => ProfileClass::Display,
            b"prtr" => ProfileClass::Output,
            b"spac" => ProfileClass::ColorSpace,
            _ => return Err(ColorError::UnsupportedProfileClass(sig_str(h.class))),
        };
        let color_space = match &h.color_space {
            b"RGB " => ProfileColorSpace::Rgb,
            b"GRAY" => ProfileColorSpace::Gray,
            _ => return Err(ColorError::UnsupportedColorSpace(sig_str(h.color_space))),
        };
        let pcs_sig = h.pcs;
        let rendering_intent = h.rendering_intent;

        // ---- tag table
        let table = parse_tag_table(bytes)?;
        let has_lut_tags = LUT_TAGS.iter().any(|t| table.has(t));

        if &pcs_sig != b"XYZ " {
            return Err(if has_lut_tags {
                ColorError::LutOnlyProfile
            } else {
                ColorError::InvalidProfile("matrix/TRC profiles require the XYZ PCS")
            });
        }

        // ---- colorimetric model
        let model = match color_space {
            ProfileColorSpace::Rgb => {
                let req: [&[u8; 4]; 6] = [b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"];
                if let Some(missing) = req.iter().find(|t| !table.has(t)) {
                    if has_lut_tags {
                        return Err(ColorError::LutOnlyProfile);
                    }
                    return Err(ColorError::MissingTag(match *missing {
                        b"rXYZ" => "rXYZ",
                        b"gXYZ" => "gXYZ",
                        b"bXYZ" => "bXYZ",
                        b"rTRC" => "rTRC",
                        b"gTRC" => "gTRC",
                        _ => "bTRC",
                    }));
                }
                let get = |t: &[u8; 4]| table.get(t).ok_or_else(|| tag_err(*t, "tag data out of bounds"));
                let r = parse_xyz_tag(*b"rXYZ", get(b"rXYZ")?)?;
                let g = parse_xyz_tag(*b"gXYZ", get(b"gXYZ")?)?;
                let b = parse_xyz_tag(*b"bXYZ", get(b"bXYZ")?)?;
                let matrix = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
                if math::invert(&matrix).is_none() {
                    return Err(ColorError::InvalidProfile("singular colorant matrix"));
                }
                let trc = Box::new([
                    parse_curve_tag(*b"rTRC", get(b"rTRC")?)?,
                    parse_curve_tag(*b"gTRC", get(b"gTRC")?)?,
                    parse_curve_tag(*b"bTRC", get(b"bTRC")?)?,
                ]);
                Model::Rgb { matrix, trc }
            }
            ProfileColorSpace::Gray => {
                let Some(d) = table.get(b"kTRC") else {
                    return Err(if has_lut_tags { ColorError::LutOnlyProfile } else { ColorError::MissingTag("kTRC") });
                };
                Model::Gray { trc: parse_curve_tag(*b"kTRC", d)? }
            }
        };

        let media_white = match table.get(b"wtpt") {
            Some(d) => Some(parse_xyz_tag(*b"wtpt", d)?),
            None => None,
        };
        let chad = match table.get(b"chad") {
            Some(d) => {
                if sig(d, 0) != Some(*b"sf32") {
                    return Err(tag_err(*b"chad", "expected s15Fixed16ArrayType"));
                }
                let mut m = [[0.0; 3]; 3];
                for (i, v) in m.iter_mut().flatten().enumerate() {
                    *v = s15f16(d, 8 + 4 * i).ok_or_else(|| tag_err(*b"chad", "chad needs 9 values"))?;
                }
                Some(m)
            }
            None => None,
        };
        let description = table.get(b"desc").and_then(parse_text_tag).unwrap_or_default();
        let copyright = table.get(b"cprt").and_then(parse_text_tag);

        Ok(Self::assemble(
            bytes.into(),
            version,
            class,
            rendering_intent,
            description,
            copyright,
            media_white,
            chad,
            has_lut_tags,
            model,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        bytes: Arc<[u8]>,
        version: IccVersion,
        class: ProfileClass,
        rendering_intent: u32,
        description: String,
        copyright: Option<String>,
        media_white: Option<Vec3>,
        chad: Option<Mat3>,
        has_lut_tags: bool,
        model: Model,
    ) -> Self {
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let mut p = IccProfile {
            bytes,
            id: ProfileId::new(hash, String::new()),
            version,
            class,
            rendering_intent,
            description,
            copyright,
            media_white,
            chad,
            has_lut_tags,
            model,
        };
        p.id.semantic_name = p.semantic_name();
        p
    }

    fn semantic_name(&self) -> String {
        if self.is_srgb_equivalent() {
            SRGB_SEMANTIC_NAME.to_string()
        } else if self.is_display_p3_equivalent() {
            DISPLAY_P3_SEMANTIC_NAME.to_string()
        } else if !self.description.is_empty() {
            self.description.clone()
        } else {
            match self.color_space() {
                ProfileColorSpace::Rgb => "Unnamed RGB profile".to_string(),
                ProfileColorSpace::Gray => "Unnamed Gray profile".to_string(),
            }
        }
    }

    /// Veskra-generated sRGB IEC 61966-2.1 ICC v4.3 matrix/TRC profile (shared singleton).
    pub fn srgb() -> Arc<IccProfile> {
        static P: OnceLock<Arc<IccProfile>> = OnceLock::new();
        P.get_or_init(|| Arc::new(writer::build_profile(&RgbProfileSpec::srgb()))).clone()
    }

    /// Veskra-generated Display P3 ICC v4.3 matrix/TRC profile (shared singleton). For gated P3
    /// tests/export only — v1 authoring stays sRGB (MASTER_PLAN §6.4).
    pub fn display_p3() -> Arc<IccProfile> {
        static P: OnceLock<Arc<IccProfile>> = OnceLock::new();
        P.get_or_init(|| Arc::new(writer::build_profile(&RgbProfileSpec::display_p3()))).clone()
    }

    /// Veskra-generated ICC v4.3 matrix/TRC profile with the colorimetry of Adobe RGB (1998)
    /// (see [`RgbProfileSpec::adobe_rgb_compatible`]); export target only.
    pub fn adobe_rgb_compatible() -> Arc<IccProfile> {
        static P: OnceLock<Arc<IccProfile>> = OnceLock::new();
        P.get_or_init(|| Arc::new(writer::build_profile(&RgbProfileSpec::adobe_rgb_compatible()))).clone()
    }

    /// The exact profile bytes (embed these on export).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// SHA-256 of [`bytes`](Self::bytes) plus semantic name (`"sRGB IEC61966-2.1"` for any
    /// sRGB-equivalent profile, `"Display P3"` for P3 equivalents, otherwise the description).
    pub fn id(&self) -> ProfileId {
        self.id.clone()
    }

    /// Profile description (`desc` tag), sanitized; empty if absent or malformed.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Copyright text (`cprt`), if present.
    pub fn copyright(&self) -> Option<&str> {
        self.copyright.as_deref()
    }

    /// Header version.
    pub fn version(&self) -> IccVersion {
        self.version
    }

    /// Header device class.
    pub fn class(&self) -> ProfileClass {
        self.class
    }

    /// Header rendering intent (0 perceptual, 1 relative, 2 saturation, 3 absolute).
    pub fn header_rendering_intent(&self) -> u32 {
        self.rendering_intent
    }

    /// Data color space.
    pub fn color_space(&self) -> ProfileColorSpace {
        match self.model {
            Model::Rgb { .. } => ProfileColorSpace::Rgb,
            Model::Gray { .. } => ProfileColorSpace::Gray,
        }
    }

    /// `wtpt` tag as stored, if present.
    pub fn media_white_point(&self) -> Option<[f64; 3]> {
        self.media_white
    }

    /// Media white used for absolute colorimetric scaling, following LittleCMS
    /// (`_cmsReadMediaWhitePoint`): D50 if `wtpt` is absent, D50 for v2 display profiles
    /// (whose `wtpt` historically held the unadapted display white), otherwise `wtpt`.
    pub fn effective_media_white(&self) -> [f64; 3] {
        match self.media_white {
            None => D50_XYZ,
            Some(_) if self.version.major < 4 && self.class == ProfileClass::Display => D50_XYZ,
            Some(w) => w,
        }
    }

    /// `chad` chromatic adaptation matrix, if present.
    pub fn chromatic_adaptation(&self) -> Option<[[f64; 3]; 3]> {
        self.chad
    }

    /// Whether A2Bx/B2Ax/D2Bx tags are present (ignored by Veskra's matrix/TRC engine).
    pub fn has_lut_tags(&self) -> bool {
        self.has_lut_tags
    }

    /// Linear device RGB → PCS XYZ (D50) matrix (`[[rX gX bX] [rY gY bY] [rZ gZ bZ]]`).
    /// For Gray profiles this is `diag(D50)`: a replicated linear gray `(g,g,g)` maps to `g·D50`.
    pub fn to_xyz_d50_matrix(&self) -> [[f64; 3]; 3] {
        match &self.model {
            Model::Rgb { matrix, .. } => *matrix,
            Model::Gray { .. } => math::diag(D50_XYZ),
        }
    }

    /// Tone curve for channel 0..3 (RGB) or 0 (Gray).
    pub fn trc(&self, channel: usize) -> Option<&ToneCurve> {
        match &self.model {
            Model::Rgb { trc, .. } => trc.get(channel),
            Model::Gray { trc } => (channel == 0).then_some(trc),
        }
    }

    /// Encoded device values → PCS XYZ (D50, relative colorimetric). Gray profiles use `rgb[0]`.
    pub fn to_pcs_xyz(&self, rgb: [f64; 3]) -> [f64; 3] {
        match &self.model {
            Model::Rgb { matrix, trc } => {
                math::mul_vec(matrix, [trc[0].eval(rgb[0]), trc[1].eval(rgb[1]), trc[2].eval(rgb[2])])
            }
            Model::Gray { trc } => {
                let y = trc.eval(rgb[0]);
                [D50_XYZ[0] * y, y, D50_XYZ[2] * y]
            }
        }
    }

    /// Colorimetric comparison for RGB profiles: `(max colorant XYZ difference, max encoded
    /// curve round-trip error)`; `None` if either profile is not RGB or `other`'s curves are not
    /// invertible.
    pub fn colorimetric_difference(&self, other: &IccProfile) -> Option<(f64, f64)> {
        let (Model::Rgb { matrix: ma, trc: ta }, Model::Rgb { matrix: mb, trc: tb }) = (&self.model, &other.model)
        else {
            return None;
        };
        if !tb.iter().all(ToneCurve::is_invertible) {
            return None;
        }
        let curve = (0..3).map(|i| ta[i].encoded_roundtrip_error(&tb[i])).fold(0.0, f64::max);
        Some((math::max_abs_diff(ma, mb), curve))
    }

    /// Whether `self` and `other` describe the same relative colorimetry within the given
    /// tolerances (colorant XYZ, encoded curve round-trip error).
    pub fn is_colorimetrically_equivalent(&self, other: &IccProfile, matrix_tol: f64, curve_tol: f64) -> bool {
        if self.bytes == other.bytes {
            return true;
        }
        match (&self.model, &other.model) {
            (Model::Gray { trc: a }, Model::Gray { trc: b }) => {
                b.is_invertible() && a.encoded_roundtrip_error(b) <= curve_tol
            }
            (Model::Rgb { matrix: ma, .. }, Model::Rgb { matrix: mb, .. }) => {
                // Cheap matrix check first; curves are only sampled when colorants agree.
                math::max_abs_diff(ma, mb) <= matrix_tol
                    && self.colorimetric_difference(other).is_some_and(|(_, c)| c <= curve_tol)
            }
            _ => false,
        }
    }

    /// True when this RGB profile has sRGB colorimetry (Bradford-adapted D50 colorants and the
    /// IEC 61966-2.1 curve) within [`SRGB_EQUIVALENCE_MATRIX_TOL`]/[`SRGB_EQUIVALENCE_CURVE_TOL`],
    /// regardless of its vendor, version, description or `wtpt`.
    pub fn is_srgb_equivalent(&self) -> bool {
        self.matches_reference(&reference_model(&RgbProfileSpec::srgb()))
    }

    /// True when this RGB profile has Display P3 colorimetry (same tolerances as sRGB).
    pub fn is_display_p3_equivalent(&self) -> bool {
        self.matches_reference(&reference_model(&RgbProfileSpec::display_p3()))
    }

    fn matches_reference(&self, reference: &(Mat3, ToneCurve)) -> bool {
        let Model::Rgb { matrix, trc } = &self.model else {
            return false;
        };
        math::max_abs_diff(matrix, &reference.0) <= SRGB_EQUIVALENCE_MATRIX_TOL
            && trc.iter().all(|t| t.encoded_roundtrip_error(&reference.1) <= SRGB_EQUIVALENCE_CURVE_TOL)
    }
}

/// Unquantized reference colorimetry of a spec (for equivalence checks).
fn reference_model(spec: &RgbProfileSpec) -> (Mat3, ToneCurve) {
    (spec.d50_matrix().unwrap_or(math::IDENTITY), ToneCurve::Parametric(spec.trc))
}

#[cfg(test)]
mod tests;
