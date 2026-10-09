//! Export request / preflight / report contracts (MASTER_PLAN §2.3, §7.6; ARCHITECTURE §H).
//!
//! These types are shared verbatim by `veskra-io`, `veskra-engine`, the FFI JSON boundary
//! and the Android export sheet. Serialized field names are part of the FFI contract.

use crate::{ErrorCode, VeskraError};
use serde::{Deserialize, Serialize};

/// Output format. There is intentionally **no** `Ai` variant: native `.ai` export is
/// unsupported (MASTER_PLAN §7.5). Illustrator handoff is `Svg` or `Pdf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportFormat {
    Png,
    Psd,
    Svg,
    Pdf,
}

impl ExportFormat {
    /// The only file extension a file of this format may be given.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Psd => "psd",
            ExportFormat::Svg => "svg",
            ExportFormat::Pdf => "pdf",
        }
    }

    pub fn mime_type(self) -> &'static str {
        match self {
            ExportFormat::Png => "image/png",
            ExportFormat::Psd => "image/vnd.adobe.photoshop",
            ExportFormat::Svg => "image/svg+xml",
            ExportFormat::Pdf => "application/pdf",
        }
    }

    /// Whether the Illustrator vector/raster toggles apply to this format.
    pub fn is_illustrator_handoff(self) -> bool {
        matches!(self, ExportFormat::Svg | ExportFormat::Pdf)
    }
}

/// The three independent toggles of the export sheet (MASTER_PLAN §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    /// `[x] Editable vector strokes` (default true).
    pub include_vector_paths: bool,
    /// `[x] Preserve original appearance` (default true).
    pub include_raster_appearance: bool,
    /// `[x] Embed color profile / color information` (default true).
    pub embed_color_profile: bool,
    /// Optional output scale for raster formats (1.0 = document pixels). Must be in (0, 4].
    #[serde(default = "one")]
    pub raster_scale: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_vector_paths: true,
            include_raster_appearance: true,
            embed_color_profile: true,
            raster_scale: 1.0,
        }
    }
}

impl ExportOptions {
    /// Validate options for a format. For SVG/PDF at least one of vector/raster must be on (ILL-004).
    pub fn validate(&self, format: ExportFormat) -> Result<(), VeskraError> {
        if format.is_illustrator_handoff() && !self.include_vector_paths && !self.include_raster_appearance {
            return Err(VeskraError::new(
                ErrorCode::InvalidArgument,
                "Select 'Editable vector strokes', 'Preserve original appearance', or both.",
            ));
        }
        if !(self.raster_scale.is_finite() && self.raster_scale > 0.0 && self.raster_scale <= 4.0) {
            return Err(VeskraError::new(ErrorCode::InvalidArgument, "raster_scale must be in (0, 4]"));
        }
        Ok(())
    }
}

/// Machine-readable warning categories surfaced in preflight and reports.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportWarningKind {
    /// Stroke is raster-only (texture/airbrush/etc.) and cannot become an editable vector.
    RasterOnlyStroke,
    /// Layer content was rasterized (e.g. raster edits after strokes, fills).
    LayerRasterized,
    /// A layer feature is not representable and was flattened into pixels.
    FeatureFlattened,
    /// Blend mode not representable in the target; layer composited/rasterized instead.
    BlendModeApproximated,
    /// Color profile could not be embedded; values written as sRGB.
    ProfileNotEmbedded,
    /// Document uses linear blend space; other apps may recomposite differently.
    BlendSpaceMismatch,
    /// Vector content omitted because the user disabled vector output.
    VectorOmitted,
    /// Raster appearance omitted because the user disabled raster output.
    AppearanceOmitted,
    /// Hidden layers are exported hidden (or omitted for flat formats).
    HiddenLayer,
    /// Clipping/alpha-lock semantics approximated.
    ClippingApproximated,
    /// Anything else, described in the message.
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportWarning {
    pub kind: ExportWarningKind,
    /// Human-readable, safe message (no artwork bytes or storage paths).
    pub message: String,
    /// Optional affected layer (stringified LayerId).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// Number of affected objects (strokes/layers) this warning summarizes.
    #[serde(default)]
    pub count: u32,
}

/// Preflight shown **before** the user exports (MASTER_PLAN §2.3: counts + warnings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreflight {
    pub format: ExportFormat,
    pub options: ExportOptions,
    /// `None` if options are valid; otherwise the reason export is blocked.
    pub blocked_reason: Option<String>,
    pub vector_path_count: u32,
    pub raster_only_stroke_count: u32,
    pub rasterized_layer_count: u32,
    /// Profile semantics, e.g. `"sRGB IEC61966-2.1 (embedded)"` or `"sRGB values, profile not embedded"`.
    pub profile: String,
    pub width_px: u32,
    pub height_px: u32,
    pub estimated_bytes: u64,
    pub warnings: Vec<ExportWarning>,
}

/// Validation outcome for a written file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    /// Re-read with Veskra's own structural validator after flush.
    StructurallyValidated,
    /// Written but not re-read (e.g. streaming target without read access).
    NotValidated,
    /// Validation failed; the file must not be reported as successful.
    Failed,
}

/// What was **actually** written (MASTER_PLAN §7.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub success: bool,
    /// Destination token/URI as provided by the caller (opaque to the engine).
    pub file_uri: Option<String>,
    pub format: ExportFormat,
    pub profile: String,
    pub profile_embedded: bool,
    pub vector_object_count: u32,
    pub rasterized_object_count: u32,
    pub flattened_feature_list: Vec<String>,
    pub warnings: Vec<ExportWarning>,
    pub duration_ms: u64,
    pub file_bytes: u64,
    pub width_px: u32,
    pub height_px: u32,
    pub validation: ValidationStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_unchecked_blocks_illustrator_export() {
        let o = ExportOptions { include_vector_paths: false, include_raster_appearance: false, ..Default::default() };
        assert!(o.validate(ExportFormat::Svg).is_err());
        assert!(o.validate(ExportFormat::Pdf).is_err());
        // PSD/PNG do not use the vector/raster toggles.
        assert!(o.validate(ExportFormat::Psd).is_ok());
        assert!(ExportOptions::default().validate(ExportFormat::Svg).is_ok());
    }

    #[test]
    fn no_ai_extension_anywhere() {
        for f in [ExportFormat::Png, ExportFormat::Psd, ExportFormat::Svg, ExportFormat::Pdf] {
            assert_ne!(f.extension(), "ai");
            assert!(!f.mime_type().contains("illustrator"));
        }
    }

    #[test]
    fn options_json_contract() {
        let j = serde_json::to_string(&ExportOptions::default()).unwrap();
        assert_eq!(
            j,
            r#"{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"rasterScale":1.0}"#
        );
        let o: ExportOptions = serde_json::from_str(
            r#"{"includeVectorPaths":false,"includeRasterAppearance":true,"embedColorProfile":false}"#,
        )
        .unwrap();
        assert_eq!(o.raster_scale, 1.0);
        assert!(!o.include_vector_paths);
    }

    #[test]
    fn bad_scale_rejected() {
        let o = ExportOptions { raster_scale: 0.0, ..Default::default() };
        assert!(o.validate(ExportFormat::Png).is_err());
        let o = ExportOptions { raster_scale: f32::NAN, ..Default::default() };
        assert!(o.validate(ExportFormat::Png).is_err());
    }
}
