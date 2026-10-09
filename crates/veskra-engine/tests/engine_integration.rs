#![allow(clippy::unwrap_used, clippy::expect_used)]
//! INT-01 (host part) and engine-level contract tests: stroke → tiles → composite → `.veskra`
//! → reopen → identical composite; atomic undo boundaries; layer policies; JSON boundary.

use serde_json::{Value, json};
use veskra_core::RectI;
use veskra_engine::{Command, EngineConfig, SampleSpec, Session};

fn session() -> Session {
    Session::new(EngineConfig { deterministic_seed: Some(42), ..Default::default() })
}

fn ok(s: &mut Session, cmd: Value) -> Value {
    let out: Value = serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap();
    assert!(out.get("error").is_none(), "command {cmd} failed: {out}");
    out
}

fn err(s: &mut Session, cmd: Value) -> Value {
    let out: Value = serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap();
    assert!(out.get("error").is_some(), "command {cmd} unexpectedly succeeded: {out}");
    out["error"].clone()
}

fn stroke(s: &mut Session, pts: &[(f32, f32, f32)]) {
    ok(s, json!({"cmd": "stroke.begin"}));
    let samples: Vec<Value> = pts
        .iter()
        .enumerate()
        .map(|(i, (x, y, p))| json!({"x": x, "y": y, "t": i as u64 * 4_000_000, "p": p}))
        .collect();
    ok(s, json!({"cmd": "stroke.samples", "samples": samples}));
    ok(s, json!({"cmd": "stroke.commit"}));
}

fn wave(n: usize, w: f32, y: f32) -> Vec<(f32, f32, f32)> {
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            (w * (0.1 + 0.8 * t), y + 20.0 * (t * 9.0).sin(), 0.2 + 0.8 * (t * 3.0).sin())
        })
        .collect()
}

fn render(s: &Session, w: u32, h: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    s.render_display(RectI::from_size(w, h), &mut buf).unwrap();
    buf
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-engine-it-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn int01_stroke_save_reopen_identical() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 512, "height": 384, "background": "#FFFFFF"}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#1B2A6B"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.brush"}));
    stroke(&mut s, &wave(200, 512.0, 150.0));
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector", "name": "Ink"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    stroke(&mut s, &wave(120, 512.0, 260.0));
    ok(&mut s, json!({"cmd": "layer.add", "kind": "raster", "name": "Texture"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "charcoal"}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#B04020"}));
    stroke(&mut s, &wave(150, 512.0, 80.0));
    let before = render(&s, 512, 384);
    let info = ok(&mut s, json!({"cmd": "document.info"}))["ok"].clone();
    assert_eq!(info["strokeCount"], 3);
    assert_eq!(info["dirty"], true);

    let path = tmp("int01.veskra");
    let saved = ok(&mut s, json!({"cmd": "document.save", "path": path.to_string_lossy()}));
    assert!(saved["ok"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(ok(&mut s, json!({"cmd": "document.info"}))["ok"]["dirty"], false);

    let mut s2 = session();
    ok(&mut s2, json!({"cmd": "document.open", "path": path.to_string_lossy()}));
    let after = render(&s2, 512, 384);
    assert_eq!(before, after, "reopened composite must be bit-identical (G1, SAV-001)");
    assert_eq!(s.document().unwrap().as_ref(), s2.document().unwrap().as_ref(), "document state round-trips");
    let layers = ok(&mut s2, json!({"cmd": "layer.list"}))["ok"]["layers"].clone();
    let names: Vec<&str> = layers.as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Texture", "Ink", "Layer 1", "Background"]);
    let ink = layers.as_array().unwrap().iter().find(|l| l["name"] == "Ink").unwrap();
    assert_eq!(ink["vectorFidelity"], "exact");
    let tex = layers.as_array().unwrap().iter().find(|l| l["name"] == "Texture").unwrap();
    assert_eq!(tex["vectorFidelity"], "partially_vector");
}

#[test]
fn stroke_is_one_atomic_undo_step_and_cancel_leaves_no_trace() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 256, "height": 256, "background": "#FFFFFF"}));
    let blank = render(&s, 256, 256);
    // Many sample batches -> still one undo step.
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    for chunk in wave(100, 256.0, 128.0).chunks(7) {
        let samples: Vec<Value> = chunk.iter().map(|(x, y, p)| json!({"x": x, "y": y, "t": 0, "p": p})).collect();
        ok(&mut s, json!({"cmd": "stroke.samples", "samples": samples}));
    }
    // undo is refused mid-stroke (no partial history)
    err(&mut s, json!({"cmd": "history.undo"}));
    ok(&mut s, json!({"cmd": "stroke.commit"}));
    let painted = render(&s, 256, 256);
    assert_ne!(painted, blank);
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(render(&s, 256, 256), blank);
    ok(&mut s, json!({"cmd": "history.redo"}));
    assert_eq!(render(&s, 256, 256), painted);
    // Cancelled stroke changes nothing and creates no history.
    let ver = s.version();
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    let samples: Vec<Value> = wave(30, 256.0, 60.0).iter().map(|(x, y, p)| json!({"x": x, "y": y, "p": p})).collect();
    ok(&mut s, json!({"cmd": "stroke.samples", "samples": samples}));
    assert_ne!(render(&s, 256, 256), painted, "live stroke is displayed");
    ok(&mut s, json!({"cmd": "stroke.cancel"}));
    assert_eq!(render(&s, 256, 256), painted);
    assert_eq!(s.version(), ver);
    assert_eq!(ok(&mut s, json!({"cmd": "document.info"}))["ok"]["undoLabel"], "Stroke");
}

