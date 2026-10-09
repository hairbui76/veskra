//! Small deterministic `f64` 3×3 linear algebra and CIE colorimetry helpers.
//!
//! Only IEEE-754 basic operations (`+ - * /`) are used here, so results are bit-identical on
//! every platform Rust targets (Rust never contracts to FMA implicitly). This matters because the
//! generated ICC profiles are derived from these computations.

/// Row-major 3×3 matrix. `m[row][col]`.
pub type Mat3 = [[f64; 3]; 3];
/// Column vector.
pub type Vec3 = [f64; 3];

/// Identity matrix.
pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// ICC profile connection space illuminant D50 (ICC.1 §7.2.16): exactly X=0.9642, Y=1, Z=0.8249.
/// This is also the reference white Veskra uses for CIELAB.
pub const D50_XYZ: Vec3 = [0.9642, 1.0, 0.8249];

/// CIE D65 chromaticity as specified by IEC 61966-2.1 (sRGB) and SMPTE EG 432-1 (Display P3).
pub const D65_XY: [f64; 2] = [0.3127, 0.3290];

/// Linearized Bradford cone response matrix (as used by ICC.1 Annex E and LittleCMS).
pub const BRADFORD: Mat3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

/// `a · b`.
pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    out
}

/// `m · v`.
pub fn mul_vec(m: &Mat3, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Determinant.
pub fn det(m: &Mat3) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Inverse via adjugate. `None` when the matrix is singular (|det| < 1e-12) or non-finite.
pub fn invert(m: &Mat3) -> Option<Mat3> {
    let d = det(m);
    if !d.is_finite() || d.abs() < 1e-12 {
        return None;
    }
    let inv_d = 1.0 / d;
    let out = [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_d,
        ],
    ];
    out.iter().flatten().all(|v| v.is_finite()).then_some(out)
}

/// Diagonal matrix.
pub fn diag(v: Vec3) -> Mat3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

/// Largest absolute element-wise difference.
pub fn max_abs_diff(a: &Mat3, b: &Mat3) -> f64 {
    let mut m: f64 = 0.0;
    for r in 0..3 {
        for c in 0..3 {
            m = m.max((a[r][c] - b[r][c]).abs());
        }
    }
    m
}

/// CIE xy chromaticity → XYZ with Y = 1. `None` if `y` is not positive.
pub fn xy_to_xyz(xy: [f64; 2]) -> Option<Vec3> {
    let [x, y] = xy;
    if y.is_nan() || y <= 0.0 || !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some([x / y, 1.0, (1.0 - x - y) / y])
}

/// Normalized primary matrix (SMPTE RP 177): linear RGB → XYZ for the given primaries and white,
/// with white (1,1,1) → white XYZ (Y = 1).
pub fn rgb_to_xyz_matrix(primaries: [[f64; 2]; 3], white: [f64; 2]) -> Option<Mat3> {
    let r = xy_to_xyz(primaries[0])?;
    let g = xy_to_xyz(primaries[1])?;
    let b = xy_to_xyz(primaries[2])?;
    let w = xy_to_xyz(white)?;
    let p = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let s = mul_vec(&invert(&p)?, w);
    Some(mul(&p, &diag(s)))
}

/// Linear Bradford chromatic adaptation matrix mapping XYZ under `src_white` to `dst_white`.
pub fn bradford(src_white: Vec3, dst_white: Vec3) -> Option<Mat3> {
    let s = mul_vec(&BRADFORD, src_white);
    let d = mul_vec(&BRADFORD, dst_white);
    if s.iter().any(|v| v.abs() < 1e-12) {
        return None;
    }
    let scale = diag([d[0] / s[0], d[1] / s[1], d[2] / s[2]]);
    Some(mul(&invert(&BRADFORD)?, &mul(&scale, &BRADFORD)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_roundtrip() {
        let inv = invert(&BRADFORD).unwrap();
        assert!(max_abs_diff(&mul(&BRADFORD, &inv), &IDENTITY) < 1e-14);
        assert!(invert(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]]).is_none());
    }

    #[test]
    fn srgb_matrix_matches_iec_values() {
        // IEC 61966-2.1 publishes this matrix rounded to 4 decimals.
        let m = rgb_to_xyz_matrix([[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], D65_XY).unwrap();
        let iec = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
        assert!(max_abs_diff(&m, &iec) < 1e-4, "{m:?}");
    }

    #[test]
    fn bradford_maps_white_to_white() {
        let d65 = xy_to_xyz(D65_XY).unwrap();
        let m = bradford(d65, D50_XYZ).unwrap();
        let w = mul_vec(&m, d65);
        for i in 0..3 {
            assert!((w[i] - D50_XYZ[i]).abs() < 1e-12);
        }
        // ICC sRGB v4 `chad` (Bradford D65→ICC D50) to 4 decimals. Note: tables using the
        // CIE D50 (Z = 0.82521) instead of the ICC PCS white differ in m[2][2] by ~3e-4.
        let icc = [[1.0479, 0.0229, -0.0502], [0.0296, 0.9905, -0.0171], [-0.0093, 0.0151, 0.7517]];
        assert!(max_abs_diff(&m, &icc) < 1e-4, "{m:?}");
    }
}
