#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Stroke-time tools: symmetry, guides, shapes, smudge and the stabilizer, on BOTH stroke paths
//! (headless `stroke.*` commands and the packed `Session::input_batch` hot path).
//!
//! Contracts pinned here: every gesture is exactly one undo step; undo/redo restore the document
//! exactly; symmetry replicas are real stroke records (vector form, `.veskra` round trip);
//! guides and symmetry never touch pressure/tilt; shape samples carry no hardware pressure;
//! `invalidated` / dirty rects cover every changed display pixel; hostile field values are typed
//! errors that change nothing.

use serde_json::{Value, json};
use veskra_core::{RectI, SampleFlags, ToolType};
use veskra_engine::veskra_doc::{StrokeMode, StrokeRecord};
use veskra_engine::veskra_input::{PointerAction, RawPointerEvent};
use veskra_engine::{Command, EngineConfig, GuideKind, Session, SymmetryMode, ToolKind, flags};

// ------------------------------------------------------------------ helpers

fn session() -> Session {
    Session::new(EngineConfig { deterministic_seed: Some(11), ..Default::default() })
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

/// New document (white background + transparent active "Layer 1") with a 1:1 view.
fn setup(w: u32, h: u32) -> Session {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": w, "height": h, "background": "#FFFFFF"}));
    ok(&mut s, json!({"cmd": "view.size", "width": w, "height": h}));
    ok(&mut s, json!({"cmd": "view.set", "zoom": 1.0, "rotation": 0.0, "panX": 0.0, "panY": 0.0}));
    s
}

fn size(s: &Session) -> (u32, u32) {
    let d = s.document().unwrap();
    (d.width(), d.height())
}

fn render(s: &Session) -> Vec<u8> {
    let (w, h) = size(s);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    s.render_display(RectI::from_size(w, h), &mut buf).unwrap();
    buf
}

fn px(buf: &[u8], w: u32, x: i32, y: i32) -> [u8; 4] {
    let i = (y as usize * w as usize + x as usize) * 4;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

const WHITE: [u8; 4] = [255, 255, 255, 255];

/// Premultiplied 16-bit pixel of the active layer.
fn layer_px(s: &Session, x: i32, y: i32) -> [u16; 4] {
    let d = s.document().unwrap();
    d.layer(d.active_layer.unwrap()).unwrap().pixels().unwrap().tiles.get_pixel(x, y)
}

fn strokes(s: &Session) -> Vec<StrokeRecord> {
    let d = s.document().unwrap();
    d.strokes_of(d.active_layer.unwrap()).into_iter().cloned().collect()
}

fn active_layer_json(s: &mut Session) -> Value {
    let layers = ok(s, json!({"cmd": "layer.list"}))["ok"]["layers"].clone();
    layers.as_array().unwrap().iter().find(|l| l["active"] == true).unwrap().clone()
}

/// Headless stroke through `pts` (x, y, pressure); returns the commit response.
fn stroke(s: &mut Session, begin: Value, pts: &[(f32, f32, f32)]) -> Value {
    ok(s, begin);
    let samples: Vec<Value> = pts
        .iter()
        .enumerate()
        .map(|(i, (x, y, p))| json!({"x": x, "y": y, "t": i as u64 * 4_000_000, "p": p}))
        .collect();
    ok(s, json!({"cmd": "stroke.samples", "samples": samples}));
    ok(s, json!({"cmd": "stroke.commit"}))
}

fn line_pts(x0: f32, y0: f32, x1: f32, y1: f32, n: usize, p: f32) -> Vec<(f32, f32, f32)> {
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, p)
        })
        .collect()
}

fn pen(action: PointerAction, x: f32, y: f32, t_ms: u64) -> RawPointerEvent {
    let mut e = RawPointerEvent::new(action, 0, ToolType::Stylus, x, y, t_ms * 1_000_000);
    e.pressure = Some(0.8);
    e
}

/// Down, moves, up along `pts` (view == document coordinates in these tests).
fn pen_path(pts: &[(f32, f32)], t0_ms: u64) -> Vec<RawPointerEvent> {
    pts.iter()
        .enumerate()
        .map(|(i, (x, y))| {
            let a = if i == 0 {
                PointerAction::Down
            } else if i == pts.len() - 1 {
                PointerAction::Up
            } else {
                PointerAction::Move
            };
            pen(a, *x, *y, t0_ms + i as u64 * 4)
        })
        .collect()
}

/// First changed pixel outside every rect, if any.
fn stray_change(a: &[u8], b: &[u8], w: u32, rects: &[RectI]) -> Option<(i32, i32)> {
    for (i, (p, q)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
        let (x, y) = ((i as u32 % w) as i32, (i as u32 / w) as i32);
        if p != q && !rects.iter().any(|r| r.contains(x, y)) {
            return Some((x, y));
        }
    }
    None
}

fn rects_of(out: &Value) -> Vec<RectI> {
    out["invalidated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let a: Vec<i32> = a.as_array().unwrap().iter().map(|v| v.as_i64().unwrap() as i32).collect();
            RectI::new(a[0], a[1], a[2], a[3])
        })
        .collect()
}

fn max_channel_diff(a: [u8; 4], b: [u8; 4]) -> u8 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(y)).max().unwrap()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-tools-it-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

// ------------------------------------------------------------------ symmetry

#[test]
fn symmetry_vertical_mirrors_pixels_and_is_one_undo_step() {
    let (w, h) = (200u32, 120u32);
    let mut s = setup(w, h);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 10.0}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#C02020"}));
    let set = ok(&mut s, json!({"cmd": "symmetry.set", "mode": "vertical"}));
    assert_eq!(set["ok"]["strokes"], 2);
    assert_eq!(set["historyChanged"], false);
    let blank = render(&s);
    let doc0 = s.document().unwrap().clone();
    let v0 = s.version();

    let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(20.0, 20.0, 70.0, 95.0, 30, 0.7));
    assert_eq!(c["ok"]["committed"], true);
    assert_eq!(c["ok"]["strokeIds"].as_array().unwrap().len(), 2);
    assert_eq!(c["ok"]["strokeId"], c["ok"]["strokeIds"][0]);
    assert_eq!(c["historyChanged"], true);
    assert_eq!(s.version(), v0 + 1, "N strokes, one version step");
    assert_eq!(info(&mut s)["strokeCount"], 2);

    // Real, mirrored stroke records with untouched pressure.
    let recs = strokes(&s);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].samples().len(), recs[1].samples().len());
    for (a, b) in recs[0].samples().iter().zip(recs[1].samples()) {
        assert_eq!(b.x, 200.0 - a.x);
        assert_eq!((b.y, b.pressure, b.t_ns, b.flags), (a.y, a.pressure, a.t_ns, a.flags));
    }
    assert!(recs.iter().all(|r| r.has_vector_form()), "replicas of a vector-eligible brush keep a vector form");

    // Pixels: the canvas is its own mirror image, and both halves were painted.
    let img = render(&s);
    let mut painted = [0usize; 2];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (a, b) = (px(&img, w, x, y), px(&img, w, w as i32 - 1 - x, y));
            assert!(max_channel_diff(a, b) <= 1, "({x},{y}) {a:?} vs mirror {b:?}");
            if a != WHITE {
                painted[usize::from(x >= w as i32 / 2)] += 1;
            }
        }
    }
    assert!(painted[0] > 300 && painted[1] > 300, "{painted:?}");

    // One undo removes both strokes; redo restores them exactly.
    let doc1 = s.document().unwrap().clone();
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(render(&s), blank);
    assert_eq!(s.document().unwrap().as_ref(), doc0.as_ref());
    assert_eq!(info(&mut s)["canUndo"], false);
    ok(&mut s, json!({"cmd": "history.redo"}));
    assert_eq!(render(&s), img);
    assert_eq!(s.document().unwrap().as_ref(), doc1.as_ref());
}

#[test]
fn symmetry_modes_produce_the_documented_stroke_counts() {
    for (set, n) in [
        (json!({"cmd": "symmetry.set", "mode": "off"}), 1),
        (json!({"cmd": "symmetry.set", "mode": "horizontal"}), 2),
        (json!({"cmd": "symmetry.set", "mode": "quad"}), 4),
        (json!({"cmd": "symmetry.set", "mode": "radial"}), 6),
        (json!({"cmd": "symmetry.set", "mode": "radial", "count": 5}), 5),
        (json!({"cmd": "symmetry.set", "mode": "radial", "count": 3, "mirror": true}), 6),
        (json!({"cmd": "symmetry.set", "mode": "radial", "count": 16, "mirror": true}), 32),
    ] {
        let mut s = setup(160, 160);
        ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
        let r = ok(&mut s, set.clone());
        assert_eq!(r["ok"]["strokes"], n, "{set}");
        let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(30.0, 40.0, 60.0, 70.0, 12, 0.6));
        assert_eq!(c["ok"]["strokeIds"].as_array().unwrap().len(), n, "{set}");
        assert_eq!(info(&mut s)["strokeCount"], n);
        assert_eq!(info(&mut s)["undoLabel"], "Stroke");
        ok(&mut s, json!({"cmd": "history.undo"}));
        assert_eq!(info(&mut s)["strokeCount"], 0, "{set}: one undo removes every replica");
    }
}

#[test]
fn symmetry_quad_image_has_both_mirror_axes_and_custom_center_is_used() {
    let (w, h) = (160u32, 120u32);
    let mut s = setup(w, h);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 8.0}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "quad"}));
    stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(15.0, 12.0, 60.0, 45.0, 20, 0.8));
    let img = render(&s);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let a = px(&img, w, x, y);
            assert!(max_channel_diff(a, px(&img, w, w as i32 - 1 - x, y)) <= 1, "x mirror at ({x},{y})");
            assert!(max_channel_diff(a, px(&img, w, x, h as i32 - 1 - y)) <= 1, "y mirror at ({x},{y})");
        }
    }
    // Explicit center: mirror about x = 40 instead of the canvas center (80).
    ok(&mut s, json!({"cmd": "history.undo"}));
    let st = ok(&mut s, json!({"cmd": "symmetry.set", "mode": "vertical", "cx": 40.0, "cy": 10.0}));
    assert_eq!((st["ok"]["cx"].as_f64(), st["ok"]["autoCenter"].as_bool()), (Some(40.0), Some(false)));
    stroke(&mut s, json!({"cmd": "stroke.begin"}), &[(10.0, 60.0, 0.9), (30.0, 60.0, 0.9)]);
    let recs = strokes(&s);
    assert_eq!((recs[1].samples()[0].x, recs[1].samples()[1].x), (70.0, 50.0));
}

