//! Brush presets (BRH-01/02, ADR-019): stable ids, versioned JSON schema, migration, validation
//! and the vector-eligibility honesty rule. Built-in presets live in [`crate::builtin`].
//!
//! ## Schema history
//! * **schema 1** (BRH-01 draft layout): spacing was stored as `spacing_percent` (percent of the
//!   dab diameter); `stabilization` and `tilt_size` did not exist.
//! * **schema 2**: `spacing` is a fraction of the diameter; adds `stabilization` and `tilt_size`.
//!   Flat fields `kind`, `hardness`, `min_size_ratio`, `pressure_size`, `pressure_opacity`,
//!   `pressure_flow`, `tilt_size`, `grain`.
//! * **schema 3** (current, [`PRESET_SCHEMA_VERSION`], brush system v2): `category`, `source`,
//!   `folder`; a [`Tip`] (round or image, angle, roundness, rotation); per-dab [`Jitter`]; a
//!   [`Texture`] (procedural grain or image, fixed or following the dab); a [`Dynamics`] table
//!   (pressure / tilt / velocity × size / opacity / flow / texture / scatter); `wet_edges`.
//!
//! Migration 2 → 3 is exact: `kind` → `category`, `hardness` → `tip.hardness`,
//! `min_size_ratio` + `pressure_size` → `dynamics.pressure.size` (`min` = `min_size_ratio`),
//! `pressure_opacity`/`pressure_flow` → `dynamics.pressure.opacity/flow` (always enabled, as in
//! schema 2), `tilt_size.max_scale` → `dynamics.tilt.size` (`min` 1, `max` = `max_scale`,
//! identity curve), `grain` → `texture` (`source: procedural`, `depth` = `strength`, `mode:
//! fixed`). A migrated schema-2 preset renders byte-identically (pinned by
//! `tests/legacy_regression.rs`); the only arithmetic change is the tilt factor, which now goes
//! through the curve evaluator (≤ 1e-6 relative; no built-in ever used `tilt_size`).
//!
//! Loading ([`load_preset_json`]) is tolerant: unknown keys are ignored *and reported*
//! ([`PresetLoad::ignored_keys`]), missing fields take documented defaults, older schemas are
//! migrated step by step, newer schemas are rejected with a typed error. The preset `id` is
//! never changed by migration (PNT-003).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::{BrushError, ResponseCurve};

/// Current preset JSON schema.
pub const PRESET_SCHEMA_VERSION: u32 = 3;
/// Maximum preset JSON size accepted by [`load_preset_json`].
pub const MAX_PRESET_JSON_BYTES: usize = 256 * 1024;
/// Largest brush diameter setting in document pixels.
pub const MAX_BRUSH_SIZE_PX: f32 = 1000.0;
/// Largest rendered dab diameter (size × dynamics), document pixels.
pub const MAX_DAB_DIAMETER_PX: f32 = 4000.0;
/// Rope length (document px) of the stabilizer at `stabilization = 1.0`.
pub const MAX_STABILIZER_ROPE_PX: f32 = 40.0;
/// Smallest / largest dab spacing (fraction of the diameter).
pub const MIN_SPACING: f32 = 0.005;
pub const MAX_SPACING: f32 = 4.0;
/// Largest scatter jitter (multiples of the dab diameter).
pub const MAX_SCATTER: f32 = 4.0;
/// Range of a dynamics target's `min` / `max` factor.
pub const MAX_DYNAMICS_FACTOR: f32 = 4.0;
/// Pen speed (document px per millisecond) that maps to velocity input 1.0.
pub const VELOCITY_FULL_SCALE_PX_PER_MS: f32 = 4.0;
/// Smoothing time constant (ms) of the velocity input.
pub const VELOCITY_SMOOTHING_MS: f32 = 30.0;
/// Smallest tip roundness.
pub const MIN_ROUNDNESS: f32 = 0.05;
/// Largest `folder` length in bytes.
pub const MAX_FOLDER_BYTES: usize = 255;

/// UI category of a brush (grouping only; behaviour is described by the other fields).
/// `watercolor`, `calligraphy`, `nature` and `pattern` were added by ADR-019 amendment 1; a build
/// that predates them loads them as `custom` (`#[serde(other)]`), as it does any unknown value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BrushCategory {
    Pencil,
    Ink,
    Marker,
    Paint,
    Airbrush,
    Texture,
    Effect,
    Eraser,
    Smudge,
    Imported,
    Watercolor,
    Calligraphy,
    Nature,
    Pattern,
    /// Also what an unknown category written by a newer build loads as.
    #[default]
    #[serde(other)]
    Custom,
}

impl BrushCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            BrushCategory::Pencil => "pencil",
            BrushCategory::Ink => "ink",
            BrushCategory::Marker => "marker",
            BrushCategory::Paint => "paint",
            BrushCategory::Airbrush => "airbrush",
            BrushCategory::Texture => "texture",
            BrushCategory::Effect => "effect",
            BrushCategory::Eraser => "eraser",
            BrushCategory::Smudge => "smudge",
            BrushCategory::Imported => "imported",
            BrushCategory::Watercolor => "watercolor",
            BrushCategory::Calligraphy => "calligraphy",
            BrushCategory::Nature => "nature",
            BrushCategory::Pattern => "pattern",
            BrushCategory::Custom => "custom",
        }
    }
}

/// Where a preset came from. JSON: `"builtin"`, `"custom"` or `"imported:<format>"` where
/// `<format>` is 1..=32 chars of `[a-z0-9_]` (e.g. `imported:infinite_painter`). Unknown strings
/// load as `custom`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum BrushSource {
    Builtin,
    #[default]
    Custom,
    Imported(String),
}

