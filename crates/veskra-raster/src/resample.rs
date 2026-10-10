//! Whole-canvas resampling of pixels and masks (`canvas.resize` with `resample: true`).
//!
//! Policy (identical to the PNG exporter's `raster_scale`): per axis an **area-average (box)
//! filter when shrinking** and **bilinear interpolation when enlarging** (pixel-centre aligned,
//! clamped at the canvas edge), separable, applied to **premultiplied** values so no dark or
//! colored fringes appear. Weights are `f32`, sums are rounded half-up, and color channels are
//! clamped to alpha (`r,g,b ≤ a`). Only the rows/columns influenced by existing content are
//! visited; memory use is a few rows.
//!
//! Content outside the source canvas `[0, w) × [0, h)` is ignored (the engine keeps layer pixels
//! inside the canvas).

use veskra_core::RectI;

use crate::{MAX_RECT_PIXELS, MaskGrid, RasterError, TileGrid};

#[derive(Debug, Clone)]
struct Taps {
    start: u32,
    weights: Vec<f32>,
}

/// Taps mapping `src` samples to `dst` samples (box when shrinking, bilinear when enlarging).
fn filter_taps(src: u32, dst: u32) -> Vec<Taps> {
    let s = dst as f64 / src as f64;
    (0..dst)
        .map(|o| {
            if dst < src {
                let (a, b) = (o as f64 / s, (o as f64 + 1.0) / s);
                let i0 = (a.floor().max(0.0) as u32).min(src - 1);
                let i1 = (b.ceil() as u32).clamp(i0 + 1, src);
                let mut w: Vec<f32> =
                    (i0..i1).map(|i| ((b.min(i as f64 + 1.0) - a.max(i as f64)).max(0.0) * s) as f32).collect();
                let sum: f32 = w.iter().sum();
                if sum > 0.0 {
                    w.iter_mut().for_each(|v| *v /= sum);
                }
                Taps { start: i0, weights: w }
            } else {
                let c = (o as f64 + 0.5) / s - 0.5;
                let i0 = c.floor();
                let t = (c - i0) as f32;
                let lo = (i0.max(0.0) as u32).min(src - 1);
                let hi = ((i0 + 1.0).max(0.0) as u32).min(src - 1);
                if lo == hi {
                    Taps { start: lo, weights: vec![1.0] }
                } else {
                    Taps { start: lo, weights: vec![1.0 - t, t] }
                }
            }
        })
        .collect()
}

/// Half-open range of output indices whose taps touch source samples `[lo, hi)`.
fn influenced(taps: &[Taps], lo: u32, hi: u32) -> (usize, usize) {
    let first = taps.iter().position(|t| t.start + t.weights.len() as u32 > lo && t.start < hi);
    let last = taps.iter().rposition(|t| t.start + t.weights.len() as u32 > lo && t.start < hi);
    match (first, last) {
        (Some(a), Some(b)) => (a, b + 1),
        _ => (0, 0),
    }
}

fn check(src: (u32, u32), dst: (u32, u32)) -> Result<(), RasterError> {
    if src.0 == 0 || src.1 == 0 || dst.0 == 0 || dst.1 == 0 {
        return Err(RasterError::InvalidArgument("resample sizes must be nonzero"));
    }
    for (w, h) in [src, dst] {
        let px = w as u64 * h as u64;
        if px > MAX_RECT_PIXELS || w > i32::MAX as u32 / 2 || h > i32::MAX as u32 / 2 {
            return Err(RasterError::TooLarge { pixels: px, limit: MAX_RECT_PIXELS });
        }
    }
    Ok(())
}

/// Fills a buffer with source row `y`, columns `[x0, x1)`.
type ReadRow<'a> = dyn FnMut(i32, i32, i32, &mut Vec<f32>) + 'a;
/// Receives one output row segment starting at column `x0`.
type WriteRow<'a> = dyn FnMut(i32, i32, &[f32]) -> Result<(), RasterError> + 'a;

