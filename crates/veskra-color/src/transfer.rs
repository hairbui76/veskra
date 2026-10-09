//! Transfer functions ("TRC"s): the exact sRGB curve, lookup tables and ICC curve types.
//!
//! # Domain conventions
//! All functions take and return normalized values. Inputs are **clamped to `[0, 1]`** and NaN
//! is treated as `0`; Veskra's v1 pipeline never needs extended-range values and clamping keeps
//! every downstream integer conversion well-defined.

use crate::error::ColorError;
use std::sync::{Arc, OnceLock};

#[inline]
fn unit(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// sRGB decode (IEC 61966-2.1 §5.2, exact piecewise form): encoded `[0,1]` → linear `[0,1]`.
#[inline]
pub fn srgb_to_linear_f64(v: f64) -> f64 {
    let v = unit(v);
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// sRGB encode (IEC 61966-2.1 §5.3, exact piecewise form): linear `[0,1]` → encoded `[0,1]`.
#[inline]
pub fn linear_to_srgb_f64(l: f64) -> f64 {
    let l = unit(l);
    if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
}

/// `f32` sRGB decode (evaluated in `f64`, then rounded once to `f32`).
#[inline]
pub fn srgb_to_linear(v: f32) -> f32 {
    srgb_to_linear_f64(v as f64) as f32
}

/// `f32` sRGB encode (evaluated in `f64`, then rounded once to `f32`).
#[inline]
pub fn linear_to_srgb(v: f32) -> f32 {
    linear_to_srgb_f64(v as f64) as f32
}

/// Table `i → srgb_to_linear(i / 255)`, built once.
pub fn srgb_u8_to_linear_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| srgb_to_linear_f64(i as f64 / 255.0) as f32))
}

/// Table `i → srgb_to_linear(i / 65535)`, built once (256 KiB).
pub fn srgb_u16_to_linear_lut() -> &'static [f32; 65536] {
    static LUT: OnceLock<Box<[f32; 65536]>> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = Box::new([0.0f32; 65536]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = srgb_to_linear_f64(i as f64 / 65535.0) as f32;
        }
        t
    })
}

/// Linear `[0,1]` → sRGB-encoded u8, rounding half up in the encoded domain.
#[inline]
pub fn linear_to_srgb_u8(l: f32) -> u8 {
    (linear_to_srgb_f64(l as f64) * 255.0 + 0.5).floor() as u8
}

/// Linear `[0,1]` → sRGB-encoded u16, rounding half up in the encoded domain.
#[inline]
pub fn linear_to_srgb_u16(l: f32) -> u16 {
    (linear_to_srgb_f64(l as f64) * 65535.0 + 0.5).floor() as u16
}

/// ICC `parametricCurveType` (`para`, ICC.1 §10.18), function types 0–4.
///
/// Parameters are stored as `[g, a, b, c, d, e, f]`; unused trailing ones are zero.
///
/// | type | `Y` for `X ≥ threshold` | `Y` below threshold | threshold |
/// |---|---|---|---|
/// | 0 | `X^g` | — | — |
/// | 1 | `(aX+b)^g` | `0` | `-b/a` |
/// | 2 | `(aX+b)^g + c` | `c` | `-b/a` |
/// | 3 | `(aX+b)^g` | `cX` | `d` |
/// | 4 | `(aX+b)^g + e` | `cX + f` | `d` |
///
/// Outputs are clamped to `[0,1]` (ICC curves map into the unit interval).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParametricCurve {
    function_type: u16,
    p: [f64; 7],
}

impl ParametricCurve {
    /// The exact IEC 61966-2.1 sRGB curve expressed as an ICC type-3 parametric curve.
    pub const SRGB: ParametricCurve =
        ParametricCurve { function_type: 3, p: [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045, 0.0, 0.0] };

    /// Number of parameters used by an ICC function type, `None` for unknown types.
    pub const fn param_count(function_type: u16) -> Option<usize> {
        match function_type {
            0 => Some(1),
            1 => Some(3),
            2 => Some(4),
            3 => Some(5),
            4 => Some(7),
            _ => None,
        }
    }

    /// Validate and build. Requires the exact parameter count, finite values, `g > 0` and
    /// `a ≠ 0` for types 1–4 (otherwise the curve is constant/undefined).
    pub fn new(function_type: u16, params: &[f64]) -> Result<Self, ColorError> {
        let bad = |reason| ColorError::InvalidTag { tag: "para".into(), reason };
        let n = Self::param_count(function_type).ok_or_else(|| bad("unknown parametric function type"))?;
        if params.len() != n {
            return Err(bad("wrong parameter count"));
        }
        if params.iter().any(|v| !v.is_finite()) {
            return Err(bad("non-finite parameter"));
        }
        let mut p = [0.0; 7];
        p[..n].copy_from_slice(params);
        if p[0].is_nan() || p[0] <= 0.0 {
            return Err(bad("gamma must be positive"));
        }
        if function_type > 0 && p[1] == 0.0 {
            return Err(bad("parameter a must be non-zero"));
        }
        Ok(Self { function_type, p })
    }

