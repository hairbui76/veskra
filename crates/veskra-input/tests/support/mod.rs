//! Shared test support: fixed-seed PRNG, deterministic SYNTHETIC trace generators for
//! `tests/fixtures/input/*.jsonl`, replay helpers and an action-stream invariant checker.
//!
//! Generators use only IEEE basic arithmetic and `sqrt` (correctly rounded everywhere), so
//! the produced fixture text is bit-identical across platforms.
#![allow(dead_code, clippy::unwrap_used)]

use std::path::PathBuf;
use veskra_core::{SampleFlags, StrokeSample, ToolType};
use veskra_input::{
    InputAction, InputArbiter, MAX_ZOOM, MIN_ZOOM, PointerAction as A, RawPointerEvent, ViewTransform, trace,
};

/// SplitMix64 — tiny deterministic PRNG.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }
}

pub fn ev(action: A, id: i32, tool: ToolType, x: f32, y: f32, t: u64) -> RawPointerEvent {
    RawPointerEvent::new(action, id, tool, x, y, t)
}

pub fn pen(action: A, x: f32, y: f32, t: u64, p: f32) -> RawPointerEvent {
    let mut e = ev(action, 0, ToolType::Stylus, x, y, t);
    e.pressure = Some(p);
    e
}

pub fn finger(action: A, id: i32, x: f32, y: f32, t: u64) -> RawPointerEvent {
    let mut e = ev(action, id, ToolType::Finger, x, y, t);
    e.pressure = Some(1.0);
    e.touch_major = Some(40.0);
    e.touch_minor = Some(35.0);
    e
}

/// Quantize to 1/64 px so fixture text stays short.
fn q(v: f64) -> f32 {
    ((v * 64.0).round() / 64.0) as f32
}

/// Triangle wave in `[0,1]` with period 1.
fn tri(v: f64) -> f64 {
    1.0 - (2.0 * (v - v.floor()) - 1.0).abs()
}

const MS: u64 = 1_000_000;
/// 240 Hz digitizer period.
const DT: u64 = 4_166_667;

struct StrokeGen {
    x0: f64,
    y0: f64,
    tool: ToolType,
    frames: u32,
    axes: bool,
    up_duplicate: bool,
}

fn gen_stroke(out: &mut Vec<RawPointerEvent>, t: &mut u64, rng: &mut Rng, g: &StrokeGen) {
    let sample = |i: u32, rng: &mut Rng| {
        let u = f64::from(i);
        let x = g.x0 + 3.0 * u + rng.range(0.0, 0.5);
        let y = g.y0 + 120.0 * tri(u / 80.0) + rng.range(0.0, 0.5);
        let p = 0.15 + 0.7 * tri(u / 150.0) + rng.range(0.0, 0.01);
        let tilt = 0.2 + 0.6 * tri(u / 97.0);
        let orient = -2.5 + 5.0 * tri(u / 211.0);
        (q(x), q(y), (p * 1024.0).round() as f32 / 1024.0, q(tilt), q(orient))
    };
    let mk = |action: A, s: (f32, f32, f32, f32, f32), t: u64| {
        let mut e = ev(action, 0, g.tool, s.0, s.1, t);
        e.pressure = Some(s.2);
        if g.axes {
            e.tilt = Some(s.3);
            e.orientation = Some(s.4);
        }
        e
    };
    out.push(mk(A::Down, sample(0, rng), *t));
    let mut i = 0;
    let mut last = sample(0, rng);
    for _ in 0..g.frames {
        // 4 digitizer samples per 60 Hz frame: 3 historical + 1 current.
        for k in 1..=4 {
            i += 1;
            *t += DT;
            last = sample(i, rng);
            let mut e = mk(A::Move, last, *t);
            e.historical = k < 4;
            out.push(e);
        }
        // Two motion predictions after the current sample (presentation only).
        for k in 1..=2u32 {
            let mut e = mk(A::Move, sample(i + k, rng), *t + u64::from(k) * DT);
            e.predicted = true;
            out.push(e);
        }
    }
    if g.up_duplicate {
        *t += DT / 2;
        out.push(mk(A::Up, last, *t));
    } else {
        *t += DT;
        out.push(mk(A::Up, sample(i + 1, rng), *t));
    }
}

