//! CMYK print export (ADR-018): output profile loading (bundled or a user file), the conversion
//! plan shared by the PSD and PDF writers, the preflight estimate, and `profile.inspect`.
//!
//! # Profiles
//! `cmykProfile: null` selects the bundled `Coated_Fogra39L_VIGC_300.icc`; a path selects an ICC
//! file of at most [`MAX_OUTPUT_PROFILE_BYTES`] that must be a CMYK output (`prtr`) profile with
//! A2B/B2A tables. Parsed files are cached per process by (path, length, modification time), and
//! transforms (incl. their 33³ device link) by (profile, document profile, intent, BPC), so a
//! preflight followed by an export, or repeated picker conversions, pay the setup once.
//!
//! # Conversion
//! Pixels are composited by Veskra in the document's RGB space (sRGB), then converted per pixel
//! (straight color, alpha unchanged) through the transform's device link. Statistics — pixels
//! outside the CMYK gamut (round-trip ΔE00 > 2) and the largest C+M+Y+K — are taken on the
//! flattened image only (PSD merged image; PDF: every embedded raster image), never twice for the
//! same pixel. Total area coverage is computed from the 16-bit conversion before quantization.

use crate::png::warning;
use std::cell::Cell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use veskra_color::cmyk::{CmykProfile, CmykStats, CmykTransform, inspect_for_cmyk_export};
use veskra_color::{ColorError, IccProfile, RenderingIntent};
use veskra_compose::ComposeOptions;
use veskra_core::export::{
    CmykProfileInfo, ExportFormat, ExportIntent, ExportOptions, ExportWarning, ExportWarningKind,
};
use veskra_core::limits::MAX_OUTPUT_PROFILE_BYTES;
use veskra_core::{BlendMode, ErrorCode, RectI, VeskraError};
use veskra_doc::Document;

/// Pixels composited at most for the preflight estimate (every other tile-row band beyond this).
pub const PREFLIGHT_SAMPLE_PIXELS: u64 = 4 << 20;
/// Parsed profile files kept in the process-wide cache.
const PROFILE_CACHE_ENTRIES: usize = 4;
/// Transforms kept in the process-wide cache (each holds a ~700 KiB device link once built).
const TRANSFORM_CACHE_ENTRIES: usize = 4;

/// Veskra's color-engine intent for an export intent.
pub fn rendering_intent(i: ExportIntent) -> RenderingIntent {
    match i {
        ExportIntent::Perceptual => RenderingIntent::Perceptual,
        _ => RenderingIntent::RelativeColorimetric,
    }
}

/// User-facing error for a CMYK profile that cannot be used (never contains the path).
pub fn profile_error(e: ColorError) -> VeskraError {
    let msg = match &e {
        ColorError::UnsupportedProfileClass(c) => {
            format!("The selected profile is not a CMYK output (printer) profile (class '{c}').")
        }
        ColorError::UnsupportedColorSpace(s) => {
            format!("The selected profile describes '{}' colors, not CMYK.", s.trim_end())
        }
        ColorError::TooLarge { .. } => {
            format!("The selected profile is larger than {} MB.", MAX_OUTPUT_PROFILE_BYTES / (1024 * 1024))
        }
        ColorError::MissingTag(t) => {
            format!("The selected CMYK profile has no {t} table, so it cannot be used to convert colors.")
        }
        other => format!("The selected file is not a usable ICC profile: {other}"),
    };
    VeskraError::new(e.code(), msg)
}

fn io_error(e: &std::io::Error) -> VeskraError {
    match e.kind() {
        std::io::ErrorKind::NotFound => VeskraError::new(ErrorCode::NotFound, "The CMYK profile file was not found."),
        std::io::ErrorKind::PermissionDenied => VeskraError::new(
            ErrorCode::FilePermissionDenied,
            "The CMYK profile file cannot be read (permission denied).",
        ),
        k => VeskraError::new(ErrorCode::Io, format!("The CMYK profile file cannot be read: {k}.")),
    }
}

