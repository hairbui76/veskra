//! Byte-for-byte pin of every exporter's output for the **default** `ExportOptions`.
//!
//! The digests below were produced by running this very file against the tree *before* the
//! export color options (`profile`, `bitDepth`) were added (commit 15eecf8), so a pass proves the
//! default-option output of PNG, PSD, SVG and PDF did not change by a single byte.
//! Regenerate (only for an intended format change) with
//! `VESKRA_PRINT_EXPORT_PINS=1 cargo test -p veskra-io --test export_default_pin -- --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod psd_fixtures;

use psd_fixtures as fx;
use veskra_core::export::{ExportFormat, ExportOptions};
use veskra_io::{CancelToken, exporter_for};

/// CRC-32 + FNV-1a 64 + length: cheap, dependency-free and ample for regression pinning.
fn digest(bytes: &[u8]) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}-{h:016x}-{}", crc32fast::hash(bytes), bytes.len())
}

fn export(doc: &veskra_doc::Document, format: ExportFormat) -> Vec<u8> {
    let exp = exporter_for(format);
    let mut out = Vec::new();
    exp.export(doc, &ExportOptions::default(), &mut out, &CancelToken::new()).unwrap();
    exp.validate(&out).unwrap();
    out
}

const PINS: &[(&str, &str, &str)] = &[
    // (fixture, format, digest) — generated on the pre-change tree, see module docs.
    ("simple", "png", "e346bd23-930ff35a16e49a8e-630"),
    ("simple", "psd", "e36a32c5-d3fa6a3f400cb584-11472"),
    ("simple", "svg", "39324868-a3dd30a45ea60b10-1703"),
    ("simple", "pdf", "a5d60ccb-0795878844ef1804-8326"),
    ("six_layer", "png", "f9064c0a-fafd5e9f6f25c8d3-11272"),
    ("six_layer", "psd", "c00d46c4-378211e73a8d983f-84786"),
    ("six_layer", "svg", "c795eeb3-ac759b8660a9ada6-15739"),
    ("six_layer", "pdf", "71bcd142-b0fbb53233c63fc8-10948"),
    ("twelve_layer", "png", "c562311b-5643e91c021cd1f7-35215"),
    ("twelve_layer", "psd", "7c5ea166-be7844dd2d23f0a1-522963"),
    ("twelve_layer", "svg", "21a2eb6d-d6a09ebce3c7f7f8-47987"),
    ("twelve_layer", "pdf", "d36502d4-c572606d5c97ae6d-53229"),
    ("empty_and_inverse_mask", "png", "7ccedcee-8c401c8d0b4857fc-1222"),
    ("empty_and_inverse_mask", "psd", "77b0d186-498d1b87bcee68e3-7824"),
    ("empty_and_inverse_mask", "svg", "3d79fe7d-263e216a1e04870e-2509"),
    ("empty_and_inverse_mask", "pdf", "a09e7336-7471c511cd321384-5825"),
    ("large_sparse", "png", "16e732e5-9e11014790a8c117-34361"),
    ("large_sparse", "psd", "a2a197ab-fdcf55aed813c620-405954"),
    ("large_sparse", "svg", "86a6688a-6293999edb5ec082-44672"),
    ("large_sparse", "pdf", "58d38c61-a4bf2e91b30da861-30294"),
];

#[test]
fn default_option_exports_are_byte_identical_to_the_pre_change_tree() {
    let formats = [
        ("png", ExportFormat::Png),
        ("psd", ExportFormat::Psd),
        ("svg", ExportFormat::Svg),
        ("pdf", ExportFormat::Pdf),
    ];
    let mut lines = Vec::new();
    let mut mismatches = Vec::new();
    for (name, doc) in fx::all() {
        for (ext, format) in formats {
            let d = digest(&export(&doc, format));
            lines.push(format!("    (\"{name}\", \"{ext}\", \"{d}\"),"));
            match PINS.iter().find(|(n, e, _)| *n == name && *e == ext) {
                Some((_, _, want)) if *want == d => {}
                Some((_, _, want)) => mismatches.push(format!("{name}.{ext}: got {d}, pinned {want}")),
                None => mismatches.push(format!("{name}.{ext}: no pin (got {d})")),
            }
        }
    }
    if std::env::var_os("VESKRA_PRINT_EXPORT_PINS").is_some() {
        println!("{}", lines.join("\n"));
        return;
    }
    assert_eq!(PINS.len(), lines.len(), "one pin per fixture and format");
    assert!(mismatches.is_empty(), "default-option export bytes changed:\n{}", mismatches.join("\n"));
}
