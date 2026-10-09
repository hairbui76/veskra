//! Bucket fill (EDIT-01): bounded, non-recursive region finding plus color application.
//!
//! ## Tolerance metric
//! Each reference pixel is converted to **straight-alpha RGBA8**: `a8 = round(a·255/65535)`,
//! `c8 = round(round(c·65535/a)·255/65535)` (fully transparent pixels are `(0,0,0,0)`
//! regardless of stray color bits). Distance is the **maximum absolute channel difference
//! over R, G, B and A** (0..=255). A pixel matches the seed if
//! `distance <= round(tolerance · 255)`; `tolerance = 0` matches the exact 8-bit color,
//! `tolerance = 1` matches everything.
//!
//! ## Algorithm and bounds
//! * Contiguous mode: 4-connected span fill with an explicit FIFO of runs. A run is marked
//!   filled when discovered, so every pixel is marked once and every run queued once;
//!   time is O(bounds area), no recursion. FIFO keeps the live queue near the size of the
//!   fill front (small for mazes/spirals and dot screens).
//! * Non-contiguous mode: every matching pixel inside `bounds`.
//! * Work memory: two lazily allocated 1-bit planes over tiles in `bounds` (≤ 2 bits/pixel)
//!   plus the run queue. `bounds.area()` above [`FillLimits::max_pixels`] (default
//!   [`crate::MAX_RECT_PIXELS`]) returns [`RasterError::TooLarge`]; a queue above
//!   [`FillLimits::max_queue`] returns [`RasterError::FillStackLimit`]. Nothing panics.
//! * Selection: pixels with selection coverage 0 never match; output coverage is scaled by
//!   the selection value.
//! * Antialias: pixels outside the region but 8-adjacent to it get partial coverage
//!   `(255 - d) / (255 - threshold)` (d = distance to the seed color), so soft line-art
//!   edges are half-covered while solid lines stay uncovered.

use std::collections::{BTreeSet, VecDeque};

use veskra_core::{ColorRgba, RectI};

use crate::ops::{CompositeMode, composite_mask_color};
use crate::tile::tile_span;
use crate::{MAX_RECT_PIXELS, MaskGrid, Px16, RasterError, TILE_I, TILE_U, TileCoord, TileGrid};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillOptions {
    /// `0..=1`, see module docs for the metric. NaN is rejected.
    pub tolerance: f32,
    /// `true`: 4-connected region around the seed; `false`: every matching pixel in bounds.
    pub contiguous: bool,
    /// Add a 1-px soft edge outside the region.
    pub antialias: bool,
}

impl Default for FillOptions {
    fn default() -> Self {
        Self { tolerance: 0.0, contiguous: true, antialias: false }
    }
}

/// Hard limits for one fill call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillLimits {
    /// Maximum `bounds.area()`.
    pub max_pixels: u64,
    /// Maximum simultaneously queued runs (12 bytes each).
    pub max_queue: usize,
}

impl Default for FillLimits {
    fn default() -> Self {
        Self { max_pixels: MAX_RECT_PIXELS, max_queue: 1 << 22 }
    }
}

/// Diagnostics for memory/termination tests (PNT-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FillStats {
    pub filled_pixels: u64,
    pub max_queue_len: usize,
    pub runs_processed: u64,
    /// Peak bytes of fill working memory (bit planes + queue + tile index).
    pub work_bytes: usize,
}

/// Compute the fill region as a coverage mask (see module docs).
/// A seed outside `bounds` or outside the selection yields an empty mask.
pub fn region(
    reference: &TileGrid,
    seed: (i32, i32),
    opts: &FillOptions,
    bounds: RectI,
    selection: Option<&MaskGrid>,
) -> Result<MaskGrid, RasterError> {
    region_with_limits(reference, seed, opts, bounds, selection, FillLimits::default()).map(|(m, _)| m)
}

