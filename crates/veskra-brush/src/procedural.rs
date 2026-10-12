//! Deterministic procedural masks for the built-in brushes (ADR-019).
//!
//! Every mask is generated here from integer hashing and IEEE-exact arithmetic only (`+ − × ÷`,
//! `sqrt`, `floor`; no `sin`/`exp`/`pow` from the platform libm), so all platforms produce
//! byte-identical masks and therefore identical asset content hashes. Nothing is copied from any
//! application or brush pack.

use crate::noise::splitmix64;

/// Uniform value in `[0, 1)` for (seed, n).
fn h01(seed: u64, n: u64) -> f64 {
    (splitmix64(seed ^ splitmix64(n.wrapping_add(0x51A7_0C0F_FEE5_EED5))) >> 11) as f64 / (1u64 << 53) as f64
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 1 inside `r − soft`, 0 beyond `r`, smooth in between.
fn disc(d: f64, r: f64, soft: f64) -> f64 {
    1.0 - smoothstep(r - soft, r, d)
}

/// Value noise with integer lattice period `period` (tileable when the image size is
/// `period × cell`), cell size `cell` px.
fn noise(x: f64, y: f64, cell: f64, period: i64, seed: u64) -> f64 {
    let fx = x / cell;
    let fy = y / cell;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let lat = |ix: i64, iy: i64| {
        let (ix, iy) = (ix.rem_euclid(period), iy.rem_euclid(period));
        h01(seed, ((ix as u64) << 32) ^ iy as u64)
    };
    let (ix, iy) = (x0 as i64, y0 as i64);
    let a = lat(ix, iy);
    let b = lat(ix + 1, iy);
    let c = lat(ix, iy + 1);
    let d = lat(ix + 1, iy + 1);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

fn to_u8(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

fn render(w: u32, h: u32, f: impl Fn(f64, f64) -> f64) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            out.push(to_u8(f(x as f64 + 0.5, y as f64 + 0.5)));
        }
    }
    out
}

/// `(x, y, radius, intensity)` blobs placed by rejection sampling inside `accept`.
fn blobs(
    seed: u64,
    count: usize,
    span: (f64, f64, f64, f64),
    radius: (f64, f64),
    intensity: (f64, f64),
    accept: impl Fn(f64, f64) -> bool,
) -> Vec<[f64; 4]> {
    let (x0, y0, x1, y1) = span;
    let mut out = Vec::with_capacity(count);
    let mut n = 0u64;
    while out.len() < count && n < 100_000 {
        let x = x0 + (x1 - x0) * h01(seed, 4 * n);
        let y = y0 + (y1 - y0) * h01(seed, 4 * n + 1);
        let r = radius.0 + (radius.1 - radius.0) * h01(seed, 4 * n + 2);
        let i = intensity.0 + (intensity.1 - intensity.0) * h01(seed, 4 * n + 3);
        if accept(x, y) {
            out.push([x, y, r, i]);
        }
        n += 1;
    }
    out
}

fn blob_max(b: &[[f64; 4]], x: f64, y: f64, soft: f64) -> f64 {
    let mut v: f64 = 0.0;
    for [bx, by, r, i] in b {
        let dx = x - bx;
        let dy = y - by;
        let d2 = dx * dx + dy * dy;
        if d2 < r * r {
            v = v.max(i * disc(d2.sqrt(), *r, soft));
        }
    }
    v
}

/// Round bristle brush head: ~48 bristles inside a disc.
pub(crate) fn bristle_round() -> (u32, u32, Vec<u8>) {
    let b = blobs(0xB715_7E01, 48, (10.0, 10.0, 118.0, 118.0), (4.0, 8.0), (0.55, 1.0), |x, y| {
        (x - 64.0) * (x - 64.0) + (y - 64.0) * (y - 64.0) <= 54.0 * 54.0
    });
    let m = render(128, 128, |x, y| {
        let d = ((x - 64.0) * (x - 64.0) + (y - 64.0) * (y - 64.0)).sqrt();
        blob_max(&b, x, y, 1.5) * disc(d, 62.0, 10.0)
    });
    (128, 128, m)
}

/// Flat brush head: bristles in a vertical band (perpendicular to a rightward stroke).
pub(crate) fn bristle_flat() -> (u32, u32, Vec<u8>) {
    let b = blobs(0xF1A7_0002, 60, (50.0, 8.0, 78.0, 120.0), (2.5, 5.0), (0.5, 1.0), |_, _| true);
    let m = render(128, 128, |x, y| {
        let env = disc((x - 64.0).abs(), 16.0, 4.0) * disc((y - 64.0).abs(), 60.0, 6.0);
        blob_max(&b, x, y, 1.2) * env
    });
    (128, 128, m)
}

