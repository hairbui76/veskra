//! RGBA16 premultiplied tiles and the sparse copy-on-write [`TileGrid`].

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use veskra_core::RectI;

use crate::{MAX_RECT_PIXELS, Px16, RasterError, TILE_I, TILE_U};

/// Pixels per tile (64 * 64).
pub const TILE_PIXELS: usize = TILE_U * TILE_U;
/// `u16` values per tile (RGBA).
pub const TILE_LEN: usize = TILE_PIXELS * 4;
/// Heap bytes of one tile's pixel payload.
pub const TILE_BYTES: usize = TILE_LEN * std::mem::size_of::<u16>();

/// Tile coordinate (`tx = floor(x / 64)`, `ty = floor(y / 64)`).
///
/// Ordering is **row-major `(ty, tx)`** so iteration over a `BTreeMap<TileCoord, _>` is
/// sorted top-to-bottom, left-to-right (CONTRACTS §2 `iter_tiles`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct TileCoord {
    pub tx: i32,
    pub ty: i32,
}

impl Ord for TileCoord {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.ty, self.tx).cmp(&(o.ty, o.tx))
    }
}

impl PartialOrd for TileCoord {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl TileCoord {
    pub const fn new(tx: i32, ty: i32) -> Self {
        Self { tx, ty }
    }

    /// Tile containing document pixel `(x, y)` (floor division, correct for negatives).
    pub const fn of_pixel(x: i32, y: i32) -> Self {
        Self { tx: x.div_euclid(TILE_I), ty: y.div_euclid(TILE_I) }
    }

    /// Tile containing `(x, y)` plus the pixel's local coordinate inside the tile (`0..64`).
    pub const fn split(x: i32, y: i32) -> (Self, u32, u32) {
        (Self::of_pixel(x, y), x.rem_euclid(TILE_I) as u32, y.rem_euclid(TILE_I) as u32)
    }

    /// Document pixel of this tile's top-left corner, as `i64` (never overflows).
    pub const fn origin_i64(self) -> (i64, i64) {
        (self.tx as i64 * TILE_I as i64, self.ty as i64 * TILE_I as i64)
    }

    /// Pixel rectangle covered by this tile (clamped to the `i32` range).
    pub fn rect(self) -> RectI {
        let (x, y) = self.origin_i64();
        let c = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        RectI::new(c(x), c(y), c(x + TILE_I as i64), c(y + TILE_I as i64))
    }
}

/// One 64×64 tile of premultiplied RGBA16 pixels, row-major, heap allocated.
#[derive(Clone, PartialEq, Eq)]
pub struct Tile {
    px: Box<[u16]>,
}

impl fmt::Debug for Tile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tile").field("transparent", &self.is_transparent()).finish()
    }
}

impl Default for Tile {
    fn default() -> Self {
        Self::new_transparent()
    }
}

impl Tile {
    pub fn new_transparent() -> Tile {
        Tile { px: vec![0u16; TILE_LEN].into_boxed_slice() }
    }

    /// Tile where every pixel is `p` (rgb clamped to alpha to keep the premultiplied invariant).
    pub fn new_filled(p: Px16) -> Tile {
        let p = clamp_premul(p);
        let mut v = Vec::with_capacity(TILE_LEN);
        for _ in 0..TILE_PIXELS {
            v.extend_from_slice(&p);
        }
        Tile { px: v.into_boxed_slice() }
    }

    /// Row-major RGBA, length [`TILE_LEN`].
    pub fn pixels(&self) -> &[u16] {
        &self.px
    }

    /// Mutable raw access. Callers must keep `r,g,b <= a`.
    pub fn pixels_mut(&mut self) -> &mut [u16] {
        &mut self.px
    }

