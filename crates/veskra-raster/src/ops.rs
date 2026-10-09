//! Pixel math on premultiplied RGBA16 and grid-level compositing helpers.
//!
//! All arithmetic is integer with a **single round-half-up division by 65535** per channel:
//! `div65535(x) = (x + 32767) / 65535` for `x <= 65535²`. Every operation is monotone in
//! each channel and applies identical weights to color and alpha, so `r,g,b <= a` holds for
//! outputs whenever it holds for inputs (property-tested).

use std::sync::Arc;

use veskra_core::{ColorRgba, RectI};

use crate::mask::MaskTile;
use crate::tile::{TILE_PIXELS, Tile, clamp_premul};
use crate::{MaskGrid, Px16, TILE_U, TileCoord, TileGrid, UNBOUNDED};

/// `round(x / 65535)` (half up), valid for `x <= 65535 * 65535 + 32767`.
#[inline]
pub const fn div65535(x: u32) -> u32 {
    (x + 32767) / 65535
}

/// `round(a * b / 65535)`.
#[inline]
pub const fn mul65535(a: u16, b: u16) -> u16 {
    div65535(a as u32 * b as u32) as u16
}

/// 8-bit coverage to 16-bit coverage (exact: `v * 257`).
#[inline]
pub const fn cov8_to_16(v: u8) -> u16 {
    v as u16 * 257
}

/// Straight, encoded [`ColorRgba`] → premultiplied RGBA16.
///
/// Non-finite components become 0; components are clamped to `[0, 1]`.
/// `a16 = floor(a * 65535 + 0.5)`, `c16 = floor(c * a * 65535 + 0.5)` (computed in `f64`,
/// the CONTRACTS §1 float→u16 rule applied to the premultiplied value), then `c16 <= a16`.
pub fn color_to_px16(c: ColorRgba) -> Px16 {
    let s = |v: f32| if v.is_finite() { (v as f64).clamp(0.0, 1.0) } else { 0.0 };
    let a = s(c.a);
    let q = |v: f64| (v * 65535.0 + 0.5).floor().clamp(0.0, 65535.0) as u16;
    let a16 = q(a);
    clamp_premul([q(s(c.r) * a), q(s(c.g) * a), q(s(c.b) * a), a16])
}

/// Porter-Duff source-over: `out = src + dst * (1 - src.a)`.
#[inline]
pub fn src_over(dst: Px16, src: Px16) -> Px16 {
    let k = 65535 - src[3] as u32;
    let f = |i: usize| (src[i] as u32 + div65535(dst[i] as u32 * k)).min(65535) as u16;
    [f(0), f(1), f(2), f(3)]
}

/// Source-atop ("alpha lock" / preserve transparency): color is painted over existing
/// pixels, destination alpha is unchanged. `out.c = dst.c * (1 - src.a) + src.c * dst.a`,
/// `out.a = dst.a`. Single rounding of the combined numerator, which is `<= dst.a * 65535`,
/// so `out.c <= out.a` exactly.
#[inline]
pub fn src_atop(dst: Px16, src: Px16) -> Px16 {
    let k = 65535 - src[3] as u32;
    let da = dst[3] as u32;
    let f = |i: usize| div65535(dst[i] as u32 * k + src[i] as u32 * da).min(da) as u16;
    [f(0), f(1), f(2), dst[3]]
}

/// Destination-out with 16-bit coverage: `out = dst * (1 - coverage)`.
#[inline]
pub fn erase(dst: Px16, coverage: u16) -> Px16 {
    scale(dst, 65535 - coverage)
}

/// Multiply all four channels by `k / 65535` (rounded).
#[inline]
pub fn scale(p: Px16, k: u16) -> Px16 {
    if k == 65535 {
        return p;
    }
    [mul65535(p[0], k), mul65535(p[1], k), mul65535(p[2], k), mul65535(p[3], k)]
}