fn hover(out: &mut Vec<RawPointerEvent>, t: &mut u64, x: f64, y: f64, n: u32) {
    let mut e = ev(A::HoverEnter, 0, ToolType::Stylus, q(x), q(y), *t);
    e.pressure = Some(0.0);
    out.push(e);
    for i in 1..=n {
        *t += DT;
        let mut e = ev(A::HoverMove, 0, ToolType::Stylus, q(x + 2.0 * f64::from(i)), q(y + f64::from(i)), *t);
        e.pressure = Some(0.0);
        e.tilt = Some(0.5);
        out.push(e);
    }
    *t += DT;
    out.push(ev(A::HoverExit, 0, ToolType::Stylus, q(x + 2.0 * f64::from(n)), q(y + f64::from(n)), *t));
    *t += DT;
}

/// ≥1000 stylus events: hover, a stylus stroke with tilt/orientation (UP duplicates last
/// position), an eraser stroke without tilt/orientation (UP at a new position). Historical
/// batches of 3 + current, plus 2 predicted events per frame.
pub fn stylus_confirmed() -> Vec<RawPointerEvent> {
    let mut out = Vec::new();
    let mut t = 1_000 * MS;
    let mut rng = Rng::new(0x5EED_0001);
    hover(&mut out, &mut t, 280.0, 380.0, 10);
    let a = StrokeGen { x0: 300.0, y0: 400.0, tool: ToolType::Stylus, frames: 150, axes: true, up_duplicate: true };
    gen_stroke(&mut out, &mut t, &mut rng, &a);
    t += 300 * MS;
    hover(&mut out, &mut t, 280.0, 900.0, 5);
    let b =
        StrokeGen { x0: 300.0, y0: 900.0, tool: ToolType::StylusEraser, frames: 100, axes: false, up_duplicate: false };
    gen_stroke(&mut out, &mut t, &mut rng, &b);
    out
}