/// Dry-brush ink tip: a disc broken up by noise holes.
pub(crate) fn dry() -> (u32, u32, Vec<u8>) {
    let m = render(96, 96, |x, y| {
        let d = ((x - 48.0) * (x - 48.0) + (y - 48.0) * (y - 48.0)).sqrt();
        let n = 0.6 * noise(x, y, 7.0, 1 << 20, 0xD7_0003) + 0.4 * noise(x, y, 3.0, 1 << 20, 0xD7_0004);
        disc(d, 44.0, 3.0) * smoothstep(0.32, 0.5, n)
    });
    (96, 96, m)
}

/// Spray speckle: many tiny dots inside a disc.
pub(crate) fn speckle() -> (u32, u32, Vec<u8>) {
    let b = blobs(0x5BEC_0005, 160, (4.0, 4.0, 124.0, 124.0), (0.8, 2.6), (0.6, 1.0), |x, y| {
        (x - 64.0) * (x - 64.0) + (y - 64.0) * (y - 64.0) <= 60.0 * 60.0
    });
    (128, 128, render(128, 128, |x, y| blob_max(&b, x, y, 0.8)))
}

/// Chalk tip: irregular edge and pitted interior.
pub(crate) fn chalk() -> (u32, u32, Vec<u8>) {
    let m = render(96, 96, |x, y| {
        let d = ((x - 48.0) * (x - 48.0) + (y - 48.0) * (y - 48.0)).sqrt();
        let r = 38.0 + 8.0 * (noise(x, y, 18.0, 1 << 20, 0xC4A1_0006) - 0.5);
        let pits = smoothstep(0.25, 0.6, noise(x, y, 2.5, 1 << 20, 0xC4A1_0007));
        disc(d, r, 4.0) * (0.45 + 0.55 * pits)
    });
    (96, 96, m)
}

/// Sponge tip: thresholded multi-octave noise inside a soft disc.
pub(crate) fn sponge() -> (u32, u32, Vec<u8>) {
    let m = render(128, 128, |x, y| {
        let d = ((x - 64.0) * (x - 64.0) + (y - 64.0) * (y - 64.0)).sqrt();
        let n = 0.5 * noise(x, y, 9.0, 1 << 20, 0x5F06_0008)
            + 0.3 * noise(x, y, 4.5, 1 << 20, 0x5F06_0009)
            + 0.2 * noise(x, y, 2.2, 1 << 20, 0x5F06_000A);
        disc(d, 60.0, 14.0) * smoothstep(0.42, 0.58, n)
    });
    (128, 128, m)
}

/// Tileable rough paper (256 px period). White = paint passes, dark = pits.
pub(crate) fn paper() -> (u32, u32, Vec<u8>) {
    let m = render(256, 256, |x, y| {
        let n = 0.5 * noise(x, y, 8.0, 32, 0x9A9E_000B)
            + 0.3 * noise(x, y, 4.0, 64, 0x9A9E_000C)
            + 0.2 * noise(x, y, 2.0, 128, 0x9A9E_000D);
        smoothstep(0.25, 0.75, n)
    });
    (256, 256, m)
}

/// Tileable canvas weave (64 px period).
pub(crate) fn canvas() -> (u32, u32, Vec<u8>) {
    let m = render(64, 64, |x, y| {
        let (cx, cy) = ((x / 8.0).floor(), (y / 8.0).floor());
        let (tx, ty) = (x / 8.0 - cx, y / 8.0 - cy);
        let tri = |t: f64| 1.0 - (2.0 * t - 1.0).abs();
        let over = (cx as i64 + cy as i64).rem_euclid(2) == 0;
        let thread = if over { tri(ty) } else { tri(tx) };
        let grain = noise(x, y, 2.0, 32, 0xCA7A_000E);
        0.5 + 0.4 * thread + 0.1 * grain
    });
    (64, 64, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_nontrivial() {
        for (name, f) in [
            ("bristle_round", bristle_round as fn() -> (u32, u32, Vec<u8>)),
            ("bristle_flat", bristle_flat),
            ("dry", dry),
            ("speckle", speckle),
            ("chalk", chalk),
            ("sponge", sponge),
            ("paper", paper),
            ("canvas", canvas),
        ] {
            let (w, h, a) = f();
            let (_, _, b) = f();
            assert_eq!(a, b, "{name}");
            assert_eq!(a.len(), (w * h) as usize);
            let painted = a.iter().filter(|v| **v > 128).count();
            let (lo, hi) = (a.iter().min().copied().unwrap_or(0), a.iter().max().copied().unwrap_or(0));
            assert!(painted > a.len() / 50 && hi - lo > 60, "{name}: {painted} {lo}..{hi}");
        }
    }

    #[test]
    fn paper_tiles_seamlessly() {
        let (w, h, m) = paper();
        // Wrapped neighbours across the seam differ no more than typical neighbours inside.
        let at = |x: u32, y: u32| m[(y % h * w + x % w) as usize] as i32;
        let seam: i32 = (0..h).map(|y| (at(w - 1, y) - at(0, y)).abs()).max().unwrap_or(0);
        let inner: i32 = (0..h).map(|y| (at(100, y) - at(101, y)).abs()).max().unwrap_or(0);
        assert!(seam <= inner + 40, "seam {seam} inner {inner}");
    }
}
