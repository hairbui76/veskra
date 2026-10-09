//! Test-only reference tools: supersampled nonzero scanline rasterizer, exact disc-union
//! rasterizer, and a minimal PNG (zlib/inflate) decoder for reading `rsvg-convert` output.
//! All independently written for verification; none of this ships in the library.
#![allow(dead_code)]

use veskra_core::PointF;
use veskra_vector::{BezPath, WidthPoint};

/// Sample grid: `s × s` point samples per document pixel over `[x0, x0+w) × [y0, y0+h)`.
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub x0: f64,
    pub y0: f64,
    pub w: usize,
    pub h: usize,
    pub s: usize,
}

impl Grid {
    pub fn cols(&self) -> usize {
        self.w * self.s
    }
    pub fn rows(&self) -> usize {
        self.h * self.s
    }
    pub fn len(&self) -> usize {
        self.cols() * self.rows()
    }
    fn row_y(&self, r: usize) -> f64 {
        self.y0 + (r as f64 + 0.5) / self.s as f64
    }
    /// Sample indices `r` with `lo <= y_r < hi` (half-open), clamped.
    fn row_range(&self, lo: f64, hi: f64) -> (usize, usize) {
        let s = self.s as f64;
        let a = ((lo - self.y0) * s - 0.5).ceil().max(0.0);
        let b = ((hi - self.y0) * s - 0.5).ceil().max(0.0);
        (a.min(self.rows() as f64) as usize, b.min(self.rows() as f64) as usize)
    }
    /// Column indices with `lo <= x_c < hi`.
    fn col_range(&self, lo: f64, hi: f64) -> (usize, usize) {
        let s = self.s as f64;
        let a = ((lo - self.x0) * s - 0.5).ceil().max(0.0);
        let b = ((hi - self.x0) * s - 0.5).ceil().max(0.0);
        (a.min(self.cols() as f64) as usize, b.min(self.cols() as f64) as usize)
    }
    pub fn area_per_sample(&self) -> f64 {
        1.0 / (self.s * self.s) as f64
    }
}

/// Nonzero-winding point-sampled rasterization of a path (flattened at 0.002 px).
pub fn raster_path(path: &BezPath, g: &Grid) -> Vec<bool> {
    let mut rows: Vec<Vec<(f64, i32)>> = vec![Vec::new(); g.rows()];
    for poly in path.flatten(0.002) {
        if poly.len() < 2 {
            continue;
        }
        let n = poly.len();
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n]; // implicit close for fill
            if a.y == b.y {
                continue;
            }
            let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            let (r0, r1) = g.row_range(lo.y, hi.y);
            for (r, xs) in rows.iter_mut().enumerate().take(r1).skip(r0) {
                let y = g.row_y(r);
                let t = (y - lo.y) / (hi.y - lo.y);
                xs.push((lo.x + (hi.x - lo.x) * t, dir));
            }
        }
    }
    let mut out = vec![false; g.len()];
    let cols = g.cols();
    for (r, xs) in rows.iter_mut().enumerate() {
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut wind = 0;
        for k in 0..xs.len() {
            wind += xs[k].1;
            if wind != 0 && k + 1 < xs.len() {
                let (c0, c1) = g.col_range(xs[k].0, xs[k + 1].0);
                out[r * cols + c0..r * cols + c1].iter_mut().for_each(|v| *v = true);
            }
        }
    }
    out
}

/// Dense centerline discs `(x, y, r)` interpolated linearly at spacing ≤ `step`.
pub fn dense_discs(pts: &[WidthPoint], step: f64, min_width: f64) -> Vec<(f64, f64, f64)> {
    let r = |w: &WidthPoint| 0.5 * w.width.max(min_width).max(0.0);
    let mut out = Vec::new();
    if let Some(first) = pts.first() {
        out.push((first.p.x, first.p.y, r(first)));
    }
    for w in pts.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let len = a.p.distance(b.p);
        let n = (len / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let p = a.p.lerp(b.p, t);
            out.push((p.x, p.y, r(a) + (r(b) - r(a)) * t));
        }
    }
    out
}

