//! Deterministic writer for Veskra's own ICC v4.3 RGB matrix/TRC display profiles.
//!
//! The generated data is computed from published colorimetric definitions (primaries and white
//! chromaticities, transfer-curve parameters); it contains no third-party profile bytes.
//!
//! Layout (ICC.1:2022, version field 4.3): header, 10 tag-table entries (`desc cprt wtpt chad
//! rXYZ gXYZ bXYZ rTRC gTRC bTRC`, the three TRC entries share one `para` element), tag data
//! 4-byte aligned, MD5 Profile ID in the header. Header date is fixed (2026-01-01T00:00:00) so the
//! bytes are identical across runs and platforms.

use super::{IccProfile, md5};
use crate::error::ColorError;
use crate::math::{self, D50_XYZ, D65_XY, Mat3};
use crate::transfer::ParametricCurve;

/// Inputs for a generated RGB matrix/TRC profile.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbProfileSpec {
    /// `desc` text (en-US `mluc`).
    pub description: &'static str,
    /// `cprt` text (en-US `mluc`).
    pub copyright: &'static str,
    /// Red, green, blue CIE 1931 xy chromaticities.
    pub primaries: [[f64; 2]; 3],
    /// Native white xy (adapted to D50 with Bradford).
    pub white: [f64; 2],
    /// Transfer curve written as `para` (types 0–4) for all three channels.
    pub trc: ParametricCurve,
}

