//! Autosave checkpoints in an app-private recovery directory (ADR-009, FMT-03 Rust side).
//!
//! Each checkpoint is a complete `.veskra` bundle named `<docId>-<seq:010>.veskra`, written with
//! the same validated atomic protocol as [`crate::save_atomic`]. After a new checkpoint has been
//! written *and validated*, older **valid** checkpoints of the same document beyond the newest
//! `keep` are deleted. Invalid/corrupt checkpoint files are reported by [`AutosaveStore::list`]
//! and never deleted implicitly (only via [`AutosaveStore::discard`]). The store only ever
//! touches files inside its own directory, never the user's original document.
//!
//! Single writer: one `AutosaveStore` per directory per process (sequence numbers are derived
//! from a directory scan). Listing/pruning fully decodes each checkpoint, so cost is O(keep)
//! decodes per checkpoint.

use crate::codec::{LoadedDocument, SaveMeta, decode};
use crate::error::FormatError;
use crate::save::{TEMP_MARKER, open_temp_file, save_atomic_with};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use veskra_core::DocumentId;
use veskra_doc::Document;

const EXT: &str = ".veskra";
const SEQ_DIGITS: usize = 10;
const UUID_LEN: usize = 36;

/// One checkpoint file found in the recovery directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointInfo {
    pub path: PathBuf,
    pub doc_id: DocumentId,
    pub seq: u64,
    pub bytes: u64,
    /// The file decodes and validates completely and belongs to `doc_id`.
    pub valid: bool,
    /// Why the file is invalid (safe diagnostic, no paths).
    pub error: Option<String>,
}

/// Bounded autosave checkpoint store.
#[derive(Debug, Clone)]
pub struct AutosaveStore {
    dir: PathBuf,
    keep: usize,
}

fn parse_name(name: &str) -> Option<(DocumentId, u64)> {
    let stem = name.strip_suffix(EXT)?;
    if stem.len() != UUID_LEN + 1 + SEQ_DIGITS || !stem.is_ascii() {
        return None;
    }
    let (id, rest) = stem.split_at(UUID_LEN);
    let digits = rest.strip_prefix('-')?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((DocumentId::parse(id).ok()?, digits.parse().ok()?))
}

impl AutosaveStore {
    /// Open (creating if needed) a recovery directory keeping the newest `keep` valid
    /// checkpoints per document (`keep` is clamped to at least 1).
    pub fn new(dir: impl Into<PathBuf>, keep: usize) -> Result<Self, FormatError> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, keep: keep.max(1) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn scan(&self) -> Result<Vec<(PathBuf, DocumentId, u64, u64)>, FormatError> {
        let mut out = Vec::new();
        for e in fs::read_dir(&self.dir)? {
            let e = e?;
            let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
            if name.contains(TEMP_MARKER) || !e.file_type()?.is_file() {
                continue;
            }
            if let Some((id, seq)) = parse_name(&name) {
                out.push((e.path(), id, seq, e.metadata()?.len()));
            }
        }
        out.sort_by_key(|a| (a.1, a.2));
        Ok(out)
    }

    fn info(path: PathBuf, doc_id: DocumentId, seq: u64, bytes: u64) -> CheckpointInfo {
        let check = fs::read(&path).map_err(FormatError::from).and_then(|b| decode(&b));
        let (valid, error) = match check {
            Ok(l) if l.doc.id == doc_id => (true, None),
            Ok(_) => (false, Some("checkpoint belongs to a different document".to_string())),
            Err(e) => (false, Some(e.to_string())),
        };
        CheckpointInfo { path, doc_id, seq, bytes, valid, error }
    }

    /// All checkpoint files (every document), sorted by `(doc_id, seq)`, each fully validated.
    /// Temp files of interrupted writes are ignored.
    pub fn list(&self) -> Result<Vec<CheckpointInfo>, FormatError> {
        Ok(self.scan()?.into_iter().map(|(p, id, seq, n)| Self::info(p, id, seq, n)).collect())
    }