/// Separable resampling core over `ch` interleaved channels. `content` is the source area that
/// may hold non-default data (inside the source canvas). `read_row(y, x0, x1, buf)` fills `buf`
/// with source row `y`, columns `[x0, x1)`; `write_row(y, x0, row)` receives one output row
/// segment starting at column `x0`.
fn resample_core(
    ch: usize,
    src: (u32, u32),
    dst: (u32, u32),
    content: RectI,
    read_row: &mut ReadRow<'_>,
    write_row: &mut WriteRow<'_>,
) -> Result<(), RasterError> {
    let content = content.intersect(&RectI::from_size(src.0, src.1));
    if content.is_empty() {
        return Ok(());
    }
    let xt = filter_taps(src.0, dst.0);
    let yt = filter_taps(src.1, dst.1);
    let (dx0, dx1) = influenced(&xt, content.x0 as u32, content.x1 as u32);
    let (dy0, dy1) = influenced(&yt, content.y0 as u32, content.y1 as u32);
    if dx0 >= dx1 || dy0 >= dy1 {
        return Ok(());
    }
    let sx0 = xt[dx0].start;
    let sx1 = xt[dx0..dx1].iter().map(|t| t.start + t.weights.len() as u32).max().unwrap_or(sx0);
    let n = dx1 - dx0;
    let mut raw: Vec<f32> = Vec::new();
    // Up to two horizontally filtered source rows are reused between adjacent output rows.
    let mut cache: Vec<(u32, Vec<f32>)> = Vec::with_capacity(2);
    let mut acc = vec![0f32; n * ch];
    for (oy, ty) in yt.iter().enumerate().take(dy1).skip(dy0) {
        acc.iter_mut().for_each(|v| *v = 0.0);
        for (k, wy) in ty.weights.iter().enumerate() {
            let sy = ty.start + k as u32;
            if *wy == 0.0 {
                continue;
            }
            if !cache.iter().any(|(y, _)| *y == sy) {
                read_row(sy as i32, sx0 as i32, sx1 as i32, &mut raw);
                let mut h = if cache.len() == 2 { cache.remove(0).1 } else { Vec::new() };
                h.clear();
                h.resize(n * ch, 0.0);
                for (i, t) in xt[dx0..dx1].iter().enumerate() {
                    for (j, wx) in t.weights.iter().enumerate() {
                        let si = (t.start as usize + j - sx0 as usize) * ch;
                        if let Some(s) = raw.get(si..si + ch) {
                            for c in 0..ch {
                                h[i * ch + c] += *wx * s[c];
                            }
                        }
                    }
                }
                cache.push((sy, h));
            }
            if let Some((_, h)) = cache.iter().find(|(y, _)| *y == sy) {
                for (a, v) in acc.iter_mut().zip(h.iter()) {
                    *a += *wy * *v;
                }
            }
        }
        write_row(oy as i32, dx0 as i32, &acc)?;
    }
    Ok(())
}

/// Resample the pixels of a `src`-sized canvas to a `dst`-sized canvas.
pub fn resample_pixels(grid: &TileGrid, src: (u32, u32), dst: (u32, u32)) -> Result<TileGrid, RasterError> {
    check(src, dst)?;
    let mut out = TileGrid::new();
    let mut row16: Vec<u16> = Vec::new();
    let mut read = |y: i32, x0: i32, x1: i32, buf: &mut Vec<f32>| {
        let px = grid.read_rect(RectI::new(x0, y, x1, y + 1));
        buf.clear();
        buf.extend(px.iter().map(|&v| v as f32));
    };
    let mut write = |y: i32, x0: i32, row: &[f32]| -> Result<(), RasterError> {
        let q = |v: f32| (v + 0.5).floor().clamp(0.0, 65535.0) as u16;
        row16.clear();
        for p in row.chunks_exact(4) {
            let a = q(p[3]);
            row16.extend_from_slice(&[q(p[0]).min(a), q(p[1]).min(a), q(p[2]).min(a), a]);
        }
        out.write_rect(RectI::new(x0, y, x0 + (row.len() / 4) as i32, y + 1), &row16)
    };
    resample_core(4, src, dst, grid.content_bounds(), &mut read, &mut write)?;
    out.prune_transparent();
    Ok(out)
}