/// Palm / cancel scenarios (expected outcomes documented in the README).
pub fn stylus_cancel_and_palm() -> Vec<RawPointerEvent> {
    let mut out = Vec::new();
    let mut t = 2_000 * MS;
    let step = |t: &mut u64| {
        *t += DT;
        *t
    };
    // 1. Palm-sized finger contact before any stylus: never draws.
    let mut palm = finger(A::Down, 0, 500.0, 600.0, t);
    palm.touch_major = Some(180.0);
    palm.touch_minor = Some(140.0);
    out.push(palm);
    for i in 1..=10 {
        let mut e = palm;
        e.action = A::Move;
        e.x += 2.0 * i as f32;
        e.t_ns = step(&mut t);
        out.push(e);
    }
    let mut e = palm;
    e.action = A::Up;
    e.x += 20.0;
    e.t_ns = step(&mut t);
    out.push(e);
    t += 200 * MS;
    // 2. Finger stroke whose UP carries FLAG_CANCELED (Android 13 palm cancellation).
    out.push(finger(A::Down, 0, 700.0, 300.0, t));
    for i in 1..=10 {
        out.push(finger(A::Move, 0, 700.0 + 4.0 * i as f32, 300.0, step(&mut t)));
    }
    let mut e = finger(A::Up, 0, 744.0, 300.0, step(&mut t));
    e.canceled_flag = true;
    out.push(e);
    t += 200 * MS;
    // 3. Finger stroke whose contact grows to palm size: cancelled.
    out.push(finger(A::Down, 0, 700.0, 500.0, t));
    for i in 1..=10 {
        let mut e = finger(A::Move, 0, 700.0 + 4.0 * i as f32, 500.0, step(&mut t));
        if i > 5 {
            e.touch_major = Some(150.0);
        }
        out.push(e);
    }
    let mut e = finger(A::Up, 0, 744.0, 500.0, step(&mut t));
    e.touch_major = Some(150.0);
    out.push(e);
    t += 200 * MS;
    // 4. Stylus stroke A: hover then a normal stroke (commit).
    hover(&mut out, &mut t, 300.0, 700.0, 4);
    out.push(pen(A::Down, 310.0, 705.0, t, 0.3));
    for i in 1..=30 {
        out.push(pen(A::Move, 310.0 + 3.0 * i as f32, 705.0 + i as f32, step(&mut t), 0.3 + 0.01 * i as f32));
    }
    out.push(pen(A::Up, 400.0, 735.0, step(&mut t), 0.6));
    t += 200 * MS;
    // 5. Stylus stroke B: ACTION_CANCEL midway (cancel).
    out.push(pen(A::Down, 310.0, 805.0, t, 0.4));
    for i in 1..=30 {
        out.push(pen(A::Move, 310.0 + 3.0 * i as f32, 805.0, step(&mut t), 0.4));
    }
    out.push(pen(A::Cancel, 400.0, 805.0, step(&mut t), 0.4));
    t += 200 * MS;
    // 6. Stylus stroke C with a resting palm beside it; palm lifted with FLAG_CANCELED.
    out.push(pen(A::Down, 310.0, 905.0, t, 0.5));
    for i in 1..=10 {
        out.push(pen(A::Move, 310.0 + 3.0 * i as f32, 905.0, step(&mut t), 0.5));
    }
    let mut p = finger(A::PointerDown, 1, 480.0, 1105.0, step(&mut t));
    p.touch_major = Some(200.0);
    p.touch_minor = Some(150.0);
    out.push(p);
    for i in 11..=30 {
        let tt = step(&mut t);
        out.push(pen(A::Move, 310.0 + 3.0 * i as f32, 905.0, tt, 0.5));
        let mut m = p;
        m.action = A::Move;
        m.x += 5.0 * (i - 10) as f32;
        m.t_ns = tt;
        out.push(m);
    }
    let mut pu = p;
    pu.action = A::PointerUp;
    pu.x += 100.0;
    pu.t_ns = step(&mut t);
    pu.canceled_flag = true;
    out.push(pu);
    for i in 31..=40 {
        out.push(pen(A::Move, 310.0 + 3.0 * i as f32, 905.0, step(&mut t), 0.5));
    }
    out.push(pen(A::Up, 430.0, 905.0, step(&mut t), 0.5));
    t += 200 * MS;
    // 7. Normal finger after a stylus was seen: navigates (pan), never draws.
    out.push(finger(A::Down, 0, 1000.0, 400.0, t));
    for i in 1..=5 {
        out.push(finger(A::Move, 0, 1000.0 + 30.0 * i as f32, 400.0, step(&mut t)));
    }
    out.push(finger(A::Up, 0, 1150.0, 400.0, step(&mut t)));
    out
}