impl BrushSource {
    pub fn as_string(&self) -> String {
        match self {
            BrushSource::Builtin => "builtin".into(),
            BrushSource::Custom => "custom".into(),
            BrushSource::Imported(f) => format!("imported:{f}"),
        }
    }

    pub fn parse(s: &str) -> Option<BrushSource> {
        match s {
            "builtin" => Some(BrushSource::Builtin),
            "custom" => Some(BrushSource::Custom),
            _ => {
                let f = s.strip_prefix("imported:")?;
                let ok = !f.is_empty()
                    && f.len() <= 32
                    && f.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
                ok.then(|| BrushSource::Imported(f.to_string()))
            }
        }
    }
}

impl Serialize for BrushSource {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_string())
    }
}

impl<'de> Deserialize<'de> for BrushSource {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(BrushSource::parse(&s).unwrap_or(BrushSource::Custom))
    }
}

/// How dabs combine inside one stroke's coverage buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Accumulation {
    /// `m = max(m, dab)`: overlapping dabs of one stroke never darken (ink, pencil, marker).
    #[default]
    Max,
    /// `m = m + dab·(1 − m)`: coverage builds up where dabs overlap (airbrush).
    BuildUp,
}

/// Tip footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TipShape {
    /// Analytic disc (ellipse when `roundness < 1`) with `hardness` falloff.
    #[default]
    Round,
    /// Grayscale coverage mask asset (`tip.image` = content hash).
    Image,
}

/// What turns the tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TipRotation {
    /// `angle` only.
    #[default]
    Fixed,
    /// `angle` + the stroke direction at the dab (0° = moving right).
    Direction,
    /// `angle` − the hardware azimuth (0° = pen pointing up). Samples without a reported azimuth
    /// use `angle` alone; an azimuth is never invented.
    Pen,
}

/// Brush tip. Angles are degrees, counter-clockwise on screen (Photoshop / Infinite Painter
/// convention).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tip {
    pub shape: TipShape,
    /// Round tips: `1` = hard edge (1-px antialiased), `0` = smooth falloff over the radius.
    pub hardness: f32,
    /// Image tips: content hash (64 lowercase hex) of the coverage-mask asset; `null` otherwise.
    pub image: Option<String>,
    /// `-360..=360` degrees.
    pub angle: f32,
    /// Height/width of the footprint, `0.05..=1`.
    pub roundness: f32,
    pub rotation: TipRotation,
}

impl Default for Tip {
    fn default() -> Self {
        Self {
            shape: TipShape::Round,
            hardness: 1.0,
            image: None,
            angle: 0.0,
            roundness: 1.0,
            rotation: TipRotation::Fixed,
        }
    }
}

/// Random variation, deterministic from the stroke seed (and the dab index for per-dab values).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Jitter {
    /// Per dab: diameter × `1 − size·u` (`0..=1`).
    pub size: f32,
    /// Per dab: tip angle ± `angle·180°` (`0..=1`).
    pub angle: f32,
    /// Per dab: alpha × `1 − opacity·u` (`0..=1`).
    pub opacity: f32,
    /// Per dab: alpha × `1 − flow·u` (`0..=1`).
    pub flow: f32,
    /// Per dab: offset perpendicular to the stroke, ± `scatter × diameter` (`0..=4`).
    pub scatter: f32,
    /// Per dab color variation: hue ± `hue·180°`, saturation / brightness ± value (`0..=1`).
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    /// Once per stroke (same ranges).
    pub stroke_hue: f32,
    pub stroke_saturation: f32,
    pub stroke_brightness: f32,
}

impl Jitter {
    pub fn is_zero(&self) -> bool {
        *self == Jitter::default()
    }
    /// Per-dab color jitter is active (needs the per-stroke color buffer).
    pub fn per_dab_color(&self) -> bool {
        self.hue > 0.0 || self.saturation > 0.0 || self.brightness > 0.0
    }
    pub fn per_stroke_color(&self) -> bool {
        self.stroke_hue > 0.0 || self.stroke_saturation > 0.0 || self.stroke_brightness > 0.0
    }
}

/// Texture source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextureSource {
    /// Seeded two-octave value noise (the schema-2 `grain`).
    #[default]
    Procedural,
    /// Tiled grayscale mask asset (`texture.image`).
    Image,
}

/// Where the texture lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextureMode {
    /// Document space: the texture does not swim with the dabs (paper grain).
    #[default]
    Fixed,
    /// Dab space: the texture moves and turns with every dab (bristle streaks).
    Follow,
}

/// Paper / grain texture multiplying dab alpha by `1 − depth · t`, where `t` is the procedural
/// noise value (high = less paint) or, for images, `1 − mask` (white texels let paint through).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Texture {
    pub source: TextureSource,
    /// Image textures: content hash of the mask asset; `null` for procedural.
    pub image: Option<String>,
    /// Procedural: feature size in document px (`0.5..=256`). Image: texel size in document px
    /// (`0.05..=64`).
    pub scale: f32,
    /// Strength `0..=1`.
    pub depth: f32,
    pub invert: bool,
    pub mode: TextureMode,
}

impl Default for Texture {
    fn default() -> Self {
        Self {
            source: TextureSource::Procedural,
            image: None,
            scale: 2.0,
            depth: 0.5,
            invert: false,
            mode: TextureMode::Fixed,
        }
    }
}

/// One input → target response: `factor = min + (max − min) · curve(input)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DynamicsTarget {
    pub enabled: bool,
    pub curve: ResponseCurve,
    /// `0..=4`.
    pub min: f32,
    /// `0..=4`.
    pub max: f32,
}

