//! Pointer arbitration and the stroke state machine (AND-03/04/05/06).
//!
//! # State machine
//! `Idle → Hover → Begin → Streaming → Commit|Cancel → Idle` (see [`ArbiterState`]), plus a
//! separate `Gesture` mode for finger navigation. Every [`InputAction::StrokeBegin`] is followed
//! by exactly one [`InputAction::StrokeCommit`] or [`InputAction::StrokeCancel`]; samples and
//! predictions are only emitted between them; no stroke action is emitted while a navigation
//! gesture is active (between its first `ViewChanged` and `GestureEnd`).
//!
//! # Who draws
//! * Stylus and stylus-eraser draw (`eraser: true` for `TOOL_TYPE_ERASER`); mouse draws with
//!   fallback pressure.
//! * Fingers navigate once any stylus event (contact or hover) has been seen in this arbiter's
//!   lifetime, or when `finger_draws == false`. Otherwise a *single* finger draws.
//! * Palm heuristic: a finger whose `touch_major` exceeds `palm_major_threshold_px` (at down or
//!   on any later move) never draws and never navigates; if it was drawing, that stroke is
//!   **cancelled**. Android `FLAG_CANCELED` on `UP/POINTER_UP` also cancels (palm, Android 13+).
//! * A second finger during a finger stroke cancels that stroke (never commits) and starts a
//!   navigation gesture with both fingers. A stylus touching down during a finger stroke cancels
//!   the finger stroke and draws. A finger touching down during a stylus stroke is ignored until
//!   lifted.
//! * Stylus down during a finger gesture: [`StylusGesturePolicy::StylusWins`] (default) ends the
//!   gesture (`GestureEnd` iff it emitted a `ViewChanged`), the fingers are ignored until lifted and
//!   the stylus draws; [`StylusGesturePolicy::IgnoreStylus`] ignores that stylus contact.
//!
//! # Samples
//! * Only confirmed (non-predicted) events produce [`InputAction::StrokeSamples`], converted to
//!   document coordinates with the view's `view_to_doc`. Timestamps within a stroke are
//!   non-decreasing: out-of-order events are **dropped and counted**
//!   ([`InputStats::out_of_order_dropped`]), never reordered.
//! * No duplicate initial/up samples: the down event produces the first sample; an `UP` at the
//!   same view position as the last sample adds nothing; a `MOVE` identical in position *and*
//!   timestamp to the last sample is dropped. A zero-length tap therefore yields a valid
//!   **single-sample stroke** (a dot), which is committed.
//! * Pressure: stylus hardware pressure is clamped to `[0,1]` (clamps counted), mapped through
//!   the configured [`PressureCurve`] and flagged `PRESSURE_REPORTED`. When pressure is not
//!   reported (NaN), or the tool is a finger or mouse (whose "pressure" is not a pen force), the
//!   documented constant [`FALLBACK_PRESSURE`] = 0.5 is used **without** `PRESSURE_REPORTED`
//!   and without applying the curve.
//! * Tilt and azimuth are only passed for stylus tools and only when reported; absent → `None`.
//!   Tilt is clamped to `[0, π/2]`. Azimuth is Android `AXIS_ORIENTATION` re-expressed in the
//!   document frame (0 = toward document −y, clockwise positive), so view rotation/mirroring do
//!   not change the recorded pen direction. Barrel rotation is not in the packed layout → `None`.
//! * Predicted events produce [`InputAction::StrokePrediction`] only (flag `PREDICTED`, never
//!   `CONFIRMED`). A prediction is valid until the next `StrokeSamples`, `StrokeCommit` or
//!   `StrokeCancel`; each `StrokePrediction` replaces the previous one. Use
//!   [`InputArbiter::handle_batch`] so all predicted points of one MotionEvent arrive together.
//!
//! # Interruption policies
//! * `ACTION_CANCEL` / `FLAG_CANCELED` → **StrokeCancel** (the stroke is discarded).
//! * Lifecycle pause ([`InputArbiter::reset`]) → **StrokeCommit** of the confirmed samples already
//!   emitted (the user saw them; they are real hardware samples), `GestureEnd` for an active
//!   gesture, `Hover(None)` and release of held stylus buttons. Pointers still down after resume
//!   are ignored until a new `DOWN`.
//! * Protocol anomalies (a `DOWN` while pointers are tracked, a repeated `DOWN` for a tracked
//!   id, an `UP` leaving other pointers tracked) are treated as lost `UP`s: the pending stroke is
//!   committed, consistent with the lifecycle policy, and counted in
//!   [`InputStats::protocol_anomalies`].
//!
//! # Gestures
//! One finger pans (if `one_finger_pan`), two fingers pan/zoom/rotate around a stable anchor
//! (see [`ViewGesture`]); with three or more fingers the first two by touch order drive the
//! view. Nothing changes until a finger moves more than `touch_slop_px`; after that the
//! transform is solved from the touch-down baseline, so fingers stay pinned to the document
//! points they touched. Two/three fingers lifted within `tap_max_duration_ns` with less than
//! `tap_max_move_px` movement and no view change → `TwoFingerTap` / `ThreeFingerTap`.
//!
//! # Stylus buttons and hover
//! Stylus hover emits `Hover(Some(doc point))`, `HOVER_EXIT` emits `Hover(None)`. Button
//! transitions of `BUTTON_STYLUS_PRIMARY/SECONDARY` emit `StylusButton` with the configured
//! action; nothing is emitted for `ButtonAction::None` or when the device never reports buttons.