#[test]
fn symmetry_on_the_input_path_is_live_predicted_and_one_undo_step() {
    let (w, h) = (200u32, 120u32);
    let mut s = setup(w, h);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 8.0}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "vertical"}));
    let blank = render(&s);
    let v0 = s.version();
    let mut prev = blank.clone();
    let mut step = |s: &mut Session, batch: &[RawPointerEvent], what: &str| -> (i32, Vec<RectI>) {
        let out = s.input_batch(batch);
        let next = render(s);
        if let Some((x, y)) = stray_change(&prev, &next, w, &out.dirty) {
            panic!("{what}: pixel ({x},{y}) changed outside dirty {:?}", out.dirty);
        }
        prev = next;
        (out.flags, out.dirty)
    };

    // Pen down + two confirmed moves on the left: the mirrored copy is visible immediately.
    let (f, dirty) = step(
        &mut s,
        &[
            pen(PointerAction::Down, 30.0, 30.0, 0),
            pen(PointerAction::Move, 40.0, 45.0, 4),
            pen(PointerAction::Move, 50.0, 60.0, 8),
        ],
        "confirmed batch",
    );
    assert!(f & flags::DIRTY != 0 && f & flags::COMMITTED == 0);
    assert!(s.has_live_stroke());
    assert_eq!(s.version(), v0, "nothing is committed while the pen is down");
    let live = render(&s);
    assert_ne!(px(&live, w, 40, 45), WHITE, "user's stroke is live");
    assert_ne!(px(&live, w, 159, 45), WHITE, "mirrored stroke is live");
    assert!(dirty.len() >= 2, "far-apart replicas are reported as separate rects, not their bounding box: {dirty:?}");
    assert!(dirty.iter().all(|r| r.width() < 100), "{dirty:?}");

    // A predicted tail is shown on both sides…
    let mut predicted = pen(PointerAction::Move, 80.0, 100.0, 16);
    predicted.predicted = true;
    step(&mut s, &[pen(PointerAction::Move, 55.0, 65.0, 12), predicted], "predicted batch");
    let with_tail = render(&s);
    assert_ne!(px(&with_tail, w, 78, 97), WHITE, "predicted tail");
    assert_ne!(px(&with_tail, w, 121, 97), WHITE, "mirrored predicted tail");
    // …and disappears again when the pen goes elsewhere.
    step(&mut s, &[pen(PointerAction::Move, 60.0, 62.0, 16)], "superseding batch");
    let without_tail = render(&s);
    assert_eq!(px(&without_tail, w, 78, 97), WHITE);
    assert_eq!(px(&without_tail, w, 121, 97), WHITE);

    // Pen up: one committed gesture, two records, and no predicted sample in either.
    let (f, _) = step(&mut s, &[pen(PointerAction::Up, 62.0, 60.0, 20)], "up");
    assert!(f & flags::COMMITTED != 0 && f & flags::HISTORY != 0);
    assert_eq!(s.version(), v0 + 1);
    let recs = strokes(&s);
    assert_eq!(recs.len(), 2);
    for r in &recs {
        assert!(r.samples().iter().all(|q| q.is_confirmed() && !q.flags.contains(SampleFlags::PREDICTED)));
        assert!(r.samples().iter().all(|q| q.flags.contains(SampleFlags::PRESSURE_REPORTED)));
    }
    assert_eq!(recs[0].samples().len(), recs[1].samples().len());
    assert!(recs[0].samples().iter().all(|q| q.x < 100.0) && recs[1].samples().iter().all(|q| q.x > 100.0));

    // Live preview == committed pixels, and a single undo clears both.
    let committed = render(&s);
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(render(&s), blank);
    ok(&mut s, json!({"cmd": "history.redo"}));
    assert_eq!(render(&s), committed);

    // Cancel (palm / lifecycle) removes the live replicas too.
    ok(&mut s, json!({"cmd": "history.undo"}));
    let out = s.input_batch(&[pen(PointerAction::Down, 30.0, 30.0, 100), pen(PointerAction::Move, 60.0, 70.0, 104)]);
    assert!(out.flags & flags::DIRTY != 0);
    assert_ne!(render(&s), blank);
    let out = s.input_batch(&[pen(PointerAction::Cancel, 60.0, 70.0, 108)]);
    assert_eq!(render(&s), blank);
    assert!(stray_change(&render(&s), &blank, w, &out.dirty).is_none());
    assert_eq!(info(&mut s)["strokeCount"], 0);
}

#[test]
fn symmetry_applies_to_the_eraser() {
    let (w, h) = (120u32, 80u32);
    let mut s = setup(w, h);
    ok(&mut s, json!({"cmd": "color.set", "hex": "#2040C0"}));
    ok(&mut s, json!({"cmd": "edit.fill_selection"}));
    assert_eq!(layer_px(&s, 20, 40)[3], 65535);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 12.0}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "vertical"}));
    let before = s.document().unwrap().clone();
    let c = stroke(&mut s, json!({"cmd": "stroke.begin", "eraser": true}), &line_pts(15.0, 40.0, 35.0, 40.0, 8, 1.0));
    assert_eq!(c["ok"]["strokeIds"].as_array().unwrap().len(), 2);
    assert_eq!(layer_px(&s, 25, 40)[3], 0, "erased on the pen side");
    assert_eq!(layer_px(&s, 94, 40)[3], 0, "erased on the mirrored side");
    assert_eq!(layer_px(&s, 60, 40)[3], 65535, "untouched in between");
    assert!(strokes(&s).iter().all(|r| r.composition.mode == StrokeMode::Erase));
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), before.as_ref());
}

#[test]
fn symmetry_strokes_stay_vector_and_round_trip_through_save_and_svg() {
    let mut s = setup(240, 240);
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector", "name": "Ink"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "radial", "count": 4}));
    stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(40.0, 50.0, 100.0, 90.0, 25, 0.7));
    let ink = active_layer_json(&mut s);
    assert_eq!((ink["kind"].as_str(), ink["strokes"].as_u64()), (Some("vector"), Some(4)));
    assert_eq!(ink["vectorFidelity"], "exact");
    let pf = ok(&mut s, json!({"cmd": "export.preflight", "format": "svg"}));
    assert_eq!(pf["ok"]["vectorPathCount"], 4, "{pf}");
    assert_eq!(pf["ok"]["rasterOnlyStrokeCount"], 0);

    let path = tmp("symmetry.veskra");
    ok(&mut s, json!({"cmd": "document.save", "path": path.to_string_lossy()}));
    let mut s2 = session();
    ok(&mut s2, json!({"cmd": "document.open", "path": path.to_string_lossy()}));
    assert_eq!(s.document().unwrap().as_ref(), s2.document().unwrap().as_ref(), "replicas round-trip as strokes");
    assert_eq!(render(&s), render(&s2));
    // Symmetry itself is session state: a fresh session opens the file with symmetry off, and the
    // session that saved keeps its setting.
    assert_eq!(info(&mut s2)["symmetry"]["mode"], "off");
    assert_eq!(info(&mut s)["symmetry"]["mode"], "radial");

    let svg = tmp("symmetry.svg");
    let rep = ok(&mut s2, json!({"cmd": "export.run", "format": "svg", "path": svg.to_string_lossy()}));
    assert_eq!(rep["ok"]["vectorObjectCount"], 4, "{rep}");
}

#[test]
fn symmetry_state_is_reported_and_hostile_values_are_typed_errors() {
    let mut s = session();
    // Works without a document; the automatic center is unknown until one is open.
    let g = ok(&mut s, json!({"cmd": "symmetry.get"}));
    assert_eq!(
        g["ok"],
        json!({"mode": "off", "count": 6, "mirror": false, "cx": null, "cy": null, "autoCenter": true, "strokes": 1})
    );
    ok(&mut s, json!({"cmd": "document.create", "width": 300, "height": 200}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "radial", "count": 8, "mirror": true}));
    let want = json!({"mode": "radial", "count": 8, "mirror": true, "cx": 150.0, "cy": 100.0, "autoCenter": true, "strokes": 16});
    assert_eq!(ok(&mut s, json!({"cmd": "symmetry.get"}))["ok"], want);
    assert_eq!(info(&mut s)["symmetry"], want);

    let v = s.version();
    for bad in [
        json!({"cmd": "symmetry.set"}),
        json!({"cmd": "symmetry.set", "mode": "diagonal"}),
        json!({"cmd": "symmetry.set", "mode": "radial", "count": 1}),
        json!({"cmd": "symmetry.set", "mode": "radial", "count": 17}),
        json!({"cmd": "symmetry.set", "mode": "radial", "count": 4_294_967_296u64}),
        json!({"cmd": "symmetry.set", "mode": "radial", "count": -3}),
        json!({"cmd": "symmetry.set", "mode": "radial", "count": 2.5}),
        json!({"cmd": "symmetry.set", "mode": "vertical", "cx": 10.0}),
        json!({"cmd": "symmetry.set", "mode": "vertical", "cx": 1e9, "cy": 0.0}),
        json!({"cmd": "symmetry.set", "mode": "vertical", "cnt": 3}),
    ] {
        let e = err(&mut s, bad.clone());
        assert_eq!(e["code"], "InvalidArgument", "{bad} -> {e}");
    }
    // Values JSON cannot carry (NaN/inf) through the typed API.
    for (cx, cy) in [(f64::NAN, 1.0), (1.0, f64::INFINITY)] {
        let e = s
            .execute(Command::SymmetrySet {
                mode: SymmetryMode::Quad,
                count: None,
                mirror: None,
                cx: Some(cx),
                cy: Some(cy),
            })
            .unwrap_err();
        assert_eq!(e.code, veskra_core::ErrorCode::InvalidArgument);
    }
    let raw = s.execute_json(r#"{"cmd":"symmetry.set","mode":"vertical","cx":1e999,"cy":0}"#);
    assert!(raw.contains("\"error\""), "{raw}");
    // Failed sets changed nothing.
    assert_eq!(ok(&mut s, json!({"cmd": "symmetry.get"}))["ok"], want);
    assert_eq!(s.version(), v);
}

// ------------------------------------------------------------------ guides

