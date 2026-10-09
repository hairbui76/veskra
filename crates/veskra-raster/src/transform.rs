//! Affine resampling of pixels and masks (EDIT-03).
//!
//! Inverse mapping: each destination pixel center `(x + 0.5, y + 0.5)` is mapped through
//! `m⁻¹`, shifted by −0.5 to sample space and quantized to **1/256 px**; the result is a
//! bilinear blend of 4 source pixels with integer weights summing to 65536 and one
//! round-half-up shift. Consequences:
//! * identity, integer translations and 90°/180°/270° rotations onto the pixel grid are
//!   **bit-exact** (sub-1/512-px float noise quantizes to weight 0);
//! * premultiplied data is blended with identical weights per channel, so `r,g,b <= a` holds;
//! * only the destination bounding box of the transformed source bounds (± 1 px) ∩ `clip`
//!   is visited.
//!
//! Tile-aligned translations reuse the source tile allocations (no copy).

use std::borrow::Cow;
use std::sync::Arc;

use veskra_core::{Affine, PointF, RectF, RectI};

use crate::mask::MaskTile;
use crate::{MAX_RECT_PIXELS, MaskGrid, Px16, RasterError, TILE_I, TILE_U, Tile, TileCoord, TileGrid};

/// Sub-pixel quantization of sample positions (bits).
pub const SUBPIXEL_BITS: u32 = 8;
const ONE: i64 = 1 << SUBPIXEL_BITS;

/// Transform `src` (restricted to `region` coverage if given) by `m`, producing pixels only
/// inside `clip`. Returns an empty grid for a singular/non-finite `m` or when the
/// destination area exceeds [`MAX_RECT_PIXELS`] (use [`try_transform_pixels`] to get the error).
pub fn transform_pixels(src: &TileGrid, region: Option<&MaskGrid>, m: Affine, clip: RectI) -> TileGrid {
    try_transform_pixels(src, region, m, clip).unwrap_or_default()
}

/// Checked [`transform_pixels`]: `InvalidArgument` for singular/non-finite `m`,
/// `TooLarge` if the destination area exceeds [`MAX_RECT_PIXELS`].
pub fn try_transform_pixels(
    src: &TileGrid,
    region: Option<&MaskGrid>,
    m: Affine,
    clip: RectI,
) -> Result<TileGrid, RasterError> {
    let inv = checked_inverse(&m)?;
    let source: Cow<'_, TileGrid> = match region {
        Some(r) => Cow::Owned(crate::ops::mask_pixels(src, r)),
        None => Cow::Borrowed(src),
    };
    let sb = source.content_bounds();
    let mut out = TileGrid::new();
    if sb.is_empty() {
        return Ok(out);
    }
    let dbox = dest_box(&m, sb, clip)?;
    if dbox.is_empty() {
        return Ok(out);
    }
    // Fast path: tile-aligned integer translation moves shared tiles.
    if let Some((dx, dy)) = integer_translation(&m)
        && dx % TILE_I as i64 == 0
        && dy % TILE_I as i64 == 0
    {
        let (tdx, tdy) = ((dx / TILE_I as i64) as i32, (dy / TILE_I as i64) as i32);
        for (c, t) in source.iter_tiles() {
            let (Some(tx), Some(ty)) = (c.tx.checked_add(tdx), c.ty.checked_add(tdy)) else { continue };
            let nc = TileCoord::new(tx, ty);
            let nr = nc.rect();
            if nr.intersect(&clip) == nr {
                out.set_tile(nc, t.clone());
            } else if !nr.intersect(&clip).is_empty() {
                let ir = nr.intersect(&clip);
                let mut nt = Tile::new_transparent();
                for y in ir.y0..ir.y1 {
                    for x in ir.x0..ir.x1 {
                        nt.set((x - nr.x0) as u32, (y - nr.y0) as u32, t.get((x - nr.x0) as u32, (y - nr.y0) as u32));
                    }
                }
                if !nt.is_transparent() {
                    out.set_tile(nc, Arc::new(nt));
                }
            }
        }
        return Ok(out);
    }
    let lim = sb_limits(sb);
    let mut cache = Cache { grid: &source, key: None, tile: None };
    for c in TileGrid::tiles_in_rect(dbox) {
        let tr = c.rect();
        let ir = tr.intersect(&dbox);
        let mut tile: Option<Tile> = None;
        for y in ir.y0..ir.y1 {
            for x in ir.x0..ir.x1 {
                let Some((sx, sy)) = sample_pos(&inv, x, y, &lim) else { continue };
                let p = bilinear(sx, sy, |x, y| cache.get(x, y));
                if p != [0; 4] {
                    tile.get_or_insert_with(Tile::new_transparent).set((x - tr.x0) as u32, (y - tr.y0) as u32, p);
                }
            }
        }
        if let Some(t) = tile {
            out.set_tile(c, Arc::new(t));
        }
    }
    Ok(out)
}