impl Default for DynamicsTarget {
    fn default() -> Self {
        Self { enabled: false, curve: ResponseCurve::identity(), min: 0.0, max: 1.0 }
    }
}

impl DynamicsTarget {
    /// Enabled target with `curve` and the `[0, 1]` factor range.
    pub fn on(curve: ResponseCurve) -> Self {
        Self { enabled: true, curve, min: 0.0, max: 1.0 }
    }

    /// Enabled target with an explicit factor range.
    pub fn range(curve: ResponseCurve, min: f32, max: f32) -> Self {
        Self { enabled: true, curve, min, max }
    }

    /// Factor for input `x` (no enabled check).
    #[inline]
    pub fn factor(&self, x: f32) -> f32 {
        self.min + (self.max - self.min) * self.curve.eval(x)
    }

    /// Disabled, or a constant factor of exactly 1.
    pub fn is_unity(&self) -> bool {
        !self.enabled || (self.curve.is_constant() && self.factor(0.0) == 1.0)
    }

    fn validate(&self, what: &str) -> Result<(), BrushError> {
        self.curve.validate_shape().map_err(|e| BrushError::InvalidPreset(format!("{what}: {e}")))?;
        let ok = |v: f32| v.is_finite() && (0.0..=MAX_DYNAMICS_FACTOR).contains(&v);
        if !ok(self.min) || !ok(self.max) {
            return Err(BrushError::InvalidPreset(format!("{what}: min and max must be in [0, 4]")));
        }
        Ok(())
    }
}

/// Targets driven by one input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DynamicsSource {
    pub size: DynamicsTarget,
    pub opacity: DynamicsTarget,
    pub flow: DynamicsTarget,
    pub texture: DynamicsTarget,
    pub scatter: DynamicsTarget,
}

impl DynamicsSource {
    pub fn any_enabled(&self) -> bool {
        self.targets().iter().any(|(_, t)| t.enabled)
    }
    fn targets(&self) -> [(&'static str, &DynamicsTarget); 5] {
        [
            ("size", &self.size),
            ("opacity", &self.opacity),
            ("flow", &self.flow),
            ("texture", &self.texture),
            ("scatter", &self.scatter),
        ]
    }
}

/// Input sources: `pressure` (hardware pressure, or the engine's documented fallback 0.5 when
/// the device reports none), `tilt` (hardware tilt / (π/2); targets are skipped for samples
/// without tilt) and `velocity` (smoothed pen speed from sample timestamps, see
/// [`VELOCITY_FULL_SCALE_PX_PER_MS`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Dynamics {
    pub pressure: DynamicsSource,
    pub tilt: DynamicsSource,
    pub velocity: DynamicsSource,
}

/// Per-dab inputs of the dynamics table.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DabInput {
    /// `0..=1`.
    pub pressure: f32,
    /// Radians from perpendicular, only when the hardware reported it.
    pub tilt: Option<f32>,
    /// `0..=1` (smoothed speed / full scale).
    pub velocity: f32,
}

impl DabInput {
    fn tilt01(&self) -> Option<f32> {
        self.tilt.filter(|t| t.is_finite()).map(|t| (t / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0))
    }
}

/// A brush preset (ADR-004, ADR-019). See the module docs for the schema and load rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushPreset {
    /// JSON schema version ([`PRESET_SCHEMA_VERSION`] after loading).
    pub schema: u32,
    /// Stable id (`[a-z0-9._-]`, 1..=64 bytes). Never changed by migration.
    pub id: String,
    /// Content revision of this preset id (recorded in stroke metadata).
    pub version: u32,
    pub name: String,
    pub category: BrushCategory,
    pub source: BrushSource,
    /// Library grouping (pack or user folder); empty for built-ins.
    pub folder: String,
    /// Dab diameter at full dynamics, document px (`(0, 1000]`).
    pub size_px: f32,
    /// Stroke-level opacity (applied once to the accumulated stroke coverage).
    pub opacity: f32,
    /// Per-dab alpha (`(0, 1]`).
    pub flow: f32,
    /// Dab spacing as a fraction of the current diameter (`0.005..=4`).
    pub spacing: f32,
    pub tip: Tip,
    pub jitter: Jitter,
    pub texture: Option<Texture>,
    pub dynamics: Dynamics,
    /// Watercolor-style edge darkening `0..=1`: the stroke interior keeps `1 − 0.75·wet_edges`
    /// of its coverage, the rim keeps all of it.
    pub wet_edges: f32,
    pub accumulation: Accumulation,
    /// Destination-out brush (forces erase mode).
    pub eraser: bool,
    /// Strokes of this preset have a faithful editable vector form. Must satisfy
    /// [`BrushPreset::satisfies_vector_rule`]; validation rejects dishonest claims.
    pub vector_eligible: bool,
    /// Default stabilization `0..=1` (rope length `stabilization · MAX_STABILIZER_ROPE_PX`).
    pub stabilization: f32,
}

impl Default for BrushPreset {
    fn default() -> Self {
        Self {
            schema: PRESET_SCHEMA_VERSION,
            id: String::new(),
            version: 1,
            name: String::new(),
            category: BrushCategory::Custom,
            source: BrushSource::Custom,
            folder: String::new(),
            size_px: 10.0,
            opacity: 1.0,
            flow: 1.0,
            spacing: 0.1,
            tip: Tip::default(),
            jitter: Jitter::default(),
            texture: None,
            dynamics: Dynamics {
                pressure: DynamicsSource { size: DynamicsTarget::on(ResponseCurve::identity()), ..Default::default() },
                ..Default::default()
            },
            wet_edges: 0.0,
            accumulation: Accumulation::Max,
            eraser: false,
            vector_eligible: false,
            stabilization: 0.0,
        }
    }
}

