//! Fixture-driven verification (STY-001, -002, -004, -005, -007, palm, pressure curve, taps).
//! Fixtures are SYNTHETIC (see tests/fixtures/input/README.md), not hardware captures.
#![allow(clippy::unwrap_used)]

mod support;

use support::{Checker, Ended, load_fixture, replay, replay_frames};
use veskra_core::{PointF, SampleFlags, ToolType};
use veskra_input::view::normalize_angle;
use veskra_input::{
    FALLBACK_PRESSURE, GestureConfig, InputAction, InputArbiter, InputConfig, PointerAction as A, PressureCurve,
    RawPointerEvent, ViewTransform, pack_events, trace, unpack_events,
};

const REGEN_ENV: &str = "VESKRA_REGEN_INPUT_FIXTURES";

#[test]
fn fixtures_match_deterministic_generator() {
    let regen = std::env::var(REGEN_ENV).is_ok_and(|v| v == "1");
    for (name, generate) in support::FIXTURES {
        let text = trace::to_jsonl(&generate()).unwrap();
        let path = support::fixture_dir().join(name);
        if regen {
            std::fs::write(&path, &text).unwrap();
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            on_disk == text,
            "{name} is stale; regenerate with {REGEN_ENV}=1 cargo test -p veskra-input --test fixtures"
        );
        // The packed JNI path is lossless for every fixture event.
        let events = trace::parse_jsonl(&on_disk).unwrap();
        let (i, f, l) = pack_events(&events);
        assert_eq!(unpack_events(&i, &f, &l, events.len()).unwrap(), events, "{name} pack/unpack");
    }
}

fn view() -> ViewTransform {
    ViewTransform::identity(2560.0, 1600.0)
}

/// Confirmed samples the arbiter must emit for a single-pointer stylus trace: every
/// non-predicted DOWN/MOVE plus UPs that move away from the previous sample.
fn expected_confirmed(events: &[RawPointerEvent]) -> usize {
    let mut n = 0;
    let mut last: Option<(f32, f32)> = None;
    for e in events.iter().filter(|e| !e.predicted) {
        match e.action {
            A::Down | A::Move => {
                n += 1;
                last = Some((e.x, e.y));
            }
            A::Up => {
                if last != Some((e.x, e.y)) {
                    n += 1;
                }
                last = None;
            }
            _ => {}
        }
    }
    n
}

#[test]
fn sty_001_thousand_events_monotonic_pressure_in_range() {
    let events = load_fixture("stylus_confirmed.jsonl");
    let confirmed_contact = events.iter().filter(|e| !e.predicted && matches!(e.action, A::Down | A::Move)).count();
    assert!(events.len() >= 1000 && confirmed_contact >= 1000, "fixture too small");
    let historical = events.iter().filter(|e| e.historical).count();
    assert!(historical > 700);

    let mut arb = InputArbiter::new(InputConfig::default());
    let mut v = view();
    let acts = replay_frames(&mut arb, &events, &mut v);
    let mut chk = Checker::default();
    chk.feed(&acts);
    chk.finish();
    assert_eq!(chk.strokes.len(), 2);
    assert_eq!(chk.count(Ended::Commit), 2);
    let samples: Vec<_> = chk.strokes.iter().flat_map(|s| s.samples.iter()).collect();
    assert_eq!(samples.len(), expected_confirmed(&events));
    assert!(samples.len() >= 1000);
    for s in &samples {
        assert!((0.0..=1.0).contains(&s.pressure));
        assert!(s.flags.contains(SampleFlags::PRESSURE_REPORTED | SampleFlags::CONFIRMED));
    }
    for st in &chk.strokes {
        assert!(st.samples.windows(2).all(|w| w[1].t_ns >= w[0].t_ns));
    }
    let hist_samples = samples.iter().filter(|s| s.flags.contains(SampleFlags::HISTORICAL)).count();
    assert_eq!(hist_samples, historical);
    // Identity view: document coords equal view coords; UP duplicate was not added twice.
    assert_eq!(arb.stats().duplicate_samples_dropped, 1);
    assert_eq!(arb.stats().out_of_order_dropped, 0);
    assert!(chk.strokes[1].eraser);
    assert_eq!(chk.strokes[0].tool, ToolType::Stylus);

    // Per-event replay yields exactly the same confirmed samples as batched replay.
    let mut arb2 = InputArbiter::new(InputConfig::default());
    let mut v2 = view();
    let mut chk2 = Checker::default();
    chk2.feed(&replay(&mut arb2, &events, &mut v2));
    let s2: Vec<_> = chk2.strokes.iter().flat_map(|s| s.samples.iter()).collect();
    assert_eq!(samples, s2);
}