    /// Pixel at local `(x, y)`; out-of-range coordinates return transparent (debug-asserted).
    pub fn get(&self, x: u32, y: u32) -> Px16 {
        debug_assert!(x < crate::TILE && y < crate::TILE, "tile-local coordinate out of range");
        if x >= crate::TILE || y >= crate::TILE {
            return [0; 4];
        }
        let i = (y as usize * TILE_U + x as usize) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    /// Store `p` at local `(x, y)` (rgb clamped to alpha). Out-of-range is ignored (debug-asserted).
    pub fn set(&mut self, x: u32, y: u32, p: Px16) {
        debug_assert!(x < crate::TILE && y < crate::TILE, "tile-local coordinate out of range");
        if x >= crate::TILE || y >= crate::TILE {
            return;
        }
        let i = (y as usize * TILE_U + x as usize) * 4;
        self.px[i..i + 4].copy_from_slice(&clamp_premul(p));
    }

    /// True if every channel of every pixel is zero.
    pub fn is_transparent(&self) -> bool {
        self.px.iter().all(|&v| v == 0)
    }

    /// True if every pixel equals `p`.
    pub fn is_uniform(&self, p: Px16) -> bool {
        self.px.chunks_exact(4).all(|c| c == p)
    }

    /// Build a tile from decoded data. Rejects wrong lengths and pixels violating
    /// `r,g,b <= a` (both indicate corrupt input) with a typed error.
    pub fn from_pixels(v: &[u16]) -> Result<Tile, RasterError> {
        if v.len() != TILE_LEN {
            return Err(RasterError::SizeMismatch { expected: TILE_LEN, got: v.len() });
        }
        if v.chunks_exact(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
            return Err(RasterError::InvalidTileData("premultiplied channel exceeds alpha"));
        }
        Ok(Tile { px: v.to_vec().into_boxed_slice() })
    }

    /// Like [`Tile::from_pixels`] but clamps channels exceeding alpha instead of rejecting.
    pub fn from_pixels_clamped(v: &[u16]) -> Result<Tile, RasterError> {
        if v.len() != TILE_LEN {
            return Err(RasterError::SizeMismatch { expected: TILE_LEN, got: v.len() });
        }
        let mut px = v.to_vec().into_boxed_slice();
        for p in px.chunks_exact_mut(4) {
            for i in 0..3 {
                p[i] = p[i].min(p[3]);
            }
        }
        Ok(Tile { px })
    }

    /// Tight local bounds `(x0, y0, x1, y1)` of non-zero pixels, or `None` if transparent.
    pub fn local_content_bounds(&self) -> Option<(u32, u32, u32, u32)> {
        let row_len = TILE_U * 4;
        let mut y0 = None;
        let mut y1 = 0;
        let (mut x0, mut x1) = (TILE_U, 0usize);
        for (y, row) in self.px.chunks_exact(row_len).enumerate() {
            let first = row.chunks_exact(4).position(|p| p != [0; 4]);
            if let Some(f) = first {
                let last = row.chunks_exact(4).rposition(|p| p != [0; 4]).unwrap_or(f);
                y0.get_or_insert(y);
                y1 = y + 1;
                x0 = x0.min(f);
                x1 = x1.max(last + 1);
            }
        }
        y0.map(|y0| (x0 as u32, y0 as u32, x1 as u32, y1 as u32))
    }
}

#[inline]
pub(crate) fn clamp_premul(p: Px16) -> Px16 {
    [p[0].min(p[3]), p[1].min(p[3]), p[2].min(p[3]), p[3]]
}

/// Sparse copy-on-write grid of [`Tile`]s. Missing tiles are fully transparent.
///
/// `Clone` is O(tiles) `Arc` clones; mutation copies only touched tiles. Equality compares
/// pixel content (a materialized transparent tile equals a missing tile).
#[derive(Clone, Default)]
pub struct TileGrid {
    tiles: BTreeMap<TileCoord, Arc<Tile>>,
}

impl fmt::Debug for TileGrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TileGrid").field("tiles", &self.tiles.len()).field("bounds", &self.content_bounds()).finish()
    }
}

impl PartialEq for TileGrid {
    fn eq(&self, other: &Self) -> bool {
        for (c, t) in &self.tiles {
            match other.tiles.get(c) {
                Some(o) => {
                    if !Arc::ptr_eq(t, o) && t.px != o.px {
                        return false;
                    }
                }
                None => {
                    if !t.is_transparent() {
                        return false;
                    }
                }
            }
        }
        other.tiles.iter().all(|(c, t)| self.tiles.contains_key(c) || t.is_transparent())
    }
}

impl Eq for TileGrid {}

impl TileGrid {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_pixel(&self, x: i32, y: i32) -> Px16 {
        let (c, lx, ly) = TileCoord::split(x, y);
        self.tiles.get(&c).map_or([0; 4], |t| t.get(lx, ly))
    }

    /// Store a pixel (rgb clamped to alpha). Writing transparent into a missing tile does
    /// not allocate.
    pub fn put_pixel(&mut self, x: i32, y: i32, p: Px16) {
        let (c, lx, ly) = TileCoord::split(x, y);
        if p == [0; 4] && !self.tiles.contains_key(&c) {
            return;
        }
        self.tile_mut(c).set(lx, ly, p);
    }