/// Read a profile file of at most [`MAX_OUTPUT_PROFILE_BYTES`] (bounded even if it grows).
fn read_profile_file(path: &Path) -> Result<(Vec<u8>, u64, Option<SystemTime>), VeskraError> {
    let meta = std::fs::metadata(path).map_err(|e| io_error(&e))?;
    if !meta.is_file() {
        return Err(VeskraError::new(ErrorCode::InvalidArgument, "The CMYK profile path is not a file."));
    }
    if meta.len() > MAX_OUTPUT_PROFILE_BYTES as u64 {
        return Err(profile_error(ColorError::TooLarge {
            size: usize::try_from(meta.len()).unwrap_or(usize::MAX),
            limit: MAX_OUTPUT_PROFILE_BYTES,
        }));
    }
    let f = std::fs::File::open(path).map_err(|e| io_error(&e))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    f.take(MAX_OUTPUT_PROFILE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| io_error(&e))?;
    if bytes.len() > MAX_OUTPUT_PROFILE_BYTES {
        return Err(profile_error(ColorError::TooLarge { size: bytes.len(), limit: MAX_OUTPUT_PROFILE_BYTES }));
    }
    Ok((bytes, meta.len(), meta.modified().ok()))
}

type ProfileKey = (PathBuf, u64, Option<SystemTime>);
type ProfileCache = Mutex<Vec<(ProfileKey, Arc<CmykProfile>)>>;

fn profile_cache() -> &'static ProfileCache {
    static C: OnceLock<ProfileCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// The CMYK output profile for `path` (`None` = bundled), parsed and cached.
pub fn load_cmyk_profile(path: Option<&str>) -> Result<Arc<CmykProfile>, VeskraError> {
    let Some(path) = path else {
        return CmykProfile::bundled().map_err(profile_error);
    };
    let p = PathBuf::from(path);
    let meta = std::fs::metadata(&p).map_err(|e| io_error(&e))?;
    let key: ProfileKey = (p.clone(), meta.len(), meta.modified().ok());
    if let Ok(c) = profile_cache().lock()
        && let Some((_, prof)) = c.iter().find(|(k, _)| *k == key)
    {
        return Ok(prof.clone());
    }
    let (bytes, len, modified) = read_profile_file(&p)?;
    let prof = Arc::new(CmykProfile::parse_vec(bytes).map_err(profile_error)?);
    if let Ok(mut c) = profile_cache().lock() {
        c.retain(|(k, _)| k.0 != p);
        if c.len() >= PROFILE_CACHE_ENTRIES {
            c.remove(0);
        }
        c.push(((p, len, modified), prof.clone()));
    }
    Ok(prof)
}

type TransformKey = (usize, [u8; 32], RenderingIntent, bool);
type TransformCache = Mutex<Vec<(TransformKey, Arc<CmykTransform>)>>;

fn transform_cache() -> &'static TransformCache {
    static C: OnceLock<TransformCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// A cached RGB ↔ CMYK transform. The cache key holds the profile's address; the cached
/// transform owns an `Arc` of that profile, so the address cannot be reused while it is cached.
pub fn cmyk_transform(
    src: &IccProfile,
    profile: &Arc<CmykProfile>,
    intent: RenderingIntent,
    bpc: bool,
) -> Result<Arc<CmykTransform>, VeskraError> {
    let key: TransformKey = (Arc::as_ptr(profile) as usize, src.id().icc_hash, intent, bpc);
    if let Ok(c) = transform_cache().lock()
        && let Some((_, t)) = c.iter().find(|(k, _)| *k == key)
    {
        return Ok(t.clone());
    }
    let t = Arc::new(CmykTransform::new(src, profile.clone(), intent, bpc).map_err(|e| {
        VeskraError::new(ErrorCode::ColorTransformFailure, format!("cannot build the CMYK conversion: {e}"))
    })?);
    if let Ok(mut c) = transform_cache().lock() {
        if c.len() >= TRANSFORM_CACHE_ENTRIES {
            c.remove(0);
        }
        c.push((key, t.clone()));
    }
    Ok(t)
}

/// Flate-compressed bytes of the bundled profile for PDF output intents (compressed once).
pub(crate) fn bundled_profile_flate() -> &'static [u8] {
    static Z: OnceLock<Vec<u8>> = OnceLock::new();
    Z.get_or_init(|| miniz_oxide::deflate::compress_to_vec_zlib(veskra_color::cmyk::BUNDLED_CMYK_PROFILE, 6))
}

/// Report name of an intent (the `renderingIntent` convention of preflight/report).
fn intent_name(i: RenderingIntent) -> &'static str {
    i.as_str()
}

/// How a CMYK export converts and tags its pixels (PSD and PDF).
#[derive(Debug)]
pub(crate) struct CmykPlan {
    pub transform: Arc<CmykTransform>,
    /// Embed the profile (PSD resource 1039 / PDF output intent).
    pub embed: bool,
    pub info: CmykProfileInfo,
    /// Human-readable profile semantics for preflight/report.
    pub label: String,
    /// Description of the CMYK profile (the space the numbers are in).
    pub target_name: String,
    /// `"relative-colorimetric"` or `"perceptual"`.
    pub intent_name: String,
    pub warnings: Vec<ExportWarning>,
    stats: Cell<CmykStats>,
}