/// Pressure ramp, out-of-range clamps, unreported pressure and a mouse stroke.
pub fn pen_pressure_curve() -> Vec<RawPointerEvent> {
    let mut out = Vec::new();
    let mut t = 3_000 * MS;
    // 1. Ramp 0.00 .. 1.00 in 0.01 steps (UP duplicates the last position).
    out.push(pen(A::Down, 100.0, 100.0, t, 0.0));
    for i in 1..=100u16 {
        t += DT;
        out.push(pen(A::Move, 100.0 + 2.0 * f32::from(i), 100.0, t, f32::from(i) / 100.0));
    }
    t += DT;
    out.push(pen(A::Up, 300.0, 100.0, t, 1.0));
    t += 200 * MS;
    // 2. Out-of-range hardware values (clamped and counted).
    for (i, p) in [-0.25f32, 0.0, 0.5, 1.0, 1.25, 1.0].into_iter().enumerate() {
        let a = if i == 0 { A::Down } else { A::Move };
        out.push(pen(a, 100.0 + 10.0 * i as f32, 200.0, t, p));
        t += DT;
    }
    out.push(pen(A::Up, 150.0, 200.0, t, 1.0));
    t += 200 * MS;
    // 3. Stylus that does not report pressure (NaN in the packed layout).
    for i in 0..20u16 {
        let a = if i == 0 { A::Down } else { A::Move };
        out.push(ev(a, 0, ToolType::Stylus, 100.0 + 5.0 * f32::from(i), 300.0, t));
        t += DT;
    }
    out.push(ev(A::Up, 0, ToolType::Stylus, 200.0, 300.0, t));
    t += 200 * MS;
    // 4. Mouse (reports 1.0, which is not a pen force): fallback pressure.
    for i in 0..20u16 {
        let a = if i == 0 { A::Down } else { A::Move };
        let mut e = ev(a, 0, ToolType::Mouse, 100.0 + 5.0 * f32::from(i), 400.0, t);
        e.pressure = Some(1.0);
        e.buttons = 1;
        out.push(e);
        t += DT;
    }
    let mut e = ev(A::Up, 0, ToolType::Mouse, 200.0, 400.0, t);
    e.pressure = Some(0.0);
    out.push(e);
    out
}

/// Two-finger scenarios: A pinch-zoom(2x)+rotate(~30°)+pan; B two-finger tap; C three-finger
/// tap; D slow two-finger hold (no tap); E two-finger pan.
pub fn two_finger_gesture() -> Vec<RawPointerEvent> {
    let mut out = Vec::new();
    let mut t = 4_000 * MS;
    // A. Pinch: v_k = R^k (100,0) (1 + k/60), centroid drifts (+50, +40).
    let (s, c) = (0.008_726_5f64, (1.0 - 0.008_726_5f64 * 0.008_726_5).sqrt());
    let (mut rx, mut ry) = (1.0f64, 0.0f64);
    let pos = |k: u32, rx: f64, ry: f64| {
        let f = 1.0 + f64::from(k) / 60.0;
        let (vx, vy) = (100.0 * rx * f, 100.0 * ry * f);
        let (cx, cy) = (900.0 + 50.0 * f64::from(k) / 60.0, 600.0 + 40.0 * f64::from(k) / 60.0);
        ((q(cx - vx), q(cy - vy)), (q(cx + vx), q(cy + vy)))
    };
    let (p0, q0) = pos(0, rx, ry);
    out.push(finger(A::Down, 0, p0.0, p0.1, t));
    t += 16 * MS;
    out.push(finger(A::PointerDown, 1, q0.0, q0.1, t));
    let mut last = (p0, q0);
    for k in 1..=60 {
        (rx, ry) = (rx * c - ry * s, rx * s + ry * c);
        t += 8 * MS;
        last = pos(k, rx, ry);
        out.push(finger(A::Move, 0, last.0.0, last.0.1, t));
        out.push(finger(A::Move, 1, last.1.0, last.1.1, t));
    }
    t += 8 * MS;
    out.push(finger(A::PointerUp, 1, last.1.0, last.1.1, t));
    t += 8 * MS;
    out.push(finger(A::Up, 0, last.0.0, last.0.1, t));
    t += 500 * MS;
    // B. Two-finger tap.
    out.push(finger(A::Down, 0, 500.0, 500.0, t));
    out.push(finger(A::PointerDown, 1, 620.0, 505.0, t + 20 * MS));
    out.push(finger(A::Move, 0, 502.0, 500.0, t + 60 * MS));
    out.push(finger(A::Move, 1, 621.0, 506.0, t + 60 * MS));
    out.push(finger(A::PointerUp, 0, 502.0, 500.0, t + 120 * MS));
    out.push(finger(A::Up, 1, 621.0, 506.0, t + 130 * MS));
    t += 500 * MS;
    // C. Three-finger tap.
    out.push(finger(A::Down, 0, 500.0, 500.0, t));
    out.push(finger(A::PointerDown, 1, 600.0, 480.0, t + 15 * MS));
    out.push(finger(A::PointerDown, 2, 700.0, 500.0, t + 30 * MS));
    out.push(finger(A::Move, 2, 701.0, 501.0, t + 70 * MS));
    out.push(finger(A::PointerUp, 1, 600.0, 480.0, t + 120 * MS));
    out.push(finger(A::PointerUp, 0, 500.0, 500.0, t + 130 * MS));
    out.push(finger(A::Up, 2, 701.0, 501.0, t + 150 * MS));
    t += 500 * MS;
    // D. Slow two-finger hold (400 ms): no tap, no view change.
    out.push(finger(A::Down, 0, 500.0, 500.0, t));
    out.push(finger(A::PointerDown, 1, 620.0, 505.0, t + 20 * MS));
    out.push(finger(A::PointerUp, 0, 500.0, 500.0, t + 390 * MS));
    out.push(finger(A::Up, 1, 620.0, 505.0, t + 400 * MS));
    t += 900 * MS;
    // E. Two-finger pan by (+150, -80).
    out.push(finger(A::Down, 0, 400.0, 400.0, t));
    t += 10 * MS;
    out.push(finger(A::PointerDown, 1, 600.0, 400.0, t));
    for k in 1..=30u16 {
        t += 8 * MS;
        let (dx, dy) = (5.0 * f32::from(k), -8.0 * f32::from(k) / 3.0);
        out.push(finger(A::Move, 0, 400.0 + dx, q(400.0 + f64::from(dy)), t));
        out.push(finger(A::Move, 1, 600.0 + dx, q(400.0 + f64::from(dy)), t));
    }
    t += 8 * MS;
    out.push(finger(A::PointerUp, 0, 550.0, 320.0, t));
    t += 8 * MS;
    out.push(finger(A::Up, 1, 750.0, 320.0, t));
    out
}