#[test]
fn sty_001_out_of_order_historical_dropped_and_counted() {
    let mut events = load_fixture("stylus_confirmed.jsonl");
    // Swap two historical samples inside the first stroke: the later-delivered older one drops.
    let idx = events.iter().position(|e| e.historical).unwrap() + 1;
    assert!(events[idx].historical);
    let late = events[idx - 1].t_ns - 1;
    events[idx].t_ns = late;
    let mut arb = InputArbiter::new(InputConfig::default());
    let mut chk = Checker::default();
    chk.feed(&replay_frames(&mut arb, &events, &mut view()));
    chk.finish();
    assert_eq!(arb.stats().out_of_order_dropped, 1);
    assert!(chk.strokes[0].samples.iter().all(|s| s.t_ns != late));
    let total: usize = chk.strokes.iter().map(|s| s.samples.len()).sum();
    assert_eq!(total, expected_confirmed(&events) - 1);
}

#[test]
fn sty_002_tilt_azimuth_preserved_or_none() {
    let events = load_fixture("stylus_confirmed.jsonl");
    for rotation in [0.0f64, 0.5] {
        let mut arb = InputArbiter::new(InputConfig::default());
        let mut v = ViewTransform { rotation, ..view() };
        let mut chk = Checker::default();
        chk.feed(&replay_frames(&mut arb, &events, &mut v));
        let (a, b) = (&chk.strokes[0], &chk.strokes[1]);
        // stroke A reports tilt/orientation; samples are in event order.
        let reported: Vec<_> = events
            .iter()
            .filter(|e| !e.predicted && e.tilt.is_some() && matches!(e.action, A::Down | A::Move))
            .collect();
        assert_eq!(reported.len(), a.samples.len());
        for (s, e) in a.samples.iter().zip(&reported) {
            assert_eq!(s.tilt, e.tilt);
            let expect = normalize_angle(f64::from(e.orientation.unwrap()) - rotation);
            let got = f64::from(s.azimuth.unwrap());
            assert!(normalize_angle(got - expect).abs() < 1e-5, "azimuth {got} vs {expect}");
            assert_eq!(s.rotation, None);
        }
        // stroke B (eraser) does not report them: never invented.
        assert!(b.samples.iter().all(|s| s.tilt.is_none() && s.azimuth.is_none() && s.rotation.is_none()));
    }
}

#[test]
fn sty_007_predictions_never_confirmed() {
    let events = load_fixture("stylus_confirmed.jsonl");
    let predicted = events.iter().filter(|e| e.predicted).count();
    assert!(predicted > 0);
    let mut arb = InputArbiter::new(InputConfig::default());
    let mut chk = Checker::default();
    chk.feed(&replay_frames(&mut arb, &events, &mut view()));
    let preds: usize = chk.strokes.iter().map(|s| s.predictions.len()).sum();
    assert_eq!(preds, predicted);
    assert_eq!(arb.stats().predicted_samples as usize, predicted);
    // Confirmed samples are exactly the non-predicted events: none came from a prediction.
    let confirmed_ts: Vec<u64> =
        events.iter().filter(|e| !e.predicted && matches!(e.action, A::Down | A::Move)).map(|e| e.t_ns).collect();
    let got_ts: Vec<u64> = chk.strokes.iter().flat_map(|s| s.samples.iter().map(|x| x.t_ns)).collect();
    assert!(
        got_ts.iter().all(|t| confirmed_ts.contains(t) || events.iter().any(|e| e.action == A::Up && e.t_ns == *t))
    );
    // Each frame's two predictions arrive together and replace the previous prediction.
    let frames = chk.strokes.iter().map(|s| s.predictions.len()).sum::<usize>() / 2;
    let pred_actions = replay_frames(&mut InputArbiter::new(InputConfig::default()), &events, &mut view())
        .iter()
        .filter(|a| matches!(a, InputAction::StrokePrediction(v) if v.len() == 2))
        .count();
    assert_eq!(pred_actions, frames);
}

