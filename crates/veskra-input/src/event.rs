//! Raw pointer events and the packed JNI batch layout (CONTRACTS §8).
//!
//! # Packed layout
//! For event `i` of a batch of `count` events:
//! * `ints[i*5 .. i*5+5]   = [action, pointerId, toolType, buttons, flags]`
//!   (`flags`: bit0 historical, bit1 predicted, bit2 Android `FLAG_CANCELED`)
//! * `floats[i*8 .. i*8+8] = [x, y, pressure, tilt, orientation, touchMajor, touchMinor, reserved]`
//!   (`NaN` = axis not reported; `x`/`y` must be finite)
//! * `longs[i]             = eventTimeNanos` (non-negative)
//!
//! Arrays may be *longer* than `count` requires (Kotlin may reuse pooled buffers); trailing
//! entries are ignored. `action`/`toolType` use Android `MotionEvent` constant values.
//!
//! # Flattening expected from the Kotlin adapter
//! One `RawPointerEvent` per (pointer, sample): for `ACTION_MOVE`, every historical sample of
//! every pointer is sent with `historical = true` (oldest first), then each pointer's current
//! sample. `ACTION_DOWN/UP/POINTER_DOWN/POINTER_UP` are sent once for the *action* pointer.
//! `ACTION_CANCEL` may be sent once (any pointer id); it cancels the whole stream.
//! Motion predictions are sent as `Move` with `predicted = true`. Other actions
//! (`ACTION_OUTSIDE`, `ACTION_SCROLL`, `ACTION_BUTTON_*`) must not be packed: they are
//! rejected with [`InputError::UnknownAction`]. Button state travels in `buttons`.

use crate::InputError;
use serde::{Deserialize, Serialize};
use veskra_core::ToolType;

/// Android `MotionEvent` constants used by the packed layout.
pub mod android {
    pub const ACTION_DOWN: i32 = 0;
    pub const ACTION_UP: i32 = 1;
    pub const ACTION_MOVE: i32 = 2;
    pub const ACTION_CANCEL: i32 = 3;
    pub const ACTION_POINTER_DOWN: i32 = 5;
    pub const ACTION_POINTER_UP: i32 = 6;
    pub const ACTION_HOVER_MOVE: i32 = 7;
    pub const ACTION_HOVER_ENTER: i32 = 9;
    pub const ACTION_HOVER_EXIT: i32 = 10;

    pub const TOOL_TYPE_UNKNOWN: i32 = 0;
    pub const TOOL_TYPE_FINGER: i32 = 1;
    pub const TOOL_TYPE_STYLUS: i32 = 2;
    pub const TOOL_TYPE_MOUSE: i32 = 3;
    pub const TOOL_TYPE_ERASER: i32 = 4;

    /// `MotionEvent.BUTTON_STYLUS_PRIMARY`.
    pub const BUTTON_STYLUS_PRIMARY: u32 = 0x20;
    /// `MotionEvent.BUTTON_STYLUS_SECONDARY`.
    pub const BUTTON_STYLUS_SECONDARY: u32 = 0x40;
}

pub const INTS_PER_EVENT: usize = 5;
pub const FLOATS_PER_EVENT: usize = 8;
/// Packed `flags` bit: sample came from MotionEvent history.
pub const FLAG_HISTORICAL: i32 = 1 << 0;
/// Packed `flags` bit: sample is a motion prediction.
pub const FLAG_PREDICTED: i32 = 1 << 1;
/// Packed `flags` bit: Android `FLAG_CANCELED` (palm / accidental touch, Android 13+).
pub const FLAG_CANCELED: i32 = 1 << 2;
const KNOWN_FLAGS: i32 = FLAG_HISTORICAL | FLAG_PREDICTED | FLAG_CANCELED;
/// Upper bound on events in one JNI batch (bounded allocation).
pub const MAX_EVENTS_PER_BATCH: usize = 1 << 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PointerAction {
    Down,
    Move,
    Up,
    Cancel,
    PointerDown,
    PointerUp,
    HoverEnter,
    HoverMove,
    HoverExit,
}

