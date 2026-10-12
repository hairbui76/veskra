//! ICC LUT-based transforms (ADR-018): `lut8Type` (`mft1`), `lut16Type` (`mft2`),
//! `lutAToBType` (`mAB `) and `lutBToAType` (`mBA `), parsed with strict bounds and evaluated in
//! `f64` the way LittleCMS evaluates its floating-point pipeline.
//!
//! # Stages
//! * `mft1` / `mft2`: optional 3×3 matrix (only for 3 input channels and when not identity, as
//!   LittleCMS does), input curves (8-bit or 16-bit tables, piecewise linear), CLUT (uniform grid),
//!   output curves.
//! * `mAB ` (device → PCS): A curves, CLUT, M curves, matrix + offset (3 channels), B curves.
//! * `mBA ` (PCS → device): B curves, matrix + offset, M curves, CLUT, A curves.
//!   Curves are `curv` (identity, gamma or table) or `para` (types 0–4); every element is optional
//!   as in LittleCMS, but the channel counts of consecutive stages must agree.
//!
//! # Interpolation
//! Matching LittleCMS: 1 input → linear, 2 → bilinear, 3 → tetrahedral (the six tetrahedra around
//! the cube diagonal), 4 → linear along the **first** input between two tetrahedral evaluations of
//! the other three. Inputs are clamped to `[0, 1]` (NaN → 0). More than 4 CLUT inputs is rejected.
//!
//! # Encodings
//! Values between stages are normalized to `[0, 1]` like LittleCMS's float pipeline. The caller
//! ([`crate::cmyk`]) maps PCS values to and from that normalization and applies the legacy
//! 16-bit Lab encoding of `lut16Type` (L* = 100 at `0xFF00`).
//!
//! # Robustness
//! Every offset, count and table size is checked with `u64`/checked arithmetic against the tag
//! size before anything is read; CLUT data is **not copied** (it is read in place from the shared
//! profile bytes), so the only allocations are curve tables (≤ 15 × 4096 entries for `mft2`, ≤
//! [`MAX_CURVE_ENTRIES`](super::MAX_CURVE_ENTRIES) per `curv` element).

use super::{be_u16, be_u32, parse_curve_tag, s15f16, sig, slice, tag_err};
use crate::error::ColorError;
use crate::transfer::{ParametricCurve, TableCurve, ToneCurve};
use std::sync::Arc;

/// Most channels an ICC LUT may declare.
pub const MAX_LUT_CHANNELS: usize = 15;
/// Most CLUT input dimensions Veskra evaluates (gray, RGB and CMYK devices).
pub const MAX_CLUT_INPUTS: usize = 4;
/// Most entries per `mft2` input/output table (ICC.1 limit).
pub const MAX_LUT16_TABLE_ENTRIES: usize = 4096;

/// Profile bytes shared without copying by CLUT stages (`'static` for the bundled profile).
#[derive(Clone)]
pub(crate) enum SharedBytes {
    Static(&'static [u8]),
    Owned(Arc<[u8]>),
}

impl SharedBytes {
    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            SharedBytes::Static(s) => s,
            SharedBytes::Owned(a) => a,
        }
    }
}

/// Tag type a pipeline was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LutType {
    /// `mft1`
    Lut8,
    /// `mft2`
    Lut16,
    /// `mAB `
    AToB,
    /// `mBA `
    BToA,
}

/// Multidimensional lookup table read in place from the profile bytes.
#[derive(Clone)]
pub(crate) struct Clut {
    data: SharedBytes,
    /// Byte offset of the first entry.
    start: usize,
    /// 16-bit entries (else 8-bit).
    wide: bool,
    inputs: usize,
    outputs: usize,
    grid: [usize; MAX_CLUT_INPUTS],
    /// Entries (not bytes) per step along each input dimension; the last input has stride `outputs`.
    stride: [usize; MAX_CLUT_INPUTS],
}

