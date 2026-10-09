//! Scenario tests for the arbiter state machine (STY-004, STY-006, lifecycle, arbitration).
#![allow(clippy::unwrap_used)]

mod support;

use support::{Checker, Ended, ev, finger, pen, replay};
use veskra_core::{PointF, SampleFlags, ToolType};
use veskra_input::event::android::{BUTTON_STYLUS_PRIMARY as P1, BUTTON_STYLUS_SECONDARY as P2};
use veskra_input::{
    ArbiterState, ButtonAction, InputAction as IA, InputArbiter, InputConfig, PointerAction as A, RawPointerEvent,
    StylusGesturePolicy, ViewTransform, pack_events, unpack_events,
};

const MS: u64 = 1_000_000;

fn view() -> ViewTransform {
    ViewTransform::identity(1000.0, 800.0)
}

fn arb() -> InputArbiter {
    InputArbiter::new(InputConfig::default())
}

fn run(arb: &mut InputArbiter, evs: &[RawPointerEvent]) -> (Vec<IA>, Checker) {
    let mut v = view();
    let acts = replay(arb, evs, &mut v);
    let mut c = Checker::default();
    c.feed(&acts);
    (acts, c)
}

fn with_buttons(mut e: RawPointerEvent, b: u32) -> RawPointerEvent {
    e.buttons = b;
    e
}

fn stylus_stroke(t0: u64, n: u32) -> Vec<RawPointerEvent> {
    let mut v = vec![pen(A::Down, 100.0, 100.0, t0, 0.5)];
    for i in 1..=n {
        v.push(pen(A::Move, 100.0 + 5.0 * i as f32, 100.0, t0 + u64::from(i) * MS, 0.5));
    }
    v.push(pen(A::Up, 100.0 + 5.0 * n as f32, 100.0, t0 + u64::from(n + 1) * MS, 0.5));
    v
}

#[test]
fn state_machine_hover_begin_streaming_commit() {
    let mut a = arb();
    let v = ViewTransform { zoom: 2.0, ..view() };
    assert_eq!(a.state(), ArbiterState::Idle);
    let acts = a.handle(&ev(A::HoverEnter, 0, ToolType::Stylus, 200.0, 100.0, 1), &v);
    assert_eq!(acts, vec![IA::Hover(Some(PointF::new(100.0, 50.0)))]);
    assert_eq!(a.state(), ArbiterState::Hover);
    assert_eq!(a.handle(&ev(A::HoverExit, 0, ToolType::Stylus, 200.0, 100.0, 2), &v), vec![IA::Hover(None)]);
    let acts = a.handle(&pen(A::Down, 200.0, 100.0, 3, 0.4), &v);
    assert!(matches!(acts[0], IA::StrokeBegin { tool: ToolType::Stylus, eraser: false }));
    assert_eq!(a.state(), ArbiterState::Begin);
    a.handle(&pen(A::Move, 210.0, 100.0, 4, 0.4), &v);
    assert_eq!(a.state(), ArbiterState::Streaming);
    assert_eq!(a.handle(&pen(A::Up, 210.0, 100.0, 5, 0.4), &v), vec![IA::StrokeCommit]);
    assert_eq!(a.state(), ArbiterState::Idle);
    // Hover while stylus is down without HOVER_EXIT is hidden at stroke begin.
    a.handle(&ev(A::HoverMove, 0, ToolType::Stylus, 1.0, 1.0, 6), &v);
    let acts = a.handle(&pen(A::Down, 1.0, 1.0, 7, 0.4), &v);
    assert_eq!(acts[0], IA::Hover(None));
    assert!(matches!(acts[1], IA::StrokeBegin { .. }));
}

#[test]
fn zero_length_tap_is_single_sample_stroke() {
    let mut a = arb();
    let (acts, c) = run(&mut a, &[pen(A::Down, 5.0, 5.0, 10, 0.3), pen(A::Up, 5.0, 5.0, 20, 0.0)]);
    c.finish();
    assert_eq!(acts.len(), 3);
    assert_eq!(c.strokes[0].samples.len(), 1);
    assert_eq!(c.strokes[0].ended, Some(Ended::Commit));
    assert_eq!(a.stats().duplicate_samples_dropped, 1);
}