pub type Generator = fn() -> Vec<RawPointerEvent>;

pub const FIXTURES: [(&str, Generator); 4] = [
    ("stylus_confirmed.jsonl", stylus_confirmed),
    ("stylus_cancel_and_palm.jsonl", stylus_cancel_and_palm),
    ("pen_pressure_curve.jsonl", pen_pressure_curve),
    ("two_finger_gesture.jsonl", two_finger_gesture),
];

pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/input")
}

pub fn load_fixture(name: &str) -> Vec<RawPointerEvent> {
    let text = std::fs::read_to_string(fixture_dir().join(name)).unwrap();
    trace::parse_jsonl(&text).unwrap()
}

/// Replay one event at a time, feeding `ViewChanged` back into the view (like the engine does).
pub fn replay(arb: &mut InputArbiter, events: &[RawPointerEvent], view: &mut ViewTransform) -> Vec<InputAction> {
    let mut out = Vec::new();
    for e in events {
        let acts = arb.handle(e, view);
        apply_views(&acts, view);
        out.extend(acts);
    }
    out
}

/// Replay grouped per MotionEvent frame (historical..., current, predicted...) via `handle_batch`.
pub fn replay_frames(arb: &mut InputArbiter, events: &[RawPointerEvent], view: &mut ViewTransform) -> Vec<InputAction> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..events.len() {
        let cur = &events[i];
        let flush = match events.get(i + 1) {
            None => true,
            Some(next) => !next.predicted && !cur.historical,
        };
        if flush {
            let acts = arb.handle_batch(&events[start..=i], view);
            apply_views(&acts, view);
            out.extend(acts);
            start = i + 1;
        }
    }
    out
}

