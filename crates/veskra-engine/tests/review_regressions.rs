#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Independent-review regressions (AND + QAP review of veskra-engine). Each test pins a defect
//! that was found by review and fixed; see the test doc comments.

use serde_json::{Value, json};
use veskra_core::{RectI, ToolType};
use veskra_engine::veskra_input::{PointerAction, RawPointerEvent};
use veskra_engine::{Command, EngineConfig, Session, flags};

fn session() -> Session {
    Session::new(EngineConfig { deterministic_seed: Some(7), ..Default::default() })
}

fn run(s: &mut Session, cmd: Value) -> Value {
    serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap()
}

fn ok(s: &mut Session, cmd: Value) -> Value {
    let out = run(s, cmd.clone());
    assert!(out.get("error").is_none(), "command {cmd} failed: {out}");
    out
}

fn err(s: &mut Session, cmd: Value) -> Value {
    let out = run(s, cmd.clone());
    assert!(out.get("error").is_some(), "command {cmd} unexpectedly succeeded: {out}");
    out["error"].clone()
}

fn info(s: &mut Session) -> Value {
    ok(s, json!({"cmd": "document.info"}))["ok"].clone()
}

fn render(s: &Session) -> Vec<u8> {
    let d = s.document().unwrap();
    let (w, h) = (d.width(), d.height());
    let mut buf = vec![0u8; (w * h * 4) as usize];
    s.render_display(RectI::from_size(w, h), &mut buf).unwrap();
    buf
}

/// Defect: samples the brush renderer silently rejects (non-monotonic time) were still stored in
/// the live stroke, so `StrokeRecord::new` refused them at commit and the whole stroke — already
/// shown on screen — was discarded with an error (artwork loss).
#[test]
fn non_monotonic_samples_do_not_lose_the_stroke() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 128, "height": 128, "background": "#FFFFFF"}));
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    ok(
        &mut s,
        json!({"cmd": "stroke.samples", "samples": [
            {"x": 10.0, "y": 10.0, "t": 5_000_000, "p": 0.6},
            {"x": 60.0, "y": 60.0, "t": 9_000_000, "p": 0.6},
            {"x": 70.0, "y": 60.0, "t": 1_000_000, "p": 0.6},  // goes back in time: rejected by renderer
            {"x": 100.0, "y": 100.0, "t": 12_000_000, "p": 0.6}
        ]}),
    );
    let c = ok(&mut s, json!({"cmd": "stroke.commit"}));
    assert_eq!(c["ok"]["committed"], true, "{c}");
    let d = s.document().unwrap();
    let st = d.iter_strokes().next().unwrap();
    assert_eq!(st.samples().len(), 3, "record holds exactly the samples that were rendered");
    assert!(st.samples().windows(2).all(|w| w[0].t_ns <= w[1].t_ns));
}

/// Defect: a sample the renderer rejects for being out of coordinate range (|x| > 1e6) was stored
/// in the record although it never contributed pixels (record ≠ pixels; replay diverges).
#[test]
fn record_samples_match_rendered_samples() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 64, "height": 64}));
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    ok(&mut s, json!({"cmd": "stroke.samples", "samples": [{"x": 5e6, "y": 5.0, "t": 0, "p": 0.5}]}));
    let c = ok(&mut s, json!({"cmd": "stroke.commit"}));
    assert_eq!(c["ok"]["committed"], false, "nothing was rendered, so nothing is committed: {c}");
    assert_eq!(c["historyChanged"], false);
    assert_eq!(info(&mut s)["strokeCount"], 0);
}

