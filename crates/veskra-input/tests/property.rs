//! Exactly-once / confirmed-only properties over 10 000 fixed-seed random event sequences,
//! plus a no-panic fuzz of the packed JNI decoder.
#![allow(clippy::unwrap_used)]

mod support;

use support::{Checker, Rng};
use veskra_core::{PointF, ToolType};
use veskra_input::{
    ButtonAction, GestureConfig, InputAction, InputArbiter, InputConfig, PointerAction as A, RawPointerEvent,
    StylusGesturePolicy, ViewTransform, unpack_events,
};

const SEQUENCES: u64 = 10_000;
const ACTIONS: [A; 9] =
    [A::Down, A::Move, A::Up, A::Cancel, A::PointerDown, A::PointerUp, A::HoverEnter, A::HoverMove, A::HoverExit];
const TOOLS: [ToolType; 5] =
    [ToolType::Stylus, ToolType::StylusEraser, ToolType::Finger, ToolType::Mouse, ToolType::Unknown];

fn opt(rng: &mut Rng, pct: u64, lo: f64, hi: f64) -> Option<f32> {
    if rng.chance(pct, 100) { Some(rng.range(lo, hi) as f32) } else { None }
}

fn random_event(rng: &mut Rng, t: &mut u64, last_xy: &mut (f32, f32)) -> RawPointerEvent {
    // Moves dominate, like real traces.
    let action = if rng.chance(45, 100) { A::Move } else { ACTIONS[rng.below(9) as usize] };
    let tool = TOOLS[(rng.below(100) as usize * TOOLS.len()) / 100 % 5];
    if rng.chance(1, 10) {
        *t = t.saturating_sub(rng.below(5_000_000)); // out-of-order timestamps
    } else {
        *t += rng.below(10_000_000);
    }
    let (x, y) = if rng.chance(1, 50) {
        (f32::NAN, 0.0) // the arbiter must survive unvalidated events too
    } else if rng.chance(1, 8) {
        *last_xy // exact repeats (duplicate detection)
    } else {
        (rng.range(-100.0, 2100.0) as f32, rng.range(-100.0, 1700.0) as f32)
    };
    *last_xy = if x.is_finite() { (x, y) } else { *last_xy };
    let mut e = RawPointerEvent::new(action, rng.below(4) as i32, tool, x, y, *t);
    e.pressure = opt(rng, 70, -0.3, 1.3);
    e.tilt = opt(rng, 40, -0.2, 2.0);
    e.orientation = opt(rng, 40, -4.0, 4.0);
    e.touch_major = opt(rng, 60, 0.0, 260.0);
    e.touch_minor = e.touch_major.map(|m| m * 0.8);
    e.buttons = [0, 0, 0, 0x20, 0x40, 0x60, 1][rng.below(7) as usize];
    e.historical = rng.chance(20, 100);
    e.predicted = rng.chance(15, 100);
    e.canceled_flag = rng.chance(5, 100);
    e
}

