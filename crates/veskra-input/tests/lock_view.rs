//! Camera lock (`InputConfig::lock_view`): finger gestures never change the view, drawing still works.
#![allow(clippy::unwrap_used, dead_code)]

mod support;

use support::{finger, pen, replay, two_finger_gesture};
use veskra_input::{InputAction as IA, InputArbiter, InputConfig, PointerAction as A, RawPointerEvent, ViewTransform};

const MS: u64 = 1_000_000;

fn view() -> ViewTransform {
    ViewTransform::identity(1000.0, 800.0)
}

fn run(cfg: InputConfig, evs: &[RawPointerEvent]) -> (Vec<IA>, ViewTransform) {
    let mut arb = InputArbiter::new(cfg);
    let mut v = view();
    let acts = replay(&mut arb, evs, &mut v);
    (acts, v)
}

fn views(acts: &[IA]) -> usize {
    acts.iter().filter(|a| matches!(a, IA::ViewChanged(_) | IA::GestureEnd)).count()
}

fn stylus_stroke(t0: u64) -> Vec<RawPointerEvent> {
    let mut v = vec![pen(A::Down, 100.0, 100.0, t0, 0.5)];
    for i in 1..=10u32 {
        v.push(pen(A::Move, 100.0 + 5.0 * i as f32, 100.0, t0 + u64::from(i) * MS, 0.5));
    }
    v.push(pen(A::Up, 150.0, 100.0, t0 + 11 * MS, 0.5));
    v
}

#[test]
fn locked_view_ignores_pan_zoom_rotate_but_still_draws() {
    let gesture = two_finger_gesture();
    let (free, moved) = run(InputConfig::default(), &gesture);
    assert!(views(&free) > 0, "the fixture navigates when the view is not locked");
    assert_ne!(moved, view());

    let locked = InputConfig { lock_view: true, ..Default::default() };
    let (acts, v) = run(locked.clone(), &gesture);
    assert_eq!(views(&acts), 0, "no ViewChanged / GestureEnd while locked: {acts:?}");
    assert_eq!(v, view());
    // Stroke arbitration is unchanged by the lock: nothing the gesture fingers do is committed.
    let strokes = |a: &[IA]| {
        a.iter().filter(|x| matches!(x, IA::StrokeBegin { .. } | IA::StrokeCommit | IA::StrokeCancel)).count()
    };
    assert_eq!(strokes(&acts), strokes(&free));
    assert!(!acts.iter().any(|a| matches!(a, IA::StrokeCommit)), "navigation fingers never commit a stroke");

    // One-finger pan (finger drawing disabled) is ignored too.
    let pan = vec![
        finger(A::Down, 0, 300.0, 300.0, 0),
        finger(A::Move, 0, 380.0, 340.0, 20 * MS),
        finger(A::Move, 0, 460.0, 380.0, 40 * MS),
        finger(A::Up, 0, 460.0, 380.0, 60 * MS),
    ];
    let no_draw = InputConfig { finger_draws: false, ..Default::default() };
    assert!(views(&run(no_draw.clone(), &pan).0) > 0);
    assert_eq!(views(&run(InputConfig { lock_view: true, ..no_draw }, &pan).0), 0);

    // The stylus draws exactly as without the lock, also right after a locked gesture.
    let mut evs = gesture.clone();
    let t0 = evs.iter().map(|e| e.t_ns).max().unwrap_or(0) + 100 * MS;
    evs.extend(stylus_stroke(t0));
    let (acts, v) = run(locked, &evs);
    assert_eq!(v, view());
    let commits = acts.iter().filter(|a| matches!(a, IA::StrokeCommit)).count();
    assert_eq!(commits, 1);
    let (plain, _) = run(InputConfig::default(), &stylus_stroke(0));
    // Samples of the last stroke (the fixture's fingers begin strokes that are cancelled).
    let samples = |a: &[IA]| -> Vec<(f32, f32)> {
        let from = a.iter().rposition(|x| matches!(x, IA::StrokeBegin { .. })).unwrap();
        let mut out = Vec::new();
        for x in &a[from..] {
            if let IA::StrokeSamples(s) = x {
                out.extend(s.iter().map(|p| (p.x, p.y)));
            }
        }
        out
    };
    assert_eq!(samples(&acts).len(), 11);
    assert_eq!(samples(&acts), samples(&plain), "locked view keeps document coordinates stable");
}

#[test]
fn lock_view_defaults_off_and_round_trips() {
    assert!(!InputConfig::default().lock_view);
    let j = serde_json::to_string(&InputConfig { lock_view: true, ..Default::default() }).unwrap();
    assert!(serde_json::from_str::<InputConfig>(&j).unwrap().lock_view);
    assert!(!serde_json::from_str::<InputConfig>("{}").unwrap().lock_view, "old configs still parse");
}