/// Result of [`load_preset_json`].
#[derive(Debug, Clone, PartialEq)]
pub struct PresetLoad {
    pub preset: BrushPreset,
    /// Dotted paths of keys that were present in the input but not understood (sorted).
    pub ignored_keys: Vec<String>,
    /// Schema the input was migrated from, if it was older than current.
    pub migrated_from: Option<u32>,
}

fn in01(v: f32) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

/// Content hash rule for asset references: 64 lowercase hex characters.
pub fn is_asset_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl BrushPreset {
    /// The honesty rule for `vector_eligible` (ADR-004/019, MASTER_PLAN §5.2): a solid,
    /// untextured, round (circular) paint brush whose per-dab alpha is constant 1 under `Max`
    /// accumulation, without jitter, wet edges or velocity dynamics, so its coverage is exactly a
    /// union of discs that a filled outline reproduces. Pressure- and tilt-driven *size* are
    /// allowed (the outline follows them). The inherently textural categories (airbrush, texture,
    /// effect, and since ADR-019 amendment 1 watercolor, nature and pattern) are never eligible;
    /// `calligraphy` is judged by the brush's features like `ink` or `paint`.
    pub fn satisfies_vector_rule(&self) -> bool {
        let d = &self.dynamics;
        !self.eraser
            && !matches!(
                self.category,
                BrushCategory::Eraser
                    | BrushCategory::Airbrush
                    | BrushCategory::Texture
                    | BrushCategory::Smudge
                    | BrushCategory::Effect
                    | BrushCategory::Watercolor
                    | BrushCategory::Nature
                    | BrushCategory::Pattern
            )
            && self.tip.shape == TipShape::Round
            && self.tip.roundness == 1.0
            && self.tip.hardness >= 0.5
            && self.texture.is_none()
            && self.wet_edges == 0.0
            && self.accumulation == Accumulation::Max
            && self.flow == 1.0
            && self.jitter.is_zero()
            && !d.velocity.any_enabled()
            && d.pressure.opacity.is_unity()
            && d.pressure.flow.is_unity()
            && d.tilt.opacity.is_unity()
            && d.tilt.flow.is_unity()
    }

    /// Diameter in document px for `pressure` (and optional hardware tilt) with base size `size`
    /// and zero velocity. See [`BrushPreset::dab_diameter`].
    pub fn diameter(&self, size: f32, pressure: f32, tilt: Option<f32>) -> f32 {
        self.dab_diameter(size, &DabInput { pressure, tilt, velocity: 0.0 })
    }

    /// Diameter in document px at `inp` with base size `size`, before jitter, clamped to
    /// [`MAX_DAB_DIAMETER_PX`]. Disabled targets and samples without tilt contribute exactly 1.
    pub fn dab_diameter(&self, size: f32, inp: &DabInput) -> f32 {
        let d = &self.dynamics;
        let mut v = size;
        if d.pressure.size.enabled {
            v *= d.pressure.size.factor(inp.pressure);
        }
        if d.tilt.size.enabled
            && let Some(t) = inp.tilt01()
        {
            v *= d.tilt.size.factor(t);
        }
        if d.velocity.size.enabled {
            v *= d.velocity.size.factor(inp.velocity);
        }
        if v > MAX_DAB_DIAMETER_PX { MAX_DAB_DIAMETER_PX } else { v }
    }

    /// Per-dab alpha (`flow × flow dynamics × opacity dynamics`) for `pressure` at zero velocity
    /// and no tilt.
    pub fn dab_alpha(&self, pressure: f32) -> f32 {
        self.dab_alpha_at(&DabInput { pressure, tilt: None, velocity: 0.0 })
    }

    /// Per-dab alpha at `inp`, before jitter, clamped to `[0, 1]`.
    pub fn dab_alpha_at(&self, inp: &DabInput) -> f32 {
        let d = &self.dynamics;
        let t = inp.tilt01();
        let mut a = self.flow;
        for (target, x) in
            [(&d.pressure.flow, Some(inp.pressure)), (&d.tilt.flow, t), (&d.velocity.flow, Some(inp.velocity))]
                .into_iter()
                .chain([
                    (&d.pressure.opacity, Some(inp.pressure)),
                    (&d.tilt.opacity, t),
                    (&d.velocity.opacity, Some(inp.velocity)),
                ])
        {
            if target.enabled
                && let Some(x) = x
            {
                a *= target.factor(x);
            }
        }
        a.clamp(0.0, 1.0)
    }

    /// Product of the enabled factors of one target kind (`texture` or `scatter`) at `inp`.
    pub(crate) fn target_factor(&self, inp: &DabInput, pick: fn(&DynamicsSource) -> &DynamicsTarget) -> f32 {
        let d = &self.dynamics;
        let mut f = 1.0f32;
        for (src, x) in [(&d.pressure, Some(inp.pressure)), (&d.tilt, inp.tilt01()), (&d.velocity, Some(inp.velocity))]
        {
            let t = pick(src);
            if t.enabled
                && let Some(x) = x
            {
                f *= t.factor(x);
            }
        }
        f
    }

    /// Asset hashes this preset references (tip image, texture image), deduplicated, sorted.
    pub fn asset_refs(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if self.tip.shape == TipShape::Image
            && let Some(h) = &self.tip.image
        {
            v.push(h.clone());
        }
        if let Some(t) = &self.texture
            && t.source == TextureSource::Image
            && let Some(h) = &t.image
        {
            v.push(h.clone());
        }
        v.sort();
        v.dedup();
        v
    }

    pub fn validate(&self) -> Result<(), BrushError> {
        let bad = |m: &str| Err(BrushError::InvalidPreset(m.to_string()));
        if self.schema != PRESET_SCHEMA_VERSION {
            return bad("schema must be current after migration");
        }
        if !id_is_valid(&self.id) {
            return bad("id must be 1..=64 chars of [a-z0-9._-]");
        }
        if self.name.len() > veskra_core::limits::MAX_NAME_BYTES || self.name.chars().any(char::is_control) {
            return bad("name must be at most 255 bytes without control characters");
        }
        if self.folder.len() > MAX_FOLDER_BYTES || self.folder.chars().any(char::is_control) {
            return bad("folder must be at most 255 bytes without control characters");
        }
        if !(self.size_px.is_finite() && self.size_px > 0.0 && self.size_px <= MAX_BRUSH_SIZE_PX) {
            return bad("size_px must be in (0, 1000]");
        }
        if !in01(self.opacity) || !in01(self.stabilization) || !in01(self.wet_edges) {
            return bad("opacity, stabilization and wet_edges must be in [0,1]");
        }
        if !(in01(self.flow) && self.flow > 0.0) {
            return bad("flow must be in (0,1]");
        }
        if !(self.spacing.is_finite() && (MIN_SPACING..=MAX_SPACING).contains(&self.spacing)) {
            return bad("spacing must be in [0.005, 4]");
        }
        let t = &self.tip;
        if !in01(t.hardness) {
            return bad("tip.hardness must be in [0,1]");
        }
        if !(t.angle.is_finite() && (-360.0..=360.0).contains(&t.angle)) {
            return bad("tip.angle must be in [-360, 360] degrees");
        }
        if !(t.roundness.is_finite() && (MIN_ROUNDNESS..=1.0).contains(&t.roundness)) {
            return bad("tip.roundness must be in [0.05, 1]");
        }
        match (t.shape, &t.image) {
            (TipShape::Round, None) => {}
            (TipShape::Round, Some(_)) => return bad("tip.image must be null for a round tip"),
            (TipShape::Image, Some(h)) if is_asset_hash(h) => {}
            (TipShape::Image, _) => return bad("tip.image must be a 64-hex asset hash for an image tip"),
        }
        let j = &self.jitter;
        for (v, n) in [
            (j.size, "size"),
            (j.angle, "angle"),
            (j.opacity, "opacity"),
            (j.flow, "flow"),
            (j.hue, "hue"),
            (j.saturation, "saturation"),
            (j.brightness, "brightness"),
            (j.stroke_hue, "stroke_hue"),
            (j.stroke_saturation, "stroke_saturation"),
            (j.stroke_brightness, "stroke_brightness"),
        ] {
            if !in01(v) {
                return Err(BrushError::InvalidPreset(format!("jitter.{n} must be in [0,1]")));
            }
        }
        if !(j.scatter.is_finite() && (0.0..=MAX_SCATTER).contains(&j.scatter)) {
            return bad("jitter.scatter must be in [0, 4]");
        }
        if let Some(tx) = &self.texture {
            if !in01(tx.depth) {
                return bad("texture.depth must be in [0,1]");
            }
            match (tx.source, &tx.image) {
                (TextureSource::Procedural, None) => {
                    if !(tx.scale.is_finite() && (0.5..=256.0).contains(&tx.scale)) {
                        return bad("procedural texture scale must be in [0.5, 256]");
                    }
                }
                (TextureSource::Image, Some(h)) if is_asset_hash(h) => {
                    if !(tx.scale.is_finite() && (0.05..=64.0).contains(&tx.scale)) {
                        return bad("image texture scale must be in [0.05, 64]");
                    }
                }
                (TextureSource::Procedural, Some(_)) => {
                    return bad("texture.image must be null for a procedural texture");
                }
                (TextureSource::Image, _) => return bad("texture.image must be a 64-hex asset hash"),
            }
        }
        for (sn, src) in [
            ("pressure", &self.dynamics.pressure),
            ("tilt", &self.dynamics.tilt),
            ("velocity", &self.dynamics.velocity),
        ] {
            for (tn, t) in src.targets() {
                t.validate(&format!("dynamics.{sn}.{tn}"))?;
            }
        }
        if self.vector_eligible && !self.satisfies_vector_rule() {
            return bad(
                "vector_eligible requires a solid, untextured, circular Max-accumulation paint brush without jitter, wet edges or velocity dynamics",
            );
        }
        Ok(())
    }

    /// A copy that always passes [`BrushPreset::validate`]: numeric fields clamped into range
    /// (non-finite → default), invalid curves → identity, invalid image references → round tip /
    /// no texture, invalid id replaced by `"invalid"`, and `vector_eligible` cleared unless the
    /// rule holds.
    pub fn sanitized(&self) -> BrushPreset {
        if self.validate().is_ok() {
            return self.clone();
        }
        let d = BrushPreset::default();
        let fix = |v: f32, lo: f32, hi: f32, dflt: f32| if v.is_finite() { v.clamp(lo, hi) } else { dflt };
        let target = |t: &DynamicsTarget| DynamicsTarget {
            enabled: t.enabled,
            curve: if t.curve.validate_shape().is_ok() { t.curve.clone() } else { ResponseCurve::identity() },
            min: fix(t.min, 0.0, MAX_DYNAMICS_FACTOR, 0.0),
            max: fix(t.max, 0.0, MAX_DYNAMICS_FACTOR, 1.0),
        };
        let source = |s: &DynamicsSource| DynamicsSource {
            size: target(&s.size),
            opacity: target(&s.opacity),
            flow: target(&s.flow),
            texture: target(&s.texture),
            scatter: target(&s.scatter),
        };
        let j = &self.jitter;
        let j01 = |v: f32| fix(v, 0.0, 1.0, 0.0);
        let image_ok = self.tip.shape == TipShape::Image && self.tip.image.as_deref().is_some_and(is_asset_hash);
        let tip = Tip {
            shape: if image_ok { TipShape::Image } else { TipShape::Round },
            hardness: fix(self.tip.hardness, 0.0, 1.0, 1.0),
            image: if image_ok { self.tip.image.clone() } else { None },
            angle: fix(self.tip.angle, -360.0, 360.0, 0.0),
            roundness: fix(self.tip.roundness, MIN_ROUNDNESS, 1.0, 1.0),
            rotation: self.tip.rotation,
        };
        let texture = self.texture.as_ref().and_then(|t| {
            let depth = fix(t.depth, 0.0, 1.0, 0.0);
            match t.source {
                TextureSource::Procedural if t.scale.is_finite() && (0.5..=256.0).contains(&t.scale) => {
                    Some(Texture { image: None, depth, ..t.clone() })
                }
                TextureSource::Image
                    if t.image.as_deref().is_some_and(is_asset_hash)
                        && t.scale.is_finite()
                        && (0.05..=64.0).contains(&t.scale) =>
                {
                    Some(Texture { depth, ..t.clone() })
                }
                _ => None,
            }
        });
        let mut p = BrushPreset {
            schema: PRESET_SCHEMA_VERSION,
            size_px: fix(self.size_px, 0.01, MAX_BRUSH_SIZE_PX, d.size_px),
            opacity: fix(self.opacity, 0.0, 1.0, d.opacity),
            flow: fix(self.flow, 0.001, 1.0, d.flow),
            spacing: fix(self.spacing, MIN_SPACING, MAX_SPACING, d.spacing),
            stabilization: fix(self.stabilization, 0.0, 1.0, 0.0),
            wet_edges: fix(self.wet_edges, 0.0, 1.0, 0.0),
            tip,
            jitter: Jitter {
                size: j01(j.size),
                angle: j01(j.angle),
                opacity: j01(j.opacity),
                flow: j01(j.flow),
                scatter: fix(j.scatter, 0.0, MAX_SCATTER, 0.0),
                hue: j01(j.hue),
                saturation: j01(j.saturation),
                brightness: j01(j.brightness),
                stroke_hue: j01(j.stroke_hue),
                stroke_saturation: j01(j.stroke_saturation),
                stroke_brightness: j01(j.stroke_brightness),
            },
            texture,
            dynamics: Dynamics {
                pressure: source(&self.dynamics.pressure),
                tilt: source(&self.dynamics.tilt),
                velocity: source(&self.dynamics.velocity),
            },
            ..self.clone()
        };
        if p.name.len() > veskra_core::limits::MAX_NAME_BYTES || p.name.chars().any(char::is_control) {
            p.name = String::new();
        }
        if p.folder.len() > MAX_FOLDER_BYTES || p.folder.chars().any(char::is_control) {
            p.folder = String::new();
        }
        if !id_is_valid(&p.id) {
            p.id = "invalid".into();
        }
        p.vector_eligible = p.vector_eligible && p.satisfies_vector_rule();
        p
    }

    /// Pretty JSON of a validated preset (current schema).
    pub fn to_json(&self) -> Result<String, BrushError> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|e| BrushError::Parse(e.to_string()))
    }

    /// Compact JSON of a validated preset (stroke records, `.vbrush`).
    pub fn to_json_compact(&self) -> Result<String, BrushError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| BrushError::Parse(e.to_string()))
    }
}

