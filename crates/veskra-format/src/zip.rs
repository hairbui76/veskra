//! Minimal, bounded ZIP reader/writer (PKWARE APPNOTE 6.3 subset, ADR-009).
//!
//! Supported: local file headers, central directory, end-of-central-directory record,
//! methods STORE (0) and DEFLATE (8), CRC-32, UTF-8 names (flag bit 11).
//! Not supported (rejected on read, never written): zip64, encryption, multi-disk archives,
//! data descriptors (flag bit 3), patched/strong-encryption/masked-directory flags.
//!
//! The reader never panics on any input: every offset and length is bounds-checked, entry
//! counts and sizes are bounded by [`veskra_core::limits`], names must be safe relative paths,
//! local headers must agree with the central directory, entries must not overlap, and DEFLATE
//! output is capped at the declared size (zip-bomb guard) and CRC-verified.
//!
//! The writer is deterministic: fixed DOS timestamp (1980-01-01 00:00), fixed "version made by",
//! no extra fields, no comments, so identical inputs give identical bytes.

use crate::error::FormatError;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;
use veskra_core::limits;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const LOCAL_LEN: usize = 30;
const CENTRAL_LEN: usize = 46;
const EOCD_LEN: usize = 22;
const MAX_COMMENT: usize = 0xFFFF;
/// "Version needed to extract" / "version made by" written by Veskra (2.0, MS-DOS attributes).
const VERSION: u16 = 20;
/// General-purpose flag bit 11: names are UTF-8.
const FLAG_UTF8: u16 = 1 << 11;
/// Flag bits 1-2 only describe DEFLATE compression level; harmless.
const FLAG_DEFLATE_OPTS: u16 = 0b110;
/// Fixed DOS time 00:00:00.
const DOS_TIME: u16 = 0;
/// Fixed DOS date 1980-01-01 (year offset 0, month 1, day 1).
const DOS_DATE: u16 = (1 << 5) | 1;
/// DEFLATE cannot expand by more than ~1032:1; a declared size above this bound is a lie.
const MAX_DEFLATE_RATIO: u64 = 1032;
/// Slack added to the ratio bound for tiny entries.
const RATIO_SLACK: u64 = 1024;
/// DEFLATE level used by the writer (fixed for determinism).
const DEFLATE_LEVEL: u8 = 6;

/// Maximum entry-name length in bytes.
pub const MAX_ENTRY_NAME_BYTES: usize = 512;
/// Maximum entries representable without zip64 (`0xFFFF` is the zip64 marker).
pub const MAX_ENTRIES: usize = 0xFFFE;
/// Maximum archive size representable without zip64.
pub const MAX_ARCHIVE_BYTES: u64 = 0xFFFF_FFFE;

/// Compression method of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Stored,
    Deflated,
}

impl Method {
    fn code(self) -> u16 {
        match self {
            Method::Stored => 0,
            Method::Deflated => 8,
        }
    }
    fn from_code(c: u16) -> Option<Method> {
        match c {
            0 => Some(Method::Stored),
            8 => Some(Method::Deflated),
            _ => None,
        }
    }
}

/// One validated central-directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub method: Method,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    header_offset: usize,
    data_offset: usize,
}

impl ZipEntry {
    fn data_end(&self) -> usize {
        // Checked during parse; saturating keeps this total.
        self.data_offset.saturating_add(self.compressed_size as usize)
    }
}

fn corrupt(m: impl Into<String>) -> FormatError {
    FormatError::Corrupt(m.into())
}

fn rd_u16(d: &[u8], at: usize) -> Option<u16> {
    let b: [u8; 2] = d.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(b))
}

fn rd_u32(d: &[u8], at: usize) -> Option<u32> {
    let b: [u8; 4] = d.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

/// Validate an entry name: non-empty UTF-8 relative path of `/`-separated, non-empty
/// components, no `.`/`..`, no backslash, colon, control characters or leading slash.
pub fn validate_name(b: &[u8]) -> Result<String, FormatError> {
    let unsafe_name = || FormatError::Corrupt("unsafe or malformed entry name".into());
    if b.is_empty() || b.len() > MAX_ENTRY_NAME_BYTES {
        return Err(unsafe_name());
    }
    let s = std::str::from_utf8(b).map_err(|_| unsafe_name())?;
    if s.starts_with('/') || s.contains('\\') || s.contains(':') || s.chars().any(char::is_control) {
        return Err(unsafe_name());
    }
    if s.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(unsafe_name());
    }
    Ok(s.to_string())
}

