//! Crash-safe staged save (ADR-009 save protocol, FMT-03 Rust side).
//!
//! `save_atomic`: encode to `<target>.tmp-<random>` in the target's directory → flush →
//! `fsync` the temp file → re-read it from disk and fully `decode` it (integrity hashes, tiles,
//! document validation) and compare its SHA-256 with what was written → `rename` over the target
//! (atomic on POSIX filesystems within one directory) → `fsync` the parent directory.
//!
//! On any failure the temp file is removed, the original target is left untouched and a typed
//! [`FormatError`] is returned (`StorageFull` for ENOSPC/EDQUOT).
//!
//! Durability notes (documented limitations):
//! * The parent-directory `fsync` is best effort: it is attempted on Unix and its failure is
//!   ignored, because some filesystems (and Android FUSE/SAF layers) reject directory fsync.
//!   Before that point the rename is atomic but may not yet be durable across power loss.
//! * Android SAF destinations are not handled here: Kotlin copies the validated temp/target
//!   file into the provider stream (FMT-04); providers may not replace atomically.
//! * A process kill between temp creation and cleanup can leave a `*.tmp-*` file; it is never
//!   mistaken for a document. [`remove_stale_temps`] deletes such leftovers.

use crate::codec::{LoadedDocument, SaveMeta, decode, encode_to, hex, sha256};
use crate::error::FormatError;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use veskra_doc::Document;

/// Marker inserted between the target file name and the random suffix of temp files.
pub const TEMP_MARKER: &str = ".tmp-";

/// Result of a successful atomic save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    /// Size of the saved bundle in bytes.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the saved bundle.
    pub sha256: String,
    /// Final path (the target).
    pub path: PathBuf,
}

/// A temp-file sink that can be made durable. Implemented for [`File`]; tests wrap it to inject
/// failures (fault-injection seam of [`save_atomic_with`]).
pub trait SaveSink: Write {
    fn sync_all(&mut self) -> io::Result<()>;
}

impl SaveSink for File {
    fn sync_all(&mut self) -> io::Result<()> {
        File::sync_all(self)
    }
}

/// Open function used by [`save_atomic`]: creates the temp file exclusively.
pub fn open_temp_file(path: &Path) -> io::Result<Box<dyn SaveSink>> {
    let f = OpenOptions::new().write(true).create_new(true).open(path)?;
    Ok(Box::new(f))
}

fn random_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let local = 0u8;
    let seed = format!("{}:{nanos}:{n}:{:p}", std::process::id(), &local);
    hex(&sha256(seed.as_bytes())[..8])
}

fn temp_path_for(target: &Path) -> Result<PathBuf, FormatError> {
    let name = target.file_name().ok_or_else(|| FormatError::InvalidArgument("save target has no file name".into()))?;
    let mut tmp: OsString = name.to_os_string();
    tmp.push(TEMP_MARKER);
    tmp.push(random_suffix());
    Ok(target.with_file_name(tmp))
}

struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    bytes: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Save `doc` to `path` with the staged, validated, atomic protocol (see module docs).
pub fn save_atomic(
    doc: &Document,
    path: &Path,
    meta: &SaveMeta,
    preview_png: Option<&[u8]>,
) -> Result<SaveReport, FormatError> {
    save_atomic_with(doc, path, meta, preview_png, open_temp_file)
}

/// [`save_atomic`] with an injectable temp-file opener (fault-injection seam). `open_temp`
/// receives the temp path and must create that file (so cleanup can remove it).
pub fn save_atomic_with<F>(
    doc: &Document,
    path: &Path,
    meta: &SaveMeta,
    preview_png: Option<&[u8]>,
    open_temp: F,
) -> Result<SaveReport, FormatError>
where
    F: FnOnce(&Path) -> io::Result<Box<dyn SaveSink>>,
{
    let tmp = temp_path_for(path)?;
    // The temp name is random and exclusive to this call, so removing it on failure is safe
    // even if the opener failed after creating the file.
    let result = open_temp(&tmp)
        .map_err(FormatError::from)
        .and_then(|sink| write_validate_rename(doc, path, &tmp, meta, preview_png, sink));
    if result.is_err() {
        // Best effort: the temp may not exist (opener failed) or may already be renamed.
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_validate_rename(
    doc: &Document,
    path: &Path,
    tmp: &Path,
    meta: &SaveMeta,
    preview_png: Option<&[u8]>,
    sink: Box<dyn SaveSink>,
) -> Result<SaveReport, FormatError> {
    // 1. Encode (streaming) into the temp file while hashing.
    let mut hw = HashingWriter { inner: BufWriter::new(sink), hasher: Sha256::new(), bytes: 0 };
    encode_to(doc, meta, preview_png, &mut hw)?;
    hw.flush()?;
    let written_hash: [u8; 32] = hw.hasher.finalize().into();
    let written = hw.bytes;
    let mut sink = hw.inner.into_inner().map_err(|e| FormatError::from(e.into_error()))?;
    // 2. Durable temp file.
    sink.sync_all()?;
    drop(sink);
    // 3. Re-read from storage and fully validate.
    let on_disk = fs::read(tmp)?;
    if on_disk.len() as u64 != written || sha256(&on_disk) != written_hash {
        return Err(FormatError::Corrupt("temp bundle on storage differs from the bytes written".into()));
    }
    let loaded = decode(&on_disk)?;
    if loaded.doc != *doc {
        return Err(FormatError::Corrupt("validated temp bundle does not reproduce the document".into()));
    }
    // 4. Atomic replace + best-effort directory sync.
    fs::rename(tmp, path)?;
    sync_parent_dir(path);
    Ok(SaveReport { bytes: written, sha256: hex(&written_hash), path: path.to_path_buf() })
}

/// Best-effort `fsync` of the directory containing `path` (errors ignored; see module docs).
fn sync_parent_dir(path: &Path) {
    #[cfg(unix)]
    {
        let parent = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        if let Ok(d) = File::open(parent) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Load and fully validate a bundle from `path`.
pub fn load(path: &Path) -> Result<LoadedDocument, FormatError> {
    let len = fs::metadata(path)?.len();
    if len > crate::zip::MAX_ARCHIVE_BYTES {
        return Err(FormatError::Limit("file larger than 4 GiB (zip64 unsupported)".into()));
    }
    decode(&fs::read(path)?)
}

/// Remove leftover `<target-name>.tmp-*` files next to `target` (e.g. after a process kill
/// during save). Returns how many were removed. Never touches `target` itself.
///
/// Call only when no save to `target` is in progress (e.g. at app start), otherwise a concurrent
/// save's temp file could be removed and that save would fail (safely, with a typed error).
pub fn remove_stale_temps(target: &Path) -> Result<usize, FormatError> {
    let Some(name) = target.file_name().and_then(|n| n.to_str()) else {
        return Ok(0);
    };
    let prefix = format!("{name}{TEMP_MARKER}");
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut removed = 0;
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)) && e.file_type()?.is_file() {
            fs::remove_file(e.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}