/// Transform a coverage mask by `m` (bilinear). The default value outside materialized tiles
/// is preserved; for a nonzero default the whole `clip` is resampled.
/// Singular/non-finite `m` or an oversize destination returns a copy of the default-only mask.
pub fn transform_mask(src: &MaskGrid, m: Affine, clip: RectI) -> MaskGrid {
    try_transform_mask(src, m, clip)
        .unwrap_or_else(|_| if src.default_value() == 0 { MaskGrid::new_empty() } else { MaskGrid::new_full() })
}

/// Checked [`transform_mask`].
pub fn try_transform_mask(src: &MaskGrid, m: Affine, clip: RectI) -> Result<MaskGrid, RasterError> {
    let inv = checked_inverse(&m)?;
    let dflt = src.default_value();
    let mut out = if dflt == 0 { MaskGrid::new_empty() } else { MaskGrid::new_full() };
    let (dbox, lim) = if dflt == 0 {
        let sb = src.bounds();
        if sb.is_empty() {
            return Ok(out);
        }
        (dest_box(&m, sb, clip)?, Some(sb_limits(sb)))
    } else {
        if clip.area() > MAX_RECT_PIXELS {
            return Err(RasterError::TooLarge { pixels: clip.area(), limit: MAX_RECT_PIXELS });
        }
        (clip, None)
    };
    let fetch = |x: i64, y: i64| -> u32 {
        if x < i32::MIN as i64 || x > i32::MAX as i64 || y < i32::MIN as i64 || y > i32::MAX as i64 {
            return dflt as u32;
        }
        src.get(x as i32, y as i32) as u32
    };
    let unlimited = Limits { x0: f64::MIN, y0: f64::MIN, x1: f64::MAX, y1: f64::MAX };
    for c in TileGrid::tiles_in_rect(dbox) {
        let tr = c.rect();
        let ir = tr.intersect(&dbox);
        let mut t = MaskTile::new_filled(dflt);
        let mut changed = false;
        for y in ir.y0..ir.y1 {
            for x in ir.x0..ir.x1 {
                let v = match sample_pos(&inv, x, y, lim.as_ref().unwrap_or(&unlimited)) {
                    None => dflt,
                    Some((sx, sy)) => bilinear1(sx, sy, fetch) as u8,
                };
                if v != dflt {
                    changed = true;
                    t.data_mut()[(y - tr.y0) as usize * TILE_U + (x - tr.x0) as usize] = v;
                }
            }
        }
        if changed {
            out.set_tile(c, Arc::new(t));
        }
    }
    out.compact();
    Ok(out)
}

fn checked_inverse(m: &Affine) -> Result<Affine, RasterError> {
    if !m.is_finite() {
        return Err(RasterError::InvalidArgument("transform matrix is not finite"));
    }
    m.invert().ok_or(RasterError::InvalidArgument("transform matrix is singular"))
}