#[test]
fn line_guide_projects_samples_and_keeps_pressure() {
    let (w, h) = (200u32, 120u32);
    let mut s = setup(w, h);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 6.0, "stabilization": 1.0}));
    let g = ok(&mut s, json!({"cmd": "guide.set", "kind": "line", "x0": 20.0, "y0": 60.0, "x1": 180.0, "y1": 60.0}));
    assert_eq!(g["ok"], json!({"kind": "line", "x0": 20.0, "y0": 60.0, "x1": 180.0, "y1": 60.0}));
    assert_eq!(info(&mut s)["guide"], g["ok"]);
    // A zigzag between y = 30 and y = 90 with varying pressure and tilt.
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    let samples: Vec<Value> = (0..17)
        .map(|i| {
            json!({"x": 20.0 + i as f32 * 10.0, "y": if i % 2 == 0 { 30.0 } else { 90.0 }, "t": i * 4_000_000,
                   "p": 0.2 + 0.04 * i as f32, "tilt": 0.1 + 0.01 * i as f32})
        })
        .collect();
    ok(&mut s, json!({"cmd": "stroke.samples", "samples": samples}));
    ok(&mut s, json!({"cmd": "stroke.commit"}));
    let recs = strokes(&s);
    assert_eq!(recs.len(), 1);
    for (i, q) in recs[0].samples().iter().enumerate() {
        assert_eq!((q.x, q.y), (20.0 + i as f32 * 10.0, 60.0), "sample {i} lies on the guide");
        assert_eq!(q.pressure, 0.2 + 0.04 * i as f32, "pressure passes through unchanged");
        assert_eq!(q.tilt, Some(0.1 + 0.01 * i as f32), "tilt passes through unchanged");
        assert!(q.flags.contains(SampleFlags::PRESSURE_REPORTED));
    }
    // Paint stays within the brush radius of the line, over its whole length (no stabilizer lag:
    // ruled strokes run with stabilization 0, recorded in the stroke's own brush parameters).
    let img = render(&s);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if px(&img, w, x, y) != WHITE {
                assert!((y - 60).abs() <= 4, "paint at ({x},{y}) is off the guide");
            }
        }
    }
    assert_ne!(px(&img, w, 22, 60), WHITE);
    assert_ne!(px(&img, w, 178, 60), WHITE);
    let params: Value = serde_json::from_str(&recs[0].brush_params).unwrap();
    assert_eq!(params["stabilization"], 0.0);
    assert!(recs[0].has_vector_form());
}

#[test]
fn angle_guide_runs_through_the_first_sample_on_the_input_path() {
    let (w, h) = (200u32, 120u32);
    let mut s = setup(w, h);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 6.0}));
    let g = ok(&mut s, json!({"cmd": "guide.set", "kind": "line", "angle": 0.0}));
    assert_eq!(g["ok"], json!({"kind": "line", "angle": 0.0}));
    let blank = render(&s);
    // Down at (40, 50); the pen then wanders, with a predicted event far off the line.
    let mut batch = vec![pen(PointerAction::Down, 40.0, 50.0, 0), pen(PointerAction::Move, 70.0, 20.0, 4)];
    let mut p = pen(PointerAction::Move, 110.0, 110.0, 8);
    p.predicted = true;
    batch.push(p);
    let out = s.input_batch(&batch);
    let live = render(&s);
    assert!(stray_change(&blank, &live, w, &out.dirty).is_none());
    assert_ne!(px(&live, w, 105, 50), WHITE, "the predicted tail is projected onto the guide too");
    assert_eq!(px(&live, w, 108, 105), WHITE);
    let out2 = s.input_batch(&[pen(PointerAction::Move, 120.0, 95.0, 8), pen(PointerAction::Up, 160.0, 10.0, 12)]);
    assert!(out2.flags & flags::COMMITTED != 0);
    let img = render(&s);
    assert!(stray_change(&live, &img, w, &out2.dirty).is_none());
    let recs = strokes(&s);
    assert_eq!(recs.len(), 1);
    let xs: Vec<f32> = recs[0].samples().iter().map(|q| q.x).collect();
    assert_eq!(xs, vec![40.0, 70.0, 120.0, 160.0]);
    assert!(recs[0].samples().iter().all(|q| q.y == 50.0 && (q.pressure - 0.8).abs() < 1e-6));
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if px(&img, w, x, y) != WHITE {
                assert!((y - 50).abs() <= 4 && (36..=164).contains(&x), "paint at ({x},{y})");
            }
        }
    }
    // The next stroke anchors at its own first sample.
    s.input_batch(&pen_path(&[(30.0, 90.0), (90.0, 20.0), (150.0, 70.0)], 100));
    assert!(strokes(&s)[1].samples().iter().all(|q| q.y == 90.0));
}

#[test]
fn ellipse_guide_puts_samples_on_the_outline_and_applies_before_symmetry() {
    let mut s = setup(200, 120);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    ok(&mut s, json!({"cmd": "guide.set", "kind": "ellipse", "x0": 40.0, "y0": 20.0, "x1": 160.0, "y1": 100.0}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "horizontal"}));
    // A sloppy half circle drawn above the center, well off the outline.
    let pts: Vec<(f32, f32, f32)> = (0..=20)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 20.0);
            let r = if i % 2 == 0 { 0.6 } else { 1.5 };
            (100.0 + 60.0 * r * a.cos(), 60.0 + 40.0 * r * a.sin(), 0.5)
        })
        .collect();
    let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &pts);
    assert_eq!(c["ok"]["strokeIds"].as_array().unwrap().len(), 2);
    let recs = strokes(&s);
    for q in recs[0].samples() {
        let e = ((f64::from(q.x) - 100.0) / 60.0).powi(2) + ((f64::from(q.y) - 60.0) / 40.0).powi(2);
        assert!((e - 1.0).abs() < 1e-3, "({}, {}) is not on the ellipse: {e}", q.x, q.y);
        assert!(q.y <= 60.01);
    }
    // The replica mirrors the *projected* samples (guide first, then symmetry).
    for (a, b) in recs[0].samples().iter().zip(recs[1].samples()) {
        assert_eq!((b.x, b.y), (a.x, 120.0 - a.y));
    }
}

#[test]
fn guide_validation_and_off() {
    let mut s = setup(100, 100);
    ok(&mut s, json!({"cmd": "guide.set", "kind": "ellipse", "x0": 10.0, "y0": 10.0, "x1": 90.0, "y1": 50.0}));
    let want = json!({"kind": "ellipse", "x0": 10.0, "y0": 10.0, "x1": 90.0, "y1": 50.0});
    for bad in [
        json!({"cmd": "guide.set"}),
        json!({"cmd": "guide.set", "kind": "spiral"}),
        json!({"cmd": "guide.set", "kind": "line"}),
        json!({"cmd": "guide.set", "kind": "line", "x0": 1.0, "y0": 2.0}),
        json!({"cmd": "guide.set", "kind": "line", "x0": 5.0, "y0": 5.0, "x1": 5.0, "y1": 5.0}),
        json!({"cmd": "guide.set", "kind": "line", "x0": 5.0, "y0": 5.0, "x1": 6.0, "y1": 5.0, "angle": 1.0}),
        json!({"cmd": "guide.set", "kind": "line", "angle": 1e9}),
        json!({"cmd": "guide.set", "kind": "ellipse"}),
        json!({"cmd": "guide.set", "kind": "ellipse", "x0": 5.0, "y0": 5.0, "x1": 5.0, "y1": 50.0}),
        json!({"cmd": "guide.set", "kind": "ellipse", "x0": 5.0, "y0": 5.0, "x1": 9e12, "y1": 50.0}),
        json!({"cmd": "guide.set", "kind": "ellipse", "x0": 5.0, "y0": 5.0, "x1": 9.0, "y1": 50.0, "angle": 0.1}),
        json!({"cmd": "guide.set", "kind": "line", "angle": 0.5, "anchor": 3}),
    ] {
        let e = err(&mut s, bad.clone());
        assert_eq!(e["code"], "InvalidArgument", "{bad} -> {e}");
    }
    let e = s
        .execute(Command::GuideSet {
            kind: GuideKind::Line,
            x0: None,
            y0: None,
            x1: None,
            y1: None,
            angle: Some(f64::NAN),
        })
        .unwrap_err();
    assert_eq!(e.code, veskra_core::ErrorCode::InvalidArgument);
    assert_eq!(ok(&mut s, json!({"cmd": "guide.get"}))["ok"], want, "failed sets keep the previous guide");
    // Off: samples are free again.
    assert_eq!(ok(&mut s, json!({"cmd": "guide.set", "kind": "off"}))["ok"], json!({"kind": "off"}));
    stroke(&mut s, json!({"cmd": "stroke.begin"}), &[(10.0, 80.0, 0.5), (30.0, 95.0, 0.5)]);
    assert_eq!(strokes(&s)[0].samples()[1].y, 95.0);
}

// ------------------------------------------------------------------ shapes

fn shape_session() -> Session {
    let mut s = setup(200, 160);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 6.0}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#0000FF"}));
    s
}

const BLUE16: [u16; 4] = [0, 0, 65535, 65535];
const CLEAR16: [u16; 4] = [0; 4];

#[test]
fn shape_line_is_one_stroke_without_hardware_pressure() {
    let mut s = shape_session();
    let doc0 = s.document().unwrap().clone();
    let blank = render(&s);
    let r = ok(&mut s, json!({"cmd": "shape.line", "x0": 20.0, "y0": 20.0, "x1": 180.0, "y1": 20.0}));
    assert_eq!(r["ok"]["committed"], true);
    assert_eq!((r["ok"]["filled"].as_bool(), r["ok"]["vector"].as_bool()), (Some(false), Some(true)));
    assert_eq!(r["ok"]["strokeIds"].as_array().unwrap().len(), 1);
    assert_eq!(r["historyChanged"], true);
    assert!(stray_change(&blank, &render(&s), 200, &rects_of(&r)).is_none());
    assert_eq!(layer_px(&s, 100, 20), BLUE16, "on the line: current color, full opacity");
    assert_eq!(layer_px(&s, 100, 40), CLEAR16);
    assert_eq!(layer_px(&s, 190, 20), CLEAR16, "the line ends at its end point");

    let recs = strokes(&s);
    assert_eq!(recs.len(), 1);
    let st = &recs[0];
    assert_eq!((st.brush_id.as_str(), st.color.to_hex().as_str()), ("round.hard", "#0000FF"));
    assert_eq!(st.samples().len(), 2);
    for q in st.samples() {
        assert!(q.is_confirmed());
        assert!(!q.flags.contains(SampleFlags::PRESSURE_REPORTED), "synthetic samples claim no hardware pressure");
        assert_eq!((q.tilt, q.azimuth, q.rotation), (None, None, None));
        assert_eq!(q.pressure, veskra_engine::FALLBACK_PRESSURE);
    }
    assert!(st.has_vector_form());
    assert!(st.geometry.as_ref().unwrap().fit_error <= 0.5);
    assert_eq!(info(&mut s)["undoLabel"], "Shape");

    let doc1 = s.document().unwrap().clone();
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), doc0.as_ref());
    assert_eq!(render(&s), blank);
    ok(&mut s, json!({"cmd": "history.redo"}));
    assert_eq!(s.document().unwrap().as_ref(), doc1.as_ref());
    assert_eq!(info(&mut s)["canUndo"], true);
}

