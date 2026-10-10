#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Engine-level tests for the raster-edit, selection and canvas commands: `edit.gradient`,
//! `selection.wand`, `edit.adjust`, `edit.transform_quad`, `canvas.crop`, `canvas.resize`,
//! and the read-only `selection.outline` / `layer.bounds` / `input.config {lockView}`.
//!
//! Each mutating command is checked for: pixel results, exactly one undo step, exact undo/redo,
//! selection clipping, `invalidated` covering every changed display pixel, `.veskra` round trip,
//! and typed errors (never a panic, never a state change) for hostile input.

use serde_json::{Value, json};
use veskra_core::RectI;
use veskra_engine::{
    AdjustKindSpec, Command, EngineConfig, GradientKindSpec, GradientStopSpec, Receipt, SelectionOp, Session,
};

fn session() -> Session {
    Session::new(EngineConfig { deterministic_seed: Some(7), ..Default::default() })
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

/// Premultiplied RGBA16 pixel of the active layer.
fn px(s: &Session, x: i32, y: i32) -> [u16; 4] {
    let d = s.document().unwrap();
    d.layer(d.active_layer.unwrap()).unwrap().pixels().unwrap().tiles.get_pixel(x, y)
}

fn info(s: &mut Session) -> Value {
    ok(s, json!({"cmd": "document.info"}))["ok"].clone()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-tools-raster-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn doc(w: u32, h: u32, bg: Option<&str>) -> Session {
    let mut s = session();
    ok(&mut s, json!({"cmd": "document.create", "width": w, "height": h, "background": bg}));
    s
}

fn stroke(s: &mut Session, pts: &[(f32, f32)]) {
    ok(s, json!({"cmd": "stroke.begin"}));
    let samples: Vec<Value> = pts
        .iter()
        .enumerate()
        .map(|(i, (x, y))| json!({"x": x, "y": y, "t": i as u64 * 4_000_000, "p": 0.7}))
        .collect();
    ok(s, json!({"cmd": "stroke.samples", "samples": samples}));
    ok(s, json!({"cmd": "stroke.commit"}));
}

fn fill_rect(s: &mut Session, hex: &str, r: (i32, i32, i32, i32)) {
    ok(s, json!({"cmd": "color.set", "hex": hex}));
    ok(s, json!({"cmd": "selection.rect", "x0": r.0, "y0": r.1, "x1": r.2, "y1": r.3}));
    ok(s, json!({"cmd": "edit.fill_selection"}));
    ok(s, json!({"cmd": "selection.clear"}));
}

/// Run a mutating command and verify the generic contract: exactly one undo step, `invalidated`
/// covers every changed display pixel, undo restores the document exactly and redo reproduces it.
fn mutate_checked(s: &mut Session, cmd: Value, label: &str) -> Value {
    let before_doc = s.document().unwrap().clone();
    let before_px = render(s);
    let (bw, bh) = size(s);
    let v0 = s.version();
    let out = ok(s, cmd.clone());
    assert_eq!(s.version(), v0 + 1, "{cmd}: one version step");
    assert_eq!(out["historyChanged"], true);
    assert_eq!(info(s)["undoLabel"], label, "{cmd}");
    let after_doc = s.document().unwrap().clone();
    let after_px = render(s);
    let (w, h) = size(s);
    let rects: Vec<RectI> = out["invalidated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let v: Vec<i32> = r.as_array().unwrap().iter().map(|x| x.as_i64().unwrap() as i32).collect();
            RectI::new(v[0], v[1], v[2], v[3])
        })
        .collect();
    if (w, h) == (bw, bh) {
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let i = (y as usize * w as usize + x as usize) * 4;
                if before_px[i..i + 4] != after_px[i..i + 4] {
                    assert!(rects.iter().any(|r| r.contains(x, y)), "{cmd}: changed pixel ({x},{y}) not invalidated");
                }
            }
        }
    } else {
        let all = RectI::from_size(w.max(bw), h.max(bh));
        assert!(rects.iter().any(|r| r.intersect(&all) == all), "{cmd}: canvas change must invalidate everything");
        assert_eq!(out["viewChanged"], true);
    }
    let u = ok(s, json!({"cmd": "history.undo"}));
    assert_eq!(u["ok"]["undone"], label);
    assert_eq!(s.document().unwrap().as_ref(), before_doc.as_ref(), "{cmd}: undo is exact");
    assert_eq!(render(s), before_px);
    ok(s, json!({"cmd": "history.redo"}));
    assert_eq!(s.document().unwrap().as_ref(), after_doc.as_ref(), "{cmd}: redo is exact");
    assert_eq!(render(s), after_px);
    out
}

/// A failing command leaves version, document and history untouched.
fn fails(s: &mut Session, cmd: Value, code: &str) {
    let before = s.document().unwrap().clone();
    let v = s.version();
    let undo = info(s)["undoLabel"].clone();
    let e = err(s, cmd.clone());
    assert_eq!(e["code"], code, "{cmd}: {e}");
    assert_eq!(s.version(), v, "{cmd}");
    assert_eq!(s.document().unwrap().as_ref(), before.as_ref(), "{cmd}");
    assert_eq!(info(s)["undoLabel"], undo, "{cmd}");
}

fn fails_typed(s: &mut Session, cmd: Command) {
    let before = s.document().unwrap().clone();
    let v = s.version();
    let e = s.execute(cmd.clone()).expect_err("hostile command must fail");
    assert!(
        matches!(e.code, veskra_core::ErrorCode::InvalidArgument | veskra_core::ErrorCode::AllocationLimit),
        "{cmd:?}: {e:?}"
    );
    assert_eq!(s.version(), v);
    assert_eq!(s.document().unwrap().as_ref(), before.as_ref());
}

fn save_reopen(s: &mut Session, name: &str) -> Session {
    let path = tmp(name);
    ok(s, json!({"cmd": "document.save", "path": path.to_string_lossy()}));
    let mut s2 = session();
    ok(&mut s2, json!({"cmd": "document.open", "path": path.to_string_lossy()}));
    assert_eq!(s.document().unwrap().as_ref(), s2.document().unwrap().as_ref(), "{name}: document round-trips");
    assert_eq!(render(s), render(&s2), "{name}: composite round-trips");
    s2
}

fn q16(v: f64) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5).floor() as u16
}

// ------------------------------------------------------------------------------------ gradient

#[test]
fn gradient_default_stops_are_foreground_to_transparent() {
    let mut s = doc(64, 32, None);
    ok(&mut s, json!({"cmd": "color.set", "hex": "#FF0000"}));
    let out = mutate_checked(&mut s, json!({"cmd": "edit.gradient", "x0": 0, "y0": 0, "x1": 64, "y1": 0}), "Gradient");
    assert_eq!(out["invalidated"], json!([[0, 0, 64, 32]]));
    for x in [0, 1, 31, 32, 63] {
        let a = q16(1.0 - (x as f64 + 0.5) / 64.0);
        assert_eq!(px(&s, x, 5), [a, 0, 0, a], "x={x}: premultiplied red fading out");
        assert_eq!(px(&s, x, 31), px(&s, x, 0), "rows are identical for a horizontal gradient");
    }
    assert_eq!(ok(&mut s, json!({"cmd": "layer.list"}))["ok"]["layers"][0]["vectorFidelity"], "raster");
    save_reopen(&mut s, "gradient.veskra");
}

