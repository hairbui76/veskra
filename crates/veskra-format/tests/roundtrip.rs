//! SAV-001 (round-trip fidelity + byte-identical re-encode) and SAV-009 data part
//! (authoritative data reloads without drift across repeated save/load cycles).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use veskra_core::{ErrorCode, VeskraError};
use veskra_format::zip::{Method, ZipReader};
use veskra_format::{FormatWarning, decode, encode, inspect, read_preview};

const PNG_STUB: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

#[test]
fn sav001_rich_document_roundtrip_is_exact_and_byte_identical() {
    let doc = rich_document();
    let a = encode(&doc, &meta(), Some(PNG_STUB)).unwrap();
    let loaded = decode(&a).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.doc, doc, "decoded document differs");
    assert_eq!(loaded.manifest.document_id, doc.id);
    assert_eq!(loaded.manifest.created, "2026-10-09T00:00:00Z");
    // Spot-check semantics that PartialEq covers, so a regression message is specific.
    let l = loaded.doc.layer(lid(4)).unwrap();
    assert!(l.props.clip_to_below && l.props.alpha_locked);
    assert_eq!(l.pixels().unwrap().mask.as_ref().unwrap().get(64 + 10, 64), 40);
    assert!(loaded.doc.layer(lid(5)).unwrap().group().unwrap().pass_through);
    assert!(loaded.doc.layer(lid(8)).unwrap().props.locked);
    assert_eq!(loaded.doc.stroke(sid(1)).unwrap().samples(), doc.stroke(sid(1)).unwrap().samples());
    assert_eq!(loaded.doc.color.profile.semantic_name, "Display P3 (Veskra test)");
    assert_eq!(&*loaded.doc.color.icc_bytes, &*doc.color.icc_bytes);
    // Re-encode of the decoded document is byte-identical (determinism + no lossy fields).
    let b = encode(&loaded.doc, &meta(), Some(PNG_STUB)).unwrap();
    assert_eq!(a, b);
    assert_eq!(read_preview(&a).unwrap().as_deref(), Some(PNG_STUB));
}

#[test]
fn encode_is_deterministic_and_layout_matches_adr009() {
    let doc = rich_document();
    let a = encode(&doc, &meta(), None).unwrap();
    assert_eq!(a, encode(&doc.clone(), &meta(), None).unwrap());
    let z = ZipReader::parse(&a).unwrap();
    let names: Vec<&str> = z.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names[0], "mimetype");
    assert_eq!(z.entries()[0].method, Method::Stored);
    assert_eq!(names[1], "manifest.json");
    assert_eq!(*names.last().unwrap(), "integrity.json");
    let mid = &names[2..names.len() - 1];
    assert!(mid.windows(2).all(|w| w[0] < w[1]), "payload entries sorted");
    for prefix in ["profiles/", "project/document.json", "raster/", "masks/", "selection/", "strokes/"] {
        assert!(names.iter().any(|n| n.starts_with(prefix)), "missing {prefix}");
    }
    assert!(names.contains(&format!("raster/{}/-1_-1.tile", lid(9)).as_str()));
    // Transparent tile (3,2) of layer 9 is not stored.
    assert!(!names.contains(&format!("raster/{}/3_2.tile", lid(9)).as_str()));
    // Uniform-default mask tiles are not stored, partial ones are.
    assert!(names.contains(&format!("masks/{}/1_1.mask", lid(4)).as_str()));
    let info = inspect(&a).unwrap();
    assert!(info.schema_supported);
    assert_eq!(info.manifest.schema, 1);
    assert_eq!(info.manifest.tile_size, 64);
    assert_eq!(info.manifest.pixel, "rgba16-premul-encoded");
    assert_eq!(info.entries.len(), names.len());
    assert_eq!(info.manifest.entries.len(), names.len() - 3);
}

#[test]
fn sav009_repeated_cycles_do_not_drift() {
    let mut doc = rich_document();
    let first = encode(&doc, &meta(), None).unwrap();
    for _ in 0..5 {
        let bytes = encode(&doc, &meta(), None).unwrap();
        assert_eq!(bytes, first);
        doc = decode(&bytes).unwrap().doc;
    }
    let orig = rich_document();
    assert_eq!(doc, orig);
    // Authoritative tile pixels are bit-identical (cache rebuild input does not drift).
    for l in orig.iter_layers().filter_map(|l| l.pixels().map(|p| (l.id, p))) {
        let back = doc.layer(l.0).unwrap().pixels().unwrap();
        for (c, t) in l.1.tiles.iter_tiles().filter(|(_, t)| !t.is_transparent()) {
            assert_eq!(back.tiles.tile(c).unwrap().pixels(), t.pixels());
        }
    }
}