impl Clut {
    fn new(
        tag: [u8; 4],
        data: &SharedBytes,
        start: usize,
        available: usize,
        wide: bool,
        grid_points: &[usize],
        outputs: usize,
    ) -> Result<Clut, ColorError> {
        let inputs = grid_points.len();
        if inputs == 0 || inputs > MAX_CLUT_INPUTS {
            return Err(tag_err(tag, "CLUT with more than 4 inputs is not supported"));
        }
        if outputs == 0 || outputs > MAX_LUT_CHANNELS {
            return Err(tag_err(tag, "CLUT output channel count out of range"));
        }
        let mut grid = [0usize; MAX_CLUT_INPUTS];
        let mut entries: u64 = outputs as u64;
        for (i, &g) in grid_points.iter().enumerate() {
            if !(2..=255).contains(&g) {
                return Err(tag_err(tag, "CLUT grid points must be 2..=255"));
            }
            grid[i] = g;
            entries = entries.checked_mul(g as u64).ok_or_else(|| tag_err(tag, "CLUT size overflow"))?;
        }
        let bytes = entries.checked_mul(if wide { 2 } else { 1 }).ok_or_else(|| tag_err(tag, "CLUT size overflow"))?;
        if bytes > available as u64 {
            return Err(tag_err(tag, "CLUT exceeds tag size"));
        }
        let mut stride = [0usize; MAX_CLUT_INPUTS];
        let mut s = outputs;
        for d in (0..inputs).rev() {
            stride[d] = s;
            s *= grid[d];
        }
        Ok(Clut { data: data.clone(), start, wide, inputs, outputs, grid, stride })
    }

    #[inline]
    fn node(&self, bytes: &[u8], idx: usize) -> f64 {
        if self.wide {
            let o = self.start + 2 * idx;
            match bytes.get(o..o + 2) {
                Some(b) => u16::from_be_bytes([b[0], b[1]]) as f64 / 65535.0,
                None => 0.0,
            }
        } else {
            bytes.get(self.start + idx).map_or(0.0, |&b| b as f64 / 255.0)
        }
    }

    /// Cell index and fraction along dimension `d`.
    #[inline]
    fn locate(&self, d: usize, v: f64) -> (usize, f64) {
        let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        let n = self.grid[d];
        let p = v * (n - 1) as f64;
        let i = (p.floor() as usize).min(n - 2);
        (i, p - i as f64)
    }

    /// Tetrahedral interpolation over dimensions `d..d+3` from entry offset `base`.
    fn tetra(&self, bytes: &[u8], base: usize, d: usize, inp: &[f64], out: &mut [f64]) {
        let (x0, rx) = self.locate(d, inp[0]);
        let (y0, ry) = self.locate(d + 1, inp[1]);
        let (z0, rz) = self.locate(d + 2, inp[2]);
        let (sx, sy, sz) = (self.stride[d], self.stride[d + 1], self.stride[d + 2]);
        let o = base + x0 * sx + y0 * sy + z0 * sz;
        // Vertices along the path of the tetrahedron that contains the point (Kuhn split along
        // the main diagonal), ordered by the fractional coordinates.
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
        let s3 = sx + sy + sz;
        for (k, v) in out.iter_mut().enumerate().take(self.outputs) {
            let c0 = self.node(bytes, o + k);
            let c1 = self.node(bytes, o + s1 + k);
            let c2 = self.node(bytes, o + s2 + k);
            let c3 = self.node(bytes, o + s3 + k);
            *v = c0 + (c1 - c0) * f1 + (c2 - c1) * f2 + (c3 - c2) * f3;
        }
    }