#[test]
fn duplicate_and_stationary_samples() {
    let mut a = arb();
    let evs = [
        pen(A::Down, 5.0, 5.0, 10, 0.3),
        pen(A::Move, 5.0, 5.0, 10, 0.3), // identical position + time: duplicate
        pen(A::Move, 5.0, 5.0, 12, 0.6), // stationary pen, later time: real sample
        pen(A::Move, 6.0, 5.0, 11, 0.6), // older timestamp: dropped, not reordered
        pen(A::Move, 7.0, 5.0, 13, 0.6),
        pen(A::Up, 8.0, 5.0, 14, 0.0), // UP moved: sample added
    ];
    let (_, c) = run(&mut a, &evs);
    c.finish();
    let xs: Vec<(f32, u64)> = c.strokes[0].samples.iter().map(|s| (s.x, s.t_ns)).collect();
    assert_eq!(xs, vec![(5.0, 10), (5.0, 12), (7.0, 13), (8.0, 14)]);
    let st = a.stats();
    assert_eq!((st.duplicate_samples_dropped, st.out_of_order_dropped), (1, 1));
}

#[test]
fn sty_004_action_cancel_rolls_back_and_flag_canceled() {
    let mut a = arb();
    let mut evs = stylus_stroke(0, 5);
    evs.pop();
    evs.push(pen(A::Cancel, 0.0, 0.0, 10 * MS, 0.0));
    evs.push(pen(A::Move, 50.0, 50.0, 11 * MS, 0.5)); // stale move after cancel: orphan
    evs.push(pen(A::Up, 50.0, 50.0, 12 * MS, 0.5)); // stale up: orphan
    let (acts, c) = run(&mut a, &evs);
    c.finish();
    assert_eq!(c.count(Ended::Cancel), 1);
    assert_eq!(acts.iter().filter(|x| **x == IA::StrokeCommit).count(), 0);
    assert_eq!(a.stats().orphan_events, 2);
    // Next stroke after cancel is clean.
    let (_, c) = run(&mut a, &stylus_stroke(20 * MS, 3));
    c.finish();
    assert_eq!(c.count(Ended::Commit), 1);
    // POINTER_UP with FLAG_CANCELED for the drawing pointer cancels too.
    let mut a = arb();
    let mut evs = stylus_stroke(0, 3);
    let last = evs.last_mut().unwrap();
    last.canceled_flag = true;
    let (_, c) = run(&mut a, &evs);
    assert_eq!(c.count(Ended::Cancel), 1);
}

#[test]
fn lifecycle_reset_commits_pending_and_clears() {
    let mut a = InputArbiter::new(InputConfig { stylus_button_action: ButtonAction::Eyedropper, ..Default::default() });
    let v = view();
    let mut evs = stylus_stroke(0, 4);
    evs.pop();
    for e in &mut evs[2..] {
        e.buttons = P1;
    }
    let (_, c) = run(&mut a, &evs);
    assert_eq!(c.buttons.len(), 1);
    let out = a.reset();
    assert_eq!(out, vec![IA::StrokeCommit, IA::StylusButton { pressed: false, action: ButtonAction::Eyedropper }]);
    assert_eq!(a.state(), ArbiterState::Idle);
    // Pointer still down after resume is ignored until a new DOWN.
    assert!(a.handle(&pen(A::Move, 300.0, 100.0, 9 * MS, 0.5), &v).is_empty());
    assert!(a.handle(&pen(A::Up, 300.0, 100.0, 10 * MS, 0.5), &v).is_empty());
    assert_eq!(a.stats().orphan_events, 2);
    assert!(a.reset().is_empty(), "reset is idempotent");
    // Gesture + hover are closed by reset.
    let mut a = InputArbiter::new(InputConfig { finger_draws: false, ..Default::default() });
    a.handle(&ev(A::HoverEnter, 0, ToolType::Stylus, 10.0, 10.0, 0), &v);
    let mut v2 = v;
    let acts =
        replay(&mut a, &[finger(A::Down, 1, 500.0, 500.0, MS), finger(A::Move, 1, 560.0, 500.0, 2 * MS)], &mut v2);
    assert!(acts.iter().any(|x| matches!(x, IA::ViewChanged(_))));
    assert_eq!(a.state(), ArbiterState::Gesture);
    assert_eq!(a.reset(), vec![IA::GestureEnd, IA::Hover(None)]);
}