#[test]
fn simple_and_twelve_layer_roundtrip() {
    for doc in [simple_document(), twelve_layer_document()] {
        let a = encode(&doc, &meta(), None).unwrap();
        let back = decode(&a).unwrap();
        assert_eq!(back.doc, doc);
        assert_eq!(encode(&back.doc, &meta(), None).unwrap(), a);
    }
}

#[test]
fn json_floats_roundtrip_bit_exact() {
    // Geometry/opacity go through JSON; check many awkward f64/f32 values survive exactly.
    let mut rng = veskra_testkit::Rng::new(99);
    let mut doc = rich_document();
    let id = sid(1);
    let rec = doc.stroke(id).unwrap().clone();
    let mut g = rec.geometry.clone().unwrap();
    for el in g.centerline.els.iter_mut() {
        if let veskra_vector::PathEl::CubicTo(a, b, c) = el {
            for p in [a, b, c] {
                p.x = f64::from_bits(rng.next_u64() >> 2) % 1.0e6;
                p.y = rng.range(-1.0e4, 1.0e4) / 3.0;
            }
        }
    }
    g.fit_error = 1.0 / 3.0;
    let layer = doc.layer_mut(lid(3)).unwrap();
    layer.props.opacity = 0.1 + f32::EPSILON;
    let mut parts = doc.to_parts();
    for s in parts.strokes.iter_mut().filter(|s| s.id == id) {
        s.geometry = Some(g.clone());
    }
    let doc = veskra_doc::Document::from_parts(parts).unwrap();
    let back = decode(&encode(&doc, &meta(), None).unwrap()).unwrap().doc;
    assert_eq!(back.stroke(id).unwrap().geometry.as_ref().unwrap(), &g);
    assert_eq!(back, doc);
}

#[test]
fn encode_rejects_inconsistent_inputs() {
    let doc = simple_document();
    let e = encode(&doc, &meta(), Some(b"GIF89a")).unwrap_err();
    assert_eq!(VeskraError::from(e).code, ErrorCode::InvalidArgument);
    // Profile hash that does not match the ICC bytes is refused rather than written.
    let mut bad = doc.clone();
    bad.color.profile.icc_hash[0] ^= 1;
    assert_eq!(VeskraError::from(encode(&bad, &meta(), None).unwrap_err()).code, ErrorCode::InvalidDocument);
    // Non-finite geometry is refused (JSON cannot represent it).
    let mut parts = rich_document().to_parts();
    let s = parts.strokes.iter_mut().find(|s| s.geometry.is_some()).unwrap();
    s.geometry.as_mut().unwrap().fit_error = f64::NAN;
    let nan_doc = veskra_doc::Document::from_parts(parts).unwrap();
    assert_eq!(VeskraError::from(encode(&nan_doc, &meta(), None).unwrap_err()).code, ErrorCode::InvalidDocument);
}

#[test]
fn forward_compatible_unknown_fields_and_entries_warn() {
    let a = encode(&simple_document(), &meta(), None).unwrap();
    let b = tamper(&a, |e| {
        edit_json(e, "manifest.json", |v| {
            v["schema"] = 3.into();
            v["minReader"] = 1.into();
            v["futureThing"] = "x".into();
        });
        edit_json(e, "project/document.json", |v| {
            v["layers"][0]["props"]["glow"] = 0.5.into();
            v["newTopLevel"] = serde_json::json!({"a": 1});
        });
        e.push(("future/extra.bin".into(), vec![1, 2, 3], Method::Deflated));
    });
    let l = decode(&b).unwrap();
    assert_eq!(l.doc, simple_document());
    assert!(l.warnings.contains(&FormatWarning::NewerSchema { schema: 3 }));
    assert!(l.warnings.iter().any(|w| matches!(w, FormatWarning::UnknownFields { entry, paths }
        if entry == "manifest.json" && paths == &vec!["/futureThing".to_string()])));
    assert!(l.warnings.iter().any(|w| matches!(w, FormatWarning::UnknownFields { entry, paths }
        if entry == "project/document.json" && paths.contains(&"/layers/0/props/glow".to_string())
            && paths.contains(&"/newTopLevel".to_string()))));
    assert!(l.warnings.contains(&FormatWarning::UnknownEntries { names: vec!["future/extra.bin".into()] }));
}