    /// ICC function type (0–4).
    pub fn function_type(&self) -> u16 {
        self.function_type
    }

    /// The used parameters `[g, a, b, c, d, e, f][..param_count]`.
    pub fn params(&self) -> &[f64] {
        let n = Self::param_count(self.function_type).unwrap_or(1);
        &self.p[..n]
    }

    /// Evaluate the curve at `x` (clamped to `[0,1]`).
    pub fn eval(&self, x: f64) -> f64 {
        let x = unit(x);
        let [g, a, b, c, d, e, f] = self.p;
        let pw = |base: f64| if base > 0.0 { base.powf(g) } else { 0.0 };
        let y = match self.function_type {
            0 => pw(x),
            1 => {
                if x >= -b / a {
                    pw(a * x + b)
                } else {
                    0.0
                }
            }
            2 => {
                if x >= -b / a {
                    pw(a * x + b) + c
                } else {
                    c
                }
            }
            3 => {
                if x >= d {
                    pw(a * x + b)
                } else {
                    c * x
                }
            }
            _ => {
                if x >= d {
                    pw(a * x + b) + e
                } else {
                    c * x + f
                }
            }
        };
        unit(y)
    }

    /// Analytic inverse: the `x` with `eval(x) ≈ y`, clamped to `[0,1]`. For flat regions
    /// (e.g. type 1 below `-b/a`) the threshold is returned. Matches LittleCMS's reversed
    /// parametric types (-1 … -4) for well-formed curves.
    pub fn eval_inverse(&self, y: f64) -> f64 {
        let y = unit(y);
        let [g, a, b, c, d, e, f] = self.p;
        let root = |v: f64| if v > 0.0 { v.powf(1.0 / g) } else { 0.0 };
        let x = match self.function_type {
            0 => root(y),
            1 => {
                if y > 0.0 {
                    (root(y) - b) / a
                } else {
                    -b / a
                }
            }
            2 => {
                if y > c {
                    (root(y - c) - b) / a
                } else {
                    -b / a
                }
            }
            3 => {
                let disc = if a * d + b > 0.0 { (a * d + b).powf(g) } else { 0.0 };
                if y >= disc {
                    (root(y) - b) / a
                } else if c != 0.0 {
                    y / c
                } else {
                    0.0
                }
            }
            _ => {
                let disc = if a * d + b > 0.0 { (a * d + b).powf(g) } else { 0.0 } + e;
                if y >= disc {
                    (root(y - e) - b) / a
                } else if c != 0.0 {
                    (y - f) / c
                } else {
                    0.0
                }
            }
        };
        unit(x)
    }
}

/// Monotonicity class of a sampled table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Monotonicity {
    /// Non-decreasing (the normal case).
    NonDecreasing,
    /// Non-increasing (inverted curves; usable as source only).
    NonIncreasing,
    /// Neither (usable as source only).
    NonMonotonic,
}

/// Sampled ICC `curv` table (≥ 2 entries, u16 normalized by 65535, linear interpolation).
#[derive(Debug, Clone, PartialEq)]
pub struct TableCurve {
    entries: Arc<[u16]>,
    monotonicity: Monotonicity,
}

impl TableCurve {
    /// Build from raw entries. Errors when fewer than 2 entries are given.
    pub fn new(entries: Vec<u16>) -> Result<Self, ColorError> {
        if entries.len() < 2 {
            return Err(ColorError::InvalidTag { tag: "curv".into(), reason: "table needs at least 2 entries" });
        }
        let inc = entries.windows(2).all(|w| w[0] <= w[1]);
        let dec = entries.windows(2).all(|w| w[0] >= w[1]);
        let monotonicity = match (inc, dec) {
            (true, _) => Monotonicity::NonDecreasing,
            (false, true) => Monotonicity::NonIncreasing,
            _ => Monotonicity::NonMonotonic,
        };
        Ok(Self { entries: entries.into(), monotonicity })
    }

    /// Raw entries.
    pub fn entries(&self) -> &[u16] {
        &self.entries
    }

    /// Monotonicity of the table.
    pub fn monotonicity(&self) -> Monotonicity {
        self.monotonicity
    }

    fn at(&self, i: usize) -> f64 {
        self.entries.get(i).copied().unwrap_or(0) as f64 / 65535.0
    }