impl PointerAction {
    /// Map `MotionEvent.getActionMasked()`; `None` for actions Veskra does not consume.
    pub fn from_android(action_masked: i32) -> Option<Self> {
        use android::*;
        Some(match action_masked {
            ACTION_DOWN => PointerAction::Down,
            ACTION_UP => PointerAction::Up,
            ACTION_MOVE => PointerAction::Move,
            ACTION_CANCEL => PointerAction::Cancel,
            ACTION_POINTER_DOWN => PointerAction::PointerDown,
            ACTION_POINTER_UP => PointerAction::PointerUp,
            ACTION_HOVER_MOVE => PointerAction::HoverMove,
            ACTION_HOVER_ENTER => PointerAction::HoverEnter,
            ACTION_HOVER_EXIT => PointerAction::HoverExit,
            _ => return None,
        })
    }

    pub fn to_android(self) -> i32 {
        use android::*;
        match self {
            PointerAction::Down => ACTION_DOWN,
            PointerAction::Up => ACTION_UP,
            PointerAction::Move => ACTION_MOVE,
            PointerAction::Cancel => ACTION_CANCEL,
            PointerAction::PointerDown => ACTION_POINTER_DOWN,
            PointerAction::PointerUp => ACTION_POINTER_UP,
            PointerAction::HoverMove => ACTION_HOVER_MOVE,
            PointerAction::HoverEnter => ACTION_HOVER_ENTER,
            PointerAction::HoverExit => ACTION_HOVER_EXIT,
        }
    }
}

/// Map `MotionEvent.getToolType()`. Unknown/future values map to [`ToolType::Unknown`]
/// (ignored by the arbiter) rather than failing the batch.
pub fn tool_from_android(tool_type: i32) -> ToolType {
    use android::*;
    match tool_type {
        TOOL_TYPE_FINGER => ToolType::Finger,
        TOOL_TYPE_STYLUS => ToolType::Stylus,
        TOOL_TYPE_MOUSE => ToolType::Mouse,
        TOOL_TYPE_ERASER => ToolType::StylusEraser,
        _ => ToolType::Unknown,
    }
}

