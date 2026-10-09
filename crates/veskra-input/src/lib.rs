//! # veskra-input — pointer pipeline, stroke state machine and gesture arbitration (AND)
//!
//! Android delivers `MotionEvent`s to Kotlin, which packs them (CONTRACTS §8) and hands them to
//! Rust via JNI. Everything after packing is decided here, headless and testable:
//!
//! 1. [`unpack_events`] — validates and decodes the packed batch into [`RawPointerEvent`]s.
//! 2. [`InputArbiter`] — stylus/finger/palm arbitration and the stroke state machine
//!    (`Idle → Hover → Begin → Streaming → Commit|Cancel → Idle`), producing [`InputAction`]s:
//!    confirmed document-space [`veskra_core::StrokeSample`]s, presentation-only predictions,
//!    view changes, hover, stylus buttons and tap shortcuts.
//! 3. [`ViewTransform`] / [`ViewGesture`] — document ↔ view mapping and the stable-anchor
//!    two-finger pan/zoom/rotate solver.
//! 4. [`PressureCurve`] — monotone (Fritsch–Carlson) pressure response.
//!
//! All policies (fallback pressure, cancel vs. commit on interruption, palm heuristic,
//! duplicate-sample rules) are documented in [`arbiter`].
//!
//! Hardware note: logic is verified with SYNTHETIC traces (`tests/fixtures/input`). Real
//! device behaviour (SPK-02) has not been captured yet.

pub mod arbiter;
pub mod error;
pub mod event;
pub mod pressure;
pub mod trace;
pub mod view;

pub use arbiter::{
    ArbiterState, ButtonAction, FALLBACK_PRESSURE, InputAction, InputArbiter, InputConfig, InputStats,
    StylusGesturePolicy,
};
pub use error::InputError;
pub use event::{
    MAX_EVENTS_PER_BATCH, PointerAction, RawPointerEvent, batch_count_from_jni, pack_events, tool_from_android,
    tool_to_android, unpack_events,
};
pub use pressure::PressureCurve;
pub use view::{GestureConfig, MAX_ZOOM, MIN_ZOOM, ViewGesture, ViewTransform};