/// Destination pixels that can receive content from source bounds `sb` (±1 px) ∩ clip.
fn dest_box(m: &Affine, sb: RectI, clip: RectI) -> Result<RectI, RasterError> {
    let r = RectF::new(sb.x0 as f64 - 1.0, sb.y0 as f64 - 1.0, sb.x1 as f64 + 1.0, sb.y1 as f64 + 1.0);
    let d = RectI::covering(&m.transform_rect_bounds(&r)).intersect(&clip);
    if d.area() > MAX_RECT_PIXELS {
        return Err(RasterError::TooLarge { pixels: d.area(), limit: MAX_RECT_PIXELS });
    }
    Ok(d)
}

fn integer_translation(m: &Affine) -> Option<(i64, i64)> {
    let int = |v: f64| v.fract() == 0.0 && v.abs() < (1u64 << 40) as f64;
    (m.a == 1.0 && m.b == 0.0 && m.c == 0.0 && m.d == 1.0 && int(m.e) && int(m.f)).then_some((m.e as i64, m.f as i64))
}

struct Limits {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

/// Sample-space region outside which every bilinear tap is transparent.
fn sb_limits(sb: RectI) -> Limits {
    Limits { x0: sb.x0 as f64 - 1.0, y0: sb.y0 as f64 - 1.0, x1: sb.x1 as f64, y1: sb.y1 as f64 }
}

/// Quantized sample position (1/256 px fixed point) for destination pixel `(x, y)`.
fn sample_pos(inv: &Affine, x: i32, y: i32, lim: &Limits) -> Option<(i64, i64)> {
    let p = inv.apply(PointF::new(x as f64 + 0.5, y as f64 + 0.5));
    let (sx, sy) = (p.x - 0.5, p.y - 0.5);
    if !(sx > lim.x0 && sx < lim.x1 && sy > lim.y0 && sy < lim.y1) {
        return None;
    }
    if sx.abs() > 1e12 || sy.abs() > 1e12 {
        return None;
    }
    let q = |v: f64| (v * ONE as f64).round() as i64;
    Some((q(sx), q(sy)))
}

fn bilinear(sx: i64, sy: i64, mut fetch: impl FnMut(i64, i64) -> Px16) -> Px16 {
    let (x0, y0) = (sx >> SUBPIXEL_BITS, sy >> SUBPIXEL_BITS);
    let (wx, wy) = ((sx & (ONE - 1)) as u64, (sy & (ONE - 1)) as u64);
    let one = ONE as u64;
    let mut acc = [0u64; 4];
    let mut tap = |x: i64, y: i64, w: u64| {
        if w != 0 {
            let p = fetch(x, y);
            for i in 0..4 {
                acc[i] += w * p[i] as u64;
            }
        }
    };
    tap(x0, y0, (one - wx) * (one - wy));
    tap(x0 + 1, y0, wx * (one - wy));
    tap(x0, y0 + 1, (one - wx) * wy);
    tap(x0 + 1, y0 + 1, wx * wy);
    let r = |v: u64| ((v + (1 << (2 * SUBPIXEL_BITS - 1))) >> (2 * SUBPIXEL_BITS)).min(65535) as u16;
    [r(acc[0]), r(acc[1]), r(acc[2]), r(acc[3])]
}

fn bilinear1(sx: i64, sy: i64, fetch: impl Fn(i64, i64) -> u32) -> u32 {
    let (x0, y0) = (sx >> SUBPIXEL_BITS, sy >> SUBPIXEL_BITS);
    let (wx, wy) = ((sx & (ONE - 1)) as u64, (sy & (ONE - 1)) as u64);
    let one = ONE as u64;
    let mut acc = 0u64;
    for (x, y, w) in [
        (x0, y0, (one - wx) * (one - wy)),
        (x0 + 1, y0, wx * (one - wy)),
        (x0, y0 + 1, (one - wx) * wy),
        (x0 + 1, y0 + 1, wx * wy),
    ] {
        if w != 0 {
            acc += w * fetch(x, y) as u64;
        }
    }
    (((acc + (1 << (2 * SUBPIXEL_BITS - 1))) >> (2 * SUBPIXEL_BITS)) as u32).min(255)
}

/// One-entry tile cache for source fetches.
struct Cache<'a> {
    grid: &'a TileGrid,
    key: Option<TileCoord>,
    tile: Option<&'a Arc<Tile>>,
}

