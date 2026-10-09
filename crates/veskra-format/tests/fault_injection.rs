//! SAV-003 (simulated kill during temp write) and SAV-005 (storage full) via the
//! `save_atomic_with` seam: the original file always stays intact and loadable, no temp files
//! are left behind, and errors are typed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use veskra_core::{ErrorCode, LayerId, VeskraError};
use veskra_format::{FormatError, SaveSink, encode, load, remove_stale_temps, save_atomic, save_atomic_with};
use veskra_raster::TileCoord;

#[derive(Clone, Copy, Debug)]
enum Fault {
    /// `write` fails once `limit` bytes have been accepted (partial data stays on disk).
    WriteAfter { limit: u64, enospc: bool },
    /// All writes succeed but `sync_all` fails.
    Sync,
    /// Storage silently corrupts one byte at `at` (bit rot / bad provider).
    Flip { at: u64 },
}

struct FaultySink {
    f: File,
    written: u64,
    fault: Fault,
}

impl Write for FaultySink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.fault {
            Fault::WriteAfter { limit, enospc } => {
                let room = limit.saturating_sub(self.written);
                if room == 0 {
                    return Err(if enospc { io::Error::from_raw_os_error(28) } else { io::Error::other("killed") });
                }
                let n = (buf.len() as u64).min(room) as usize;
                self.f.write_all(&buf[..n])?;
                self.written += n as u64;
                Ok(n)
            }
            Fault::Flip { at } => {
                let mut v = buf.to_vec();
                if at >= self.written && at < self.written + v.len() as u64 {
                    v[(at - self.written) as usize] ^= 0x40;
                }
                self.f.write_all(&v)?;
                self.written += v.len() as u64;
                Ok(v.len())
            }
            Fault::Sync => {
                self.f.write_all(buf)?;
                self.written += buf.len() as u64;
                Ok(buf.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}

impl SaveSink for FaultySink {
    fn sync_all(&mut self) -> io::Result<()> {
        match self.fault {
            Fault::Sync => Err(io::Error::other("fsync failed")),
            _ => self.f.sync_all(),
        }
    }
}

fn opener(fault: Fault) -> impl FnOnce(&Path) -> io::Result<Box<dyn SaveSink>> {
    move |p: &Path| {
        let f = OpenOptions::new().write(true).create_new(true).open(p)?;
        Ok(Box::new(FaultySink { f, written: 0, fault }) as Box<dyn SaveSink>)
    }
}

fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-format-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn dir_names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    v.sort();
    v
}

fn edited(doc: &veskra_doc::Document) -> veskra_doc::Document {
    let mut d = doc.clone();
    paint_disc(&mut d, LayerId::from_u128(0x1001), TileCoord::new(1, 1), [1, 2, 3]);
    d
}

#[test]
fn sav003_sav005_failures_at_every_stage_keep_original_intact() {
    let dir = tempdir("faults");
    let target = dir.join("art.veskra");
    let original = simple_document();
    let report = save_atomic(&original, &target, &meta(), None).unwrap();
    let original_bytes = fs::read(&target).unwrap();
    assert_eq!(report.bytes, original_bytes.len() as u64);
    assert_eq!(report.sha256, hex(&sha(&original_bytes)));

    let next = edited(&original);
    let full_len = encode(&next, &meta(), None).unwrap().len() as u64;
    let mut cases = 0;
    let mut storage_full = 0;
    let mut limit = 0;
    while limit < full_len {
        for enospc in [true, false] {
            let err = save_atomic_with(&next, &target, &meta(), None, opener(Fault::WriteAfter { limit, enospc }))
                .unwrap_err();
            if enospc {
                assert!(matches!(err, FormatError::StorageFull), "limit {limit}: {err:?}");
                assert_eq!(VeskraError::from(err).code, ErrorCode::StorageFull);
                storage_full += 1;
            } else {
                assert!(matches!(err, FormatError::Io { .. }), "limit {limit}: {err:?}");
            }
            assert_eq!(fs::read(&target).unwrap(), original_bytes, "original modified at limit {limit}");
            assert_eq!(dir_names(&dir), vec!["art.veskra".to_string()], "temp leftover at limit {limit}");
            cases += 1;
        }
        limit += if limit < 512 { 7 } else { 37 };
    }
    // fsync failure and silent storage corruption (caught by re-read validation).
    let err = save_atomic_with(&next, &target, &meta(), None, opener(Fault::Sync)).unwrap_err();
    assert_eq!(VeskraError::from(err).code, ErrorCode::Io);
    let mut flips = 0;
    let mut at = 0;
    while at < full_len {
        let err = save_atomic_with(&next, &target, &meta(), None, opener(Fault::Flip { at })).unwrap_err();
        let code = VeskraError::from(err).code;
        assert!(matches!(code, ErrorCode::CorruptData | ErrorCode::CorruptTile), "flip {at}: {code:?}");
        flips += 1;
        at += 1499;
    }
    assert_eq!(fs::read(&target).unwrap(), original_bytes);
    assert_eq!(dir_names(&dir), vec!["art.veskra".to_string()]);
    assert_eq!(load(&target).unwrap().doc, original);
    assert!(cases >= 400 && storage_full >= 200 && flips >= 5, "cases {cases} flips {flips}");
    eprintln!("fault cases: {cases} write-limit ({storage_full} ENOSPC), 1 fsync, {flips} silent flips");

    // A successful save afterwards replaces the file atomically.
    save_atomic(&next, &target, &meta(), None).unwrap();
    assert_eq!(load(&target).unwrap().doc, next);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sav003_killed_process_leaves_only_ignorable_temp() {
    let dir = tempdir("kill");
    let target = dir.join("art.veskra");
    let original = simple_document();
    save_atomic(&original, &target, &meta(), None).unwrap();
    // Simulate a kill mid-write: a partial temp file that no cleanup code ever saw.
    let partial = encode(&edited(&original), &meta(), None).unwrap();
    fs::write(dir.join("art.veskra.tmp-deadbeef00000000"), &partial[..partial.len() / 2]).unwrap();
    assert_eq!(load(&target).unwrap().doc, original);
    assert!(load(&dir.join("art.veskra.tmp-deadbeef00000000")).is_err());
    assert_eq!(remove_stale_temps(&target).unwrap(), 1);
    assert_eq!(dir_names(&dir), vec!["art.veskra".to_string()]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn opener_failure_and_rename_failure_are_typed() {
    let dir = tempdir("open");
    let target = dir.join("art.veskra");
    let doc = simple_document();
    let err = save_atomic_with(&doc, &target, &meta(), None, |_p: &Path| -> io::Result<Box<dyn SaveSink>> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    })
    .unwrap_err();
    assert_eq!(VeskraError::from(err).code, ErrorCode::FilePermissionDenied);
    // Target is a non-empty directory: rename fails, nothing is left behind.
    let blocker = dir.join("dir.veskra");
    fs::create_dir_all(blocker.join("inner")).unwrap();
    assert!(save_atomic(&doc, &blocker, &meta(), None).is_err());
    assert!(blocker.join("inner").is_dir());
    assert_eq!(dir_names(&dir), vec!["dir.veskra".to_string()]);
    // Missing parent directory.
    assert!(save_atomic(&doc, &dir.join("nope/x.veskra"), &meta(), None).is_err());
    fs::remove_dir_all(&dir).unwrap();
}