/// Defect: on a vector layer, a stroke with an active selection (or alpha lock) was accepted at
/// `stroke.begin`, drawn live, and then rejected at commit by `Document::add_stroke` (vector
/// layers accept only vector-representable strokes) — the user's stroke vanished. It must be
/// refused up front, before any pixels are shown.
#[test]
fn vector_layer_refuses_selection_clipped_or_alpha_locked_strokes_at_begin() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 128, "height": 128}));
    let v = ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}))["ok"]["layerId"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 0, "y0": 0, "x1": 64, "y1": 64}));
    let e = err(&mut s, json!({"cmd": "stroke.begin"}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
    assert!(!s.has_live_stroke());
    ok(&mut s, json!({"cmd": "selection.clear"}));
    ok(&mut s, json!({"cmd": "layer.set", "id": v, "alphaLocked": true}));
    let e = err(&mut s, json!({"cmd": "stroke.begin"}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
    // Same policy on the hot input path: the stroke is swallowed with a warning, never shown.
    s.execute(Command::ViewSize { width: 128.0, height: 128.0 }).unwrap();
    let before = render(&s);
    let out = s.input_batch(&stylus_line(10.0, 10.0, 100.0, 100.0, 12, 0));
    assert_eq!(out.flags & flags::COMMITTED, 0);
    assert!(!out.warnings.is_empty());
    assert_eq!(render(&s), before);
}

/// Defect: duplicating a group made the *group* the active layer, so the next stroke failed with
/// "cannot paint on a group" (layer.add/layer.select never activate groups).
#[test]
fn duplicating_a_group_does_not_activate_the_group() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 64, "height": 64}));
    let active = info(&mut s)["activeLayer"].clone();
    let g = ok(&mut s, json!({"cmd": "layer.add", "kind": "group"}))["ok"]["layerId"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "layer.duplicate", "id": g}));
    assert_eq!(info(&mut s)["activeLayer"], active);
    ok(&mut s, json!({"cmd": "stroke.begin"}));
}

/// Defect: `stroke.commit` of a stroke with no samples reported `historyChanged: true` although
/// neither history nor version changed (UI would refresh undo state needlessly / inconsistently).
#[test]
fn empty_commit_does_not_claim_history_change() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 64, "height": 64}));
    let v = s.version();
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    let c = ok(&mut s, json!({"cmd": "stroke.commit"}));
    assert_eq!(c["ok"]["committed"], false);
    assert_eq!(c["historyChanged"], false, "{c}");
    assert_eq!(s.version(), v);
}

/// Decision + defect: failed commands must not advance the deterministic ID stream, so a script
/// replayed without its failing lines produces identical IDs (deterministic replay / bug repro).
#[test]
fn failed_commands_do_not_consume_ids() {
    let script_ok =
        [json!({"cmd": "document.create", "width": 32, "height": 32}), json!({"cmd": "layer.add", "kind": "raster"})];
    let failing = [
        json!({"cmd": "document.create", "width": 32, "height": 32, "background": "#nothex"}),
        json!({"cmd": "document.create", "width": 0, "height": 32}),
        json!({"cmd": "layer.duplicate", "id": "00000000-0000-0000-0000-000000000001"}),
        json!({"cmd": "layer.add", "kind": "raster", "placement": {"at": "into", "group": "00000000-0000-0000-0000-000000000001"}}),
    ];
    let mut a = session();
    for c in &script_ok {
        ok(&mut a, c.clone());
    }
    let mut b = session();
    for c in &failing {
        err(&mut b, c.clone());
    }
    ok(&mut b, script_ok[0].clone());
    for c in &failing[2..] {
        err(&mut b, c.clone());
    }
    ok(&mut b, script_ok[1].clone());
    assert_eq!(a.document().unwrap().as_ref(), b.document().unwrap().as_ref());
}

/// Decision: unknown fields are rejected. A misspelled optional field (`opactiy`) used to be
/// ignored, the command reported success and recorded an undo step that changed nothing.
#[test]
fn unknown_fields_are_rejected() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 32, "height": 32}));
    let id = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    let v = s.version();
    let e = err(&mut s, json!({"cmd": "layer.set", "id": id, "opactiy": 0.5}));
    assert_eq!(e["code"], "InvalidArgument");
    err(&mut s, json!({"cmd": "selection.rect", "x0": 0, "y0": 0, "x1": 4, "y1": 4, "mode": "add"}));
    // Field-less commands (unit variants) cannot misspell an argument; serde ignores extras there.
    ok(&mut s, json!({"cmd": "document.info", "verbose": true}));
    err(&mut s, json!({"cmd": "layer.move", "id": id, "placement": {"at": "above", "id": id, "x": 1}}));
    err(&mut s, json!({"cmd": "stroke.samples", "samples": [{"x": 1.0, "y": 1.0, "pressure": 0.5}]}));
    assert_eq!(s.version(), v);
    // What the Kotlin builders send keeps working (explicit nulls for optional fields included).
    ok(&mut s, json!({"cmd": "document.create", "width": 32, "height": 32, "dpi": 300, "background": null}));
}

