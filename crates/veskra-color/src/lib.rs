//! # veskra-color — ICC profiles, transfer functions, transforms, Lab/CIEDE2000 (CLR)
//!
//! Pure safe Rust (ADR-006). Scope for v1:
//!
//! * [`transfer`]: exact IEC 61966-2.1 sRGB curve, lookup tables, ICC `curv`/`para` curves.
//! * [`icc`]: bounded parser for RGB matrix/TRC and Gray TRC profiles (ICC v2/v4); Veskra's own
//!   generated sRGB and Display P3 v4 profiles.
//! * [`transform`]: matrix/TRC transforms through PCS XYZ D50 with documented intent and
//!   clipping semantics, exact identity passthrough.
//! * [`lab`]: CIELAB D50 and CIEDE2000 for numeric fidelity checks.
//! * [`convert`]: the single implementation of all alpha/bit-depth conversions (CONTRACTS §1).
//!
//! LUT-based profiles (CMYK, A2B-only) are rejected with typed errors rather than approximated.
//! Accuracy against LittleCMS and colour-science is measured in `docs/reports/color-baseline.md`.

pub mod convert;
pub mod error;
pub mod icc;
pub mod lab;
pub mod math;
pub mod transfer;
pub mod transform;

pub use error::ColorError;
pub use icc::IccProfile;
pub use transform::{RenderingIntent, RgbTransform};

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use veskra_core::{BlendSpace, ProfileId};

/// Runtime color identity of a document: its working profile, whether that profile was
/// assumed (untagged import, MASTER_PLAN §6.2) and its blend-space policy (ADR-005).
#[derive(Debug, Clone)]
pub struct DocumentColor {
    pub profile: Arc<IccProfile>,
    /// `true` when the profile was assumed rather than read from the source.
    pub assumed: bool,
    pub blend_space: BlendSpace,
}

impl Default for DocumentColor {
    /// Veskra sRGB, explicitly chosen (not assumed), gamma-encoded blending.
    fn default() -> Self {
        Self { profile: IccProfile::srgb(), assumed: false, blend_space: BlendSpace::GammaEncoded }
    }
}

impl DocumentColor {
    /// sRGB **assumed** for untagged input (`assumed = true`, COL-004).
    pub fn assumed_srgb() -> Self {
        Self { assumed: true, ..Self::default() }
    }

    /// Serializable metadata for this color identity.
    pub fn meta(&self) -> ColorMeta {
        ColorMeta { profile: self.profile.id(), assumed: self.assumed, blend_space: self.blend_space }
    }

    /// Rebuild from saved metadata and the saved ICC bytes. Fails with
    /// [`ColorError::ProfileHashMismatch`] if the bytes are not the ones the metadata names
    /// (no silent profile substitution, COL-005), or with the parse error.
    pub fn from_meta(meta: &ColorMeta, icc_bytes: &[u8]) -> Result<Self, ColorError> {
        let profile = if icc_bytes == IccProfile::srgb().bytes() {
            IccProfile::srgb()
        } else {
            Arc::new(IccProfile::parse(icc_bytes)?)
        };
        if profile.id().icc_hash != meta.profile.icc_hash {
            return Err(ColorError::ProfileHashMismatch);
        }
        Ok(Self { profile, assumed: meta.assumed, blend_space: meta.blend_space })
    }
}

/// Serializable document color metadata (stored in the native manifest; the ICC bytes are
/// stored alongside and verified against `profile.icc_hash`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorMeta {
    pub profile: ProfileId,
    pub assumed: bool,
    pub blend_space: BlendSpace,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_color_meta_roundtrip() {
        let dc = DocumentColor::assumed_srgb();
        let meta = dc.meta();
        assert!(meta.assumed);
        assert_eq!(meta.profile.semantic_name, "sRGB IEC61966-2.1");
        let json = serde_json::to_string(&meta).unwrap();
        let back: ColorMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back, meta);
        let restored = DocumentColor::from_meta(&back, IccProfile::srgb().bytes()).unwrap();
        assert!(restored.assumed);
        assert!(Arc::ptr_eq(&restored.profile, &IccProfile::srgb()));
        let e = DocumentColor::from_meta(&back, IccProfile::display_p3().bytes()).unwrap_err();
        assert_eq!(e, ColorError::ProfileHashMismatch);
        assert!(DocumentColor::from_meta(&back, b"junk").is_err());
        let d = DocumentColor::default();
        assert!(!d.assumed);
        assert_eq!(d.blend_space, BlendSpace::GammaEncoded);
    }
}