#[test]
fn sty_006_stylus_buttons_and_fallback() {
    let cfg = InputConfig {
        stylus_button_action: ButtonAction::Eyedropper,
        stylus_secondary_button_action: ButtonAction::Undo,
        ..Default::default()
    };
    let mut a = InputArbiter::new(cfg);
    let h = |b: u32, t: u64| with_buttons(ev(A::HoverMove, 0, ToolType::Stylus, 10.0, 10.0, t), b);
    let (_, c) = run(&mut a, &[h(0, 1), h(P1, 2), h(P1, 3), h(P1 | P2, 4), h(0, 5)]);
    assert_eq!(
        c.buttons,
        vec![
            IA::StylusButton { pressed: true, action: ButtonAction::Eyedropper },
            IA::StylusButton { pressed: true, action: ButtonAction::Undo },
            IA::StylusButton { pressed: false, action: ButtonAction::Eyedropper },
            IA::StylusButton { pressed: false, action: ButtonAction::Undo },
        ]
    );
    // Held button is recorded on samples.
    let evs: Vec<_> = stylus_stroke(10 * MS, 3).into_iter().map(|e| with_buttons(e, P1)).collect();
    let (_, c) = run(&mut a, &evs);
    assert!(c.strokes[0].samples.iter().all(|s| s.flags.contains(SampleFlags::BUTTON_PRIMARY)));
    // Device that never reports buttons: nothing emitted, stroke unaffected.
    let mut a = InputArbiter::new(InputConfig { stylus_button_action: ButtonAction::Eyedropper, ..Default::default() });
    let (_, c) = run(&mut a, &stylus_stroke(0, 10));
    assert!(c.buttons.is_empty());
    assert!(c.strokes[0].samples.iter().all(|s| !s.flags.contains(SampleFlags::BUTTON_PRIMARY)));
    // ButtonAction::None (default): presses are not emitted.
    let mut a = arb();
    let (_, c) = run(&mut a, &[h(P1, 1), h(0, 2)]);
    assert!(c.buttons.is_empty());
    // Finger/mouse button bits never count as stylus buttons.
    let mut a = InputArbiter::new(InputConfig { stylus_button_action: ButtonAction::Undo, ..Default::default() });
    let mut m = ev(A::HoverMove, 0, ToolType::Mouse, 1.0, 1.0, 1);
    m.buttons = P1;
    let (_, c) = run(&mut a, &[m]);
    assert!(c.buttons.is_empty());
}

#[test]
fn second_finger_cancels_finger_stroke_and_stylus_overrides() {
    let mut a = arb();
    let evs = [
        finger(A::Down, 0, 100.0, 100.0, 0),
        finger(A::Move, 0, 130.0, 100.0, MS),
        finger(A::PointerDown, 1, 300.0, 100.0, 2 * MS),
        finger(A::Move, 0, 100.0, 100.0, 3 * MS),
        finger(A::Move, 1, 350.0, 100.0, 3 * MS),
        finger(A::PointerUp, 1, 350.0, 100.0, 4 * MS),
        finger(A::Up, 0, 100.0, 100.0, 5 * MS),
    ];
    let (_, c) = run(&mut a, &evs);
    c.finish();
    assert_eq!(c.strokes.len(), 1);
    assert_eq!(c.strokes[0].ended, Some(Ended::Cancel));
    assert_eq!(c.gesture_ends, 1);
    // Stylus touching down during a finger stroke cancels the finger stroke and draws.
    let mut a = arb();
    let mut s = pen(A::PointerDown, 400.0, 400.0, 2 * MS, 0.5);
    s.pointer_id = 1;
    let mut su = pen(A::PointerUp, 420.0, 400.0, 4 * MS, 0.5);
    su.pointer_id = 1;
    let mut sm = pen(A::Move, 420.0, 400.0, 3 * MS, 0.5);
    sm.pointer_id = 1;
    let evs = [
        finger(A::Down, 0, 100.0, 100.0, 0),
        finger(A::Move, 0, 110.0, 100.0, MS),
        s,
        finger(A::Move, 0, 150.0, 100.0, 3 * MS),
        sm,
        su,
        finger(A::Up, 0, 150.0, 100.0, 5 * MS),
    ];
    let (_, c) = run(&mut a, &evs);
    c.finish();
    let sum: Vec<_> = c.strokes.iter().map(|s| (s.tool, s.ended.clone().unwrap(), s.samples.len())).collect();
    assert_eq!(sum, vec![(ToolType::Finger, Ended::Cancel, 2), (ToolType::Stylus, Ended::Commit, 2)]);
    assert!(a.stylus_seen());
    // From now on fingers navigate (stylus seen in session).
    let (_, c) = run(&mut a, &[finger(A::Down, 0, 100.0, 100.0, 9 * MS), finger(A::Up, 0, 100.0, 100.0, 10 * MS)]);
    assert!(c.strokes.is_empty());
}