/// Stable id rule: 1..=64 bytes of `[a-z0-9._-]`.
pub fn id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// Parse, migrate and validate preset JSON (BRH-02).
pub fn load_preset_json(json: &str) -> Result<PresetLoad, BrushError> {
    if json.len() > MAX_PRESET_JSON_BYTES {
        return Err(BrushError::TooLarge { limit: MAX_PRESET_JSON_BYTES });
    }
    let v: Value = serde_json::from_str(json).map_err(|e| BrushError::Parse(e.to_string()))?;
    load_preset_value(v)
}

/// [`load_preset_json`] on an already parsed JSON value (the caller bounds its size).
pub fn load_preset_value(mut v: Value) -> Result<PresetLoad, BrushError> {
    let Some(obj) = v.as_object_mut() else {
        return Err(BrushError::Parse("preset must be a JSON object".into()));
    };
    // A preset without "schema" predates the field and is treated as schema 1.
    let schema = match obj.get("schema") {
        None => 1,
        Some(s) => s
            .as_u64()
            .and_then(|s| u32::try_from(s).ok())
            .ok_or_else(|| BrushError::Parse("schema must be an unsigned integer".into()))?,
    };
    if schema == 0 {
        return Err(BrushError::Parse("schema 0 is not a valid preset schema".into()));
    }
    if schema > PRESET_SCHEMA_VERSION {
        return Err(BrushError::UnsupportedSchema { found: schema, supported: PRESET_SCHEMA_VERSION });
    }
    let mut ignored_keys = Vec::new();
    let mut cur = schema;
    while cur < PRESET_SCHEMA_VERSION {
        cur = migrate_step(obj, cur, &mut ignored_keys)?;
    }
    obj.insert("schema".into(), Value::from(PRESET_SCHEMA_VERSION));
    let preset: BrushPreset = serde_json::from_value(v.clone()).map_err(|e| BrushError::Parse(e.to_string()))?;
    preset.validate()?;
    let canonical = serde_json::to_value(&preset).map_err(|e| BrushError::Parse(e.to_string()))?;
    diff_keys(&v, &canonical, "", &mut ignored_keys);
    ignored_keys.sort();
    ignored_keys.dedup();
    Ok(PresetLoad { preset, ignored_keys, migrated_from: (schema < PRESET_SCHEMA_VERSION).then_some(schema) })
}