/// JSON numbers that overflow f32 arrive as ±inf; every such field must be rejected, not used.
#[test]
fn overflowing_numbers_are_rejected_atomically() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 32, "height": 32}));
    let id = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    let v = s.version();
    for cmd in [
        r#"{"cmd":"layer.set","id":"ID","opacity":1e300}"#,
        r#"{"cmd":"brush.set","size":1e300}"#,
        r#"{"cmd":"color.pick","x":1e300,"y":0}"#,
        r#"{"cmd":"edit.fill","x":1e300,"y":0}"#,
        r#"{"cmd":"document.create","width":8,"height":8,"dpi":1e300}"#,
        r#"{"cmd":"view.size","width":1e999,"height":5}"#,
    ] {
        let out: Value = serde_json::from_str(&s.execute_json(&cmd.replace("ID", &id))).unwrap();
        assert!(out.get("error").is_some(), "{cmd} -> {out}");
    }
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    let out: Value =
        serde_json::from_str(&s.execute_json(r#"{"cmd":"stroke.samples","samples":[{"x":1e300,"y":1,"p":0.5}]}"#))
            .unwrap();
    assert!(out.get("error").is_some(), "{out}");
    ok(&mut s, json!({"cmd": "stroke.cancel"}));
    assert_eq!(s.version(), v);
    // Deep nesting / garbage never panics.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(s.execute_json(&deep).contains("error"));
    assert!(s.execute_json("\u{0}\u{ffff}").contains("error"));
}

/// Mixed headless + input-path use: when an input stroke is swallowed because a headless stroke
/// is already live, the arbiter's later Cancel must not cancel the headless stroke.
#[test]
fn swallowed_input_stroke_does_not_cancel_foreign_live_stroke() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 128, "height": 128}));
    s.execute(Command::ViewSize { width: 128.0, height: 128.0 }).unwrap();
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    ok(
        &mut s,
        json!({"cmd": "stroke.samples", "samples": [{"x": 5.0, "y": 5.0, "p": 0.5}, {"x": 50.0, "y": 5.0, "p": 0.5}]}),
    );
    // A stylus stroke starts (swallowed: one is already live), then a second finger cancels it.
    let mut evs = vec![ev(PointerAction::Down, 0, ToolType::Finger, 20.0, 20.0, 1)];
    evs.push(ev(PointerAction::Move, 0, ToolType::Finger, 30.0, 20.0, 2));
    evs.push(ev(PointerAction::PointerDown, 1, ToolType::Finger, 80.0, 80.0, 3));
    let _ = s.input_batch(&evs);
    assert!(s.has_live_stroke(), "the headless stroke must survive");
    let c = ok(&mut s, json!({"cmd": "stroke.commit"}));
    assert_eq!(c["ok"]["committed"], true);
}

fn ev(action: PointerAction, id: i32, tool: ToolType, x: f32, y: f32, t_ms: u64) -> RawPointerEvent {
    let mut e = RawPointerEvent::new(action, id, tool, x, y, t_ms * 1_000_000);
    e.pressure = Some(0.6);
    e
}

fn stylus_line(x0: f32, y0: f32, x1: f32, y1: f32, n: usize, t0_ms: u64) -> Vec<RawPointerEvent> {
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            let a = if i == 0 {
                PointerAction::Down
            } else if i == n - 1 {
                PointerAction::Up
            } else {
                PointerAction::Move
            };
            ev(a, 0, ToolType::Stylus, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, t0_ms + i as u64 * 4)
        })
        .collect()
}

/// Defect: the hover point used by the stylus-button eyedropper was only updated at the end of a
/// batch, so a press in the same batch as the hover sampled the previous batch's position (or
/// nothing); a hover exit never cleared it.
#[test]
fn eyedropper_uses_hover_from_the_same_batch() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 64, "height": 64, "background": "#FF0000"}));
    s.execute(Command::ViewSize { width: 64.0, height: 64.0 }).unwrap();
    ok(&mut s, json!({"cmd": "view.set", "zoom": 1.0, "rotation": 0.0, "panX": 0.0, "panY": 0.0}));
    ok(&mut s, json!({"cmd": "input.config", "stylusButton": "eyedropper"}));
    let mut hover = ev(PointerAction::HoverMove, 0, ToolType::Stylus, 10.0, 10.0, 1);
    hover.pressure = None;
    let mut press = hover;
    press.t_ns += 1_000_000;
    press.buttons = 32; // MotionEvent.BUTTON_STYLUS_PRIMARY
    let _ = s.input_batch(&[hover, press]);
    assert_eq!(ok(&mut s, json!({"cmd": "color.get"}))["ok"]["color"], "#FF0000");
}