#[test]
fn finger_during_stylus_stroke_is_ignored_no_tap() {
    let mut a = arb();
    let mut evs = stylus_stroke(0, 6);
    let up = evs.pop().unwrap();
    for (i, f) in [(1, 600.0f32), (2, 700.0)] {
        evs.push(finger(A::PointerDown, i, f, 500.0, 7 * MS));
    }
    evs.push(finger(A::Move, 1, 680.0, 520.0, 8 * MS));
    evs.push(finger(A::PointerUp, 1, 680.0, 520.0, 9 * MS));
    evs.push(finger(A::PointerUp, 2, 700.0, 500.0, 9 * MS));
    let mut up = up;
    up.t_ns = 10 * MS;
    evs.push(up);
    let (_, c) = run(&mut a, &evs);
    c.finish();
    assert_eq!(c.count(Ended::Commit), 1);
    assert_eq!((c.views, c.two_taps), (0, 0));
    assert_eq!(c.strokes[0].samples.len(), 7);
}

#[test]
fn stylus_during_gesture_policies() {
    let gesture = [
        finger(A::Down, 0, 400.0, 400.0, 0),
        finger(A::PointerDown, 1, 600.0, 400.0, MS),
        finger(A::Move, 0, 380.0, 400.0, 2 * MS),
        finger(A::Move, 1, 640.0, 400.0, 2 * MS),
    ];
    let mut s = pen(A::PointerDown, 100.0, 100.0, 3 * MS, 0.5);
    s.pointer_id = 2;
    let mut sm = pen(A::Move, 120.0, 100.0, 4 * MS, 0.5);
    sm.pointer_id = 2;
    let fm = finger(A::Move, 1, 700.0, 400.0, 4 * MS);
    let mut su = pen(A::PointerUp, 120.0, 100.0, 5 * MS, 0.5);
    su.pointer_id = 2;
    let tail = [s, sm, fm, su, finger(A::PointerUp, 1, 700.0, 400.0, 6 * MS), finger(A::Up, 0, 380.0, 400.0, 7 * MS)];
    let all: Vec<_> = gesture.iter().chain(tail.iter()).copied().collect();
    // Default: stylus wins, gesture ends, fingers ignored afterwards.
    let mut a = InputArbiter::new(InputConfig { finger_draws: false, ..Default::default() });
    let (acts, c) = run(&mut a, &all);
    c.finish();
    let gend = acts.iter().position(|x| *x == IA::GestureEnd).unwrap();
    let begin = acts.iter().position(|x| matches!(x, IA::StrokeBegin { .. })).unwrap();
    assert!(gend < begin);
    assert_eq!(c.count(Ended::Commit), 1);
    assert_eq!(c.gesture_ends, 1);
    assert!(acts[begin..].iter().all(|x| !matches!(x, IA::ViewChanged(_))), "fingers no longer navigate");
    // IgnoreStylus: no stroke, gesture continues.
    let cfg = InputConfig {
        finger_draws: false,
        stylus_during_gesture: StylusGesturePolicy::IgnoreStylus,
        ..Default::default()
    };
    let mut a = InputArbiter::new(cfg);
    let (acts, c) = run(&mut a, &all);
    c.finish();
    assert!(c.strokes.is_empty());
    let views = acts.iter().filter(|x| matches!(x, IA::ViewChanged(_))).count();
    assert_eq!(views, 3, "gesture kept updating after the stylus touched");
    assert_eq!(acts.last(), Some(&IA::GestureEnd));
}

