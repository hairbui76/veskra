//! Per-stroke sparse `u16` coverage accumulation buffer (64×64 tiles, copy-on-write).

use std::collections::BTreeMap;
use std::sync::Arc;

use veskra_raster::{TILE, TileCoord};

pub(crate) const T: usize = TILE as usize;
pub(crate) const COV_TILE_LEN: usize = T * T;

/// Sparse coverage grid; missing tiles are coverage 0. Cloning shares tiles (`Arc`), mutation
/// copies only touched tiles, so a prediction preview can branch off cheaply.
#[derive(Debug, Clone, Default)]
pub(crate) struct CoverageGrid {
    tiles: BTreeMap<TileCoord, Arc<Vec<u16>>>,
}

impl CoverageGrid {
    pub(crate) fn get(&self, x: i32, y: i32) -> u16 {
        let (c, lx, ly) = TileCoord::split(x, y);
        self.tiles.get(&c).map_or(0, |t| t[ly as usize * T + lx as usize])
    }

    pub(crate) fn tile(&self, c: TileCoord) -> Option<&Arc<Vec<u16>>> {
        self.tiles.get(&c)
    }

    pub(crate) fn tile_mut(&mut self, c: TileCoord) -> &mut Vec<u16> {
        Arc::make_mut(self.tiles.entry(c).or_insert_with(|| Arc::new(vec![0u16; COV_TILE_LEN])))
    }

    pub(crate) fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}
