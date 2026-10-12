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

/// Target color space of an export (owner-approved contract extension, 2026-10-10; `cmyk` added
/// by ADR-018).
///
/// Pixels are converted from the document working space with [`ExportOptions::rendering_intent`]
/// (RGB matrix/TRC profiles: perceptual behaves as relative colorimetric, out-of-gamut colors are
/// clipped per channel and counted in the report) and the matching ICC profile is embedded. When
/// the document already is in the target space nothing is converted and the document's own
/// profile bytes are embedded. `cmyk` converts to the CMYK output profile
/// [`ExportOptions::cmyk_profile`] (PSD and PDF only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportProfile {
    /// Keep the document working space (default; sRGB for every document Veskra can author).
    /// PNG/PSD embed the document profile unconverted; SVG/PDF, which Veskra always writes as
    /// sRGB unless a wide-gamut target is chosen, behave as [`ExportProfile::Srgb`].
    #[default]
    Document,
    /// sRGB IEC 61966-2.1.
    Srgb,
    /// Display P3 (DCI-P3 primaries, D65, sRGB curve).
    DisplayP3,
    /// Veskra's generated profile with the colorimetry of Adobe RGB (1998).
    AdobeRgb,
    /// CMYK through a LUT-based ICC output profile (`cmykProfile`, default: the bundled
    /// Coated FOGRA39 profile). PSD (color mode 4) and PDF (DeviceCMYK + output intent) only;
    /// PNG and SVG cannot carry CMYK and are refused.
    Cmyk,
}

impl ExportProfile {
    /// Stable machine name (same as the serialized form).
    pub fn as_str(self) -> &'static str {
        match self {
            ExportProfile::Document => "document",
            ExportProfile::Srgb => "srgb",
            ExportProfile::DisplayP3 => "display_p3",
            ExportProfile::AdobeRgb => "adobe_rgb",
            ExportProfile::Cmyk => "cmyk",
        }
    }

    /// The CMYK print target.
    pub fn is_cmyk(self) -> bool {
        self == ExportProfile::Cmyk
    }

    /// A wide-gamut target whose values are meaningless without the embedded profile.
    pub fn requires_embedding(self) -> bool {
        matches!(self, ExportProfile::DisplayP3 | ExportProfile::AdobeRgb)
    }
}

/// Rendering intent of the color conversions an export performs (ADR-018).
///
/// For LUT-based CMYK profiles the intent selects the profile's tables (`B2A1`/`A2B1` for
/// relative colorimetric, `B2A0`/`A2B0` for perceptual). RGB matrix/TRC profiles carry no
/// perceptual tables, so `perceptual` produces the same numbers as `relative_colorimetric` there
/// (as LittleCMS does); only the reported intent differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportIntent {
    /// Media-relative colorimetric (default; Photoshop's default for CMYK conversions).
    #[default]
    RelativeColorimetric,
    /// Perceptual (the profile's gamut-compressing tables).
    Perceptual,
}

impl ExportIntent {
    /// Stable machine name (same as the serialized form).
    pub fn as_str(self) -> &'static str {
        match self {
            ExportIntent::RelativeColorimetric => "relative_colorimetric",
            ExportIntent::Perceptual => "perceptual",
        }
    }
}

/// Longest `cmykProfile` path accepted (bytes).
pub const MAX_PROFILE_PATH_BYTES: usize = 4096;

/// Identity of the CMYK output profile used by an export (preflight and report).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CmykProfileInfo {
    /// Profile description (`desc` tag).
    pub description: String,
    /// `true` for the profile bundled with Veskra (`cmykProfile: null`).
    pub bundled: bool,
}

/// The three independent toggles of the export sheet (MASTER_PLAN §2.3) plus the color target.
///
/// Not `Copy` since ADR-018 added the `cmykProfile` path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Target color space (default `document`). Old JSON without this field keeps its behaviour.
    #[serde(default)]
    pub profile: ExportProfile,
    /// Bits per channel: `8` (default) or `16`. 16-bit output exists for PNG and PSD; SVG/PDF
    /// always write 8-bit images and report that.
    #[serde(default = "eight")]
    pub bit_depth: u8,
    /// CMYK output profile file (absolute path) for `profile: "cmyk"`; `None` = the bundled
    /// Coated FOGRA39 profile. Ignored for other targets.
    #[serde(default)]
    pub cmyk_profile: Option<String>,
    /// Intent of every conversion (default relative colorimetric). RGB matrix/TRC conversions
    /// produce identical numbers for both intents.
    #[serde(default)]
    pub rendering_intent: ExportIntent,
    /// Black point compensation (default `true`) for CMYK conversions. RGB matrix/TRC
    /// conversions are unaffected (Veskra's RGB profiles all have a zero black point, where BPC is
    /// the identity), so RGB output keeps its exact bytes.
    #[serde(default = "yes")]
    pub black_point_compensation: bool,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