#[test]
fn palm_never_draws_or_navigates() {
    for finger_draws in [true, false] {
        let mut a = InputArbiter::new(InputConfig { finger_draws, ..Default::default() });
        let mut p = finger(A::Down, 0, 100.0, 100.0, 0);
        p.touch_major = Some(250.0);
        let mut m = p;
        m.action = A::Move;
        m.x = 400.0;
        m.t_ns = MS;
        let mut u = m;
        u.action = A::Up;
        u.t_ns = 2 * MS;
        let (acts, _) = run(&mut a, &[p, m, u]);
        assert!(acts.is_empty(), "{acts:?}");
        assert_eq!(a.stats().palms_rejected, 1);
    }
}

#[test]
fn predicted_outside_stroke_dropped() {
    let mut a = arb();
    let mut p = pen(A::Move, 1.0, 1.0, 1, 0.5);
    p.predicted = true;
    assert!(a.handle(&p, &view()).is_empty());
    let mut d = p;
    d.action = A::Down;
    assert!(a.handle(&d, &view()).is_empty(), "a predicted DOWN never starts a stroke");
    assert_eq!(a.stats().predicted_dropped, 2);
    assert_eq!(a.state(), ArbiterState::Idle);
}

#[test]
fn samples_use_view_to_doc_with_mirror_and_rotation() {
    let v = ViewTransform { zoom: 2.0, rotation: 0.3, mirror: true, pan: PointF::new(40.0, -20.0), ..view() };
    let mut a = arb();
    let acts = a.handle_batch(&[pen(A::Down, 300.0, 200.0, 0, 0.5), pen(A::Move, 320.0, 260.0, 1, 0.5)], &v);
    let IA::StrokeSamples(s) = &acts[1] else { panic!("{acts:?}") };
    assert_eq!(s.len(), 2, "coalesced in one batch");
    for (smp, (x, y)) in s.iter().zip([(300.0, 200.0), (320.0, 260.0)]) {
        let back = v.doc_to_view().apply(PointF::new(f64::from(smp.x), f64::from(smp.y)));
        assert!((back.x - x).abs() < 1e-3 && (back.y - y).abs() < 1e-3);
    }
}

#[test]
fn jni_packed_path_end_to_end() {
    let evs = stylus_stroke(0, 50);
    let (ints, floats, longs) = pack_events(&evs);
    let count = veskra_input::batch_count_from_jni(evs.len() as i32).unwrap();
    let decoded = unpack_events(&ints, &floats, &longs, count).unwrap();
    let mut a = arb();
    let acts = a.handle_batch(&decoded, &view());
    let mut c = Checker::default();
    c.feed(&acts);
    c.finish();
    assert_eq!(c.strokes[0].samples.len(), 51);
    assert_eq!(acts.len(), 3, "Begin, one coalesced StrokeSamples, Commit");
}

#[test]
fn protocol_anomalies_commit_consistently() {
    let mut a = arb();
    let evs = [
        pen(A::Down, 1.0, 1.0, 0, 0.5),
        pen(A::Move, 2.0, 1.0, 1, 0.5),
        pen(A::Down, 9.0, 9.0, 2, 0.5), // lost UP: previous stroke committed
        pen(A::Up, 10.0, 9.0, 3, 0.5),
    ];
    let (_, c) = run(&mut a, &evs);
    c.finish();
    assert_eq!(c.count(Ended::Commit), 2);
    assert_eq!(a.stats().protocol_anomalies, 1);
}

#[test]
fn view_resize_during_gesture_keeps_working() {
    let mut a = InputArbiter::new(InputConfig { finger_draws: false, ..Default::default() });
    let mut v = view();
    replay(
        &mut a,
        &[
            finger(A::Down, 0, 400.0, 400.0, 0),
            finger(A::PointerDown, 1, 600.0, 400.0, MS),
            finger(A::Move, 1, 650.0, 400.0, 2 * MS),
        ],
        &mut v,
    );
    // Device rotation: the engine resizes the view mid-gesture.
    v = v.resized(800.0, 1000.0);
    let acts = a.handle(&finger(A::Move, 1, 700.0, 420.0, 3 * MS), &v);
    let Some(IA::ViewChanged(nv)) = acts.last() else { panic!("{acts:?}") };
    assert_eq!((nv.view_w, nv.view_h), (800.0, 1000.0));
    // Re-baselined: the doc point under the centroid at resize time stays anchored.
    let anchor = v.view_to_doc().apply(PointF::new(525.0, 400.0));
    assert!(nv.view_to_doc().apply(PointF::new(550.0, 410.0)).distance(anchor) < 1e-6);
}
