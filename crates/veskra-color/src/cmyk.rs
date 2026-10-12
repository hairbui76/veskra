//! CMYK output profiles and RGB ↔ CMYK transforms (ADR-018, amends ADR-006).
//!
//! # Scope
//! * [`CmykProfile`]: an ICC v2/v4 **output** (`prtr`) profile with data color space CMYK and a Lab
//!   or XYZ PCS, described by LUT tags (`A2B0..2`: CMYK → PCS, `B2A0..2`: PCS → CMYK; `mft1`,
//!   `mft2`, `mAB `/`mBA `, see [`crate::icc::lut`]). The source / working space stays RGB
//!   (matrix/TRC, [`IccProfile`]). Abstract and device-link profiles stay rejected.
//! * The profile bundled with Veskra, [`BUNDLED_CMYK_PROFILE`] (`Coated_Fogra39L_VIGC_300.icc`,
//!   X-Rite/VIGC, redistributed unaltered; see `profiles/README.md`), is compiled in and parsed
//!   once ([`CmykProfile::bundled`]).
//! * [`CmykTransform`]: RGB → CMYK (export, picker) and CMYK → RGB (picker, previews).
//!
//! # Rendering intents (as LittleCMS 2.19)
//! Perceptual uses `B2A0`/`A2B0`, relative colorimetric `B2A1`/`A2B1`, saturation `B2A2`/`A2B2`;
//! a missing intent tag falls back to the perceptual one (LittleCMS `_cmsReadOutputLUT`), and
//! — a Veskra extension for incomplete profiles — to any other present table if that is missing
//! too. Absolute colorimetric is not offered for CMYK output.
//!
//! # Black point compensation
//! Adobe / LittleCMS BPC: `XYZ' = (XYZ − bpIn) · (D50 − bpOut) / (D50 − bpIn) + bpOut` per
//! component, applied in PCS XYZ between the profiles. Black points are detected exactly as
//! LittleCMS does (`cmsDetectBlackPoint` / `cmsDetectDestinationBlackPoint`, without
//! `CMS_USE_PROFILE_BLACK_POINT_TAG`): RGB matrix/TRC → the Lab of RGB (0,0,0); CMYK as output →
//! the Adobe round-trip algorithm (Lab ramp through `B2Ax` → `A2B1`, for relative colorimetric a
//! "nearly straight mid-range" test, else a least-squares quadratic fit of the shadow region),
//! seeded for relative colorimetric with the perceptual black (`B2A0` → `A2B1` round trip of
//! Lab 0, made neutral); CMYK as input → that perceptual black (relative) or the Lab of 400 % ink
//! (perceptual / saturation; L* clipped, a*/b* kept as in LittleCMS 2.19); perceptual /
//! saturation on a v4 profile → the fixed ICC perceptual black, and BPC is then forced on
//! (LittleCMS rule). The RGB → CMYK → RGB round trip used for gamut checks undoes the forward
//! mapping exactly.
//!
//! # Gamut
//! A color is **in gamut** when RGB → CMYK → PCS, mapped back through the inverse of the forward
//! black point compensation, is within ΔE00 [`GAMUT_DELTA_E00`] (2.0) of the source color.
//!
//! # Performance: device link
//! Bulk conversion ([`CmykTransform::convert_rgba16_premul`]) uses a precomputed
//! [`LINK_GRID_POINTS`]³ (33³, LittleCMS's default for RGB input) grid of the exact pipeline
//! evaluated at gamma-encoded RGB nodes, stored as `f32` (C, M, Y, K, round-trip ΔE00) and read
//! with tetrahedral interpolation. Its cost against the direct pipeline is measured in
//! `tests/cmyk_oracle.rs` and recorded in ADR-018. Single colors ([`CmykTransform::rgb_to_cmyk`])
//! always use the direct pipeline.

use crate::error::{ColorError, sig_str};
use crate::icc::lut::{LutPipeline, LutType, SharedBytes, parse_lut};
use crate::icc::{
    IccProfile, IccVersion, ProfileSummary, parse_header, parse_tag_table, parse_text_tag, parse_xyz_tag, summarize,
};
use crate::lab::{ciede2000, lab_to_xyz_d50, xyz_d50_to_lab};
use crate::math::{self, D50_XYZ, Mat3, Vec3};
use crate::transfer::ToneCurve;
use crate::transform::{GAMUT_CLIP_EPS, RenderingIntent, destination_inverse, rgb_parts};
use std::sync::{Arc, OnceLock};
use veskra_core::limits::MAX_OUTPUT_PROFILE_BYTES;

/// The bundled CMYK output profile, byte for byte (`profiles/Coated_Fogra39L_VIGC_300.icc`; may
/// be embedded and shared, must not be altered or sold — see `profiles/README.md`).
pub const BUNDLED_CMYK_PROFILE: &[u8] = include_bytes!("../profiles/Coated_Fogra39L_VIGC_300.icc");
/// File name of the bundled profile.
pub const BUNDLED_CMYK_PROFILE_FILE: &str = "Coated_Fogra39L_VIGC_300.icc";
/// Round-trip color difference (CIEDE2000) above which a color counts as out of the CMYK gamut.
pub const GAMUT_DELTA_E00: f64 = 2.0;
/// Grid points per RGB axis of the precomputed device link.
pub const LINK_GRID_POINTS: usize = 33;

