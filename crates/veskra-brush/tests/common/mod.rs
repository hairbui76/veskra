//! Shared helpers for veskra-brush integration tests.
#![allow(dead_code)]

use std::sync::Arc;

use veskra_brush::{BrushPreset, FinishedStroke, StrokeContext, StrokeMode, StrokeRenderer};
use veskra_core::{ColorRgba, PointF, RectI, StrokeSample};
use veskra_raster::{Tile, TileGrid, ops};
use veskra_testkit::Rng;

pub const W: u32 = 256;
pub const H: u32 = 128;
pub const INK: ColorRgba = ColorRgba { r: 28.0 / 255.0, g: 52.0 / 255.0, b: 140.0 / 255.0, a: 1.0 };

pub fn canvas() -> RectI {
    RectI::from_size(W, H)
}

/// Synthetic S-curve across a 256×128 canvas with a seeded pressure taper (fixture for PNT-001/002).
pub fn s_curve(n: usize, seed: u64) -> Vec<StrokeSample> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let x = 24.0 + 208.0 * t;
            let y = 64.0 - 34.0 * (2.0 * std::f64::consts::PI * t).sin();
            let p = (0.12 + 0.88 * (std::f64::consts::PI * t).sin().powf(0.8) + rng.range(-0.03, 0.03)).clamp(0.0, 1.0);
            StrokeSample::confirmed(x as f32, y as f32, i as u64 * 4_166_667, p as f32)
        })
        .collect()
}

/// Straight line with linear pressure ramp.
pub fn line(a: (f64, f64), b: (f64, f64), n: usize, p0: f32, p1: f32) -> Vec<StrokeSample> {
    (0..n)
        .map(|i| {
            let t = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
            StrokeSample::confirmed(
                (a.0 + (b.0 - a.0) * t) as f32,
                (a.1 + (b.1 - a.1) * t) as f32,
                i as u64 * 4_000_000,
                p0 + (p1 - p0) * t as f32,
            )
        })
        .collect()
}

/// Spiral with tightening radius (tests curvature and self-overlap).
pub fn spiral(cx: f64, cy: f64, n: usize, seed: u64) -> Vec<StrokeSample> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let a = t * 4.0 * std::f64::consts::PI;
            let r = 50.0 - 38.0 * t;
            let p = (0.5 + 0.4 * (a * 0.7).sin() + rng.range(-0.02, 0.02)).clamp(0.0, 1.0);
            StrokeSample::confirmed(
                (cx + r * a.cos()) as f32,
                (cy + r * a.sin()) as f32,
                i as u64 * 4_000_000,
                p as f32,
            )
        })
        .collect()
}

pub fn ctx(mode: StrokeMode) -> StrokeContext {
    StrokeContext::new(mode, canvas())
}

/// Base filled (whole canvas) with one premultiplied pixel value, tiles shared.
pub fn filled_base(p: [u16; 4]) -> TileGrid {
    let mut g = TileGrid::new();
    let t = Arc::new(Tile::new_filled(p));
    for c in TileGrid::tiles_in_rect(canvas()) {
        g.set_tile(c, t.clone());
    }
    g
}

/// Render a stroke with the given batching (`splits` = batch sizes, cycled).
pub fn render(
    preset: &BrushPreset,
    samples: &[StrokeSample],
    seed: u64,
    base: TileGrid,
    ctx: StrokeContext,
    splits: &[usize],
) -> FinishedStroke {
    let mut r = StrokeRenderer::begin(preset.clone(), INK, seed, base, ctx);
    let mut i = 0;
    let mut k = 0;
    while i < samples.len() {
        let n = splits[k % splits.len()].max(1).min(samples.len() - i);
        r.add_samples(&samples[i..i + n]);
        i += n;
        k += 1;
    }
    r.finish()
}

/// 8-bit straight RGBA of a canvas region via the single conversion module (veskra-color).
pub fn straight8(g: &TileGrid, r: RectI) -> Vec<u8> {
    let px = g.read_rect(r);
    let mut out = vec![0u8; px.len()];
    veskra_color::convert::premul16_to_straight8(&px, &mut out);
    out
}

pub fn premul_invariant_holds(g: &TileGrid) -> bool {
    g.iter_tiles().all(|(_, t)| t.pixels().chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]))
}

pub fn px16(c: ColorRgba) -> [u16; 4] {
    ops::color_to_px16(c)
}

/// Pixel-center nonzero-winding rasterization of a closed path's flattened polylines.
pub fn rasterize_nonzero(polys: &[Vec<PointF>], w: u32, h: u32) -> Vec<bool> {
    let mut edges = Vec::new();
    for poly in polys {
        if poly.len() < 2 {
            continue;
        }
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            if a.y != b.y {
                edges.push((a, b));
            }
        }
    }
    let mut out = vec![false; (w * h) as usize];
    let mut xs: Vec<(f64, i32)> = Vec::new();
    for y in 0..h {
        let yc = y as f64 + 0.5;
        xs.clear();
        for (a, b) in &edges {
            let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            if lo.y <= yc && yc < hi.y {
                xs.push((lo.x + (yc - lo.y) * (hi.x - lo.x) / (hi.y - lo.y), dir));
            }
        }
        xs.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut wind = 0;
        for k in 0..xs.len() {
            wind += xs[k].1;
            if wind != 0 && k + 1 < xs.len() {
                let (x0, x1) = (xs[k].0, xs[k + 1].0);
                for x in 0..w {
                    let xc = x as f64 + 0.5;
                    if xc >= x0 && xc < x1 {
                        out[(y * w + x) as usize] = true;
                    }
                }
            }
        }
    }
    out
}