#[test]
fn gradient_stops_radial_opacity_and_selection_clip() {
    let mut s = doc(64, 64, Some("#FFFFFF"));
    let stops = json!([{"offset": 0.0, "color": "#000000"}, {"offset": 1.0, "color": "#FFFFFF"}]);
    mutate_checked(
        &mut s,
        json!({"cmd": "edit.gradient", "x0": 0, "y0": 0, "x1": 0, "y1": 64, "kind": "linear", "stops": stops}),
        "Gradient",
    );
    for y in [0, 20, 63] {
        let v = q16((y as f64 + 0.5) / 64.0);
        assert_eq!(px(&s, 7, y), [v, v, v, 65535]);
    }
    // Radial with three stops, 50 % opacity, clipped by a selection.
    ok(&mut s, json!({"cmd": "layer.add", "kind": "raster"}));
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 16, "y0": 16, "x1": 48, "y1": 48}));
    let stops = json!([{"offset": 0.0, "color": "#FF0000"}, {"offset": 0.5, "color": "#00FF00"},
                       {"offset": 1.0, "color": "#0000FF"}]);
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "edit.gradient", "x0": 32, "y0": 32, "x1": 32, "y1": 52, "kind": "radial", "stops": stops,
               "opacity": 0.5}),
        "Gradient",
    );
    assert_eq!(out["invalidated"], json!([[16, 16, 48, 48]]), "dirty rect is the selection");
    assert_eq!(px(&s, 15, 32), [0; 4], "outside the selection nothing is painted");
    assert_eq!(px(&s, 48, 32), [0; 4]);
    // Pixel (32,32): centre distance = |(0.5, 0.5)| / 20 → between red and green.
    let t = (0.5f64 * 0.5 * 2.0).sqrt() / 20.0;
    let f = t / 0.5;
    let half = |v: f64| ((q16(v) as u32 * 32768 + 32767) / 65535) as u16;
    assert_eq!(px(&s, 32, 32), [half(1.0 - f), half(f), 0, 32768]);
    // Beyond the radius the last stop pads.
    assert_eq!(px(&s, 17, 17), [0, 0, 32768, 32768]);
    // Alpha lock: a gradient only recolors existing pixels.
    ok(&mut s, json!({"cmd": "selection.clear"}));
    let id = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "layer.set", "id": id, "alphaLocked": true}));
    let stops = json!([{"offset": 0.0, "color": "#FFFFFF"}, {"offset": 1.0, "color": "#FFFFFF"}]);
    mutate_checked(
        &mut s,
        json!({"cmd": "edit.gradient", "x0": 0, "y0": 0, "x1": 64, "y1": 0, "stops": stops}),
        "Gradient",
    );
    assert_eq!(px(&s, 15, 32), [0; 4], "alpha lock keeps transparent pixels transparent");
    assert_eq!(px(&s, 32, 32), [32768; 4], "existing pixel recolored, alpha preserved");
}

#[test]
fn gradient_hostile_input_and_layer_policies() {
    let mut s = doc(32, 32, None);
    let base = json!({"cmd": "edit.gradient", "x0": 0, "y0": 0, "x1": 10, "y1": 0});
    let with = |k: &str, v: Value| {
        let mut c = base.clone();
        c[k] = v;
        c
    };
    fails(&mut s, with("x1", json!(0)), "InvalidArgument"); // zero-length axis
    fails(&mut s, with("stops", json!([])), "InvalidArgument");
    fails(&mut s, with("stops", json!([{"offset": 0.5, "color": "#FFFFFF"}])), "InvalidArgument");
    fails(
        &mut s,
        with("stops", json!([{"offset": 0, "color": "#FFF"}, {"offset": 1, "color": "#000000"}])),
        "InvalidArgument",
    );
    fails(
        &mut s,
        with("stops", json!([{"offset": -0.1, "color": "#FFFFFF"}, {"offset": 1, "color": "#000000"}])),
        "InvalidArgument",
    );
    fails(
        &mut s,
        with("stops", json!([{"offset": 0, "color": "#FFFFFF", "x": 1}, {"offset": 1, "color": "#000000"}])),
        "InvalidArgument",
    );
    fails(&mut s, with("opacity", json!(1.5)), "InvalidArgument");
    fails(&mut s, with("kind", json!("conic")), "InvalidArgument");
    fails(&mut s, with("stopz", json!([])), "InvalidArgument"); // unknown field
    fails(&mut s, with("x0", json!(1e300)), "InvalidArgument");
    let typed = |x0: f64, opacity: f32, stops: Option<Vec<GradientStopSpec>>| Command::EditGradient {
        x0,
        y0: 0.0,
        x1: 10.0,
        y1: 0.0,
        kind: GradientKindSpec::Linear,
        stops,
        opacity,
    };
    fails_typed(&mut s, typed(f64::NAN, 1.0, None));
    fails_typed(&mut s, typed(f64::INFINITY, 1.0, None));
    fails_typed(&mut s, typed(0.0, f32::NAN, None));
    let million: Vec<GradientStopSpec> =
        (0..1_000_000).map(|i| GradientStopSpec { offset: i as f64 / 1e6, color: "#000000".into() }).collect();
    fails_typed(&mut s, typed(0.0, 1.0, Some(million)));
    fails_typed(&mut s, typed(0.0, 1.0, Some(vec![GradientStopSpec { offset: f64::NAN, color: "#000000".into() }; 2])));
    // 256 stops are accepted.
    let many: Vec<Value> = (0..256).map(|i| json!({"offset": i as f64 / 255.0, "color": "#102030"})).collect();
    ok(&mut s, with("stops", json!(many)));
    // Locked layer and vector layer are refused with their typed codes.
    let id = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "layer.set", "id": id, "locked": true}));
    fails(&mut s, base.clone(), "LayerLocked");
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    fails(&mut s, base.clone(), "UnsupportedLayerFeature");
    // No document at all.
    let e = err(&mut session(), base);
    assert_eq!(e["code"], "InvalidState");
}

// ------------------------------------------------------------------------------------ magic wand