/// LittleCMS's normalization of PCS XYZ (`u1Fixed15`): `1 + 32767/32768`.
const MAX_ENCODEABLE_XYZ: f64 = 1.0 + 32767.0 / 32768.0;
/// ICC v4 perceptual reference medium black (LittleCMS `cmsPERCEPTUAL_BLACK_*`).
const PERCEPTUAL_BLACK: Vec3 = [0.00336, 0.0034731, 0.00287];
/// Legacy 16-bit Lab (`lut16Type`) ↔ LittleCMS v4 normalization.
const LAB_V2_TO_V4: f64 = 65535.0 / 65280.0;

/// Profile connection space of a CMYK profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pcs {
    Lab,
    Xyz,
}

fn unit(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

fn tag_index(intent: RenderingIntent) -> usize {
    match intent {
        RenderingIntent::Perceptual => 0,
        RenderingIntent::Saturation => 2,
        RenderingIntent::RelativeColorimetric | RenderingIntent::AbsoluteColorimetric => 1,
    }
}

/// A parsed CMYK output profile. The original bytes are kept verbatim (embed them on export).
#[derive(Clone)]
pub struct CmykProfile {
    bytes: SharedBytes,
    version: IccVersion,
    description: String,
    copyright: Option<String>,
    pcs: Pcs,
    media_white: Option<Vec3>,
    /// Which of `A2B0..2` / `B2A0..2` the profile itself contains.
    a2b_present: [bool; 3],
    b2a_present: [bool; 3],
    /// Table used per intent (perceptual, relative, saturation) after the fallback rules.
    a2b: [Arc<LutPipeline>; 3],
    b2a: [Arc<LutPipeline>; 3],
    bundled: bool,
}

impl std::fmt::Debug for CmykProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CmykProfile")
            .field("description", &self.description)
            .field("version", &self.version)
            .field("pcs", &self.pcs)
            .field("bytes", &self.bytes.as_slice().len())
            .field("bundled", &self.bundled)
            .finish()
    }
}

/// Pick the table for each intent: the intent's own tag, else tag 0, else any present one.
fn resolve(tables: [Option<LutPipeline>; 3], missing: &'static str) -> Result<[Arc<LutPipeline>; 3], ColorError> {
    let arcs: [Option<Arc<LutPipeline>>; 3] = tables.map(|t| t.map(Arc::new));
    let any = arcs.iter().flatten().next().cloned().ok_or(ColorError::MissingTag(missing))?;
    let pick = |i: usize| arcs[i].clone().or_else(|| arcs[0].clone()).unwrap_or_else(|| any.clone());
    Ok([pick(0), pick(1), pick(2)])
}

impl CmykProfile {
    /// The bundled Coated FOGRA39 profile, parsed once.
    pub fn bundled() -> Result<Arc<CmykProfile>, ColorError> {
        static P: OnceLock<Result<Arc<CmykProfile>, ColorError>> = OnceLock::new();
        P.get_or_init(|| Self::parse_shared(SharedBytes::Static(BUNDLED_CMYK_PROFILE), true).map(Arc::new)).clone()
    }

    /// Parse a CMYK output profile (≤ [`MAX_OUTPUT_PROFILE_BYTES`]). Hostile data yields typed
    /// errors: wrong class ([`ColorError::UnsupportedProfileClass`]), wrong color space
    /// ([`ColorError::UnsupportedColorSpace`]), missing tables ([`ColorError::MissingTag`]),
    /// malformed tables ([`ColorError::InvalidTag`]), size ([`ColorError::TooLarge`]).
    pub fn parse(bytes: &[u8]) -> Result<CmykProfile, ColorError> {
        if bytes.len() > MAX_OUTPUT_PROFILE_BYTES {
            return Err(ColorError::TooLarge { size: bytes.len(), limit: MAX_OUTPUT_PROFILE_BYTES });
        }
        Self::parse_shared(SharedBytes::Owned(Arc::from(bytes)), false)
    }

    /// [`parse`](Self::parse) taking ownership of the buffer.
    pub fn parse_vec(bytes: Vec<u8>) -> Result<CmykProfile, ColorError> {
        if bytes.len() > MAX_OUTPUT_PROFILE_BYTES {
            return Err(ColorError::TooLarge { size: bytes.len(), limit: MAX_OUTPUT_PROFILE_BYTES });
        }
        Self::parse_shared(SharedBytes::Owned(Arc::from(bytes)), false)
    }

    fn parse_shared(bytes: SharedBytes, bundled: bool) -> Result<CmykProfile, ColorError> {
        let b = bytes.as_slice();
        let h = parse_header(b, MAX_OUTPUT_PROFILE_BYTES)?;
        if &h.class != b"prtr" {
            return Err(ColorError::UnsupportedProfileClass(sig_str(h.class)));
        }
        if &h.color_space != b"CMYK" {
            return Err(ColorError::UnsupportedColorSpace(sig_str(h.color_space)));
        }
        let pcs = match &h.pcs {
            b"Lab " => Pcs::Lab,
            b"XYZ " => Pcs::Xyz,
            _ => return Err(ColorError::InvalidProfile("PCS must be Lab or XYZ")),
        };
        let table = parse_tag_table(b)?;
        let mut a2b: [Option<LutPipeline>; 3] = [None, None, None];
        let mut b2a: [Option<LutPipeline>; 3] = [None, None, None];
        for (i, tag) in [b"A2B0", b"A2B1", b"A2B2"].into_iter().enumerate() {
            if let Some((off, len)) = table.range(tag) {
                let mut p = parse_lut(*tag, &bytes, off, len, true, 4, 3)?;
                if pcs == Pcs::Lab && p.kind() == LutType::Lut16 {
                    p.scale_output(LAB_V2_TO_V4);
                }
                a2b[i] = Some(p);
            }
        }
        for (i, tag) in [b"B2A0", b"B2A1", b"B2A2"].into_iter().enumerate() {
            if let Some((off, len)) = table.range(tag) {
                let mut p = parse_lut(*tag, &bytes, off, len, false, 3, 4)?;
                if pcs == Pcs::Lab && p.kind() == LutType::Lut16 {
                    p.scale_input(1.0 / LAB_V2_TO_V4);
                }
                b2a[i] = Some(p);
            }
        }
        let a2b_present = [a2b[0].is_some(), a2b[1].is_some(), a2b[2].is_some()];
        let b2a_present = [b2a[0].is_some(), b2a[1].is_some(), b2a[2].is_some()];
        let b2a = resolve(b2a, "B2A0")?;
        let a2b = resolve(a2b, "A2B0")?;
        let media_white = match table.get(b"wtpt") {
            Some(d) => Some(parse_xyz_tag(*b"wtpt", d)?),
            None => None,
        };
        let description = table.get(b"desc").and_then(parse_text_tag).unwrap_or_default();
        let copyright = table.get(b"cprt").and_then(parse_text_tag);
        Ok(CmykProfile {
            version: h.version,
            description,
            copyright,
            pcs,
            media_white,
            a2b_present,
            b2a_present,
            a2b,
            b2a,
            bundled,
            bytes,
        })
    }

