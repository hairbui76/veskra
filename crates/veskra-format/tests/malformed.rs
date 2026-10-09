//! SAV-006 / SAV-007 / FMT-05: hostile, truncated, corrupted and future-schema bundles are
//! rejected with typed errors and never panic.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use veskra_core::{ErrorCode, VeskraError, limits};
use veskra_format::zip::{Method, ZipReader};
use veskra_format::{FormatError, decode, encode, inspect};

fn code(r: Result<veskra_format::LoadedDocument, FormatError>) -> ErrorCode {
    VeskraError::from(r.expect_err("expected rejection")).code
}

fn simple_bundle() -> Vec<u8> {
    encode(&simple_document(), &meta(), None).unwrap()
}

#[test]
fn truncation_at_every_prefix_is_a_typed_error() {
    let b = simple_bundle();
    let mut checked = 0;
    let mut n = 0;
    while n < b.len() {
        let c = code(decode(&b[..n]));
        assert!(matches!(c, ErrorCode::CorruptData | ErrorCode::AllocationLimit), "prefix {n}: {c:?}");
        checked += 1;
        n += if n + 256 > b.len() { 1 } else { 3 };
    }
    assert!(checked > 1000, "checked {checked}");
}

#[test]
fn random_byte_flips_never_panic_and_never_decode_differently() {
    let doc = rich_document();
    let b = encode(&doc, &meta(), None).unwrap();
    let mut rng = veskra_testkit::Rng::new(0x5EED);
    let (mut rejected, mut identical) = (0, 0);
    for _ in 0..600 {
        let mut m = b.clone();
        let flips = 1 + rng.below(3) as usize;
        for _ in 0..flips {
            let i = rng.below(m.len() as u64) as usize;
            m[i] ^= 1 << rng.below(8);
        }
        match decode(&m) {
            Ok(l) => {
                assert_eq!(l.doc, doc, "a flipped bundle decoded to a different document");
                identical += 1;
            }
            Err(_) => rejected += 1,
        }
    }
    assert!(rejected > 500, "rejected {rejected}, identical {identical}");
}

#[test]
fn not_a_bundle() {
    assert_eq!(code(decode(b"")), ErrorCode::CorruptData);
    assert_eq!(code(decode(b"PK\x05\x06garbage")), ErrorCode::CorruptData);
    assert_eq!(code(decode(&[0u8; 4096])), ErrorCode::CorruptData);
    // Valid ZIP but wrong / missing mimetype.
    let z = raw_zip(&[RawEntry::stored("mimetype", b"application/zip")]);
    assert_eq!(code(decode(&z)), ErrorCode::CorruptData);
    let z = raw_zip(&[RawEntry::deflated("mimetype", b"application/x-veskra")]);
    assert_eq!(code(decode(&z)), ErrorCode::CorruptData);
}

#[test]
fn raw_zip_helper_positive_control() {
    // The hostile-archive helper must produce archives the reader accepts when they are benign,
    // so the rejections below are attributable to the injected defect.
    let z = raw_zip(&[RawEntry::stored("mimetype", b"application/x-veskra"), RawEntry::deflated("a/b", &[9u8; 5000])]);
    let r = ZipReader::parse(&z).unwrap();
    assert_eq!(r.read_name("a/b").unwrap(), vec![9u8; 5000]);
}

fn assert_parse_err_contains(z: &[u8], needle: &str) {
    let e = ZipReader::parse(z).unwrap_err().to_string();
    assert!(e.contains(needle), "{e}");
}

#[test]
fn path_traversal_and_unsafe_names_rejected() {
    for bad in ["../evil", "/abs/path", "a/../../b", "a\\b", "C:/x", "a//b", "dir/", "./x"] {
        let z = raw_zip(&[RawEntry::stored("mimetype", b"application/x-veskra"), RawEntry::stored(bad, b"x")]);
        assert_parse_err_contains(&z, "unsafe or malformed entry name");
        assert_eq!(code(decode(&z)), ErrorCode::CorruptData, "{bad}");
    }
    let mut e = RawEntry::stored("x", b"x");
    e.name = vec![0xC3, 0x28];
    let z = raw_zip(&[RawEntry::stored("mimetype", b"application/x-veskra"), e]);
    assert_eq!(code(decode(&z)), ErrorCode::CorruptData);
}