#[test]
fn wand_selects_the_bucket_fill_region() {
    let build = || {
        let mut s = doc(96, 64, None);
        fill_rect(&mut s, "#FF0000", (10, 10, 50, 40));
        fill_rect(&mut s, "#00FF00", (60, 20, 90, 60));
        ok(&mut s, json!({"cmd": "color.set", "hex": "#0000FF"}));
        s
    };
    // wand + fill_selection == edit.fill (same region logic, same soft edge).
    for (x, y, tol, contiguous) in [(20.5, 20.5, 0.1, true), (5.0, 5.0, 0.0, true), (70.0, 30.0, 0.5, false)] {
        let mut a = build();
        ok(&mut a, json!({"cmd": "edit.fill", "x": x, "y": y, "tolerance": tol, "contiguous": contiguous}));
        let mut b = build();
        let out = mutate_checked(
            &mut b,
            json!({"cmd": "selection.wand", "x": x, "y": y, "tolerance": tol, "contiguous": contiguous}),
            "Selection",
        );
        assert_eq!(out["ok"]["empty"], false);
        ok(&mut b, json!({"cmd": "edit.fill_selection"}));
        ok(&mut b, json!({"cmd": "selection.clear"}));
        let (da, db) = (a.document().unwrap(), b.document().unwrap());
        let tiles = |d: &veskra_engine::veskra_doc::Document| {
            d.layer(d.active_layer.unwrap()).unwrap().pixels().unwrap().tiles.clone()
        };
        assert_eq!(tiles(da), tiles(db), "wand({x},{y},{tol},{contiguous})");
    }
    // The red rectangle is selected exactly (defaults: tolerance 0.1, contiguous, active layer).
    let mut s = build();
    ok(&mut s, json!({"cmd": "selection.wand", "x": 20, "y": 20}));
    let sel = s.document().unwrap().selection.clone().unwrap();
    assert_eq!(sel.bounds(), RectI::new(10, 10, 50, 40));
    assert_eq!(sel.get(10, 10), 255);
    assert_eq!(sel.get(9, 10), 0);
    assert_eq!(sel.get(70, 30), 0);
    // add / subtract / intersect compose with the existing selection.
    ok(&mut s, json!({"cmd": "selection.wand", "x": 70, "y": 30, "op": "add"}));
    let sel = s.document().unwrap().selection.clone().unwrap();
    assert_eq!((sel.get(20, 20), sel.get(70, 30), sel.get(55, 30)), (255, 255, 0));
    ok(&mut s, json!({"cmd": "selection.wand", "x": 20, "y": 20, "op": "subtract"}));
    let sel = s.document().unwrap().selection.clone().unwrap();
    assert_eq!((sel.get(20, 20), sel.get(70, 30)), (0, 255));
    let out = ok(&mut s, json!({"cmd": "selection.wand", "x": 20, "y": 20, "op": "intersect"}));
    assert_eq!(out["ok"]["empty"], true);
    assert_eq!(info(&mut s)["hasSelection"], false);
    // Non-contiguous picks every matching pixel; sampleMerged reads the composite.
    ok(&mut s, json!({"cmd": "layer.add", "kind": "raster"}));
    let out = ok(&mut s, json!({"cmd": "selection.wand", "x": 20, "y": 20, "sampleMerged": true}));
    assert_eq!(out["ok"]["empty"], false);
    assert_eq!(s.document().unwrap().selection.clone().unwrap().bounds(), RectI::new(10, 10, 50, 40));
    ok(&mut s, json!({"cmd": "selection.wand", "x": 20, "y": 20}));
    assert_eq!(
        s.document().unwrap().selection.clone().unwrap().bounds(),
        RectI::new(0, 0, 96, 64),
        "the empty active layer matches everywhere"
    );
    save_reopen(&mut s, "wand.veskra");
}

#[test]
fn wand_hostile_input() {
    let mut s = doc(32, 32, None);
    fails(&mut s, json!({"cmd": "selection.wand", "x": 100, "y": 5}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.wand", "x": -1, "y": 5}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.wand", "x": 5, "y": 5, "tolerance": 2}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.wand", "x": 5, "y": 5, "tolerence": 0.2}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.wand", "x": 5, "y": 5, "op": "xor"}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.wand", "x": 5}), "InvalidArgument");
    let wand = |x: f32, tolerance: f32| Command::SelectionWand {
        x,
        y: 1.0,
        tolerance,
        contiguous: true,
        sample_merged: false,
        op: SelectionOp::Replace,
    };
    fails_typed(&mut s, wand(f32::NAN, 0.1));
    fails_typed(&mut s, wand(1.0, f32::NAN));
    fails_typed(&mut s, wand(f32::INFINITY, 0.1));
}

// ------------------------------------------------------------------------------------ adjustments

#[test]
fn adjustments_change_color_never_alpha() {
    let mut s = doc(64, 64, None);
    fill_rect(&mut s, "#804020", (8, 8, 40, 40));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#80402080"}));
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 40, "y0": 8, "x1": 56, "y1": 40}));
    ok(&mut s, json!({"cmd": "edit.fill_selection"}));
    ok(&mut s, json!({"cmd": "selection.clear"}));
    let alpha = |s: &Session| -> Vec<u16> {
        (0..64).flat_map(|y| (0..64).map(move |x| (x, y))).map(|(x, y)| px(s, x, y)[3]).collect()
    };
    let a0 = alpha(&s);
    let original = s.document().unwrap().clone();

    // Invert: exact premultiplied complement, and twice is the identity.
    let p = px(&s, 10, 10);
    let h = px(&s, 45, 10);
    let out = mutate_checked(&mut s, json!({"cmd": "edit.adjust", "kind": "invert"}), "Adjust");
    assert_eq!(out["invalidated"], json!([[8, 8, 56, 40]]));
    assert_eq!(px(&s, 10, 10), [65535 - p[0], 65535 - p[1], 65535 - p[2], 65535]);
    assert_eq!(px(&s, 45, 10), [h[3] - h[0], h[3] - h[1], h[3] - h[2], h[3]], "half-transparent pixel");
    assert_eq!(ok(&mut s, json!({"cmd": "color.pick", "x": 10, "y": 10, "layer": true}))["ok"]["picked"], "#7FBFDF");
    ok(&mut s, json!({"cmd": "edit.adjust", "kind": "invert"}));
    let d = s.document().unwrap();
    let tiles = |d: &veskra_engine::veskra_doc::Document| {
        d.layer(d.active_layer.unwrap()).unwrap().pixels().unwrap().tiles.clone()
    };
    assert_eq!(tiles(d), tiles(&original), "invert twice restores every pixel bit-exactly");
    assert_eq!(alpha(&s), a0);

    // HSB: hue rotation of pure red, saturation to gray, brightness to black.
    fill_rect(&mut s, "#FF0000", (8, 8, 40, 40));
    mutate_checked(&mut s, json!({"cmd": "edit.adjust", "kind": "hsb", "hue": 120}), "Adjust");
    assert_eq!(px(&s, 10, 10), [0, 65535, 0, 65535]);
    mutate_checked(&mut s, json!({"cmd": "edit.adjust", "kind": "hsb", "saturation": -1}), "Adjust");
    assert_eq!(px(&s, 10, 10), [65535; 4], "desaturated pure green keeps its HSB value");
    mutate_checked(&mut s, json!({"cmd": "edit.adjust", "kind": "hsb", "brightness": -0.5}), "Adjust");
    assert_eq!(px(&s, 10, 10), [32768, 32768, 32768, 65535]);
    assert_eq!(alpha(&s), a0);

    // Brightness/contrast: v' = (v − 0.5)·k + 0.5 + b with k = (1 + c)/(1 − c).
    mutate_checked(&mut s, json!({"cmd": "edit.adjust", "kind": "brightness_contrast", "brightness": 0.25}), "Adjust");
    assert_eq!(px(&s, 10, 10)[0], q16(32768.0 / 65535.0 + 0.25));
    mutate_checked(&mut s, json!({"cmd": "edit.adjust", "kind": "brightness_contrast", "contrast": 1}), "Adjust");
    assert_eq!(px(&s, 10, 10), [65535; 4], "full contrast thresholds at mid gray");

    // Curves: master curve through (0.5 → 0.25), then a red-only curve.
    fill_rect(&mut s, "#808080", (8, 8, 40, 40));
    mutate_checked(
        &mut s,
        json!({"cmd": "edit.adjust", "kind": "curves", "rgb": [[0, 0], [0.5, 0.25], [1, 1]], "r": [[0, 1], [1, 0]]}),
        "Adjust",
    );
    let v = px(&s, 10, 10);
    assert!((v[1] as f64 / 65535.0 - 0.2523).abs() < 2e-3, "gray 0x80 through the master curve: {v:?}");
    assert_eq!(v[1], v[2]);
    assert_eq!(v[0], 65535 - v[1], "red is additionally inverted by its own curve");
    assert_eq!(alpha(&s), a0, "no adjustment touches alpha");

    // Selection clipping: only selected pixels change, the dirty rect is the selection.
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 8, "y0": 8, "x1": 16, "y1": 16}));
    let outside = px(&s, 20, 20);
    let out = mutate_checked(&mut s, json!({"cmd": "edit.adjust", "kind": "invert"}), "Adjust");
    assert_eq!(out["invalidated"], json!([[8, 8, 16, 16]]));
    assert_eq!(px(&s, 20, 20), outside);
    assert_ne!(px(&s, 10, 10), v);
    save_reopen(&mut s, "adjust.veskra");
}