    /// The exact profile bytes (embed these).
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// Profile description (`desc`), sanitized; empty if absent.
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

    /// Profile connection space.
    pub fn pcs(&self) -> Pcs {
        self.pcs
    }

    /// `wtpt` as stored.
    pub fn media_white_point(&self) -> Option<[f64; 3]> {
        self.media_white
    }

    /// `true` for [`CmykProfile::bundled`].
    pub fn is_bundled(&self) -> bool {
        self.bundled
    }

    /// Whether the profile has its own table for `intent` in the output (`B2Ax`, `output = true`)
    /// or input (`A2Bx`) direction — LittleCMS `cmsIsIntentSupported` for LUT profiles.
    pub fn has_table(&self, intent: RenderingIntent, output: bool) -> bool {
        let i = tag_index(intent);
        if output { self.b2a_present[i] } else { self.a2b_present[i] }
    }

    fn a2b(&self, intent: RenderingIntent) -> &LutPipeline {
        &self.a2b[tag_index(intent)]
    }

    fn b2a(&self, intent: RenderingIntent) -> &LutPipeline {
        &self.b2a[tag_index(intent)]
    }

    fn lab_to_pcs(&self, lab: Vec3) -> Vec3 {
        match self.pcs {
            Pcs::Lab => [lab[0] / 100.0, (lab[1] + 128.0) / 255.0, (lab[2] + 128.0) / 255.0],
            Pcs::Xyz => lab_to_xyz_d50(lab).map(|v| v / MAX_ENCODEABLE_XYZ),
        }
    }

    fn pcs_to_lab(&self, v: Vec3) -> Vec3 {
        match self.pcs {
            Pcs::Lab => [v[0] * 100.0, v[1] * 255.0 - 128.0, v[2] * 255.0 - 128.0],
            Pcs::Xyz => xyz_d50_to_lab(v.map(|x| x * MAX_ENCODEABLE_XYZ)),
        }
    }

    fn xyz_to_pcs(&self, xyz: Vec3) -> Vec3 {
        match self.pcs {
            Pcs::Lab => self.lab_to_pcs(xyz_d50_to_lab(xyz)),
            Pcs::Xyz => xyz.map(|v| v / MAX_ENCODEABLE_XYZ),
        }
    }

    fn pcs_to_xyz(&self, v: Vec3) -> Vec3 {
        match self.pcs {
            Pcs::Lab => lab_to_xyz_d50(self.pcs_to_lab(v)),
            Pcs::Xyz => v.map(|x| x * MAX_ENCODEABLE_XYZ),
        }
    }

    /// PCS Lab (D50, media-relative) → CMYK `[0, 1]` (1 = full ink) through `B2Ax`.
    pub fn lab_to_cmyk(&self, lab: [f64; 3], intent: RenderingIntent) -> [f64; 4] {
        let mut out = [0.0; 4];
        self.b2a(intent).eval(&self.lab_to_pcs(lab), &mut out);
        out.map(unit)
    }

    /// CMYK `[0, 1]` → PCS Lab (D50, media-relative) through `A2Bx`.
    pub fn cmyk_to_lab(&self, cmyk: [f64; 4], intent: RenderingIntent) -> [f64; 3] {
        let mut out = [0.0; 3];
        self.a2b(intent).eval(&cmyk.map(unit), &mut out);
        self.pcs_to_lab(out)
    }

    fn xyz_to_cmyk(&self, xyz: Vec3, intent: RenderingIntent) -> [f64; 4] {
        let mut out = [0.0; 4];
        self.b2a(intent).eval(&self.xyz_to_pcs(xyz), &mut out);
        out.map(unit)
    }

    fn cmyk_to_xyz(&self, cmyk: [f64; 4], intent: RenderingIntent) -> Vec3 {
        let mut out = [0.0; 3];
        self.a2b(intent).eval(&cmyk.map(unit), &mut out);
        self.pcs_to_xyz(out)
    }

    /// LittleCMS `CreateRoundtripXForm`: Lab → `B2A[intent]` → `A2B1` → Lab.
    fn roundtrip_lab(&self, lab: Vec3, intent: RenderingIntent) -> Vec3 {
        let mut cmyk = [0.0; 4];
        self.b2a(intent).eval(&self.lab_to_pcs(lab), &mut cmyk);
        let mut back = [0.0; 3];
        self.a2b(RenderingIntent::RelativeColorimetric).eval(&cmyk, &mut back);
        self.pcs_to_lab(back)
    }