    fn eval(&self, inp: &[f64], out: &mut [f64]) {
        let bytes = self.data.as_slice();
        match self.inputs {
            1 => {
                let (i, f) = self.locate(0, inp[0]);
                let o = i * self.stride[0];
                for (k, v) in out.iter_mut().enumerate().take(self.outputs) {
                    let a = self.node(bytes, o + k);
                    let b = self.node(bytes, o + self.stride[0] + k);
                    *v = a + (b - a) * f;
                }
            }
            2 => {
                let (x, fx) = self.locate(0, inp[0]);
                let (y, fy) = self.locate(1, inp[1]);
                let (sx, sy) = (self.stride[0], self.stride[1]);
                let o = x * sx + y * sy;
                for (k, v) in out.iter_mut().enumerate().take(self.outputs) {
                    let n = |off: usize| self.node(bytes, o + off + k);
                    let a = n(0) + (n(sy) - n(0)) * fy;
                    let b = n(sx) + (n(sx + sy) - n(sx)) * fy;
                    *v = a + (b - a) * fx;
                }
            }
            3 => self.tetra(bytes, 0, 0, inp, out),
            _ => {
                // LittleCMS `Eval4Inputs`: tetrahedral on inputs 1..3, linear along input 0.
                let (k0, rk) = self.locate(0, inp[0]);
                let mut lo = [0.0f64; MAX_LUT_CHANNELS];
                let mut hi = [0.0f64; MAX_LUT_CHANNELS];
                self.tetra(bytes, k0 * self.stride[0], 1, &inp[1..4], &mut lo);
                self.tetra(bytes, (k0 + 1) * self.stride[0], 1, &inp[1..4], &mut hi);
                for (k, v) in out.iter_mut().enumerate().take(self.outputs) {
                    *v = lo[k] + (hi[k] - lo[k]) * rk;
                }
            }
        }
    }
}

/// One processing element.
#[derive(Clone)]
pub(crate) enum Stage {
    Curves(Vec<ToneCurve>),
    Matrix {
        m: [[f64; 3]; 3],
        off: [f64; 3],
    },
    Clut(Clut),
    /// Multiply every channel (Lab legacy ↔ v4 16-bit encoding adjustments).
    Scale(f64),
}

/// A parsed LUT tag: a chain of stages from `inputs` to `outputs` normalized channels.
#[derive(Clone)]
pub struct LutPipeline {
    inputs: usize,
    outputs: usize,
    kind: LutType,
    pub(crate) stages: Vec<Stage>,
}

impl std::fmt::Debug for LutPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LutPipeline")
            .field("kind", &self.kind)
            .field("inputs", &self.inputs)
            .field("outputs", &self.outputs)
            .field("stages", &self.stages.len())
            .finish()
    }
}

impl LutPipeline {
    /// Number of input channels.
    pub fn inputs(&self) -> usize {
        self.inputs
    }

    /// Number of output channels.
    pub fn outputs(&self) -> usize {
        self.outputs
    }

    /// Tag type the pipeline was read from.
    pub fn kind(&self) -> LutType {
        self.kind
    }

    /// Prepend a scale stage (input encoding adjustment).
    pub(crate) fn scale_input(&mut self, k: f64) {
        self.stages.insert(0, Stage::Scale(k));
    }

    /// Append a scale stage (output encoding adjustment).
    pub(crate) fn scale_output(&mut self, k: f64) {
        self.stages.push(Stage::Scale(k));
    }

    /// Evaluate normalized inputs. `input.len() ≥ inputs()`, `out.len() ≥ outputs()`. Curves and
    /// CLUT stages clamp their inputs to `[0, 1]`; the final output is not clamped (PCS values may
    /// slightly exceed the unit range after encoding adjustments).
    pub fn eval(&self, input: &[f64], out: &mut [f64]) {
        let mut buf = [0.0f64; MAX_LUT_CHANNELS];
        let mut tmp = [0.0f64; MAX_LUT_CHANNELS];
        let n_in = self.inputs.min(input.len());
        buf[..n_in].copy_from_slice(&input[..n_in]);
        let mut n = self.inputs;
        for st in &self.stages {
            match st {
                Stage::Curves(cs) => {
                    for (v, c) in buf.iter_mut().zip(cs) {
                        *v = c.eval(*v);
                    }
                }
                Stage::Matrix { m, off } => {
                    let v = [buf[0], buf[1], buf[2]];
                    for r in 0..3 {
                        buf[r] = m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + off[r];
                    }
                }
                Stage::Clut(c) => {
                    c.eval(&buf[..c.inputs], &mut tmp);
                    buf[..c.outputs].copy_from_slice(&tmp[..c.outputs]);
                    n = c.outputs;
                }
                Stage::Scale(k) => {
                    for v in buf.iter_mut().take(n) {
                        *v *= k;
                    }
                }
            }
        }
        let n_out = self.outputs.min(out.len());
        out[..n_out].copy_from_slice(&buf[..n_out]);
    }
}

