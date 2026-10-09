//! Typed error model shared across the engine boundary (ARCHITECTURE.md §J).
//!
//! Crates define their own `thiserror` enums and convert into [`VeskraError`] at the
//! engine boundary. A `VeskraError` never contains artwork bytes or storage paths.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable diagnostic codes. Serialized names are part of the FFI/CLI contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ErrorCode {
    InvalidArgument,
    InvalidDocument,
    UnsupportedSchema,
    CorruptData,
    CorruptTile,
    AllocationLimit,
    InvalidProfile,
    UnsupportedColorMode,
    ColorTransformFailure,
    UnsupportedLayerFeature,
    ExportLossWarning,
    IncompatibleOutputProfile,
    AndroidSurfaceLost,
    DeviceUnavailable,
    GpuOutOfMemory,
    FilePermissionDenied,
    StorageFull,
    ProviderWriteFailed,
    Io,
    Cancelled,
    PsdEncodeError,
    SvgEncodeError,
    PdfEncodeError,
    PngEncodeError,
    NotFound,
    InvalidState,
    LayerLocked,
    NothingToUndo,
    NothingToRedo,
    Internal,
}

impl ErrorCode {
    /// Whether the user can typically retry or continue after this error.
    pub fn recoverable(self) -> bool {
        !matches!(self, ErrorCode::Internal | ErrorCode::CorruptData | ErrorCode::CorruptTile)
    }

    /// Stable string key for UI localization lookups (`error.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            ErrorCode::InvalidArgument => "invalid_argument",
            ErrorCode::InvalidDocument => "invalid_document",
            ErrorCode::UnsupportedSchema => "unsupported_schema",
            ErrorCode::CorruptData => "corrupt_data",
            ErrorCode::CorruptTile => "corrupt_tile",
            ErrorCode::AllocationLimit => "allocation_limit",
            ErrorCode::InvalidProfile => "invalid_profile",
            ErrorCode::UnsupportedColorMode => "unsupported_color_mode",
            ErrorCode::ColorTransformFailure => "color_transform_failure",
            ErrorCode::UnsupportedLayerFeature => "unsupported_layer_feature",
            ErrorCode::ExportLossWarning => "export_loss_warning",
            ErrorCode::IncompatibleOutputProfile => "incompatible_output_profile",
            ErrorCode::AndroidSurfaceLost => "android_surface_lost",
            ErrorCode::DeviceUnavailable => "device_unavailable",
            ErrorCode::GpuOutOfMemory => "gpu_out_of_memory",
            ErrorCode::FilePermissionDenied => "file_permission_denied",
            ErrorCode::StorageFull => "storage_full",
            ErrorCode::ProviderWriteFailed => "provider_write_failed",
            ErrorCode::Io => "io",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::PsdEncodeError => "psd_encode_error",
            ErrorCode::SvgEncodeError => "svg_encode_error",
            ErrorCode::PdfEncodeError => "pdf_encode_error",
            ErrorCode::PngEncodeError => "png_encode_error",
            ErrorCode::NotFound => "not_found",
            ErrorCode::InvalidState => "invalid_state",
            ErrorCode::LayerLocked => "layer_locked",
            ErrorCode::NothingToUndo => "nothing_to_undo",
            ErrorCode::NothingToRedo => "nothing_to_redo",
            ErrorCode::Internal => "internal",
        }
    }
}

/// Public boundary error: code + safe human-readable diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub struct VeskraError {
    pub code: ErrorCode,
    /// Safe diagnostic text. Must not include artwork bytes or full storage paths.
    pub message: String,
    pub recoverable: bool,
}

impl VeskraError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), recoverable: code.recoverable() }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub fn alloc(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::AllocationLimit, message)
    }
}

impl fmt::Display for VeskraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.key(), self.message)
    }
}

pub type VResult<T> = Result<T, VeskraError>;
