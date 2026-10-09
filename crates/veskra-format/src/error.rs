//! Typed errors and non-fatal warnings of the native format.

use std::fmt;
use std::io;
use veskra_core::{ErrorCode, VeskraError};

/// Errors from encoding, decoding, saving and recovery. Messages never contain artwork bytes or
/// storage paths (entry names are bundle-internal identifiers, not paths).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FormatError {
    /// Structural damage: not a ZIP / not a bundle, bad CRC or hash, malformed JSON, missing entry.
    #[error("corrupt bundle: {0}")]
    Corrupt(String),
    /// A raster/mask tile payload is malformed (wrong size, premultiplied invariant violated).
    #[error("corrupt tile: {0}")]
    CorruptTile(String),
    /// The bundle requires a newer reader (or an older schema without a migration path).
    #[error("unsupported schema {schema} (minimum reader {min_reader}; this reader supports schema {supported})")]
    UnsupportedSchema { schema: u32, min_reader: u32, supported: u32 },
    /// The bundle uses a feature this reader cannot represent (other tile size or pixel layout).
    #[error("unsupported bundle feature: {0}")]
    UnsupportedFeature(String),
    /// A size/count limit from `veskra_core::limits` (or the container) was exceeded.
    #[error("limit exceeded: {0}")]
    Limit(String),
    /// ICC profile bytes could not be validated.
    #[error("invalid ICC profile: {0}")]
    InvalidProfile(String),
    /// Decoded data does not form a valid document (tree invariants, stroke validation, ...).
    #[error("invalid document: {0}")]
    InvalidDocument(String),
    /// Caller error on the encode side (e.g. preview is not a PNG).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The device ran out of space (ENOSPC / EDQUOT).
    #[error("storage full")]
    StorageFull,
    /// Any other I/O failure.
    #[error("i/o error ({kind:?}): {message}")]
    Io { kind: io::ErrorKind, message: String },
}

impl FormatError {
    /// Stable engine error code.
    pub fn code(&self) -> ErrorCode {
        match self {
            FormatError::Corrupt(_) => ErrorCode::CorruptData,
            FormatError::CorruptTile(_) => ErrorCode::CorruptTile,
            FormatError::UnsupportedSchema { .. } | FormatError::UnsupportedFeature(_) => ErrorCode::UnsupportedSchema,
            FormatError::Limit(_) => ErrorCode::AllocationLimit,
            FormatError::InvalidProfile(_) => ErrorCode::InvalidProfile,
            FormatError::InvalidDocument(_) => ErrorCode::InvalidDocument,
            FormatError::InvalidArgument(_) => ErrorCode::InvalidArgument,
            FormatError::StorageFull => ErrorCode::StorageFull,
            FormatError::Io { kind: io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem, .. } => {
                ErrorCode::FilePermissionDenied
            }
            FormatError::Io { .. } => ErrorCode::Io,
        }
    }
}

/// True for "no space left" style failures (ENOSPC = 28, EDQUOT = 122 on Linux/Android).
pub(crate) fn is_storage_full(e: &io::Error) -> bool {
    if matches!(e.kind(), io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded) {
        return true;
    }
    #[cfg(unix)]
    if matches!(e.raw_os_error(), Some(28) | Some(122)) {
        return true;
    }
    false
}

impl From<io::Error> for FormatError {
    fn from(e: io::Error) -> Self {
        if is_storage_full(&e) {
            FormatError::StorageFull
        } else {
            FormatError::Io { kind: e.kind(), message: e.to_string() }
        }
    }
}

impl From<FormatError> for VeskraError {
    fn from(e: FormatError) -> Self {
        VeskraError::new(e.code(), e.to_string())
    }
}

/// Non-fatal conditions reported when a bundle opens successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatWarning {
    /// Written by a newer schema that declared itself readable by this reader (`minReader <= 1`).
    NewerSchema { schema: u32 },
    /// JSON fields this reader does not know; they are ignored and NOT preserved on re-save.
    UnknownFields { entry: String, paths: Vec<String> },
    /// Archive entries this reader does not use; integrity-checked, ignored, not preserved.
    UnknownEntries { names: Vec<String> },
    /// The bundle was migrated from an older schema on load.
    Migrated { from: u32, to: u32 },
}

impl fmt::Display for FormatWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatWarning::NewerSchema { schema } => {
                write!(f, "bundle schema {schema} is newer than this reader; unknown data will not be preserved")
            }
            FormatWarning::UnknownFields { entry, paths } => {
                write!(f, "{entry}: {} unknown field(s) ignored", paths.len())
            }
            FormatWarning::UnknownEntries { names } => write!(f, "{} unknown entr(ies) ignored", names.len()),
            FormatWarning::Migrated { from, to } => write!(f, "migrated from schema {from} to {to}"),
        }
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn permission_errors_map_to_file_permission_denied() {
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::ReadOnlyFilesystem] {
            let e: FormatError = io::Error::new(kind, "denied").into();
            assert_eq!(e.code(), ErrorCode::FilePermissionDenied);
        }
        let e: FormatError = io::Error::new(io::ErrorKind::NotFound, "x").into();
        assert_eq!(e.code(), ErrorCode::Io);
    }
}