use crate::event::android::{BUTTON_STYLUS_PRIMARY, BUTTON_STYLUS_SECONDARY};
use crate::{GestureConfig, PointerAction, PressureCurve, RawPointerEvent, ViewGesture, ViewTransform};
use serde::{Deserialize, Serialize};
use std::f32::consts::FRAC_PI_2;
use veskra_core::limits::MAX_STROKE_SAMPLES;
use veskra_core::{Affine, PointF, SampleFlags, StrokeSample, ToolType};

/// Pressure used when the device/tool reports none. Mid-range so pressure-mapped brush
/// dynamics sit at their midpoint; marked by the absence of `PRESSURE_REPORTED`.
pub const FALLBACK_PRESSURE: f32 = 0.5;
/// Simultaneously tracked pointers; further pointers are ignored (bounded state).
pub const MAX_POINTERS: usize = 16;
pub const STYLUS_BUTTON_MASK: u32 = BUTTON_STYLUS_PRIMARY | BUTTON_STYLUS_SECONDARY;

/// Action bound to a stylus barrel button. Interpreted by the UI/engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ButtonAction {
    #[default]
    None,
    Eyedropper,
    ToggleEraser,
    Undo,
}

/// What a stylus contact does while a finger navigation gesture is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum StylusGesturePolicy {
    /// End the gesture, ignore the fingers until lifted, draw with the stylus.
    #[default]
    StylusWins,
    /// Keep navigating; ignore the stylus contact until it lifts.
    IgnoreStylus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// A single finger draws while no stylus has been seen.
    pub finger_draws: bool,
    /// Finger contacts with `touch_major` above this (view px) are treated as palm. The
    /// Android layer should derive it from display density (default ≈ 15 mm at 170 dpi).
    pub palm_major_threshold_px: f32,
    pub pressure_curve: PressureCurve,
    /// Action for `BUTTON_STYLUS_PRIMARY`.
    pub stylus_button_action: ButtonAction,
    /// Action for `BUTTON_STYLUS_SECONDARY`.
    pub stylus_secondary_button_action: ButtonAction,
    pub stylus_during_gesture: StylusGesturePolicy,
    pub one_finger_pan: bool,
    /// Finger movement (view px) before a gesture starts changing the view.
    pub touch_slop_px: f32,
    pub tap_max_duration_ns: u64,
    pub tap_max_move_px: f32,
    pub gesture: GestureConfig,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            finger_draws: true,
            palm_major_threshold_px: 100.0,
            pressure_curve: PressureCurve::identity(),
            stylus_button_action: ButtonAction::None,
            stylus_secondary_button_action: ButtonAction::None,
            stylus_during_gesture: StylusGesturePolicy::StylusWins,
            one_finger_pan: true,
            touch_slop_px: 10.0,
            tap_max_duration_ns: 250_000_000,
            tap_max_move_px: 10.0,
            gesture: GestureConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    StrokeBegin {
        tool: ToolType,
        eraser: bool,
    },
    /// CONFIRMED only, document coords, ordered, monotonic non-decreasing `t_ns`.
    StrokeSamples(Vec<StrokeSample>),
    /// PREDICTED only; replaces the previous prediction.
    StrokePrediction(Vec<StrokeSample>),
    StrokeCommit,
    StrokeCancel,
    /// Hover cursor in document coords; `None` = hide.
    Hover(Option<PointF>),
    ViewChanged(ViewTransform),
    GestureEnd,
    StylusButton {
        pressed: bool,
        action: ButtonAction,
    },
    TwoFingerTap,
    ThreeFingerTap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArbiterState {
    Idle,
    Hover,
    /// Stroke started; only the down sample has been emitted.
    Begin,
    Streaming,
    Gesture,
}