/// Live-stroke invariants: no document mutation, history step, open/create or save can run under
/// a live stroke (so the stroke can never be committed onto a removed/locked layer); exports and
/// autosaves snapshot only committed content; `input.reset` commits the arbiter's stroke.
#[test]
fn live_stroke_invariants() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 96, "height": 96, "background": "#FFFFFF"}));
    s.execute(Command::ViewSize { width: 96.0, height: 96.0 }).unwrap();
    ok(&mut s, json!({"cmd": "view.set", "zoom": 1.0, "rotation": 0.0, "panX": 0.0, "panY": 0.0}));
    let layer = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    // Pen down and moving, not lifted.
    let mut evs = stylus_line(10.0, 10.0, 80.0, 80.0, 12, 0);
    evs.pop();
    let out = s.input_batch(&evs);
    assert!(out.flags & flags::DIRTY != 0 && s.has_live_stroke());
    let v = s.version();
    let doc = s.document().unwrap().clone();
    let dir = std::env::temp_dir().join(format!("veskra-review-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for cmd in [
        json!({"cmd": "layer.remove", "id": layer}),
        json!({"cmd": "layer.set", "id": layer, "locked": true}),
        json!({"cmd": "layer.set", "id": layer, "visible": false}),
        json!({"cmd": "history.undo"}),
        json!({"cmd": "selection.all"}),
        json!({"cmd": "document.create", "width": 8, "height": 8}),
        json!({"cmd": "document.save", "path": dir.join("x.veskra").to_string_lossy()}),
        json!({"cmd": "stroke.begin"}),
    ] {
        let e = err(&mut s, cmd);
        assert_eq!(e["code"], "InvalidState", "{e}");
    }
    assert_eq!(s.version(), v);
    assert_eq!(s.document().unwrap().as_ref(), doc.as_ref());
    // Export snapshots committed content only (blank white).
    let png = dir.join("live.png");
    ok(&mut s, json!({"cmd": "export.run", "format": "png", "path": png.to_string_lossy()}));
    let (_, _, px) = veskra_testkit::decode_png_rgba8(&std::fs::read(&png).unwrap()).unwrap();
    assert!(px.chunks_exact(4).all(|p| p == [255, 255, 255, 255]), "export must not include the live stroke");
    // Lifecycle pause commits the confirmed samples (one undo step) and reports the refresh.
    let r = ok(&mut s, json!({"cmd": "input.reset"}));
    assert_eq!(r["historyChanged"], true, "{r}");
    assert!(!r["invalidated"].as_array().unwrap().is_empty());
    assert!(!s.has_live_stroke());
    assert_eq!(info(&mut s)["strokeCount"], 1);
    assert_eq!(info(&mut s)["undoLabel"], "Stroke");
}

/// SAV-002 semantics: the saved snapshot's version is recorded; later edits keep the document
/// dirty; undo back to the saved content is still dirty (documented, conservative); a failed
/// save changes nothing; recovery never marks saved.
#[test]
fn saved_version_semantics() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": 32, "height": 32}));
    let dir = std::env::temp_dir().join(format!("veskra-review-sav-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.veskra");
    let r = ok(&mut s, json!({"cmd": "document.save", "path": p.to_string_lossy()}));
    assert_eq!(r["ok"]["savedVersion"], s.version());
    assert_eq!(info(&mut s)["dirty"], false);
    ok(&mut s, json!({"cmd": "selection.all"}));
    assert_eq!(info(&mut s)["dirty"], true);
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(info(&mut s)["dirty"], true, "conservative: versions are monotonic");
    let saved_before = info(&mut s)["savedVersion"].clone();
    err(&mut s, json!({"cmd": "document.save", "path": dir.join("missing/dir/b.veskra").to_string_lossy()}));
    assert_eq!(info(&mut s)["savedVersion"], saved_before);
    ok(&mut s, json!({"cmd": "document.autosave", "dir": dir.join("auto").to_string_lossy()}));
    ok(&mut s, json!({"cmd": "document.recover", "dir": dir.join("auto").to_string_lossy()}));
    assert_eq!(info(&mut s)["dirty"], true);
    assert_eq!(info(&mut s)["savedVersion"], Value::Null);
}
