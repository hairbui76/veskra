//! Typed input errors. Messages carry only indices, counts and axis names — never
//! coordinates or other artwork-derived data.

use veskra_core::VeskraError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    #[error("negative event count {0}")]
    NegativeCount(i64),
    #[error("event count {count} exceeds batch limit {max}")]
    TooManyEvents { count: usize, max: usize },
    #[error("packed size arithmetic overflow")]
    Overflow,
    #[error("packed {array} array too short: need {needed}, got {got}")]
    ArrayTooShort { array: &'static str, needed: usize, got: usize },
    #[error("event {index}: unknown MotionEvent action {value}")]
    UnknownAction { index: usize, value: i32 },
    #[error("event {index}: unknown flag bits {bits:#x}")]
    UnknownFlags { index: usize, bits: i32 },
    #[error("event {index}: historical and predicted flags are both set")]
    ContradictoryFlags { index: usize },
    #[error("event {index}: position is not finite")]
    NonFinitePosition { index: usize },
    #[error("event {index}: {axis} axis is infinite")]
    InfiniteAxis { index: usize, axis: &'static str },
    #[error("event {index}: negative timestamp")]
    NegativeTimestamp { index: usize },
    #[error("invalid pressure curve: {0}")]
    InvalidCurve(&'static str),
    #[error("trace line {line}: {message}")]
    Trace { line: usize, message: String },
}

impl From<InputError> for VeskraError {
    fn from(e: InputError) -> Self {
        VeskraError::invalid(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veskra_core::ErrorCode;

    #[test]
    fn converts_to_invalid_argument() {
        let v: VeskraError = InputError::NegativeCount(-1).into();
        assert_eq!(v.code, ErrorCode::InvalidArgument);
        assert!(v.recoverable);
        assert!(v.message.contains("negative"));
    }
}