#[test]
fn shape_rect_outline_and_fill() {
    let mut s = shape_session();
    // Stabilization and guides must not bend a shape.
    ok(&mut s, json!({"cmd": "brush.set", "stabilization": 1.0}));
    ok(&mut s, json!({"cmd": "guide.set", "kind": "line", "x0": 0.0, "y0": 0.0, "x1": 200.0, "y1": 0.0}));
    let doc0 = s.document().unwrap().clone();
    let r = ok(&mut s, json!({"cmd": "shape.rect", "x0": 120.0, "y0": 100.0, "x1": 40.0, "y1": 40.0}));
    assert_eq!((r["ok"]["filled"].as_bool(), r["ok"]["vector"].as_bool()), (Some(false), Some(true)));
    assert!(r["warnings"].as_array().unwrap().is_empty());
    for (x, y) in [(80, 40), (120, 70), (80, 100), (40, 70), (40, 40), (120, 100), (120, 40), (40, 100)] {
        assert_eq!(layer_px(&s, x, y), BLUE16, "outline at ({x},{y})");
    }
    assert_eq!(layer_px(&s, 80, 70), CLEAR16, "interior stays empty without fill");
    assert_eq!(layer_px(&s, 130, 70), CLEAR16);
    assert_eq!(active_layer_json(&mut s)["vectorFidelity"], "exact");
    let st = &strokes(&s)[0];
    assert_eq!(st.samples().len(), 5);
    assert!(st.geometry.as_ref().unwrap().fit_error <= 0.5, "corners survive the centerline fit");
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), doc0.as_ref());

    // With fill: interior gets the foreground color, the layer becomes raster, still one step.
    ok(&mut s, json!({"cmd": "color.set", "hex": "#0000FF"}));
    let blank = render(&s);
    let r = ok(&mut s, json!({"cmd": "shape.rect", "x0": 40.0, "y0": 40.0, "x1": 120.0, "y1": 100.0, "fill": true}));
    assert_eq!((r["ok"]["filled"].as_bool(), r["ok"]["vector"].as_bool()), (Some(true), Some(false)));
    assert!(r["warnings"][0].as_str().unwrap().contains("raster"), "{r}");
    assert!(stray_change(&blank, &render(&s), 200, &rects_of(&r)).is_none());
    assert_eq!(layer_px(&s, 80, 70), BLUE16);
    assert_eq!(layer_px(&s, 41, 99), BLUE16);
    assert_eq!(layer_px(&s, 130, 70), CLEAR16);
    assert_eq!(active_layer_json(&mut s)["vectorFidelity"], "raster");
    assert_eq!(info(&mut s)["strokeCount"], 1);
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), doc0.as_ref(), "fill + outline are a single undo step");
    assert_eq!(info(&mut s)["canUndo"], false);
}

#[test]
fn shape_ellipse_and_paths() {
    let mut s = shape_session();
    let r = ok(&mut s, json!({"cmd": "shape.ellipse", "x0": 40.0, "y0": 30.0, "x1": 160.0, "y1": 130.0, "fill": true}));
    assert_eq!(r["ok"]["filled"], true);
    for (x, y) in [(40, 80), (159, 80), (100, 30), (100, 129)] {
        assert_eq!(layer_px(&s, x, y), BLUE16, "outline at ({x},{y})");
    }
    assert_eq!(layer_px(&s, 100, 80), BLUE16, "filled center");
    assert_eq!(layer_px(&s, 45, 35), CLEAR16, "bounding-box corner is outside the ellipse");
    for q in strokes(&s)[0].samples() {
        let e = ((f64::from(q.x) - 100.0) / 60.0).powi(2) + ((f64::from(q.y) - 80.0) / 50.0).powi(2);
        assert!((e - 1.0).abs() < 1e-4);
    }
    ok(&mut s, json!({"cmd": "history.undo"}));

    // Polyline: passes through its vertices, stays open.
    let tri = json!([[30.0, 130.0], [100.0, 20.0], [170.0, 130.0]]);
    let r = ok(&mut s, json!({"cmd": "shape.path", "points": tri}));
    assert_eq!((r["ok"]["samples"].as_u64(), r["ok"]["vector"].as_bool()), (Some(3), Some(true)));
    assert_eq!(layer_px(&s, 100, 20), BLUE16);
    assert_eq!(layer_px(&s, 65, 75), BLUE16, "on the first segment");
    assert_eq!(layer_px(&s, 100, 130), CLEAR16, "open path: no closing segment");
    ok(&mut s, json!({"cmd": "history.undo"}));

    // Closed + fill.
    let r = ok(&mut s, json!({"cmd": "shape.path", "points": tri, "closed": true, "fill": true}));
    assert_eq!(r["ok"]["samples"], 4);
    assert_eq!(layer_px(&s, 100, 130), BLUE16, "closing segment");
    assert_eq!(layer_px(&s, 100, 90), BLUE16, "interior");
    assert_eq!(layer_px(&s, 40, 40), CLEAR16);
    ok(&mut s, json!({"cmd": "history.undo"}));

    // Smooth: a curve through the points, far more samples than vertices, still vector.
    let pts = json!([[20.0, 80.0], [70.0, 30.0], [120.0, 120.0], [180.0, 60.0]]);
    let r = ok(&mut s, json!({"cmd": "shape.path", "points": pts, "smooth": true}));
    assert!(r["ok"]["samples"].as_u64().unwrap() > 60, "{r}");
    assert_eq!(r["ok"]["vector"], true);
    for (x, y) in [(20, 80), (70, 30), (120, 120), (179, 60)] {
        assert_eq!(layer_px(&s, x, y), BLUE16, "the curve passes through ({x},{y})");
    }
    let st = &strokes(&s)[0];
    // A polyline never leaves the vertices' bounding box; the fitted curve rounds over the top
    // vertex (70, 30) and so rises slightly above it.
    assert!(st.samples().iter().any(|q| q.y < 29.8), "the path is a curve, not a polyline");
    assert!(st.samples().windows(2).all(|w| w[0].t_ns < w[1].t_ns));
    assert!(st.has_vector_form());
}

#[test]
fn shapes_on_a_vector_layer_export_as_paths_and_refuse_fill() {
    let mut s = shape_session();
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    for cmd in [
        json!({"cmd": "shape.line", "x0": 10.0, "y0": 10.0, "x1": 190.0, "y1": 30.0}),
        json!({"cmd": "shape.rect", "x0": 30.0, "y0": 50.0, "x1": 90.0, "y1": 110.0}),
        json!({"cmd": "shape.ellipse", "x0": 100.0, "y0": 50.0, "x1": 180.0, "y1": 140.0}),
        json!({"cmd": "shape.path", "points": [[20.0, 150.0], [60.0, 120.0], [90.0, 150.0]], "smooth": true}),
    ] {
        let r = ok(&mut s, cmd.clone());
        assert_eq!(r["ok"]["vector"], true, "{cmd}");
    }
    let l = active_layer_json(&mut s);
    assert_eq!((l["strokes"].as_u64(), l["vectorFidelity"].as_str()), (Some(4), Some("exact")));
    let pf = ok(&mut s, json!({"cmd": "export.preflight", "format": "pdf"}));
    assert_eq!(pf["ok"]["vectorPathCount"], 4);

    // Save → open keeps the shapes as strokes.
    let path = tmp("shapes.veskra");
    ok(&mut s, json!({"cmd": "document.save", "path": path.to_string_lossy()}));
    let mut s2 = session();
    ok(&mut s2, json!({"cmd": "document.open", "path": path.to_string_lossy()}));
    assert_eq!(s.document().unwrap().as_ref(), s2.document().unwrap().as_ref());
    assert_eq!(render(&s), render(&s2));

    // A fill is raster content: refused on a vector layer, and nothing changes.
    let (doc, v) = (s.document().unwrap().clone(), s.version());
    let e = err(&mut s, json!({"cmd": "shape.rect", "x0": 1.0, "y0": 1.0, "x1": 9.0, "y1": 9.0, "fill": true}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
    // A raster-only brush cannot draw shapes there either.
    ok(&mut s, json!({"cmd": "brush.select", "preset": "charcoal"}));
    let e = err(&mut s, json!({"cmd": "shape.line", "x0": 1.0, "y0": 1.0, "x1": 9.0, "y1": 9.0}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
    assert_eq!((s.document().unwrap().as_ref(), s.version()), (doc.as_ref(), v));
    assert!(!s.has_live_stroke());
}

#[test]
fn shapes_honor_selection_alpha_lock_layer_lock_and_symmetry() {
    let mut s = shape_session();
    // Selection: left half only.
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 0.0, "y0": 0.0, "x1": 100.0, "y1": 160.0}));
    let r = ok(&mut s, json!({"cmd": "shape.rect", "x0": 60.0, "y0": 40.0, "x1": 140.0, "y1": 120.0, "fill": true}));
    assert_eq!(layer_px(&s, 80, 80), BLUE16, "fill inside the selection");
    assert_eq!(layer_px(&s, 60, 80), BLUE16, "outline inside the selection");
    assert_eq!(layer_px(&s, 120, 80), CLEAR16, "fill outside the selection");
    assert_eq!(layer_px(&s, 140, 80), CLEAR16, "outline outside the selection");
    assert!(strokes(&s)[0].composition.selection_clipped);
    assert_eq!(r["ok"]["vector"], false);
    ok(&mut s, json!({"cmd": "selection.clear"}));

    // Alpha lock: only existing pixels are recolored, by fill and outline alike.
    let active = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "alphaLocked": true}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#FF0000"}));
    ok(&mut s, json!({"cmd": "shape.ellipse", "x0": 70.0, "y0": 50.0, "x1": 150.0, "y1": 110.0, "fill": true}));
    assert_eq!(layer_px(&s, 90, 80), [65535, 0, 0, 65535], "existing pixel recolored");
    assert_eq!(layer_px(&s, 120, 80), CLEAR16, "transparent pixel stays transparent under alpha lock");
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "alphaLocked": false}));

    // Symmetry: outline and fill are mirrored; still one undo step.
    ok(&mut s, json!({"cmd": "layer.clear", "id": active}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "vertical"}));
    let before = s.document().unwrap().clone();
    let r = ok(&mut s, json!({"cmd": "shape.rect", "x0": 20.0, "y0": 40.0, "x1": 60.0, "y1": 80.0, "fill": true}));
    assert_eq!(r["ok"]["strokeIds"].as_array().unwrap().len(), 2);
    assert_eq!(layer_px(&s, 40, 60), [65535, 0, 0, 65535]);
    assert_eq!(layer_px(&s, 159, 60), [65535, 0, 0, 65535], "mirrored fill");
    assert_eq!(layer_px(&s, 179, 60), [65535, 0, 0, 65535], "mirrored outline");
    assert_eq!(layer_px(&s, 100, 60), CLEAR16);
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), before.as_ref());
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "off"}));

    // Locked layer: typed error, nothing drawn.
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "locked": true}));
    let v = s.version();
    for cmd in [
        json!({"cmd": "shape.line", "x0": 1.0, "y0": 1.0, "x1": 50.0, "y1": 50.0}),
        json!({"cmd": "shape.rect", "x0": 1.0, "y0": 1.0, "x1": 50.0, "y1": 50.0, "fill": true}),
    ] {
        assert_eq!(err(&mut s, cmd)["code"], "LayerLocked");
    }
    assert_eq!(s.version(), v);
}

