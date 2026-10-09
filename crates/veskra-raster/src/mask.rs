//! Sparse copy-on-write `u8` coverage grid (selections, layer masks, fill regions).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use veskra_core::RectI;

use crate::tile::{TILE_PIXELS, tile_span};
use crate::{MAX_RECT_PIXELS, RasterError, TILE_U, TileCoord, TileGrid, UNBOUNDED};

/// One 64×64 tile of `u8` coverage, row-major.
#[derive(Clone, PartialEq, Eq)]
pub struct MaskTile {
    v: Box<[u8]>,
}

impl fmt::Debug for MaskTile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaskTile").finish_non_exhaustive()
    }
}

impl MaskTile {
    pub fn new_filled(v: u8) -> MaskTile {
        MaskTile { v: vec![v; TILE_PIXELS].into_boxed_slice() }
    }
    /// Row-major coverage, length 64*64.
    pub fn data(&self) -> &[u8] {
        &self.v
    }
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.v
    }
    pub fn get(&self, x: u32, y: u32) -> u8 {
        debug_assert!(x < crate::TILE && y < crate::TILE);
        self.v.get(y as usize * TILE_U + x as usize).copied().unwrap_or(0)
    }
    pub fn set(&mut self, x: u32, y: u32, v: u8) {
        debug_assert!(x < crate::TILE && y < crate::TILE);
        if let Some(p) = self.v.get_mut(y as usize * TILE_U + x as usize) {
            *p = v;
        }
    }
    /// `Some(v)` if every value equals `v`.
    pub fn uniform_value(&self) -> Option<u8> {
        let f = self.v[0];
        self.v.iter().all(|&x| x == f).then_some(f)
    }
    /// Build from decoded data (length must be 64*64).
    pub fn from_data(v: &[u8]) -> Result<MaskTile, RasterError> {
        if v.len() != TILE_PIXELS {
            return Err(RasterError::SizeMismatch { expected: TILE_PIXELS, got: v.len() });
        }
        Ok(MaskTile { v: v.to_vec().into_boxed_slice() })
    }
}

/// Shared all-`v` tile for `v` in {0, 255}; other values allocate.
pub(crate) fn uniform_mask_tile(v: u8) -> Arc<MaskTile> {
    static ZERO: OnceLock<Arc<MaskTile>> = OnceLock::new();
    static FULL: OnceLock<Arc<MaskTile>> = OnceLock::new();
    match v {
        0 => ZERO.get_or_init(|| Arc::new(MaskTile::new_filled(0))).clone(),
        255 => FULL.get_or_init(|| Arc::new(MaskTile::new_filled(255))).clone(),
        _ => Arc::new(MaskTile::new_filled(v)),
    }
}

/// How [`MaskGrid::combine`] merges `other` into `self` (fuzzy-set operators, idempotent):
/// `Replace = b`, `Add = max(a, b)`, `Subtract = min(a, 255 - b)`, `Intersect = min(a, b)`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MaskOp {
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl MaskOp {
    #[inline]
    pub fn apply(self, a: u8, b: u8) -> u8 {
        match self {
            MaskOp::Replace => b,
            MaskOp::Add => a.max(b),
            MaskOp::Subtract => a.min(255 - b),
            MaskOp::Intersect => a.min(b),
        }
    }
}

/// Sparse `u8` coverage grid with copy-on-write tiles and a default value (0 or 255) for
/// every pixel outside materialized tiles. `Default` is the empty mask.
#[derive(Clone)]
pub struct MaskGrid {
    tiles: BTreeMap<TileCoord, Arc<MaskTile>>,
    default: u8,
}

impl Default for MaskGrid {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl fmt::Debug for MaskGrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaskGrid")
            .field("default", &self.default)
            .field("tiles", &self.tiles.len())
            .field("bounds", &self.bounds())
            .finish()
    }
}