#[test]
fn duplicate_names_rejected() {
    let z = raw_zip(&[
        RawEntry::stored("mimetype", b"application/x-veskra"),
        RawEntry::stored("manifest.json", b"{}"),
        RawEntry::stored("manifest.json", b"{}"),
    ]);
    assert_parse_err_contains(&z, "duplicate entry name");
    assert_eq!(code(decode(&z)), ErrorCode::CorruptData);
}

#[test]
fn zip_bombs_rejected() {
    let zeros = vec![0u8; 8 * 1024 * 1024];
    // Inflates far beyond the declared size: stopped at declared + 1 bytes.
    let mut lie = RawEntry::deflated("manifest.json", &zeros);
    lie.usize = 1000;
    let z = raw_zip(&[RawEntry::stored("mimetype", b"application/x-veskra"), lie]);
    let r = ZipReader::parse(&z).unwrap();
    assert!(r.read(&r.entries()[1]).is_err());
    assert_eq!(code(decode(&z)), ErrorCode::CorruptData);
    // Declared size impossible for DEFLATE's maximum ratio: rejected before inflating.
    let mut huge = RawEntry::deflated("project/document.json", b"tiny");
    huge.usize = 400 * 1024 * 1024;
    let z = raw_zip(&[RawEntry::stored("mimetype", b"application/x-veskra"), huge]);
    assert_eq!(code(decode(&z)), ErrorCode::AllocationLimit);
    // Above the per-entry limit.
    let mut big = RawEntry::stored("x", b"");
    big.usize = (limits::MAX_BUNDLE_ENTRY_BYTES + 1) as u32;
    big.method = 8;
    let z = raw_zip(&[RawEntry::stored("mimetype", b"application/x-veskra"), big]);
    assert_eq!(code(decode(&z)), ErrorCode::AllocationLimit);
}

#[test]
fn local_header_mismatch_and_bad_crc_rejected() {
    let b = simple_bundle();
    // Corrupt the CRC of a deflated payload byte without touching headers.
    let r = ZipReader::parse(&b).unwrap();
    let e = r.entry("project/document.json").unwrap().clone();
    drop(r);
    let mut m = b.clone();
    let pos = m.windows(21).position(|w| w == b"project/document.json").unwrap() + 21 + 5;
    m[pos] ^= 0xFF;
    assert_eq!(code(decode(&m)), ErrorCode::CorruptData);
    assert!(e.compressed_size > 0);
    // Rename in the local header only (central directory keeps the original name).
    let mut m = b.clone();
    let pos = m.windows(8).position(|w| w == b"manifest").unwrap();
    m[pos] = b'X';
    assert_eq!(code(decode(&m)), ErrorCode::CorruptData);
}

#[test]
fn sav006_corrupt_manifest_rejected() {
    let b = tamper(&simple_bundle(), |e| {
        e.iter_mut().find(|x| x.0 == "manifest.json").unwrap().1 = b"{ not json".to_vec();
    });
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    let b = tamper(&simple_bundle(), |e| edit_json(e, "manifest.json", |v| v["format"] = "psd".into()));
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    let b = tamper(&simple_bundle(), |e| edit_json(e, "manifest.json", |v| v["documentId"] = "nope".into()));
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    let b = tamper(&simple_bundle(), |e| edit_json(e, "manifest.json", |v| v["tileSize"] = 128.into()));
    assert_eq!(code(decode(&b)), ErrorCode::UnsupportedSchema);
    // Manifest listing an entry that is absent.
    let b = tamper(&simple_bundle(), |e| {
        edit_json(e, "manifest.json", |v| v["entries"].as_array_mut().unwrap().push("raster/x/0_0.tile".into()))
    });
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    // Missing manifest altogether.
    let b = tamper(&simple_bundle(), |e| e.retain(|x| x.0 != "manifest.json"));
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
}