    /// Piecewise-linear evaluation.
    pub fn eval(&self, x: f64) -> f64 {
        let n = self.entries.len();
        let pos = unit(x) * (n - 1) as f64;
        let i = (pos.floor() as usize).min(n - 2);
        let t = pos - i as f64;
        self.at(i) + (self.at(i + 1) - self.at(i)) * t
    }

    /// Exact inverse of the piecewise-linear evaluation for non-decreasing tables (the lowest `x`
    /// that maps to `y`). For other tables the first crossing segment is used (best effort).
    pub fn eval_inverse(&self, y: f64) -> f64 {
        let y = unit(y);
        let n = self.entries.len();
        let last = (n - 1) as f64;
        if self.monotonicity == Monotonicity::NonDecreasing {
            // First index whose value is >= y.
            let k = self.entries.partition_point(|&e| (e as f64 / 65535.0) < y);
            if k == 0 {
                return 0.0;
            }
            if k >= n {
                return 1.0;
            }
            let (y0, y1) = (self.at(k - 1), self.at(k));
            let t = if y1 > y0 { (y - y0) / (y1 - y0) } else { 1.0 };
            return unit(((k - 1) as f64 + t) / last);
        }
        for i in 0..n - 1 {
            let (y0, y1) = (self.at(i), self.at(i + 1));
            if (y0 <= y && y <= y1) || (y1 <= y && y <= y0) {
                let t = if y1 != y0 { (y - y0) / (y1 - y0) } else { 0.0 };
                return unit((i as f64 + t) / last);
            }
        }
        0.0
    }
}

/// Any ICC tone reproduction curve supported for matrix/TRC profiles.
#[derive(Debug, Clone, PartialEq)]
pub enum ToneCurve {
    /// `curv` with zero entries.
    Identity,
    /// `curv` with one entry (u8Fixed8 gamma), `Y = X^g`, `g > 0`.
    Gamma(f64),
    /// `curv` with ≥ 2 entries.
    Table(TableCurve),
    /// `para`.
    Parametric(ParametricCurve),
}

impl ToneCurve {
    /// Device (encoded) value → linear (PCS-relative) value.
    pub fn eval(&self, x: f64) -> f64 {
        match self {
            ToneCurve::Identity => unit(x),
            ToneCurve::Gamma(g) => unit(x).powf(*g),
            ToneCurve::Table(t) => t.eval(x),
            ToneCurve::Parametric(p) => p.eval(x),
        }
    }

    /// Linear → device (encoded) value. Exact for identity/gamma/parametric curves and for
    /// non-decreasing tables (inverse of the linear interpolation).
    pub fn eval_inverse(&self, y: f64) -> f64 {
        match self {
            ToneCurve::Identity => unit(y),
            ToneCurve::Gamma(g) => unit(y).powf(1.0 / *g),
            ToneCurve::Table(t) => t.eval_inverse(y),
            ToneCurve::Parametric(p) => p.eval_inverse(y),
        }
    }

    /// Whether the curve can be used on the output side of a transform: non-decreasing and
    /// with a non-empty output range (`eval(1) > eval(0)`).
    pub fn is_invertible(&self) -> bool {
        let monotone = match self {
            ToneCurve::Identity => true,
            ToneCurve::Gamma(g) => *g > 0.0,
            ToneCurve::Table(t) => t.monotonicity() == Monotonicity::NonDecreasing,
            ToneCurve::Parametric(_) => {
                let mut prev = self.eval(0.0);
                (1..=1024).all(|i| {
                    let v = self.eval(i as f64 / 1024.0);
                    let ok = v >= prev;
                    prev = v;
                    ok
                })
            }
        };
        monotone && self.eval(1.0) > self.eval(0.0)
    }