fn doc_profile(doc: &Document) -> Result<Arc<IccProfile>, VeskraError> {
    crate::png::document_profile(doc)
}

/// Visible layers/groups with a blend mode other than Normal (pass-through groups excluded).
pub(crate) fn non_normal_blend_count(doc: &Document) -> u32 {
    doc.iter_layers()
        .filter(|l| l.props.blend != BlendMode::Normal && !l.group().is_some_and(|g| g.pass_through))
        .count() as u32
}

impl CmykPlan {
    pub(crate) fn new(doc: &Document, opts: &ExportOptions, format: ExportFormat) -> Result<CmykPlan, VeskraError> {
        let src = doc_profile(doc)?;
        let profile = load_cmyk_profile(opts.cmyk_profile.as_deref())?;
        let intent = rendering_intent(opts.rendering_intent);
        let transform = cmyk_transform(&src, &profile, intent, opts.black_point_compensation)?;
        let desc = if profile.description().is_empty() {
            "Unnamed CMYK profile".to_string()
        } else {
            profile.description().to_string()
        };
        let src_name = src.id().semantic_name;
        let bpc = if transform.black_point_compensation() { " + black point compensation" } else { "" };
        let how = format!("{}{bpc}", intent_name(intent));
        let embed = opts.embed_color_profile;
        let mut warnings = Vec::new();
        let tag = match format {
            ExportFormat::Pdf => "DeviceCMYK with a PDF/X-style output intent",
            _ => "ICC profile resource",
        };
        let label = if embed {
            format!("{desc} (CMYK, {tag}, converted from {src_name}, {how})")
        } else {
            format!("CMYK values for {desc} (profile not embedded, converted from {src_name}, {how})")
        };
        warnings.push(warning(
            ExportWarningKind::ColorConverted,
            if embed {
                format!(
                    "Colors are converted from '{src_name}' to CMYK with '{desc}' ({how}); the profile is embedded."
                )
            } else {
                format!("Colors are converted from '{src_name}' to CMYK with '{desc}' ({how}).")
            },
            None,
            0,
        ));
        if !embed {
            warnings.push(warning(
                ExportWarningKind::ProfileNotEmbedded,
                format!(
                    "The CMYK profile is not embedded ('Embed color profile' is off): the file holds CMYK numbers for '{desc}' without a profile, so the printer must assume the same printing condition."
                ),
                None,
                1,
            ));
        }
        warnings.push(warning(
            ExportWarningKind::GamutClipped,
            format!(
                "Colors outside the '{desc}' CMYK gamut are mapped into it ({}); how many pixels were affected is counted in the export report.",
                intent_name(intent)
            ),
            None,
            0,
        ));
        let blends = non_normal_blend_count(doc);
        if blends > 0 {
            let msg = match format {
                ExportFormat::Psd => format!(
                    "{blends} layer(s)/group(s) use a blend mode other than Normal. Veskra composites in RGB and converts each layer to CMYK; Photoshop recomposites CMYK layers in CMYK, so these blends (and overlaps of semi-transparent layers) may look different there. The merged image is Veskra's render."
                ),
                _ => format!(
                    "{blends} layer(s)/group(s) use a blend mode other than Normal; where they are written as editable vectors, the PDF blends them in DeviceCMYK, which can look different from Veskra's RGB composite."
                ),
            };
            warnings.push(warning(ExportWarningKind::BlendModeApproximated, msg, None, blends));
        }
        Ok(CmykPlan {
            info: CmykProfileInfo { description: desc.clone(), bundled: profile.is_bundled() },
            transform,
            embed,
            label,
            target_name: desc,
            intent_name: intent_name(intent).to_string(),
            warnings,
            stats: Cell::new(CmykStats::default()),
        })
    }

    /// Profile bytes to embed.
    pub(crate) fn profile_bytes(&self) -> &[u8] {
        self.transform.profile().bytes()
    }

    /// Convert premultiplied RGBA16 to straight CMYK+A16 (5 values per pixel, ink 0..65535).
    /// `count` adds the pixels to the export statistics (flattened images only).
    pub(crate) fn convert(&self, px: &[u16], out: &mut Vec<u16>, count: bool) {
        out.resize(px.len() / 4 * 5, 0);
        let s = self.transform.convert_rgba16_premul(px, out);
        if count {
            let mut all = self.stats.get();
            all.merge(&s);
            self.stats.set(all);
        }
    }