#[test]
fn shape_parsers_reject_hostile_values_without_side_effects() {
    let mut s = shape_session();
    let (doc, v) = (s.document().unwrap().clone(), s.version());
    let many: Vec<[f64; 2]> = (0..10_001).map(|i| [f64::from(i % 100), f64::from(i / 100)]).collect();
    let far: Vec<[f64; 2]> = (0..2000).map(|i| [if i % 2 == 0 { -90_000.0 } else { 90_000.0 }, f64::from(i)]).collect();
    for (bad, code) in [
        (json!({"cmd": "shape.line", "x0": 1.0, "y0": 1.0, "x1": 2.0}), "InvalidArgument"),
        (json!({"cmd": "shape.line", "x0": 1.0, "y0": 1.0, "x1": 2.0, "y1": 1e12}), "InvalidArgument"),
        (json!({"cmd": "shape.line", "x0": "a", "y0": 1.0, "x1": 2.0, "y1": 3.0}), "InvalidArgument"),
        (json!({"cmd": "shape.rect", "x0": 1.0, "y0": 1.0, "x1": 2.0, "y1": 3.0, "filled": true}), "InvalidArgument"),
        (json!({"cmd": "shape.ellipse", "x0": -1e300, "y0": 1.0, "x1": 2.0, "y1": 3.0}), "InvalidArgument"),
        (json!({"cmd": "shape.path", "points": []}), "InvalidArgument"),
        (json!({"cmd": "shape.path", "points": [[1.0]]}), "InvalidArgument"),
        (json!({"cmd": "shape.path", "points": [[1.0, 2.0, 3.0]]}), "InvalidArgument"),
        (json!({"cmd": "shape.path", "points": [[1.0, 2.0], [3.0, 4.0]], "closed": true}), "InvalidArgument"),
        (
            json!({"cmd": "shape.path", "points": [[1.0, 2.0], [30.0, 40.0], [5.0, 60.0]], "fill": true}),
            "InvalidArgument",
        ),
        (json!({"cmd": "shape.line", "x0": 1.0, "y0": 1.0, "x1": 9.0, "y1": 9.0, "fill": true}), "InvalidArgument"),
        (json!({"cmd": "shape.path", "points": many}), "AllocationLimit"),
        (json!({"cmd": "shape.path", "points": far, "smooth": true}), "AllocationLimit"),
    ] {
        let e = err(&mut s, bad.clone());
        assert_eq!(e["code"], code, "{} -> {e}", bad["cmd"]);
    }
    for raw in [
        r#"{"cmd":"shape.line","x0":1e999,"y0":0,"x1":1,"y1":1}"#,
        r#"{"cmd":"shape.path","points":[[0,0],[NaN,1]]}"#,
        r#"{"cmd":"shape.path","points":"all"}"#,
    ] {
        assert!(s.execute_json(raw).contains("\"error\""), "{raw}");
    }
    for bad in [
        Command::ShapeLine { x0: f64::NAN, y0: 0.0, x1: 1.0, y1: 1.0 },
        Command::ShapeRect { x0: 0.0, y0: f64::INFINITY, x1: 1.0, y1: 1.0, fill: true },
        Command::ShapeEllipse { x0: 0.0, y0: 0.0, x1: f64::NEG_INFINITY, y1: 1.0, fill: false },
        Command::ShapePath { points: vec![[0.0, 0.0], [f64::NAN, 2.0]], closed: false, smooth: true, fill: false },
    ] {
        assert_eq!(s.execute(bad).unwrap_err().code, veskra_core::ErrorCode::InvalidArgument);
    }
    // An eraser brush cannot fill; a live stroke blocks shapes.
    ok(&mut s, json!({"cmd": "brush.select", "preset": "eraser.hard"}));
    let e = err(&mut s, json!({"cmd": "shape.rect", "x0": 1.0, "y0": 1.0, "x1": 9.0, "y1": 9.0, "fill": true}));
    assert_eq!(e["code"], "InvalidArgument");
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    let e = err(&mut s, json!({"cmd": "shape.line", "x0": 1.0, "y0": 1.0, "x1": 9.0, "y1": 9.0}));
    assert_eq!(e["code"], "InvalidState");
    assert!(s.has_live_stroke(), "the foreign live stroke is untouched");
    ok(&mut s, json!({"cmd": "stroke.cancel"}));
    assert_eq!((s.document().unwrap().as_ref(), s.version()), (doc.as_ref(), v));

    // Degenerate but valid shapes are dots / flat outlines, not errors.
    let r = ok(&mut s, json!({"cmd": "shape.line", "x0": 50.0, "y0": 50.0, "x1": 50.0, "y1": 50.0}));
    assert_eq!(r["ok"]["samples"], 1);
    assert_eq!(layer_px(&s, 50, 50)[3], 65535);
    ok(&mut s, json!({"cmd": "shape.path", "points": [[70.0, 70.0]]}));
    ok(&mut s, json!({"cmd": "shape.rect", "x0": 10.0, "y0": 90.0, "x1": 60.0, "y1": 90.0, "fill": true}));
}

#[test]
fn failed_tool_gestures_do_not_consume_ids() {
    // Mirror about x = 100 on a 120 px vector layer: a stroke that starts left of the canvas has a
    // replica that is partly visible but runs beyond the renderer's vector reach, so it has no
    // vector form and the vector layer refuses the gesture at commit. Nothing may change —
    // including the deterministic ID stream: replaying the command log without its failing
    // lines must give the same IDs.
    let run = |with_failing_lines: bool| -> (String, u64) {
        let mut s = setup(120, 120);
        ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
        ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
        ok(&mut s, json!({"cmd": "symmetry.set", "mode": "vertical", "cx": 100.0, "cy": 0.0}));
        let (doc, v) = (s.document().unwrap().clone(), s.version());
        if with_failing_lines {
            // Fails at commit, after the stroke's seed and its replicas were set up.
            let e = err(&mut s, json!({"cmd": "shape.line", "x0": -100.0, "y0": 60.0, "x1": 100.0, "y1": 60.0}));
            assert_eq!(e["code"], "InvalidArgument", "{e}");
            assert!(!s.has_live_stroke());
            err(&mut s, json!({"cmd": "shape.path", "points": []}));
            err(&mut s, json!({"cmd": "shape.rect", "x0": 1.0, "y0": 1.0, "x1": 9.0, "y1": 9.0, "fill": true}));
        }
        ok(&mut s, json!({"cmd": "stroke.begin"}));
        ok(
            &mut s,
            json!({"cmd": "stroke.samples", "samples": [{"x": -100.0, "y": 60.0}, {"x": 100.0, "y": 60.0, "t": 5}]}),
        );
        if with_failing_lines {
            err(&mut s, json!({"cmd": "stroke.commit"}));
            assert!(!s.has_live_stroke(), "a failed commit still ends the stroke");
        }
        ok(&mut s, json!({"cmd": "stroke.cancel"}));
        assert_eq!((s.document().unwrap().as_ref(), s.version()), (doc.as_ref(), v));
        ok(&mut s, json!({"cmd": "symmetry.set", "mode": "off"}));
        let r = ok(&mut s, json!({"cmd": "shape.line", "x0": 10.0, "y0": 10.0, "x1": 90.0, "y1": 90.0}));
        (r["ok"]["strokeId"].as_str().unwrap().to_string(), strokes(&s)[0].deterministic_seed)
    };
    assert_eq!(run(true), run(false));
}

#[test]
fn invisible_out_of_reach_replicas_are_dropped_and_counted() {
    // Mirror about x = 5000: the replica is thousands of pixels off the canvas. It changes no
    // pixel and has no vector form, so it is not recorded (it would only spoil vector fidelity).
    for kind in ["vector", "raster"] {
        let mut s = setup(120, 120);
        ok(&mut s, json!({"cmd": "layer.add", "kind": kind}));
        ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
        ok(&mut s, json!({"cmd": "symmetry.set", "mode": "vertical", "cx": 5000.0, "cy": 0.0}));
        let r = ok(&mut s, json!({"cmd": "shape.line", "x0": 10.0, "y0": 10.0, "x1": 90.0, "y1": 90.0}));
        assert_eq!(r["ok"]["strokeIds"].as_array().unwrap().len(), 1, "{kind}: {r}");
        assert_eq!((r["ok"]["skippedReplicas"].as_u64(), r["ok"]["vector"].as_bool()), (Some(1), Some(true)));
        let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(10.0, 90.0, 90.0, 20.0, 9, 0.6));
        assert_eq!(c["ok"]["skippedReplicas"], 1);
        let l = active_layer_json(&mut s);
        assert_eq!((l["strokes"].as_u64(), l["vectorFidelity"].as_str()), (Some(2), Some("exact")), "{kind}");
        // A raster-only brush has no vector form to protect: its replica is recorded as usual.
        if kind == "raster" {
            ok(&mut s, json!({"cmd": "brush.select", "preset": "charcoal"}));
            let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(10.0, 50.0, 90.0, 50.0, 9, 0.6));
            assert_eq!(c["ok"]["strokeIds"].as_array().unwrap().len(), 2);
            assert_eq!(c["ok"]["skippedReplicas"], 0);
        }
    }
}

// ------------------------------------------------------------------ smudge