/// A table of `entries` (normalized by 65535) as a curve; exact linear tables become identity.
fn table_curve(tag: [u8; 4], entries: Vec<u16>) -> Result<ToneCurve, ColorError> {
    let n = entries.len();
    let linear = n >= 2
        && entries.iter().enumerate().all(|(i, &e)| {
            let ideal = (i as f64 * 65535.0 / (n - 1) as f64).round();
            (e as f64 - ideal).abs() < 0.5
        });
    if linear {
        return Ok(ToneCurve::Identity);
    }
    TableCurve::new(entries).map(ToneCurve::Table).map_err(|_| tag_err(tag, "curve table needs at least 2 entries"))
}

fn identity_matrix(m: &[[f64; 3]; 3]) -> bool {
    (0..3).all(|r| (0..3).all(|c| m[r][c] == if r == c { 1.0 } else { 0.0 }))
}

fn read_matrix(tag: [u8; 4], d: &[u8], off: usize, with_offset: bool) -> Result<Stage, ColorError> {
    let need = if with_offset { 48 } else { 36 };
    if slice(d, off, need).is_none() {
        return Err(tag_err(tag, "matrix exceeds tag size"));
    }
    let mut m = [[0.0; 3]; 3];
    for (i, v) in m.iter_mut().flatten().enumerate() {
        *v = s15f16(d, off + 4 * i).ok_or_else(|| tag_err(tag, "matrix exceeds tag size"))?;
    }
    let mut o = [0.0; 3];
    if with_offset {
        for (i, v) in o.iter_mut().enumerate() {
            *v = s15f16(d, off + 36 + 4 * i).ok_or_else(|| tag_err(tag, "matrix exceeds tag size"))?;
        }
    }
    Ok(Stage::Matrix { m, off: o })
}