/// How [`composite_mask_color`] combines the color with the destination.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CompositeMode {
    /// [`src_over`].
    Over,
    /// [`src_atop`] (alpha lock).
    AlphaLock,
    /// [`erase`] by coverage × opacity (color ignored).
    Erase,
}

/// Paint `color` × `opacity` × mask coverage onto `target`.
///
/// The affected area is `mask.bounds() ∩ clip`. If that is unbounded (mask default is nonzero
/// and `clip` is `None`) the operation is limited to the target's materialized tiles.
/// Returns the dirty rectangle (bounds of affected nonzero coverage, `RectI::EMPTY` if none).
/// Tiles that end up fully transparent are removed; fully covered tiles painted with an
/// opaque color share one tile allocation.
pub fn composite_mask_color(
    target: &mut TileGrid,
    mask: &MaskGrid,
    color: ColorRgba,
    opacity: f32,
    mode: CompositeMode,
    clip: Option<RectI>,
) -> RectI {
    let mut area = mask.bounds().intersect(&clip.unwrap_or(UNBOUNDED));
    if area == UNBOUNDED || (clip.is_none() && mask.default_value() != 0) {
        area = area.intersect(&target.tile_bounds());
    }
    if area.is_empty() {
        return RectI::EMPTY;
    }
    let op16 = color_to_px16(ColorRgba::new(0.0, 0.0, 0.0, if opacity.is_finite() { opacity } else { 0.0 }))[3];
    let src = color_to_px16(color);
    // Per-coverage lookup: effective coverage k = cov16 * opacity, scaled source.
    let mut ktab = [0u16; 256];
    let mut stab = [[0u16; 4]; 256];
    for v in 0..256usize {
        ktab[v] = mul65535(cov8_to_16(v as u8), op16);
        stab[v] = scale(src, ktab[v]);
    }
    let full_tile =
        (mode == CompositeMode::Over && stab[255][3] == 65535).then(|| Arc::new(Tile::new_filled(stab[255])));
    let dflt = mask.default_value();
    let coords: Vec<TileCoord> = if dflt == 0 {
        mask.iter_tiles().map(|(c, _)| c).filter(|c| !c.rect().intersect(&area).is_empty()).collect()
    } else if mode == CompositeMode::Over {
        TileGrid::tiles_in_rect(area).collect()
    } else {
        // AlphaLock/Erase never change transparent pixels: only visit existing tiles.
        target.tiles_overlapping(area).map(|(c, _)| c).collect()
    };
    let default_tile = crate::mask::uniform_mask_tile(dflt);
    let mut dirty = RectI::EMPTY;
    for c in coords {
        let tr = c.rect();
        let ir = tr.intersect(&area);
        let mt: &MaskTile = mask.tile(c).unwrap_or(&default_tile);
        if mode != CompositeMode::Over && target.tile(c).is_none() {
            continue;
        }
        if ir == tr
            && mt.uniform_value() == Some(255)
            && let Some(ft) = &full_tile
        {
            target.set_tile(c, ft.clone());
            dirty = dirty.union(&tr);
            continue;
        }
        // Tight local bounds of nonzero coverage within ir.
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in ir.y0..ir.y1 {
            let row = &mt.data()[(y - tr.y0) as usize * TILE_U..][..TILE_U];
            for x in ir.x0..ir.x1 {
                if row[(x - tr.x0) as usize] != 0 {
                    x0 = x0.min(x);
                    x1 = x1.max(x + 1);
                    y0 = y0.min(y);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x0 >= x1 {
            continue;
        }
        let tile = target.tile_mut(c);
        let px = tile.pixels_mut();
        for y in y0..y1 {
            let ly = (y - tr.y0) as usize;
            for x in x0..x1 {
                let i = ly * TILE_U + (x - tr.x0) as usize;
                let cv = mt.data()[i] as usize;
                if cv == 0 {
                    continue;
                }
                let d = [px[i * 4], px[i * 4 + 1], px[i * 4 + 2], px[i * 4 + 3]];
                let o = match mode {
                    CompositeMode::Over => src_over(d, stab[cv]),
                    CompositeMode::AlphaLock => src_atop(d, stab[cv]),
                    CompositeMode::Erase => erase(d, ktab[cv]),
                };
                px[i * 4..i * 4 + 4].copy_from_slice(&o);
            }
        }
        if mode != CompositeMode::Over && tile.is_transparent() {
            target.remove_tile(c);
        }
        dirty = dirty.union(&RectI::new(x0, y0, x1, y1));
    }
    dirty
}

/// Move the content selected by `region` out of `src`: returns the cut part
/// (`round(p * coverage)`) and leaves `p - cut` in `src`, so `cut + remainder == original`
/// exactly per channel. Fully selected tiles are moved without copying.
pub fn cut(src: &mut TileGrid, region: &MaskGrid) -> TileGrid {
    let mut out = TileGrid::new();
    let dflt = region.default_value();
    let coords: Vec<TileCoord> = src.coords().collect();
    for c in coords {
        let mt = region.tile(c).cloned();
        let uni = mt.as_ref().map_or(Some(dflt), |t| t.uniform_value());
        match uni {
            Some(0) => continue,
            Some(255) => {
                if let Some(t) = src.take_tile(c) {
                    out.set_tile(c, t);
                }
                continue;
            }
            _ => {}
        }
        let Some(st) = src.tile(c).cloned() else { continue };
        let mut cut_tile = Tile::new_transparent();
        let mut rem = Tile::new_transparent();
        let (sp, cp, rp) = (st.pixels(), cut_tile.pixels_mut(), rem.pixels_mut());
        for i in 0..TILE_PIXELS {
            let cov = mt.as_ref().map_or(dflt, |t| t.data()[i]);
            let p = [sp[i * 4], sp[i * 4 + 1], sp[i * 4 + 2], sp[i * 4 + 3]];
            let part = scale(p, cov8_to_16(cov));
            for ch in 0..4 {
                cp[i * 4 + ch] = part[ch];
                rp[i * 4 + ch] = p[ch] - part[ch];
            }
        }
        if !cut_tile.is_transparent() {
            out.set_tile(c, Arc::new(cut_tile));
        }
        if rem.is_transparent() {
            src.remove_tile(c);
        } else {
            src.set_tile(c, Arc::new(rem));
        }
    }
    out
}

/// Source-over `src` onto `dst`. Tiles missing in `dst` are shared from `src` (no copy).
/// Returns the content bounds of `src` (the dirty rectangle).
pub fn paste_over(dst: &mut TileGrid, src: &TileGrid) -> RectI {
    for (c, st) in src.iter_tiles() {
        if st.is_transparent() {
            continue;
        }
        match dst.tile(c) {
            None => dst.set_tile(c, st.clone()),
            Some(dt) if dt.is_transparent() => dst.set_tile(c, st.clone()),
            Some(_) => {
                let t = dst.tile_mut(c);
                let dp = t.pixels_mut();
                for (d, s) in dp.chunks_exact_mut(4).zip(st.pixels().chunks_exact(4)) {
                    if s[3] == 0 && s[0] == 0 && s[1] == 0 && s[2] == 0 {
                        continue;
                    }
                    let o = src_over([d[0], d[1], d[2], d[3]], [s[0], s[1], s[2], s[3]]);
                    d.copy_from_slice(&o);
                }
            }
        }
    }
    src.content_bounds()
}

/// Multiply every pixel of `grid` by `region` coverage (pixels outside the region become
/// transparent). Used for "transform selected content".
pub fn mask_pixels(grid: &TileGrid, region: &MaskGrid) -> TileGrid {
    let mut tmp = grid.clone();
    cut(&mut tmp, region)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift for property tests (no external crates).
    pub(crate) struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        pub fn u16(&mut self) -> u16 {
            // Bias toward edge values to exercise rounding extremes.
            match self.next() % 8 {
                0 => 0,
                1 => 65535,
                2 => 1,
                3 => 65534,
                _ => self.next() as u16,
            }
        }
        pub fn px(&mut self) -> Px16 {
            let a = self.u16();
            let c = |r: &mut Rng| if a == 0 { 0 } else { (r.next() % (a as u64 + 1)) as u16 };
            [c(self), c(self), c(self), a]
        }
    }

    fn valid(p: Px16) -> bool {
        p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]
    }

    #[test]
    fn div65535_is_exact_round_half_up() {
        for x in [0u32, 1, 32767, 32768, 65535, 65536, 98302, 98303, 65535 * 65535] {
            let exact = (x as f64 / 65535.0 + 0.5).floor() as u32;
            assert_eq!(div65535(x), exact, "x={x}");
        }
        let mut r = Rng(0x1234_5678);
        for _ in 0..100_000 {
            let a = r.u16();
            let b = r.u16();
            let exact = ((a as u64 * b as u64) as f64 / 65535.0 + 0.5).floor() as u16;
            assert_eq!(mul65535(a, b), exact);
        }
    }

    #[test]
    fn color_conversion_rounding() {
        assert_eq!(color_to_px16(ColorRgba::WHITE), [65535; 4]);
        assert_eq!(color_to_px16(ColorRgba::BLACK), [0, 0, 0, 65535]);
        assert_eq!(color_to_px16(ColorRgba::TRANSPARENT), [0; 4]);
        assert_eq!(color_to_px16(ColorRgba::new(1.0, 0.5, 0.0, 0.5)), [32768, 16384, 0, 32768]);
        assert_eq!(color_to_px16(ColorRgba::new(f32::NAN, 2.0, -1.0, 1.0)), [0, 65535, 0, 65535]);
        assert_eq!(color_to_px16(ColorRgba::new(1.0, 1.0, 1.0, f32::INFINITY)), [0; 4]);
        let mut r = Rng(99);
        for _ in 0..10_000 {
            let f = |r: &mut Rng| (r.next() % 10_001) as f32 / 10_000.0;
            let c = ColorRgba::new(f(&mut r), f(&mut r), f(&mut r), f(&mut r));
            assert!(valid(color_to_px16(c)));
        }
    }

    #[test]
    fn src_over_identities() {
        let p = [1000, 2000, 3000, 40000];
        assert_eq!(src_over(p, [0; 4]), p);
        assert_eq!(src_over([0; 4], p), p);
        assert_eq!(src_over(p, [5, 6, 7, 65535]), [5, 6, 7, 65535]);
        assert_eq!(src_atop([0; 4], [65535; 4]), [0; 4]);
        assert_eq!(src_atop([0, 0, 0, 30000], [65535; 4]), [30000, 30000, 30000, 30000]);
        assert_eq!(erase(p, 65535), [0; 4]);
        assert_eq!(erase(p, 0), p);
    }

    #[test]
    fn premultiplied_invariant_property() {
        let mut r = Rng(0xDEAD_BEEF);
        for _ in 0..200_000 {
            let d = r.px();
            let s = r.px();
            let k = r.u16();
            assert!(valid(src_over(d, s)), "{d:?} {s:?}");
            assert!(valid(src_atop(d, s)), "{d:?} {s:?}");
            assert_eq!(src_atop(d, s)[3], d[3]);
            assert!(valid(erase(d, k)));
            assert!(valid(scale(d, k)));
            let part = scale(d, k);
            let rem = [d[0] - part[0], d[1] - part[1], d[2] - part[2], d[3] - part[3]];
            assert!(valid(rem), "{d:?} k={k}");
        }
    }

    fn solid(r: RectI, p: Px16) -> TileGrid {
        let mut g = TileGrid::new();
        let data: Vec<u16> = (0..r.area()).flat_map(|_| p).collect();
        g.write_rect(r, &data).unwrap();
        g
    }

    #[test]
    fn cut_plus_remainder_is_original() {
        let r = RectI::new(-70, -5, 100, 90);
        let orig = solid(r, [20000, 10000, 5000, 40000]);
        let mut src = orig.clone();
        let mut sel = MaskGrid::new_empty();
        for y in -10..100 {
            for x in -80..110 {
                sel.set(x, y, ((x * 7 + y * 3).rem_euclid(256)) as u8);
            }
        }
        sel.fill_rect(RectI::new(0, 0, 64, 64), 255);
        let cutp = cut(&mut src, &sel);
        for y in -10..100 {
            for x in -80..110 {
                let a = cutp.get_pixel(x, y);
                let b = src.get_pixel(x, y);
                let o = orig.get_pixel(x, y);
                for ch in 0..4 {
                    assert_eq!(a[ch] + b[ch], o[ch]);
                }
                assert!(valid(a) && valid(b));
            }
        }
        // Fully selected tile (0,0) was moved, not copied.
        assert!(cutp.shares_tile_with(&orig, TileCoord::new(0, 0)));
        assert!(src.tile(TileCoord::new(0, 0)).is_none());
    }

    #[test]
    fn paste_over_shares_and_composites() {
        let a = solid(RectI::new(0, 0, 64, 64), [0, 0, 0, 65535]);
        let b = solid(RectI::new(32, 0, 160, 10), [32768, 0, 0, 32768]);
        let mut d = a.clone();
        let dirty = paste_over(&mut d, &b);
        assert_eq!(dirty, RectI::new(32, 0, 160, 10));
        assert_eq!(d.get_pixel(40, 5), [32768, 0, 0, 65535]);
        assert_eq!(d.get_pixel(10, 5), [0, 0, 0, 65535]);
        assert!(d.shares_tile_with(&b, TileCoord::new(1, 0)));
    }

    #[test]
    fn composite_modes() {
        let mut g = solid(RectI::new(0, 0, 10, 10), [0, 0, 0, 32768]);
        let m = MaskGrid::new_rect(RectI::new(5, 0, 200, 10));
        let red = ColorRgba::new(1.0, 0.0, 0.0, 1.0);
        let mut lock = g.clone();
        let d = composite_mask_color(&mut lock, &m, red, 1.0, CompositeMode::AlphaLock, None);
        assert_eq!(lock.get_pixel(6, 1), [32768, 0, 0, 32768]);
        assert_eq!(lock.get_pixel(4, 1), [0, 0, 0, 32768]);
        assert_eq!(lock.get_pixel(50, 1), [0; 4]);
        assert!(!d.is_empty());
        let d = composite_mask_color(&mut g, &m, red, 1.0, CompositeMode::Over, None);
        assert_eq!(d, RectI::new(5, 0, 200, 10));
        assert_eq!(g.get_pixel(6, 1), [65535, 0, 0, 65535]);
        assert_eq!(g.get_pixel(150, 9), [65535, 0, 0, 65535]);
        assert_eq!(g.get_pixel(150, 10), [0; 4]);
        let d = composite_mask_color(&mut g, &MaskGrid::new_full(), red, 1.0, CompositeMode::Erase, None);
        assert!(!d.is_empty());
        assert_eq!(g.tile_count(), 0, "erased tiles are removed");
        // Unbounded mask + Over + clip.
        let mut h = TileGrid::new();
        composite_mask_color(
            &mut h,
            &MaskGrid::new_full(),
            red,
            0.5,
            CompositeMode::Over,
            Some(RectI::new(0, 0, 3, 3)),
        );
        assert_eq!(h.get_pixel(2, 2), [32768, 0, 0, 32768]);
        assert_eq!(h.content_bounds(), RectI::new(0, 0, 3, 3));
    }
}