/// Point-sampled rasterization of the union of closed discs, radii offset by `dr`.
pub fn raster_discs(discs: &[(f64, f64, f64)], dr: f64, g: &Grid) -> Vec<bool> {
    let mut rows: Vec<Vec<(f64, f64)>> = vec![Vec::new(); g.rows()];
    for &(cx, cy, r0) in discs {
        let r = r0 + dr;
        if r.is_nan() || r <= 0.0 {
            continue;
        }
        let (a, b) = g.row_range(cy - r, cy + r + 1e-12);
        for (row, iv) in rows.iter_mut().enumerate().take(b).skip(a) {
            let y = g.y0 + (row as f64 + 0.5) / g.s as f64;
            let dy = y - cy;
            let h2 = r * r - dy * dy;
            if h2 >= 0.0 {
                let hx = h2.sqrt();
                iv.push((cx - hx, cx + hx));
            }
        }
    }
    let cols = g.cols();
    let mut out = vec![false; g.len()];
    for (r, iv) in rows.iter_mut().enumerate() {
        iv.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut cur: Option<(f64, f64)> = None;
        let mut flush = |(lo, hi): (f64, f64)| {
            let (c0, c1) = g.col_range(lo, hi + 1e-12);
            out[r * cols + c0..r * cols + c1].iter_mut().for_each(|v| *v = true);
        };
        for &(lo, hi) in iv.iter() {
            cur = match cur {
                Some((a, b)) if lo <= b => Some((a, b.max(hi))),
                Some(prev) => {
                    flush(prev);
                    Some((lo, hi))
                }
                None => Some((lo, hi)),
            };
        }
        if let Some(c) = cur {
            flush(c);
        }
    }
    out
}

/// Grid covering the discs with a margin, integer-aligned.
pub fn grid_for(discs: &[(f64, f64, f64)], margin: f64, s: usize) -> Grid {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y, r) in discs {
        x0 = x0.min(x - r);
        y0 = y0.min(y - r);
        x1 = x1.max(x + r);
        y1 = y1.max(y + r);
    }
    let gx = (x0 - margin).floor();
    let gy = (y0 - margin).floor();
    Grid { x0: gx, y0: gy, w: ((x1 + margin).ceil() - gx) as usize, h: ((y1 + margin).ceil() - gy) as usize, s }
}

/// Boundary length estimate: number of set samples with an unset 4-neighbor, divided by
/// samples-per-px (slight overestimate for diagonal edges).
pub fn perimeter_estimate(mask: &[bool], g: &Grid) -> f64 {
    let (c, r) = (g.cols(), g.rows());
    let mut n = 0usize;
    for y in 0..r {
        for x in 0..c {
            if !mask[y * c + x] {
                continue;
            }
            let edge = x == 0
                || y == 0
                || x + 1 == c
                || y + 1 == r
                || !mask[y * c + x - 1]
                || !mask[y * c + x + 1]
                || !mask[(y - 1) * c + x]
                || !mask[(y + 1) * c + x];
            if edge {
                n += 1;
            }
        }
    }
    n as f64 / g.s as f64
}

pub fn pt(x: f64, y: f64) -> PointF {
    PointF::new(x, y)
}

// ---------------------------------------------------------------------------------------
// Minimal PNG decoder (8-bit RGBA / gray+alpha / RGB / gray, non-interlaced).

#[derive(Debug)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    /// Alpha per pixel, 0..=255.
    pub alpha: Vec<u8>,
}

pub fn decode_png(data: &[u8]) -> Result<Image, String> {
    const SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if data.len() < 8 || data[..8] != SIG {
        return Err("not a PNG".into());
    }
    let mut pos = 8;
    let (mut w, mut h, mut ctype, mut depth, mut interlace) = (0usize, 0usize, 0u8, 0u8, 0u8);
    let mut idat = Vec::new();
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().map_err(|_| "len")?) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data.get(pos + 8..pos + 8 + len).ok_or("truncated chunk")?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(body[0..4].try_into().map_err(|_| "w")?) as usize;
                h = u32::from_be_bytes(body[4..8].try_into().map_err(|_| "h")?) as usize;
                depth = body[8];
                ctype = body[9];
                interlace = body[12];
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if depth != 8 || interlace != 0 {
        return Err(format!("unsupported depth {depth} / interlace {interlace}"));
    }
    let channels = match ctype {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err(format!("unsupported color type {ctype}")),
    };
    if idat.len() < 2 {
        return Err("no data".into());
    }
    let raw = inflate(&idat[2..])?; // skip zlib header (adler32 trailer ignored)
    let stride = w * channels;
    if raw.len() < h * (stride + 1) {
        return Err("short image data".into());
    }
    let mut img = vec![0u8; h * stride];
    for y in 0..h {
        let filter = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= channels { img[y * stride + x - channels] } else { 0 };
            let b = if y > 0 { img[(y - 1) * stride + x] } else { 0 };
            let c = if x >= channels && y > 0 { img[(y - 1) * stride + x - channels] } else { 0 };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => {
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
                f => return Err(format!("bad filter {f}")),
            };
            img[y * stride + x] = src[x].wrapping_add(pred);
        }
    }
    let alpha = (0..w * h)
        .map(|i| match ctype {
            6 => img[i * 4 + 3],
            4 => img[i * 2 + 1],
            _ => 255,
        })
        .collect();
    Ok(Image { w, h, alpha })
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
}