impl Cache<'_> {
    fn get(&mut self, x: i64, y: i64) -> Px16 {
        if x < i32::MIN as i64 || x > i32::MAX as i64 || y < i32::MIN as i64 || y > i32::MAX as i64 {
            return [0; 4];
        }
        let (c, lx, ly) = TileCoord::split(x as i32, y as i32);
        if self.key != Some(c) {
            self.key = Some(c);
            self.tile = self.grid.tile(c);
        }
        self.tile.map_or([0; 4], |t| t.get(lx, ly))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    const CLIP: RectI = RectI::new(-512, -512, 512, 512);

    fn pattern(r: RectI, seed: u64) -> TileGrid {
        let mut g = TileGrid::new();
        let mut s = seed;
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let a = (s >> 16) as u16;
                let c = |k: u32| if a == 0 { 0 } else { ((s >> k) % (a as u64 + 1)) as u16 };
                g.put_pixel(x, y, [c(3), c(23), c(41), a]);
            }
        }
        g
    }

    fn valid(g: &TileGrid) -> bool {
        g.iter_tiles().all(|(_, t)| t.pixels().chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]))
    }

    #[test]
    fn identity_is_bit_exact() {
        let g = pattern(RectI::new(-70, -3, 90, 130), 7);
        assert_eq!(transform_pixels(&g, None, Affine::IDENTITY, CLIP), g);
    }

    #[test]
    fn integer_translation_is_bit_exact() {
        let g = pattern(RectI::new(-10, -10, 75, 40), 11);
        for (dx, dy) in [(3, -7), (-65, 1), (64, -128), (0, 1)] {
            let t = transform_pixels(&g, None, Affine::translate(dx as f64, dy as f64), CLIP);
            for y in -20..60 {
                for x in -20..90 {
                    assert_eq!(t.get_pixel(x + dx, y + dy), g.get_pixel(x, y), "d=({dx},{dy}) at ({x},{y})");
                }
            }
            assert_eq!(t.content_bounds(), RectI::new(-10 + dx, -10 + dy, 75 + dx, 40 + dy));
        }
        // Tile-aligned translation shares allocations.
        let t = transform_pixels(&g, None, Affine::translate(64.0, -128.0), CLIP);
        let moved = t.tile(TileCoord::new(1, -2)).unwrap();
        assert!(Arc::ptr_eq(moved, g.tile(TileCoord::new(0, 0)).unwrap()));
    }

    #[test]
    fn quarter_rotations_onto_grid_are_exact() {
        let g = pattern(RectI::new(0, 0, 50, 30), 3);
        // (x, y) -> (h - y, x) via rotate(90°) then translate.
        let r90 = Affine::rotate(FRAC_PI_2).then(&Affine::translate(30.0, 0.0));
        let t = transform_pixels(&g, None, r90, CLIP);
        for y in 0..30 {
            for x in 0..50 {
                assert_eq!(t.get_pixel(29 - y, x), g.get_pixel(x, y));
            }
        }
        let r180 = Affine::rotate(PI).then(&Affine::translate(50.0, 30.0));
        let t = transform_pixels(&g, None, r180, CLIP);
        for y in 0..30 {
            for x in 0..50 {
                assert_eq!(t.get_pixel(49 - x, 29 - y), g.get_pixel(x, y));
            }
        }
        // Mirror (negative determinant) is exact too.
        let flip = Affine::scale(-1.0, 1.0).then(&Affine::translate(50.0, 0.0));
        let t = transform_pixels(&g, None, flip, CLIP);
        assert_eq!(t.get_pixel(49, 7), g.get_pixel(0, 7));
        // Rotating about the origin maps pixel (x, y) to (-y-1, x): on the grid, so four
        // quarter turns give back the original bit-exactly.
        let back = (0..4).fold(g.clone(), |acc, _| transform_pixels(&acc, None, Affine::rotate(FRAC_PI_2), CLIP));
        assert_eq!(back, g);
    }

    #[test]
    fn arbitrary_transform_keeps_premultiplied_invariant_and_bounds() {
        let g = pattern(RectI::new(-20, -20, 40, 40), 5);
        let m = Affine::rotate(0.37).then(&Affine::scale(1.7, 0.6)).then(&Affine::translate(13.25, -4.5));
        let t = transform_pixels(&g, None, m, CLIP);
        assert!(valid(&t));
        let bb = RectI::covering(&m.transform_rect_bounds(&RectF::new(-21.0, -21.0, 41.0, 41.0)));
        let cb = t.content_bounds();
        assert_eq!(cb.intersect(&bb), cb);
        // Half-pixel shift averages neighbours exactly.
        let mut h = TileGrid::new();
        h.put_pixel(0, 0, [0, 0, 0, 65535]);
        h.put_pixel(1, 0, [0, 0, 0, 0]);
        let t = transform_pixels(&h, None, Affine::translate(0.5, 0.0), CLIP);
        assert_eq!(t.get_pixel(0, 0), [0, 0, 0, 32768]);
        assert_eq!(t.get_pixel(1, 0), [0, 0, 0, 32768]);
        // Clip restricts output; singular matrices give empty output / error.
        let t = transform_pixels(&g, None, Affine::IDENTITY, RectI::new(0, 0, 5, 5));
        assert_eq!(t.content_bounds(), RectI::new(0, 0, 5, 5));
        assert_eq!(transform_pixels(&g, None, Affine::scale(0.0, 1.0), CLIP).tile_count(), 0);
        assert!(try_transform_pixels(&g, None, Affine::scale(f64::NAN, 1.0), CLIP).is_err());
        let huge = RectI::new(-100_000, -100_000, 100_000, 100_000);
        assert!(matches!(
            try_transform_pixels(&g, None, Affine::scale(1000.0, 1000.0), huge),
            Err(RasterError::TooLarge { .. })
        ));
    }

    #[test]
    fn region_restricts_transformed_content() {
        let g = pattern(RectI::new(0, 0, 40, 40), 9);
        let sel = MaskGrid::new_rect(RectI::new(10, 10, 20, 20));
        let t = transform_pixels(&g, Some(&sel), Affine::translate(100.0, 0.0), CLIP);
        assert_eq!(t.content_bounds().intersect(&RectI::new(110, 10, 120, 20)), t.content_bounds());
        assert_eq!(t.get_pixel(115, 15), g.get_pixel(15, 15));
    }

    #[test]
    fn mask_transform() {
        let m = MaskGrid::new_rect(RectI::new(0, 0, 10, 4));
        assert_eq!(transform_mask(&m, Affine::IDENTITY, CLIP), m);
        assert_eq!(
            transform_mask(&m, Affine::translate(-70.0, 3.0), CLIP),
            MaskGrid::new_rect(RectI::new(-70, 3, -60, 7))
        );
        let r = transform_mask(&m, Affine::rotate(FRAC_PI_2).then(&Affine::translate(4.0, 0.0)), CLIP);
        assert_eq!(r, MaskGrid::new_rect(RectI::new(0, 0, 4, 10)));
        let s = transform_mask(&m, Affine::translate(0.5, 0.0), CLIP);
        assert_eq!(s.get(0, 0), 128);
        assert_eq!(s.get(10, 0), 128);
        // Full mask stays full.
        let f = transform_mask(&MaskGrid::new_full(), Affine::rotate(0.3), RectI::new(0, 0, 100, 100));
        assert_eq!(f.get(50, 50), 255);
        assert_eq!(f.get(5000, 5000), 255);
        assert!(transform_mask(&m, Affine::scale(0.0, 0.0), CLIP).is_empty());
    }
}