/// 160×100 document whose active layer is red on the left half and blue on the right.
fn two_tone() -> Session {
    let mut s = setup(160, 100);
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 0.0, "y0": 0.0, "x1": 80.0, "y1": 100.0}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#FF0000"}));
    ok(&mut s, json!({"cmd": "edit.fill_selection"}));
    ok(&mut s, json!({"cmd": "selection.invert"}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#0000FF"}));
    ok(&mut s, json!({"cmd": "edit.fill_selection"}));
    ok(&mut s, json!({"cmd": "selection.clear"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 20.0, "stabilization": 0.0}));
    ok(&mut s, json!({"cmd": "tool.set", "strength": 0.9}));
    s
}

const RED16: [u16; 4] = [65535, 0, 0, 65535];

#[test]
fn smudge_headless_drags_pixels_and_undo_is_exact() {
    let mut s = two_tone();
    let before = s.document().unwrap().clone();
    let before_img = render(&s);
    let v0 = s.version();
    assert_eq!(layer_px(&s, 95, 50), BLUE16);

    ok(&mut s, json!({"cmd": "stroke.begin", "tool": "smudge"}));
    let samples: Vec<Value> =
        (0..=25).map(|i| json!({"x": 60.0 + i as f32 * 2.0, "y": 50.5, "t": i * 4_000_000, "p": 1.0})).collect();
    let r = ok(&mut s, json!({"cmd": "stroke.samples", "samples": samples}));
    let live = render(&s);
    assert_ne!(live, before_img, "the smudge is displayed while the stroke is live");
    assert!(stray_change(&before_img, &live, 160, &rects_of(&r)).is_none());
    assert_eq!(s.version(), v0);
    let c = ok(&mut s, json!({"cmd": "stroke.commit"}));
    assert_eq!((c["ok"]["committed"].as_bool(), c["ok"]["tool"].as_str()), (Some(true), Some("smudge")));
    assert_eq!(c["historyChanged"], true);
    assert_eq!(s.version(), v0 + 1);
    let after_img = render(&s);
    assert!(stray_change(&live, &after_img, 160, &rects_of(&c)).is_none());

    // Red was dragged into the blue half; opaque stays opaque; far pixels are untouched.
    let p = layer_px(&s, 95, 50);
    assert!(p[0] > 15_000 && p[2] < 55_000 && p[3] == 65535, "{p:?}");
    assert_eq!(layer_px(&s, 95, 10), BLUE16);
    assert_eq!(layer_px(&s, 20, 50), RED16);
    // Raster edit, not a stroke record.
    assert_eq!(info(&mut s)["strokeCount"], 0);
    assert_eq!(active_layer_json(&mut s)["vectorFidelity"], "raster");
    assert_eq!(info(&mut s)["undoLabel"], "Smudge");

    let after = s.document().unwrap().clone();
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), before.as_ref(), "undo restores the pixels exactly");
    assert_eq!(render(&s), before_img);
    ok(&mut s, json!({"cmd": "history.redo"}));
    assert_eq!(s.document().unwrap().as_ref(), after.as_ref());
    assert_eq!(render(&s), after_img);

    // Smudged pixels survive save → open bit-exactly.
    let path = tmp("smudge.veskra");
    ok(&mut s, json!({"cmd": "document.save", "path": path.to_string_lossy()}));
    let mut s2 = session();
    ok(&mut s2, json!({"cmd": "document.open", "path": path.to_string_lossy()}));
    assert_eq!(render(&s2), after_img);
    assert_eq!(info(&mut s2)["tool"]["tool"], "brush", "the tool is session state, not document state");
}

#[test]
fn smudge_on_the_input_path_uses_the_session_tool_and_brush_size() {
    let mut s = two_tone();
    let t = ok(&mut s, json!({"cmd": "tool.set", "tool": "smudge"}));
    assert_eq!(t["ok"]["tool"], "smudge");
    assert!((t["ok"]["strength"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    assert_eq!(info(&mut s)["tool"]["tool"], "smudge");
    let before = s.document().unwrap().clone();
    let mut prev = render(&s);
    let v0 = s.version();
    let pts: Vec<(f32, f32)> = (0..=20).map(|i| (60.0 + i as f32 * 2.5, 50.5)).collect();
    let events = pen_path(&pts, 0);
    let mut flags_seen = 0;
    for chunk in events.chunks(4) {
        // Predicted events are accepted but never smudged (no prediction preview for smudge).
        let mut batch = chunk.to_vec();
        if chunk.last().unwrap().action == PointerAction::Move {
            let mut p = *chunk.last().unwrap();
            p.x += 30.0;
            p.t_ns += 1_000_000;
            p.predicted = true;
            batch.push(p);
        }
        let out = s.input_batch(&batch);
        let next = render(&s);
        if let Some((x, y)) = stray_change(&prev, &next, 160, &out.dirty) {
            panic!("pixel ({x},{y}) changed outside {:?}", out.dirty);
        }
        prev = next;
        flags_seen |= out.flags;
    }
    assert!(flags_seen & flags::COMMITTED != 0 && flags_seen & flags::HISTORY != 0);
    assert_eq!(s.version(), v0 + 1, "one gesture, one undo step");
    let small = layer_px(&s, 95, 50);
    assert!(small[0] > 10_000, "{small:?}");
    assert_eq!(layer_px(&s, 95, 30), BLUE16, "20 px smudge does not reach y = 30");
    assert_eq!(info(&mut s)["strokeCount"], 0);
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), before.as_ref());

    // Size comes from brush.set: a 60 px smudge reaches y = 30.
    ok(&mut s, json!({"cmd": "brush.set", "size": 60.0}));
    s.input_batch(&pen_path(&pts, 1000));
    assert!(layer_px(&s, 95, 30)[0] > 2_000, "{:?}", layer_px(&s, 95, 30));
    ok(&mut s, json!({"cmd": "history.undo"}));

    // The eraser flag and a per-stroke tool override still win over the session tool.
    stroke(&mut s, json!({"cmd": "stroke.begin", "eraser": true}), &line_pts(20.0, 20.0, 60.0, 20.0, 6, 1.0));
    assert_eq!(layer_px(&s, 40, 20)[3], 0, "eraser erased");
    assert_eq!(strokes(&s).last().unwrap().composition.mode, StrokeMode::Erase);
    ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#00FF00"}));
    stroke(&mut s, json!({"cmd": "stroke.begin", "tool": "brush"}), &line_pts(20.0, 80.0, 60.0, 80.0, 6, 1.0));
    assert_eq!(layer_px(&s, 40, 80), [0, 65535, 0, 65535], "brush painted");
    assert_eq!(info(&mut s)["strokeCount"], 2);
    // The stylus eraser end erases even while the smudge tool is selected.
    let mut ev: Vec<RawPointerEvent> = pen_path(&[(100.0, 80.0), (120.0, 80.0), (140.0, 80.0)], 5000);
    for e in &mut ev {
        e.tool = ToolType::StylusEraser;
    }
    s.input_batch(&ev);
    assert_eq!(layer_px(&s, 120, 80)[3], 0);
}

#[test]
fn smudge_respects_alpha_lock_selection_locks_and_layer_kind() {
    let mut s = two_tone();
    ok(&mut s, json!({"cmd": "tool.set", "tool": "smudge"}));
    let active = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    let drag = line_pts(60.0, 50.5, 110.0, 50.5, 26, 1.0);
    let alphas = |s: &Session| -> Vec<u16> {
        (0..100).flat_map(|y| (0..160).map(move |x| (x, y))).map(|(x, y)| layer_px(s, x, y)[3]).collect()
    };

    // Erase a band so there is transparency to smear, then alpha-lock.
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 85.0, "y0": 0.0, "x1": 100.0, "y1": 100.0}));
    ok(&mut s, json!({"cmd": "edit.delete_selection"}));
    ok(&mut s, json!({"cmd": "selection.clear"}));
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "alphaLocked": true}));
    let a0 = alphas(&s);
    let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &drag);
    assert_eq!(c["ok"]["committed"], true);
    assert_eq!(alphas(&s), a0, "alpha lock: no alpha value changed");
    assert_eq!(layer_px(&s, 90, 50), CLEAR16);
    assert!(layer_px(&s, 104, 50)[0] > 5_000, "color still moves under alpha lock: {:?}", layer_px(&s, 104, 50));
    ok(&mut s, json!({"cmd": "history.undo"}));
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "alphaLocked": false}));

    // Without alpha lock the same drag changes alpha (it smears into the transparent band).
    stroke(&mut s, json!({"cmd": "stroke.begin"}), &drag);
    assert_ne!(alphas(&s), a0);
    assert!(layer_px(&s, 90, 50)[3] > 10_000);
    ok(&mut s, json!({"cmd": "history.undo"}));

    // Selection: nothing outside it changes.
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 0.0, "y0": 0.0, "x1": 160.0, "y1": 50.0}));
    let doc = s.document().unwrap().clone();
    stroke(&mut s, json!({"cmd": "stroke.begin"}), &drag);
    let mut changed_inside = 0;
    for y in 0..100 {
        for x in 0..160 {
            let was = doc.layer(doc.active_layer.unwrap()).unwrap().pixels().unwrap().tiles.get_pixel(x, y);
            if layer_px(&s, x, y) != was {
                assert!(y < 50, "({x},{y}) changed outside the selection");
                changed_inside += 1;
            }
        }
    }
    assert!(changed_inside > 50, "{changed_inside}");
    ok(&mut s, json!({"cmd": "history.undo"}));
    ok(&mut s, json!({"cmd": "selection.clear"}));

    // Locked layer and vector layer: typed errors on both paths, nothing changes.
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "locked": true}));
    assert_eq!(err(&mut s, json!({"cmd": "stroke.begin"}))["code"], "LayerLocked");
    let v = s.version();
    let out = s.input_batch(&pen_path(&[(60.0, 50.0), (90.0, 50.0), (110.0, 50.0)], 0));
    assert_eq!((out.flags & flags::COMMITTED, s.version()), (0, v));
    assert!(!out.warnings.is_empty());
    ok(&mut s, json!({"cmd": "layer.set", "id": active, "locked": false}));
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    let e = err(&mut s, json!({"cmd": "stroke.begin", "tool": "smudge"}));
    assert_eq!(e["code"], "UnsupportedLayerFeature");
    assert!(!s.has_live_stroke());
}