fn curve_json(c: &ResponseCurve) -> Value {
    serde_json::to_value(c).unwrap_or(Value::Null)
}

fn target_json(enabled: bool, curve: Value, min: Value, max: Value) -> Value {
    let mut m = Map::new();
    m.insert("enabled".into(), Value::Bool(enabled));
    m.insert("curve".into(), curve);
    m.insert("min".into(), min);
    m.insert("max".into(), max);
    Value::Object(m)
}

/// Migration hook: transform an object of schema `from` to schema `from + 1`. Keys the step
/// moves into nested objects report their unknown sub-keys (original dotted path) in `ignored`.
fn migrate_step(obj: &mut Map<String, Value>, from: u32, ignored: &mut Vec<String>) -> Result<u32, BrushError> {
    match from {
        1 => {
            // spacing_percent (percent of diameter) → spacing (fraction). New fields default.
            if let Some(p) = obj.remove("spacing_percent") {
                let p = p.as_f64().ok_or_else(|| BrushError::Parse("spacing_percent must be a number".into()))?;
                obj.insert("spacing".into(), Value::from(p / 100.0));
            }
            Ok(2)
        }
        2 => {
            // Schema-2 defaults for absent fields (`BrushPreset::default()` of schema 2).
            let identity = curve_json(&ResponseCurve::identity());
            let category = match obj.remove("kind") {
                None => "paint",
                Some(Value::String(k)) => match k.as_str() {
                    "pencil" => "pencil",
                    "pen" => "ink",
                    "marker" => "marker",
                    "round" => "paint",
                    "airbrush" => "airbrush",
                    "charcoal" => "texture",
                    "eraser" => "eraser",
                    _ => "custom",
                },
                Some(_) => return Err(BrushError::Parse("kind must be a string".into())),
            };
            obj.insert("category".into(), Value::from(category));
            let builtin = obj
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| crate::builtin::LEGACY_BUILTIN_IDS.contains(&id));
            obj.insert("source".into(), Value::from(if builtin { "builtin" } else { "custom" }));

            let hardness = obj.remove("hardness").unwrap_or(Value::from(1.0));
            let mut tip = Map::new();
            tip.insert("shape".into(), Value::from("round"));
            tip.insert("hardness".into(), hardness);
            tip.insert("image".into(), Value::Null);
            tip.insert("angle".into(), Value::from(0.0));
            tip.insert("roundness".into(), Value::from(1.0));
            tip.insert("rotation".into(), Value::from("fixed"));
            obj.insert("tip".into(), Value::Object(tip));

            let min = obj.remove("min_size_ratio").unwrap_or(Value::from(0.0));
            let ps = obj.remove("pressure_size").unwrap_or_else(|| identity.clone());
            let po = obj.remove("pressure_opacity").unwrap_or_else(|| identity.clone());
            let pf = obj.remove("pressure_flow").unwrap_or_else(|| identity.clone());
            let mut pressure = Map::new();
            pressure.insert("size".into(), target_json(true, ps, min, Value::from(1.0)));
            pressure.insert("opacity".into(), target_json(true, po, Value::from(0.0), Value::from(1.0)));
            pressure.insert("flow".into(), target_json(true, pf, Value::from(0.0), Value::from(1.0)));
            let mut tilt = Map::new();
            match obj.remove("tilt_size") {
                None | Some(Value::Null) => {}
                Some(Value::Object(t)) => {
                    let ms = t
                        .get("max_scale")
                        .cloned()
                        .ok_or_else(|| BrushError::Parse("tilt_size.max_scale missing".into()))?;
                    ignored.extend(t.keys().filter(|k| *k != "max_scale").map(|k| format!("tilt_size.{k}")));
                    tilt.insert("size".into(), target_json(true, identity.clone(), Value::from(1.0), ms));
                }
                Some(_) => return Err(BrushError::Parse("tilt_size must be an object or null".into())),
            }
            let mut dynamics = Map::new();
            dynamics.insert("pressure".into(), Value::Object(pressure));
            dynamics.insert("tilt".into(), Value::Object(tilt));
            obj.insert("dynamics".into(), Value::Object(dynamics));

            match obj.remove("grain") {
                None | Some(Value::Null) => {
                    obj.insert("texture".into(), Value::Null);
                }
                Some(Value::Object(g)) => {
                    let scale =
                        g.get("scale").cloned().ok_or_else(|| BrushError::Parse("grain.scale missing".into()))?;
                    let strength =
                        g.get("strength").cloned().ok_or_else(|| BrushError::Parse("grain.strength missing".into()))?;
                    ignored
                        .extend(g.keys().filter(|k| *k != "scale" && *k != "strength").map(|k| format!("grain.{k}")));
                    let mut t = Map::new();
                    t.insert("source".into(), Value::from("procedural"));
                    t.insert("image".into(), Value::Null);
                    t.insert("scale".into(), scale);
                    t.insert("depth".into(), strength);
                    t.insert("invert".into(), Value::Bool(false));
                    t.insert("mode".into(), Value::from("fixed"));
                    obj.insert("texture".into(), Value::Object(t));
                }
                Some(_) => return Err(BrushError::Parse("grain must be an object or null".into())),
            }
            Ok(3)
        }
        _ => Err(BrushError::UnsupportedSchema { found: from, supported: PRESET_SCHEMA_VERSION }),
    }
}