/// `mft1` / `mft2`.
fn parse_lut8_16(
    tag: [u8; 4],
    data: &SharedBytes,
    tag_off: usize,
    d: &[u8],
    wide: bool,
    inputs: usize,
    outputs: usize,
) -> Result<LutPipeline, ColorError> {
    let (Some(&ic), Some(&oc), Some(&g)) = (d.get(8), d.get(9), d.get(10)) else {
        return Err(tag_err(tag, "LUT header truncated"));
    };
    let (ic, oc, g) = (ic as usize, oc as usize, g as usize);
    if ic != inputs || oc != outputs {
        return Err(tag_err(tag, "LUT channel counts do not match the profile color spaces"));
    }
    let Stage::Matrix { m, .. } = read_matrix(tag, d, 12, false)? else {
        return Err(tag_err(tag, "matrix"));
    };
    let (n_in, n_out, tables) = if wide {
        let n = be_u16(d, 48).ok_or_else(|| tag_err(tag, "LUT header truncated"))? as usize;
        let k = be_u16(d, 50).ok_or_else(|| tag_err(tag, "LUT header truncated"))? as usize;
        (n, k, 52usize)
    } else {
        (256, 256, 48usize)
    };
    for n in [n_in, n_out] {
        if n == 1 || n > MAX_LUT16_TABLE_ENTRIES {
            return Err(tag_err(tag, "LUT table entry count out of range"));
        }
    }
    let es: u64 = if wide { 2 } else { 1 };
    let in_bytes = (ic as u64) * (n_in as u64) * es;
    let clut_off = tables as u64 + in_bytes;
    let mut clut_bytes = es * oc as u64;
    for _ in 0..ic {
        clut_bytes = clut_bytes.checked_mul(g as u64).ok_or_else(|| tag_err(tag, "CLUT size overflow"))?;
    }
    let out_off = clut_off.checked_add(clut_bytes).ok_or_else(|| tag_err(tag, "CLUT size overflow"))?;
    let end = out_off + (oc as u64) * (n_out as u64) * es;
    if end > d.len() as u64 {
        return Err(tag_err(tag, "LUT tables exceed tag size"));
    }
    let read_tables = |at: usize, ch: usize, n: usize| -> Result<Vec<ToneCurve>, ColorError> {
        let mut curves = Vec::with_capacity(ch);
        for c in 0..ch {
            let mut e = Vec::with_capacity(n);
            for i in 0..n {
                let v = if wide {
                    be_u16(d, at + 2 * (c * n + i)).ok_or_else(|| tag_err(tag, "LUT table truncated"))?
                } else {
                    let b = *d.get(at + c * n + i).ok_or_else(|| tag_err(tag, "LUT table truncated"))?;
                    b as u16 * 257
                };
                e.push(v);
            }
            curves.push(table_curve(tag, e)?);
        }
        Ok(curves)
    };
    let mut stages = Vec::with_capacity(4);
    if ic == 3 && !identity_matrix(&m) {
        stages.push(Stage::Matrix { m, off: [0.0; 3] });
    }
    if n_in > 0 {
        stages.push(Stage::Curves(read_tables(tables, ic, n_in)?));
    }
    let grid = vec![g; ic];
    stages.push(Stage::Clut(Clut::new(tag, data, tag_off + clut_off as usize, clut_bytes as usize, wide, &grid, oc)?));
    if n_out > 0 {
        stages.push(Stage::Curves(read_tables(out_off as usize, oc, n_out)?));
    }
    Ok(LutPipeline { inputs, outputs, kind: if wide { LutType::Lut16 } else { LutType::Lut8 }, stages })
}

/// Byte length of the `curv` / `para` element at `pos` (without padding).
fn curve_element_len(tag: [u8; 4], d: &[u8], pos: usize) -> Result<usize, ColorError> {
    match sig(d, pos) {
        Some(t) if &t == b"curv" => {
            let count = be_u32(d, pos + 8).ok_or_else(|| tag_err(tag, "curve truncated"))? as usize;
            if count > super::MAX_CURVE_ENTRIES {
                return Err(tag_err(tag, "curv table too large"));
            }
            Ok(12 + 2 * count)
        }
        Some(t) if &t == b"para" => {
            let ft = be_u16(d, pos + 8).ok_or_else(|| tag_err(tag, "curve truncated"))?;
            let n = ParametricCurve::param_count(ft).ok_or_else(|| tag_err(tag, "unknown para function type"))?;
            Ok(12 + 4 * n)
        }
        _ => Err(tag_err(tag, "LUT curve must be curv or para")),
    }
}

fn read_curve_set(tag: [u8; 4], d: &[u8], start: usize, count: usize) -> Result<Vec<ToneCurve>, ColorError> {
    let mut pos = start;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len = curve_element_len(tag, d, pos)?;
        let el = slice(d, pos, len).ok_or_else(|| tag_err(tag, "curve exceeds tag size"))?;
        out.push(parse_curve_tag(tag, el)?);
        pos = pos.checked_add(len.div_ceil(4) * 4).ok_or_else(|| tag_err(tag, "curve offset overflow"))?;
    }
    Ok(out)
}

