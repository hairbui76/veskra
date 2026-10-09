//! PackBits run-length coding as used by PSD compression method 1 (Adobe Photoshop File Formats
//! specification; Apple TN1023). Each row is coded independently.
//!
//! Header byte `n` (as `i8`): `0..=127` → copy the next `n + 1` literal bytes; `-127..=-1` → repeat
//! the next byte `1 - n` times; `-128` → no-op (never emitted, skipped when decoding).

/// Append the PackBits encoding of `row` to `out`; returns the number of bytes appended.
pub fn encode_row(row: &[u8], out: &mut Vec<u8>) -> usize {
    let start = out.len();
    let n = row.len();
    let mut i = 0;
    while i < n {
        // Length of the run starting at i (max 128).
        let mut run = 1;
        while i + run < n && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((1i16 - run as i16) as i8 as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        // Literal: extend until a run of >= 3 starts (a 2-run inside literals costs the same).
        let lit_start = i;
        let mut j = i;
        while j < n && j - lit_start < 128 {
            if j + 2 < n && row[j] == row[j + 1] && row[j] == row[j + 2] {
                break;
            }
            j += 1;
        }
        if j == lit_start {
            j += 1;
        }
        let len = j - lit_start;
        out.push((len - 1) as u8);
        out.extend_from_slice(&row[lit_start..j]);
        i = j;
    }
    out.len() - start
}

/// Decode one PackBits row of exactly `width` bytes from `src` (which must be fully consumed).
/// Returns `None` on malformed or wrong-length data.
pub fn decode_row(src: &[u8], width: usize, out: &mut Vec<u8>) -> Option<()> {
    let start = out.len();
    let mut i = 0;
    while i < src.len() {
        let h = src[i] as i8;
        i += 1;
        if h >= 0 {
            let len = h as usize + 1;
            let lit = src.get(i..i + len)?;
            out.extend_from_slice(lit);
            i += len;
        } else if h != -128 {
            let len = (1 - h as i16) as usize;
            let b = *src.get(i)?;
            i += 1;
            out.extend(std::iter::repeat_n(b, len));
        }
        if out.len() - start > width {
            return None;
        }
    }
    (out.len() - start == width).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(row: &[u8]) {
        let mut enc = Vec::new();
        let n = encode_row(row, &mut enc);
        assert_eq!(n, enc.len());
        // Worst case bound: one header byte per 128 literals.
        assert!(enc.len() <= row.len() + row.len().div_ceil(128));
        let mut dec = Vec::new();
        decode_row(&enc, row.len(), &mut dec).expect("decode");
        assert_eq!(dec, row);
    }

    #[test]
    fn packbits_roundtrips() {
        roundtrip(&[]);
        roundtrip(&[7]);
        roundtrip(&[1, 1]);
        roundtrip(&[1, 2, 3, 3, 3, 4]);
        roundtrip(&[0; 1000]);
        roundtrip(&(0..=255u8).cycle().take(1000).collect::<Vec<_>>());
        let mut seed = 0x1234_5678u32;
        let mut noisy = Vec::new();
        for _ in 0..5000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            noisy.push(if seed >> 30 == 0 { (seed >> 8) as u8 } else { 9 });
        }
        roundtrip(&noisy);
    }

    #[test]
    fn known_vector_from_apple_tn1023() {
        // TN1023 example: unpacked AA AA AA 80 00 2A AA AA AA AA 80 00 2A 22 AA AA AA AA AA AA AA AA AA AA
        let packed = [0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7, 0xAA];
        let mut dec = Vec::new();
        decode_row(&packed, 24, &mut dec).expect("decode");
        let expected = [
            0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x22, 0xAA, 0xAA, 0xAA, 0xAA,
            0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
        ];
        assert_eq!(dec, expected);
        roundtrip(&expected);
    }

    #[test]
    fn decoder_rejects_bad_lengths() {
        let mut v = Vec::new();
        assert!(decode_row(&[0x05, 1, 2], 6, &mut v).is_none()); // truncated literal
        v.clear();
        assert!(decode_row(&[0xFE, 1], 2, &mut v).is_none()); // 3 bytes for a 2-byte row
        v.clear();
        assert!(decode_row(&[0x80, 0x00, 7], 1, &mut v).is_some()); // -128 no-op is skipped
    }
}
