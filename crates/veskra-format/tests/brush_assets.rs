//! ADR-019: schema-2 bundles embedding brush assets — round trip, schema selection, missing /
//! corrupt / oversized assets, forward compatibility.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use std::sync::Arc;
use veskra_core::{ColorRgba, ErrorCode, VeskraError};
use veskra_doc::{Document, Placement, StrokeMode};
use veskra_format::zip::Method;
use veskra_format::{CURRENT_SCHEMA, FormatError, FormatWarning, decode, encode};

const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const H2: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdef0123";

fn png(seed: u8) -> Arc<[u8]> {
    let px: Vec<u8> = (0..16 * 16).flat_map(|i| [seed, (i % 251) as u8, 0, 255]).collect();
    Arc::from(veskra_testkit::encode_png_rgba8(16, 16, &px))
}

/// simple_document + a stroke on a new layer referencing H1 and H2 (both embedded).
fn doc_with_assets() -> Document {
    let mut d = simple_document();
    d.add_layer(
        lid(7),
        veskra_doc::LayerProps::named("Brushes"),
        veskra_doc::LayerKind::Raster(veskra_doc::PixelContent::empty()),
        Placement::Top,
    )
    .unwrap();
    let prof = d.color.profile.clone();
    let rec = stroke(sid(9), lid(7), 5, ColorRgba::new(0.1, 0.2, 0.3, 1.0), StrokeMode::Paint, None, prof, false)
        .with_brush_assets(vec![H2.into(), H1.into(), H1.into()])
        .unwrap();
    assert_eq!(rec.brush_assets, vec![H1.to_string(), H2.to_string()], "sorted, deduplicated");
    d.add_stroke(rec).unwrap();
    d.insert_brush_asset(H1, png(1)).unwrap();
    d.insert_brush_asset(H2, png(2)).unwrap();
    d
}

#[test]
fn assets_round_trip_as_schema_2_and_plain_documents_stay_schema_1() {
    let d = doc_with_assets();
    let bytes = encode(&d, &meta(), None).unwrap();
    let names: Vec<String> = unpack(&bytes).into_iter().map(|e| e.0).collect();
    assert!(names.contains(&format!("brushes/{H1}.png")) && names.contains(&format!("brushes/{H2}.png")));
    let l = decode(&bytes).unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.manifest.schema, CURRENT_SCHEMA);
    assert_eq!(l.manifest.min_reader, 1, "a schema-1 reader may still open it");
    assert_eq!(l.doc, d);
    assert_eq!(l.doc.brush_asset(H1).map(|b| b.len()), Some(png(1).len()));
    // Deterministic bytes.
    assert_eq!(encode(&l.doc, &meta(), None).unwrap(), bytes);
    // No asset → schema 1, exactly as before brush system v2.
    let plain = decode(&encode(&simple_document(), &meta(), None).unwrap()).unwrap();
    assert_eq!(plain.manifest.schema, 1);
}

#[test]
fn unreferenced_assets_are_not_saved() {
    let mut d = simple_document();
    d.insert_brush_asset(H1, png(1)).unwrap();
    let bytes = encode(&d, &meta(), None).unwrap();
    assert!(!unpack(&bytes).iter().any(|e| e.0.starts_with("brushes/")));
    let l = decode(&bytes).unwrap();
    assert_eq!(l.manifest.schema, 1);
    assert!(l.doc.brush_asset(H1).is_none());
}