/// Resample a coverage mask of a `src`-sized canvas to a `dst`-sized canvas. The default value
/// outside materialized tiles (0 or 255) is preserved.
pub fn resample_mask(mask: &MaskGrid, src: (u32, u32), dst: (u32, u32)) -> Result<MaskGrid, RasterError> {
    check(src, dst)?;
    let dflt = mask.default_value();
    let mut out = if dflt == 0 { MaskGrid::new_empty() } else { MaskGrid::new_full() };
    let content = if dflt == 0 { mask.bounds() } else { RectI::from_size(src.0, src.1) };
    let mut row8: Vec<u8> = Vec::new();
    let mut read = |y: i32, x0: i32, x1: i32, buf: &mut Vec<f32>| {
        let px = mask.read_rect(RectI::new(x0, y, x1, y + 1));
        buf.clear();
        buf.extend(px.iter().map(|&v| v as f32));
    };
    let mut write = |y: i32, x0: i32, row: &[f32]| -> Result<(), RasterError> {
        row8.clear();
        row8.extend(row.iter().map(|v| (v + 0.5).floor().clamp(0.0, 255.0) as u8));
        out.write_rect(RectI::new(x0, y, x0 + row.len() as i32, y + 1), &row8)
    };
    resample_core(1, src, dst, content, &mut read, &mut write)?;
    out.compact();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_sum_to_one_and_stay_in_range() {
        for (s, d) in [(10u32, 3u32), (3, 10), (7, 7), (1, 4), (4, 1), (1000, 333), (5, 4), (8192, 1)] {
            for t in filter_taps(s, d) {
                let sum: f32 = t.weights.iter().sum();
                assert!((sum - 1.0).abs() < 1e-4, "{s}->{d}: {sum}");
                assert!(!t.weights.is_empty() && t.start as usize + t.weights.len() <= s as usize);
            }
        }
    }

    #[test]
    fn halving_averages_in_premultiplied_space() {
        // Checker of opaque red / transparent → half-alpha pure red, no dark fringe.
        let mut g = TileGrid::new();
        for y in 0..32 {
            for x in 0..64 {
                if (x + y) % 2 == 0 {
                    g.put_pixel(x, y, [65535, 0, 0, 65535]);
                }
            }
        }
        let h = resample_pixels(&g, (64, 32), (32, 16)).unwrap();
        assert_eq!(h.content_bounds(), RectI::new(0, 0, 32, 16));
        for y in 0..16 {
            for x in 0..32 {
                assert_eq!(h.get_pixel(x, y), [32768, 0, 0, 32768]);
            }
        }
    }

    #[test]
    fn doubling_replicates_flat_areas_and_interpolates_edges() {
        let mut g = TileGrid::new();
        for y in 0..4 {
            for x in 0..4 {
                g.put_pixel(x, y, if x < 2 { [0, 0, 0, 65535] } else { [65535; 4] });
            }
        }
        let h = resample_pixels(&g, (4, 4), (8, 8)).unwrap();
        assert_eq!(h.content_bounds(), RectI::new(0, 0, 8, 8));
        assert_eq!(h.get_pixel(0, 0), [0, 0, 0, 65535]);
        assert_eq!(h.get_pixel(2, 7), [0, 0, 0, 65535]);
        assert_eq!(h.get_pixel(3, 3), [16384, 16384, 16384, 65535]);
        assert_eq!(h.get_pixel(4, 3), [49151, 49151, 49151, 65535]);
        assert_eq!(h.get_pixel(7, 0), [65535; 4]);
    }

    #[test]
    fn sparse_content_only_touches_its_neighbourhood() {
        let mut g = TileGrid::new();
        for y in 1000..1010 {
            for x in 2000..2020 {
                g.put_pixel(x, y, [100, 200, 300, 40000]);
            }
        }
        let h = resample_pixels(&g, (4096, 4096), (2048, 1024)).unwrap();
        assert_eq!(h.content_bounds(), RectI::new(1000, 250, 1010, 253));
        assert_eq!(h.get_pixel(1005, 251), [100, 200, 300, 40000]);
        assert!(h.tile_count() <= 2);
        // Same size is an exact copy.
        assert_eq!(resample_pixels(&g, (4096, 4096), (4096, 4096)).unwrap(), g);
    }

    #[test]
    fn masks_keep_their_default() {
        let m = MaskGrid::new_rect(RectI::new(8, 8, 24, 16));
        let h = resample_mask(&m, (32, 32), (16, 16)).unwrap();
        assert_eq!(h, MaskGrid::new_rect(RectI::new(4, 4, 12, 8)));
        let mut full = MaskGrid::new_full();
        full.fill_rect(RectI::new(0, 0, 16, 32), 0);
        let h = resample_mask(&full, (32, 32), (64, 64)).unwrap();
        assert_eq!(h.default_value(), 255);
        assert_eq!(h.get(5, 5), 0);
        assert_eq!(h.get(40, 5), 255);
        assert_eq!(h.get(5000, 5000), 255);
        assert_eq!(resample_mask(&MaskGrid::new_empty(), (32, 32), (7, 9)).unwrap(), MaskGrid::new_empty());
    }

    #[test]
    fn hostile_sizes_are_typed_errors() {
        let g = TileGrid::new();
        assert!(matches!(resample_pixels(&g, (0, 4), (4, 4)), Err(RasterError::InvalidArgument(_))));
        assert!(matches!(resample_pixels(&g, (4, 4), (4, 0)), Err(RasterError::InvalidArgument(_))));
        assert!(matches!(resample_pixels(&g, (4, 4), (1_000_000, 1_000_000)), Err(RasterError::TooLarge { .. })));
        assert!(matches!(
            resample_mask(&MaskGrid::new_full(), (u32::MAX, 2), (4, 4)),
            Err(RasterError::TooLarge { .. })
        ));
    }
}
