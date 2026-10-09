//! Hard safety limits. Every buffer sized from external data must pass through these.
//! Values are conservative v1 engineering limits (revisit at G6 with device data).

use crate::{ErrorCode, VeskraError};

/// Maximum document edge in pixels.
pub const MAX_DOCUMENT_DIM: u32 = 16_384;
/// Maximum document pixel count (8192 x 8192).
pub const MAX_DOCUMENT_PIXELS: u64 = 8192 * 8192;
/// Maximum number of layer nodes (including groups).
pub const MAX_LAYERS: usize = 1_000;
/// Maximum group nesting depth.
pub const MAX_GROUP_DEPTH: usize = 32;
/// Maximum confirmed samples in one stroke.
pub const MAX_STROKE_SAMPLES: usize = 500_000;
/// Maximum ICC profile size accepted from any input.
pub const MAX_ICC_BYTES: usize = 4 * 1024 * 1024;
/// Maximum size of any single decompressed entry inside a native bundle.
pub const MAX_BUNDLE_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum total decompressed size of a native bundle.
pub const MAX_BUNDLE_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum number of entries in a native bundle.
pub const MAX_BUNDLE_ENTRIES: usize = 2_000_000;
/// Maximum layer name length in UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// Validate document dimensions.
pub fn check_document_size(width: u32, height: u32) -> Result<(), VeskraError> {
    if width == 0 || height == 0 {
        return Err(VeskraError::new(ErrorCode::InvalidArgument, "document dimensions must be nonzero"));
    }
    if width > MAX_DOCUMENT_DIM || height > MAX_DOCUMENT_DIM {
        return Err(VeskraError::new(
            ErrorCode::AllocationLimit,
            format!("document edge exceeds {MAX_DOCUMENT_DIM}px"),
        ));
    }
    if width as u64 * height as u64 > MAX_DOCUMENT_PIXELS {
        return Err(VeskraError::new(ErrorCode::AllocationLimit, "document pixel count exceeds limit"));
    }
    Ok(())
}

/// Checked byte length for a `width x height` buffer with `bytes_per_pixel`.
pub fn checked_buffer_len(width: u32, height: u32, bytes_per_pixel: usize) -> Result<usize, VeskraError> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(bytes_per_pixel))
        .ok_or_else(|| VeskraError::new(ErrorCode::AllocationLimit, "buffer size overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_checks() {
        assert!(check_document_size(2048, 2048).is_ok());
        assert!(check_document_size(8192, 8192).is_ok());
        assert!(check_document_size(0, 10).is_err());
        assert_eq!(check_document_size(16_385, 1).unwrap_err().code, ErrorCode::AllocationLimit);
        assert_eq!(check_document_size(16_384, 16_384).unwrap_err().code, ErrorCode::AllocationLimit);
        assert!(checked_buffer_len(u32::MAX, u32::MAX, 8).is_err() || usize::BITS > 64);
        assert_eq!(checked_buffer_len(10, 10, 4).unwrap(), 400);
    }
}
