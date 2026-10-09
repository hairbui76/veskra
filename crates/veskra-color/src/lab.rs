//! CIELAB (D50) and CIEDE2000 — the common comparison space for color fidelity tests.
//!
//! * Reference white: the ICC PCS D50 white `X=0.9642, Y=1, Z=0.8249` ([`math::D50_XYZ`]).
//! * CIE constants: exact rational `ε = 216/24389`, `κ = 24389/27` (CIE 15:2004).
//! * sRGB → XYZ D50: exact IEC 61966-2.1 decode, normalized primary matrix from the sRGB
//!   chromaticities and D65 (0.3127, 0.3290), linear Bradford adaptation D65 → D50, all
//!   unquantized `f64` (no ICC s15Fixed16 rounding).
//! * CIEDE2000: Sharma, Wu & Dalal (2005), `kL = kC = kH = 1`.

use crate::icc::IccProfile;
use crate::math::{self, D50_XYZ, D65_XY, Mat3};
use crate::transfer::srgb_to_linear_f64;
use std::sync::OnceLock;

const EPSILON: f64 = 216.0 / 24389.0;
const KAPPA: f64 = 24389.0 / 27.0;

fn f(t: f64) -> f64 {
    if t > EPSILON { t.cbrt() } else { (KAPPA * t + 16.0) / 116.0 }
}