fn random_config(rng: &mut Rng) -> InputConfig {
    let actions = [ButtonAction::None, ButtonAction::Eyedropper, ButtonAction::ToggleEraser, ButtonAction::Undo];
    InputConfig {
        finger_draws: rng.chance(1, 2),
        palm_major_threshold_px: rng.range(50.0, 200.0) as f32,
        stylus_button_action: actions[rng.below(4) as usize],
        stylus_secondary_button_action: actions[rng.below(4) as usize],
        stylus_during_gesture: if rng.chance(1, 2) {
            StylusGesturePolicy::StylusWins
        } else {
            StylusGesturePolicy::IgnoreStylus
        },
        one_finger_pan: rng.chance(1, 2),
        touch_slop_px: rng.range(0.0, 20.0) as f32,
        gesture: GestureConfig {
            rotation_snap_rad: rng.range(0.0, 0.2),
            rotate: rng.chance(3, 4),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn exactly_once_commit_and_confirmed_only_over_random_sequences() {
    let mut totals = (0u64, 0u64, 0u64, 0u64, 0u64);
    for seq in 0..SEQUENCES {
        let mut rng = Rng::new(0xC0FF_EE00_0000_0000 ^ seq.wrapping_mul(0x9E37_79B9));
        let mut arb = InputArbiter::new(random_config(&mut rng));
        let mut view = ViewTransform {
            zoom: rng.range(0.1, 8.0),
            rotation: rng.range(-3.0, 3.0),
            pan: PointF::new(rng.range(-500.0, 500.0), rng.range(-500.0, 500.0)),
            mirror: rng.chance(1, 4),
            ..ViewTransform::identity(2000.0, 1600.0)
        };
        let mut chk = Checker::default();
        let n = 1 + rng.below(80);
        let mut t = rng.below(1_000_000_000);
        let mut last_xy = (0.0, 0.0);
        let mut events = Vec::new();
        for _ in 0..n {
            events.push(random_event(&mut rng, &mut t, &mut last_xy));
        }
        // Feed in random chunk sizes, interleaving lifecycle resets.
        let mut i = 0;
        while i < events.len() {
            if rng.chance(1, 30) {
                chk.feed(&arb.reset());
            }
            let k = (1 + rng.below(6) as usize).min(events.len() - i);
            let acts = arb.handle_batch(&events[i..i + k], &view);
            support::apply_views(&acts, &mut view);
            chk.feed(&acts);
            i += k;
        }
        chk.feed(&arb.reset());
        chk.finish();
        let st = *arb.stats();
        let begun = chk.strokes.len() as u64;
        assert_eq!(st.strokes_begun, begun, "seq {seq}");
        assert_eq!(st.strokes_committed + st.strokes_cancelled, begun, "seq {seq}: exactly one end per begin");
        assert_eq!(st.strokes_committed as usize, chk.count(support::Ended::Commit));
        let samples: u64 = chk.strokes.iter().map(|s| s.samples.len() as u64).sum();
        assert_eq!(st.confirmed_samples, samples, "seq {seq}");
        let preds: u64 = chk.strokes.iter().map(|s| s.predictions.len() as u64).sum();
        assert_eq!(st.predicted_samples, preds, "seq {seq}");
        assert_eq!(st.events, n);
        totals.0 += begun;
        totals.1 += st.strokes_cancelled;
        totals.2 += st.gestures;
        totals.3 += st.two_finger_taps + st.three_finger_taps;
        totals.4 += preds;
    }
    eprintln!("random sequences (strokes, cancels, gestures, taps, predictions): {totals:?}");
    // The generator really exercises the interesting paths.
    let (strokes, cancels, gestures, _taps, preds) = totals;
    assert!(strokes > 5_000 && cancels > 1_000 && gestures > 500 && preds > 1_000, "{totals:?}");
}

#[test]
fn predicted_never_reaches_confirmed_even_when_interleaved() {
    // Dense prediction stream: every confirmed move followed by 3 predicted ones.
    let mut rng = Rng::new(7);
    let mut arb = InputArbiter::new(InputConfig::default());
    let view = ViewTransform::identity(1000.0, 1000.0);
    let mut evs = vec![RawPointerEvent::new(A::Down, 0, ToolType::Stylus, 10.0, 10.0, 0)];
    for i in 1..=500u64 {
        let x = 10.0 + i as f32;
        evs.push(RawPointerEvent::new(A::Move, 0, ToolType::Stylus, x, 10.0, i * 1000));
        for k in 1..=3u64 {
            let mut p =
                RawPointerEvent::new(A::Move, 0, ToolType::Stylus, x + rng.range(0.0, 5.0) as f32, 99.0, i * 1000 + k);
            p.predicted = true;
            evs.push(p);
        }
    }
    evs.push(RawPointerEvent::new(A::Up, 0, ToolType::Stylus, 510.0, 10.0, 501_000));
    let acts = arb.handle_batch(&evs, &view);
    let mut chk = Checker::default();
    chk.feed(&acts);
    chk.finish();
    assert_eq!(chk.strokes[0].samples.len(), 501);
    assert!(chk.strokes[0].samples.iter().all(|s| s.y == 10.0), "no predicted (y=99) point confirmed");
    assert_eq!(chk.strokes[0].predictions.len(), 1500);
    assert!(acts.iter().any(|a| matches!(a, InputAction::StrokePrediction(v) if v.len() == 3)));
}

#[test]
fn unpack_fuzz_never_panics() {
    let mut rng = Rng::new(0xFEED);
    let mut ok = 0;
    for _ in 0..20_000 {
        let count = rng.below(12) as usize;
        let slack = |rng: &mut Rng| rng.below(3) as usize;
        let ni = (count * 5 + slack(&mut rng)).saturating_sub(rng.below(2) as usize);
        let nf = (count * 8 + slack(&mut rng)).saturating_sub(rng.below(2) as usize);
        let nl = (count + slack(&mut rng)).saturating_sub(rng.below(2) as usize);
        let ints: Vec<i32> = (0..ni)
            .map(|j| match j % 5 {
                0 => [0, 1, 2, 3, 5, 6, 7, 9, 10, 4, -1][rng.below(11) as usize],
                4 => [0, 1, 2, 4, 5, 8][rng.below(6) as usize],
                _ => rng.next_u64() as i32 % 8,
            })
            .collect();
        let floats: Vec<f32> = (0..nf)
            .map(|_| match rng.below(20) {
                0 => f32::NAN,
                1 => f32::INFINITY,
                _ => rng.range(-10.0, 3000.0) as f32,
            })
            .collect();
        let longs: Vec<i64> = (0..nl).map(|_| rng.next_u64() as i64 >> rng.below(64)).collect();
        if let Ok(evs) = unpack_events(&ints, &floats, &longs, count) {
            assert_eq!(evs.len(), count);
            assert!(evs.iter().all(|e| e.validate().is_ok()));
            ok += 1;
        }
    }
    assert!(ok > 100, "fuzz produced too few valid batches ({ok})");
}
