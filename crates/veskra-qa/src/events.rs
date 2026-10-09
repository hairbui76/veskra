//! Synthetic pointer-event builders (VIEW coordinates).
//!
//! **SYNTHETIC — NOT HARDWARE CAPTURES.** These sequences exercise the Rust arbitration logic
//! through the packed JNI layout. They say nothing about real digitizer timing, palm contact
//! shapes or pressure response (STY-003/005 remain HW, blocker B-2).

use veskra_core::ToolType;
use veskra_input::{PointerAction, RawPointerEvent};

/// 240 Hz stylus report interval.
pub const STYLUS_DT_NS: u64 = 4_166_667;

fn stylus_ev(action: PointerAction, id: i32, tool: ToolType, x: f32, y: f32, t: u64, p: f32) -> RawPointerEvent {
    let mut e = RawPointerEvent::new(action, id, tool, x, y, t);
    e.pressure = Some(p);
    e.tilt = Some(0.35);
    e.orientation = Some(-0.6);
    e
}

/// A stylus stroke along `pts` (`(x, y, pressure)`, view px) starting at `t0`.
/// Every 4th sample is reported as historical; with `predicted`, two predicted events
/// (extrapolated ahead) follow every current event. `first` is the action of the first
/// contact (`Down`, or `PointerDown` when another pointer is already on the glass).
pub fn stylus_stroke(
    pts: &[(f32, f32, f32)],
    t0: u64,
    pointer_id: i32,
    tool: ToolType,
    predicted: bool,
    first: PointerAction,
) -> Vec<RawPointerEvent> {
    let mut out = Vec::new();
    let mut t = t0;
    for (i, &(x, y, p)) in pts.iter().enumerate() {
        let action = if i == 0 { first } else { PointerAction::Move };
        let mut e = stylus_ev(action, pointer_id, tool, x, y, t, p);
        e.historical = i > 0 && i % 4 != 0;
        out.push(e);
        if predicted && i > 0 && i % 4 == 0 {
            let (px, py, _) = pts[i - 1];
            for k in 1..=2 {
                let k = k as f32;
                let mut pe = stylus_ev(
                    PointerAction::Move,
                    pointer_id,
                    tool,
                    x + (x - px) * 6.0 * k,
                    y + (y - py) * 6.0 * k + 25.0,
                    t + (k as u64) * STYLUS_DT_NS,
                    p,
                );
                pe.predicted = true;
                out.push(pe);
            }
        }
        t += STYLUS_DT_NS;
    }
    if let Some(&(x, y, p)) = pts.last() {
        let up = if first == PointerAction::Down { PointerAction::Up } else { PointerAction::PointerUp };
        out.push(stylus_ev(up, pointer_id, tool, x, y, t, p));
    }
    out
}

fn finger(action: PointerAction, id: i32, x: f32, y: f32, t: u64, major: f32) -> RawPointerEvent {
    let mut e = RawPointerEvent::new(action, id, ToolType::Finger, x, y, t);
    e.pressure = Some(1.0);
    e.touch_major = Some(major);
    e.touch_minor = Some(major * 0.85);
    e
}

/// Two-finger pinch around `center`: finger distance goes `d0 → d1` horizontally in `steps`
/// 60 Hz frames (zoom factor ≈ d1/d0), no rotation.
pub fn pinch(center: (f32, f32), d0: f32, d1: f32, steps: usize, t0: u64) -> Vec<RawPointerEvent> {
    let dt = 16_666_667u64;
    let (cx, cy) = center;
    let mut out = vec![
        finger(PointerAction::Down, 10, cx - d0 / 2.0, cy, t0, 40.0),
        finger(PointerAction::PointerDown, 11, cx + d0 / 2.0, cy, t0 + dt / 4, 40.0),
    ];
    let mut t = t0 + dt;
    for i in 1..=steps.max(1) {
        let d = d0 + (d1 - d0) * i as f32 / steps.max(1) as f32;
        out.push(finger(PointerAction::Move, 10, cx - d / 2.0, cy, t, 40.0));
        out.push(finger(PointerAction::Move, 11, cx + d / 2.0, cy, t, 40.0));
        t += dt;
    }
    out.push(finger(PointerAction::PointerUp, 11, cx + d1 / 2.0, cy, t, 40.0));
    out.push(finger(PointerAction::Up, 10, cx - d1 / 2.0, cy, t + dt, 40.0));
    out
}

/// A palm-sized finger contact (touch_major `major` px) that slides `dx` and lifts.
pub fn palm(x: f32, y: f32, dx: f32, major: f32, t0: u64) -> Vec<RawPointerEvent> {
    let dt = 16_666_667u64;
    let mut out = vec![finger(PointerAction::Down, 20, x, y, t0, major)];
    for i in 1..=8 {
        out.push(finger(PointerAction::Move, 20, x + dx * i as f32 / 8.0, y + 3.0 * i as f32, t0 + dt * i, major));
    }
    let mut up = finger(PointerAction::Up, 20, x + dx, y + 24.0, t0 + dt * 9, major);
    up.canceled_flag = true;
    out.push(up);
    out
}

/// Palm resting on the glass while the stylus draws `pts`, then the palm lifts with
/// `FLAG_CANCELED` (Android 13+ palm rejection).
pub fn stylus_with_resting_palm(pts: &[(f32, f32, f32)], palm_at: (f32, f32), t0: u64) -> Vec<RawPointerEvent> {
    let dt = 16_666_667u64;
    let mut out = vec![finger(PointerAction::Down, 30, palm_at.0, palm_at.1, t0, 210.0)];
    out.push(finger(PointerAction::Move, 30, palm_at.0 + 2.0, palm_at.1 + 1.0, t0 + dt, 210.0));
    let s = stylus_stroke(pts, t0 + 2 * dt, 0, ToolType::Stylus, false, PointerAction::PointerDown);
    let t_end = s.last().map(|e| e.t_ns).unwrap_or(t0) + dt;
    out.extend(s);
    let mut up = finger(PointerAction::Up, 30, palm_at.0 + 2.0, palm_at.1 + 1.0, t_end, 210.0);
    up.canceled_flag = true;
    out.push(up);
    out
}