/// Validated, read-only view of a ZIP archive held in memory.
#[derive(Debug)]
pub struct ZipReader<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry>,
    by_name: BTreeMap<String, usize>,
}

impl<'a> ZipReader<'a> {
    /// Parse and validate the archive structure (no decompression happens here).
    pub fn parse(data: &'a [u8]) -> Result<Self, FormatError> {
        if data.len() as u64 > MAX_ARCHIVE_BYTES + 1 {
            return Err(FormatError::Limit("archive larger than 4 GiB (zip64 unsupported)".into()));
        }
        if data.len() < EOCD_LEN {
            return Err(corrupt("archive too small"));
        }
        let eocd = find_eocd(data)?;
        let field = |off: usize| rd_u16(data, eocd.saturating_add(off)).ok_or_else(|| corrupt("truncated EOCD"));
        let field32 = |off: usize| rd_u32(data, eocd.saturating_add(off)).ok_or_else(|| corrupt("truncated EOCD"));
        let (disk, cd_disk, n_disk, n_total) = (field(4)?, field(6)?, field(8)?, field(10)?);
        let (cd_size, cd_off) = (field32(12)?, field32(16)?);
        if n_total == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF {
            return Err(corrupt("zip64 archives are not supported"));
        }
        if disk != 0 || cd_disk != 0 || n_disk != n_total {
            return Err(corrupt("multi-disk archives are not supported"));
        }
        let n = n_total as usize;
        if n > limits::MAX_BUNDLE_ENTRIES {
            return Err(FormatError::Limit("too many entries".into()));
        }
        let cd_off = cd_off as usize;
        let cd_end = cd_off.checked_add(cd_size as usize).ok_or_else(|| corrupt("central directory overflow"))?;
        if cd_end != eocd {
            return Err(corrupt("central directory does not end at EOCD"));
        }
        if n.checked_mul(CENTRAL_LEN).is_none_or(|need| need > cd_size as usize) {
            return Err(corrupt("central directory too small for entry count"));
        }

        let mut entries = Vec::with_capacity(n);
        let mut by_name = BTreeMap::new();
        let mut total: u64 = 0;
        let mut p = cd_off;
        for i in 0..n {
            let e = parse_central(data, p, cd_off, cd_end)?;
            total = total.checked_add(e.0.uncompressed_size).ok_or_else(|| corrupt("size overflow"))?;
            if total > limits::MAX_BUNDLE_TOTAL_BYTES {
                return Err(FormatError::Limit("total decompressed size exceeds bundle limit".into()));
            }
            if by_name.insert(e.0.name.clone(), i).is_some() {
                return Err(corrupt("duplicate entry name"));
            }
            entries.push(e.0);
            p = e.1;
        }
        if p != cd_end {
            return Err(corrupt("trailing bytes in central directory"));
        }
        // Entries must not overlap each other (overlapping-entry bombs, aliasing).
        let mut order: Vec<&ZipEntry> = entries.iter().collect();
        order.sort_by_key(|e| e.header_offset);
        for w in order.windows(2) {
            if let [a, b] = w
                && a.data_end() > b.header_offset
            {
                return Err(corrupt("overlapping entries"));
            }
        }
        Ok(Self { data, entries, by_name })
    }

    /// Entries in central-directory order.
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    pub fn entry(&self, name: &str) -> Option<&ZipEntry> {
        self.by_name.get(name).and_then(|i| self.entries.get(*i))
    }

    /// Decompress an entry, enforcing its declared size and CRC.
    pub fn read(&self, e: &ZipEntry) -> Result<Vec<u8>, FormatError> {
        let raw = self.data.get(e.data_offset..e.data_end()).ok_or_else(|| corrupt("entry data out of bounds"))?;
        let declared =
            usize::try_from(e.uncompressed_size).map_err(|_| FormatError::Limit("entry too large".into()))?;
        let out = match e.method {
            Method::Stored => raw.to_vec(),
            Method::Deflated => inflate_bounded(raw, declared)?,
        };
        if out.len() != declared {
            return Err(corrupt("entry size does not match its declared size"));
        }
        if crc32fast::hash(&out) != e.crc32 {
            return Err(corrupt("entry CRC-32 mismatch"));
        }
        Ok(out)
    }

