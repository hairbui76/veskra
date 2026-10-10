//! FMT-03 (Rust side): bounded, validated autosave checkpoints.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use veskra_core::{DocumentId, LayerId};
use veskra_format::{AutosaveStore, SaveSink, save_atomic};
use veskra_raster::TileCoord;

/// "No space left on device" as the platform reports it: raw ENOSPC (28) on Unix, where the
/// engine also maps the raw code; the portable error kind elsewhere (28 means something else on
/// Windows).
fn storage_full_error() -> std::io::Error {
    if cfg!(unix) {
        std::io::Error::from_raw_os_error(28)
    } else {
        std::io::Error::from(std::io::ErrorKind::StorageFull)
    }
}

fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-recovery-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    d
}

fn version(n: u16) -> veskra_doc::Document {
    let mut d = simple_document();
    paint_disc(&mut d, LayerId::from_u128(0x1001), TileCoord::new(1, 0), [n, n, n]);
    d
}

#[test]
fn checkpoints_are_bounded_sequenced_and_never_touch_original() {
    let root = tempdir("bounded");
    let store = AutosaveStore::new(root.join("recovery"), 3).unwrap();
    let original_path = root.join("art.veskra");
    save_atomic(&version(0), &original_path, &meta(), None).unwrap();
    let original_bytes = fs::read(&original_path).unwrap();

    let mut last = None;
    for n in 1..=6u16 {
        let info = store.checkpoint(&version(n), &meta()).unwrap();
        assert_eq!(info.seq, n as u64);
        assert!(info.valid);
        last = Some(info);
    }
    let id = simple_document().id;
    let list = store.list_for(id).unwrap();
    assert_eq!(list.iter().map(|c| c.seq).collect::<Vec<_>>(), vec![6, 5, 4]);
    assert!(list.iter().all(|c| c.valid && c.doc_id == id));
    let latest = store.latest_valid(id).unwrap().unwrap();
    assert_eq!(Some(&latest), last.as_ref());
    assert_eq!(store.load(&latest).unwrap().doc, version(6));
    assert_eq!(fs::read(&original_path).unwrap(), original_bytes, "recovery touched the original");
    assert!(store.latest_valid(DocumentId::from_u128(42)).unwrap().is_none());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn corrupt_checkpoints_are_reported_not_deleted_and_skipped_for_recovery() {
    let root = tempdir("corrupt");
    let store = AutosaveStore::new(&root, 2).unwrap();
    let id = simple_document().id;
    store.checkpoint(&version(1), &meta()).unwrap();
    let c2 = store.checkpoint(&version(2), &meta()).unwrap();
    // Newest checkpoint gets damaged on disk (e.g. torn write outside our protocol).
    let mut bytes = fs::read(&c2.path).unwrap();
    bytes.truncate(bytes.len() - 10);
    fs::write(&c2.path, &bytes).unwrap();
    let latest = store.latest_valid(id).unwrap().unwrap();
    assert_eq!(latest.seq, 1);
    assert_eq!(store.load(&latest).unwrap().doc, version(1));
    let list = store.list().unwrap();
    assert_eq!(list.len(), 2);
    assert!(!list[1].valid && list[1].error.is_some());

    // New checkpoints continue after the corrupt one; pruning never removes the corrupt file
    // nor the last valid one before the new one is validated.
    let c3 = store.checkpoint(&version(3), &meta()).unwrap();
    assert_eq!(c3.seq, 3);
    let c4 = store.checkpoint(&version(4), &meta()).unwrap();
    let seqs: Vec<(u64, bool)> = store.list_for(id).unwrap().iter().map(|c| (c.seq, c.valid)).collect();
    assert_eq!(seqs, vec![(4, true), (3, true), (2, false)]);
    assert_eq!(store.latest_valid(id).unwrap().unwrap(), c4);
    // Explicit discard of the corrupt one.
    let bad = store.list_for(id).unwrap().into_iter().find(|c| !c.valid).unwrap();
    store.discard(&bad).unwrap();
    assert_eq!(store.list_for(id).unwrap().len(), 2);
    assert_eq!(store.clear(id).unwrap(), 2);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn failed_checkpoint_keeps_previous_and_leaves_no_temp() {
    let root = tempdir("fail");
    let store = AutosaveStore::new(&root, 1).unwrap();
    let id = simple_document().id;
    let c1 = store.checkpoint(&version(1), &meta()).unwrap();
    let full = |p: &Path| -> io::Result<Box<dyn SaveSink>> {
        let _f = OpenOptions::new().write(true).create_new(true).open(p)?;
        Err(storage_full_error())
    };
    let err = store.checkpoint_with(&version(2), &meta(), full).unwrap_err();
    assert!(matches!(err, veskra_format::FormatError::StorageFull));
    assert_eq!(store.latest_valid(id).unwrap().unwrap(), c1);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    // Temp files from a killed autosave are ignored by listing.
    fs::write(root.join(format!("{id}-0000000002.veskra.tmp-0011223344556677")), b"partial").unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    // Unrelated / foreign files are ignored too.
    fs::write(root.join("notes.txt"), b"x").unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn checkpoint_with_mismatched_doc_id_is_invalid() {
    let root = tempdir("mismatch");
    let store = AutosaveStore::new(&root, 3).unwrap();
    let c = store.checkpoint(&version(1), &meta()).unwrap();
    let other = DocumentId::from_u128(0xFEED);
    fs::copy(&c.path, root.join(format!("{other}-0000000001.veskra"))).unwrap();
    assert!(store.latest_valid(other).unwrap().is_none());
    let l = store.list_for(other).unwrap();
    assert_eq!(l.len(), 1);
    assert!(!l[0].valid);
    fs::remove_dir_all(&root).unwrap();
}
