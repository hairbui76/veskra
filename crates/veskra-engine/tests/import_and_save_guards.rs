#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `layer.import_image` (COL-004/005 through the engine) and project-save extension guards.

use serde_json::{Value, json};
use veskra_core::RectI;
use veskra_engine::{EngineConfig, Session};

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/import/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn run(s: &mut Session, cmd: Value) -> Value {
    serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap()
}

fn new_session() -> Session {
    let mut s = Session::new(EngineConfig { deterministic_seed: Some(5), ..Default::default() });
    let r = run(&mut s, json!({"cmd": "document.create", "width": 128, "height": 96}));
    assert!(r.get("ok").is_some(), "{r}");
    s
}

fn render(s: &Session) -> Vec<u8> {
    let mut b = vec![0u8; 128 * 96 * 4];
    s.render_display(RectI::from_size(128, 96), &mut b).unwrap();
    b
}

#[test]
fn untagged_import_is_flagged_as_assumed_and_undoable() {
    let mut s = new_session();
    let before = render(&s);
    let r = run(&mut s, json!({"cmd": "layer.import_image", "path": fixture("untagged_rgb8.png")}));
    assert_eq!(r["ok"]["profileAssumed"], true, "{r}");
    assert!(!r["warnings"].as_array().unwrap().is_empty(), "assumed profile must be surfaced: {r}");
    assert_eq!(r["historyChanged"], true);
    let layers = run(&mut s, json!({"cmd": "layer.list"}));
    assert_eq!(layers["ok"]["layers"][0]["name"], "Imported image");
    assert_eq!(layers["ok"]["layers"][0]["vectorFidelity"], "raster");
    assert_ne!(render(&s), before);
    run(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(render(&s), before);
}

#[test]
fn rejecting_missing_profile_changes_nothing() {
    let mut s = new_session();
    let v = s.version();
    let r = run(
        &mut s,
        json!({"cmd": "layer.import_image", "path": fixture("untagged_rgb8.png"), "missingProfile": "reject"}),
    );
    assert!(r.get("error").is_some(), "{r}");
    assert_eq!(s.version(), v);
}

#[test]
fn convert_and_assign_are_visibly_different_for_p3() {
    let mut a = new_session();
    let mut c = new_session();
    let ra =
        run(&mut a, json!({"cmd": "layer.import_image", "path": fixture("p3_icc_rgb8.png"), "handling": "assign"}));
    let rc =
        run(&mut c, json!({"cmd": "layer.import_image", "path": fixture("p3_icc_rgb8.png"), "handling": "convert"}));
    assert_eq!(ra["ok"]["converted"], false, "{ra}");
    assert_eq!(rc["ok"]["converted"], true, "{rc}");
    assert_ne!(render(&a), render(&c), "COL-005: assign and convert must differ for a P3 source");
}

#[test]
fn import_offset_places_pixels() {
    let mut s = new_session();
    let r =
        run(&mut s, json!({"cmd": "layer.import_image", "path": fixture("srgb_chunk_rgba8.png"), "x": 64, "y": 48}));
    assert!(r.get("ok").is_some(), "{r}");
    let inv = &r["invalidated"][0];
    assert_eq!(inv[0], 64);
    assert_eq!(inv[1], 48);
    let px = render(&s);
    assert_eq!(&px[0..4], &[0, 0, 0, 0], "origin stays transparent after an offset import");
}

#[test]
fn hostile_or_missing_files_are_typed_errors() {
    let mut s = new_session();
    let dir = std::env::temp_dir().join(format!("veskra-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let junk = dir.join("junk.png");
    std::fs::write(&junk, b"\x89PNG\r\n\x1a\nnot really").unwrap();
    for p in [junk.to_string_lossy().to_string(), dir.join("missing.png").to_string_lossy().to_string()] {
        let r = run(&mut s, json!({"cmd": "layer.import_image", "path": p}));
        assert!(r.get("error").is_some(), "{r}");
    }
    let r =
        run(&mut s, json!({"cmd": "layer.import_image", "path": fixture("untagged_rgb8.png"), "handling": "magic"}));
    assert_eq!(r["error"]["code"], "InvalidArgument");
}

#[test]
fn project_save_refuses_interchange_and_ai_extensions() {
    let mut s = new_session();
    let dir = std::env::temp_dir().join(format!("veskra-saveguard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for ext in ["ai", "AI", "psd", "pdf", "svg", "png"] {
        let p = dir.join(format!("project.{ext}"));
        let r = run(&mut s, json!({"cmd": "document.save", "path": p.to_string_lossy()}));
        assert_eq!(r["error"]["code"], "InvalidArgument", "{ext}: {r}");
        assert!(!p.exists());
    }
    let ok = dir.join("project.veskra");
    let r = run(&mut s, json!({"cmd": "document.save", "path": ok.to_string_lossy()}));
    assert!(r.get("ok").is_some(), "{r}");
}