#[test]
fn smudge_strength_zero_is_no_undo_step_and_tool_set_validates() {
    let mut s = two_tone();
    let v = s.version();
    ok(&mut s, json!({"cmd": "tool.set", "tool": "smudge", "strength": 0.0}));
    let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(60.0, 50.5, 110.0, 50.5, 26, 1.0));
    assert_eq!(c["ok"]["committed"], false, "nothing changed, so nothing to undo");
    assert_eq!(c["historyChanged"], false);
    assert_eq!(s.version(), v);
    // Smudging a uniform area is a no-op as well.
    ok(&mut s, json!({"cmd": "tool.set", "strength": 1.0}));
    let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(10.0, 20.0, 60.0, 80.0, 20, 1.0));
    assert_eq!(c["ok"]["committed"], false);
    // Cancel discards a live smudge.
    let img = render(&s);
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    ok(
        &mut s,
        json!({"cmd": "stroke.samples", "samples": [{"x": 60.0, "y": 50.5, "p": 1.0}, {"x": 110.0, "y": 50.5, "t": 9, "p": 1.0}]}),
    );
    assert_ne!(render(&s), img);
    let c = ok(&mut s, json!({"cmd": "stroke.cancel"}));
    assert_eq!(render(&s), img);
    assert!(!rects_of(&c).is_empty());
    assert_eq!(s.version(), v);

    let state = ok(&mut s, json!({"cmd": "tool.get"}))["ok"].clone();
    assert_eq!(
        (state["tool"].as_str(), state["strength"].as_f64(), state["smudgeMaxSize"].as_f64()),
        (Some("smudge"), Some(1.0), Some(256.0))
    );
    for bad in [
        json!({"cmd": "tool.set", "tool": "blur"}),
        json!({"cmd": "tool.set", "strength": 1.5}),
        json!({"cmd": "tool.set", "strength": -0.1}),
        json!({"cmd": "tool.set", "tool": "brush", "strength": 7.0}),
        json!({"cmd": "tool.set", "tool": "smudge", "strenght": 0.5}),
        json!({"cmd": "stroke.begin", "tool": "smear"}),
        json!({"cmd": "stroke.begin", "tol": "smudge"}),
    ] {
        let e = err(&mut s, bad.clone());
        assert_eq!(e["code"], "InvalidArgument", "{bad} -> {e}");
    }
    let e = s.execute(Command::ToolSet { tool: Some(ToolKind::Brush), strength: Some(f32::NAN) }).unwrap_err();
    assert_eq!(e.code, veskra_core::ErrorCode::InvalidArgument);
    assert_eq!(ok(&mut s, json!({"cmd": "tool.get"}))["ok"], state, "failed sets change nothing (atomic)");
    assert!(!s.has_live_stroke());
}

#[test]
fn smudge_is_replicated_by_symmetry() {
    let mut s = two_tone();
    ok(&mut s, json!({"cmd": "tool.set", "tool": "smudge"}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "horizontal"}));
    let before = s.document().unwrap().clone();
    let v = s.version();
    let c = stroke(&mut s, json!({"cmd": "stroke.begin"}), &line_pts(60.0, 25.5, 110.0, 25.5, 26, 1.0));
    assert_eq!(c["ok"]["committed"], true);
    assert_eq!(s.version(), v + 1);
    for x in [85, 90, 95, 100] {
        let (a, b) = (layer_px(&s, x, 25), layer_px(&s, x, 74));
        assert_eq!(a, b, "x = {x}: the mirrored head smudges the mirrored row identically");
        assert_ne!(a, BLUE16);
    }
    assert_eq!(layer_px(&s, 95, 50), BLUE16, "nothing between the two heads");
    ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(s.document().unwrap().as_ref(), before.as_ref());
}

// ------------------------------------------------------------------ work budget

#[test]
fn gesture_work_budget_refuses_hostile_paths_quickly() {
    let t0 = std::time::Instant::now();
    let mut s = two_tone();
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pencil.smooth"}));
    ok(&mut s, json!({"cmd": "brush.set", "size": 0.5}));
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "radial", "count": 16, "mirror": true}));
    let (doc, v) = (s.document().unwrap().clone(), s.version());

    // A shape whose outline × 32 replicas would mean walking 25 million px of path.
    let e = err(&mut s, json!({"cmd": "shape.rect", "x0": -1e5, "y0": -1e5, "x1": 1e5, "y1": 1e5, "fill": true}));
    assert_eq!(e["code"], "AllocationLimit", "{e}");
    let zigzag: Vec<[f64; 2]> = (0..9000).map(|i| [if i % 2 == 0 { -9e4 } else { 9e4 }, f64::from(i)]).collect();
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "off"}));
    let e = err(&mut s, json!({"cmd": "shape.path", "points": zigzag}));
    assert_eq!(e["code"], "AllocationLimit", "a 1.6 billion px polyline is refused even without symmetry: {e}");
    assert_eq!((s.document().unwrap().as_ref(), s.version()), (doc.as_ref(), v));

    // A symmetric stroke: the batch that would exceed the budget is refused as a whole, the
    // stroke keeps what it had and still commits.
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "quad"}));
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    ok(
        &mut s,
        json!({"cmd": "stroke.samples", "samples": [{"x": 20.0, "y": 20.0, "t": 0}, {"x": 50.0, "y": 30.0, "t": 1}]}),
    );
    let far: Vec<Value> =
        (0..40).map(|i| json!({"x": if i % 2 == 0 { -9e5 } else { 9e5 }, "y": 10.0 * i as f32, "t": 2 + i})).collect();
    let e = err(&mut s, json!({"cmd": "stroke.samples", "samples": far.clone()}));
    assert_eq!(e["code"], "AllocationLimit", "{e}");
    assert!(s.has_live_stroke());
    let c = ok(&mut s, json!({"cmd": "stroke.commit"}));
    assert_eq!(c["ok"]["strokeIds"].as_array().unwrap().len(), 4);
    assert!(strokes(&s).iter().all(|r| r.samples().len() == 2), "the refused batch left no sample behind");
    ok(&mut s, json!({"cmd": "history.undo"}));

    // Smudge is budgeted even without symmetry.
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "off"}));
    ok(&mut s, json!({"cmd": "stroke.begin", "tool": "smudge"}));
    let e = err(&mut s, json!({"cmd": "stroke.samples", "samples": far}));
    assert_eq!(e["code"], "AllocationLimit", "{e}");
    ok(&mut s, json!({"cmd": "stroke.cancel"}));

    // A huge ellipse guide turns a 2 px wiggle at its center into 180 000 px jumps along the
    // outline: the budget is measured on the projected path, and a refused batch does not move
    // the guide's per-stroke state.
    ok(
        &mut s,
        json!({"cmd": "guide.set", "kind": "ellipse", "x0": -89_920.0, "y0": -89_950.0, "x1": 90_080.0, "y1": 90_050.0}),
    );
    ok(&mut s, json!({"cmd": "stroke.begin", "tool": "brush"}));
    let wiggle: Vec<Value> =
        (0..40).map(|i| json!({"x": if i % 2 == 0 { 79.0 } else { 81.0 }, "y": 50.0, "t": i})).collect();
    let e = err(&mut s, json!({"cmd": "stroke.samples", "samples": wiggle}));
    assert_eq!(e["code"], "AllocationLimit", "{e}");
    let c = ok(&mut s, json!({"cmd": "stroke.commit"}));
    assert_eq!(c["ok"]["committed"], false, "nothing was accepted");
    ok(&mut s, json!({"cmd": "guide.set", "kind": "off"}));

    // Input path: an absurd predicted tail of a symmetric stroke is not previewed, and the
    // refusal of an over-long confirmed batch surfaces as a warning, not a panic.
    ok(&mut s, json!({"cmd": "symmetry.set", "mode": "radial", "count": 16, "mirror": true}));
    let out = s.input_batch(&[pen(PointerAction::Down, 30.0, 30.0, 0), pen(PointerAction::Move, 40.0, 40.0, 4)]);
    assert!(out.warnings.is_empty());
    let confirmed_only = render(&s);
    let mut p = pen(PointerAction::Move, 60_000.0, 50.0, 8);
    p.predicted = true;
    let out = s.input_batch(&[p]);
    assert!(out.warnings.is_empty());
    assert_eq!(render(&s), confirmed_only, "no preview for an over-budget prediction");
    let out = s.input_batch(&[pen(PointerAction::Move, 90_000.0, 60.0, 12), pen(PointerAction::Up, 50.0, 50.0, 16)]);
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(out.flags & flags::COMMITTED != 0, "the stroke still commits what was accepted");
    assert!(t0.elapsed().as_secs() < 20, "hostile gestures must be refused, not ground through");
}

// ------------------------------------------------------------------ stabilizer on the hot path

/// Rows of the active layer that contain paint in columns `x0..x1`.
fn painted_rows(s: &Session, x0: i32, x1: i32) -> (i32, i32) {
    let (_, h) = size(s);
    let rows: Vec<i32> = (0..h as i32).filter(|y| (x0..x1).any(|x| layer_px(s, x, *y)[3] > 0)).collect();
    (*rows.first().unwrap(), *rows.last().unwrap())
}

#[test]
fn stabilization_affects_the_input_batch_path() {
    // A ±20 px zigzag drawn through `input_batch`, once raw and once fully stabilized.
    let zigzag: Vec<(f32, f32)> =
        (0..=26).map(|i| (20.0 + i as f32 * 10.0, if i % 2 == 0 { 80.0 } else { 120.0 })).collect();
    let draw = |stabilization: f32| -> Session {
        let mut s = setup(300, 200);
        ok(&mut s, json!({"cmd": "brush.select", "preset": "round.hard"}));
        ok(&mut s, json!({"cmd": "brush.set", "size": 4.0, "stabilization": stabilization}));
        let events = pen_path(&zigzag, 0);
        let mut committed = false;
        for chunk in events.chunks(5) {
            committed |= s.input_batch(chunk).flags & flags::COMMITTED != 0;
        }
        assert!(committed);
        s
    };
    let raw = draw(0.0);
    let smooth = draw(1.0);
    let (r0, r1) = painted_rows(&raw, 100, 200);
    let (s0, s1) = painted_rows(&smooth, 100, 200);
    assert!(r0 <= 80 && r1 >= 119, "raw zigzag spans its full amplitude: {r0}..{r1}");
    assert!(s1 - s0 < (r1 - r0) - 20, "stabilized stroke is visibly flatter: {s0}..{s1} vs {r0}..{r1}");
    // The stabilizer changes the paint path, never the recorded samples, and it catches up to
    // the pen at the end of the stroke.
    let (a, b) = (&strokes(&raw)[0], &strokes(&smooth)[0]);
    assert_eq!(a.samples(), b.samples());
    assert_eq!(a.samples().len(), zigzag.len());
    assert!(layer_px(&smooth, 280, 80)[3] > 0, "stroke end reaches the last pen position");
    let params: Value = serde_json::from_str(&b.brush_params).unwrap();
    assert_eq!(params["stabilization"], 1.0);
    // Same result as the headless path with the same samples (one pipeline for both).
    let mut headless = setup(300, 200);
    ok(&mut headless, json!({"cmd": "brush.select", "preset": "round.hard"}));
    ok(&mut headless, json!({"cmd": "brush.set", "size": 4.0, "stabilization": 1.0}));
    ok(&mut headless, json!({"cmd": "stroke.begin"}));
    let samples: Vec<Value> =
        b.samples().iter().map(|q| json!({"x": q.x, "y": q.y, "t": q.t_ns, "p": q.pressure})).collect();
    ok(&mut headless, json!({"cmd": "stroke.samples", "samples": samples}));
    ok(&mut headless, json!({"cmd": "stroke.commit"}));
    assert_eq!(render(&headless), render(&smooth));
}