    fn is_v4_perceptual_like(&self, intent: RenderingIntent) -> bool {
        self.version.major >= 4 && matches!(intent, RenderingIntent::Perceptual | RenderingIntent::Saturation)
    }

    /// LittleCMS `cmsDetectBlackPoint` for this profile used as **input** (PCS XYZ).
    pub fn input_black_point(&self, intent: RenderingIntent) -> [f64; 3] {
        if intent == RenderingIntent::AbsoluteColorimetric {
            return [0.0; 3];
        }
        if self.is_v4_perceptual_like(intent) {
            return PERCEPTUAL_BLACK;
        }
        if intent == RenderingIntent::RelativeColorimetric {
            // Output-class CMYK: "discount ink limiting" — BlackPointUsingPerceptualBlack.
            if !self.has_table(RenderingIntent::Perceptual, false) {
                return [0.0; 3];
            }
            let lab = self.roundtrip_lab([0.0; 3], RenderingIntent::Perceptual);
            return lab_to_xyz_d50([lab[0].min(50.0), 0.0, 0.0]);
        }
        // BlackPointAsDarkerColorant: the darkest device color (all inks 100 %).
        if !self.has_table(intent, false) {
            return [0.0; 3];
        }
        darker_colorant_black(self.cmyk_to_lab([1.0; 4], intent))
    }

    /// LittleCMS `cmsDetectDestinationBlackPoint` for this profile used as **output** (Adobe
    /// algorithm for LUT-based CMYK output profiles).
    pub fn output_black_point(&self, intent: RenderingIntent) -> [f64; 3] {
        if self.is_v4_perceptual_like(intent) {
            return PERCEPTUAL_BLACK;
        }
        if !self.has_table(intent, true) {
            return self.input_black_point(intent);
        }
        let relative = intent == RenderingIntent::RelativeColorimetric;
        let initial = if relative {
            xyz_d50_to_lab(self.input_black_point(RenderingIntent::RelativeColorimetric))
        } else {
            [0.0; 3]
        };
        let (a, b) = (initial[1].clamp(-50.0, 50.0), initial[2].clamp(-50.0, 50.0));
        let mut in_ramp = [0.0f64; 256];
        let mut out_ramp = [0.0f64; 256];
        for l in 0..256 {
            let lab = [l as f64 * 100.0 / 255.0, a, b];
            in_ramp[l] = lab[0];
            out_ramp[l] = self.roundtrip_lab(lab, intent)[0];
        }
        for l in (1..=254).rev() {
            out_ramp[l] = out_ramp[l].min(out_ramp[l + 1]);
        }
        if out_ramp[0].partial_cmp(&out_ramp[255]) != Some(std::cmp::Ordering::Less) {
            return [0.0; 3];
        }
        let (min_l, max_l) = (out_ramp[0], out_ramp[255]);
        if relative {
            let straight =
                (0..256).all(|l| in_ramp[l] <= min_l + 0.2 * (max_l - min_l) || (in_ramp[l] - out_ramp[l]).abs() < 4.0);
            if straight {
                return lab_to_xyz_d50(initial);
            }
        }
        let (lo, hi) = if relative { (0.1, 0.5) } else { (0.03, 0.25) };
        let mut xs = Vec::with_capacity(256);
        let mut ys = Vec::with_capacity(256);
        for l in 0..256 {
            let y = (out_ramp[l] - min_l) / (max_l - min_l);
            if y >= lo && y < hi {
                xs.push(in_ramp[l]);
                ys.push(y);
            }
        }
        if xs.len() < 3 {
            return [0.0; 3];
        }
        let l = root_of_least_squares_quadratic(&xs, &ys).max(0.0);
        lab_to_xyz_d50([l, initial[1], initial[2]])
    }
}

/// LittleCMS `RootOfLeastSquaresFitQuadraticCurve`, including its edge cases.
fn root_of_least_squares_quadratic(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n < 4 {
        return 0.0;
    }
    let (mut sx, mut sx2, mut sx3, mut sx4, mut sy, mut syx, mut syx2) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let (xn, yn) = (x[i], y[i]);
        sx += xn;
        sx2 += xn * xn;
        sx3 += xn * xn * xn;
        sx4 += xn * xn * xn * xn;
        sy += yn;
        syx += yn * xn;
        syx2 += yn * xn * xn;
    }
    let m: Mat3 = [[n as f64, sx, sx2], [sx, sx2, sx3], [sx2, sx3, sx4]];
    if math::det(&m).abs() < 1e-4 {
        return 0.0;
    }
    let Some(inv) = math::invert(&m) else { return 0.0 };
    let res = math::mul_vec(&inv, [sy, syx, syx2]);
    let (c, b, a) = (res[0], res[1], res[2]);
    if a.abs() < 1e-10 {
        // LittleCMS returns cmsmin(0, cmsmax(50, -c/b)), i.e. 0, for a linear fit.
        return 0.0;
    }
    let d = b * b - 4.0 * a * c;
    if d <= 0.0 {
        return 0.0;
    }
    ((-b + d.sqrt()) / (2.0 * a)).clamp(0.0, 50.0)
}

/// Black point compensation `XYZ' = a·XYZ + b` (per component).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bpc {
    a: Vec3,
    b: Vec3,
}