/// XYZ (D50-relative, Y of white = 1) → CIELAB with the ICC D50 white.
pub fn xyz_d50_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    let fx = f(xyz[0] / D50_XYZ[0]);
    let fy = f(xyz[1] / D50_XYZ[1]);
    let fz = f(xyz[2] / D50_XYZ[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIELAB (ICC D50 white) → XYZ D50.
pub fn lab_to_xyz_d50(lab: [f64; 3]) -> [f64; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = fy + lab[1] / 500.0;
    let fz = fy - lab[2] / 200.0;
    let inv = |t: f64| if t * t * t > EPSILON { t * t * t } else { (116.0 * t - 16.0) / KAPPA };
    let y = if lab[0] > KAPPA * EPSILON { fy * fy * fy } else { lab[0] / KAPPA };
    [inv(fx) * D50_XYZ[0], y * D50_XYZ[1], inv(fz) * D50_XYZ[2]]
}

/// Exact (unquantized) linear sRGB → XYZ D50 matrix (Bradford-adapted).
pub fn srgb_linear_to_xyz_d50_matrix() -> [[f64; 3]; 3] {
    static M: OnceLock<Mat3> = OnceLock::new();
    *M.get_or_init(|| {
        let native = math::rgb_to_xyz_matrix([[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], D65_XY);
        let cat = math::xy_to_xyz(D65_XY).and_then(|w| math::bradford(w, D50_XYZ));
        match (native, cat) {
            (Some(n), Some(c)) => math::mul(&c, &n),
            // Unreachable for these constants (unit-tested); identity keeps the function total.
            _ => math::IDENTITY,
        }
    })
}

/// Encoded sRGB (`[0,1]`) → XYZ D50.
pub fn srgb_to_xyz_d50(rgb: [f64; 3]) -> [f64; 3] {
    math::mul_vec(&srgb_linear_to_xyz_d50_matrix(), rgb.map(srgb_to_linear_f64))
}

/// 8-bit sRGB → CIELAB D50 (reference path for fidelity tests).
pub fn srgb8_to_lab_d50(rgb: [u8; 3]) -> [f64; 3] {
    xyz_d50_to_lab(srgb_to_xyz_d50(rgb.map(|v| v as f64 / 255.0)))
}

/// Encoded device values in `profile` → CIELAB D50 via the profile's PCS (relative
/// colorimetric).
pub fn profile_rgb_to_lab_d50(profile: &IccProfile, rgb: [f64; 3]) -> [f64; 3] {
    xyz_d50_to_lab(profile.to_pcs_xyz(rgb))
}

/// CIE76 ΔE*ab.
pub fn delta_e76(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// CIEDE2000 color difference (Sharma, Wu & Dalal 2005, eq. 1–24; `kL = kC = kH = 1`).
pub fn ciede2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let pow25_7 = 25f64.powi(7);

    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let c_bar = (c1 + c2) / 2.0;
    let c_bar7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + pow25_7)).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();
    let hue = |b: f64, ap: f64| {
        if b == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let h1p = hue(b1, a1p);
    let h2p = hue(b2, a2p);

    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let d_big_h = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();

    let l_bar = (l1 + l2) / 2.0;
    let c_bar_p = (c1p + c2p) / 2.0;
    let h_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (h_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * h_bar).to_radians().cos()
        + 0.32 * (3.0 * h_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * h_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((h_bar - 275.0) / 25.0).powi(2)).exp();
    let c_bar_p7 = c_bar_p.powi(7);
    let rc = 2.0 * (c_bar_p7 / (c_bar_p7 + pow25_7)).sqrt();
    let l50 = (l_bar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * c_bar_p;
    let sh = 1.0 + 0.015 * c_bar_p * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;

    let (tl, tc, th) = (dl / sl, dc / sc, d_big_h / sh);
    (tl * tl + tc * tc + th * th + rt * tc * th).max(0.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sharma, Wu & Dalal (2005), Table 1: (L1, a1, b1, L2, a2, b2, ΔE00).
    pub(crate) const SHARMA: [[f64; 7]; 34] = [
        [50.0000, 2.6772, -79.7751, 50.0000, 0.0000, -82.7485, 2.0425],
        [50.0000, 3.1571, -77.2803, 50.0000, 0.0000, -82.7485, 2.8615],
        [50.0000, 2.8361, -74.0200, 50.0000, 0.0000, -82.7485, 3.4412],
        [50.0000, -1.3802, -84.2814, 50.0000, 0.0000, -82.7485, 1.0000],
        [50.0000, -1.1848, -84.8006, 50.0000, 0.0000, -82.7485, 1.0000],
        [50.0000, -0.9009, -85.5211, 50.0000, 0.0000, -82.7485, 1.0000],
        [50.0000, 0.0000, 0.0000, 50.0000, -1.0000, 2.0000, 2.3669],
        [50.0000, -1.0000, 2.0000, 50.0000, 0.0000, 0.0000, 2.3669],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0009, 7.1792],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0010, 7.1792],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0011, 7.2195],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0012, 7.2195],
        [50.0000, -0.0010, 2.4900, 50.0000, 0.0009, -2.4900, 4.8045],
        [50.0000, -0.0010, 2.4900, 50.0000, 0.0010, -2.4900, 4.8045],
        [50.0000, -0.0010, 2.4900, 50.0000, 0.0011, -2.4900, 4.7461],
        [50.0000, 2.5000, 0.0000, 50.0000, 0.0000, -2.5000, 4.3065],
        [50.0000, 2.5000, 0.0000, 73.0000, 25.0000, -18.0000, 27.1492],
        [50.0000, 2.5000, 0.0000, 61.0000, -5.0000, 29.0000, 22.8977],
        [50.0000, 2.5000, 0.0000, 56.0000, -27.0000, -3.0000, 31.9030],
        [50.0000, 2.5000, 0.0000, 58.0000, 24.0000, 15.0000, 19.4535],
        [50.0000, 2.5000, 0.0000, 50.0000, 3.1736, 0.5854, 1.0000],
        [50.0000, 2.5000, 0.0000, 50.0000, 3.2972, 0.0000, 1.0000],
        [50.0000, 2.5000, 0.0000, 50.0000, 1.8634, 0.5757, 1.0000],
        [50.0000, 2.5000, 0.0000, 50.0000, 3.2592, 0.3350, 1.0000],
        [60.2574, -34.0099, 36.2677, 60.4626, -34.1751, 39.4387, 1.2644],
        [63.0109, -31.0961, -5.8663, 62.8187, -29.7946, -4.0864, 1.2630],
        [61.2901, 3.7196, -5.3901, 61.4292, 2.2480, -4.9620, 1.8731],
        [35.0831, -44.1164, 3.7933, 35.0232, -40.0716, 1.5901, 1.8645],
        [22.7233, 20.0904, -46.6940, 23.0331, 14.9730, -42.5619, 2.0373],
        [36.4612, 47.8580, 18.3852, 36.2715, 50.5065, 21.2231, 1.4146],
        [90.8027, -2.0831, 1.4410, 91.1528, -1.6435, 0.0447, 1.4441],
        [90.9257, -0.5406, -0.9208, 88.6381, -0.8985, -0.7239, 1.5381],
        [6.7747, -0.2908, -2.4247, 5.8714, -0.0985, -2.2286, 0.6377],
        [2.0776, 0.0795, -1.1350, 0.9033, -0.0636, -0.5514, 0.9082],
    ];

    #[test]
    fn sharma_2005_dataset() {
        for (i, r) in SHARMA.iter().enumerate() {
            let a = [r[0], r[1], r[2]];
            let b = [r[3], r[4], r[5]];
            let d = ciede2000(a, b);
            // The published values are rounded to 4 decimals.
            assert!((d - r[6]).abs() < 5e-5, "pair {}: got {d}, expected {}", i + 1, r[6]);
            assert!((ciede2000(b, a) - d).abs() < 1e-12, "symmetry pair {}", i + 1);
        }
        assert_eq!(ciede2000([50.0, 10.0, -5.0], [50.0, 10.0, -5.0]), 0.0);
    }

    #[test]
    fn lab_known_values() {
        assert_eq!(xyz_d50_to_lab(D50_XYZ), [100.0, 0.0, 0.0]);
        assert_eq!(xyz_d50_to_lab([0.0; 3]), [0.0, 0.0, 0.0]);
        let white = srgb8_to_lab_d50([255, 255, 255]);
        assert!(white[0] > 99.9999 && white[1].abs() < 1e-9 && white[2].abs() < 1e-9, "{white:?}");
        // sRGB red in Lab D50 (Bradford): ≈ (54.29, 80.80, 69.89) (Lindbloom, D50 Z=0.82521 differs slightly).
        let red = srgb8_to_lab_d50([255, 0, 0]);
        assert!(
            (red[0] - 54.29).abs() < 0.05 && (red[1] - 80.80).abs() < 0.2 && (red[2] - 69.89).abs() < 0.2,
            "{red:?}"
        );
        for lab in [[50.0, 20.0, -30.0], [5.0, 1.0, 1.0], [95.0, -3.0, 8.0], [0.5, 0.1, -0.1]] {
            let back = xyz_d50_to_lab(lab_to_xyz_d50(lab));
            for i in 0..3 {
                assert!((back[i] - lab[i]).abs() < 1e-9, "{lab:?} -> {back:?}");
            }
        }
        assert!((delta_e76([0.0; 3], [3.0, 4.0, 0.0]) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn profile_lab_matches_reference_path() {
        let s = IccProfile::srgb();
        for rgb in [[255u8, 0, 0], [12, 200, 77], [128, 128, 128], [1, 2, 3]] {
            let a = srgb8_to_lab_d50(rgb);
            let b = profile_rgb_to_lab_d50(&s, rgb.map(|v| v as f64 / 255.0));
            // ICC s15Fixed16 quantization of the colorants only.
            assert!(ciede2000(a, b) < 0.01, "{rgb:?}: {a:?} vs {b:?}");
        }
    }
}