/// Counters for dropped/repaired input (exposed for diagnostics and tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InputStats {
    pub events: u64,
    pub strokes_begun: u64,
    pub strokes_committed: u64,
    pub strokes_cancelled: u64,
    pub confirmed_samples: u64,
    pub predicted_samples: u64,
    /// Predicted events outside an active stroke or for another pointer.
    pub predicted_dropped: u64,
    pub out_of_order_dropped: u64,
    pub duplicate_samples_dropped: u64,
    pub pressure_clamped: u64,
    /// Events/samples with non-finite positions or invalid derived samples.
    pub invalid_dropped: u64,
    pub palms_rejected: u64,
    /// Move/up events for pointers that are not tracked (e.g. after `reset`).
    pub orphan_events: u64,
    /// Contacts ignored by arbitration (unknown tool, finger during pen stroke, ...).
    pub ignored_events: u64,
    pub protocol_anomalies: u64,
    /// Navigation gestures that changed the view.
    pub gestures: u64,
    pub two_finger_taps: u64,
    pub three_finger_taps: u64,
    /// Strokes committed early because they reached `MAX_STROKE_SAMPLES`.
    pub sample_limit_commits: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Stroke,
    Gesture,
    Ignored,
}

#[derive(Debug, Clone, Copy)]
struct Pointer {
    id: i32,
    tool: ToolType,
    down: PointF,
    last: PointF,
    max_dist: f64,
    palm: bool,
    role: Role,
}

#[derive(Debug, Clone, Copy)]
struct StrokeState {
    pointer_id: i32,
    tool: ToolType,
    last_t: u64,
    last_xy: (f32, f32),
    samples: usize,
    streaming: bool,
}

#[derive(Debug, Clone)]
struct GestureState {
    /// Gesture fingers in touch order.
    ids: Vec<i32>,
    solver: Option<(ViewGesture, i32, Option<i32>)>,
    current: ViewTransform,
    /// Slop exceeded: the gesture is a navigation (disqualifies taps).
    transformed: bool,
    /// At least one `ViewChanged` emitted (a `GestureEnd` is owed).
    emitted: bool,
}

#[derive(Debug, Clone)]
enum Mode {
    Idle,
    Stroke(StrokeState),
    Gesture(GestureState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Idle,
    FingerStroke(i32),
    PenStroke,
    Gesture,
}

#[derive(Debug, Clone, Copy)]
struct FingerSession {
    start_t: u64,
    active: u32,
    max_fingers: u32,
    max_move: f64,
    disqualified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Commit,
    Cancel,
}

fn is_stylus(t: ToolType) -> bool {
    matches!(t, ToolType::Stylus | ToolType::StylusEraser)
}

fn push_sample(out: &mut Vec<InputAction>, s: StrokeSample) {
    if let Some(InputAction::StrokeSamples(v)) = out.last_mut() {
        v.push(s);
    } else {
        out.push(InputAction::StrokeSamples(vec![s]));
    }
}

fn push_prediction(out: &mut Vec<InputAction>, s: StrokeSample) {
    if let Some(InputAction::StrokePrediction(v)) = out.last_mut() {
        v.push(s);
    } else {
        out.push(InputAction::StrokePrediction(vec![s]));
    }
}

fn push_view(out: &mut Vec<InputAction>, v: ViewTransform) {
    if let Some(InputAction::ViewChanged(old)) = out.last_mut() {
        *old = v;
    } else {
        out.push(InputAction::ViewChanged(v));
    }
}

fn push_hover(out: &mut Vec<InputAction>, h: Option<PointF>) {
    if let Some(InputAction::Hover(old)) = out.last_mut() {
        *old = h;
    } else {
        out.push(InputAction::Hover(h));
    }
}

/// Android orientation (view frame) → azimuth in the document frame.
fn orientation_to_azimuth(o: f32, inv: &Affine) -> Option<f32> {
    let (s, c) = f64::from(o).sin_cos();
    let (ux, uy) = (s, -c);
    let (dx, dy) = (inv.a * ux + inv.c * uy, inv.b * ux + inv.d * uy);
    let len = dx.hypot(dy);
    if !(len.is_finite() && len > 0.0) {
        return None;
    }
    Some(dx.atan2(-dy) as f32)
}

/// Headless pointer arbiter. Feed it every packed event in order; apply the returned actions.
#[derive(Debug, Clone)]
pub struct InputArbiter {
    cfg: InputConfig,
    mode: Mode,
    pointers: Vec<Pointer>,
    hovering: bool,
    stylus_seen: bool,
    stylus_buttons: u32,
    session: Option<FingerSession>,
    stats: InputStats,
}

impl InputArbiter {
    pub fn new(cfg: InputConfig) -> Self {
        Self {
            cfg,
            mode: Mode::Idle,
            pointers: Vec::new(),
            hovering: false,
            stylus_seen: false,
            stylus_buttons: 0,
            session: None,
            stats: InputStats::default(),
        }
    }

    pub fn config(&self) -> &InputConfig {
        &self.cfg
    }

    /// Replace the configuration; takes effect for the next event.
    pub fn set_config(&mut self, cfg: InputConfig) {
        self.cfg = cfg;
    }