impl Bpc {
    /// `None` when the black points are equal (LittleCMS then adds nothing).
    fn new(bp_in: Vec3, bp_out: Vec3) -> Option<Bpc> {
        if bp_in == bp_out {
            return None;
        }
        let mut a = [0.0; 3];
        let mut b = [0.0; 3];
        for i in 0..3 {
            let t = bp_in[i] - D50_XYZ[i];
            if t.abs() < 1e-12 || !t.is_finite() {
                return None;
            }
            a[i] = (bp_out[i] - D50_XYZ[i]) / t;
            b[i] = -D50_XYZ[i] * (bp_out[i] - bp_in[i]) / t;
        }
        (a.iter().all(|v| v.is_finite() && v.abs() > 1e-12) && b.iter().all(|v| v.is_finite())).then_some(Bpc { a, b })
    }

    fn apply(&self, x: Vec3) -> Vec3 {
        [self.a[0] * x[0] + self.b[0], self.a[1] * x[1] + self.b[1], self.a[2] * x[2] + self.b[2]]
    }

    fn invert(&self, x: Vec3) -> Vec3 {
        [(x[0] - self.b[0]) / self.a[0], (x[1] - self.b[1]) / self.a[1], (x[2] - self.b[2]) / self.a[2]]
    }
}

/// LittleCMS 2.19 `BlackPointAsDarkerColorant` clipping of the darkest device color's Lab:
/// `L* > 95` → 0 (synthetic negative profiles), `L* < 0` → 0, `L* > 50` → 50; a* and b* are kept
/// (older LittleCMS releases forced the point neutral — verified against the 2.19 oracle, where
/// the neutral variant differs by up to 6 ΔE00 on CMYK → sRGB perceptual).
fn darker_colorant_black(lab: Vec3) -> Vec3 {
    let l = if lab[0] > 95.0 || lab[0] < 0.0 {
        0.0
    } else if lab[0] > 50.0 {
        50.0
    } else {
        lab[0]
    };
    lab_to_xyz_d50([l, lab[1], lab[2]])
}

/// LittleCMS black point of an RGB matrix/TRC profile (input or destination): the darker
/// colorant, i.e. the Lab of device black (0, 0, 0), clipped as [`darker_colorant_black`].
fn rgb_black_point(m: &Mat3, trc: &[ToneCurve; 3]) -> Vec3 {
    let xyz = math::mul_vec(m, [trc[0].eval(0.0), trc[1].eval(0.0), trc[2].eval(0.0)]);
    darker_colorant_black(xyz_d50_to_lab(xyz))
}

/// Running statistics of a bulk conversion.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CmykStats {
    /// Pixels with alpha > 0 that were converted.
    pub pixels: u64,
    /// Of those, pixels whose color is outside the CMYK gamut (round-trip ΔE00 > 2).
    pub out_of_gamut: u64,
    /// Largest C+M+Y+K (each 0..=1, so 0..=4) of a converted pixel.
    pub max_ink: f64,
}

impl CmykStats {
    /// Accumulate `other` into `self`.
    pub fn merge(&mut self, other: &CmykStats) {
        self.pixels = self.pixels.saturating_add(other.pixels);
        self.out_of_gamut = self.out_of_gamut.saturating_add(other.out_of_gamut);
        self.max_ink = self.max_ink.max(other.max_ink);
    }

    /// Maximum total area coverage in percent (0–400), rounded to 0.01.
    pub fn tac_percent(&self) -> f64 {
        (self.max_ink * 10_000.0).round() / 100.0
    }
}

/// Precomputed RGB → (C, M, Y, K, round-trip ΔE00) grid, tetrahedral interpolation.
#[derive(Clone)]
pub struct DeviceLink {
    n: usize,
    nodes: Box<[[f32; 5]]>,
}

impl std::fmt::Debug for DeviceLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceLink").field("grid", &self.n).finish()
    }
}

impl DeviceLink {
    fn build(t: &CmykTransform, n: usize) -> DeviceLink {
        let plane = n * n;
        let mut nodes = vec![[0.0f32; 5]; plane * n];
        let step = 1.0 / (n - 1) as f64;
        let fill = |r: usize, out: &mut [[f32; 5]]| {
            for (i, node) in out.iter_mut().enumerate() {
                let rgb = [r as f64 * step, (i / n) as f64 * step, (i % n) as f64 * step];
                let (cmyk, de) = t.rgb_to_cmyk_with_delta_e(rgb);
                *node = [cmyk[0] as f32, cmyk[1] as f32, cmyk[2] as f32, cmyk[3] as f32, de as f32];
            }
        };
        // One work item per red plane, drained by up to 8 scoped threads and this one (if a
        // thread cannot be spawned, the others drain its share). Results never depend on
        // scheduling: every plane is written by exactly one worker.
        let queue = std::sync::Mutex::new(nodes.chunks_mut(plane).enumerate().collect::<Vec<_>>());
        let next = || queue.lock().ok().and_then(|mut q| q.pop());
        let threads = std::thread::available_parallelism().map_or(1, |v| v.get()).clamp(1, 8);
        std::thread::scope(|s| {
            for _ in 1..threads {
                let _ = std::thread::Builder::new().spawn_scoped(s, || {
                    while let Some((r, p)) = next() {
                        fill(r, p);
                    }
                });
            }
            while let Some((r, p)) = next() {
                fill(r, p);
            }
        });
        DeviceLink { n, nodes: nodes.into_boxed_slice() }
    }

    /// Grid points per axis.
    pub fn grid_points(&self) -> usize {
        self.n
    }