    /// Checkpoints of one document, newest first.
    pub fn list_for(&self, doc_id: DocumentId) -> Result<Vec<CheckpointInfo>, FormatError> {
        let mut v: Vec<CheckpointInfo> = self
            .scan()?
            .into_iter()
            .filter(|c| c.1 == doc_id)
            .map(|(p, id, seq, n)| Self::info(p, id, seq, n))
            .collect();
        v.reverse();
        Ok(v)
    }

    /// Newest valid checkpoint of `doc_id`, if any.
    pub fn latest_valid(&self, doc_id: DocumentId) -> Result<Option<CheckpointInfo>, FormatError> {
        for (p, id, seq, n) in self.scan()?.into_iter().rev().filter(|c| c.1 == doc_id) {
            let info = Self::info(p, id, seq, n);
            if info.valid {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    /// Load a checkpoint (full validation).
    pub fn load(&self, info: &CheckpointInfo) -> Result<LoadedDocument, FormatError> {
        crate::save::load(&info.path)
    }

    /// Write a new checkpoint of `doc` and prune older valid ones beyond `keep`.
    pub fn checkpoint(&self, doc: &Document, meta: &SaveMeta) -> Result<CheckpointInfo, FormatError> {
        self.checkpoint_with(doc, meta, open_temp_file)
    }

    /// [`AutosaveStore::checkpoint`] with an injectable temp opener (fault-injection seam).
    pub fn checkpoint_with<F>(
        &self,
        doc: &Document,
        meta: &SaveMeta,
        open_temp: F,
    ) -> Result<CheckpointInfo, FormatError>
    where
        F: FnOnce(&Path) -> io::Result<Box<dyn crate::save::SaveSink>>,
    {
        let existing = self.scan()?;
        // Next sequence after every existing file of this document (valid or not): never reuse a name.
        let seq = existing.iter().filter(|c| c.1 == doc.id).map(|c| c.2).max().map_or(1, |s| s.saturating_add(1));
        let path = self.dir.join(format!("{}-{seq:0width$}{EXT}", doc.id, width = SEQ_DIGITS));
        let report = save_atomic_with(doc, &path, meta, None, open_temp)?;
        let new = CheckpointInfo { path, doc_id: doc.id, seq, bytes: report.bytes, valid: true, error: None };

        // Prune: only after the new checkpoint is durable and validated; only valid, older files.
        let mut kept = 1usize;
        for (p, id, s, n) in existing.into_iter().rev().filter(|c| c.1 == doc.id) {
            let info = Self::info(p, id, s, n);
            if !info.valid {
                continue;
            }
            if kept < self.keep {
                kept += 1;
            } else {
                fs::remove_file(&info.path)?;
            }
        }
        Ok(new)
    }

    /// Explicitly delete one checkpoint file (e.g. user dismissed a corrupt one).
    pub fn discard(&self, info: &CheckpointInfo) -> Result<(), FormatError> {
        if info.path.parent() != Some(self.dir.as_path()) {
            return Err(FormatError::InvalidArgument("checkpoint is not in this recovery directory".into()));
        }
        fs::remove_file(&info.path)?;
        Ok(())
    }

    /// Remove every checkpoint of `doc_id` (after the user saved or discarded recovery).
    pub fn clear(&self, doc_id: DocumentId) -> Result<usize, FormatError> {
        let mut n = 0;
        for (p, ..) in self.scan()?.into_iter().filter(|c| c.1 == doc_id) {
            fs::remove_file(p)?;
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let id = DocumentId::from_u128(0xabc);
        let n = format!("{id}-{:010}.veskra", 42);
        assert_eq!(parse_name(&n), Some((id, 42)));
        assert_eq!(parse_name("x.veskra"), None);
        assert_eq!(parse_name(&format!("{id}-00000000x1.veskra")), None);
        assert_eq!(parse_name(&format!("{id}-0000000001.veskra.tmp-ab")), None);
    }
}