#[test]
fn failed_commands_are_atomic() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 64, "height": 64}));
    let doc_before = s.document().unwrap().clone();
    let v = s.version();
    err(&mut s, json!({"cmd": "layer.set", "id": "00000000-0000-0000-0000-000000000000", "opacity": 0.5}));
    err(&mut s, json!({"cmd": "layer.move", "id": "not-an-id", "placement": {"at": "top"}}));
    err(&mut s, json!({"cmd": "edit.fill", "x": 1000.0, "y": 1.0}));
    err(&mut s, json!({"cmd": "edit.transform", "matrix": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]}));
    err(&mut s, json!({"cmd": "color.set", "hex": "#XYZ"}));
    err(&mut s, json!({"cmd": "brush.set", "size": -3.0}));
    err(&mut s, json!({"cmd": "no.such.command"}));
    assert_eq!(s.version(), v);
    assert_eq!(s.document().unwrap().as_ref(), doc_before.as_ref());
}

#[test]
fn locked_and_vector_layer_policies() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 128, "height": 128}));
    let active = ok(&mut s, json!({"cmd": "document.info"}))["ok"]["activeLayer"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "locked": true}));
    let e = err(&mut s, json!({"cmd": "stroke.begin"}));
    assert_eq!(e["code"], "LayerLocked");
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "charcoal"}));
    let e = err(&mut s, json!({"cmd": "stroke.begin"}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
    let e = err(&mut s, json!({"cmd": "edit.fill_selection"}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    stroke(&mut s, &[(10.0, 10.0, 0.5), (100.0, 100.0, 0.9)]);
    let e = err(&mut s, json!({"cmd": "stroke.begin", "eraser": true}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
}

#[test]
fn fill_selection_transform_and_pick() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 128, "height": 128, "background": "#FFFFFF"}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#FF0000"}));
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 10, "y0": 10, "x1": 50, "y1": 50}));
    ok(&mut s, json!({"cmd": "edit.fill_selection"}));
    let p = ok(&mut s, json!({"cmd": "color.pick", "x": 20.5, "y": 20.5}));
    assert_eq!(p["ok"]["picked"], "#FF0000");
    let p = ok(&mut s, json!({"cmd": "color.pick", "x": 70.5, "y": 70.5}));
    assert_eq!(p["ok"]["picked"], "#FFFFFF");
    // Move selection content by +40,+40.
    ok(&mut s, json!({"cmd": "edit.transform", "matrix": [1, 0, 0, 1, 40, 40]}));
    assert_eq!(ok(&mut s, json!({"cmd": "color.pick", "x": 70.5, "y": 70.5}))["ok"]["picked"], "#FF0000");
    assert_eq!(ok(&mut s, json!({"cmd": "color.pick", "x": 20.5, "y": 20.5}))["ok"]["picked"], "#FFFFFF");
    ok(&mut s, json!({"cmd": "selection.clear"}));
    // Bucket fill on the active (transparent) layer bounded by nothing -> whole canvas.
    ok(&mut s, json!({"cmd": "color.set", "hex": "#00FF00"}));
    ok(&mut s, json!({"cmd": "edit.fill", "x": 5.0, "y": 5.0, "tolerance": 0.0, "sampleMerged": true}));
    assert_eq!(ok(&mut s, json!({"cmd": "color.pick", "x": 5.5, "y": 5.5}))["ok"]["picked"], "#00FF00");
    assert_eq!(ok(&mut s, json!({"cmd": "color.pick", "x": 70.5, "y": 70.5}))["ok"]["picked"], "#FF0000");
    let layers = ok(&mut s, json!({"cmd": "layer.list"}))["ok"]["layers"].clone();
    assert_eq!(layers[0]["vectorFidelity"], "raster", "fills make a layer raster-authoritative");
}