    /// Interpolated `[c, m, y, k, ΔE00]` for encoded RGB in `[0, 1]`.
    #[inline]
    pub fn eval(&self, rgb: [f32; 3]) -> [f32; 5] {
        let n = self.n;
        let nm1 = (n - 1) as f32;
        let loc = |v: f32| {
            let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
            let p = v * nm1;
            let i = (p as usize).min(n - 2);
            (i, p - i as f32)
        };
        let (x0, rx) = loc(rgb[0]);
        let (y0, ry) = loc(rgb[1]);
        let (z0, rz) = loc(rgb[2]);
        let (sx, sy, sz) = (n * n, n, 1usize);
        let o = x0 * sx + y0 * sy + z0;
        let (s1, s2, f1, f2, f3) = if rx >= ry {
            if ry >= rz {
                (sx, sx + sy, rx, ry, rz)
            } else if rz >= rx {
                (sz, sz + sx, rz, rx, ry)
            } else {
                (sx, sx + sz, rx, rz, ry)
            }
        } else if rx >= rz {
            (sy, sy + sx, ry, rx, rz)
        } else if ry >= rz {
            (sy, sy + sz, ry, rz, rx)
        } else {
            (sz, sz + sy, rz, ry, rx)
        };
        let zero = [0.0f32; 5];
        let c0 = self.nodes.get(o).unwrap_or(&zero);
        let c1 = self.nodes.get(o + s1).unwrap_or(&zero);
        let c2 = self.nodes.get(o + s2).unwrap_or(&zero);
        let c3 = self.nodes.get(o + sx + sy + sz).unwrap_or(&zero);
        std::array::from_fn(|k| c0[k] + (c1[k] - c0[k]) * f1 + (c2[k] - c1[k]) * f2 + (c3[k] - c2[k]) * f3)
    }
}

/// RGB (matrix/TRC) ↔ CMYK (LUT output profile) transform. Cheap to share (`Arc`); the device
/// link is built on first bulk use.
pub struct CmykTransform {
    profile: Arc<CmykProfile>,
    intent: RenderingIntent,
    src_matrix: Mat3,
    src_trc: [ToneCurve; 3],
    dst_inv: Mat3,
    fwd: Option<Bpc>,
    rev: Option<Bpc>,
    fwd_black: (Vec3, Vec3),
    bpc: bool,
    link: OnceLock<DeviceLink>,
}

impl std::fmt::Debug for CmykTransform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CmykTransform")
            .field("profile", &self.profile.description())
            .field("intent", &self.intent)
            .field("bpc", &self.bpc)
            .finish()
    }
}

impl CmykTransform {
    /// Build `rgb` (an RGB matrix/TRC profile, normally the document's sRGB) ↔ `profile`.
    /// `bpc` = black point compensation; it is forced on for perceptual/saturation when the
    /// destination of a direction is a v4 profile (LittleCMS rule) and never used for absolute
    /// colorimetric, which is not supported for CMYK output.
    pub fn new(
        rgb: &IccProfile,
        profile: Arc<CmykProfile>,
        intent: RenderingIntent,
        bpc: bool,
    ) -> Result<CmykTransform, ColorError> {
        if intent == RenderingIntent::AbsoluteColorimetric {
            return Err(ColorError::TransformFailure("absolute colorimetric is not supported for CMYK output"));
        }
        let (src_matrix, src_trc) = rgb_parts(rgb)?;
        let dst_inv = destination_inverse(rgb, &src_trc)?;
        let perceptual_like = matches!(intent, RenderingIntent::Perceptual | RenderingIntent::Saturation);
        let fwd_on = bpc || (perceptual_like && profile.version().major >= 4);
        let rev_on = bpc || (perceptual_like && rgb.version().major >= 4);
        let rgb_black = rgb_black_point(&src_matrix, &src_trc);
        let dst_black = profile.output_black_point(intent);
        let fwd = if fwd_on { Bpc::new(rgb_black, dst_black) } else { None };
        let rev = if rev_on { Bpc::new(profile.input_black_point(intent), rgb_black) } else { None };
        Ok(CmykTransform {
            profile,
            intent,
            src_matrix,
            src_trc,
            dst_inv,
            fwd,
            rev,
            fwd_black: (rgb_black, dst_black),
            bpc: fwd_on,
            link: OnceLock::new(),
        })
    }

    /// The CMYK profile.
    pub fn profile(&self) -> &Arc<CmykProfile> {
        &self.profile
    }

    /// Intent requested at construction.
    pub fn intent(&self) -> RenderingIntent {
        self.intent
    }

    /// Whether black point compensation is in effect for RGB → CMYK.
    pub fn black_point_compensation(&self) -> bool {
        self.bpc
    }

    /// `(source black point, destination black point)` in PCS XYZ as detected for RGB → CMYK.
    pub fn black_points(&self) -> ([f64; 3], [f64; 3]) {
        self.fwd_black
    }

    fn src_xyz(&self, rgb: [f64; 3]) -> Vec3 {
        let rgb = rgb.map(unit);
        math::mul_vec(
            &self.src_matrix,
            [self.src_trc[0].eval(rgb[0]), self.src_trc[1].eval(rgb[1]), self.src_trc[2].eval(rgb[2])],
        )
    }

    /// Encoded RGB `[0, 1]` → CMYK `[0, 1]` (1 = 100 % ink), direct pipeline.
    pub fn rgb_to_cmyk(&self, rgb: [f64; 3]) -> [f64; 4] {
        let mut xyz = self.src_xyz(rgb);
        if let Some(b) = &self.fwd {
            xyz = b.apply(xyz);
        }
        self.profile.xyz_to_cmyk(xyz, self.intent)
    }