    pub fn stats(&self) -> &InputStats {
        &self.stats
    }

    pub fn stylus_seen(&self) -> bool {
        self.stylus_seen
    }

    pub fn state(&self) -> ArbiterState {
        match &self.mode {
            Mode::Stroke(s) if s.streaming => ArbiterState::Streaming,
            Mode::Stroke(_) => ArbiterState::Begin,
            Mode::Gesture(_) => ArbiterState::Gesture,
            Mode::Idle if self.hovering => ArbiterState::Hover,
            Mode::Idle => ArbiterState::Idle,
        }
    }

    /// Process one event. `view` must be the current view (apply `ViewChanged` before the next
    /// call). Prefer [`Self::handle_batch`] for whole MotionEvents.
    pub fn handle(&mut self, ev: &RawPointerEvent, view: &ViewTransform) -> Vec<InputAction> {
        let mut out = Vec::new();
        self.handle_batch_into(std::slice::from_ref(ev), view, &mut out);
        out
    }

    /// Process a batch. `view` is the view at batch start; `ViewChanged` emitted within the batch
    /// is applied to later events. Consecutive samples/predictions are coalesced.
    pub fn handle_batch(&mut self, events: &[RawPointerEvent], view: &ViewTransform) -> Vec<InputAction> {
        let mut out = Vec::new();
        self.handle_batch_into(events, view, &mut out);
        out
    }

    /// As [`Self::handle_batch`], appending to a caller-owned buffer (no per-batch allocation).
    pub fn handle_batch_into(&mut self, events: &[RawPointerEvent], view: &ViewTransform, out: &mut Vec<InputAction>) {
        let mut v = *view;
        for ev in events {
            self.process(ev, &mut v, out);
        }
    }

    /// Lifecycle pause/stop: commit the pending stroke (policy in module docs), end gestures,
    /// hide hover, release held stylus buttons and forget all pointers.
    pub fn reset(&mut self) -> Vec<InputAction> {
        let mut out = Vec::new();
        self.end_stroke(End::Commit, &mut out);
        self.end_gesture(&mut out);
        if self.hovering {
            self.hovering = false;
            push_hover(&mut out, None);
        }
        self.update_buttons(0, &mut out);
        self.pointers.clear();
        self.session = None;
        out
    }

    // ---- dispatch -------------------------------------------------------------------------

    fn process(&mut self, ev: &RawPointerEvent, view: &mut ViewTransform, out: &mut Vec<InputAction>) {
        self.stats.events += 1;
        let finite = ev.x.is_finite() && ev.y.is_finite();
        let needs_pos = matches!(
            ev.action,
            PointerAction::Down
                | PointerAction::PointerDown
                | PointerAction::Move
                | PointerAction::HoverEnter
                | PointerAction::HoverMove
        );
        if !finite && needs_pos {
            self.stats.invalid_dropped += 1;
            return;
        }
        if is_stylus(ev.tool) {
            self.stylus_seen = true;
            if !ev.predicted {
                self.update_buttons(ev.buttons, out);
            }
        }
        if ev.predicted {
            self.on_predicted(ev, view, out);
            return;
        }
        self.sync_gesture_view(view);
        match ev.action {
            PointerAction::HoverEnter | PointerAction::HoverMove => self.on_hover(ev, view, out),
            PointerAction::HoverExit => {
                if self.hovering {
                    self.hovering = false;
                    push_hover(out, None);
                }
            }
            PointerAction::Down => {
                if !self.pointers.is_empty() {
                    self.stats.protocol_anomalies += 1;
                    self.abandon_all(out);
                }
                self.on_down(ev, view, out);
            }
            PointerAction::PointerDown => self.on_down(ev, view, out),
            PointerAction::Move => self.on_move(ev, view, out),
            PointerAction::PointerUp => self.on_up(ev, view, out),
            PointerAction::Up => {
                self.on_up(ev, view, out);
                if !self.pointers.is_empty() {
                    self.stats.protocol_anomalies += 1;
                    self.abandon_all(out);
                }
            }
            PointerAction::Cancel => self.cancel_all(out),
        }
    }

    fn ctx(&self) -> Ctx {
        match &self.mode {
            Mode::Idle => Ctx::Idle,
            Mode::Stroke(s) if s.tool == ToolType::Finger => Ctx::FingerStroke(s.pointer_id),
            Mode::Stroke(_) => Ctx::PenStroke,
            Mode::Gesture(_) => Ctx::Gesture,
        }
    }

    fn find(&self, id: i32) -> Option<usize> {
        self.pointers.iter().position(|p| p.id == id)
    }

    fn set_role(&mut self, id: i32, role: Role) {
        if let Some(p) = self.pointers.iter_mut().find(|p| p.id == id) {
            p.role = role;
        }
    }