    pub(crate) fn stats(&self) -> CmykStats {
        self.stats.get()
    }

    /// Total area coverage (percent) over the counted pixels.
    pub(crate) fn tac(&self) -> f64 {
        self.stats().tac_percent()
    }

    /// Warning for the pixels actually outside the gamut during this export.
    pub(crate) fn gamut_warning(&self) -> Option<ExportWarning> {
        let n = self.stats().out_of_gamut;
        (n > 0).then(|| {
            warning(
                ExportWarningKind::GamutClipped,
                format!(
                    "{n} pixel(s) had colors outside the '{}' CMYK gamut and were mapped into it ({}).",
                    self.target_name, self.intent_name
                ),
                None,
                u32::try_from(n).unwrap_or(u32::MAX),
            )
        })
    }

    /// Preflight estimate on a downsampled composite: whole tile-row bands of the visible
    /// composite, every `k`-th band so that at most [`PREFLIGHT_SAMPLE_PIXELS`] are composited.
    /// Returns the statistics and whether the image was sampled (not every pixel looked at).
    pub(crate) fn estimate(&self, doc: &Document) -> (CmykStats, bool) {
        let (w, h) = (doc.width(), doc.height());
        let band = veskra_raster::TILE;
        let bands = h.div_ceil(band);
        let total = w as u64 * h as u64;
        let step = total.div_ceil(PREFLIGHT_SAMPLE_PIXELS).max(1) as u32;
        let mut stats = CmykStats::default();
        let mut out = Vec::new();
        let mut b = 0u32;
        while b < bands {
            let y0 = b * band;
            let rect = RectI::new(0, y0 as i32, w as i32, (y0 + band).min(h) as i32);
            let px = veskra_compose::composite_rect(doc, rect, &ComposeOptions::default());
            out.resize(px.len() / 4 * 5, 0);
            stats.merge(&self.transform.convert_rgba16_premul(&px, &mut out));
            b += step;
        }
        (stats, step > 1)
    }

    /// Fill the CMYK fields and estimate of a preflight.
    pub(crate) fn preflight_warnings(&self, doc: &Document) -> (Option<f64>, Vec<ExportWarning>) {
        let (s, sampled) = self.estimate(doc);
        let mut w = Vec::new();
        if s.out_of_gamut > 0 {
            let share = s.out_of_gamut as f64 * 100.0 / s.pixels.max(1) as f64;
            let what = if sampled {
                "sampled painted pixels (estimate from a downsampled composite)"
            } else {
                "painted pixels"
            };
            w.push(warning(
                ExportWarningKind::GamutClipped,
                format!("About {share:.1} % of the {what} are outside the '{}' CMYK gamut.", self.target_name),
                None,
                u32::try_from(s.out_of_gamut).unwrap_or(u32::MAX),
            ));
        }
        (Some(s.tac_percent()), w)
    }
}

/// `profile.inspect` result (FFI JSON, camelCase).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInspection {
    pub description: String,
    pub copyright: Option<String>,
    /// Device class signature (`prtr`, `mntr`, `scnr`, `spac`, `abst`, `link`, `nmcl`).
    #[serde(rename = "class")]
    pub class_sig: String,
    /// Data color space (`CMYK`, `RGB`, `GRAY`, `Lab`, …).
    pub color_space: String,
    /// `Lab` or `XYZ`.
    pub pcs: String,
    /// `"2.1"`, `"4.3"`, …
    pub version: String,
    /// Intents the profile has tables for (`perceptual`, `relative_colorimetric`, `saturation`);
    /// matrix/TRC profiles support all three colorimetrically.
    pub intents: Vec<String>,
    pub bytes: u64,
    pub bundled: bool,
    pub usable_for_cmyk_export: bool,
    pub reason: Option<String>,
}