#[test]
fn layer_tree_commands_and_history_labels() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 64, "height": 64}));
    let g = ok(&mut s, json!({"cmd": "layer.add", "kind": "group", "name": "Group"}))["ok"]["layerId"]
        .as_str()
        .unwrap()
        .to_string();
    let a =
        ok(&mut s, json!({"cmd": "layer.add", "kind": "raster", "name": "A", "placement": {"at": "into", "group": g}}))
            ["ok"]["layerId"]
            .as_str()
            .unwrap()
            .to_string();
    let e = err(&mut s, json!({"cmd": "layer.move", "id": g, "placement": {"at": "into", "group": g}}));
    assert_eq!(e["code"], "InvalidArgument");
    ok(
        &mut s,
        json!({"cmd": "layer.set", "id": a, "blend": "multiply", "opacity": 0.5, "clip": true, "alphaLocked": true}),
    );
    let dup = ok(&mut s, json!({"cmd": "layer.duplicate", "id": g}))["ok"]["layerId"].as_str().unwrap().to_string();
    assert_ne!(dup, g);
    let list = ok(&mut s, json!({"cmd": "layer.list"}))["ok"]["layers"].clone();
    assert_eq!(list.as_array().unwrap().len(), 6);
    assert_eq!(ok(&mut s, json!({"cmd": "document.info"}))["ok"]["undoLabel"], "Duplicate layer");
    ok(&mut s, json!({"cmd": "layer.remove", "id": dup}));
    ok(&mut s, json!({"cmd": "history.undo"}));
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(ok(&mut s, json!({"cmd": "layer.list"}))["ok"]["layers"].as_array().unwrap().len(), 4);
    let e = err(&mut s, json!({"cmd": "layer.set", "id": a, "blend": "dissolve"}));
    assert_eq!(e["code"], "InvalidArgument");
}

#[test]
fn predicted_samples_never_reach_document() {
    use veskra_core::{SampleFlags, StrokeSample};
    let mut s = session();
    s.execute(Command::DocumentCreate { width: 64, height: 64, dpi: 72.0, background: None }).unwrap();
    s.execute(Command::StrokeBegin { eraser: false }).unwrap();
    s.execute(Command::StrokeSamples {
        samples: vec![SampleSpec { x: 5.0, y: 5.0, t: 0, p: Some(0.5), tilt: None, azimuth: None }],
    })
    .unwrap();
    s.execute(Command::StrokeCommit).unwrap();
    let doc = s.document().unwrap();
    for st in doc.iter_strokes() {
        assert!(
            st.samples().iter().all(|x: &StrokeSample| x.is_confirmed() && !x.flags.contains(SampleFlags::PREDICTED))
        );
    }
}

#[test]
fn export_all_formats_and_reject_ai() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 200, "height": 120, "background": "#FFFFFF"}));
    stroke(&mut s, &wave(50, 200.0, 60.0));
    for ext in ["png", "psd", "svg", "pdf"] {
        let path = tmp(&format!("export.{ext}"));
        let pre = ok(&mut s, json!({"cmd": "export.preflight", "format": ext}));
        assert!(pre["ok"]["blockedReason"].is_null(), "{ext}: {pre}");
        let rep = ok(&mut s, json!({"cmd": "export.run", "format": ext, "path": path.to_string_lossy()}));
        assert_eq!(rep["ok"]["success"], true, "{ext}: {rep}");
        assert_eq!(rep["ok"]["validation"], "structurally_validated");
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }
    // ILL-004: both toggles off blocks SVG export.
    let opts = json!({"includeVectorPaths": false, "includeRasterAppearance": false, "embedColorProfile": true});
    let pre = ok(&mut s, json!({"cmd": "export.preflight", "format": "svg", "options": opts}));
    assert!(pre["ok"]["blockedReason"].is_string());
    err(&mut s, json!({"cmd": "export.run", "format": "svg", "path": tmp("x.svg").to_string_lossy(), "options": opts}));
    // ILL-008: no .ai output, ever.
    err(&mut s, json!({"cmd": "export.run", "format": "pdf", "path": tmp("x.ai").to_string_lossy()}));
    err(&mut s, json!({"cmd": "export.run", "format": "ai", "path": tmp("x.ai").to_string_lossy()}));
    assert!(!tmp("x.ai").exists());
}

#[test]
fn autosave_and_recovery_listing() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 64, "height": 64}));
    stroke(&mut s, &[(5.0, 5.0, 0.5), (50.0, 40.0, 0.8)]);
    let dir = tmp("autosave");
    ok(&mut s, json!({"cmd": "document.autosave", "dir": dir.to_string_lossy()}));
    ok(&mut s, json!({"cmd": "document.autosave", "dir": dir.to_string_lossy()}));
    let list = ok(&mut s, json!({"cmd": "recovery.list", "dir": dir.to_string_lossy()}))["ok"]["checkpoints"].clone();
    let arr = list.as_array().unwrap();
    assert!(!arr.is_empty());
    assert!(arr.iter().all(|c| c["valid"] == true));
    let p = arr.last().unwrap()["path"].as_str().unwrap().to_string();
    let mut s2 = session();
    ok(&mut s2, json!({"cmd": "recovery.open", "path": p}));
    let info = ok(&mut s2, json!({"cmd": "document.info"}))["ok"].clone();
    assert_eq!(info["strokeCount"], 1);
    assert_eq!(info["dirty"], true, "a recovered checkpoint is never marked as saved");
}