    /// Largest encoded-domain error when values are decoded with `self` and re-encoded with
    /// `other` (`max |other⁻¹(self(x)) − x|` over 1025 evenly spaced samples, i.e. 4× denser
    /// than 8-bit codes; both curves are smooth or piecewise linear between samples).
    pub fn encoded_roundtrip_error(&self, other: &ToneCurve) -> f64 {
        (0..=1024)
            .map(|i| {
                let x = i as f64 / 1024.0;
                (other.eval_inverse(self.eval(x)) - x).abs()
            })
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_known_values() {
        assert_eq!(srgb_to_linear_f64(0.0), 0.0);
        assert_eq!(srgb_to_linear_f64(1.0), 1.0);
        assert!((srgb_to_linear_f64(0.5) - 0.214_041_140_482_232_5).abs() < 1e-15);
        assert!((srgb_to_linear_f64(0.04045) - 0.04045 / 12.92).abs() < 1e-18);
        assert!((linear_to_srgb_f64(0.214_041_140_482_232_5) - 0.5).abs() < 1e-14);
        assert_eq!(srgb_to_linear_f64(f64::NAN), 0.0);
        assert_eq!(srgb_to_linear_f64(2.0), 1.0);
        assert_eq!(linear_to_srgb(-1.0), 0.0);
    }

    #[test]
    fn srgb_roundtrip_u8_u16_exact() {
        let l8 = srgb_u8_to_linear_lut();
        for (i, &l) in l8.iter().enumerate() {
            assert_eq!(linear_to_srgb_u8(l) as usize, i);
        }
        let l16 = srgb_u16_to_linear_lut();
        for (i, &l) in l16.iter().enumerate() {
            // f32 linear storage cannot resolve every 16-bit code near white: ≤ 1 code.
            let back = linear_to_srgb_u16(l) as i64;
            assert!((back - i as i64).abs() <= 1, "{i} -> {back}");
        }
        assert_eq!(l16[0], 0.0);
        assert_eq!(l16[65535], 1.0);
    }

    #[test]
    fn parametric_srgb_matches_formula() {
        let p = ParametricCurve::SRGB;
        for i in 0..=1000 {
            let x = i as f64 / 1000.0;
            assert!((p.eval(x) - srgb_to_linear_f64(x)).abs() < 1e-12);
            assert!((p.eval_inverse(p.eval(x)) - x).abs() < 1e-9);
        }
    }

    #[test]
    fn parametric_all_types_invert() {
        let curves = [
            ParametricCurve::new(0, &[2.2]).unwrap(),
            ParametricCurve::new(1, &[2.2, 1.1, -0.1]).unwrap(),
            ParametricCurve::new(2, &[2.0, 1.2, -0.2, 0.05]).unwrap(),
            ParametricCurve::new(3, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]).unwrap(),
            ParametricCurve::new(4, &[2.2, 0.9, 0.1, 0.2, 0.1, 0.0, 0.005]).unwrap(),
        ];
        for (k, p) in curves.iter().enumerate() {
            let tc = ToneCurve::Parametric(*p);
            assert!(tc.is_invertible(), "type {k}");
            for i in 0..=512 {
                let x = i as f64 / 512.0;
                let y = p.eval(x);
                let back = p.eval_inverse(y);
                // Flat regions invert to the threshold; compare in the output domain.
                assert!((p.eval(back) - y).abs() < 1e-9, "type {k} x={x} y={y} back={back}");
            }
        }
        // Discontinuous, decreasing jump at d: not usable as a destination curve.
        let jump = ParametricCurve::new(4, &[2.2, 0.9, 0.1, 0.5, 0.1, 0.0, 0.01]).unwrap();
        assert!(!ToneCurve::Parametric(jump).is_invertible());
        assert!(ParametricCurve::new(5, &[1.0]).is_err());
        assert!(ParametricCurve::new(0, &[0.0]).is_err());
        assert!(ParametricCurve::new(1, &[2.2, 0.0, 0.0]).is_err());
        assert!(ParametricCurve::new(3, &[2.2, 1.0]).is_err());
        assert!(ParametricCurve::new(0, &[f64::NAN]).is_err());
    }

    #[test]
    fn table_curve_inverse_is_exact() {
        let t = TableCurve::new(
            (0..1024).map(|i| (srgb_to_linear_f64(i as f64 / 1023.0) * 65535.0).round() as u16).collect(),
        )
        .unwrap();
        assert_eq!(t.monotonicity(), Monotonicity::NonDecreasing);
        for i in 0..=2000 {
            let x = i as f64 / 2000.0;
            assert!((t.eval_inverse(t.eval(x)) - x).abs() < 1e-9 || t.eval(x) == 0.0);
        }
        let tc = ToneCurve::Table(t);
        assert!(tc.encoded_roundtrip_error(&ToneCurve::Parametric(ParametricCurve::SRGB)) < 2e-4);
        let dec = TableCurve::new(vec![65535, 0]).unwrap();
        assert_eq!(dec.monotonicity(), Monotonicity::NonIncreasing);
        assert!((dec.eval_inverse(0.25) - 0.75).abs() < 1e-12);
        assert!(!ToneCurve::Table(dec).is_invertible());
        assert!(TableCurve::new(vec![1]).is_err());
        let flat = ToneCurve::Table(TableCurve::new(vec![100, 100]).unwrap());
        assert!(!flat.is_invertible());
    }

    #[test]
    fn gamma_vs_srgb_error_is_visible() {
        let g = ToneCurve::Gamma(2.2);
        let s = ToneCurve::Parametric(ParametricCurve::SRGB);
        assert!(g.encoded_roundtrip_error(&s) > 4e-3);
        assert!(s.encoded_roundtrip_error(&s) < 1e-12);
    }
}
