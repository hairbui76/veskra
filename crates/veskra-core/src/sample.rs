//! Stylus/pointer samples (ARCHITECTURE.md §B `StrokeSample`).
//!
//! Rules enforced here and relied on everywhere else:
//! * Only samples flagged [`SampleFlags::CONFIRMED`] may enter a committed stroke,
//!   history, native file or export. [`SampleFlags::PREDICTED`] is presentation-only.
//! * Absent hardware axes are `None` — never invented. Synthesized pressure for
//!   devices without pressure is marked by the *absence* of [`SampleFlags::PRESSURE_REPORTED`].

use crate::{ErrorCode, VeskraError};
use serde::{Deserialize, Serialize};

/// Bit flags for a sample. Serialized as a raw `u16` (stable bit assignments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SampleFlags(pub u16);

impl SampleFlags {
    /// Hardware-confirmed sample (historical or current MotionEvent position).
    pub const CONFIRMED: SampleFlags = SampleFlags(1 << 0);
    /// Motion-predicted sample. Presentation only; never persisted.
    pub const PREDICTED: SampleFlags = SampleFlags(1 << 1);
    /// Hover (no contact).
    pub const HOVER: SampleFlags = SampleFlags(1 << 2);
    /// Pressure value came from hardware (otherwise it is the documented fallback curve value).
    pub const PRESSURE_REPORTED: SampleFlags = SampleFlags(1 << 3);
    /// Primary stylus button held.
    pub const BUTTON_PRIMARY: SampleFlags = SampleFlags(1 << 4);
    /// Secondary stylus button held.
    pub const BUTTON_SECONDARY: SampleFlags = SampleFlags(1 << 5);
    /// Sample came from MotionEvent history (batched) rather than the current position.
    pub const HISTORICAL: SampleFlags = SampleFlags(1 << 6);

    pub const fn empty() -> Self {
        SampleFlags(0)
    }
    pub const fn contains(self, other: SampleFlags) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn union(self, other: SampleFlags) -> SampleFlags {
        SampleFlags(self.0 | other.0)
    }
    pub fn insert(&mut self, other: SampleFlags) {
        self.0 |= other.0;
    }
    pub fn remove(&mut self, other: SampleFlags) {
        self.0 &= !other.0;
    }
}

impl std::ops::BitOr for SampleFlags {
    type Output = SampleFlags;
    fn bitor(self, rhs: SampleFlags) -> SampleFlags {
        self.union(rhs)
    }
}

/// Pointer tool type as reported by the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ToolType {
    Stylus,
    /// Stylus eraser end (Android `TOOL_TYPE_ERASER`).
    StylusEraser,
    Finger,
    Mouse,
    Unknown,
}

/// One input sample in **document** coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StrokeSample {
    pub x: f32,
    pub y: f32,
    /// Monotonic timestamp in nanoseconds (platform event time base).
    pub t_ns: u64,
    /// Normalized pressure in `[0, 1]` after calibration.
    pub pressure: f32,
    /// Tilt from perpendicular, radians `[0, PI/2]`, if reported.
    pub tilt: Option<f32>,
    /// Azimuth/orientation, radians `[-PI, PI]`, if reported.
    pub azimuth: Option<f32>,
    /// Barrel rotation, radians, if reported.
    pub rotation: Option<f32>,
    pub flags: SampleFlags,
}

impl StrokeSample {
    /// Convenience constructor for a confirmed sample with hardware pressure.
    pub fn confirmed(x: f32, y: f32, t_ns: u64, pressure: f32) -> Self {
        Self {
            x,
            y,
            t_ns,
            pressure,
            tilt: None,
            azimuth: None,
            rotation: None,
            flags: SampleFlags::CONFIRMED | SampleFlags::PRESSURE_REPORTED,
        }
    }

    pub fn is_confirmed(&self) -> bool {
        self.flags.contains(SampleFlags::CONFIRMED) && !self.flags.contains(SampleFlags::PREDICTED)
    }

    /// Validate a sample for entry into a stroke. Rejects non-finite values,
    /// out-of-range pressure/angles and confirmed+predicted contradictions.
    pub fn validate(&self) -> Result<(), VeskraError> {
        let bad = |m: &str| Err(VeskraError::new(ErrorCode::InvalidArgument, m.to_string()));
        if !self.x.is_finite() || !self.y.is_finite() {
            return bad("sample position not finite");
        }
        if !(self.pressure.is_finite() && (0.0..=1.0).contains(&self.pressure)) {
            return bad("pressure outside [0,1]");
        }
        if let Some(t) = self.tilt
            && !(t.is_finite() && (0.0..=std::f32::consts::FRAC_PI_2 + 1e-4).contains(&t))
        {
            return bad("tilt outside [0, pi/2]");
        }
        if let Some(a) = self.azimuth
            && !(a.is_finite() && a.abs() <= std::f32::consts::PI * 2.0 + 1e-4)
        {
            return bad("azimuth out of range");
        }
        if let Some(r) = self.rotation
            && !r.is_finite()
        {
            return bad("rotation not finite");
        }
        if self.flags.contains(SampleFlags::CONFIRMED) && self.flags.contains(SampleFlags::PREDICTED) {
            return bad("sample cannot be both confirmed and predicted");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        let s = StrokeSample::confirmed(1.0, 2.0, 10, 0.5);
        assert!(s.validate().is_ok());
        assert!(s.is_confirmed());
        let mut bad = s;
        bad.pressure = 1.5;
        assert!(bad.validate().is_err());
        bad = s;
        bad.x = f32::NAN;
        assert!(bad.validate().is_err());
        bad = s;
        bad.flags = SampleFlags::CONFIRMED | SampleFlags::PREDICTED;
        assert!(bad.validate().is_err());
        bad = s;
        bad.tilt = Some(3.0);
        assert!(bad.validate().is_err());
    }

    #[test]
    fn predicted_not_confirmed() {
        let mut s = StrokeSample::confirmed(0.0, 0.0, 0, 0.1);
        s.flags = SampleFlags::PREDICTED;
        assert!(!s.is_confirmed());
    }

    #[test]
    fn flags_serialize_as_u16() {
        let f = SampleFlags::CONFIRMED | SampleFlags::HISTORICAL;
        assert_eq!(serde_json::to_string(&f).unwrap(), "65");
    }
}
