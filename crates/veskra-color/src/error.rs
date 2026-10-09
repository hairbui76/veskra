//! Typed errors of the color subsystem and their mapping onto the engine boundary error.

use veskra_core::{ErrorCode, VeskraError};

/// Errors produced by ICC parsing, profile construction and transform setup.
///
/// Messages never contain profile bytes beyond 4-character signatures (sanitized to printable
/// ASCII), so they are safe to surface through the FFI.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ColorError {
    /// The data ends before a structure that the header/tag table says must be present.
    #[error("ICC profile truncated: {what} needs {needed} bytes, only {available} available")]
    Truncated { what: &'static str, needed: u64, available: u64 },
    /// The header's declared size is smaller than the data handed in (trailing garbage).
    #[error("ICC profile size mismatch: header declares {declared} bytes, data has {actual}")]
    SizeMismatch { declared: u64, actual: u64 },
    /// Profile exceeds [`veskra_core::limits::MAX_ICC_BYTES`].
    #[error("ICC profile of {size} bytes exceeds the {limit}-byte limit")]
    TooLarge { size: usize, limit: usize },
    /// Structural violation of ICC.1 (bad magic, overlapping tags, singular matrix, ...).
    #[error("invalid ICC profile: {0}")]
    InvalidProfile(&'static str),
    /// A specific tag is malformed or of an unsupported type.
    #[error("invalid ICC tag '{tag}': {reason}")]
    InvalidTag { tag: String, reason: &'static str },
    /// A required tag is absent.
    #[error("ICC profile is missing required tag '{0}'")]
    MissingTag(&'static str),
    /// ICC major version other than 2 or 4 (e.g. iccMAX v5).
    #[error("unsupported ICC version {major}.{minor}")]
    UnsupportedVersion { major: u8, minor: u8 },
    /// Data color space other than RGB or Gray (CMYK, Lab, YCbCr, n-channel ...).
    #[error("unsupported ICC color space '{0}'")]
    UnsupportedColorSpace(String),
    /// Device link, abstract or named-color profile.
    #[error("unsupported ICC profile class '{0}'")]
    UnsupportedProfileClass(String),
    /// Profile only describes its color via LUT tags (A2B0/B2A0/...) without matrix/TRC tags.
    #[error("LUT-based ICC profile without matrix/TRC tags is not supported in v1")]
    LutOnlyProfile,
    /// Operation needs an RGB profile but got a Gray one (or vice versa).
    #[error("profile color space not supported by this operation: {0}")]
    WrongProfileKind(&'static str),
    /// A transform cannot be constructed (e.g. destination curve not invertible).
    #[error("color transform failure: {0}")]
    TransformFailure(&'static str),
    /// Stored color metadata does not match the profile bytes it refers to.
    #[error("profile hash does not match color metadata")]
    ProfileHashMismatch,
}

impl ColorError {
    /// Stable engine error code for this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            ColorError::TooLarge { .. } => ErrorCode::AllocationLimit,
            ColorError::UnsupportedColorSpace(_)
            | ColorError::UnsupportedProfileClass(_)
            | ColorError::LutOnlyProfile
            | ColorError::WrongProfileKind(_) => ErrorCode::UnsupportedColorMode,
            ColorError::TransformFailure(_) => ErrorCode::ColorTransformFailure,
            ColorError::Truncated { .. }
            | ColorError::SizeMismatch { .. }
            | ColorError::InvalidProfile(_)
            | ColorError::InvalidTag { .. }
            | ColorError::MissingTag(_)
            | ColorError::UnsupportedVersion { .. }
            | ColorError::ProfileHashMismatch => ErrorCode::InvalidProfile,
        }
    }
}

impl From<ColorError> for VeskraError {
    fn from(e: ColorError) -> Self {
        VeskraError::new(e.code(), e.to_string())
    }
}

/// Render a 4-byte ICC signature as printable ASCII (non-printables become `?`).
pub(crate) fn sig_str(sig: [u8; 4]) -> String {
    sig.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '?' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_contract_codes() {
        let cases = [
            (ColorError::TooLarge { size: 9, limit: 1 }, ErrorCode::AllocationLimit),
            (ColorError::UnsupportedColorSpace("CMYK".into()), ErrorCode::UnsupportedColorMode),
            (ColorError::LutOnlyProfile, ErrorCode::UnsupportedColorMode),
            (ColorError::TransformFailure("x"), ErrorCode::ColorTransformFailure),
            (ColorError::InvalidProfile("x"), ErrorCode::InvalidProfile),
            (ColorError::Truncated { what: "x", needed: 2, available: 1 }, ErrorCode::InvalidProfile),
        ];
        for (e, code) in cases {
            let v: VeskraError = e.into();
            assert_eq!(v.code, code);
            assert!(v.recoverable);
        }
        assert_eq!(sig_str(*b"RG\0\xff"), "RG??");
    }
}