/// [`region`] with explicit limits, also returning work statistics.
pub fn region_with_limits(
    reference: &TileGrid,
    seed: (i32, i32),
    opts: &FillOptions,
    bounds: RectI,
    selection: Option<&MaskGrid>,
    limits: FillLimits,
) -> Result<(MaskGrid, FillStats), RasterError> {
    if !opts.tolerance.is_finite() {
        return Err(RasterError::InvalidArgument("fill tolerance must be finite"));
    }
    if bounds.area() > limits.max_pixels {
        return Err(RasterError::TooLarge { pixels: bounds.area(), limit: limits.max_pixels });
    }
    let mut stats = FillStats::default();
    if bounds.is_empty() || !bounds.contains(seed.0, seed.1) {
        return Ok((MaskGrid::new_empty(), stats));
    }
    let sel_at = |x: i32, y: i32| selection.map_or(255, |s| s.get(x, y));
    if sel_at(seed.0, seed.1) == 0 {
        return Ok((MaskGrid::new_empty(), stats));
    }
    let thr = (opts.tolerance.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
    let mut ctx = Ctx {
        reference,
        selection,
        bounds,
        seed_key: straight8(reference.get_pixel(seed.0, seed.1)),
        thr,
        span: tile_span(bounds),
        matched: Vec::new(),
        filled: Vec::new(),
    };
    let n = ctx.ntx() * ctx.nty();
    ctx.matched.resize_with(n, || None);
    ctx.filled.resize_with(n, || None);

    if opts.contiguous {
        let mut q: VecDeque<Run> = VecDeque::new();
        ctx.discover(seed.0, seed.1, &mut q);
        stats.max_queue_len = q.len();
        while let Some(r) = q.pop_front() {
            stats.runs_processed += 1;
            for ny in [r.y - 1, r.y + 1] {
                if ny < bounds.y0 || ny >= bounds.y1 {
                    continue;
                }
                ctx.scan_row(ny, r.x0, r.x1, &mut q, limits.max_queue)?;
            }
            stats.max_queue_len = stats.max_queue_len.max(q.len());
        }
    } else {
        let (tx0, ty0, tx1, ty1) = ctx.span;
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let i = ctx.idx(tx, ty);
                let m = ctx.match_tile(tx, ty);
                if m.iter().any(|&b| b != 0) {
                    ctx.filled[i] = Some(Box::new(m));
                }
            }
        }
    }

    let mut out = MaskGrid::new_empty();
    let (tx0, ty0, tx1, ty1) = ctx.span;
    for ty in ty0..ty1 {
        for tx in tx0..tx1 {
            let Some(bits) = &ctx.filled[ctx.idx(tx, ty)] else { continue };
            let c = TileCoord::new(tx, ty);
            let (ox, oy) = (tx * TILE_I, ty * TILE_I);
            let mut any = false;
            for &row in bits.iter() {
                if row != 0 {
                    any = true;
                    stats.filled_pixels += row.count_ones() as u64;
                }
            }
            if !any {
                continue;
            }
            let t = out.tile_mut(c);
            for (ly, &row) in bits.iter().enumerate() {
                let mut r = row;
                while r != 0 {
                    let lx = r.trailing_zeros() as usize;
                    r &= r - 1;
                    t.data_mut()[ly * TILE_U + lx] = sel_at(ox + lx as i32, oy + ly as i32);
                }
            }
        }
    }

    if opts.antialias && thr < 255 {
        ctx.antialias_ring(&mut out);
    }
    let planes = ctx.matched.iter().chain(ctx.filled.iter()).filter(|p| p.is_some()).count();
    stats.work_bytes = planes * std::mem::size_of::<[u64; 64]>()
        + n * 2 * std::mem::size_of::<Option<Box<[u64; 64]>>>()
        + stats.max_queue_len * std::mem::size_of::<Run>();
    out.compact();
    Ok((out, stats))
}

/// Fill `target` with `color` through `region` coverage (src-over; `alpha_lock` uses
/// src-atop so destination alpha is preserved). Returns the dirty rectangle.
pub fn apply(target: &mut TileGrid, region: &MaskGrid, color: ColorRgba, alpha_lock: bool) -> RectI {
    let mode = if alpha_lock { CompositeMode::AlphaLock } else { CompositeMode::Over };
    composite_mask_color(target, region, color, 1.0, mode, None)
}

