//! Minimal private PNG encoder for images embedded in SVG (FMT-B). RGBA8 straight alpha,
//! non-interlaced, adaptive per-row filter (minimum sum of absolute differences), optional
//! `iCCP` chunk. Deterministic: fixed zlib level, no time/text chunks.

use veskra_core::{ErrorCode, VeskraError};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), VeskraError> {
    let len =
        u32::try_from(data.len()).map_err(|_| VeskraError::new(ErrorCode::SvgEncodeError, "PNG chunk too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&h.finalize().to_be_bytes());
    Ok(())
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Filter one row with filter type `ft` into `dst` (same length as `row`).
fn filter_row(ft: u8, row: &[u8], prev: &[u8], dst: &mut [u8]) {
    const BPP: usize = 4;
    for i in 0..row.len() {
        let a = if i >= BPP { row[i - BPP] } else { 0 };
        let b = prev[i];
        let c = if i >= BPP { prev[i - BPP] } else { 0 };
        let pred = match ft {
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            4 => paeth(a, b, c),
            _ => 0,
        };
        dst[i] = row[i].wrapping_sub(pred);
    }
}

/// Encode straight RGBA8. `icc` = (profile name, ICC bytes) for an `iCCP` chunk.
pub(crate) fn encode_rgba8(w: u32, h: u32, rgba: &[u8], icc: Option<(&str, &[u8])>) -> Result<Vec<u8>, VeskraError> {
    let stride = (w as usize).checked_mul(4).ok_or_else(|| VeskraError::alloc("PNG row too large"))?;
    if w == 0 || h == 0 || rgba.len() != stride * h as usize {
        return Err(VeskraError::new(ErrorCode::SvgEncodeError, "PNG buffer size mismatch"));
    }
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    let zero = vec![0u8; stride];
    let mut cand = vec![0u8; stride];
    let mut best = vec![0u8; stride];
    for (y, row) in rgba.chunks_exact(stride).enumerate() {
        let prev = if y == 0 { &zero[..] } else { &rgba[(y - 1) * stride..y * stride] };
        let mut best_ft = 0u8;
        let mut best_score = u64::MAX;
        for ft in 0..5u8 {
            filter_row(ft, row, prev, &mut cand);
            let score: u64 = cand.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum();
            if score < best_score {
                best_score = score;
                best_ft = ft;
                best.copy_from_slice(&cand);
            }
        }
        raw.push(best_ft);
        raw.extend_from_slice(&best);
    }
    let mut out = Vec::with_capacity(raw.len() / 3 + 1024);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr)?;
    if let Some((name, bytes)) = icc {
        // Profile name: 1–79 Latin-1 printable bytes; we keep printable ASCII only.
        let mut n: Vec<u8> = name.bytes().filter(|b| (0x20..0x7F).contains(b)).take(79).collect();
        while n.last() == Some(&b' ') {
            n.pop();
        }
        if n.is_empty() {
            n.extend_from_slice(b"ICC profile");
        }
        let mut data = n;
        data.push(0); // separator
        data.push(0); // compression method: zlib
        data.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(bytes, 9));
        chunk(&mut out, b"iCCP", &data)?;
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    for part in z.chunks(1 << 20) {
        chunk(&mut out, b"IDAT", part)?;
    }
    chunk(&mut out, b"IEND", &[])?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_independent_decoder() {
        let (w, h) = (7u32, 5u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 37 % 251) as u8).collect();
        let icc = veskra_color::IccProfile::srgb();
        let png = encode_rgba8(w, h, &px, Some(("sRGB IEC61966-2.1", icc.bytes()))).unwrap();
        let (dw, dh, back) = veskra_testkit::decode_png_rgba8(&png).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(back, px);
        assert!(png.windows(4).any(|c| c == b"iCCP"));
        let plain = encode_rgba8(w, h, &px, None).unwrap();
        assert!(!plain.windows(4).any(|c| c == b"iCCP"));
        assert!(encode_rgba8(0, 1, &[], None).is_err());
        assert!(encode_rgba8(2, 2, &px[..4], None).is_err());
    }
}
