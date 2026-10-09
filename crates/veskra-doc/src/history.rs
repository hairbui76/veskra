//! Snapshot history (DOC-04): copy-on-write document states with a bounded memory budget.
//!
//! Each entry stores the complete document state *before* a committed command. Because tiles
//! and layers are shared by `Arc`, an entry only costs the tiles that differ from its successor.
//! When the budget is exceeded the **oldest** entries are dropped and the drop is reported via
//! [`HistoryStats::trimmed_entries`] so the UI can tell the user (never a silent wrong undo).

use crate::Document;
use std::collections::VecDeque;
use std::sync::Arc;
use veskra_core::TILE_SIZE;

const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE * 4 * 2) as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryConfig {
    pub max_entries: usize,
    /// Budget for tiles held only by history (not by the current document).
    pub max_bytes: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { max_entries: 200, max_bytes: 384 * 1024 * 1024 }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    state: Arc<Document>,
    /// Bytes of tiles in `state` not shared with the following state.
    unique_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HistoryStats {
    pub undo_len: usize,
    pub redo_len: usize,
    pub bytes: usize,
    /// Total entries dropped because of the budget since creation.
    pub trimmed_entries: u64,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    config: HistoryConfig,
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    trimmed: u64,
}

/// Bytes of tiles held by `a` that are not shared (pointer-equal) with `b`.
pub(crate) fn unique_tile_bytes(a: &Document, b: &Document) -> usize {
    let mut bytes = 0usize;
    for la in a.iter_layers() {
        let Some(pa) = la.pixels() else { continue };
        match b.layer_arc(la.id) {
            Some(lb_arc) if a.layer_arc(la.id).is_some_and(|x| Arc::ptr_eq(x, lb_arc)) => {}
            Some(lb_arc) => {
                if let Some(pb) = lb_arc.pixels() {
                    for (c, _) in pa.tiles.iter_tiles() {
                        if !pa.tiles.shares_tile_with(&pb.tiles, c) {
                            bytes += TILE_BYTES;
                        }
                    }
                } else {
                    bytes += pa.tiles.byte_size();
                }
            }
            None => bytes += pa.tiles.byte_size(),
        }
    }
    bytes
}

impl History {
    pub fn new(config: HistoryConfig) -> Self {
        Self { config, ..Default::default() }
    }

    /// Record a committed command: `before` is the state prior to it, `after` the new current state.
    /// Clears the redo stack. Returns the number of entries trimmed by this call.
    pub fn record(&mut self, label: impl Into<String>, before: Arc<Document>, after: &Document) -> usize {
        let unique_bytes = unique_tile_bytes(&before, after);
        // The newest existing entry's successor was `before`, which is unchanged — no recount needed.
        self.undo.push_back(Entry { label: label.into(), state: before, unique_bytes });
        self.redo.clear();
        self.enforce_budget()
    }

    fn total_bytes(&self) -> usize {
        self.undo.iter().chain(self.redo.iter()).map(|e| e.unique_bytes).sum()
    }

    fn enforce_budget(&mut self) -> usize {
        let mut trimmed = 0;
        while self.undo.len() > 1
            && (self.undo.len() > self.config.max_entries || self.total_bytes() > self.config.max_bytes)
        {
            self.undo.pop_front();
            trimmed += 1;
        }
        self.trimmed += trimmed as u64;
        trimmed
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(|e| e.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    /// Step back: returns the state to make current, taking ownership of `current` for redo.
    pub fn undo(&mut self, current: Arc<Document>) -> Option<(String, Arc<Document>)> {
        let e = self.undo.pop_back()?;
        let unique = unique_tile_bytes(&current, &e.state);
        self.redo.push(Entry { label: e.label.clone(), state: current, unique_bytes: unique });
        Some((e.label, e.state))
    }

    /// Step forward: inverse of [`History::undo`].
    pub fn redo(&mut self, current: Arc<Document>) -> Option<(String, Arc<Document>)> {
        let e = self.redo.pop()?;
        let unique = unique_tile_bytes(&current, &e.state);
        self.undo.push_back(Entry { label: e.label.clone(), state: current, unique_bytes: unique });
        Some((e.label, e.state))
    }

    /// Drop all history (e.g. after opening a document).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn stats(&self) -> HistoryStats {
        HistoryStats {
            undo_len: self.undo.len(),
            redo_len: self.redo.len(),
            bytes: self.total_bytes(),
            trimmed_entries: self.trimmed,
        }
    }
}