    fn is_palm(&self, ev: &RawPointerEvent) -> bool {
        ev.tool == ToolType::Finger
            && ev.touch_major.is_some_and(|m| m.is_finite() && m > self.cfg.palm_major_threshold_px)
    }

    // ---- buttons / hover --------------------------------------------------------------------

    fn update_buttons(&mut self, buttons: u32, out: &mut Vec<InputAction>) {
        let b = buttons & STYLUS_BUTTON_MASK;
        let changed = b ^ self.stylus_buttons;
        self.stylus_buttons = b;
        if changed == 0 {
            return;
        }
        for (mask, action) in [
            (BUTTON_STYLUS_PRIMARY, self.cfg.stylus_button_action),
            (BUTTON_STYLUS_SECONDARY, self.cfg.stylus_secondary_button_action),
        ] {
            if changed & mask != 0 && action != ButtonAction::None {
                out.push(InputAction::StylusButton { pressed: b & mask != 0, action });
            }
        }
    }

    fn on_hover(&mut self, ev: &RawPointerEvent, view: &ViewTransform, out: &mut Vec<InputAction>) {
        if !(is_stylus(ev.tool) || ev.tool == ToolType::Mouse) || matches!(self.mode, Mode::Stroke(_)) {
            self.stats.ignored_events += 1;
            return;
        }
        let d = view.view_to_doc().apply(PointF::new(f64::from(ev.x), f64::from(ev.y)));
        if !d.is_finite() {
            self.stats.invalid_dropped += 1;
            return;
        }
        self.hovering = true;
        push_hover(out, Some(d));
    }

    // ---- contacts ---------------------------------------------------------------------------

    fn on_down(&mut self, ev: &RawPointerEvent, view: &mut ViewTransform, out: &mut Vec<InputAction>) {
        if let Some(i) = self.find(ev.pointer_id) {
            // Repeated DOWN for a tracked pointer: its UP was lost.
            self.stats.protocol_anomalies += 1;
            let p = self.pointers.remove(i);
            self.release(p, None, view, out);
        }
        if self.pointers.len() >= MAX_POINTERS {
            self.stats.ignored_events += 1;
            return;
        }
        let pos = PointF::new(f64::from(ev.x), f64::from(ev.y));
        let p = Pointer {
            id: ev.pointer_id,
            tool: ev.tool,
            down: pos,
            last: pos,
            max_dist: 0.0,
            palm: false,
            role: Role::Ignored,
        };
        match ev.tool {
            ToolType::Finger => self.finger_down(p, ev, view, out),
            ToolType::Stylus | ToolType::StylusEraser | ToolType::Mouse => self.pen_down(p, ev, view, out),
            _ => {
                self.stats.ignored_events += 1;
                self.pointers.push(p);
            }
        }
    }

    fn finger_down(&mut self, mut p: Pointer, ev: &RawPointerEvent, view: &ViewTransform, out: &mut Vec<InputAction>) {
        if self.is_palm(ev) {
            p.palm = true;
            self.stats.palms_rejected += 1;
            self.pointers.push(p);
            return;
        }
        let ctx = self.ctx();
        self.session_finger_down(ev.t_ns, ctx == Ctx::PenStroke);
        match ctx {
            Ctx::Idle => {
                let other_fingers = self.pointers.iter().any(|q| q.tool == ToolType::Finger && !q.palm);
                if self.cfg.finger_draws && !self.stylus_seen && !other_fingers {
                    p.role = Role::Stroke;
                    self.pointers.push(p);
                    self.begin_stroke(ev, view, out);
                } else {
                    p.role = Role::Gesture;
                    self.pointers.push(p);
                    self.start_gesture(vec![p.id], view);
                }
            }
            Ctx::FingerStroke(sid) => {
                // Second finger: the first finger's stroke is cancelled, never committed.
                self.end_stroke(End::Cancel, out);
                self.set_role(sid, Role::Gesture);
                p.role = Role::Gesture;
                self.pointers.push(p);
                self.start_gesture(vec![sid, p.id], view);
            }
            Ctx::PenStroke => {
                self.stats.ignored_events += 1;
                self.pointers.push(p);
            }
            Ctx::Gesture => {
                p.role = Role::Gesture;
                self.pointers.push(p);
                if let Mode::Gesture(g) = &mut self.mode {
                    g.ids.push(p.id);
                }
                self.rebaseline();
            }
        }
    }