#[test]
fn integrity_violations_rejected() {
    let a = simple_bundle();
    // Content changed but integrity.json NOT recomputed (CRC is recomputed by the zip writer).
    let mut e = unpack(&a);
    edit_json(&mut e, "project/document.json", |v| v["dpi"] = 72.0.into());
    assert_eq!(code(decode(&repack(&e, false))), ErrorCode::CorruptData);
    // Entry not covered by integrity.json.
    let mut e = unpack(&a);
    e.insert(2, ("zzz/extra".into(), vec![1], Method::Stored));
    assert_eq!(code(decode(&repack(&e, false))), ErrorCode::CorruptData);
    // integrity.json missing.
    let mut e = unpack(&a);
    e.retain(|x| x.0 != "integrity.json");
    assert_eq!(code(decode(&repack(&e, false))), ErrorCode::CorruptData);
}

#[test]
fn sav007_future_schema_refused_without_modification() {
    let a = simple_bundle();
    let b = tamper(&a, |e| {
        edit_json(e, "manifest.json", |v| {
            v["schema"] = 2.into();
            v["minReader"] = 2.into();
        })
    });
    let before = b.clone();
    let err = decode(&b).unwrap_err();
    assert!(matches!(err, FormatError::UnsupportedSchema { schema: 2, min_reader: 2, supported: 1 }), "{err:?}");
    assert_eq!(VeskraError::from(err).code, ErrorCode::UnsupportedSchema);
    assert_eq!(b, before);
    let info = inspect(&b).unwrap();
    assert!(!info.schema_supported);
    assert_eq!(info.manifest.schema, 2);
    // Older schema without a registered migration is refused too.
    let old = tamper(&a, |e| {
        edit_json(e, "manifest.json", |v| {
            v["schema"] = 0.into();
            v["minReader"] = 0.into();
        })
    });
    assert_eq!(code(decode(&old)), ErrorCode::UnsupportedSchema);
    // minReader > schema is malformed.
    let bad = tamper(&a, |e| edit_json(e, "manifest.json", |v| v["minReader"] = 5.into()));
    assert_eq!(code(decode(&bad)), ErrorCode::CorruptData);
}

#[test]
fn sav006_oversized_icc_rejected() {
    let big = vec![0u8; limits::MAX_ICC_BYTES + 1];
    let name = format!("profiles/{}.icc", hex(&sha(&big)));
    let b = tamper(&simple_bundle(), |e| {
        let old = e.iter().position(|x| x.0.starts_with("profiles/")).unwrap();
        e.remove(old);
        e.push((name.clone(), big.clone(), Method::Deflated));
        edit_json(e, "project/document.json", |v| {
            v["color"]["profile"]["icc_hash"] = hex(&sha(&big)).into();
            v["color"]["profileEntry"] = name.clone().into();
        });
        edit_json(e, "manifest.json", |v| {
            let list = v["entries"].as_array_mut().unwrap();
            list.retain(|x| !x.as_str().unwrap().starts_with("profiles/"));
            list.push(name.clone().into());
        });
    });
    assert_eq!(code(decode(&b)), ErrorCode::AllocationLimit);
}

#[test]
fn invalid_or_mismatched_icc_rejected() {
    // Bytes swapped for garbage of a matching hash/name: hash ok, ICC parse fails.
    let junk = b"definitely not an ICC profile".to_vec();
    let name = format!("profiles/{}.icc", hex(&sha(&junk)));
    let b = tamper(&simple_bundle(), |e| {
        e.retain(|x| !x.0.starts_with("profiles/"));
        e.push((name.clone(), junk.clone(), Method::Deflated));
        edit_json(e, "project/document.json", |v| {
            v["color"]["profile"]["icc_hash"] = hex(&sha(&junk)).into();
            v["color"]["profileEntry"] = name.clone().into();
        });
        edit_json(e, "manifest.json", |v| {
            let list = v["entries"].as_array_mut().unwrap();
            list.retain(|x| !x.as_str().unwrap().starts_with("profiles/"));
            list.push(name.clone().into());
        });
    });
    assert_eq!(code(decode(&b)), ErrorCode::InvalidProfile);
    // Profile hash in document.json no longer matches the bytes.
    let b = tamper(&simple_bundle(), |e| {
        edit_json(e, "project/document.json", |v| v["color"]["profile"]["semantic_name"] = "x".into());
        edit_json(e, "project/document.json", |v| {
            let h = v["color"]["profile"]["icc_hash"].as_str().unwrap().replacen('0', "1", 1);
            v["color"]["profile"]["icc_hash"] = h.into();
        });
    });
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
}