#[test]
fn adjust_hostile_input_and_layer_policies() {
    let mut s = doc(32, 32, None);
    fill_rect(&mut s, "#336699", (0, 0, 32, 32));
    for cmd in [
        json!({"cmd": "edit.adjust"}),
        json!({"cmd": "edit.adjust", "kind": "sepia"}),
        json!({"cmd": "edit.adjust", "kind": "hsb", "hue": 181}),
        json!({"cmd": "edit.adjust", "kind": "hsb", "saturation": -1.01}),
        json!({"cmd": "edit.adjust", "kind": "hsb", "contrast": 0.1}),
        json!({"cmd": "edit.adjust", "kind": "hsb", "hue": 10, "huee": 10}),
        json!({"cmd": "edit.adjust", "kind": "invert", "hue": 10}),
        json!({"cmd": "edit.adjust", "kind": "brightness_contrast", "brightness": 2}),
        json!({"cmd": "edit.adjust", "kind": "brightness_contrast", "hue": 2}),
        json!({"cmd": "edit.adjust", "kind": "curves"}),
        json!({"cmd": "edit.adjust", "kind": "curves", "rgb": []}),
        json!({"cmd": "edit.adjust", "kind": "curves", "rgb": [[0.5, 0.5]]}),
        json!({"cmd": "edit.adjust", "kind": "curves", "rgb": [[0, 0], [0, 1]]}),
        json!({"cmd": "edit.adjust", "kind": "curves", "rgb": [[0, 0], [2, 1]]}),
        json!({"cmd": "edit.adjust", "kind": "curves", "rgb": [[0, 0, 0], [1, 1, 1]]}),
        json!({"cmd": "edit.adjust", "kind": "curves", "g": [[0, 0], [1, 1]], "brightness": 0.1}),
    ] {
        fails(&mut s, cmd, "InvalidArgument");
    }
    let adjust = |kind, hue: Option<f64>, rgb: Option<Vec<[f64; 2]>>| Command::EditAdjust {
        kind,
        hue,
        saturation: None,
        brightness: None,
        contrast: None,
        rgb,
        r: None,
        g: None,
        b: None,
    };
    fails_typed(&mut s, adjust(AdjustKindSpec::Hsb, Some(f64::NAN), None));
    fails_typed(&mut s, adjust(AdjustKindSpec::Curves, None, Some(vec![[0.0, f64::NAN], [1.0, 1.0]])));
    fails_typed(&mut s, adjust(AdjustKindSpec::Curves, None, Some(vec![[0.5, 0.5]; 1_000_000])));
    let id = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "layer.set", "id": id, "locked": true}));
    fails(&mut s, json!({"cmd": "edit.adjust", "kind": "invert"}), "LayerLocked");
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    fails(&mut s, json!({"cmd": "edit.adjust", "kind": "invert"}), "UnsupportedLayerFeature");
}

// ------------------------------------------------------------------------------------ distort

#[test]
fn transform_quad_maps_content_bounds_onto_the_quad() {
    let mut s = doc(128, 128, None);
    fill_rect(&mut s, "#FF0000", (20, 20, 60, 50));
    let before = s.document().unwrap().clone();
    // Identity quad: pixels unchanged.
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "edit.transform_quad", "quad": [[20, 20], [60, 20], [60, 50], [20, 50]]}),
        "Transform",
    );
    assert_eq!(out["ok"]["sourceBounds"], json!([20, 20, 60, 50]));
    let tiles = |d: &veskra_engine::veskra_doc::Document| {
        d.layer(d.active_layer.unwrap()).unwrap().pixels().unwrap().tiles.clone()
    };
    assert_eq!(tiles(s.document().unwrap()), tiles(&before));
    // Pure translation by (+30, +40) via the quad is exact.
    mutate_checked(
        &mut s,
        json!({"cmd": "edit.transform_quad", "quad": [[50, 60], [90, 60], [90, 90], [50, 90]]}),
        "Transform",
    );
    assert_eq!(px(&s, 50, 60), [65535, 0, 0, 65535]);
    assert_eq!(px(&s, 89, 89), [65535, 0, 0, 65535]);
    assert_eq!(px(&s, 49, 60), [0; 4]);
    assert_eq!(px(&s, 25, 25), [0; 4]);
    assert_eq!(ok(&mut s, json!({"cmd": "layer.bounds"}))["ok"]["bounds"], json!([50, 60, 90, 90]));
    // Perspective trapezoid (narrow top).
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "edit.transform_quad", "quad": [[40, 10], [80, 10], [110, 110], [10, 110]]}),
        "Transform",
    );
    assert_eq!(out["ok"]["sourceBounds"], json!([50, 60, 90, 90]));
    assert_eq!(px(&s, 60, 60), [65535, 0, 0, 65535]);
    assert_eq!(px(&s, 44, 14), [65535, 0, 0, 65535], "inside near the top-left corner");
    assert_eq!(px(&s, 20, 14), [0; 4], "left of the slanted edge");
    assert_eq!(px(&s, 20, 100), [65535, 0, 0, 65535], "inside near the bottom-left corner");
    assert_eq!(px(&s, 100, 100), [65535, 0, 0, 65535], "inside near the bottom-right corner");
    let b = ok(&mut s, json!({"cmd": "layer.bounds"}))["ok"]["bounds"].clone();
    let b: Vec<i64> = b.as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    assert!(b[0] >= 9 && b[1] >= 9 && b[2] <= 111 && b[3] <= 111 && b[2] - b[0] >= 98 && b[3] - b[1] >= 98, "{b:?}");
    assert_eq!(ok(&mut s, json!({"cmd": "layer.list"}))["ok"]["layers"][0]["vectorFidelity"], "raster");
    save_reopen(&mut s, "quad.veskra");
}