/// Straight-alpha 8-bit key of a premultiplied pixel (see module docs).
pub fn straight8(p: Px16) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0; 4];
    }
    let to8 = |v: u32| ((v * 255 + 32767) / 65535) as u8;
    let un = |c: u16| ((c as u32).min(a) * 65535 + a / 2) / a;
    [to8(un(p[0])), to8(un(p[1])), to8(un(p[2])), to8(a)]
}

/// Max channel distance between two straight RGBA8 keys.
pub fn distance(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..4).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
struct Run {
    y: i32,
    x0: i32,
    x1: i32,
}

type Bits = [u64; 64];

struct Ctx<'a> {
    reference: &'a TileGrid,
    selection: Option<&'a MaskGrid>,
    bounds: RectI,
    seed_key: [u8; 4],
    thr: u8,
    span: (i32, i32, i32, i32),
    matched: Vec<Option<Box<Bits>>>,
    filled: Vec<Option<Box<Bits>>>,
}

impl Ctx<'_> {
    fn ntx(&self) -> usize {
        (self.span.2 - self.span.0) as usize
    }
    fn nty(&self) -> usize {
        (self.span.3 - self.span.1) as usize
    }
    fn idx(&self, tx: i32, ty: i32) -> usize {
        (ty - self.span.1) as usize * self.ntx() + (tx - self.span.0) as usize
    }
    fn in_span(&self, tx: i32, ty: i32) -> bool {
        tx >= self.span.0 && tx < self.span.2 && ty >= self.span.1 && ty < self.span.3
    }

    /// Match bits for one tile (pixels outside bounds or unselected never match).
    fn match_tile(&self, tx: i32, ty: i32) -> Bits {
        let c = TileCoord::new(tx, ty);
        let tr = c.rect();
        let ir = tr.intersect(&self.bounds);
        let mut bits = [0u64; 64];
        let rt = self.reference.tile(c);
        let st = self.selection.and_then(|s| s.tile(c));
        let sdef = self.selection.map_or(255, |s| s.default_value());
        let transparent_matches = distance([0; 4], self.seed_key) <= self.thr;
        for y in ir.y0..ir.y1 {
            let ly = (y - tr.y0) as usize;
            let mut row = 0u64;
            for x in ir.x0..ir.x1 {
                let lx = (x - tr.x0) as usize;
                let sel = st.map_or(sdef, |t| t.data()[ly * TILE_U + lx]);
                if sel == 0 {
                    continue;
                }
                let m = match rt {
                    None => transparent_matches,
                    Some(t) => distance(straight8(t.get(lx as u32, ly as u32)), self.seed_key) <= self.thr,
                };
                if m {
                    row |= 1 << lx;
                }
            }
            bits[ly] = row;
        }
        bits
    }

    /// Bits of row `y` in tile column `tx` that match and are not yet filled
    /// (bit `lx` = pixel `tx*64 + lx`). Pixels outside `bounds` never match.
    fn avail(&mut self, tx: i32, y: i32) -> u64 {
        let (ty, ly) = (y.div_euclid(TILE_I), y.rem_euclid(TILE_I) as usize);
        if !self.in_span(tx, ty) {
            return 0;
        }
        let i = self.idx(tx, ty);
        if self.matched[i].is_none() {
            let b = self.match_tile(tx, ty);
            self.matched[i] = Some(Box::new(b));
        }
        let m = self.matched[i].as_ref().map_or(0, |b| b[ly]);
        let f = self.filled[i].as_ref().map_or(0, |b| b[ly]);
        m & !f
    }

    /// Mark pixels `[x0, x1)` of row `y` filled (word-wise).
    fn mark(&mut self, y: i32, x0: i32, x1: i32) {
        let (ty, ly) = (y.div_euclid(TILE_I), y.rem_euclid(TILE_I) as usize);
        let mut x = x0;
        while x < x1 {
            let (tx, lx) = (x.div_euclid(TILE_I), x.rem_euclid(TILE_I) as u32);
            let n = (64 - lx).min((x1 - x) as u32);
            let mask = if n == 64 { u64::MAX } else { ((1u64 << n) - 1) << lx };
            let i = self.idx(tx, ty);
            self.filled[i].get_or_insert_with(|| Box::new([0; 64]))[ly] |= mask;
            x += n as i32;
        }
    }

    /// `(x, y)` is available: extend to the maximal run (word-wise bit scans), mark it,
    /// queue it. Returns the exclusive right end of the run.
    fn discover(&mut self, x: i32, y: i32, q: &mut VecDeque<Run>) -> i32 {
        // Right.
        let (mut tx, mut lx) = (x.div_euclid(TILE_I), x.rem_euclid(TILE_I) as u32);
        let mut x1 = x;
        loop {
            let a = self.avail(tx, y) >> lx;
            let run = (!a).trailing_zeros().min(64 - lx);
            x1 += run as i32;
            if lx + run < 64 {
                break;
            }
            tx += 1;
            lx = 0;
        }
        // Left.
        let (mut tx, mut lx) = (x.div_euclid(TILE_I), x.rem_euclid(TILE_I) as u32);
        let mut x0 = x;
        loop {
            if lx == 0 {
                tx -= 1;
                lx = 64;
            }
            let a = self.avail(tx, y);
            let b = if lx == 64 { a } else { a << (64 - lx) };
            let run = (!b).leading_zeros().min(lx);
            x0 -= run as i32;
            if run < lx {
                break;
            }
            lx = 0;
        }
        self.mark(y, x0, x1);
        q.push_back(Run { y, x0, x1 });
        x1
    }

    /// Discover every available run of row `y` that overlaps `[x0, x1)`.
    fn scan_row(
        &mut self,
        y: i32,
        x0: i32,
        x1: i32,
        q: &mut VecDeque<Run>,
        max_queue: usize,
    ) -> Result<(), RasterError> {
        let mut x = x0;
        while x < x1 {
            let (tx, lx) = (x.div_euclid(TILE_I), x.rem_euclid(TILE_I) as u32);
            let n = (64 - lx).min((x1 - x) as u32);
            let seg = if n == 64 { u64::MAX } else { ((1u64 << n) - 1) << lx };
            let a = self.avail(tx, y) & seg;
            if a == 0 {
                x += n as i32;
                continue;
            }
            let start = tx * TILE_I + a.trailing_zeros() as i32;
            x = self.discover(start, y, q);
            if q.len() > max_queue {
                return Err(RasterError::FillStackLimit { limit: max_queue });
            }
        }
        Ok(())
    }

    /// Filled bits of row `ly` (-1..=64) of tile (tx, ty) extended by one pixel on each side:
    /// bit `lx + 1` for `lx` in -1..=64.
    fn ext_row(&self, tx: i32, ty: i32, ly: i32) -> u128 {
        let (ty, ly) = if ly < 0 {
            (ty - 1, 63)
        } else if ly > 63 {
            (ty + 1, 0)
        } else {
            (ty, ly)
        };
        let row = |tx: i32| -> u64 {
            if !self.in_span(tx, ty) {
                return 0;
            }
            self.filled[self.idx(tx, ty)].as_ref().map_or(0, |b| b[ly as usize])
        };
        ((row(tx - 1) >> 63) as u128) | ((row(tx) as u128) << 1) | (((row(tx + 1) & 1) as u128) << 65)
    }

    fn antialias_ring(&self, out: &mut MaskGrid) {
        let (tx0, ty0, tx1, ty1) = self.span;
        let mut cand = BTreeSet::new();
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                if self.filled[self.idx(tx, ty)].as_ref().is_some_and(|b| b.iter().any(|&r| r != 0)) {
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if self.in_span(tx + dx, ty + dy) {
                                cand.insert(TileCoord::new(tx + dx, ty + dy));
                            }
                        }
                    }
                }
            }
        }
        let span = 255 - self.thr as u32;
        for c in cand {
            let tr = c.rect();
            let ir = tr.intersect(&self.bounds);
            let rows: Vec<u128> = (-1..=64).map(|ly| self.ext_row(c.tx, c.ty, ly)).collect();
            for y in ir.y0..ir.y1 {
                let ly = (y - tr.y0) as usize; // rows[ly + 1] is this row
                let mut dil = 0u128;
                for r in &rows[ly..ly + 3] {
                    dil |= r | (r << 1) | (r >> 1);
                }
                let own = rows[ly + 1];
                let mut cbits = ((dil & !own) >> 1) as u64;
                while cbits != 0 {
                    let lx = cbits.trailing_zeros() as i32;
                    cbits &= cbits - 1;
                    let x = tr.x0 + lx;
                    if x < ir.x0 || x >= ir.x1 {
                        continue;
                    }
                    let sel = self.selection.map_or(255, |s| s.get(x, y)) as u32;
                    if sel == 0 {
                        continue;
                    }
                    let d = distance(straight8(self.reference.get_pixel(x, y)), self.seed_key) as u32;
                    if d <= self.thr as u32 {
                        // Matching but not connected (contiguous mode): do not leak into it.
                        continue;
                    }
                    let cov = ((255 - d) * 255 + span / 2) / span;
                    let v = (cov * sel + 127) / 255;
                    if v > 0 {
                        out.set(x, y, v as u8);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::color_to_px16;

    const B: RectI = RectI::new(0, 0, 256, 256);
    const BLACK: Px16 = [0, 0, 0, 65535];

    fn count(m: &MaskGrid) -> u64 {
        m.iter_tiles().map(|(_, t)| t.data().iter().filter(|&&v| v == 255).count() as u64).sum()
    }

    fn opts(tol: f32, contiguous: bool) -> FillOptions {
        FillOptions { tolerance: tol, contiguous, antialias: false }
    }

    /// Box outline (1px black) at r.
    fn outline(g: &mut TileGrid, r: RectI) {
        for x in r.x0..r.x1 {
            g.put_pixel(x, r.y0, BLACK);
            g.put_pixel(x, r.y1 - 1, BLACK);
        }
        for y in r.y0..r.y1 {
            g.put_pixel(r.x0, y, BLACK);
            g.put_pixel(r.x1 - 1, y, BLACK);
        }
    }

    #[test]
    fn straight_key_and_distance() {
        assert_eq!(straight8([0; 4]), [0; 4]);
        assert_eq!(straight8([5, 5, 5, 0]), [0; 4], "garbage under zero alpha ignored");
        assert_eq!(straight8([32768, 0, 0, 32768]), [255, 0, 0, 128]);
        assert_eq!(straight8(BLACK), [0, 0, 0, 255]);
        assert_eq!(distance([10, 20, 30, 40], [12, 15, 30, 40]), 5);
    }

    #[test]
    fn contiguous_fill_inside_outline_crosses_tiles_and_negatives() {
        let mut g = TileGrid::new();
        let r = RectI::new(-70, -10, 100, 90);
        outline(&mut g, r);
        let bounds = RectI::new(-200, -200, 200, 200);
        let m = region(&g, (0, 0), &opts(0.0, true), bounds, None).unwrap();
        assert_eq!(count(&m), (r.width() as u64 - 2) * (r.height() as u64 - 2));
        assert_eq!(m.bounds(), RectI::new(r.x0 + 1, r.y0 + 1, r.x1 - 1, r.y1 - 1));
        // Outside seed fills the rest of the bounds but not the inside.
        let m2 = region(&g, (-150, -150), &opts(0.0, true), bounds, None).unwrap();
        assert_eq!(count(&m2), bounds.area() - r.area());
        assert_eq!(m2.get(0, 0), 0);
    }

    #[test]
    fn tolerance_edges() {
        let mut g = TileGrid::new();
        // Gray ramp: column x has straight gray value x (8-bit), opaque.
        for x in 0..256 {
            for y in 0..4 {
                let v = (x as u16) * 257;
                g.put_pixel(x, y, [v, v, v, 65535]);
            }
        }
        let b = RectI::new(0, 0, 256, 4);
        let seed = (100, 0);
        // tolerance 10/255 → columns 90..=110 exactly.
        let m = region(&g, seed, &opts(10.0 / 255.0, true), b, None).unwrap();
        assert_eq!(m.bounds(), RectI::new(90, 0, 111, 4));
        let m = region(&g, seed, &opts(0.0, true), b, None).unwrap();
        assert_eq!(m.bounds(), RectI::new(100, 0, 101, 4));
        let m = region(&g, seed, &opts(1.0, true), b, None).unwrap();
        assert_eq!(count(&m), b.area());
        assert!(region(&g, seed, &opts(f32::NAN, true), b, None).is_err());
        // Transparent vs. semi-transparent: alpha participates in distance.
        let mut h = TileGrid::new();
        h.put_pixel(1, 0, [0, 0, 0, 257 * 20]);
        let m = region(&h, (0, 0), &opts(19.0 / 255.0, true), RectI::new(0, 0, 3, 1), None).unwrap();
        assert_eq!(m.get(1, 0), 0);
        let m = region(&h, (0, 0), &opts(20.0 / 255.0, true), RectI::new(0, 0, 3, 1), None).unwrap();
        assert_eq!(m.get(2, 0), 255);
    }

    #[test]
    fn contiguous_vs_global() {
        let mut g = TileGrid::new();
        outline(&mut g, RectI::new(10, 10, 50, 50));
        outline(&mut g, RectI::new(100, 100, 140, 140));
        let c = region(&g, (20, 20), &opts(0.0, true), B, None).unwrap();
        assert_eq!(count(&c), 38 * 38);
        assert_eq!(c.get(120, 120), 0);
        let nc = region(&g, (20, 20), &opts(0.0, false), B, None).unwrap();
        assert_eq!(count(&nc), B.area() - 2 * (40 * 4 - 4));
        assert_eq!(nc.get(120, 120), 255);
        assert_eq!(nc.get(10, 10), 0);
    }

    #[test]
    fn restricted_by_selection_and_bounds() {
        let g = TileGrid::new();
        let mut sel = MaskGrid::new_rect(RectI::new(0, 0, 100, 100));
        sel.fill_rect(RectI::new(0, 0, 10, 10), 128);
        let m = region(&g, (50, 50), &opts(0.0, true), B, Some(&sel)).unwrap();
        assert_eq!(m.bounds(), RectI::new(0, 0, 100, 100));
        assert_eq!(m.get(5, 5), 128);
        assert_eq!(m.get(50, 50), 255);
        // Seed outside selection / bounds → empty.
        assert!(region(&g, (150, 150), &opts(0.0, true), B, Some(&sel)).unwrap().is_empty());
        assert!(region(&g, (-1, 0), &opts(0.0, true), B, None).unwrap().is_empty());
        // Empty canvas fill is clipped to bounds even though transparent extends forever.
        let m = region(&g, (3, 3), &opts(0.0, true), RectI::new(-5, -5, 30, 7), None).unwrap();
        assert_eq!(m.bounds(), RectI::new(-5, -5, 30, 7));
    }

    #[test]
    fn checkerboard_terminates_with_single_pixel() {
        let mut g = TileGrid::new();
        for y in 0..256 {
            for x in 0..256 {
                if (x + y) % 2 == 0 {
                    g.put_pixel(x, y, BLACK);
                }
            }
        }
        let (m, st) = region_with_limits(&g, (1, 0), &opts(0.0, true), B, None, FillLimits::default()).unwrap();
        assert_eq!(st.filled_pixels, 1);
        assert_eq!(count(&m), 1);
        let (m, _) = region_with_limits(&g, (1, 0), &opts(0.0, false), B, None, FillLimits::default()).unwrap();
        assert_eq!(count(&m), 256 * 128);
    }

    /// Square spiral maze: a spiral wall drawn by a turtle (segment lengths n-1, n-1, n-1,
    /// n-3, n-3, n-5, n-5, …), leaving one 1-px corridor spiralling into the center.
    fn spiral(n: i32) -> (TileGrid, Vec<bool>) {
        let mut wall = vec![false; (n * n) as usize];
        let (mut x, mut y) = (0i32, 0i32);
        wall[0] = true;
        let dirs = [(1, 0), (0, 1), (-1, 0), (0, -1)];
        let mut lens = vec![n - 1];
        let mut l = n - 1;
        while l > 0 {
            lens.push(l);
            lens.push(l);
            l -= 2;
        }
        for (k, len) in lens.into_iter().enumerate() {
            let (dx, dy) = dirs[k % 4];
            for _ in 0..len {
                x += dx;
                y += dy;
                wall[(y * n + x) as usize] = true;
            }
        }
        let mut g = TileGrid::new();
        for y in 0..n {
            for x in 0..n {
                if wall[(y * n + x) as usize] {
                    g.put_pixel(x, y, BLACK);
                }
            }
        }
        (g, wall)
    }

    /// Independent oracle: naive per-pixel 4-connected BFS over a dense wall map.
    fn oracle_fill(wall: &[bool], n: i32, seed: (i32, i32)) -> Vec<bool> {
        let mut seen = vec![false; wall.len()];
        let mut q = VecDeque::from([seed]);
        seen[(seed.1 * n + seed.0) as usize] = true;
        while let Some((x, y)) = q.pop_front() {
            for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if nx < 0 || ny < 0 || nx >= n || ny >= n {
                    continue;
                }
                let i = (ny * n + nx) as usize;
                if !wall[i] && !seen[i] {
                    seen[i] = true;
                    q.push_back((nx, ny));
                }
            }
        }
        seen
    }

    #[test]
    fn random_walls_match_oracle_bfs_across_negative_tiles() {
        let (n, off) = (200i32, -137i32);
        let mut st = 0x9E37_79B9_7F4A_7C15u64;
        for density in [30u64, 45, 55] {
            let mut wall = vec![false; (n * n) as usize];
            let mut g = TileGrid::new();
            for y in 0..n {
                for x in 0..n {
                    st ^= st << 13;
                    st ^= st >> 7;
                    st ^= st << 17;
                    if st % 100 < density {
                        wall[(y * n + x) as usize] = true;
                        g.put_pixel(x + off, y + off, BLACK);
                    }
                }
            }
            let b = RectI::new(off, off, off + n, off + n);
            for seed in [(0, 0), (n / 2, n / 3), (n - 1, n - 1), (63, 64)] {
                let si = (seed.1 * n + seed.0) as usize;
                let m = region(&g, (seed.0 + off, seed.1 + off), &opts(0.0, true), b, None).unwrap();
                if wall[si] {
                    // Seeding on a wall fills the connected wall component instead.
                    let inv: Vec<bool> = wall.iter().map(|w| !w).collect();
                    let want = oracle_fill(&inv, n, seed);
                    for (i, &w) in want.iter().enumerate() {
                        let (x, y) = (i as i32 % n + off, i as i32 / n + off);
                        assert_eq!(m.get(x, y) == 255, w);
                    }
                    continue;
                }
                let want = oracle_fill(&wall, n, seed);
                for (i, &w) in want.iter().enumerate() {
                    let (x, y) = (i as i32 % n + off, i as i32 / n + off);
                    assert_eq!(m.get(x, y) == 255, w, "density {density} seed {seed:?} at ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn spiral_maze_terminates_with_small_queue() {
        let n = 255;
        let (g, wall) = spiral(n);
        assert_eq!(g.get_pixel(1, 1), [0; 4]);
        assert_eq!(g.get_pixel(1, 2), BLACK);
        let b = RectI::new(0, 0, n, n);
        let (m, st) = region_with_limits(&g, (1, 1), &opts(0.0, true), b, None, FillLimits::default()).unwrap();
        let want = oracle_fill(&wall, n, (1, 1));
        let free = want.iter().filter(|&&v| v).count() as u64;
        assert!(free > 20_000, "corridor spirals deep: {free}");
        assert_eq!(st.filled_pixels, free);
        assert_eq!(st.filled_pixels, count(&m));
        assert!(st.max_queue_len < 64, "queue {}", st.max_queue_len);
        for y in 0..n {
            for x in 0..n {
                assert_eq!(m.get(x, y) == 255, want[(y * n + x) as usize], "({x},{y})");
            }
        }
    }

    #[test]
    fn pnt008_bounded_memory_and_caps() {
        let g = TileGrid::new();
        let b = RectI::new(0, 0, 1024, 1024);
        let (m, st) = region_with_limits(&g, (0, 0), &opts(0.0, true), b, None, FillLimits::default()).unwrap();
        assert_eq!(st.filled_pixels, b.area());
        // ≤ 2 bits per pixel of bit planes + index + queue.
        let bound = (b.area() / 4) as usize + 256 * 2 * 8 + st.max_queue_len * 12;
        assert!(st.work_bytes <= bound, "{} > {}", st.work_bytes, bound);
        assert!(st.max_queue_len <= 4, "empty-canvas fill keeps a tiny queue: {}", st.max_queue_len);
        assert_eq!(m.byte_size(), 64 * 64, "uniform result tiles are shared");
        // Pixel cap → typed error before any allocation.
        let huge = RectI::new(0, 0, 10_000, 10_000);
        assert!(matches!(region(&g, (0, 0), &opts(0.0, true), huge, None), Err(RasterError::TooLarge { .. })));
        let small = FillLimits { max_pixels: 100, max_queue: 8 };
        assert!(matches!(
            region_with_limits(&g, (0, 0), &opts(0.0, true), RectI::new(0, 0, 11, 10), None, small),
            Err(RasterError::TooLarge { .. })
        ));
        // Queue cap → typed error (comb pattern produces many runs at once).
        let mut comb = TileGrid::new();
        for x in (0..256).step_by(2) {
            comb.put_pixel(x, 1, BLACK);
        }
        let tight = FillLimits { max_pixels: MAX_RECT_PIXELS, max_queue: 16 };
        let r = region_with_limits(&comb, (0, 0), &opts(0.0, true), RectI::new(0, 0, 256, 3), None, tight);
        assert!(matches!(r, Err(RasterError::FillStackLimit { limit: 16 })));
        let e: veskra_core::VeskraError = r.unwrap_err().into();
        assert_eq!(e.code, veskra_core::ErrorCode::AllocationLimit);
    }

    #[test]
    fn antialias_ring_covers_soft_edges_only() {
        let mut g = TileGrid::new();
        // Solid wall at x=10, half-alpha black at x=5.
        for y in 0..20 {
            g.put_pixel(10, y, BLACK);
            g.put_pixel(5, y, [0, 0, 0, 32768]);
        }
        let b = RectI::new(0, 0, 20, 20);
        let o = FillOptions { tolerance: 0.0, contiguous: true, antialias: true };
        let m = region(&g, (0, 0), &o, b, None).unwrap();
        assert_eq!(m.get(4, 3), 255);
        let v = m.get(5, 3);
        assert!((120..=135).contains(&v), "half-alpha edge half covered: {v}");
        assert_eq!(m.get(6, 3), 0, "ring is 1 px");
        let o2 = FillOptions { antialias: true, ..o };
        let m = region(&g, (7, 3), &o2, b, None).unwrap();
        assert_eq!(m.get(10, 3), 0, "solid line stays uncovered");
        assert_eq!(m.get(9, 3), 255);
    }

    #[test]
    fn apply_fills_and_alpha_lock() {
        let mut g = TileGrid::new();
        outline(&mut g, RectI::new(0, 0, 10, 10));
        let m = region(&g, (5, 5), &opts(0.0, true), RectI::new(0, 0, 10, 10), None).unwrap();
        let red = ColorRgba::new(1.0, 0.0, 0.0, 1.0);
        let mut t = g.clone();
        let dirty = apply(&mut t, &m, red, false);
        assert_eq!(dirty, RectI::new(1, 1, 9, 9));
        assert_eq!(t.get_pixel(5, 5), color_to_px16(red));
        assert_eq!(t.get_pixel(0, 0), BLACK);
        // Alpha lock on transparent interior paints nothing.
        let mut t2 = g.clone();
        apply(&mut t2, &m, red, true);
        assert_eq!(t2, g);
    }
}