impl RgbProfileSpec {
    /// sRGB per IEC 61966-2.1: ITU-R BT.709 primaries, D65 white, sRGB piecewise curve.
    pub fn srgb() -> Self {
        Self {
            description: "sRGB IEC61966-2.1 (Veskra)",
            copyright: "No copyright, use freely",
            primaries: [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
            white: D65_XY,
            trc: ParametricCurve::SRGB,
        }
    }

    /// Display P3: DCI-P3 primaries (SMPTE EG 432-1), D65 white, sRGB piecewise curve.
    pub fn display_p3() -> Self {
        Self {
            description: "Display P3 (Veskra)",
            copyright: "No copyright, use freely",
            primaries: [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
            white: D65_XY,
            trc: ParametricCurve::SRGB,
        }
    }

    /// A wide-gamut RGB space **compatible with Adobe RGB (1998)**: the published colorimetry of
    /// that encoding (primaries R (0.64, 0.33), G (0.21, 0.71), B (0.15, 0.06), D65 white, pure
    /// gamma 563/256 = 2.19921875). The profile is generated here from those numbers; it is not
    /// Adobe's profile file and does not use its name as its own.
    pub fn adobe_rgb_compatible() -> Self {
        Self {
            description: "Veskra RGB (compatible with Adobe RGB 1998)",
            copyright: "No copyright, use freely",
            primaries: [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
            white: D65_XY,
            trc: ParametricCurve::new(0, &[563.0 / 256.0]).unwrap_or(ParametricCurve::SRGB),
        }
    }

    /// Unquantized Bradford D50-adapted linear RGB → PCS XYZ matrix.
    pub fn d50_matrix(&self) -> Option<Mat3> {
        let native = math::rgb_to_xyz_matrix(self.primaries, self.white)?;
        let cat = self.chad()?;
        Some(math::mul(&cat, &native))
    }

    /// Unquantized Bradford adaptation from the native white to D50.
    pub fn chad(&self) -> Option<Mat3> {
        math::bradford(math::xy_to_xyz(self.white)?, D50_XYZ)
    }
}

fn s15(v: f64) -> i32 {
    (v * 65536.0).round() as i32
}

fn put_s15(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn xyz_tag(v: [i32; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    v.iter().for_each(|&x| put_s15(&mut t, x));
    t
}

fn mluc_tag(text: &str) -> Vec<u8> {
    let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut t = b"mluc\0\0\0\0".to_vec();
    t.extend_from_slice(&1u32.to_be_bytes()); // record count
    t.extend_from_slice(&12u32.to_be_bytes()); // record size
    t.extend_from_slice(b"enUS");
    t.extend_from_slice(&(utf16.len() as u32).to_be_bytes());
    t.extend_from_slice(&28u32.to_be_bytes()); // string offset from tag start
    t.extend_from_slice(&utf16);
    t
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

/// Quantize the colorant matrix to s15Fixed16 so that each row (X, Y, Z) sums **exactly** to the
/// encoded D50 PCS white; the rounding residual (|·| ≤ 1 unit) goes to the largest entry of the
/// row. This makes device white map to the PCS white bit-exactly, as ICC.1 Annex F recommends.
fn quantize_colorants(m: &Mat3) -> [[i32; 3]; 3] {
    let mut q = [[0i32; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            q[r][c] = s15(m[r][c]);
        }
        let residual = s15(D50_XYZ[r]) - q[r].iter().sum::<i32>();
        let mut big = 0;
        for c in 1..3 {
            if m[r][c].abs() > m[r][big].abs() {
                big = c;
            }
        }
        q[r][big] += residual;
    }
    q
}

/// Generate the profile bytes for `spec`. Deterministic: identical inputs give identical bytes.
pub fn build_rgb_profile_bytes(spec: &RgbProfileSpec) -> Result<Vec<u8>, ColorError> {
    let m = spec.d50_matrix().ok_or(ColorError::InvalidProfile("degenerate primaries or white point"))?;
    let chad = spec.chad().ok_or(ColorError::InvalidProfile("degenerate white point"))?;
    let q = quantize_colorants(&m);
    let column = |c: usize| [q[0][c], q[1][c], q[2][c]];

    let mut para = b"para\0\0\0\0".to_vec();
    para.extend_from_slice(&spec.trc.function_type().to_be_bytes());
    para.extend_from_slice(&[0, 0]);
    spec.trc.params().iter().for_each(|&p| put_s15(&mut para, s15(p)));

    let mut chad_tag = b"sf32\0\0\0\0".to_vec();
    chad.iter().flatten().for_each(|&v| put_s15(&mut chad_tag, s15(v)));

    let blobs: Vec<Vec<u8>> = vec![
        mluc_tag(spec.description),
        mluc_tag(spec.copyright),
        xyz_tag([s15(D50_XYZ[0]), s15(D50_XYZ[1]), s15(D50_XYZ[2])]),
        chad_tag,
        xyz_tag(column(0)),
        xyz_tag(column(1)),
        xyz_tag(column(2)),
        para,
    ];
    let entries: [(&[u8; 4], usize); 10] = [
        (b"desc", 0),
        (b"cprt", 1),
        (b"wtpt", 2),
        (b"chad", 3),
        (b"rXYZ", 4),
        (b"gXYZ", 5),
        (b"bXYZ", 6),
        (b"rTRC", 7),
        (b"gTRC", 7),
        (b"bTRC", 7),
    ];

    let mut data = Vec::new();
    let data_start = 128 + 4 + 12 * entries.len();
    let mut placed = Vec::with_capacity(blobs.len());
    for b in &blobs {
        placed.push((data_start + data.len(), b.len()));
        data.extend_from_slice(b);
        pad4(&mut data);
    }
    let total = data_start + data.len();

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes()); // 0 size
    out.extend_from_slice(&[0; 4]); // 4 preferred CMM: none
    out.extend_from_slice(&[0x04, 0x30, 0, 0]); // 8 version 4.3.0.0
    out.extend_from_slice(b"mntr"); // 12 class
    out.extend_from_slice(b"RGB "); // 16 data color space
    out.extend_from_slice(b"XYZ "); // 20 PCS
    for v in [2026u16, 1, 1, 0, 0, 0] {
        out.extend_from_slice(&v.to_be_bytes()); // 24 creation date (fixed for determinism)
    }
    out.extend_from_slice(b"acsp"); // 36
    out.extend_from_slice(&[0; 4]); // 40 platform: none
    out.extend_from_slice(&[0; 4]); // 44 flags
    out.extend_from_slice(&[0; 4]); // 48 manufacturer
    out.extend_from_slice(&[0; 4]); // 52 model
    out.extend_from_slice(&[0; 8]); // 56 attributes
    out.extend_from_slice(&0u32.to_be_bytes()); // 64 rendering intent: perceptual
    for v in D50_XYZ {
        put_s15(&mut out, s15(v)); // 68 PCS illuminant
    }
    out.extend_from_slice(&[0; 4]); // 80 creator
    out.extend_from_slice(&[0; 16]); // 84 profile ID (filled below)
    out.extend_from_slice(&[0; 28]); // 100 reserved
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (sig, blob) in entries {
        let (off, len) = placed[blob];
        out.extend_from_slice(sig);
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(&data);
    debug_assert_eq!(out.len(), total);
    if let Some(id) = md5::icc_profile_id(&out) {
        out[84..100].copy_from_slice(&id);
    }
    Ok(out)
}

/// Build and parse a generated profile. Only called with the built-in constant specs, whose
/// validity is covered by unit tests, so failure is a programming error.
pub(super) fn build_profile(spec: &RgbProfileSpec) -> IccProfile {
    build_rgb_profile_bytes(spec)
        .and_then(|b| IccProfile::parse(&b))
        .expect("built-in Veskra profile spec must produce a valid profile (covered by unit tests)")
}