// ------------------------------------------------------------------ dirty-rect property

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len() as u64) as usize]
    }
}

const PW: u32 = 140;
const PH: u32 = 100;

fn random_tool_command(r: &mut Rng, s: &Session) -> Value {
    let ids: Vec<String> = s.document().unwrap().paint_order().iter().map(|l| l.to_string()).collect();
    let id = ids[r.below(ids.len() as u64) as usize].clone();
    let (x0, y0) = (r.f(-10.0, PW as f32), r.f(-10.0, PH as f32));
    let (x1, y1) = (x0 + r.f(-40.0, 80.0), y0 + r.f(-30.0, 60.0));
    match r.below(30) {
        0 | 1 => match r.below(5) {
            0 => json!({"cmd": "symmetry.set", "mode": "off"}),
            1 => json!({"cmd": "symmetry.set", "mode": r.pick(&["vertical", "horizontal", "quad"])}),
            2 => json!({"cmd": "symmetry.set", "mode": "radial", "count": 2 + r.below(7), "mirror": r.chance(40)}),
            3 => {
                json!({"cmd": "symmetry.set", "mode": "vertical", "cx": r.f(0.0, PW as f32), "cy": r.f(0.0, PH as f32)})
            }
            _ => {
                json!({"cmd": "symmetry.set", "mode": "radial", "count": 3, "cx": r.f(-50.0, 200.0), "cy": r.f(-50.0, 150.0)})
            }
        },
        2 => match r.below(4) {
            0 => json!({"cmd": "guide.set", "kind": "off"}),
            1 => json!({"cmd": "guide.set", "kind": "line", "x0": x0, "y0": y0, "x1": x1 + 1.0, "y1": y1}),
            2 => json!({"cmd": "guide.set", "kind": "line", "angle": r.f(-3.2, 3.2)}),
            _ => {
                json!({"cmd": "guide.set", "kind": "ellipse", "x0": x0, "y0": y0, "x1": x0 + r.f(5.0, 90.0), "y1": y0 + r.f(5.0, 70.0)})
            }
        },
        3 | 4 => {
            json!({"cmd": "tool.set", "tool": r.pick(&["brush", "brush", "eraser", "smudge"]), "strength": r.f(0.0, 1.0)})
        }
        5 => json!({"cmd": "shape.line", "x0": x0, "y0": y0, "x1": x1, "y1": y1}),
        6 => json!({"cmd": "shape.rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1, "fill": r.chance(50)}),
        7 => json!({"cmd": "shape.ellipse", "x0": x0, "y0": y0, "x1": x1, "y1": y1, "fill": r.chance(50)}),
        8 => {
            let n = 1 + r.below(6);
            let pts: Vec<[f32; 2]> =
                (0..n).map(|_| [r.f(-10.0, PW as f32 + 10.0), r.f(-10.0, PH as f32 + 10.0)]).collect();
            json!({"cmd": "shape.path", "points": pts, "closed": r.chance(50), "smooth": r.chance(50), "fill": r.chance(40)})
        }
        9 => json!({"cmd": "layer.add", "kind": r.pick(&["raster", "raster", "vector"])}),
        10 => json!({"cmd": "layer.select", "id": id}),
        11 => json!({"cmd": "layer.set", "id": id, "alphaLocked": r.chance(40)}),
        12 => json!({"cmd": "layer.set", "id": id, "locked": r.chance(20)}),
        13 => json!({"cmd": "layer.set", "id": id, "opacity": r.f(0.2, 1.0)}),
        14 => {
            json!({"cmd": "selection.rect", "x0": x0, "y0": y0, "x1": x0 + r.f(5.0, 90.0), "y1": y0 + r.f(5.0, 70.0)})
        }
        15 => json!({"cmd": "selection.clear"}),
        16 | 17 => json!({"cmd": "history.undo"}),
        18 => json!({"cmd": "history.redo"}),
        19 => json!({"cmd": "color.set", "hex": format!("#{:06X}", r.below(0x100_0000))}),
        20 => {
            json!({"cmd": "brush.select", "preset": r.pick(&["pencil.smooth", "pen.brush", "round.hard", "charcoal", "round.soft", "eraser.soft"])})
        }
        21 => {
            json!({"cmd": "brush.set", "size": r.f(1.0, 40.0), "stabilization": r.f(0.0, 1.0), "opacity": r.f(0.3, 1.0)})
        }
        22 => {
            json!({"cmd": "stroke.begin", "eraser": r.chance(15), "tool": r.pick(&["brush", "eraser", "smudge", "smudge"])})
        }
        23 => json!({"cmd": "stroke.begin"}),
        24 | 25 => {
            let n = 1 + r.below(6);
            let t0 = r.below(1000) * 1_000_000;
            let samples: Vec<Value> = (0..n)
                .map(|i| json!({"x": r.f(-5.0, PW as f32 + 5.0), "y": r.f(-5.0, PH as f32 + 5.0), "t": t0 + i * 4_000_000, "p": r.f(0.0, 1.0)}))
                .collect();
            json!({"cmd": "stroke.samples", "samples": samples})
        }
        26 => json!({"cmd": "stroke.commit"}),
        27 => json!({"cmd": "stroke.cancel"}),
        28 => json!({"cmd": "input.reset"}),
        _ => json!({"cmd": "edit.fill", "x": r.f(0.0, PW as f32), "y": r.f(0.0, PH as f32)}),
    }
}

fn random_pen_batch(r: &mut Rng, t: &mut u64, pen_down: &mut bool) -> Vec<RawPointerEvent> {
    let mut out = Vec::new();
    for _ in 0..1 + r.below(8) {
        *t += 1 + r.below(8) * 1_000_000;
        let (x, y) = (r.f(-5.0, PW as f32 + 5.0), r.f(-5.0, PH as f32 + 5.0));
        let action = if !*pen_down {
            if r.chance(20) { PointerAction::HoverMove } else { PointerAction::Down }
        } else {
            match r.below(10) {
                0 => PointerAction::Up,
                1 => PointerAction::Cancel,
                _ => PointerAction::Move,
            }
        };
        let tool = if r.chance(8) { ToolType::StylusEraser } else { ToolType::Stylus };
        let mut e = RawPointerEvent::new(action, 0, tool, x, y, *t);
        e.pressure = Some(r.f(0.05, 1.0));
        match action {
            PointerAction::Down => *pen_down = true,
            PointerAction::Up | PointerAction::Cancel => *pen_down = false,
            _ => {}
        }
        out.push(e);
        if *pen_down && action == PointerAction::Move && r.chance(60) {
            for k in 0..1 + r.below(3) {
                let mut p = RawPointerEvent::new(
                    action,
                    0,
                    tool,
                    x + r.f(-30.0, 30.0),
                    y + r.f(-30.0, 30.0),
                    *t + (k + 1) * 4_000_000,
                );
                p.pressure = Some(r.f(0.05, 1.0));
                p.predicted = true;
                out.push(p);
            }
        }
    }
    out
}

fn check_tool_sequence(seed: u64, steps: usize) {
    let mut r = Rng(seed);
    let mut s = Session::new(EngineConfig { deterministic_seed: Some(seed), ..Default::default() });
    ok(&mut s, json!({"cmd": "document.create", "width": PW, "height": PH, "background": "#E8D8C8"}));
    ok(&mut s, json!({"cmd": "view.size", "width": PW, "height": PH}));
    ok(&mut s, json!({"cmd": "view.set", "zoom": 1.0, "rotation": 0.0, "panX": 0.0, "panY": 0.0}));
    // Start from non-uniform pixels so smudge has something to move.
    ok(&mut s, json!({"cmd": "color.set", "hex": "#B03060"}));
    ok(&mut s, json!({"cmd": "shape.ellipse", "x0": 20.0, "y0": 15.0, "x1": 100.0, "y1": 85.0, "fill": true}));
    let mut t = 1_000_000_000u64;
    let mut pen_down = false;
    let mut prev = render(&s);
    for step in 0..steps {
        let v0 = s.version();
        let doc0 = s.document().unwrap().clone();
        let (rects, what): (Vec<RectI>, String) = if r.chance(45) {
            let batch = random_pen_batch(&mut r, &mut t, &mut pen_down);
            let out = s.input_batch(&batch);
            let changed = s.version() != v0;
            assert_eq!(out.flags & flags::HISTORY != 0, changed, "seed {seed} step {step}: HISTORY flag vs version");
            if out.flags & flags::COMMITTED != 0 {
                assert!(changed, "seed {seed} step {step}: COMMITTED without a new version");
            }
            assert_eq!(out.flags & flags::DIRTY != 0, !out.dirty.is_empty(), "seed {seed} step {step}: DIRTY flag");
            (out.dirty, format!("input batch of {} events", batch.len()))
        } else {
            let cmd = random_tool_command(&mut r, &s);
            let out = run(&mut s, cmd.clone());
            if out.get("error").is_some() {
                assert_eq!(s.version(), v0, "seed {seed} step {step}: failed {cmd} changed the version");
                assert_eq!(s.document().unwrap().as_ref(), doc0.as_ref(), "seed {seed} step {step}: failed {cmd}");
                // A failed commit carries no rects although its live preview is gone: headless
                // callers refresh everything (COMMANDS.md). Possible here because a symmetry
                // replica far off a vector layer's canvas has no vector form.
                let rects = if cmd["cmd"] == "stroke.commit" { vec![RectI::from_size(PW, PH)] } else { vec![] };
                (rects, format!("FAILED {cmd} -> {out}"))
            } else {
                assert_eq!(
                    out["historyChanged"].as_bool().unwrap(),
                    s.version() != v0,
                    "seed {seed} step {step}: historyChanged vs version for {cmd} -> {out}"
                );
                (rects_of(&out), format!("{cmd} -> {out}"))
            }
        };
        s.document().unwrap().validate().unwrap();
        let next = render(&s);
        if let Some((x, y)) = stray_change(&prev, &next, PW, &rects) {
            panic!("seed {seed} step {step}: pixel ({x},{y}) changed outside invalidated {rects:?} after {what}");
        }
        prev = next;
    }
}

#[test]
fn changed_pixels_stay_inside_invalidated_rects_with_tools_active() {
    // VESKRA_PROPTEST_SEEDS=N widens the sweep.
    let seeds: u64 = std::env::var("VESKRA_PROPTEST_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for seed in 1..=seeds {
        check_tool_sequence(seed, 150);
    }
}