    fn pen_down(&mut self, mut p: Pointer, ev: &RawPointerEvent, view: &ViewTransform, out: &mut Vec<InputAction>) {
        if let Some(s) = &mut self.session {
            s.disqualified = true;
        }
        let draw = match self.ctx() {
            Ctx::Idle => true,
            Ctx::FingerStroke(_) if is_stylus(ev.tool) => {
                self.end_stroke(End::Cancel, out);
                true
            }
            Ctx::FingerStroke(_) | Ctx::PenStroke => false,
            Ctx::Gesture => match self.cfg.stylus_during_gesture {
                StylusGesturePolicy::StylusWins => {
                    self.end_gesture(out);
                    true
                }
                StylusGesturePolicy::IgnoreStylus => false,
            },
        };
        if draw {
            p.role = Role::Stroke;
            self.pointers.push(p);
            self.begin_stroke(ev, view, out);
        } else {
            self.stats.ignored_events += 1;
            self.pointers.push(p);
        }
    }

    fn on_move(&mut self, ev: &RawPointerEvent, view: &mut ViewTransform, out: &mut Vec<InputAction>) {
        let Some(i) = self.find(ev.pointer_id) else {
            self.stats.orphan_events += 1;
            return;
        };
        let pos = PointF::new(f64::from(ev.x), f64::from(ev.y));
        let Some(p) = self.pointers.get_mut(i) else { return };
        p.last = pos;
        p.max_dist = p.max_dist.max(p.down.distance(pos));
        let p = *p;
        if p.tool == ToolType::Finger && !p.palm {
            if self.is_palm(ev) {
                self.become_palm(i, out);
                return;
            }
            if let Some(s) = &mut self.session {
                s.max_move = s.max_move.max(p.max_dist);
            }
        }
        match p.role {
            Role::Stroke => self.stroke_sample(ev, view, out, false),
            Role::Gesture => self.gesture_update(view, out),
            Role::Ignored => {}
        }
    }

    fn become_palm(&mut self, i: usize, out: &mut Vec<InputAction>) {
        let Some(p) = self.pointers.get_mut(i) else { return };
        let (id, role) = (p.id, p.role);
        p.palm = true;
        p.role = Role::Ignored;
        self.stats.palms_rejected += 1;
        if let Some(s) = &mut self.session {
            s.disqualified = true;
            s.active = s.active.saturating_sub(1);
            if s.active == 0 {
                self.session = None;
            }
        }
        match role {
            Role::Stroke => self.end_stroke(End::Cancel, out),
            Role::Gesture => self.gesture_remove(id, out),
            Role::Ignored => {}
        }
    }

    fn on_up(&mut self, ev: &RawPointerEvent, view: &ViewTransform, out: &mut Vec<InputAction>) {
        match self.find(ev.pointer_id) {
            Some(i) => {
                let p = self.pointers.remove(i);
                self.release(p, Some(ev), view, out);
            }
            None => self.stats.orphan_events += 1,
        }
    }

    /// A tracked pointer lifted (`ev` is `None` when its UP was lost: commit, no sample, no tap).
    fn release(&mut self, p: Pointer, ev: Option<&RawPointerEvent>, view: &ViewTransform, out: &mut Vec<InputAction>) {
        let canceled = ev.is_some_and(|e| e.canceled_flag);
        match p.role {
            Role::Stroke => {
                let ours = matches!(&self.mode, Mode::Stroke(s) if s.pointer_id == p.id);
                if ours {
                    if canceled {
                        self.end_stroke(End::Cancel, out);
                    } else {
                        if let Some(e) = ev {
                            self.stroke_sample(e, view, out, true);
                        }
                        self.end_stroke(End::Commit, out);
                    }
                }
            }
            Role::Gesture => self.gesture_remove(p.id, out),
            Role::Ignored => {}
        }
        if p.tool == ToolType::Finger && !p.palm {
            self.session_finger_up(ev.map(|e| e.t_ns), canceled || ev.is_none(), out);
        }
    }

    /// Lost-UP handling: commit pending stroke, end gesture, forget pointers.
    fn abandon_all(&mut self, out: &mut Vec<InputAction>) {
        self.end_stroke(End::Commit, out);
        self.end_gesture(out);
        self.pointers.clear();
        self.session = None;
    }

    /// `ACTION_CANCEL`: discard pending stroke, end gesture, forget pointers.
    fn cancel_all(&mut self, out: &mut Vec<InputAction>) {
        self.end_stroke(End::Cancel, out);
        self.end_gesture(out);
        self.pointers.clear();
        self.session = None;
    }

    // ---- strokes ----------------------------------------------------------------------------

