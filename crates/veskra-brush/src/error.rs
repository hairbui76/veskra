use veskra_core::{ErrorCode, VeskraError};

/// Typed brush errors. Messages never contain artwork data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BrushError {
    /// A preset field is out of range or inconsistent (including a dishonest `vector_eligible`).
    #[error("invalid brush preset: {0}")]
    InvalidPreset(String),
    /// Preset JSON is syntactically invalid or has a field of the wrong type.
    #[error("brush preset JSON could not be parsed: {0}")]
    Parse(String),
    /// Preset schema is newer than this build understands (never silently downgraded).
    #[error("brush preset schema {found} is newer than supported schema {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    /// Preset JSON exceeds the size bound.
    #[error("brush preset JSON exceeds {limit} bytes")]
    TooLarge { limit: usize },
    /// Invalid stroke argument (color, clip, overrides).
    #[error("invalid stroke argument: {0}")]
    InvalidArgument(&'static str),
    /// A brush asset (PNG / mask) is malformed or does not match its content hash.
    #[error("invalid brush asset: {0}")]
    InvalidAsset(String),
    /// A brush asset exceeds a size bound.
    #[error("brush asset too large: {0}")]
    AssetTooLarge(String),
    /// A preset references an asset that is not available.
    #[error("brush asset {0} is missing")]
    MissingAsset(String),
}

impl From<BrushError> for VeskraError {
    fn from(e: BrushError) -> Self {
        let code = match &e {
            BrushError::InvalidPreset(_) | BrushError::InvalidArgument(_) => ErrorCode::InvalidArgument,
            BrushError::Parse(_) => ErrorCode::CorruptData,
            BrushError::UnsupportedSchema { .. } => ErrorCode::UnsupportedSchema,
            BrushError::TooLarge { .. } | BrushError::AssetTooLarge(_) => ErrorCode::AllocationLimit,
            BrushError::InvalidAsset(_) => ErrorCode::CorruptData,
            BrushError::MissingAsset(_) => ErrorCode::NotFound,
        };
        VeskraError::new(code, e.to_string())
    }
}