#[test]
fn transform_quad_moves_only_the_selection_and_warps_it() {
    let mut s = doc(96, 96, None);
    fill_rect(&mut s, "#00FF00", (0, 0, 96, 96));
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 10, "y0": 10, "x1": 30, "y1": 30}));
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "edit.transform_quad", "quad": [[50, 50], [90, 50], [90, 90], [50, 90]]}),
        "Transform",
    );
    assert_eq!(out["ok"]["sourceBounds"], json!([10, 10, 30, 30]));
    assert_eq!(px(&s, 20, 20), [0; 4], "selected pixels were cut out");
    assert_eq!(px(&s, 5, 5), [0, 65535, 0, 65535]);
    assert_eq!(px(&s, 70, 70), [0, 65535, 0, 65535]);
    let sel = s.document().unwrap().selection.clone().unwrap();
    // 2x bilinear enlargement leaves a 1 px soft fringe; the 50 % contour is the quad itself.
    assert_eq!(sel.bounds(), RectI::new(49, 49, 91, 91), "the selection follows the content");
    assert_eq!((sel.get(60, 60), sel.get(20, 20), sel.get(50, 60), sel.get(49, 60)), (255, 0, 191, 64));
    let o = ok(&mut s, json!({"cmd": "selection.outline"}))["ok"].clone();
    assert_eq!(o["contours"], json!([[[50, 50], [90, 50], [90, 90], [50, 90]]]));
}

#[test]
fn transform_quad_hostile_input() {
    let mut s = doc(64, 64, None);
    let cmd = |q: Value| json!({"cmd": "edit.transform_quad", "quad": q});
    fails(&mut s, cmd(json!([[0, 0], [10, 0], [10, 10], [0, 10]])), "InvalidState"); // empty layer
    fill_rect(&mut s, "#000000", (10, 10, 20, 20));
    fails(&mut s, cmd(json!([[0, 0], [10, 0], [0, 10], [10, 10]])), "InvalidArgument"); // bow-tie
    fails(&mut s, cmd(json!([[0, 0], [10, 0], [3, 3], [0, 10]])), "InvalidArgument"); // concave
    fails(&mut s, cmd(json!([[5, 5], [5, 5], [5, 5], [5, 5]])), "InvalidArgument"); // point
    fails(&mut s, cmd(json!([[0, 0], [10, 0], [10, 10]])), "InvalidArgument"); // 3 corners
    fails(&mut s, cmd(json!([[0, 0], [1e300, 0], [1e300, 10], [0, 10]])), "InvalidArgument");
    fails(
        &mut s,
        json!({"cmd": "edit.transform_quad", "quad": [[0, 0], [9, 0], [9, 9], [0, 9]], "matrix": []}),
        "InvalidArgument",
    );
    fails_typed(&mut s, Command::EditTransformQuad { quad: [[0.0, 0.0], [f64::NAN, 0.0], [9.0, 9.0], [0.0, 9.0]] });
    fails_typed(
        &mut s,
        Command::EditTransformQuad { quad: [[0.0, 0.0], [f64::INFINITY, 0.0], [9.0, 9.0], [0.0, 9.0]] },
    );
    // A huge (valid) quad is clipped to the canvas: no panic, bounded work.
    ok(&mut s, cmd(json!([[-9e6, -9e6], [9e6, -9e6], [9e6, 9e6], [-9e6, 9e6]])));
    assert_eq!(px(&s, 30, 30), [0, 0, 0, 65535]);
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    fails(&mut s, cmd(json!([[0, 0], [10, 0], [10, 10], [0, 10]])), "UnsupportedLayerFeature");
}

// ------------------------------------------------------------------------------------ canvas

/// Background + vector ink layer with a stroke + raster layer with a fill and a mask + selection.
fn rich_doc() -> Session {
    let mut s = doc(128, 96, Some("#FFFFFF"));
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector", "name": "Ink"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#1B2A6B"}));
    stroke(&mut s, &[(20.0, 20.0), (40.0, 30.0), (70.0, 25.0), (100.0, 60.0), (110.0, 80.0)]);
    ok(&mut s, json!({"cmd": "layer.add", "kind": "raster", "name": "Paint"}));
    fill_rect(&mut s, "#FF8000", (30, 40, 90, 70));
    let id = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    ok(&mut s, json!({"cmd": "selection.ellipse", "x0": 30, "y0": 30, "x1": 100, "y1": 80}));
    ok(&mut s, json!({"cmd": "layer.mask_from_selection", "id": id}));
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 24, "y0": 16, "x1": 72, "y1": 64}));
    s
}

fn outline_bounds(s: &Session) -> veskra_core::RectF {
    let d = s.document().unwrap();
    d.iter_strokes().next().unwrap().geometry.as_ref().unwrap().outline.bounds()
}

fn rgba(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

#[test]
fn crop_moves_pixels_masks_selection_and_stroke_geometry_together() {
    let mut s = rich_doc();
    let before = render(&s);
    let ob = outline_bounds(&s);
    let out =
        mutate_checked(&mut s, json!({"cmd": "canvas.crop", "x0": 16, "y0": 8, "x1": 80, "y1": 72}), "Crop canvas");
    assert_eq!(out["ok"], json!({"width": 64, "height": 64, "offsetX": -16, "offsetY": -8}));
    assert_eq!(size(&s), (64, 64));
    let i = info(&mut s);
    assert_eq!((i["width"].as_u64(), i["height"].as_u64()), (Some(64), Some(64)));
    // Every pixel of the new canvas is the old composite at (x+16, y+8).
    let after = render(&s);
    for y in 0..64 {
        for x in 0..64 {
            assert_eq!(rgba(&after, 64, x, y), rgba(&before, 128, x + 16, y + 8), "({x},{y})");
        }
    }
    // Stroke geometry moved by exactly the same offset (vector export still lines up).
    let nb = outline_bounds(&s);
    for (a, b) in [(nb.x0, ob.x0 - 16.0), (nb.y0, ob.y0 - 8.0), (nb.x1, ob.x1 - 16.0), (nb.y1, ob.y1 - 8.0)] {
        assert!((a - b).abs() < 1e-9, "outline offset exactly: {a} vs {b}");
    }
    let d = s.document().unwrap().clone();
    let st = d.iter_strokes().next().unwrap();
    assert_eq!((st.samples()[0].x, st.samples()[0].y), (4.0, 12.0));
    // Selection and layer content are trimmed to the canvas.
    assert_eq!(d.selection.clone().unwrap().bounds(), RectI::new(8, 8, 56, 56));
    for l in d.iter_layers().filter_map(|l| l.pixels()) {
        let cb = l.tiles.content_bounds();
        assert_eq!(cb.intersect(&d.bounds()), cb, "no pixels outside the canvas");
    }
    let layers = ok(&mut s, json!({"cmd": "layer.list"}))["ok"]["layers"].clone();
    let ink = layers.as_array().unwrap().iter().find(|l| l["name"] == "Ink").unwrap();
    assert_eq!(ink["vectorFidelity"], "exact", "cropping keeps vector layers vector-exportable");
    // SVG export uses the new canvas and still writes the (offset) path.
    let path = tmp("crop.svg");
    let rep = ok(&mut s, json!({"cmd": "export.run", "format": "svg", "path": path.to_string_lossy()}));
    assert_eq!((rep["ok"]["widthPx"].as_u64(), rep["ok"]["vectorObjectCount"].as_u64()), (Some(64), Some(1)));
    let svg = std::fs::read_to_string(&path).unwrap();
    assert!(svg.contains("viewBox=\"0 0 64 64\""));
    let s2 = save_reopen(&mut s, "crop.veskra");
    assert_eq!(size(&s2), (64, 64));

    // Cropping beyond the canvas extends it with transparent pixels.
    let before = render(&s);
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "canvas.crop", "x0": -10.4, "y0": -5.6, "x1": 70, "y1": 64}),
        "Crop canvas",
    );
    assert_eq!(out["ok"], json!({"width": 80, "height": 70, "offsetX": 10, "offsetY": 6}));
    let after = render(&s);
    assert_eq!(rgba(&after, 80, 10, 6), rgba(&before, 64, 0, 0));
    assert_eq!(rgba(&after, 80, 73, 69), rgba(&before, 64, 63, 63));
    assert_eq!(rgba(&after, 80, 79, 69), [0; 4]);
    assert_eq!(rgba(&after, 80, 3, 3), [0; 4], "new area is transparent");
}

