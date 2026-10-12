//! Committed `.veskra` fixtures (tests/fixtures/documents) — format compatibility guard.
//!
//! The committed files must keep loading (and decode to the same documents) even when the
//! encoder changes. Regenerate deliberately with:
//! `VESKRA_REGEN_FIXTURES=1 cargo test -p veskra-format --test fixtures`
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use veskra_core::{ErrorCode, VeskraError};
use veskra_format::{FormatError, decode, encode, inspect};

const MAX_FIXTURE_BYTES: usize = 200 * 1024;

fn dir() -> PathBuf {
    veskra_testkit::fixtures_dir().join("documents")
}

fn generate() -> Vec<(&'static str, Vec<u8>)> {
    let simple = encode(&simple_document(), &meta(), None).unwrap();
    let twelve = encode(&twelve_layer_document(), &meta(), None).unwrap();
    let corrupt = tamper(&simple, |e| {
        e.iter_mut().find(|x| x.0 == "manifest.json").unwrap().1 = b"{\"format\": \"veskra\", \"schema\": 1,".to_vec();
    });
    let future = tamper(&simple, |e| {
        edit_json(e, "manifest.json", |v| {
            v["schema"] = 3.into();
            v["minReader"] = 3.into();
        })
    });
    let truncated = simple[..simple.len() * 6 / 10].to_vec();
    vec![
        ("simple-1-layer.veskra", simple),
        ("12-layer-illustration.veskra", twelve),
        ("corrupt-manifest.veskra", corrupt),
        ("future-schema.veskra", future),
        ("truncated.veskra", truncated),
    ]
}

/// Write the fixtures once per test process when `VESKRA_REGEN_FIXTURES=1` (before any read).
fn regenerate_if_requested() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        if std::env::var("VESKRA_REGEN_FIXTURES").as_deref() != Ok("1") {
            return;
        }
        fs::create_dir_all(dir()).unwrap();
        for (name, bytes) in generate() {
            assert!(bytes.len() < MAX_FIXTURE_BYTES, "{name} is {} bytes", bytes.len());
            fs::write(dir().join(name), bytes).unwrap();
        }
    });
}

#[test]
fn regenerate_fixtures_when_requested() {
    regenerate_if_requested();
}

fn read(name: &str) -> Vec<u8> {
    regenerate_if_requested();
    let b = fs::read(dir().join(name)).unwrap_or_else(|e| panic!("fixture {name} missing ({e}); regenerate"));
    assert!(b.len() < MAX_FIXTURE_BYTES, "{name} too large");
    b
}

#[test]
fn committed_valid_fixtures_load_to_expected_documents() {
    let l = decode(&read("simple-1-layer.veskra")).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.doc, simple_document());
    let l = decode(&read("12-layer-illustration.veskra")).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.doc, twelve_layer_document());
    assert_eq!(l.doc.layer_count(), 12);
    assert_eq!(l.manifest.schema, 1);
}

#[test]
fn committed_invalid_fixtures_fail_with_typed_errors() {
    let code = |n: &str| VeskraError::from(decode(&read(n)).unwrap_err()).code;
    assert_eq!(code("corrupt-manifest.veskra"), ErrorCode::CorruptData);
    assert_eq!(code("truncated.veskra"), ErrorCode::CorruptData);
    let e = decode(&read("future-schema.veskra")).unwrap_err();
    assert!(matches!(e, FormatError::UnsupportedSchema { schema: 3, min_reader: 3, .. }), "{e:?}");
    assert!(!inspect(&read("future-schema.veskra")).unwrap().schema_supported);
}

#[test]
fn generator_output_matches_committed_semantics() {
    // Fresh output decodes to the same documents as the committed files (guards the generator
    // and the fixture builders against silent drift; bytes may legitimately differ).
    for (name, bytes) in generate() {
        let fresh = decode(&bytes).map(|l| l.doc);
        let committed = decode(&read(name)).map(|l| l.doc);
        match (fresh, committed) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{name}"),
            (Err(a), Err(b)) => assert_eq!(a.code(), b.code(), "{name}"),
            (a, b) => panic!("{name}: fresh {:?} vs committed {:?}", a.is_ok(), b.is_ok()),
        }
    }
}
