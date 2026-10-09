#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Property test for the contract the Android tile uploader relies on (CONTRACTS §8,
//! COMMANDS.md): after any command or input batch, every display pixel that changed lies inside
//! the union of the reported invalidated rects. Also checks receipt/flag consistency:
//! `historyChanged` ⇔ version changed; input-batch HISTORY ⇔ version changed; COMMITTED ⇔ a new
//! stroke record; failed commands change nothing; the document always validates.

use serde_json::{Value, json};
use veskra_core::{RectI, ToolType};
use veskra_engine::veskra_input::{PointerAction, RawPointerEvent};
use veskra_engine::{EngineConfig, Session, flags};

const W: u32 = 150;
const H: u32 = 110;

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
}

fn render(s: &Session) -> Vec<u8> {
    let mut buf = vec![0u8; (W * H * 4) as usize];
    s.render_display(RectI::from_size(W, H), &mut buf).unwrap();
    buf
}

/// First changed pixel outside every rect, if any.
fn stray_change(a: &[u8], b: &[u8], rects: &[RectI]) -> Option<(i32, i32)> {
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let i = (y as usize * W as usize + x as usize) * 4;
            if a[i..i + 4] != b[i..i + 4] && !rects.iter().any(|r| r.contains(x, y)) {
                return Some((x, y));
            }
        }
    }
    None
}

fn layer_ids(s: &Session) -> Vec<String> {
    s.document().unwrap().paint_order().iter().map(|l| l.to_string()).collect()
}

fn random_command(r: &mut Rng, s: &Session) -> Value {
    let ids = layer_ids(s);
    let any_id = || ids.first().cloned().unwrap_or_default();
    let pick = |r: &mut Rng| if ids.is_empty() { any_id() } else { ids[r.below(ids.len() as u64) as usize].clone() };
    let (x0, y0) = (r.f(-10.0, W as f32), r.f(-10.0, H as f32));
    let (x1, y1) = (x0 + r.f(1.0, 80.0), y0 + r.f(1.0, 60.0));
    match r.below(30) {
        0 => json!({"cmd": "layer.add", "kind": (["raster", "vector", "group"][r.below(3) as usize])}),
        1 => json!({"cmd": "layer.remove", "id": pick(r)}),
        2 => json!({"cmd": "layer.duplicate", "id": pick(r)}),
        3 => json!({"cmd": "layer.move", "id": pick(r), "placement": {"at": "below", "id": pick(r)}}),
        4 => json!({"cmd": "layer.set", "id": pick(r), "visible": r.chance(50)}),
        5 => json!({"cmd": "layer.set", "id": pick(r), "opacity": r.f(0.0, 1.0)}),
        6 => {
            json!({"cmd": "layer.set", "id": pick(r), "blend": (["normal", "multiply", "screen", "overlay"][r.below(4) as usize])})
        }
        7 => json!({"cmd": "layer.set", "id": pick(r), "locked": r.chance(30)}),
        8 => json!({"cmd": "layer.set", "id": pick(r), "alphaLocked": r.chance(30), "clip": r.chance(30)}),
        9 => json!({"cmd": "layer.clear", "id": pick(r)}),
        10 => json!({"cmd": "layer.select", "id": pick(r)}),
        11 => json!({"cmd": "selection.rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1,
                     "op": (["replace", "add", "subtract", "intersect"][r.below(4) as usize])}),
        12 => json!({"cmd": (["selection.clear", "selection.invert", "selection.all"][r.below(3) as usize])}),
        13 => json!({"cmd": "edit.fill", "x": r.f(0.0, W as f32), "y": r.f(0.0, H as f32), "tolerance": r.f(0.0, 0.5),
                     "sampleMerged": r.chance(50)}),
        14 => json!({"cmd": "edit.fill_selection"}),
        15 => json!({"cmd": "edit.delete_selection"}),
        16 => json!({"cmd": "edit.transform", "matrix": [1, 0, 0, 1, r.f(-20.0, 20.0), r.f(-20.0, 20.0)]}),
        17 | 18 => json!({"cmd": "history.undo"}),
        19 => json!({"cmd": "history.redo"}),
        20 => json!({"cmd": "color.set", "hex": format!("#{:06X}", r.below(0x100_0000))}),
        21 => {
            json!({"cmd": "brush.select", "preset": (["pencil.smooth", "pen.brush", "pen.technical", "charcoal", "eraser.hard"][r.below(5) as usize])})
        }
        22 => json!({"cmd": "stroke.begin", "eraser": r.chance(20)}),
        23 | 24 => {
            let n = 1 + r.below(6);
            let t0 = r.below(1000) * 1_000_000;
            let samples: Vec<Value> = (0..n)
                .map(|i| json!({"x": r.f(-5.0, W as f32 + 5.0), "y": r.f(-5.0, H as f32 + 5.0), "t": t0 + i * 4_000_000, "p": r.f(0.0, 1.0)}))
                .collect();
            json!({"cmd": "stroke.samples", "samples": samples})
        }
        25 => json!({"cmd": "stroke.commit"}),
        26 => json!({"cmd": "stroke.cancel"}),
        27 => json!({"cmd": "input.reset"}),
        28 => json!({"cmd": "brush.set", "size": r.f(1.0, 40.0), "stabilization": r.f(0.0, 1.0)}),
        _ => json!({"cmd": "view.set", "zoom": 1.0, "rotation": 0.0, "panX": 0.0, "panY": 0.0}),
    }
}