pub fn tool_to_android(tool: ToolType) -> i32 {
    use android::*;
    match tool {
        ToolType::Finger => TOOL_TYPE_FINGER,
        ToolType::Stylus => TOOL_TYPE_STYLUS,
        ToolType::Mouse => TOOL_TYPE_MOUSE,
        ToolType::StylusEraser => TOOL_TYPE_ERASER,
        _ => TOOL_TYPE_UNKNOWN,
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// One pointer sample in VIEW coordinates (px), as delivered by the platform.
/// Absent axes are `None` (packed as NaN). Serialized compactly for JSONL traces:
/// `None`/`false`/`0` fields are omitted and default on read.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPointerEvent {
    pub action: PointerAction,
    pub pointer_id: i32,
    pub tool: ToolType,
    pub x: f32,
    pub y: f32,
    pub t_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure: Option<f32>,
    /// Radians from perpendicular (Android `AXIS_TILT`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt: Option<f32>,
    /// Radians, Android `AXIS_ORIENTATION` (0 = pointing to view top, clockwise positive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touch_major: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touch_minor: Option<f32>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub buttons: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub historical: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub predicted: bool,
    /// Android `FLAG_CANCELED` (palm rejection on Android 13+).
    #[serde(default, skip_serializing_if = "is_false")]
    pub canceled_flag: bool,
}

impl RawPointerEvent {
    /// Event with all optional axes absent and no flags.
    pub fn new(action: PointerAction, pointer_id: i32, tool: ToolType, x: f32, y: f32, t_ns: u64) -> Self {
        Self {
            action,
            pointer_id,
            tool,
            x,
            y,
            t_ns,
            pressure: None,
            tilt: None,
            orientation: None,
            touch_major: None,
            touch_minor: None,
            buttons: 0,
            historical: false,
            predicted: false,
            canceled_flag: false,
        }
    }

    /// Boundary validation shared by [`unpack_events`] and trace parsing.
    pub fn validate(&self) -> Result<(), InputError> {
        self.validate_at(0)
    }

    pub(crate) fn validate_at(&self, index: usize) -> Result<(), InputError> {
        if !self.x.is_finite() || !self.y.is_finite() {
            return Err(InputError::NonFinitePosition { index });
        }
        if self.historical && self.predicted {
            return Err(InputError::ContradictoryFlags { index });
        }
        let axes = [
            ("pressure", self.pressure),
            ("tilt", self.tilt),
            ("orientation", self.orientation),
            ("touch_major", self.touch_major),
            ("touch_minor", self.touch_minor),
        ];
        for (axis, v) in axes {
            if let Some(v) = v
                && !v.is_finite()
            {
                return Err(InputError::InfiniteAxis { index, axis });
            }
        }
        Ok(())
    }
}

/// Convert the JNI `count: Int` to a validated `usize`.
pub fn batch_count_from_jni(count: i32) -> Result<usize, InputError> {
    let c = usize::try_from(count).map_err(|_| InputError::NegativeCount(i64::from(count)))?;
    if c > MAX_EVENTS_PER_BATCH {
        return Err(InputError::TooManyEvents { count: c, max: MAX_EVENTS_PER_BATCH });
    }
    Ok(c)
}

fn axis(v: f32, index: usize, name: &'static str) -> Result<Option<f32>, InputError> {
    if v.is_nan() {
        Ok(None)
    } else if v.is_infinite() {
        Err(InputError::InfiniteAxis { index, axis: name })
    } else {
        Ok(Some(v))
    }
}

/// Decode a packed batch (CONTRACTS §8). Validates every length with checked arithmetic,
/// maps NaN axes to `None`, and rejects unknown actions/flags, non-finite positions,
/// contradictory flags and negative timestamps. Never panics on any input.
pub fn unpack_events(
    ints: &[i32],
    floats: &[f32],
    longs: &[i64],
    count: usize,
) -> Result<Vec<RawPointerEvent>, InputError> {
    if count > MAX_EVENTS_PER_BATCH {
        return Err(InputError::TooManyEvents { count, max: MAX_EVENTS_PER_BATCH });
    }
    let need_i = count.checked_mul(INTS_PER_EVENT).ok_or(InputError::Overflow)?;
    let need_f = count.checked_mul(FLOATS_PER_EVENT).ok_or(InputError::Overflow)?;
    if ints.len() < need_i {
        return Err(InputError::ArrayTooShort { array: "ints", needed: need_i, got: ints.len() });
    }
    if floats.len() < need_f {
        return Err(InputError::ArrayTooShort { array: "floats", needed: need_f, got: floats.len() });
    }
    if longs.len() < count {
        return Err(InputError::ArrayTooShort { array: "longs", needed: count, got: longs.len() });
    }
    let mut out = Vec::with_capacity(count);
    for ((index, ic), (fc, &t)) in
        ints.chunks_exact(INTS_PER_EVENT).enumerate().zip(floats.chunks_exact(FLOATS_PER_EVENT).zip(longs)).take(count)
    {
        let [action, pointer_id, tool, buttons, flags]: [i32; INTS_PER_EVENT] =
            ic.try_into().map_err(|_| InputError::Overflow)?;
        let [x, y, pressure, tilt, orientation, major, minor, _reserved]: [f32; FLOATS_PER_EVENT] =
            fc.try_into().map_err(|_| InputError::Overflow)?;
        let action = PointerAction::from_android(action).ok_or(InputError::UnknownAction { index, value: action })?;
        if flags & !KNOWN_FLAGS != 0 {
            return Err(InputError::UnknownFlags { index, bits: flags & !KNOWN_FLAGS });
        }
        let t_ns = u64::try_from(t).map_err(|_| InputError::NegativeTimestamp { index })?;
        let ev = RawPointerEvent {
            action,
            pointer_id,
            tool: tool_from_android(tool),
            x,
            y,
            t_ns,
            pressure: axis(pressure, index, "pressure")?,
            tilt: axis(tilt, index, "tilt")?,
            orientation: axis(orientation, index, "orientation")?,
            touch_major: axis(major, index, "touch_major")?,
            touch_minor: axis(minor, index, "touch_minor")?,
            buttons: buttons as u32, // bit pattern of the Java int
            historical: flags & FLAG_HISTORICAL != 0,
            predicted: flags & FLAG_PREDICTED != 0,
            canceled_flag: flags & FLAG_CANCELED != 0,
        };
        ev.validate_at(index)?;
        out.push(ev);
    }
    Ok(out)
}

/// Inverse of [`unpack_events`] (tests, tooling and replay harnesses). Absent axes become NaN.
pub fn pack_events(events: &[RawPointerEvent]) -> (Vec<i32>, Vec<f32>, Vec<i64>) {
    let mut ints = Vec::with_capacity(events.len().saturating_mul(INTS_PER_EVENT));
    let mut floats = Vec::with_capacity(events.len().saturating_mul(FLOATS_PER_EVENT));
    let mut longs = Vec::with_capacity(events.len());
    for e in events {
        let flags = (if e.historical { FLAG_HISTORICAL } else { 0 })
            | (if e.predicted { FLAG_PREDICTED } else { 0 })
            | (if e.canceled_flag { FLAG_CANCELED } else { 0 });
        ints.extend_from_slice(&[
            e.action.to_android(),
            e.pointer_id,
            tool_to_android(e.tool),
            e.buttons as i32,
            flags,
        ]);
        let n = |v: Option<f32>| v.unwrap_or(f32::NAN);
        floats.extend_from_slice(&[
            e.x,
            e.y,
            n(e.pressure),
            n(e.tilt),
            n(e.orientation),
            n(e.touch_major),
            n(e.touch_minor),
            f32::NAN,
        ]);
        longs.push(i64::try_from(e.t_ns).unwrap_or(i64::MAX));
    }
    (ints, floats, longs)
}

#[cfg(test)]
mod tests {
    use super::android::*;
    use super::*;

    #[test]
    fn action_mapping_roundtrip() {
        for v in [0, 1, 2, 3, 5, 6, 7, 9, 10] {
            let a = PointerAction::from_android(v).unwrap();
            assert_eq!(a.to_android(), v);
        }
        for v in [4, 8, 11, 12, -1, 255] {
            assert!(PointerAction::from_android(v).is_none());
        }
        assert_eq!(PointerAction::from_android(ACTION_POINTER_UP), Some(PointerAction::PointerUp));
    }

    #[test]
    fn tool_mapping() {
        assert_eq!(tool_from_android(TOOL_TYPE_UNKNOWN), ToolType::Unknown);
        assert_eq!(tool_from_android(TOOL_TYPE_FINGER), ToolType::Finger);
        assert_eq!(tool_from_android(TOOL_TYPE_STYLUS), ToolType::Stylus);
        assert_eq!(tool_from_android(TOOL_TYPE_MOUSE), ToolType::Mouse);
        assert_eq!(tool_from_android(TOOL_TYPE_ERASER), ToolType::StylusEraser);
        assert_eq!(tool_from_android(99), ToolType::Unknown);
        for t in 0..5 {
            assert_eq!(tool_to_android(tool_from_android(t)), t);
        }
    }

    #[test]
    fn unpack_layout_and_nan() {
        let ints = [ACTION_MOVE, 7, TOOL_TYPE_STYLUS, 0x20, FLAG_HISTORICAL, ACTION_UP, 7, TOOL_TYPE_ERASER, 0, 4];
        let nan = f32::NAN;
        let floats = [1.5, 2.5, 0.75, 0.3, -1.0, 4.0, 3.0, 0.0, 10.0, 20.0, nan, nan, nan, nan, nan, nan];
        let longs = [100, 200];
        let ev = unpack_events(&ints, &floats, &longs, 2).unwrap();
        assert_eq!(ev.len(), 2);
        let a = ev[0];
        assert_eq!((a.action, a.pointer_id, a.tool, a.buttons), (PointerAction::Move, 7, ToolType::Stylus, 0x20));
        assert_eq!((a.x, a.y, a.t_ns), (1.5, 2.5, 100));
        assert_eq!((a.pressure, a.tilt, a.orientation), (Some(0.75), Some(0.3), Some(-1.0)));
        assert_eq!((a.touch_major, a.touch_minor), (Some(4.0), Some(3.0)));
        assert!(a.historical && !a.predicted && !a.canceled_flag);
        let b = ev[1];
        assert_eq!((b.action, b.tool), (PointerAction::Up, ToolType::StylusEraser));
        assert_eq!((b.pressure, b.tilt, b.orientation, b.touch_major, b.touch_minor), (None, None, None, None, None));
        assert!(b.canceled_flag && !b.historical);
        // pack -> unpack is lossless
        let (i2, f2, l2) = pack_events(&ev);
        assert_eq!(unpack_events(&i2, &f2, &l2, 2).unwrap(), ev);
        // zero events, longer pooled arrays are fine
        assert!(unpack_events(&ints, &floats, &longs, 0).unwrap().is_empty());
        assert_eq!(unpack_events(&ints, &floats, &longs, 1).unwrap().len(), 1);
    }

    #[test]
    fn unpack_rejects_bad_input() {
        let ints = [ACTION_DOWN, 0, 1, 0, 0];
        let floats = [0.0f32; 8];
        let longs = [0i64];
        assert!(matches!(unpack_events(&ints, &floats, &longs, 2), Err(InputError::ArrayTooShort { .. })));
        assert!(matches!(unpack_events(&ints[..4], &floats, &longs, 1), Err(InputError::ArrayTooShort { .. })));
        assert!(matches!(unpack_events(&ints, &floats[..7], &longs, 1), Err(InputError::ArrayTooShort { .. })));
        assert!(matches!(unpack_events(&ints, &floats, &[], 1), Err(InputError::ArrayTooShort { .. })));
        assert!(matches!(unpack_events(&ints, &floats, &longs, usize::MAX), Err(InputError::TooManyEvents { .. })));
        assert!(matches!(batch_count_from_jni(-1), Err(InputError::NegativeCount(-1))));
        assert!(matches!(batch_count_from_jni(i32::MAX), Err(InputError::TooManyEvents { .. })));
        assert_eq!(batch_count_from_jni(3).unwrap(), 3);
        let bad_action = [4, 0, 1, 0, 0];
        assert!(matches!(
            unpack_events(&bad_action, &floats, &longs, 1),
            Err(InputError::UnknownAction { value: 4, .. })
        ));
        let bad_flags = [0, 0, 1, 0, 8];
        assert!(matches!(unpack_events(&bad_flags, &floats, &longs, 1), Err(InputError::UnknownFlags { .. })));
        let both = [0, 0, 1, 0, 3];
        assert!(matches!(unpack_events(&both, &floats, &longs, 1), Err(InputError::ContradictoryFlags { .. })));
        assert!(matches!(unpack_events(&ints, &floats, &[-5], 1), Err(InputError::NegativeTimestamp { .. })));
        let mut f = floats;
        f[0] = f32::NAN;
        assert!(matches!(unpack_events(&ints, &f, &longs, 1), Err(InputError::NonFinitePosition { .. })));
        let mut f = floats;
        f[2] = f32::INFINITY;
        assert!(matches!(unpack_events(&ints, &f, &longs, 1), Err(InputError::InfiniteAxis { axis: "pressure", .. })));
    }
}