/// Keys in `input` absent from `canonical` (recursively through objects and arrays).
fn diff_keys(input: &Value, canonical: &Value, prefix: &str, out: &mut Vec<String>) {
    match (input, canonical) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                match b.get(k) {
                    Some(vb) => diff_keys(va, vb, &path, out),
                    None => out.push(path),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (va, vb)) in a.iter().zip(b).enumerate() {
                diff_keys(va, vb, &format!("{prefix}[{i}]"), out);
            }
        }
        (Value::Object(a), _) => {
            // An object where the schema expects a scalar/null: all of its keys are ignored.
            for k in a.keys() {
                out.push(if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{builtin_preset, builtin_presets};

    #[test]
    fn builtins_valid_and_ids_stable() {
        let all = builtin_presets();
        let ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, crate::BUILTIN_IDS);
        for p in &all {
            p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
            assert_eq!(p.source, BrushSource::Builtin, "{}", p.id);
        }
    }

    #[test]
    fn dishonest_vector_claim_rejected() {
        let mut p = builtin_preset("charcoal").unwrap();
        p.vector_eligible = true;
        assert!(matches!(p.validate(), Err(BrushError::InvalidPreset(_))));
        for f in [
            |p: &mut BrushPreset| p.tip.roundness = 0.5,
            |p: &mut BrushPreset| p.jitter.size = 0.2,
            |p: &mut BrushPreset| p.jitter.hue = 0.1,
            |p: &mut BrushPreset| p.jitter.scatter = 0.3,
            |p: &mut BrushPreset| p.wet_edges = 0.3,
            |p: &mut BrushPreset| p.dynamics.velocity.size = DynamicsTarget::on(ResponseCurve::identity()),
            |p: &mut BrushPreset| {
                p.tip.shape = TipShape::Image;
                p.tip.image = Some("a".repeat(64));
            },
        ] {
            let mut q = builtin_preset("round.hard").unwrap();
            assert!(q.vector_eligible);
            f(&mut q);
            assert!(!q.satisfies_vector_rule());
            assert!(matches!(q.validate(), Err(BrushError::InvalidPreset(_))));
        }
        // Tilt-driven size keeps a faithful outline.
        let mut q = builtin_preset("round.hard").unwrap();
        q.dynamics.tilt.size = DynamicsTarget::range(ResponseCurve::identity(), 1.0, 2.0);
        q.validate().unwrap();
    }

    #[test]
    fn newer_schema_rejected_and_bad_json() {
        let e = load_preset_json(r#"{"schema": 99, "id": "x"}"#).unwrap_err();
        assert_eq!(e, BrushError::UnsupportedSchema { found: 99, supported: PRESET_SCHEMA_VERSION });
        assert!(matches!(load_preset_json("[1,2]"), Err(BrushError::Parse(_))));
        assert!(matches!(load_preset_json(r#"{"id": "x", "size_px": "big"}"#), Err(BrushError::Parse(_))));
        assert!(matches!(load_preset_json(r#"{"id": "x", "size_px": -3}"#), Err(BrushError::InvalidPreset(_))));
        let big = format!("{{\"id\":\"x\",\"pad\":\"{}\"}}", "a".repeat(MAX_PRESET_JSON_BYTES));
        assert!(matches!(load_preset_json(&big), Err(BrushError::TooLarge { .. })));
    }

    #[test]
    fn sanitized_always_valid() {
        let mut p = builtin_preset("pen.brush").unwrap();
        p.size_px = f32::NAN;
        p.spacing = 0.0;
        p.tip.hardness = 7.0;
        p.tip.roundness = 0.0;
        p.id = "Bad Id".into();
        p.dynamics.pressure.size.curve = ResponseCurve::constant(1.0);
        p.dynamics.pressure.size.min = 9.0;
        p.texture = Some(Texture { scale: 1.0, depth: 0.5, ..Texture::default() });
        p.tip.shape = TipShape::Image;
        p.tip.image = Some("not-a-hash".into());
        p.jitter.scatter = 99.0;
        let s = p.sanitized();
        s.validate().unwrap();
        assert!(!s.vector_eligible, "textured preset loses its vector claim");
        assert_eq!(s.id, "invalid");
        assert_eq!(s.tip.shape, TipShape::Round);
        assert_eq!(builtin_preset("marker").unwrap().sanitized(), builtin_preset("marker").unwrap());
    }

    #[test]
    fn tilt_only_when_reported() {
        let mut p = builtin_preset("round.hard").unwrap();
        p.dynamics.tilt.size = DynamicsTarget::range(ResponseCurve::identity(), 1.0, 2.0);
        assert_eq!(p.diameter(20.0, 1.0, None), 20.0);
        assert!((p.diameter(20.0, 1.0, Some(std::f32::consts::FRAC_PI_2)) - 40.0).abs() < 1e-3);
    }

    #[test]
    fn source_strings_round_trip() {
        for s in ["builtin", "custom", "imported:infinite_painter", "imported:x1"] {
            assert_eq!(BrushSource::parse(s).unwrap().as_string(), s);
        }
        for s in ["imported:", "imported:Bad", "imported:a-b", "other"] {
            assert!(BrushSource::parse(s).is_none(), "{s}");
        }
        let p: BrushPreset = serde_json::from_str(r#"{"source": "weird"}"#).unwrap();
        assert_eq!(p.source, BrushSource::Custom);
    }
}