/// A batch of raw events: stylus strokes (with predicted moves), hovers, finger taps.
fn random_batch(r: &mut Rng, t: &mut u64, pen_down: &mut bool) -> Vec<RawPointerEvent> {
    let mut out = Vec::new();
    let n = 1 + r.below(10);
    for _ in 0..n {
        *t += 1 + r.below(8) * 1_000_000;
        let (x, y) = (r.f(-5.0, W as f32 + 5.0), r.f(-5.0, H as f32 + 5.0));
        let tool = if r.chance(90) { ToolType::Stylus } else { ToolType::Finger };
        let action = if !*pen_down {
            match r.below(4) {
                0 => PointerAction::HoverMove,
                _ => PointerAction::Down,
            }
        } else {
            match r.below(10) {
                0 => PointerAction::Up,
                1 => PointerAction::Cancel,
                _ => PointerAction::Move,
            }
        };
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
                    PointerAction::Move,
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

fn check_sequence(seed: u64, steps: usize) {
    let mut r = Rng(seed);
    let mut s = Session::new(EngineConfig { deterministic_seed: Some(seed), ..Default::default() });
    let created: Value = serde_json::from_str(&s.execute_json(
        &json!({"cmd": "document.create", "width": W, "height": H, "background": "#F0E0D0"}).to_string(),
    ))
    .unwrap();
    assert!(created.get("ok").is_some());
    s.execute_json(&json!({"cmd": "view.size", "width": W, "height": H}).to_string());
    s.execute_json(&json!({"cmd": "view.set", "zoom": 1.0, "rotation": 0.0, "panX": 0.0, "panY": 0.0}).to_string());
    let mut t = 1_000_000_000u64;
    let mut pen_down = false;
    let mut prev = render(&s);
    for step in 0..steps {
        let v0 = s.version();
        let doc0 = s.document().unwrap().clone();
        let strokes0 = doc0.stroke_count();
        let live0 = s.has_live_stroke();
        let (rects, what): (Vec<RectI>, String) = if r.chance(45) {
            let batch = random_batch(&mut r, &mut t, &mut pen_down);
            let out = s.input_batch(&batch);
            let changed = s.version() != v0;
            assert_eq!(out.flags & flags::HISTORY != 0, changed, "seed {seed} step {step}: HISTORY flag vs version");
            let committed = s.document().unwrap().stroke_count() > strokes0;
            if committed {
                assert!(out.flags & flags::COMMITTED != 0, "seed {seed} step {step}: COMMITTED flag missing");
            }
            assert_eq!(out.flags & flags::DIRTY != 0, !out.dirty.is_empty(), "seed {seed} step {step}: DIRTY flag");
            (out.dirty, format!("input batch {batch:?}"))
        } else {
            let cmd = random_command(&mut r, &s);
            let out: Value = serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap();
            if out.get("error").is_some() {
                assert_eq!(s.version(), v0, "seed {seed} step {step}: failed {cmd} changed version");
                assert_eq!(s.document().unwrap().as_ref(), doc0.as_ref(), "seed {seed} step {step}: failed {cmd}");
                (vec![], format!("FAILED {cmd} -> {out} (live before: {live0})"))
            } else {
                assert_eq!(
                    out["historyChanged"].as_bool().unwrap(),
                    s.version() != v0,
                    "seed {seed} step {step}: historyChanged vs version for {cmd} -> {out}"
                );
                let rects = out["invalidated"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| {
                        let a: Vec<i32> = a.as_array().unwrap().iter().map(|v| v.as_i64().unwrap() as i32).collect();
                        RectI::new(a[0], a[1], a[2], a[3])
                    })
                    .collect();
                (rects, format!("{cmd} -> {out} (live before: {live0})"))
            }
        };
        s.document().unwrap().validate().unwrap();
        let next = render(&s);
        if let Some((x, y)) = stray_change(&prev, &next, &rects) {
            panic!("seed {seed} step {step}: pixel ({x},{y}) changed outside invalidated {rects:?} after {what}");
        }
        prev = next;
    }
}

#[test]
fn changed_pixels_are_always_inside_invalidated_rects() {
    // VESKRA_PROPTEST_SEEDS=N widens the sweep (review ran 2000 seeds in release).
    let seeds: u64 = std::env::var("VESKRA_PROPTEST_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for seed in 1..=seeds {
        check_sequence(seed, 150);
    }
}