    fn make_sample(&mut self, ev: &RawPointerEvent, view: &ViewTransform, confirmed: bool) -> Option<StrokeSample> {
        let inv = view.view_to_doc();
        let d = inv.apply(PointF::new(f64::from(ev.x), f64::from(ev.y)));
        let (x, y) = (d.x as f32, d.y as f32);
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let stylus = is_stylus(ev.tool);
        let mut flags = if confirmed { SampleFlags::CONFIRMED } else { SampleFlags::PREDICTED };
        let pressure = match ev.pressure {
            Some(p) if stylus && p.is_finite() => {
                let c = p.clamp(0.0, 1.0);
                if c != p {
                    self.stats.pressure_clamped += 1;
                }
                flags.insert(SampleFlags::PRESSURE_REPORTED);
                self.cfg.pressure_curve.eval(c)
            }
            _ => FALLBACK_PRESSURE,
        };
        let (tilt, azimuth) = if stylus {
            (
                ev.tilt.filter(|t| t.is_finite()).map(|t| t.clamp(0.0, FRAC_PI_2)),
                ev.orientation.filter(|o| o.is_finite()).and_then(|o| orientation_to_azimuth(o, &inv)),
            )
        } else {
            (None, None)
        };
        if confirmed && ev.historical {
            flags.insert(SampleFlags::HISTORICAL);
        }
        if stylus {
            if ev.buttons & BUTTON_STYLUS_PRIMARY != 0 {
                flags.insert(SampleFlags::BUTTON_PRIMARY);
            }
            if ev.buttons & BUTTON_STYLUS_SECONDARY != 0 {
                flags.insert(SampleFlags::BUTTON_SECONDARY);
            }
        }
        let s = StrokeSample { x, y, t_ns: ev.t_ns, pressure, tilt, azimuth, rotation: None, flags };
        s.validate().ok().map(|()| s)
    }

    fn begin_stroke(&mut self, ev: &RawPointerEvent, view: &ViewTransform, out: &mut Vec<InputAction>) {
        let Some(sample) = self.make_sample(ev, view, true) else {
            self.stats.invalid_dropped += 1;
            self.set_role(ev.pointer_id, Role::Ignored);
            return;
        };
        if self.hovering {
            self.hovering = false;
            push_hover(out, None);
        }
        out.push(InputAction::StrokeBegin { tool: ev.tool, eraser: ev.tool == ToolType::StylusEraser });
        push_sample(out, sample);
        self.stats.strokes_begun += 1;
        self.stats.confirmed_samples += 1;
        self.mode = Mode::Stroke(StrokeState {
            pointer_id: ev.pointer_id,
            tool: ev.tool,
            last_t: ev.t_ns,
            last_xy: (ev.x, ev.y),
            samples: 1,
            streaming: false,
        });
    }

    fn stroke_sample(&mut self, ev: &RawPointerEvent, view: &ViewTransform, out: &mut Vec<InputAction>, is_up: bool) {
        let Mode::Stroke(s) = self.mode.clone() else { return };
        if s.pointer_id != ev.pointer_id {
            return;
        }
        if ev.t_ns < s.last_t {
            self.stats.out_of_order_dropped += 1;
            return;
        }
        if (ev.x, ev.y) == s.last_xy && (is_up || ev.t_ns == s.last_t) {
            self.stats.duplicate_samples_dropped += 1;
            return;
        }
        if s.samples >= MAX_STROKE_SAMPLES {
            self.stats.sample_limit_commits += 1;
            self.end_stroke(End::Commit, out);
            return;
        }
        let Some(sample) = self.make_sample(ev, view, true) else {
            self.stats.invalid_dropped += 1;
            return;
        };
        push_sample(out, sample);
        self.stats.confirmed_samples += 1;
        if let Mode::Stroke(st) = &mut self.mode {
            st.last_t = ev.t_ns;
            st.last_xy = (ev.x, ev.y);
            st.samples += 1;
            st.streaming = true;
        }
    }

    fn on_predicted(&mut self, ev: &RawPointerEvent, view: &ViewTransform, out: &mut Vec<InputAction>) {
        let ours = ev.action == PointerAction::Move
            && matches!(&self.mode, Mode::Stroke(s) if s.pointer_id == ev.pointer_id)
            && ev.x.is_finite()
            && ev.y.is_finite();
        if !ours {
            self.stats.predicted_dropped += 1;
            return;
        }
        match self.make_sample(ev, view, false) {
            Some(s) => {
                push_prediction(out, s);
                self.stats.predicted_samples += 1;
            }
            None => self.stats.invalid_dropped += 1,
        }
    }

    /// The single place a stroke ends: exactly one Commit or Cancel per Begin.
    fn end_stroke(&mut self, how: End, out: &mut Vec<InputAction>) {
        let Mode::Stroke(s) = &self.mode else { return };
        let id = s.pointer_id;
        self.mode = Mode::Idle;
        match how {
            End::Commit => {
                out.push(InputAction::StrokeCommit);
                self.stats.strokes_committed += 1;
            }
            End::Cancel => {
                out.push(InputAction::StrokeCancel);
                self.stats.strokes_cancelled += 1;
            }
        }
        self.set_role(id, Role::Ignored);
    }

    // ---- gestures ---------------------------------------------------------------------------