#[test]
fn resize_extent_respects_the_anchor() {
    for (anchor, dx, dy) in [
        (None, 16, 10),
        (Some("center"), 16, 10),
        (Some("top_left"), 0, 0),
        (Some("top"), 16, 0),
        (Some("top_right"), 33, 0),
        (Some("left"), 0, 10),
        (Some("right"), 33, 10),
        (Some("bottom_left"), 0, 21),
        (Some("bottom"), 16, 21),
        (Some("bottom_right"), 33, 21),
    ] {
        let mut s = rich_doc();
        let before = render(&s);
        let mut cmd = json!({"cmd": "canvas.resize", "width": 161, "height": 117});
        if let Some(a) = anchor {
            cmd["anchor"] = json!(a);
        }
        let out = mutate_checked(&mut s, cmd, "Resize canvas");
        assert_eq!(out["ok"]["offsetX"], dx, "{anchor:?}");
        assert_eq!(out["ok"]["offsetY"], dy, "{anchor:?}");
        assert_eq!(out["ok"]["resampled"], false);
        assert_eq!(size(&s), (161, 117));
        let after = render(&s);
        for (x, y) in [(0u32, 0u32), (50, 50), (127, 95), (64, 30)] {
            assert_eq!(rgba(&after, 161, x + dx as u32, y + dy as u32), rgba(&before, 128, x, y), "{anchor:?}");
        }
    }
    // Shrinking the extent crops around the anchor (odd difference: floor toward the start edge).
    let mut s = rich_doc();
    let before = render(&s);
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "canvas.resize", "width": 101, "height": 96, "anchor": "center"}),
        "Resize canvas",
    );
    assert_eq!((out["ok"]["offsetX"].as_i64(), out["ok"]["offsetY"].as_i64()), (Some(-14), Some(0)));
    assert_eq!(rgba(&render(&s), 101, 0, 40), rgba(&before, 128, 14, 40));
    save_reopen(&mut s, "resize-extent.veskra");
}

#[test]
fn resize_resample_scales_pixels_and_stroke_geometry() {
    let mut s = rich_doc();
    let before = render(&s);
    let ob = outline_bounds(&s);
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "canvas.resize", "width": 256, "height": 192, "resample": true}),
        "Resize canvas",
    );
    assert_eq!(out["ok"]["resampled"], true);
    assert_eq!((out["ok"]["scaleX"].as_f64(), out["ok"]["scaleY"].as_f64()), (Some(2.0), Some(2.0)));
    assert_eq!(size(&s), (256, 192));
    let after = render(&s);
    // Flat areas keep their color exactly; (x, y) → (2x, 2y).
    for (x, y) in [(5u32, 5u32), (60, 55), (120, 90), (2, 90)] {
        assert_eq!(rgba(&after, 256, 2 * x, 2 * y), rgba(&before, 128, x, y), "({x},{y})");
    }
    let nb = outline_bounds(&s);
    for (a, b) in [(nb.x0, ob.x0), (nb.y0, ob.y0), (nb.x1, ob.x1), (nb.y1, ob.y1)] {
        assert!((a - 2.0 * b).abs() < 1e-9, "outline scaled exactly: {a} vs {b}");
    }
    let d = s.document().unwrap().clone();
    // Bilinear enlargement softens the selection edge by 1 px; its 50 % contour scales exactly.
    assert_eq!(d.selection.clone().unwrap().bounds(), RectI::new(47, 31, 145, 129));
    assert_eq!(
        ok(&mut s, json!({"cmd": "selection.outline"}))["ok"]["contours"],
        json!([[[48, 32], [144, 32], [144, 128], [48, 128]]])
    );
    let layers = ok(&mut s, json!({"cmd": "layer.list"}))["ok"]["layers"].clone();
    let ink = layers.as_array().unwrap().iter().find(|l| l["name"] == "Ink").unwrap();
    assert_eq!(ink["vectorFidelity"], "exact");
    // The resampled ink pixels still sit on the scaled outline: the stroke's pixel bounds match
    // the outline bounds to within the brush's soft edge.
    let ink_id = ink["id"].as_str().unwrap().to_string();
    let b = ok(&mut s, json!({"cmd": "layer.bounds", "id": ink_id}))["ok"]["bounds"].clone();
    let b: Vec<f64> = b.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    for (pix, vec) in [(b[0], nb.x0), (b[1], nb.y0), (b[2], nb.x1), (b[3], nb.y1)] {
        assert!((pix - vec).abs() <= 4.0, "pixel bounds {b:?} vs outline {nb:?}");
    }
    save_reopen(&mut s, "resize-resample.veskra");

    // Non-uniform downscale: box filter keeps flat colors, anisotropic geometry scale.
    let before = render(&s);
    let ob = outline_bounds(&s);
    let out = mutate_checked(
        &mut s,
        json!({"cmd": "canvas.resize", "width": 64, "height": 96, "resample": true}),
        "Resize canvas",
    );
    assert_eq!((out["ok"]["scaleX"].as_f64(), out["ok"]["scaleY"].as_f64()), (Some(0.25), Some(0.5)));
    let after = render(&s);
    assert_eq!(rgba(&after, 64, 2, 3), rgba(&before, 256, 8, 6));
    assert_eq!(rgba(&after, 64, 30, 55), rgba(&before, 256, 120, 110));
    let nb = outline_bounds(&s);
    assert!((nb.x0 - 0.25 * ob.x0).abs() < 1e-9 && (nb.y1 - 0.5 * ob.y1).abs() < 1e-9);
}