#[test]
fn sty_004_cancel_and_palm_trace() {
    let events = load_fixture("stylus_cancel_and_palm.jsonl");
    let mut arb = InputArbiter::new(InputConfig::default());
    let mut chk = Checker::default();
    chk.feed(&replay_frames(&mut arb, &events, &mut view()));
    chk.finish();
    let summary: Vec<_> = chk.strokes.iter().map(|s| (s.tool, s.ended.clone().unwrap())).collect();
    assert_eq!(
        summary,
        vec![
            (ToolType::Finger, Ended::Cancel), // FLAG_CANCELED on UP
            (ToolType::Finger, Ended::Cancel), // contact grew to palm size
            (ToolType::Stylus, Ended::Commit), // A
            (ToolType::Stylus, Ended::Cancel), // B: ACTION_CANCEL
            (ToolType::Stylus, Ended::Commit), // C: palm beside it ignored
        ]
    );
    let st = arb.stats();
    assert_eq!(st.palms_rejected, 3);
    assert_eq!((st.strokes_begun, st.strokes_committed, st.strokes_cancelled), (5, 2, 3));
    // Stroke C contains only stylus samples (y = 905), nothing from the palm.
    assert!(chk.strokes[4].samples.iter().all(|s| s.y == 905.0));
    assert_eq!(chk.strokes[4].samples.len(), 41);
    // The palm never navigated; only scenario 7 (a normal finger after the stylus) panned.
    assert_eq!(chk.gesture_ends, 1);
    assert_eq!(st.gestures, 1);
    assert_eq!(arb.state(), veskra_input::ArbiterState::Idle);

    // With finger drawing disabled only the three stylus strokes exist.
    let mut arb = InputArbiter::new(InputConfig { finger_draws: false, ..InputConfig::default() });
    let mut chk = Checker::default();
    chk.feed(&replay_frames(&mut arb, &events, &mut view()));
    chk.finish();
    assert_eq!(chk.strokes.len(), 3);
    assert!(chk.strokes.iter().all(|s| s.tool == ToolType::Stylus));
}

#[test]
fn pressure_curve_clamp_and_fallback() {
    let events = load_fixture("pen_pressure_curve.jsonl");
    let curve = PressureCurve::new(&[[0.0, 0.0], [0.5, 0.25], [0.8, 0.7], [1.0, 1.0]]).unwrap();
    let cfg = InputConfig { pressure_curve: curve.clone(), ..InputConfig::default() };
    let mut arb = InputArbiter::new(cfg);
    let mut chk = Checker::default();
    chk.feed(&replay_frames(&mut arb, &events, &mut view()));
    chk.finish();
    assert_eq!(chk.strokes.len(), 4);
    // 1. Ramp: curve applied to hardware pressure, monotone, flagged as reported.
    let ramp = &chk.strokes[0];
    assert_eq!(ramp.samples.len(), 101);
    for (i, s) in ramp.samples.iter().enumerate() {
        assert_eq!(s.pressure, curve.eval(i as f32 / 100.0));
        assert!(s.flags.contains(SampleFlags::PRESSURE_REPORTED));
    }
    assert!(ramp.samples.windows(2).all(|w| w[1].pressure >= w[0].pressure));
    assert!(ramp.samples.iter().any(|s| s.pressure != curve.eval(0.0) && s.pressure < 0.25));
    // 2. Out-of-range hardware values clamped (and counted), still reported.
    let clamp = &chk.strokes[1];
    let p: Vec<f32> = clamp.samples.iter().map(|s| s.pressure).collect();
    assert_eq!(p, vec![0.0, 0.0, 0.25, 1.0, 1.0, 1.0]);
    assert_eq!(arb.stats().pressure_clamped, 2);
    // 3. Stylus without pressure: documented fallback, NOT flagged as reported, curve not applied.
    // 4. Mouse: fallback as well (mouse "pressure" is not a pen force).
    for (st, tool) in [(&chk.strokes[2], ToolType::Stylus), (&chk.strokes[3], ToolType::Mouse)] {
        assert_eq!(st.tool, tool);
        assert!(st.samples.len() >= 20);
        for s in &st.samples {
            assert_eq!(s.pressure, FALLBACK_PRESSURE);
            assert!(!s.flags.contains(SampleFlags::PRESSURE_REPORTED));
        }
    }
}

