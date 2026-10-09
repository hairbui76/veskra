//! Persistent stroke metadata (BRH-08). The committed tiles are authoritative for appearance;
//! a stroke record preserves the *source* of those pixels: confirmed samples, brush identity,
//! seed and optional editable geometry.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use veskra_core::{ColorRgba, ErrorCode, LayerId, ProfileId, RectI, StrokeId, StrokeSample, VeskraError, limits};
use veskra_vector::BezPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeMode {
    /// Source-over paint.
    Paint,
    /// Destination-out pixel eraser.
    Erase,
}

/// How the stroke was composited into its layer when it was committed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StrokeComposition {
    pub mode: StrokeMode,
    /// Stroke-level opacity applied to the accumulated stroke coverage.
    pub opacity: f32,
    /// Layer alpha lock was active (stroke could only recolor existing pixels).
    pub alpha_locked: bool,
    /// Stroke was clipped by an active selection.
    pub selection_clipped: bool,
}

/// Editable vector interpretation of a vector-eligible stroke (VEC-01/02).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokeGeometry {
    /// Fitted centerline (document coordinates).
    pub centerline: BezPath,
    /// Closed variable-width outline, nonzero fill rule (document coordinates).
    pub outline: BezPath,
    /// Max distance of confirmed samples from `centerline`, document px (PNT-010).
    pub fit_error: f64,
}

/// A committed stroke. Only confirmed samples are representable (constructor enforces it).
/// Not serde-serializable on purpose: the native format owns its on-disk representation
/// (binary sample entries) and rebuilds records through [`StrokeRecord::new`] validation.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeRecord {
    pub id: StrokeId,
    pub layer: LayerId,
    samples: Arc<[StrokeSample]>,
    pub brush_id: String,
    pub brush_version: u32,
    /// Serialized brush parameters at commit time (opaque to the document; owned by veskra-brush).
    pub brush_params: String,
    pub color: ColorRgba,
    pub deterministic_seed: u64,
    pub color_profile: ProfileId,
    pub composition: StrokeComposition,
    /// The brush declared this stroke representable as an editable vector outline.
    pub vector_eligible: bool,
    pub geometry: Option<StrokeGeometry>,
    /// Pixel bounds the stroke touched.
    pub bounds: RectI,
}

impl StrokeRecord {
    /// Build a record, validating every sample. Rejects predicted/unconfirmed samples (STY-007).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: StrokeId,
        layer: LayerId,
        samples: Vec<StrokeSample>,
        brush_id: impl Into<String>,
        brush_version: u32,
        brush_params: String,
        color: ColorRgba,
        deterministic_seed: u64,
        color_profile: ProfileId,
        composition: StrokeComposition,
        vector_eligible: bool,
        geometry: Option<StrokeGeometry>,
        bounds: RectI,
    ) -> Result<Self, VeskraError> {
        Self::check_samples(&samples)?;
        if !color.is_valid() {
            return Err(VeskraError::invalid("stroke color out of range"));
        }
        if !(composition.opacity.is_finite() && (0.0..=1.0).contains(&composition.opacity)) {
            return Err(VeskraError::invalid("stroke opacity out of range"));
        }
        Ok(Self {
            id,
            layer,
            samples: samples.into(),
            brush_id: brush_id.into(),
            brush_version,
            brush_params,
            color,
            deterministic_seed,
            color_profile,
            composition,
            vector_eligible,
            geometry,
            bounds,
        })
    }

    fn check_samples(samples: &[StrokeSample]) -> Result<(), VeskraError> {
        if samples.is_empty() {
            return Err(VeskraError::invalid("stroke has no samples"));
        }
        if samples.len() > limits::MAX_STROKE_SAMPLES {
            return Err(VeskraError::new(ErrorCode::AllocationLimit, "stroke exceeds sample limit"));
        }
        let mut last_t = 0u64;
        for (i, s) in samples.iter().enumerate() {
            s.validate()?;
            if !s.is_confirmed() {
                return Err(VeskraError::invalid("only confirmed samples may be committed"));
            }
            if i > 0 && s.t_ns < last_t {
                return Err(VeskraError::invalid("stroke timestamps not monotonic"));
            }
            last_t = s.t_ns;
        }
        Ok(())
    }

    pub fn samples(&self) -> &[StrokeSample] {
        &self.samples
    }

    /// Copy of this record with new identity (used by layer duplication).
    pub fn with_identity(&self, id: StrokeId, layer: LayerId) -> StrokeRecord {
        StrokeRecord { id, layer, ..self.clone() }
    }

    /// Can this stroke be emitted as an editable vector path faithfully?
    pub fn has_vector_form(&self) -> bool {
        self.vector_eligible
            && self.geometry.is_some()
            && self.composition.mode == StrokeMode::Paint
            && !self.composition.alpha_locked
            && !self.composition.selection_clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veskra_core::SampleFlags;

    fn comp() -> StrokeComposition {
        StrokeComposition { mode: StrokeMode::Paint, opacity: 1.0, alpha_locked: false, selection_clipped: false }
    }

    fn rec(samples: Vec<StrokeSample>) -> Result<StrokeRecord, VeskraError> {
        StrokeRecord::new(
            StrokeId::from_u128(1),
            LayerId::from_u128(2),
            samples,
            "pen.technical",
            1,
            "{}".into(),
            ColorRgba::BLACK,
            42,
            ProfileId::new([0; 32], "sRGB"),
            comp(),
            true,
            None,
            RectI::new(0, 0, 10, 10),
        )
    }

    #[test]
    fn predicted_samples_rejected() {
        let mut s = StrokeSample::confirmed(1.0, 1.0, 5, 0.5);
        assert!(rec(vec![s]).is_ok());
        s.flags = SampleFlags::PREDICTED;
        assert!(rec(vec![s]).is_err());
        assert!(rec(vec![]).is_err());
    }

    #[test]
    fn non_monotonic_rejected() {
        let a = StrokeSample::confirmed(1.0, 1.0, 10, 0.5);
        let b = StrokeSample::confirmed(2.0, 1.0, 5, 0.5);
        assert!(rec(vec![a, b]).is_err());
    }

    #[test]
    fn vector_form_requires_geometry_and_paint() {
        let r = rec(vec![StrokeSample::confirmed(1.0, 1.0, 5, 0.5)]).unwrap();
        assert!(!r.has_vector_form());
        let mut g = r.clone();
        g.geometry =
            Some(StrokeGeometry { centerline: BezPath::default(), outline: BezPath::default(), fit_error: 0.0 });
        assert!(g.has_vector_form());
        g.composition.mode = StrokeMode::Erase;
        assert!(!g.has_vector_form());
    }
}