    /// Read an entry by name.
    pub fn read_name(&self, name: &str) -> Result<Vec<u8>, FormatError> {
        let e = self.entry(name).ok_or_else(|| corrupt(format!("missing entry {name}")))?;
        self.read(e)
    }
}

fn find_eocd(data: &[u8]) -> Result<usize, FormatError> {
    let last = data.len() - EOCD_LEN;
    let min = data.len().saturating_sub(EOCD_LEN + MAX_COMMENT);
    let mut pos = last;
    loop {
        if rd_u32(data, pos) == Some(SIG_EOCD)
            && let Some(clen) = rd_u16(data, pos.saturating_add(20))
            && pos.saturating_add(EOCD_LEN).saturating_add(clen as usize) == data.len()
        {
            return Ok(pos);
        }
        if pos == min {
            return Err(corrupt("end of central directory not found (truncated or not a ZIP)"));
        }
        pos -= 1;
    }
}

/// Parse one central-directory record at `p` and validate its local header.
/// Returns the entry and the offset of the next record.
fn parse_central(data: &[u8], p: usize, cd_off: usize, cd_end: usize) -> Result<(ZipEntry, usize), FormatError> {
    let trunc = || corrupt("truncated central directory");
    if rd_u32(data, p) != Some(SIG_CENTRAL) {
        return Err(corrupt("bad central directory signature"));
    }
    let u16_at = |o: usize| rd_u16(data, p.saturating_add(o)).ok_or_else(trunc);
    let u32_at = |o: usize| rd_u32(data, p.saturating_add(o)).ok_or_else(trunc);
    let flags = u16_at(8)?;
    let method_code = u16_at(10)?;
    let crc = u32_at(16)?;
    let csize = u32_at(20)?;
    let usize_ = u32_at(24)?;
    let (nlen, xlen, clen) = (u16_at(28)? as usize, u16_at(30)? as usize, u16_at(32)? as usize);
    let disk_start = u16_at(34)?;
    let local_off = u32_at(42)? as usize;

    let name_start = p.saturating_add(CENTRAL_LEN);
    let next = name_start.saturating_add(nlen).saturating_add(xlen).saturating_add(clen);
    if next > cd_end {
        return Err(trunc());
    }
    let name_bytes = data.get(name_start..name_start.saturating_add(nlen)).ok_or_else(trunc)?;
    if flags & !(FLAG_UTF8 | FLAG_DEFLATE_OPTS) != 0 {
        return Err(corrupt("unsupported ZIP flags (encryption, data descriptor or patch data)"));
    }
    let method = Method::from_code(method_code).ok_or_else(|| corrupt("unsupported compression method"))?;
    if disk_start != 0 {
        return Err(corrupt("multi-disk archives are not supported"));
    }
    let name = validate_name(name_bytes)?;
    let (csize, usize_) = (csize as u64, usize_ as u64);
    if usize_ > limits::MAX_BUNDLE_ENTRY_BYTES {
        return Err(FormatError::Limit("entry exceeds per-entry size limit".into()));
    }
    match method {
        Method::Stored if csize != usize_ => return Err(corrupt("stored entry sizes differ")),
        Method::Deflated if usize_ > csize.saturating_mul(MAX_DEFLATE_RATIO).saturating_add(RATIO_SLACK) => {
            return Err(FormatError::Limit("declared compression ratio impossible for DEFLATE (zip bomb)".into()));
        }
        _ => {}
    }

    // Local header must agree with the central directory.
    let lerr = || corrupt("local header does not match central directory");
    if rd_u32(data, local_off) != Some(SIG_LOCAL) {
        return Err(corrupt("bad local header signature"));
    }
    let l16 = |o: usize| rd_u16(data, local_off.saturating_add(o)).ok_or_else(trunc);
    let l32 = |o: usize| rd_u32(data, local_off.saturating_add(o)).ok_or_else(trunc);
    if l16(6)? != flags
        || l16(8)? != method_code
        || l32(14)? != crc
        || l32(18)? as u64 != csize
        || l32(22)? as u64 != usize_
    {
        return Err(lerr());
    }
    let (lnlen, lxlen) = (l16(26)? as usize, l16(28)? as usize);
    let lname_start = local_off.saturating_add(LOCAL_LEN);
    if data.get(lname_start..lname_start.saturating_add(lnlen)) != Some(name_bytes) {
        return Err(lerr());
    }
    let data_offset = lname_start.saturating_add(lnlen).saturating_add(lxlen);
    let data_end = data_offset.checked_add(csize as usize).ok_or_else(trunc)?;
    if data_end > cd_off {
        return Err(corrupt("entry data overlaps central directory"));
    }
    let entry = ZipEntry {
        name,
        method,
        crc32: crc,
        compressed_size: csize,
        uncompressed_size: usize_,
        header_offset: local_off,
        data_offset,
    };
    Ok((entry, next))
}