    /// [`rgb_to_cmyk`](Self::rgb_to_cmyk) plus the round-trip ΔE00 (see module docs).
    pub fn rgb_to_cmyk_with_delta_e(&self, rgb: [f64; 3]) -> ([f64; 4], f64) {
        let xyz0 = self.src_xyz(rgb);
        let xyz = self.fwd.as_ref().map_or(xyz0, |b| b.apply(xyz0));
        let cmyk = self.profile.xyz_to_cmyk(xyz, self.intent);
        let mut back = self.profile.cmyk_to_xyz(cmyk, self.intent);
        if let Some(b) = &self.fwd {
            back = b.invert(back);
        }
        let de = ciede2000(xyz_d50_to_lab(xyz0), xyz_d50_to_lab(back));
        (cmyk, if de.is_finite() { de } else { f64::MAX })
    }

    /// Whether the RGB color survives RGB → CMYK → RGB within [`GAMUT_DELTA_E00`].
    pub fn in_gamut(&self, rgb: [f64; 3]) -> bool {
        self.rgb_to_cmyk_with_delta_e(rgb).1 <= GAMUT_DELTA_E00
    }

    /// CMYK `[0, 1]` → PCS Lab D50 (media-relative, no black point compensation).
    pub fn cmyk_to_lab(&self, cmyk: [f64; 4]) -> [f64; 3] {
        self.profile.cmyk_to_lab(cmyk, self.intent)
    }

    /// CMYK `[0, 1]` → encoded RGB, and whether the color was outside the RGB gamut and
    /// clipped (a linear channel beyond `[0, 1]` by more than [`GAMUT_CLIP_EPS`]).
    pub fn cmyk_to_rgb(&self, cmyk: [f64; 4]) -> ([f64; 3], bool) {
        let mut xyz = self.profile.cmyk_to_xyz(cmyk, self.intent);
        if let Some(b) = &self.rev {
            xyz = b.apply(xyz);
        }
        let lin = math::mul_vec(&self.dst_inv, xyz);
        let clipped = lin.iter().any(|v| !v.is_finite() || *v < -GAMUT_CLIP_EPS || *v > 1.0 + GAMUT_CLIP_EPS);
        let rgb = std::array::from_fn(|c| self.src_trc[c].eval_inverse(unit(lin[c])));
        (rgb, clipped)
    }

    /// The precomputed device link (built on first use, in parallel, ~36k pipeline evaluations).
    pub fn link(&self) -> &DeviceLink {
        self.link.get_or_init(|| DeviceLink::build(self, LINK_GRID_POINTS))
    }