    fn start_gesture(&mut self, ids: Vec<i32>, view: &ViewTransform) {
        self.mode =
            Mode::Gesture(GestureState { ids, solver: None, current: *view, transformed: false, emitted: false });
        self.rebaseline();
    }

    /// Restart the solver from the current view and finger positions (finger set changed).
    fn rebaseline(&mut self) {
        let one_finger_pan = self.cfg.one_finger_pan;
        let Mode::Gesture(g) = &mut self.mode else { return };
        let pos = |id: i32| self.pointers.iter().find(|p| p.id == id).map(|p| p.last);
        let a = g.ids.first().copied();
        let b = g.ids.get(1).copied();
        g.solver = match (a, a.and_then(pos), b, b.and_then(pos)) {
            (Some(ia), Some(pa), Some(ib), Some(pb)) => {
                Some((ViewGesture::two_finger(&g.current, pa, pb), ia, Some(ib)))
            }
            (Some(ia), Some(pa), _, _) if one_finger_pan => Some((ViewGesture::one_finger(&g.current, pa), ia, None)),
            _ => None,
        };
    }

    /// The engine changed the view size mid-gesture (rotation / resize): adopt the caller's view
    /// and re-baseline from finger positions *before* the current event.
    fn sync_gesture_view(&mut self, view: &ViewTransform) {
        let resized = match &mut self.mode {
            Mode::Gesture(g) if (g.current.view_w, g.current.view_h) != (view.view_w, view.view_h) => {
                g.current = *view;
                true
            }
            _ => false,
        };
        if resized {
            self.rebaseline();
        }
    }

    fn gesture_update(&mut self, view: &mut ViewTransform, out: &mut Vec<InputAction>) {
        let slop = f64::from(self.cfg.touch_slop_px);
        let gcfg = self.cfg.gesture;
        let Mode::Gesture(g) = &mut self.mode else { return };
        if !g.transformed {
            let moved = g.ids.iter().any(|id| self.pointers.iter().any(|p| p.id == *id && p.max_dist > slop));
            if !moved {
                return;
            }
            g.transformed = true;
            if let Some(s) = &mut self.session {
                s.disqualified = true;
            }
        }
        let Some((solver, ia, ib)) = g.solver else { return };
        let pos = |id: i32| self.pointers.iter().find(|p| p.id == id).map(|p| p.last);
        let Some(p1) = pos(ia) else { return };
        let q1 = ib.and_then(pos);
        let nv = solver.solve(p1, q1, &gcfg);
        if nv != g.current {
            if !g.emitted {
                g.emitted = true;
                self.stats.gestures += 1;
            }
            g.current = nv;
            *view = nv;
            push_view(out, nv);
        }
    }

    fn gesture_remove(&mut self, id: i32, out: &mut Vec<InputAction>) {
        let empty = match &mut self.mode {
            Mode::Gesture(g) => {
                g.ids.retain(|&x| x != id);
                g.ids.is_empty()
            }
            _ => return,
        };
        if empty {
            self.end_gesture(out);
        } else {
            self.rebaseline();
        }
    }

    fn end_gesture(&mut self, out: &mut Vec<InputAction>) {
        let Mode::Gesture(g) = &self.mode else { return };
        let emitted = g.emitted;
        self.mode = Mode::Idle;
        if emitted {
            out.push(InputAction::GestureEnd);
        }
        for p in &mut self.pointers {
            if p.role == Role::Gesture {
                p.role = Role::Ignored;
            }
        }
    }

    // ---- finger taps ------------------------------------------------------------------------

    fn session_finger_down(&mut self, t: u64, pen_stroke: bool) {
        let s = self.session.get_or_insert(FingerSession {
            start_t: t,
            active: 0,
            max_fingers: 0,
            max_move: 0.0,
            disqualified: false,
        });
        s.active = s.active.saturating_add(1);
        s.max_fingers = s.max_fingers.max(s.active);
        if pen_stroke {
            s.disqualified = true;
        }
    }

    fn session_finger_up(&mut self, t: Option<u64>, disqualify: bool, out: &mut Vec<InputAction>) {
        let Some(s) = &mut self.session else { return };
        if disqualify {
            s.disqualified = true;
        }
        s.active = s.active.saturating_sub(1);
        if s.active > 0 {
            return;
        }
        let s = *s;
        self.session = None;
        let Some(t) = t else { return };
        if s.disqualified
            || t.saturating_sub(s.start_t) >= self.cfg.tap_max_duration_ns
            || s.max_move >= f64::from(self.cfg.tap_max_move_px)
        {
            return;
        }
        match s.max_fingers {
            2 => {
                out.push(InputAction::TwoFingerTap);
                self.stats.two_finger_taps += 1;
            }
            3 => {
                out.push(InputAction::ThreeFingerTap);
                self.stats.three_finger_taps += 1;
            }
            _ => {}
        }
    }
}