impl PartialEq for MaskGrid {
    fn eq(&self, other: &Self) -> bool {
        if self.default != other.default {
            return false;
        }
        let d = self.default;
        let eq_one = |a: &BTreeMap<TileCoord, Arc<MaskTile>>, b: &BTreeMap<TileCoord, Arc<MaskTile>>| {
            a.iter().all(|(c, t)| match b.get(c) {
                Some(o) => Arc::ptr_eq(t, o) || t.v == o.v,
                None => t.v.iter().all(|&v| v == d),
            })
        };
        eq_one(&self.tiles, &other.tiles) && eq_one(&other.tiles, &self.tiles)
    }
}

impl Eq for MaskGrid {}

impl MaskGrid {
    /// Coverage 0 everywhere.
    pub fn new_empty() -> Self {
        Self { tiles: BTreeMap::new(), default: 0 }
    }

    /// Coverage 255 everywhere ("select all" without bounds).
    pub fn new_full() -> Self {
        Self { tiles: BTreeMap::new(), default: 255 }
    }

    /// Coverage 255 inside `r`, 0 outside (interior tiles share one allocation).
    pub fn new_rect(r: RectI) -> Self {
        let mut m = Self::new_empty();
        m.fill_rect(r, 255);
        m
    }

    /// Value of every pixel outside materialized tiles.
    pub fn default_value(&self) -> u8 {
        self.default
    }

    pub fn get(&self, x: i32, y: i32) -> u8 {
        let (c, lx, ly) = TileCoord::split(x, y);
        self.tiles.get(&c).map_or(self.default, |t| t.get(lx, ly))
    }

    /// Set coverage; writing the default value into a missing tile does not allocate.
    pub fn set(&mut self, x: i32, y: i32, v: u8) {
        let (c, lx, ly) = TileCoord::split(x, y);
        if v == self.default && !self.tiles.contains_key(&c) {
            return;
        }
        self.tile_mut(c).set(lx, ly, v);
    }

    pub fn tile(&self, c: TileCoord) -> Option<&Arc<MaskTile>> {
        self.tiles.get(&c)
    }

    /// Mutable tile, materializing it filled with the default value.
    pub fn tile_mut(&mut self, c: TileCoord) -> &mut MaskTile {
        let d = self.default;
        Arc::make_mut(self.tiles.entry(c).or_insert_with(|| uniform_mask_tile(d)))
    }

    pub fn set_tile(&mut self, c: TileCoord, t: Arc<MaskTile>) {
        self.tiles.insert(c, t);
    }

    pub fn remove_tile(&mut self, c: TileCoord) {
        self.tiles.remove(&c);
    }