impl Bits<'_> {
    fn need(&mut self, n: u32) -> Result<u32, String> {
        while self.cnt < n {
            let byte = *self.d.get(self.pos).ok_or("inflate: out of input")?;
            self.pos += 1;
            self.buf |= (byte as u64) << self.cnt;
            self.cnt += 8;
        }
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.cnt -= n;
        Ok(v)
    }
}

struct Huff {
    count: [u16; 16],
    sym: Vec<u16>,
}

impl Huff {
    fn new(lengths: &[u8]) -> Huff {
        let mut count = [0u16; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut offs = [0u16; 16];
        for l in 1..16 {
            offs[l] = offs[l - 1] + count[l - 1];
        }
        let mut sym = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                sym[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huff { count, sym }
    }
    fn decode(&self, b: &mut Bits<'_>) -> Result<u16, String> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.need(1)? as i32;
            let count = self.count[len] as i32;
            if code - count < first {
                return Ok(self.sym[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("inflate: bad code".into())
    }
}

const LBASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// Raw DEFLATE (RFC 1951) decoder.
pub fn inflate(d: &[u8]) -> Result<Vec<u8>, String> {
    let mut b = Bits { d, pos: 0, buf: 0, cnt: 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = b.need(1)?;
        let kind = b.need(2)?;
        match kind {
            0 => {
                b.buf = 0;
                b.cnt = 0;
                let hdr = d.get(b.pos..b.pos + 4).ok_or("stored hdr")?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
                b.pos += 4;
                out.extend_from_slice(d.get(b.pos..b.pos + len).ok_or("stored data")?);
                b.pos += len;
            }
            1 | 2 => {
                let (lit, dist) = if kind == 1 {
                    let mut l = [0u8; 288];
                    l[..144].fill(8);
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    l[280..].fill(8);
                    (Huff::new(&l), Huff::new(&[5u8; 30]))
                } else {
                    let hlit = b.need(5)? as usize + 257;
                    let hdist = b.need(5)? as usize + 1;
                    let hclen = b.need(4)? as usize + 4;
                    const ORD: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let mut cl = [0u8; 19];
                    for &o in ORD.iter().take(hclen) {
                        cl[o] = b.need(3)? as u8;
                    }
                    let clh = Huff::new(&cl);
                    let mut lens = Vec::with_capacity(hlit + hdist);
                    while lens.len() < hlit + hdist {
                        let s = clh.decode(&mut b)?;
                        match s {
                            0..=15 => lens.push(s as u8),
                            16 => {
                                let prev = *lens.last().ok_or("repeat with no prev")?;
                                let n = 3 + b.need(2)?;
                                lens.extend(std::iter::repeat_n(prev, n as usize));
                            }
                            17 => {
                                let n = 3 + b.need(3)?;
                                lens.extend(std::iter::repeat_n(0, n as usize));
                            }
                            _ => {
                                let n = 11 + b.need(7)?;
                                lens.extend(std::iter::repeat_n(0, n as usize));
                            }
                        }
                    }
                    (Huff::new(&lens[..hlit]), Huff::new(&lens[hlit..hlit + hdist]))
                };
                loop {
                    let s = lit.decode(&mut b)? as usize;
                    if s < 256 {
                        out.push(s as u8);
                    } else if s == 256 {
                        break;
                    } else {
                        let i = s - 257;
                        let len = *LBASE.get(i).ok_or("bad len")? as usize + b.need(LEXT[i] as u32)? as usize;
                        let ds = dist.decode(&mut b)? as usize;
                        let dd = *DBASE.get(ds).ok_or("bad dist")? as usize + b.need(DEXT[ds] as u32)? as usize;
                        if dd > out.len() {
                            return Err("distance too far".into());
                        }
                        let start = out.len() - dd;
                        for k in 0..len {
                            out.push(out[start + k]);
                        }
                    }
                }
            }
            _ => return Err("bad block type".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}
