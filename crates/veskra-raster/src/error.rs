use veskra_core::{ErrorCode, VeskraError};

/// Typed errors for raster operations. Never contains pixel data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RasterError {
    /// A caller-supplied buffer had the wrong length for the rectangle/tile.
    #[error("pixel buffer length {got} does not match expected {expected}")]
    SizeMismatch { expected: usize, got: usize },
    /// Tile data (typically decoded from a file) is malformed.
    #[error("invalid tile data: {0}")]
    InvalidTileData(&'static str),
    /// A rectangle or work area exceeds the configured pixel limit.
    #[error("area of {pixels} pixels exceeds the limit of {limit}")]
    TooLarge { pixels: u64, limit: u64 },
    /// Flood fill exceeded its bounded work stack (should not happen for valid bounds).
    #[error("fill work stack exceeded its bound of {limit} entries")]
    FillStackLimit { limit: usize },
    /// Invalid non-data argument (NaN tolerance etc.).
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
}

impl From<RasterError> for VeskraError {
    fn from(e: RasterError) -> Self {
        let code = match &e {
            RasterError::SizeMismatch { .. } | RasterError::InvalidArgument(_) => ErrorCode::InvalidArgument,
            RasterError::InvalidTileData(_) => ErrorCode::CorruptTile,
            RasterError::TooLarge { .. } | RasterError::FillStackLimit { .. } => ErrorCode::AllocationLimit,
        };
        VeskraError::new(code, e.to_string())
    }
}