    pub fn tile(&self, c: TileCoord) -> Option<&Arc<Tile>> {
        self.tiles.get(&c)
    }

    /// Mutable tile, allocating a transparent one if missing and un-sharing it (`Arc::make_mut`).
    pub fn tile_mut(&mut self, c: TileCoord) -> &mut Tile {
        Arc::make_mut(self.tiles.entry(c).or_insert_with(|| Arc::new(Tile::new_transparent())))
    }

    pub fn set_tile(&mut self, c: TileCoord, t: Arc<Tile>) {
        self.tiles.insert(c, t);
    }

    pub fn remove_tile(&mut self, c: TileCoord) {
        self.tiles.remove(&c);
    }

    /// Remove and return a tile.
    pub fn take_tile(&mut self, c: TileCoord) -> Option<Arc<Tile>> {
        self.tiles.remove(&c)
    }

    /// Materialized tiles sorted by `(ty, tx)`.
    pub fn iter_tiles(&self) -> impl Iterator<Item = (TileCoord, &Arc<Tile>)> {
        self.tiles.iter().map(|(c, t)| (*c, t))
    }

    /// Materialized tile coordinates sorted by `(ty, tx)`.
    pub fn coords(&self) -> impl Iterator<Item = TileCoord> + '_ {
        self.tiles.keys().copied()
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.values().all(|t| t.is_transparent())
    }

    /// Read `r` as `r.width * r.height * 4` row-major premultiplied RGBA16, transparent outside
    /// tiles. Returns an empty `Vec` if `r` is empty or larger than [`MAX_RECT_PIXELS`]
    /// (use [`TileGrid::try_read_rect`] to get a typed error instead).
    pub fn read_rect(&self, r: RectI) -> Vec<u16> {
        self.try_read_rect(r).unwrap_or_default()
    }

    /// Checked variant of [`TileGrid::read_rect`].
    pub fn try_read_rect(&self, r: RectI) -> Result<Vec<u16>, RasterError> {
        if r.is_empty() {
            return Ok(Vec::new());
        }
        if r.area() > MAX_RECT_PIXELS {
            return Err(RasterError::TooLarge { pixels: r.area(), limit: MAX_RECT_PIXELS });
        }
        let w = r.width() as usize;
        let mut out = vec![0u16; r.area() as usize * 4];
        for (c, t) in self.tiles_overlapping(r) {
            let tr = c.rect().intersect(&r);
            let (ox, oy) = c.origin_i64();
            let lx0 = (tr.x0 as i64 - ox) as usize;
            let n = tr.width() as usize * 4;
            for y in tr.y0..tr.y1 {
                let ly = (y as i64 - oy) as usize;
                let src = &t.px[(ly * TILE_U + lx0) * 4..][..n];
                let di = ((y - r.y0) as usize * w + (tr.x0 - r.x0) as usize) * 4;
                out[di..di + n].copy_from_slice(src);
            }
        }
        Ok(out)
    }

    /// Write `data` (`r.width * r.height * 4` premultiplied RGBA16) into `r`.
    ///
    /// Only tiles intersecting `r` are touched; a tile whose content would not change is left
    /// shared (not copied), and all-transparent data never allocates a missing tile.
    /// Channels exceeding alpha are clamped to alpha.
    pub fn write_rect(&mut self, r: RectI, data: &[u16]) -> Result<(), RasterError> {
        let expected = (r.area() as usize)
            .checked_mul(4)
            .ok_or(RasterError::TooLarge { pixels: r.area(), limit: MAX_RECT_PIXELS })?;
        if data.len() != expected {
            return Err(RasterError::SizeMismatch { expected, got: data.len() });
        }
        if r.is_empty() {
            return Ok(());
        }
        let w = r.width() as usize;
        for c in Self::tiles_in_rect(r) {
            let tr = c.rect().intersect(&r);
            let (ox, oy) = c.origin_i64();
            let lx0 = (tr.x0 as i64 - ox) as usize;
            let n = tr.width() as usize * 4;
            let src_row = |y: i32| {
                let si = ((y - r.y0) as usize * w + (tr.x0 - r.x0) as usize) * 4;
                &data[si..si + n]
            };
            let changed = match self.tiles.get(&c) {
                None => (tr.y0..tr.y1).any(|y| src_row(y).iter().any(|&v| v != 0)),
                Some(t) => (tr.y0..tr.y1).any(|y| {
                    let ly = (y as i64 - oy) as usize;
                    &t.px[(ly * TILE_U + lx0) * 4..][..n] != src_row(y)
                }),
            };
            if !changed {
                continue;
            }
            let tile = self.tile_mut(c);
            for y in tr.y0..tr.y1 {
                let ly = (y as i64 - oy) as usize;
                let dst = &mut tile.px[(ly * TILE_U + lx0) * 4..][..n];
                for (d, s) in dst.chunks_exact_mut(4).zip(src_row(y).chunks_exact(4)) {
                    d.copy_from_slice(&clamp_premul([s[0], s[1], s[2], s[3]]));
                }
            }
        }
        Ok(())
    }

    /// Tight bounds of non-transparent pixels (`RectI::EMPTY` if none).
    pub fn content_bounds(&self) -> RectI {
        let mut b = RectI::EMPTY;
        for (c, t) in &self.tiles {
            let tr = c.rect();
            if !b.is_empty() && b.intersect(&tr) == tr {
                continue;
            }
            if let Some((x0, y0, x1, y1)) = t.local_content_bounds() {
                let r = RectI::new(tr.x0 + x0 as i32, tr.y0 + y0 as i32, tr.x0 + x1 as i32, tr.y0 + y1 as i32);
                b = b.union(&r);
            }
        }
        b
    }

    /// Union of the rectangles of all materialized tiles (`RectI::EMPTY` if none).
    pub fn tile_bounds(&self) -> RectI {
        self.tiles.keys().fold(RectI::EMPTY, |b, c| b.union(&c.rect()))
    }

    /// Drop materialized tiles whose pixels are all zero.
    pub fn prune_transparent(&mut self) {
        self.tiles.retain(|_, t| !t.is_transparent());
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    /// `tile_count * TILE_BYTES` (payload bytes; shared tiles counted once per grid entry).
    pub fn byte_size(&self) -> usize {
        self.tiles.len() * TILE_BYTES
    }

    /// Tile coordinates intersecting `r`, sorted by `(ty, tx)`. Empty for an empty rect.
    pub fn tiles_in_rect(r: RectI) -> impl Iterator<Item = TileCoord> {
        let (tx0, ty0, tx1, ty1) = tile_span(r);
        (ty0..ty1).flat_map(move |ty| (tx0..tx1).map(move |tx| TileCoord { tx, ty }))
    }

    /// Materialized tiles intersecting `r`, sorted. Cost is O(materialized tiles in range),
    /// independent of the rectangle's area.
    pub fn tiles_overlapping(&self, r: RectI) -> impl Iterator<Item = (TileCoord, &Arc<Tile>)> {
        let (tx0, ty0, tx1, ty1) = tile_span(r);
        let range = if ty1 > ty0 && tx1 > tx0 {
            self.tiles.range(TileCoord::new(tx0, ty0)..TileCoord::new(i32::MIN, ty1))
        } else {
            self.tiles.range(TileCoord::new(0, 0)..TileCoord::new(0, 0))
        };
        range.filter(move |(c, _)| c.tx >= tx0 && c.tx < tx1 && c.ty < ty1).map(|(c, t)| (*c, t))
    }

    /// True if both grids hold the *same allocation* for tile `c` (`Arc::ptr_eq`).
    /// False if either side is missing the tile.
    pub fn shares_tile_with(&self, other: &TileGrid, c: TileCoord) -> bool {
        match (self.tiles.get(&c), other.tiles.get(&c)) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Number of tiles in `self` that are not the same allocation as `other`'s tile at the
    /// same coordinate (undo memory accounting: the extra tiles a snapshot keeps alive).
    pub fn unshared_tile_count(&self, other: &TileGrid) -> usize {
        self.tiles.iter().filter(|(c, t)| other.tiles.get(c).is_none_or(|o| !Arc::ptr_eq(t, o))).count()
    }

    /// `unshared_tile_count(other) * TILE_BYTES`.
    pub fn unshared_byte_size(&self, other: &TileGrid) -> usize {
        self.unshared_tile_count(other) * TILE_BYTES
    }
}

/// Exclusive tile index span `(tx0, ty0, tx1, ty1)` covering a pixel rect.
pub(crate) fn tile_span(r: RectI) -> (i32, i32, i32, i32) {
    if r.is_empty() {
        return (0, 0, 0, 0);
    }
    (
        r.x0.div_euclid(TILE_I),
        r.y0.div_euclid(TILE_I),
        (r.x1 - 1).div_euclid(TILE_I) + 1,
        (r.y1 - 1).div_euclid(TILE_I) + 1,
    )
}