pub fn apply_views(acts: &[InputAction], view: &mut ViewTransform) {
    for a in acts {
        if let InputAction::ViewChanged(v) = a {
            *view = *v;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    Commit,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Stroke {
    pub tool: ToolType,
    pub eraser: bool,
    pub samples: Vec<StrokeSample>,
    pub predictions: Vec<StrokeSample>,
    pub ended: Option<Ended>,
}

/// Validates action-stream invariants and records strokes.
#[derive(Debug, Default)]
pub struct Checker {
    pub strokes: Vec<Stroke>,
    open: bool,
    gesture_open: bool,
    pub views: usize,
    pub gesture_ends: usize,
    pub two_taps: usize,
    pub three_taps: usize,
    pub hovers: Vec<Option<veskra_core::PointF>>,
    pub buttons: Vec<InputAction>,
}

impl Checker {
    pub fn feed(&mut self, acts: &[InputAction]) {
        for a in acts {
            self.one(a);
        }
    }

    fn one(&mut self, a: &InputAction) {
        let stroke_action = matches!(
            a,
            InputAction::StrokeBegin { .. }
                | InputAction::StrokeSamples(_)
                | InputAction::StrokePrediction(_)
                | InputAction::StrokeCommit
                | InputAction::StrokeCancel
        );
        assert!(!(stroke_action && self.gesture_open), "stroke action during gesture: {a:?}");
        match a {
            InputAction::StrokeBegin { tool, eraser } => {
                assert!(!self.open, "StrokeBegin while a stroke is open");
                self.open = true;
                self.strokes.push(Stroke {
                    tool: *tool,
                    eraser: *eraser,
                    samples: Vec::new(),
                    predictions: Vec::new(),
                    ended: None,
                });
            }
            InputAction::StrokeSamples(v) => {
                assert!(self.open, "samples outside a stroke");
                assert!(!v.is_empty(), "empty StrokeSamples");
                let s = self.strokes.last_mut().unwrap();
                for x in v {
                    assert!(x.is_confirmed(), "unconfirmed sample in StrokeSamples");
                    assert!(!x.flags.contains(SampleFlags::PREDICTED), "predicted in confirmed");
                    x.validate().unwrap();
                    if let Some(prev) = s.samples.last() {
                        assert!(x.t_ns >= prev.t_ns, "non-monotonic t");
                    }
                    s.samples.push(*x);
                }
            }
            InputAction::StrokePrediction(v) => {
                assert!(self.open, "prediction outside a stroke");
                for x in v {
                    assert!(x.flags.contains(SampleFlags::PREDICTED));
                    assert!(!x.flags.contains(SampleFlags::CONFIRMED));
                    assert!(!x.is_confirmed());
                    x.validate().unwrap();
                }
                self.strokes.last_mut().unwrap().predictions.extend_from_slice(v);
            }
            InputAction::StrokeCommit | InputAction::StrokeCancel => {
                assert!(self.open, "end without begin: {a:?}");
                self.open = false;
                let s = self.strokes.last_mut().unwrap();
                assert!(!s.samples.is_empty(), "stroke ended without confirmed samples");
                s.ended = Some(if *a == InputAction::StrokeCommit { Ended::Commit } else { Ended::Cancel });
            }
            InputAction::ViewChanged(v) => {
                assert!(!self.open, "view change during stroke");
                self.gesture_open = true;
                self.views += 1;
                assert!(v.zoom.is_finite() && (MIN_ZOOM..=MAX_ZOOM).contains(&v.zoom));
                assert!(v.doc_to_view().is_finite() && v.view_to_doc().is_finite());
            }
            InputAction::GestureEnd => {
                assert!(self.gesture_open, "GestureEnd without view change");
                self.gesture_open = false;
                self.gesture_ends += 1;
            }
            InputAction::Hover(h) => {
                if let Some(p) = h {
                    assert!(p.is_finite());
                }
                self.hovers.push(*h);
            }
            InputAction::StylusButton { .. } => self.buttons.push(a.clone()),
            InputAction::TwoFingerTap => self.two_taps += 1,
            InputAction::ThreeFingerTap => self.three_taps += 1,
        }
    }

    pub fn finish(&self) {
        assert!(!self.open, "stroke left open");
        assert!(!self.gesture_open, "gesture left open");
    }

    pub fn count(&self, e: Ended) -> usize {
        self.strokes.iter().filter(|s| s.ended.as_ref() == Some(&e)).count()
    }
}