fn eight() -> u8 {
    8
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_vector_paths: true,
            include_raster_appearance: true,
            embed_color_profile: true,
            raster_scale: 1.0,
            profile: ExportProfile::Document,
            bit_depth: 8,
            cmyk_profile: None,
            rendering_intent: ExportIntent::RelativeColorimetric,
            black_point_compensation: true,
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
        if self.bit_depth != 8 && self.bit_depth != 16 {
            return Err(VeskraError::new(ErrorCode::InvalidArgument, "bitDepth must be 8 or 16"));
        }
        if self.profile.requires_embedding() && !self.embed_color_profile {
            // Untagged wide-gamut values would be read as sRGB by every other application.
            return Err(VeskraError::new(
                ErrorCode::IncompatibleOutputProfile,
                "A Display P3 or Adobe RGB export must embed its color profile; enable 'Embed color profile' or choose sRGB.",
            ));
        }
        if self.profile.is_cmyk() {
            match format {
                ExportFormat::Png => {
                    return Err(VeskraError::new(
                        ErrorCode::IncompatibleOutputProfile,
                        "PNG cannot store CMYK. Export PSD or PDF for print, or choose an RGB profile (sRGB, Display P3, Adobe RGB).",
                    ));
                }
                ExportFormat::Svg => {
                    return Err(VeskraError::new(
                        ErrorCode::IncompatibleOutputProfile,
                        "SVG cannot store CMYK colors. Export PDF (for Illustrator) or PSD for print, or choose an RGB profile.",
                    ));
                }
                _ => {}
            }
            if let Some(p) = &self.cmyk_profile {
                if p.trim().is_empty() {
                    return Err(VeskraError::invalid(
                        "cmykProfile must be the path of an ICC file, or null for the bundled profile",
                    ));
                }
                if p.len() > MAX_PROFILE_PATH_BYTES {
                    return Err(VeskraError::invalid("cmykProfile path is too long"));
                }
            }
        }
        Ok(())
    }

    /// Whether this export converts to CMYK.
    pub fn is_cmyk(&self) -> bool {
        self.profile.is_cmyk()
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
    /// Pixels were converted from the document profile to the chosen target profile.
    ColorConverted,
    /// Colors outside the target gamut were (or may be) clipped; `count` = affected pixels.
    GamutClipped,
    /// The format cannot carry the target profile for vector colors; they stay sRGB values.
    VectorColorsSrgb,
    /// The requested bit depth is not available for this format; 8-bit was written.
    BitDepthReduced,
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
    /// Name of the color space the pixel values will be written in (e.g. `"Display P3"`).
    #[serde(default)]
    pub target_profile: String,
    /// Bits per channel that will actually be written (8 or 16).
    #[serde(default = "eight")]
    pub bit_depth: u8,
    /// `"relative-colorimetric"` when pixels will be converted, `None` when written unconverted.
    #[serde(default)]
    pub rendering_intent: Option<String>,
    /// The document's colors can fall outside the target gamut (clipping is counted on export).
    #[serde(default)]
    pub gamut_clipping_possible: bool,
    /// CMYK only: estimated maximum total area coverage (C+M+Y+K in percent, 0–400) of the
    /// converted image, from a downsampled composite (the report has the exact value); `None`
    /// for RGB targets or when blocked.
    #[serde(default)]
    pub total_area_coverage_max: Option<f64>,
    /// CMYK only: the output profile that will be used.
    #[serde(default)]
    pub cmyk_profile: Option<CmykProfileInfo>,
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
    /// Name of the color space the pixel values were written in.
    #[serde(default)]
    pub target_profile: String,
    /// Bits per channel actually written (8 or 16).
    #[serde(default = "eight")]
    pub bit_depth: u8,
    /// `"relative-colorimetric"` when pixels were converted, `None` when written unconverted.
    #[serde(default)]
    pub rendering_intent: Option<String>,
    /// Pixels (alpha > 0) of the written image(s) whose color was outside the target gamut and
    /// was clipped. PNG/PSD: the flattened image; SVG/PDF: all embedded raster images. CMYK: a
    /// color counts when it does not survive RGB → CMYK → RGB within ΔE00 2 (ADR-018).
    #[serde(default)]
    pub out_of_gamut_pixels: u64,
    /// CMYK only: maximum total area coverage (C+M+Y+K in percent) over the written CMYK pixels
    /// (alpha > 0, unmatted color); `None` for RGB targets.
    #[serde(default)]
    pub total_area_coverage_max: Option<f64>,
    /// CMYK only: the output profile used.
    #[serde(default)]
    pub cmyk_profile: Option<CmykProfileInfo>,
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
            r#"{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"rasterScale":1.0,"profile":"document","bitDepth":8,"cmykProfile":null,"renderingIntent":"relative_colorimetric","blackPointCompensation":true}"#
        );
        let o: ExportOptions = serde_json::from_str(
            r#"{"includeVectorPaths":false,"includeRasterAppearance":true,"embedColorProfile":false}"#,
        )
        .unwrap();
        assert_eq!(o.raster_scale, 1.0);
        assert!(!o.include_vector_paths);
        // Old JSON (no color fields) keeps the old behaviour: document space, 8 bit.
        assert_eq!((o.profile, o.bit_depth), (ExportProfile::Document, 8));
        let o: ExportOptions = serde_json::from_str(
            r#"{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"profile":"display_p3","bitDepth":16}"#,
        )
        .unwrap();
        assert_eq!((o.profile, o.bit_depth), (ExportProfile::DisplayP3, 16));
        assert!(o.validate(ExportFormat::Png).is_ok());
        // Old JSON keeps the old color behaviour: relative colorimetric, BPC on (no-op for RGB).
        assert_eq!(o.cmyk_profile, None);
        assert_eq!(o.rendering_intent, ExportIntent::RelativeColorimetric);
        assert!(o.black_point_compensation);
        let o: ExportOptions = serde_json::from_str(
            r#"{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"profile":"cmyk","cmykProfile":"/p/x.icc","renderingIntent":"perceptual","blackPointCompensation":false}"#,
        )
        .unwrap();
        assert_eq!(o.profile, ExportProfile::Cmyk);
        assert_eq!(o.cmyk_profile.as_deref(), Some("/p/x.icc"));
        assert_eq!(o.rendering_intent, ExportIntent::Perceptual);
        assert!(!o.black_point_compensation);
        assert!(
            serde_json::from_str::<ExportOptions>(
                r#"{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"renderingIntent":"saturation"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn cmyk_options_are_validated() {
        let o = ExportOptions { profile: ExportProfile::Cmyk, ..Default::default() };
        for f in [ExportFormat::Png, ExportFormat::Svg] {
            let e = o.validate(f).unwrap_err();
            assert_eq!(e.code, ErrorCode::IncompatibleOutputProfile);
            assert!(e.message.contains("CMYK"), "{e:?}");
        }
        for f in [ExportFormat::Psd, ExportFormat::Pdf] {
            assert!(o.validate(f).is_ok());
            // Untagged CMYK is allowed (common in print workflows; the exporter warns).
            assert!(ExportOptions { embed_color_profile: false, ..o.clone() }.validate(f).is_ok());
            assert!(ExportOptions { bit_depth: 16, ..o.clone() }.validate(f).is_ok());
            let empty = ExportOptions { cmyk_profile: Some("  ".into()), ..o.clone() };
            assert_eq!(empty.validate(f).unwrap_err().code, ErrorCode::InvalidArgument);
            let long = ExportOptions { cmyk_profile: Some("x".repeat(MAX_PROFILE_PATH_BYTES + 1)), ..o.clone() };
            assert_eq!(long.validate(f).unwrap_err().code, ErrorCode::InvalidArgument);
        }
        // A stale path is ignored for RGB targets.
        let rgb = ExportOptions { cmyk_profile: Some(String::new()), ..Default::default() };
        assert!(rgb.validate(ExportFormat::Png).is_ok());
        assert_eq!(ExportProfile::Cmyk.as_str(), "cmyk");
        assert_eq!(serde_json::to_string(&ExportIntent::Perceptual).unwrap(), "\"perceptual\"");
        assert_eq!(ExportIntent::RelativeColorimetric.as_str(), "relative_colorimetric");
        let info = CmykProfileInfo { description: "d".into(), bundled: true };
        assert_eq!(serde_json::to_string(&info).unwrap(), r#"{"description":"d","bundled":true}"#);
    }

    #[test]
    fn color_options_are_validated() {
        for depth in [0u8, 1, 12, 32, 255] {
            let o = ExportOptions { bit_depth: depth, ..Default::default() };
            assert_eq!(o.validate(ExportFormat::Png).unwrap_err().code, ErrorCode::InvalidArgument);
        }
        for p in [ExportProfile::DisplayP3, ExportProfile::AdobeRgb] {
            let o = ExportOptions { profile: p, embed_color_profile: false, ..Default::default() };
            for f in [ExportFormat::Png, ExportFormat::Psd, ExportFormat::Svg, ExportFormat::Pdf] {
                assert_eq!(o.validate(f).unwrap_err().code, ErrorCode::IncompatibleOutputProfile);
            }
            assert!(
                ExportOptions { profile: p, bit_depth: 16, ..Default::default() }.validate(ExportFormat::Psd).is_ok()
            );
        }
        for p in [ExportProfile::Document, ExportProfile::Srgb] {
            let o = ExportOptions { profile: p, embed_color_profile: false, ..Default::default() };
            assert!(o.validate(ExportFormat::Png).is_ok());
        }
        assert_eq!(ExportProfile::AdobeRgb.as_str(), "adobe_rgb");
        assert_eq!(serde_json::to_string(&ExportProfile::DisplayP3).unwrap(), "\"display_p3\"");
    }

    #[test]
    fn bad_scale_rejected() {
        let o = ExportOptions { raster_scale: 0.0, ..Default::default() };
        assert!(o.validate(ExportFormat::Png).is_err());
        let o = ExportOptions { raster_scale: f32::NAN, ..Default::default() };
        assert!(o.validate(ExportFormat::Png).is_err());
    }
}