/// `mAB ` / `mBA `.
fn parse_lut_ab(
    tag: [u8; 4],
    data: &SharedBytes,
    tag_off: usize,
    d: &[u8],
    a_to_b: bool,
    inputs: usize,
    outputs: usize,
) -> Result<LutPipeline, ColorError> {
    let (Some(&ic), Some(&oc)) = (d.get(8), d.get(9)) else {
        return Err(tag_err(tag, "LUT header truncated"));
    };
    let (ic, oc) = (ic as usize, oc as usize);
    if ic != inputs || oc != outputs {
        return Err(tag_err(tag, "LUT channel counts do not match the profile color spaces"));
    }
    let mut offs = [0usize; 5]; // B, matrix, M, CLUT, A
    for (i, o) in offs.iter_mut().enumerate() {
        let v = be_u32(d, 12 + 4 * i).ok_or_else(|| tag_err(tag, "LUT header truncated"))? as usize;
        if v != 0 && (v < 32 || v >= d.len()) {
            return Err(tag_err(tag, "LUT element offset out of bounds"));
        }
        *o = v;
    }
    let [b_off, mat_off, m_off, clut_off, a_off] = offs;
    let mut stages = Vec::with_capacity(5);
    let mut ch = ic;
    let curves = |off: usize, n: usize, ch: usize| -> Result<Stage, ColorError> {
        if n != ch {
            return Err(tag_err(tag, "LUT curve set does not match the channel count"));
        }
        Ok(Stage::Curves(read_curve_set(tag, d, off, n)?))
    };
    let clut = |ch: usize| -> Result<Stage, ColorError> {
        if ch > MAX_CLUT_INPUTS {
            return Err(tag_err(tag, "CLUT with more than 4 inputs is not supported"));
        }
        let head = slice(d, clut_off, 20).ok_or_else(|| tag_err(tag, "CLUT header exceeds tag size"))?;
        let grid: Vec<usize> = head[..ch].iter().map(|&g| g as usize).collect();
        let wide = match head[16] {
            1 => false,
            2 => true,
            _ => return Err(tag_err(tag, "CLUT precision must be 1 or 2 bytes")),
        };
        let start = clut_off + 20;
        Ok(Stage::Clut(Clut::new(tag, data, tag_off + start, d.len() - start, wide, &grid, oc)?))
    };
    let matrix = |ch: usize| -> Result<Stage, ColorError> {
        if ch != 3 {
            return Err(tag_err(tag, "LUT matrix needs 3 channels"));
        }
        read_matrix(tag, d, mat_off, true)
    };
    if a_to_b {
        if a_off != 0 {
            stages.push(curves(a_off, ic, ch)?);
        }
        if clut_off != 0 {
            stages.push(clut(ch)?);
            ch = oc;
        }
        if m_off != 0 {
            stages.push(curves(m_off, oc, ch)?);
        }
        if mat_off != 0 {
            stages.push(matrix(ch)?);
        }
        if b_off != 0 {
            stages.push(curves(b_off, oc, ch)?);
        }
    } else {
        if b_off != 0 {
            stages.push(curves(b_off, ic, ch)?);
        }
        if mat_off != 0 {
            stages.push(matrix(ch)?);
        }
        if m_off != 0 {
            stages.push(curves(m_off, ic, ch)?);
        }
        if clut_off != 0 {
            stages.push(clut(ch)?);
            ch = oc;
        }
        if a_off != 0 {
            stages.push(curves(a_off, oc, ch)?);
        }
    }
    if ch != oc {
        return Err(tag_err(tag, "LUT without CLUT cannot change the channel count"));
    }
    Ok(LutPipeline { inputs, outputs, kind: if a_to_b { LutType::AToB } else { LutType::BToA }, stages })
}