/// Inflate raw DEFLATE data producing at most `declared + 1` bytes (so overflow is detected
/// without unbounded allocation).
fn inflate_bounded(raw: &[u8], declared: usize) -> Result<Vec<u8>, FormatError> {
    let limit = declared.saturating_add(1);
    match miniz_oxide::inflate::decompress_to_vec_with_limit(raw, limit) {
        Ok(v) if v.len() > declared => Err(corrupt("entry inflates beyond its declared size")),
        Ok(v) => Ok(v),
        Err(e) if e.status == miniz_oxide::inflate::TINFLStatus::HasMoreOutput => {
            Err(corrupt("entry inflates beyond its declared size"))
        }
        Err(_) => Err(corrupt("invalid DEFLATE stream")),
    }
}

/// Streaming, deterministic ZIP writer.
pub struct ZipWriter<W: Write> {
    w: W,
    offset: u64,
    central: Vec<u8>,
    count: usize,
    names: BTreeSet<String>,
}

impl<W: Write> fmt::Debug for ZipWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZipWriter").field("offset", &self.offset).field("count", &self.count).finish()
    }
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

impl<W: Write> ZipWriter<W> {
    pub fn new(w: W) -> Self {
        Self { w, offset: 0, central: Vec::new(), count: 0, names: BTreeSet::new() }
    }

    /// Bytes written so far.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Append one entry. Names are validated with the reader's rules.
    pub fn add(&mut self, name: &str, data: &[u8], method: Method) -> Result<(), FormatError> {
        validate_name(name.as_bytes()).map_err(|_| FormatError::InvalidArgument("unsafe entry name".into()))?;
        if !self.names.insert(name.to_string()) {
            return Err(FormatError::InvalidArgument("duplicate entry name".into()));
        }
        if self.count >= MAX_ENTRIES {
            return Err(FormatError::Limit(format!(
                "bundle needs more than {MAX_ENTRIES} entries (zip64 unsupported)"
            )));
        }
        if data.len() as u64 > limits::MAX_BUNDLE_ENTRY_BYTES {
            return Err(FormatError::Limit("entry exceeds per-entry size limit".into()));
        }
        let crc = crc32fast::hash(data);
        let body: Cow<'_, [u8]> = match method {
            Method::Stored => Cow::Borrowed(data),
            Method::Deflated => Cow::Owned(miniz_oxide::deflate::compress_to_vec(data, DEFLATE_LEVEL)),
        };
        let header_off = self.offset;
        let local_total = LOCAL_LEN as u64 + name.len() as u64 + body.len() as u64;
        if header_off + local_total > MAX_ARCHIVE_BYTES {
            return Err(FormatError::Limit("bundle exceeds 4 GiB (zip64 unsupported)".into()));
        }
        // All of these fit u32 thanks to the archive bound above.
        let (csize, usize_, noff) = (body.len() as u32, data.len() as u32, header_off as u32);
        let nlen = name.len() as u16;