#[test]
fn sav006_giant_dimensions_and_tile_coords_rejected() {
    let b = tamper(&simple_bundle(), |e| edit_json(e, "project/document.json", |v| v["width"] = 1_000_000.into()));
    assert_eq!(code(decode(&b)), ErrorCode::AllocationLimit);
    let b = tamper(&simple_bundle(), |e| {
        edit_json(e, "project/document.json", |v| v["layers"][0]["pixels"]["tiles"] = serde_json::json!([[100000, 0]]))
    });
    assert_eq!(code(decode(&b)), ErrorCode::AllocationLimit);
}

#[test]
fn corrupt_tiles_rejected() {
    let a = simple_bundle();
    let tile_name = unpack(&a).into_iter().find(|e| e.0.ends_with(".tile")).unwrap().0;
    // Wrong payload size ("giant tile").
    let b = tamper(&a, |e| e.iter_mut().find(|x| x.0 == tile_name).unwrap().1 = vec![0u8; 128 * 128 * 8]);
    assert_eq!(code(decode(&b)), ErrorCode::CorruptTile);
    // Premultiplied invariant violated (r > a).
    let b = tamper(&a, |e| {
        let t = &mut e.iter_mut().find(|x| x.0 == tile_name).unwrap().1;
        t[0..8].copy_from_slice(&[0xFF, 0xFF, 0, 0, 0, 0, 0x10, 0]);
    });
    assert_eq!(code(decode(&b)), ErrorCode::CorruptTile);
    // Tile listed twice / tile entry missing.
    let b = tamper(&a, |e| {
        edit_json(e, "project/document.json", |v| {
            v["layers"][0]["pixels"]["tiles"] = serde_json::json!([[0, 0], [0, 0]])
        })
    });
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    let b = tamper(&a, |e| {
        edit_json(e, "project/document.json", |v| {
            v["layers"][0]["pixels"]["tiles"] = serde_json::json!([[0, 0], [1, 0]])
        })
    });
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
}

#[test]
fn stroke_payload_validation() {
    let a = simple_bundle();
    let name = unpack(&a).into_iter().find(|e| e.0.starts_with("strokes/")).unwrap().0;
    // Predicted sample smuggled into the file: refused (only confirmed samples load).
    let b = tamper(&a, |e| {
        let s = &mut e.iter_mut().find(|x| x.0 == name).unwrap().1;
        let flags_at = 10 + 32;
        s[flags_at..flags_at + 2].copy_from_slice(&2u16.to_le_bytes());
    });
    assert_eq!(code(decode(&b)), ErrorCode::InvalidDocument);
    // Truncated sample array, bad magic, absurd count.
    let b = tamper(&a, |e| e.iter_mut().find(|x| x.0 == name).unwrap().1.truncate(30));
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    let b = tamper(&a, |e| e.iter_mut().find(|x| x.0 == name).unwrap().1[0] = b'X');
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    let b =
        tamper(&a, |e| e.iter_mut().find(|x| x.0 == name).unwrap().1[6..10].copy_from_slice(&u32::MAX.to_le_bytes()));
    assert_eq!(code(decode(&b)), ErrorCode::AllocationLimit);
}

#[test]
fn document_tree_violations_rejected() {
    let a = encode(&rich_document(), &meta(), None).unwrap();
    // Dangling child reference.
    let b = tamper(&a, |e| {
        edit_json(e, "project/document.json", |v| v["root"].as_array_mut().unwrap().push(lid(77).to_string().into()))
    });
    assert_eq!(code(decode(&b)), ErrorCode::InvalidDocument);
    // Unknown blend mode / kind-content mismatch.
    let b =
        tamper(&a, |e| edit_json(e, "project/document.json", |v| v["layers"][0]["props"]["blend"] = "dissolve".into()));
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    let b = tamper(&a, |e| edit_json(e, "project/document.json", |v| v["layers"][0]["kind"] = "group".into()));
    assert_eq!(code(decode(&b)), ErrorCode::CorruptData);
    // Out-of-range opacity.
    let b = tamper(&a, |e| edit_json(e, "project/document.json", |v| v["layers"][0]["props"]["opacity"] = 2.0.into()));
    assert!(matches!(code(decode(&b)), ErrorCode::InvalidDocument));
}