/// Parse the LUT tag at `(off, len)` of `data`. `a_to_b` selects the device → PCS direction
/// (`A2Bx`: `mft1`, `mft2`, `mAB `) or PCS → device (`B2Ax`: `mft1`, `mft2`, `mBA `); the tag must
/// declare exactly `inputs` → `outputs` channels.
pub(crate) fn parse_lut(
    tag: [u8; 4],
    data: &SharedBytes,
    off: usize,
    len: usize,
    a_to_b: bool,
    inputs: usize,
    outputs: usize,
) -> Result<LutPipeline, ColorError> {
    if inputs == 0 || inputs > MAX_LUT_CHANNELS || outputs == 0 || outputs > MAX_LUT_CHANNELS {
        return Err(tag_err(tag, "LUT channel count out of range"));
    }
    let d = slice(data.as_slice(), off, len).ok_or_else(|| tag_err(tag, "tag data out of bounds"))?;
    match sig(d, 0) {
        Some(t) if &t == b"mft2" => parse_lut8_16(tag, data, off, d, true, inputs, outputs),
        Some(t) if &t == b"mft1" => parse_lut8_16(tag, data, off, d, false, inputs, outputs),
        Some(t) if &t == b"mAB " && a_to_b => parse_lut_ab(tag, data, off, d, true, inputs, outputs),
        Some(t) if &t == b"mBA " && !a_to_b => parse_lut_ab(tag, data, off, d, false, inputs, outputs),
        _ => Err(tag_err(tag, "unsupported LUT tag type (expected mft1, mft2, mAB or mBA)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(v: Vec<u8>) -> SharedBytes {
        SharedBytes::Owned(Arc::from(v))
    }

    /// `mft2` 3 → 1 with identity tables and a 2³ CLUT holding `x + 2y + 4z` (scaled).
    fn mft2_3to1() -> Vec<u8> {
        let mut d = b"mft2\0\0\0\0".to_vec();
        d.extend_from_slice(&[3, 1, 2, 0]);
        for v in [1.0f64, 0., 0., 0., 1., 0., 0., 0., 1.] {
            d.extend_from_slice(&((v * 65536.0) as i32).to_be_bytes());
        }
        d.extend_from_slice(&2u16.to_be_bytes());
        d.extend_from_slice(&2u16.to_be_bytes());
        for _ in 0..3 {
            d.extend_from_slice(&0u16.to_be_bytes());
            d.extend_from_slice(&65535u16.to_be_bytes());
        }
        for x in 0..2u32 {
            for y in 0..2u32 {
                for z in 0..2u32 {
                    let v = (x + 2 * y + 4 * z) as f64 / 7.0;
                    d.extend_from_slice(&((v * 65535.0).round() as u16).to_be_bytes());
                }
            }
        }
        d.extend_from_slice(&0u16.to_be_bytes());
        d.extend_from_slice(&65535u16.to_be_bytes());
        d
    }

    #[test]
    fn trilinear_function_is_reproduced_exactly() {
        let d = mft2_3to1();
        let n = d.len();
        let p = parse_lut(*b"B2A0", &shared(d), 0, n, false, 3, 1).unwrap();
        assert_eq!(p.kind(), LutType::Lut16);
        // A linear function is reproduced exactly by tetrahedral interpolation.
        for inp in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.25, 0.5, 0.75], [0.9, 0.1, 0.3], [0.3, 0.3, 0.9]] {
            let mut o = [0.0];
            p.eval(&inp, &mut o);
            let want = (inp[0] + 2.0 * inp[1] + 4.0 * inp[2]) / 7.0;
            assert!((o[0] - want).abs() < 2e-5, "{inp:?}: {} vs {want}", o[0]);
        }
        let mut o = [0.0];
        p.eval(&[f64::NAN, 2.0, -1.0], &mut o);
        assert!((o[0] - 2.0 / 7.0).abs() < 2e-5);
    }

    #[test]
    fn truncations_and_bad_counts_are_typed_errors() {
        let d = mft2_3to1();
        for cut in 0..d.len() {
            let v = d[..cut].to_vec();
            let n = v.len();
            assert!(parse_lut(*b"B2A0", &shared(v), 0, n, false, 3, 1).is_err(), "cut {cut}");
        }
        let n = d.len();
        assert!(parse_lut(*b"B2A0", &shared(d.clone()), 0, n, false, 3, 4).is_err());
        assert!(parse_lut(*b"A2B0", &shared(d.clone()), 0, n, true, 4, 1).is_err());
        let mut bad = d.clone();
        bad[10] = 1; // grid of 1 point
        assert!(parse_lut(*b"B2A0", &shared(bad), 0, n, false, 3, 1).is_err());
        let mut bad = d;
        bad[0..4].copy_from_slice(b"mBA ");
        assert!(parse_lut(*b"A2B0", &shared(bad), 0, n, true, 3, 1).is_err());
    }
}