        let mut h = Vec::with_capacity(LOCAL_LEN + name.len());
        put32(&mut h, SIG_LOCAL);
        put16(&mut h, VERSION);
        put16(&mut h, FLAG_UTF8);
        put16(&mut h, method.code());
        put16(&mut h, DOS_TIME);
        put16(&mut h, DOS_DATE);
        put32(&mut h, crc);
        put32(&mut h, csize);
        put32(&mut h, usize_);
        put16(&mut h, nlen);
        put16(&mut h, 0);
        h.extend_from_slice(name.as_bytes());
        self.w.write_all(&h)?;
        self.w.write_all(&body)?;
        self.offset += local_total;

        let c = &mut self.central;
        put32(c, SIG_CENTRAL);
        put16(c, VERSION);
        put16(c, VERSION);
        put16(c, FLAG_UTF8);
        put16(c, method.code());
        put16(c, DOS_TIME);
        put16(c, DOS_DATE);
        put32(c, crc);
        put32(c, csize);
        put32(c, usize_);
        put16(c, nlen);
        put16(c, 0); // extra
        put16(c, 0); // comment
        put16(c, 0); // disk start
        put16(c, 0); // internal attrs
        put32(c, 0); // external attrs
        put32(c, noff);
        c.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    /// Write the central directory and EOCD; returns the inner writer and total bytes written.
    pub fn finish(mut self) -> Result<(W, u64), FormatError> {
        let cd_off = self.offset;
        let cd_size = self.central.len() as u64;
        let total = cd_off + cd_size + EOCD_LEN as u64;
        if total > MAX_ARCHIVE_BYTES {
            return Err(FormatError::Limit("bundle exceeds 4 GiB (zip64 unsupported)".into()));
        }
        self.w.write_all(&self.central)?;
        let mut e = Vec::with_capacity(EOCD_LEN);
        put32(&mut e, SIG_EOCD);
        put16(&mut e, 0);
        put16(&mut e, 0);
        put16(&mut e, self.count as u16);
        put16(&mut e, self.count as u16);
        put32(&mut e, cd_size as u32);
        put32(&mut e, cd_off as u32);
        put16(&mut e, 0);
        self.w.write_all(&e)?;
        self.w.flush()?;
        Ok((self.w, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut w = ZipWriter::new(Vec::new());
        w.add("mimetype", b"application/x-veskra", Method::Stored).unwrap();
        w.add("a/b.txt", &vec![7u8; 10_000], Method::Deflated).unwrap();
        w.add("empty", b"", Method::Deflated).unwrap();
        w.finish().unwrap().0
    }

    #[test]
    fn roundtrip_and_determinism() {
        let z = sample();
        assert_eq!(z, sample());
        let r = ZipReader::parse(&z).unwrap();
        assert_eq!(r.entries().len(), 3);
        assert_eq!(r.read_name("mimetype").unwrap(), b"application/x-veskra");
        assert_eq!(r.read_name("a/b.txt").unwrap(), vec![7u8; 10_000]);
        assert_eq!(r.read_name("empty").unwrap(), b"");
        assert!(r.entry("a/b.txt").unwrap().compressed_size < 200);
    }

    #[test]
    fn names_validated() {
        for bad in ["", "/abs", "../x", "a/../b", "a\\b", "c:x", "a//b", "a/", "./a", "a\u{0}b"] {
            assert!(validate_name(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert!(validate_name(&[0xff, 0xfe]).is_err());
        assert!(validate_name(b"raster/x/-1_2.tile").is_ok());
        let mut w = ZipWriter::new(Vec::new());
        assert!(w.add("../evil", b"x", Method::Stored).is_err());
        w.add("ok", b"x", Method::Stored).unwrap();
        assert!(w.add("ok", b"y", Method::Stored).is_err());
    }

    #[test]
    fn every_prefix_rejected_without_panic() {
        let z = sample();
        for n in 0..z.len() {
            assert!(ZipReader::parse(&z[..n]).is_err(), "prefix {n}");
        }
    }

    #[test]
    fn every_single_byte_flip_never_panics() {
        let z = sample();
        for i in 0..z.len() {
            let mut m = z.clone();
            m[i] ^= 0x5A;
            if let Ok(r) = ZipReader::parse(&m) {
                for e in r.entries() {
                    let _ = r.read(e);
                }
            }
        }
    }
}