/// Inspect an ICC file (`None` = the bundled CMYK profile). Unsuitable profiles are reported
/// (`usableForCmykExport: false` + `reason`); unreadable or structurally invalid files are errors.
pub fn inspect_profile(path: Option<&str>) -> Result<ProfileInspection, VeskraError> {
    let owned;
    let (bytes, bundled): (&[u8], bool) = match path {
        None => (veskra_color::cmyk::BUNDLED_CMYK_PROFILE, true),
        Some(p) => {
            owned = read_profile_file(Path::new(p))?.0;
            (&owned, false)
        }
    };
    let (s, reason) = inspect_for_cmyk_export(bytes).map_err(|e| {
        VeskraError::new(
            e.code(),
            match e {
                ColorError::TooLarge { .. } => profile_error(e).message,
                other => format!("The file is not a valid ICC profile: {other}"),
            },
        )
    })?;
    let mut intents = Vec::new();
    let matrix_trc = s.has_tag("rXYZ") && s.has_tag("rTRC") || s.has_tag("kTRC");
    for (i, name) in ["perceptual", "relative_colorimetric", "saturation"].iter().enumerate() {
        if matrix_trc || s.has_tag(&format!("B2A{i}")) || s.has_tag(&format!("A2B{i}")) {
            intents.push((*name).to_string());
        }
    }
    Ok(ProfileInspection {
        description: s.description.clone(),
        copyright: s.copyright.clone(),
        class_sig: s.class.clone(),
        color_space: s.color_space.clone(),
        pcs: s.pcs.clone(),
        version: s.version_string(),
        intents,
        bytes: s.bytes as u64,
        bundled,
        usable_for_cmyk_export: reason.is_none(),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_inspection_and_errors() {
        let i = inspect_profile(None).unwrap();
        assert_eq!(i.description, "Coated_Fogra39L_VIGC_300.icc");
        assert_eq!(i.copyright.as_deref(), Some("Copyright X-Rite, Inc."));
        assert_eq!((i.class_sig.as_str(), i.color_space.as_str(), i.pcs.as_str()), ("prtr", "CMYK", "Lab"));
        assert_eq!(i.version, "2.1");
        assert_eq!(i.intents, ["perceptual", "relative_colorimetric", "saturation"]);
        assert_eq!(i.bytes, 8_652_444);
        assert!(i.bundled && i.usable_for_cmyk_export && i.reason.is_none());
        let j = serde_json::to_value(&i).unwrap();
        assert_eq!(j["class"], "prtr");
        assert_eq!(j["usableForCmykExport"], true);
        assert!(j["reason"].is_null());

        let dir = std::env::temp_dir().join(format!("veskra-cmyk-inspect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let srgb = dir.join("srgb.icc");
        std::fs::write(&srgb, IccProfile::srgb().bytes()).unwrap();
        let i = inspect_profile(srgb.to_str()).unwrap();
        assert!(!i.usable_for_cmyk_export && i.reason.as_deref().unwrap().contains("display"));
        assert_eq!((i.class_sig.as_str(), i.color_space.as_str(), i.pcs.as_str()), ("mntr", "RGB", "XYZ"));
        assert_eq!(i.intents.len(), 3);
        let junk = dir.join("junk.icc");
        std::fs::write(&junk, b"not a profile at all").unwrap();
        assert_eq!(inspect_profile(junk.to_str()).unwrap_err().code, ErrorCode::InvalidProfile);
        let missing = dir.join("missing.icc");
        assert_eq!(inspect_profile(missing.to_str()).unwrap_err().code, ErrorCode::NotFound);
        let e = load_cmyk_profile(srgb.to_str()).unwrap_err();
        assert_eq!(e.code, ErrorCode::UnsupportedColorMode);
        assert!(e.message.contains("not a CMYK output"), "{e:?}");
        assert!(!e.message.contains("srgb.icc"), "no paths in messages");
        assert_eq!(load_cmyk_profile(junk.to_str()).unwrap_err().code, ErrorCode::InvalidProfile);
        assert_eq!(load_cmyk_profile(missing.to_str()).unwrap_err().code, ErrorCode::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_profile_is_cached_and_reloaded_when_changed() {
        let dir = std::env::temp_dir().join(format!("veskra-cmyk-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("press.icc");
        std::fs::write(&f, veskra_color::cmyk::BUNDLED_CMYK_PROFILE).unwrap();
        let a = load_cmyk_profile(f.to_str()).unwrap();
        let b = load_cmyk_profile(f.to_str()).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!a.is_bundled());
        assert_eq!(a.bytes(), veskra_color::cmyk::BUNDLED_CMYK_PROFILE);
        let t1 = cmyk_transform(&IccProfile::srgb(), &a, RenderingIntent::RelativeColorimetric, true).unwrap();
        let t2 = cmyk_transform(&IccProfile::srgb(), &b, RenderingIntent::RelativeColorimetric, true).unwrap();
        assert!(Arc::ptr_eq(&t1, &t2));
        // A different file content (length changes) is re-read.
        std::fs::write(&f, b"short").unwrap();
        assert!(load_cmyk_profile(f.to_str()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