#[test]
fn canvas_hostile_input() {
    let mut s = rich_doc();
    for cmd in [
        json!({"cmd": "canvas.crop", "x0": 10, "y0": 10, "x1": 10, "y1": 50}), // zero width
        json!({"cmd": "canvas.crop", "x0": 10.2, "y0": 10, "x1": 10.4, "y1": 50}), // rounds to zero width
        json!({"cmd": "canvas.crop", "x0": 500, "y0": 500, "x1": 600, "y1": 600}), // off canvas
        json!({"cmd": "canvas.crop", "x0": -200, "y0": 0, "x1": -100, "y1": 50}),
        json!({"cmd": "canvas.crop", "x0": 0, "y0": 0, "x1": 128, "y1": 96}), // no-op
        json!({"cmd": "canvas.crop", "x0": 0, "y0": 0, "x1": 1e300, "y1": 96}),
        json!({"cmd": "canvas.crop", "x0": 0, "y0": 0, "x1": 64}),
        json!({"cmd": "canvas.crop", "x0": 0, "y0": 0, "x1": 64, "y1": 64, "anchor": "center"}),
        json!({"cmd": "canvas.resize", "width": 0, "height": 10}),
        json!({"cmd": "canvas.resize", "width": -5, "height": 10}),
        json!({"cmd": "canvas.resize", "width": 128, "height": 96}), // no-op
        json!({"cmd": "canvas.resize", "width": 64, "height": 64, "resample": true, "anchor": "top"}),
        json!({"cmd": "canvas.resize", "width": 64, "height": 64, "anchor": "middle"}),
        json!({"cmd": "canvas.resize", "width": 64, "height": 64, "resmaple": true}),
        json!({"cmd": "canvas.resize", "width": 64.5, "height": 64}),
    ] {
        fails(&mut s, cmd, "InvalidArgument");
    }
    for cmd in [
        json!({"cmd": "canvas.crop", "x0": 0, "y0": 0, "x1": 1_000_000, "y1": 96}),
        json!({"cmd": "canvas.crop", "x0": -8000, "y0": -8000, "x1": 8000, "y1": 8000}), // > 8192² px
        json!({"cmd": "canvas.resize", "width": 1_000_000, "height": 1_000_000}),
        json!({"cmd": "canvas.resize", "width": 16385, "height": 1, "resample": true}),
        json!({"cmd": "canvas.resize", "width": 10000, "height": 10000, "resample": true}),
    ] {
        fails(&mut s, cmd, "AllocationLimit");
    }
    fails_typed(&mut s, Command::CanvasCrop { x0: f64::NAN, y0: 0.0, x1: 10.0, y1: 10.0 });
    fails_typed(&mut s, Command::CanvasCrop { x0: 0.0, y0: 0.0, x1: f64::INFINITY, y1: 10.0 });
    fails_typed(&mut s, Command::CanvasResize { width: u32::MAX, height: u32::MAX, resample: true, anchor: None });
    // Refused while a stroke is live.
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    let e = err(&mut s, json!({"cmd": "canvas.resize", "width": 64, "height": 64}));
    assert_eq!(e["code"], "InvalidState");
    ok(&mut s, json!({"cmd": "stroke.cancel"}));
    // Maximum edge is accepted when the pixel budget allows it.
    let mut s = doc(8, 8, None);
    ok(&mut s, json!({"cmd": "canvas.resize", "width": 16384, "height": 2}));
    assert_eq!(size(&s), (16384, 2));
}

#[test]
fn canvas_change_refits_the_view_on_command_undo_and_redo() {
    let mut s = rich_doc();
    ok(&mut s, json!({"cmd": "view.size", "width": 800, "height": 600}));
    let zoom = |s: &mut Session| ok(s, json!({"cmd": "view.set"}))["ok"]["zoom"].as_f64().unwrap();
    let z0 = zoom(&mut s);
    let out = ok(&mut s, json!({"cmd": "canvas.resize", "width": 256, "height": 192, "resample": true}));
    assert_eq!(out["viewChanged"], true);
    assert_eq!(out["invalidated"], json!([[0, 0, 256, 192]]));
    let z1 = zoom(&mut s);
    assert!((z1 - z0 / 2.0).abs() < 1e-9, "fit zoom halves when the canvas doubles: {z0} -> {z1}");
    let u = ok(&mut s, json!({"cmd": "history.undo"}));
    assert_eq!(u["viewChanged"], true);
    assert_eq!(u["invalidated"], json!([[0, 0, 256, 192]]), "undo invalidates the larger of both canvases");
    assert!((zoom(&mut s) - z0).abs() < 1e-9);
    let r = ok(&mut s, json!({"cmd": "history.redo"}));
    assert_eq!(r["viewChanged"], true);
    // Ordinary undo steps do not touch the view.
    ok(&mut s, json!({"cmd": "edit.adjust", "kind": "invert"}));
    assert_eq!(ok(&mut s, json!({"cmd": "history.undo"}))["viewChanged"], false);
}

// ------------------------------------------------------------------------------------ queries

#[test]
fn selection_outline_is_read_only_and_bounded() {
    let mut s = doc(256, 200, None);
    let v0 = s.version();
    let none = ok(&mut s, json!({"cmd": "selection.outline"}));
    assert_eq!(none["ok"], json!({"empty": true, "bounds": null, "contours": [], "truncated": false, "pointCount": 0}));
    assert_eq!(none["historyChanged"], false);
    ok(&mut s, json!({"cmd": "selection.rect", "x0": 10, "y0": 20, "x1": 100, "y1": 60}));
    let before = s.document().unwrap().clone();
    let (v1, undo) = (s.version(), info(&mut s)["undoLabel"].clone());
    let o = ok(&mut s, json!({"cmd": "selection.outline"}));
    assert_eq!(
        o["ok"],
        json!({"empty": false, "bounds": [10, 20, 100, 60], "truncated": false, "pointCount": 4,
               "contours": [[[10, 20], [100, 20], [100, 60], [10, 60]]]})
    );
    assert_eq!((o["historyChanged"].clone(), o["invalidated"].clone()), (json!(false), json!([])));
    // Inverted selection: the canvas border plus the hole.
    ok(&mut s, json!({"cmd": "selection.invert"}));
    let o = ok(&mut s, json!({"cmd": "selection.outline"}))["ok"].clone();
    assert_eq!(o["bounds"], json!([0, 0, 256, 200]));
    assert_eq!(
        o["contours"],
        json!([[[0, 0], [256, 0], [256, 200], [0, 200]], [[10, 20], [10, 60], [100, 60], [100, 20]]])
    );
    ok(&mut s, json!({"cmd": "selection.invert"}));
    // Ellipse: exact trace has many corners; maxPoints simplifies and reports it.
    ok(&mut s, json!({"cmd": "selection.ellipse", "x0": 8, "y0": 8, "x1": 248, "y1": 192}));
    let full = ok(&mut s, json!({"cmd": "selection.outline", "maxPoints": 100000}))["ok"].clone();
    assert_eq!(full["truncated"], false);
    assert!(full["pointCount"].as_u64().unwrap() > 400);
    let small = ok(&mut s, json!({"cmd": "selection.outline", "maxPoints": 64}))["ok"].clone();
    assert_eq!(small["truncated"], true);
    let n = small["pointCount"].as_u64().unwrap();
    assert!((8..=64).contains(&n), "{n}");
    assert_eq!(small["contours"][0].as_array().unwrap().len() as u64, n);
    assert_eq!(small["bounds"], json!([8, 8, 248, 192]));
    // Lasso and wand selections trace too.
    ok(&mut s, json!({"cmd": "selection.lasso", "points": [[20, 20], [200, 40], [120, 180]]}));
    let o = ok(&mut s, json!({"cmd": "selection.outline"}))["ok"].clone();
    assert_eq!(o["contours"].as_array().unwrap().len(), 1);
    assert!(o["pointCount"].as_u64().unwrap() <= 4000);
    fill_rect(&mut s, "#FF00FF", (30, 30, 60, 50));
    ok(&mut s, json!({"cmd": "selection.wand", "x": 40, "y": 40}));
    let o = ok(&mut s, json!({"cmd": "selection.outline"}))["ok"].clone();
    assert_eq!(o["contours"], json!([[[30, 30], [60, 30], [60, 50], [30, 50]]]));
    // Never an undo step; bad arguments are typed errors.
    let (v2, undo2) = (s.version(), info(&mut s)["undoLabel"].clone());
    ok(&mut s, json!({"cmd": "selection.outline"}));
    assert_eq!((s.version(), info(&mut s)["undoLabel"].clone()), (v2, undo2));
    assert!(v1 > v0 && undo == "Selection");
    let _ = before;
    fails(&mut s, json!({"cmd": "selection.outline", "maxPoints": 3}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.outline", "maxPoints": 1_000_000}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.outline", "maxPoints": -1}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "selection.outline", "maxPts": 10}), "InvalidArgument");
    assert_eq!(err(&mut session(), json!({"cmd": "selection.outline"}))["code"], "InvalidState");
}