fn split_sequences(events: &[RawPointerEvent]) -> Vec<Vec<RawPointerEvent>> {
    let mut out = vec![Vec::new()];
    for e in events {
        out.last_mut().unwrap().push(*e);
        if e.action == A::Up {
            out.push(Vec::new());
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

fn close(a: PointF, b: PointF, eps: f64) -> bool {
    (a.x - b.x).abs() <= eps && (a.y - b.y).abs() <= eps
}

#[test]
fn sty_005_pinch_anchor_stable_no_strokes() {
    let events = load_fixture("two_finger_gesture.jsonl");
    let seqs = split_sequences(&events);
    assert_eq!(seqs.len(), 5);
    let pinch = &seqs[0];
    for snap in [0.0, 5f64.to_radians()] {
        let cfg = InputConfig {
            gesture: GestureConfig { rotation_snap_rad: snap, ..Default::default() },
            ..Default::default()
        };
        let mut arb = InputArbiter::new(cfg);
        let v0 = view();
        let mut v = v0;
        let (mut p, mut q) = (PointF::default(), PointF::default());
        let mut base: Option<(PointF, PointF)> = None;
        let mut chk = Checker::default();
        let mut checked = 0;
        for e in pinch {
            let pt = PointF::new(f64::from(e.x), f64::from(e.y));
            if e.pointer_id == 0 {
                p = pt
            } else {
                q = pt
            }
            if e.action == A::PointerDown {
                base = Some((p, q));
            }
            let acts = arb.handle(e, &v);
            chk.feed(&acts);
            for a in &acts {
                if let InputAction::ViewChanged(nv) = a {
                    let (p0, q0) = base.unwrap();
                    let anchor = v0.view_to_doc().apply(p0.lerp(q0, 0.5));
                    // Stable anchor: doc point under the centroid stays under the centroid.
                    assert!(close(nv.view_to_doc().apply(p.lerp(q, 0.5)), anchor, 1e-6));
                    if snap == 0.0 {
                        // Without snapping both fingers stay pinned to their document points.
                        assert!(close(nv.view_to_doc().apply(p), v0.view_to_doc().apply(p0), 1e-6));
                        assert!(close(nv.view_to_doc().apply(q), v0.view_to_doc().apply(q0), 1e-6));
                    }
                    v = *nv;
                    checked += 1;
                }
            }
        }
        chk.finish();
        assert!(checked >= 55, "only {checked} view updates");
        assert!((v.zoom - 2.0).abs() < 1e-3, "zoom {}", v.zoom);
        let expected_rot = 60.0 * 0.008_726_5f64.asin();
        assert!((v.rotation - expected_rot).abs() < 1e-3, "rotation {}", v.rotation);
        // Only the provisional one-finger stroke exists, and it was cancelled (never committed).
        assert_eq!(chk.strokes.len(), 1);
        assert_eq!(chk.strokes[0].ended, Some(Ended::Cancel));
        assert_eq!(chk.strokes[0].samples.len(), 1);
        assert_eq!(chk.gesture_ends, 1);
    }
}

#[test]
fn sty_005_repeated_gestures_taps_and_pan() {
    let events = load_fixture("two_finger_gesture.jsonl");
    for finger_draws in [true, false] {
        let mut arb = InputArbiter::new(InputConfig { finger_draws, ..InputConfig::default() });
        let mut v = view();
        let mut chk = Checker::default();
        let mut views_seen = Vec::new();
        for seq in split_sequences(&events) {
            let before = v;
            chk.feed(&replay(&mut arb, &seq, &mut v));
            views_seen.push((before, v));
        }
        chk.finish();
        // Strokes: only provisional single-sample finger strokes, all cancelled by the 2nd finger.
        let expected = if finger_draws { 5 } else { 0 };
        assert_eq!(chk.strokes.len(), expected);
        assert!(chk.strokes.iter().all(|s| s.ended == Some(Ended::Cancel) && s.samples.len() == 1));
        assert_eq!((chk.two_taps, chk.three_taps), (1, 1), "B and C are taps, D (slow) is not");
        assert_eq!(chk.gesture_ends, 2);
        // B, C, D did not change the view.
        for (b, a) in &views_seen[1..4] {
            assert_eq!(b, a);
        }
        // E: pure two-finger pan — zoom/rotation bit-identical, pan moved by (150, -80).
        let (b, a) = views_seen[4];
        assert_eq!((a.zoom, a.rotation), (b.zoom, b.rotation));
        assert!((a.pan.x - b.pan.x - 150.0).abs() < 1e-6 && (a.pan.y - b.pan.y + 80.0).abs() < 1e-3);
    }
}