#[test]
fn missing_reference_is_a_warning_and_survives_resave() {
    let mut d = simple_document();
    d.add_layer(
        lid(7),
        veskra_doc::LayerProps::named("B"),
        veskra_doc::LayerKind::Raster(veskra_doc::PixelContent::empty()),
        Placement::Top,
    )
    .unwrap();
    let prof = d.color.profile.clone();
    let rec = stroke(sid(9), lid(7), 5, ColorRgba::BLACK, StrokeMode::Paint, None, prof, false)
        .with_brush_assets(vec![H1.into()])
        .unwrap();
    d.add_stroke(rec).unwrap();
    // Asset bytes absent (e.g. the file came from a reader that dropped them).
    let bytes = encode(&d, &meta(), None).unwrap();
    let l = decode(&bytes).unwrap();
    assert_eq!(l.manifest.schema, 2, "a reference alone is schema-2 data");
    assert!(l.warnings.contains(&FormatWarning::MissingBrushAssets { count: 1 }));
    assert_eq!(l.doc.stroke(sid(9)).unwrap().brush_assets, vec![H1.to_string()]);
    // Re-saving keeps the reference (never silently rewritten).
    let again = decode(&encode(&l.doc, &meta(), None).unwrap()).unwrap();
    assert_eq!(again.doc, l.doc);
}

#[test]
fn corrupt_missing_or_oversized_assets_are_typed_errors() {
    let good = encode(&doc_with_assets(), &meta(), None).unwrap();
    let code = |b: &[u8]| VeskraError::from(decode(b).unwrap_err()).code;
    // Entry listed in document.json but absent from the archive.
    let gone = tamper(&good, |e| e.retain(|x| x.0 != format!("brushes/{H1}.png")));
    assert_eq!(code(&gone), ErrorCode::CorruptData);
    // Not a PNG (integrity recomputed so only the content check can catch it).
    let not_png = tamper(&good, |e| {
        let x = e.iter_mut().find(|x| x.0 == format!("brushes/{H1}.png")).unwrap();
        x.1 = b"GIF89a....".to_vec();
    });
    assert_eq!(code(&not_png), ErrorCode::CorruptData);
    // Malformed hash in the list / in a stroke reference.
    let bad_list = tamper(&good, |e| edit_json(e, "project/document.json", |v| v["brushAssets"][0] = "../../x".into()));
    assert_eq!(code(&bad_list), ErrorCode::CorruptData);
    let bad_ref = tamper(&good, |e| {
        edit_json(e, "project/document.json", |v| {
            let strokes = v["strokes"].as_array_mut().unwrap();
            let s = strokes.iter_mut().find(|s| !s["brush"]["assets"].is_null()).unwrap();
            s["brush"]["assets"][0] = "ZZ".into();
        })
    });
    assert_eq!(code(&bad_ref), ErrorCode::CorruptData);
    // Oversized asset entry.
    let big_hash = H2;
    let big = tamper(&good, |e| {
        let x = e.iter_mut().find(|x| x.0 == format!("brushes/{big_hash}.png")).unwrap();
        let mut b = vec![0u8; veskra_doc::MAX_BRUSH_ASSET_BYTES + 1];
        b[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        x.1 = b;
        x.2 = Method::Deflated;
    });
    let e = decode(&big).unwrap_err();
    assert!(matches!(e, FormatError::Limit(_)), "{e:?}");
}

#[test]
fn document_store_bounds() {
    let mut d = simple_document();
    assert!(d.insert_brush_asset("nothex", png(1)).is_err());
    assert!(d.insert_brush_asset(H1, Arc::from(&b"not a png"[..])).is_err());
    let too_big: Arc<[u8]> = {
        let mut b = vec![0u8; veskra_doc::MAX_BRUSH_ASSET_BYTES + 1];
        b[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        Arc::from(b)
    };
    assert_eq!(d.insert_brush_asset(H1, too_big).unwrap_err().code, ErrorCode::AllocationLimit);
    let rec = stroke(sid(99), lid(1), 3, ColorRgba::BLACK, StrokeMode::Paint, None, d.color.profile.clone(), false);
    assert!(rec.clone().with_brush_assets(vec!["A".repeat(64)]).is_err(), "uppercase hex rejected");
    assert!(rec.with_brush_assets((0..9).map(|i| format!("{i:064x}")).collect()).is_err());
}