#[test]
fn layer_bounds_reports_tight_content() {
    let mut s = doc(128, 128, None);
    let v = s.version();
    let active = info(&mut s)["activeLayer"].as_str().unwrap().to_string();
    assert_eq!(ok(&mut s, json!({"cmd": "layer.bounds"}))["ok"], json!({"layerId": active, "bounds": null}));
    assert_eq!(s.version(), v, "read-only");
    fill_rect(&mut s, "#123456", (12, 34, 56, 78));
    assert_eq!(ok(&mut s, json!({"cmd": "layer.bounds"}))["ok"]["bounds"], json!([12, 34, 56, 78]));
    assert_eq!(ok(&mut s, json!({"cmd": "layer.bounds", "id": active}))["ok"]["bounds"], json!([12, 34, 56, 78]));
    // Strokes are rendered into the layer's pixels, so a vector layer's bounds cover its strokes.
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    stroke(&mut s, &[(80.0, 20.0), (110.0, 40.0)]);
    let b = ok(&mut s, json!({"cmd": "layer.bounds"}))["ok"]["bounds"].clone();
    let b: Vec<i64> = b.as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    assert!(b[0] <= 80 && b[0] >= 70 && b[2] >= 110 && b[2] <= 120 && b[1] <= 20 && b[3] >= 40, "{b:?}");
    // Groups report the union of their children.
    let g = ok(&mut s, json!({"cmd": "layer.add", "kind": "group"}))["ok"]["layerId"].as_str().unwrap().to_string();
    assert_eq!(ok(&mut s, json!({"cmd": "layer.bounds", "id": g}))["ok"]["bounds"], Value::Null);
    ok(&mut s, json!({"cmd": "layer.move", "id": active, "placement": {"at": "into", "group": g}}));
    assert_eq!(ok(&mut s, json!({"cmd": "layer.bounds", "id": g}))["ok"]["bounds"], json!([12, 34, 56, 78]));
    let ver = s.version();
    ok(&mut s, json!({"cmd": "layer.bounds"}));
    assert_eq!(s.version(), ver);
    fails(&mut s, json!({"cmd": "layer.bounds", "id": "nope"}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "layer.bounds", "id": "00000000-0000-0000-0000-000000000000"}), "NotFound");
    fails(&mut s, json!({"cmd": "layer.bounds", "layer": active}), "InvalidArgument");
}

#[test]
fn input_config_lock_view_blocks_navigation_but_not_drawing() {
    use veskra_core::ToolType;
    use veskra_engine::veskra_input::{PointerAction as A, RawPointerEvent};
    let mut s = doc(256, 256, Some("#FFFFFF"));
    ok(&mut s, json!({"cmd": "view.size", "width": 512, "height": 512}));
    let cfg = ok(&mut s, json!({"cmd": "input.config"}))["ok"].clone();
    assert_eq!(cfg, json!({"fingerDraws": true, "stylusButton": "none", "lockView": false}));
    let cfg = ok(&mut s, json!({"cmd": "input.config", "lockView": true, "fingerDraws": false}))["ok"].clone();
    assert_eq!(cfg, json!({"fingerDraws": false, "stylusButton": "none", "lockView": true}));
    let ms = 1_000_000u64;
    let pinch = |t0: u64| -> Vec<RawPointerEvent> {
        let f = |a, id, x: f32, y: f32, t: u64| RawPointerEvent::new(a, id, ToolType::Finger, x, y, t0 + t * ms);
        let mut v = vec![f(A::Down, 0, 200.0, 250.0, 0), f(A::PointerDown, 1, 300.0, 250.0, 8)];
        for k in 1..=20u64 {
            let d = 4.0 * k as f32;
            v.push(f(A::Move, 0, 200.0 - d, 250.0 + d, 8 + 8 * k));
            v.push(f(A::Move, 1, 300.0 + d, 250.0 + 2.0 * d, 8 + 8 * k));
        }
        v.push(f(A::PointerUp, 1, 380.0, 410.0, 200));
        v.push(f(A::Up, 0, 120.0, 330.0, 208));
        v
    };
    let view0 = s.view();
    let out = s.input_batch(&pinch(0));
    assert_eq!(out.flags & veskra_engine::flags::VIEW, 0, "locked: no view change reported");
    assert_eq!(s.view(), view0);
    // The stylus still draws while the view is locked.
    let pen = |a, x: f32, y: f32, t: u64| {
        let mut e = RawPointerEvent::new(a, 5, ToolType::Stylus, x, y, 1000 * ms + t * ms);
        e.pressure = Some(0.8);
        e
    };
    let before = render(&s);
    let mut evs = vec![pen(A::Down, 100.0, 100.0, 0)];
    evs.extend((1..=20u64).map(|k| pen(A::Move, 100.0 + 8.0 * k as f32, 100.0 + 4.0 * k as f32, 4 * k)));
    evs.push(pen(A::Up, 260.0, 180.0, 90));
    let out = s.input_batch(&evs);
    assert_ne!(out.flags & veskra_engine::flags::COMMITTED, 0, "stroke committed");
    assert_ne!(render(&s), before);
    assert_eq!(s.view(), view0);
    // Unlocking restores navigation.
    assert_eq!(ok(&mut s, json!({"cmd": "input.config", "lockView": false}))["ok"]["lockView"], false);
    let out = s.input_batch(&pinch(5000 * ms));
    assert_ne!(out.flags & veskra_engine::flags::VIEW, 0);
    assert_ne!(s.view(), view0);
    fails(&mut s, json!({"cmd": "input.config", "lockView": "yes"}), "InvalidArgument");
    fails(&mut s, json!({"cmd": "input.config", "lockVeiw": true}), "InvalidArgument");
}

#[test]
fn receipts_expose_typed_results() {
    // Typed API parity with the JSON boundary (the Kotlin kit uses JSON; Rust callers use `Command`).
    let mut s = doc(32, 32, None);
    let r: Receipt = s
        .execute(Command::EditGradient {
            x0: 0.0,
            y0: 0.0,
            x1: 32.0,
            y1: 32.0,
            kind: GradientKindSpec::Radial,
            stops: None,
            opacity: 1.0,
        })
        .unwrap();
    assert!(r.history_changed && !r.view_changed);
    assert_eq!(r.invalidated, vec![RectI::new(0, 0, 32, 32)]);
    let j = serde_json::to_value(Command::CanvasResize { width: 1, height: 2, resample: false, anchor: None }).unwrap();
    assert_eq!(j, json!({"cmd": "canvas.resize", "width": 1, "height": 2, "resample": false, "anchor": null}));
}