    /// Convert premultiplied RGBA16 pixels (`px.len() / 4` pixels) to **straight** CMYK + alpha,
    /// 5 values per pixel in `out` (ink 0 = none, 65535 = 100 %; alpha unchanged; alpha 0 →
    /// all zero) through the device link. Extra values are left untouched.
    pub fn convert_rgba16_premul(&self, px: &[u16], out: &mut [u16]) -> CmykStats {
        let link = self.link();
        let mut stats = CmykStats::default();
        let mut last_key = [0u16; 4];
        let mut last = ([0u16; 4], false, 0.0f64);
        let mut have_last = false;
        for (p, o) in px.chunks_exact(4).zip(out.chunks_exact_mut(5)) {
            let a = p[3];
            if a == 0 {
                o.fill(0);
                continue;
            }
            let key = [p[0], p[1], p[2], a];
            if !have_last || key != last_key {
                let af = a as f32;
                let v =
                    link.eval([(p[0] as f32 / af).min(1.0), (p[1] as f32 / af).min(1.0), (p[2] as f32 / af).min(1.0)]);
                let ink: [u16; 4] = std::array::from_fn(|k| (v[k].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16);
                let sum = ink.iter().map(|&i| i as f64).sum::<f64>() / 65535.0;
                last = (ink, v[4] > GAMUT_DELTA_E00 as f32, sum);
                last_key = key;
                have_last = true;
            }
            o[..4].copy_from_slice(&last.0);
            o[4] = a;
            stats.pixels += 1;
            stats.out_of_gamut += last.1 as u64;
            if last.2 > stats.max_ink {
                stats.max_ink = last.2;
            }
        }
        stats
    }
}

/// Why `summary` cannot be used as a CMYK export target (`None` if its header and tag table
/// qualify; the LUT tables themselves are checked by [`CmykProfile::parse`]).
pub fn cmyk_export_unsuitability(summary: &ProfileSummary) -> Option<String> {
    let class_name = match summary.class.as_str() {
        "prtr" => None,
        "mntr" => Some("a display (monitor) profile"),
        "scnr" => Some("an input (scanner/camera) profile"),
        "spac" => Some("a color space profile"),
        "abst" => Some("an abstract profile"),
        "link" => Some("a device link profile"),
        "nmcl" => Some("a named color profile"),
        _ => Some("a profile of an unknown class"),
    };
    if let Some(c) = class_name {
        return Some(format!(
            "This is {c} (class '{}'); CMYK export needs an output (printer) profile.",
            summary.class
        ));
    }
    if summary.color_space != "CMYK" {
        return Some(format!(
            "This output profile describes {} data, not CMYK.",
            if summary.color_space.is_empty() { "unknown" } else { &summary.color_space }
        ));
    }
    if summary.pcs != "Lab" && summary.pcs != "XYZ" {
        return Some(format!("Unsupported profile connection space '{}' (expected Lab or XYZ).", summary.pcs));
    }
    if !["B2A0", "B2A1", "B2A2"].iter().any(|t| summary.has_tag(t)) {
        return Some("The profile has no B2A table, so it cannot convert colors to CMYK.".into());
    }
    if !["A2B0", "A2B1", "A2B2"].iter().any(|t| summary.has_tag(t)) {
        return Some("The profile has no A2B table, so CMYK colors cannot be previewed or checked.".into());
    }
    None
}

/// Inspect profile bytes for `profile.inspect`: the structural summary plus `None` when usable for
/// CMYK export, else the reason. Structurally invalid data (and corrupt LUT tables of an
/// otherwise suitable CMYK profile) is an error.
pub fn inspect_for_cmyk_export(bytes: &[u8]) -> Result<(ProfileSummary, Option<String>), ColorError> {
    let summary = summarize(bytes, MAX_OUTPUT_PROFILE_BYTES)?;
    if let Some(reason) = cmyk_export_unsuitability(&summary) {
        return Ok((summary, Some(reason)));
    }
    CmykProfile::parse(bytes)?;
    Ok((summary, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_profile_parses_and_is_unaltered() {
        let p = CmykProfile::bundled().unwrap();
        assert!(p.is_bundled());
        assert_eq!(p.bytes().len(), 8_652_444);
        assert_eq!(p.bytes(), BUNDLED_CMYK_PROFILE);
        assert_eq!(p.version().major, 2);
        assert_eq!(p.pcs(), Pcs::Lab);
        assert_eq!(p.description(), "Coated_Fogra39L_VIGC_300.icc");
        assert_eq!(p.copyright(), Some("Copyright X-Rite, Inc."));
        for i in [RenderingIntent::Perceptual, RenderingIntent::RelativeColorimetric, RenderingIntent::Saturation] {
            assert!(p.has_table(i, true) && p.has_table(i, false));
        }
        // Paper white → no ink; 100 % K is dark and neutral-ish.
        let w = p.lab_to_cmyk([100.0, 0.0, 0.0], RenderingIntent::RelativeColorimetric);
        assert!(w.iter().all(|v| *v < 0.01), "{w:?}");
        let k = p.cmyk_to_lab([0.0, 0.0, 0.0, 1.0], RenderingIntent::RelativeColorimetric);
        assert!(k[0] < 35.0 && k[1].abs() < 5.0 && k[2].abs() < 5.0, "{k:?}");
    }

    #[test]
    fn black_point_and_bpc_round_trip() {
        let p = CmykProfile::bundled().unwrap();
        let t = CmykTransform::new(&IccProfile::srgb(), p, RenderingIntent::RelativeColorimetric, true).unwrap();
        let (src, dst) = t.black_points();
        assert_eq!(src, [0.0; 3]);
        assert!(dst[1] > 0.0 && dst[1] < 0.05, "{dst:?}");
        let b = Bpc::new(src, dst).unwrap();
        for x in [[0.0; 3], D50_XYZ, [0.3, 0.2, 0.1]] {
            let r = b.invert(b.apply(x));
            assert!((0..3).all(|i| (r[i] - x[i]).abs() < 1e-12));
        }
        assert!(Bpc::new(dst, dst).is_none());
        let wd = b.apply(D50_XYZ);
        assert!((0..3).all(|i| (wd[i] - D50_XYZ[i]).abs() < 1e-12), "white is preserved");
        // sRGB black maps to the darkest CMYK and survives the round trip.
        let (cmyk, de) = t.rgb_to_cmyk_with_delta_e([0.0; 3]);
        assert!(cmyk[3] > 0.95, "{cmyk:?}");
        assert!(de < 1.0, "{de}");
        let (rgb, clipped) = t.cmyk_to_rgb(cmyk);
        assert!(rgb.iter().all(|v| *v < 0.02) && !clipped, "{rgb:?}");
        assert!(
            CmykTransform::new(
                &IccProfile::srgb(),
                CmykProfile::bundled().unwrap(),
                RenderingIntent::AbsoluteColorimetric,
                true
            )
            .is_err()
        );
    }

    #[test]
    fn quadratic_root_edge_cases() {
        assert_eq!(root_of_least_squares_quadratic(&[1.0, 2.0], &[1.0, 2.0]), 0.0);
        // Linear data → LittleCMS returns 0.
        assert_eq!(root_of_least_squares_quadratic(&[1.0, 2.0, 3.0, 4.0], &[2.0, 4.0, 6.0, 8.0]), 0.0);
        // y = (x - 5)(x + 3) / 100: the larger root is 5.
        let xs: Vec<f64> = (6..20).map(|v| v as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| (x - 5.0) * (x + 3.0) / 100.0).collect();
        let r = root_of_least_squares_quadratic(&xs, &ys);
        assert!((r - 5.0).abs() < 1e-6, "{r}");
        // A double root (discriminant 0) is reported as 0, like LittleCMS.
        let ys: Vec<f64> = xs.iter().map(|x| (x - 5.0) * (x - 5.0) / 100.0).collect();
        assert!(root_of_least_squares_quadratic(&xs, &ys) < 5.0 + 1e-3);
    }

    #[test]
    fn suitability_reasons() {
        let s = summarize(IccProfile::srgb().bytes(), MAX_OUTPUT_PROFILE_BYTES).unwrap();
        assert_eq!(s.class, "mntr");
        assert_eq!(s.color_space, "RGB");
        assert_eq!(s.pcs, "XYZ");
        let r = cmyk_export_unsuitability(&s).unwrap();
        assert!(r.contains("display"), "{r}");
        let (sum, reason) = inspect_for_cmyk_export(BUNDLED_CMYK_PROFILE).unwrap();
        assert_eq!(reason, None);
        assert_eq!(sum.version_string(), "2.1");
        assert_eq!(sum.class, "prtr");
        assert!(sum.has_tag("B2A1") && sum.has_tag("gamt"));
        assert!(matches!(CmykProfile::parse(IccProfile::srgb().bytes()), Err(ColorError::UnsupportedProfileClass(_))));
    }
}