    /// Sorted by `(ty, tx)`.
    pub fn iter_tiles(&self) -> impl Iterator<Item = (TileCoord, &Arc<MaskTile>)> {
        self.tiles.iter().map(|(c, t)| (*c, t))
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Tight bounds of nonzero coverage. [`UNBOUNDED`] if the default value is nonzero;
    /// `RectI::EMPTY` if the mask is empty.
    pub fn bounds(&self) -> RectI {
        if self.default != 0 {
            return UNBOUNDED;
        }
        let mut b = RectI::EMPTY;
        for (c, t) in &self.tiles {
            let tr = c.rect();
            if !b.is_empty() && b.intersect(&tr) == tr {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (TILE_U, TILE_U, 0, 0);
            for (y, row) in t.v.chunks_exact(TILE_U).enumerate() {
                if let Some(f) = row.iter().position(|&v| v != 0) {
                    let l = row.iter().rposition(|&v| v != 0).unwrap_or(f);
                    y0 = y0.min(y);
                    y1 = y + 1;
                    x0 = x0.min(f);
                    x1 = x1.max(l + 1);
                }
            }
            if y1 > 0 {
                let r = RectI::new(tr.x0 + x0 as i32, tr.y0 + y0 as i32, tr.x0 + x1 as i32, tr.y0 + y1 as i32);
                b = b.union(&r);
            }
        }
        b
    }

    /// True if every pixel has coverage 0.
    pub fn is_empty(&self) -> bool {
        self.default == 0 && self.tiles.values().all(|t| t.v.iter().all(|&v| v == 0))
    }

    /// `v -> 255 - v` for every pixel inside `within` (outside is unchanged).
    /// Tiles fully inside `within` that are uniform become shared uniform tiles.
    /// `within == UNBOUNDED` inverts the whole plane (flips the default); any other rect is
    /// clamped to [`MASK_OP_LIMIT`].
    pub fn invert(&mut self, within: RectI) {
        if within == UNBOUNDED {
            self.default = 255 - self.default;
            for t in self.tiles.values_mut() {
                for v in Arc::make_mut(t).v.iter_mut() {
                    *v = 255 - *v;
                }
            }
            self.compact();
            return;
        }
        let within = within.intersect(&MASK_OP_LIMIT);
        if within.is_empty() {
            return;
        }
        for c in TileGrid::tiles_in_rect(within) {
            let tr = c.rect();
            let ir = tr.intersect(&within);
            if ir == tr {
                let uni = self.tiles.get(&c).map_or(Some(self.default), |t| t.uniform_value());
                if let Some(u) = uni {
                    self.tiles.insert(c, uniform_mask_tile(255 - u));
                    continue;
                }
            }
            let t = self.tile_mut(c);
            for_each_local(tr, ir, |i| t.v[i] = 255 - t.v[i]);
        }
        self.compact();
    }

    /// Set every pixel of `r` (clamped to [`MASK_OP_LIMIT`]) to `v`.
    pub fn fill_rect(&mut self, r: RectI, v: u8) {
        let r = r.intersect(&MASK_OP_LIMIT);
        if r.is_empty() {
            return;
        }
        for c in TileGrid::tiles_in_rect(r) {
            let tr = c.rect();
            let ir = tr.intersect(&r);
            if ir == tr {
                self.tiles.insert(c, uniform_mask_tile(v));
                continue;
            }
            let t = self.tile_mut(c);
            for_each_local(tr, ir, |i| t.v[i] = v);
        }
        self.compact();
    }

    /// Merge `other` into `self` per pixel with `op` (see [`MaskOp`]).
    pub fn combine(&mut self, other: &MaskGrid, op: MaskOp) {
        if op == MaskOp::Replace {
            *self = other.clone();
            return;
        }
        let (da, db) = (self.default, other.default);
        let new_default = op.apply(da, db);
        let mut coords: Vec<TileCoord> = self.tiles.keys().chain(other.tiles.keys()).copied().collect();
        coords.sort_unstable();
        coords.dedup();
        let mut out = BTreeMap::new();
        for c in coords {
            let a = self.tiles.get(&c);
            let b = other.tiles.get(&c);
            // Fast paths for absent/uniform tiles.
            let ua = a.map_or(Some(da), |t| t.uniform_value());
            let ub = b.map_or(Some(db), |t| t.uniform_value());
            let t = match (ua, ub) {
                (Some(x), Some(y)) => uniform_mask_tile(op.apply(x, y)),
                _ => {
                    let mut t = MaskTile::new_filled(0);
                    for (i, o) in t.v.iter_mut().enumerate() {
                        let x = a.map_or(da, |t| t.v[i]);
                        let y = b.map_or(db, |t| t.v[i]);
                        *o = op.apply(x, y);
                    }
                    Arc::new(t)
                }
            };
            out.insert(c, t);
        }
        self.tiles = out;
        self.default = new_default;
        self.compact();
    }

    /// Read `r` as `width * height` coverage values (default outside tiles). Empty `Vec` if
    /// `r` is empty or exceeds [`MAX_RECT_PIXELS`].
    pub fn read_rect(&self, r: RectI) -> Vec<u8> {
        if r.is_empty() || r.area() > MAX_RECT_PIXELS {
            return Vec::new();
        }
        let w = r.width() as usize;
        let mut out = vec![self.default; r.area() as usize];
        let (tx0, ty0, tx1, ty1) = tile_span(r);
        let range = self.tiles.range(TileCoord::new(tx0, ty0)..TileCoord::new(i32::MIN, ty1));
        for (c, t) in range.filter(|(c, _)| c.tx >= tx0 && c.tx < tx1) {
            let tr = c.rect().intersect(&r);
            let (ox, oy) = c.origin_i64();
            let lx0 = (tr.x0 as i64 - ox) as usize;
            let n = tr.width() as usize;
            for y in tr.y0..tr.y1 {
                let ly = (y as i64 - oy) as usize;
                let di = (y - r.y0) as usize * w + (tr.x0 - r.x0) as usize;
                out[di..di + n].copy_from_slice(&t.v[ly * TILE_U + lx0..][..n]);
            }
        }
        out
    }

    /// Write `width * height` coverage values into `r`.
    pub fn write_rect(&mut self, r: RectI, data: &[u8]) -> Result<(), RasterError> {
        let expected = r.area() as usize;
        if data.len() != expected {
            return Err(RasterError::SizeMismatch { expected, got: data.len() });
        }
        if r.is_empty() {
            return Ok(());
        }
        let w = r.width() as usize;
        for c in TileGrid::tiles_in_rect(r) {
            let tr = c.rect().intersect(&r);
            let n = tr.width() as usize;
            let row = |y: i32| &data[(y - r.y0) as usize * w + (tr.x0 - r.x0) as usize..][..n];
            if !self.tiles.contains_key(&c) && (tr.y0..tr.y1).all(|y| row(y).iter().all(|&v| v == self.default)) {
                continue;
            }
            let (ox, oy) = c.origin_i64();
            let lx0 = (tr.x0 as i64 - ox) as usize;
            let t = self.tile_mut(c);
            for y in tr.y0..tr.y1 {
                let ly = (y as i64 - oy) as usize;
                t.v[ly * TILE_U + lx0..][..n].copy_from_slice(row(y));
            }
        }
        Ok(())
    }

    /// Drop tiles equal to the default everywhere and replace uniform 0/255 tiles with the
    /// shared allocation (memory dedupe).
    pub fn compact(&mut self) {
        let d = self.default;
        self.tiles.retain(|_, t| t.uniform_value() != Some(d));
        for t in self.tiles.values_mut() {
            if let Some(u @ (0 | 255)) = t.uniform_value() {
                let shared = uniform_mask_tile(u);
                if !Arc::ptr_eq(t, &shared) {
                    *t = shared;
                }
            }
        }
    }

    /// Payload bytes of distinct tile allocations held by this grid.
    pub fn byte_size(&self) -> usize {
        let mut ptrs: Vec<*const MaskTile> = self.tiles.values().map(Arc::as_ptr).collect();
        ptrs.sort_unstable();
        ptrs.dedup();
        ptrs.len() * TILE_PIXELS
    }

    /// Same allocation for tile `c` in both grids (`Arc::ptr_eq`); false if either is missing.
    pub fn shares_tile_with(&self, other: &MaskGrid, c: TileCoord) -> bool {
        match (self.tiles.get(&c), other.tiles.get(&c)) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Tiles in `self` not shared with `other` at the same coordinate.
    pub fn unshared_tile_count(&self, other: &MaskGrid) -> usize {
        self.tiles.iter().filter(|(c, t)| other.tiles.get(c).is_none_or(|o| !Arc::ptr_eq(t, o))).count()
    }

    /// Scale every coverage value by `k / 255` (rounded), e.g. to apply feathering/opacity.
    pub fn scale(&mut self, k: u8) {
        if k == 255 {
            return;
        }
        let f = |v: u8| ((v as u32 * k as u32 + 127) / 255) as u8;
        self.default = f(self.default);
        for t in self.tiles.values_mut() {
            for v in Arc::make_mut(t).v.iter_mut() {
                *v = f(*v);
            }
        }
        self.compact();
    }
}

/// Region-wide mask edits ([`MaskGrid::invert`], [`MaskGrid::fill_rect`]) are clamped to
/// this rectangle (±32768 px, 2× the maximum document edge) so a pathological rect can never
/// enumerate billions of tiles.
pub const MASK_OP_LIMIT: RectI = RectI::new(-32_768, -32_768, 32_768, 32_768);

/// Call `f(local_index)` for each pixel of `ir` (a sub-rect of tile rect `tr`).
fn for_each_local(tr: RectI, ir: RectI, mut f: impl FnMut(usize)) {
    for y in ir.y0..ir.y1 {
        let ly = (y - tr.y0) as usize;
        for x in ir.x0..ir.x1 {
            f(ly * TILE_U + (x - tr.x0) as usize);
        }
    }
}
