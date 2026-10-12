//! Per-stroke sparse accumulation buffers (64×64 tiles, copy-on-write): `u16` coverage, and for
//! the brush-v2 features an `u8` wet-edge depth and a straight `u16` RGB color per pixel.

use std::collections::BTreeMap;
use std::sync::Arc;

use veskra_raster::{TILE, TileCoord};

pub(crate) const T: usize = TILE as usize;
pub(crate) const COV_TILE_LEN: usize = T * T;

/// Sparse grid; missing tiles read as `T::default()`. Cloning shares tiles (`Arc`), mutation
/// copies only touched tiles, so a prediction preview can branch off cheaply.
#[derive(Debug, Clone)]
pub(crate) struct Grid<V> {
    tiles: BTreeMap<TileCoord, Arc<Vec<V>>>,
}

impl<V> Default for Grid<V> {
    fn default() -> Self {
        Self { tiles: BTreeMap::new() }
    }
}

impl<V: Copy + Default> Grid<V> {
    pub(crate) fn get(&self, x: i32, y: i32) -> V {
        let (c, lx, ly) = TileCoord::split(x, y);
        self.tiles.get(&c).map_or(V::default(), |t| t[ly as usize * T + lx as usize])
    }

    pub(crate) fn tile(&self, c: TileCoord) -> Option<&Arc<Vec<V>>> {
        self.tiles.get(&c)
    }

    pub(crate) fn tile_mut(&mut self, c: TileCoord) -> &mut Vec<V> {
        Arc::make_mut(self.tiles.entry(c).or_insert_with(|| Arc::new(vec![V::default(); COV_TILE_LEN])))
    }

    pub(crate) fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

/// Accumulated stroke coverage `0..=65535`.
pub(crate) type CoverageGrid = Grid<u16>;
/// Wet-edge depth: max over dabs of how deep inside a dab the pixel lies (`255` = center).
pub(crate) type DepthGrid = Grid<u8>;
/// Coverage-weighted straight RGB (encoded, `0..=65535`) of per-dab color jitter.
pub(crate) type ColorGrid = Grid<[u16; 3]>;
