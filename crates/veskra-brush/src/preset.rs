//! Brush presets (BRH-01/02): stable ids, versioned JSON schema, migration and built-ins.
//!
//! ## Schema history
//! * **schema 1** (BRH-01 draft layout): spacing was stored as `spacing_percent` (percent of the
//!   dab diameter); `stabilization` and `tilt_size` did not exist.
//! * **schema 2** (current, [`PRESET_SCHEMA_VERSION`]): `spacing` is a fraction of the diameter;
//!   adds `stabilization` and `tilt_size`.
//!
//! Loading ([`load_preset_json`]) is tolerant: unknown keys are ignored *and reported*
//! ([`PresetLoad::ignored_keys`]), missing fields take documented defaults, older schemas are
//! migrated step by step, newer schemas are rejected with a typed error. The preset `id` is
//! never changed by migration (PNT-003).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{BrushError, ResponseCurve};

/// Current preset JSON schema.
pub const PRESET_SCHEMA_VERSION: u32 = 2;
/// Maximum preset JSON size accepted by [`load_preset_json`].
pub const MAX_PRESET_JSON_BYTES: usize = 64 * 1024;
/// Largest brush diameter in document pixels.
pub const MAX_BRUSH_SIZE_PX: f32 = 1000.0;
/// Rope length (document px) of the stabilizer at `stabilization = 1.0`.
pub const MAX_STABILIZER_ROPE_PX: f32 = 40.0;

/// Stable ids of the built-in presets, in display order.
pub const BUILTIN_IDS: [&str; 10] = [
    "pencil.rough",
    "pencil.smooth",
    "pen.technical",
    "pen.brush",
    "marker",
    "round.hard",
    "round.soft",
    "charcoal",
    "eraser.hard",
    "eraser.soft",
];

/// Broad brush family (UI grouping; behaviour is fully described by the other fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrushKind {
    Pencil,
    Pen,
    Marker,
    Round,
    Airbrush,
    Charcoal,
    Eraser,
    /// Kind written by a newer build; the preset still loads.
    #[serde(other)]
    Other,
}

/// How dabs combine inside one stroke's coverage buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accumulation {
    /// `m = max(m, dab)`: overlapping dabs of one stroke never darken (ink, pencil, marker).
    Max,
    /// `m = m + dab·(1 − m)`: coverage builds up where dabs overlap (airbrush).
    BuildUp,
}

/// Seeded procedural value-noise grain, evaluated in **document** space so the texture does not
/// swim with dab positions; it multiplies dab alpha.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Grain {
    /// Feature size in document px (`0.5..=256`).
    pub scale: f32,
    /// `0..=1`; dab alpha is multiplied by `1 − strength · noise'`.
    pub strength: f32,
}

/// Optional tilt → size response: diameter is multiplied by `1 + (max_scale − 1) · tilt/(π/2)`.
/// Applied only when the sample carries hardware tilt; absent tilt is never invented.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TiltSize {
    /// Diameter multiplier at full tilt (`0.1..=4`).
    pub max_scale: f32,
}

/// A brush preset (ADR-004). See the module docs for the schema and load rules.
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
    pub kind: BrushKind,
    /// Dab diameter at full pressure, document px.
    pub size_px: f32,
    /// Diameter at zero pressure as a fraction of `size_px` (`1.0` = constant width).
    pub min_size_ratio: f32,
    /// Stroke-level opacity (applied once to the accumulated stroke coverage).
    pub opacity: f32,
    /// Per-dab alpha.
    pub flow: f32,
    /// Dab spacing as a fraction of the current diameter (`0.01..=4`).
    pub spacing: f32,
    /// `1` = hard edge (1-px antialiased), `0` = smooth falloff over the whole radius.
    pub hardness: f32,
    /// Pressure → size factor between `min_size_ratio` and 1.
    pub pressure_size: ResponseCurve,
    /// Pressure → per-dab alpha multiplier.
    pub pressure_opacity: ResponseCurve,
    /// Pressure → per-dab flow multiplier.
    pub pressure_flow: ResponseCurve,
    pub tilt_size: Option<TiltSize>,
    pub grain: Option<Grain>,
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
            kind: BrushKind::Round,
            size_px: 10.0,
            min_size_ratio: 0.0,
            opacity: 1.0,
            flow: 1.0,
            spacing: 0.1,
            hardness: 1.0,
            pressure_size: ResponseCurve::identity(),
            pressure_opacity: ResponseCurve::identity(),
            pressure_flow: ResponseCurve::identity(),
            tilt_size: None,
            grain: None,
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

impl BrushPreset {
    /// The honesty rule for `vector_eligible` (ADR-004, MASTER_PLAN §5.2): a solid, untextured,
    /// round paint brush whose per-dab alpha is constant 1 under `Max` accumulation, so its
    /// coverage is exactly a union of discs that a filled outline reproduces.
    pub fn satisfies_vector_rule(&self) -> bool {
        !self.eraser
            && !matches!(self.kind, BrushKind::Eraser | BrushKind::Airbrush | BrushKind::Charcoal | BrushKind::Other)
            && self.grain.is_none()
            && self.accumulation == Accumulation::Max
            && self.flow == 1.0
            && self.hardness >= 0.5
            && self.pressure_opacity.is_constant_value(1.0)
            && self.pressure_flow.is_constant_value(1.0)
    }

    /// Diameter in document px for `pressure` (and optional hardware tilt) with base size `size`.
    pub fn diameter(&self, size: f32, pressure: f32, tilt: Option<f32>) -> f32 {
        let k = self.min_size_ratio + (1.0 - self.min_size_ratio) * self.pressure_size.eval(pressure);
        let t = match (self.tilt_size, tilt) {
            (Some(ts), Some(t)) if t.is_finite() => {
                1.0 + (ts.max_scale - 1.0) * (t / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0)
            }
            _ => 1.0,
        };
        size * k * t
    }

    /// Per-dab alpha (`flow × pressure curves`) for `pressure`.
    pub fn dab_alpha(&self, pressure: f32) -> f32 {
        (self.flow * self.pressure_flow.eval(pressure) * self.pressure_opacity.eval(pressure)).clamp(0.0, 1.0)
    }

    pub fn validate(&self) -> Result<(), BrushError> {
        let bad = |m: &str| Err(BrushError::InvalidPreset(m.to_string()));
        let in01 = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        if self.schema != PRESET_SCHEMA_VERSION {
            return bad("schema must be current after migration");
        }
        if !id_is_valid(&self.id) {
            return bad("id must be 1..=64 chars of [a-z0-9._-]");
        }
        if self.name.len() > veskra_core::limits::MAX_NAME_BYTES {
            return bad("name too long");
        }
        if !(self.size_px.is_finite() && self.size_px > 0.0 && self.size_px <= MAX_BRUSH_SIZE_PX) {
            return bad("size_px must be in (0, 1000]");
        }
        if !in01(self.min_size_ratio) || !in01(self.opacity) || !in01(self.hardness) || !in01(self.stabilization) {
            return bad("min_size_ratio, opacity, hardness and stabilization must be in [0,1]");
        }
        if !(in01(self.flow) && self.flow > 0.0) {
            return bad("flow must be in (0,1]");
        }
        if !(self.spacing.is_finite() && (0.01..=4.0).contains(&self.spacing)) {
            return bad("spacing must be in [0.01, 4]");
        }
        self.pressure_size.validate()?;
        self.pressure_opacity.validate()?;
        self.pressure_flow.validate()?;
        if let Some(g) = self.grain
            && !(g.scale.is_finite() && (0.5..=256.0).contains(&g.scale) && in01(g.strength))
        {
            return bad("grain scale must be in [0.5, 256] and strength in [0,1]");
        }
        if let Some(t) = self.tilt_size
            && !(t.max_scale.is_finite() && (0.1..=4.0).contains(&t.max_scale))
        {
            return bad("tilt_size.max_scale must be in [0.1, 4]");
        }
        if self.vector_eligible && !self.satisfies_vector_rule() {
            return bad("vector_eligible requires a solid untextured Max-accumulation round paint brush");
        }
        Ok(())
    }

    /// A copy that always passes [`BrushPreset::validate`]: numeric fields clamped into range
    /// (non-finite → default), invalid curves → identity, invalid grain/tilt dropped, invalid id
    /// replaced by `"invalid"`, and `vector_eligible` cleared unless the rule holds.
    pub fn sanitized(&self) -> BrushPreset {
        if self.validate().is_ok() {
            return self.clone();
        }
        let d = BrushPreset::default();
        let fix = |v: f32, lo: f32, hi: f32, dflt: f32| if v.is_finite() { v.clamp(lo, hi) } else { dflt };
        let curve = |c: &ResponseCurve| if c.validate().is_ok() { c.clone() } else { ResponseCurve::identity() };
        let mut p = BrushPreset {
            schema: PRESET_SCHEMA_VERSION,
            size_px: fix(self.size_px, 0.01, MAX_BRUSH_SIZE_PX, d.size_px),
            min_size_ratio: fix(self.min_size_ratio, 0.0, 1.0, d.min_size_ratio),
            opacity: fix(self.opacity, 0.0, 1.0, d.opacity),
            flow: fix(self.flow, 0.001, 1.0, d.flow),
            spacing: fix(self.spacing, 0.01, 4.0, d.spacing),
            hardness: fix(self.hardness, 0.0, 1.0, d.hardness),
            stabilization: fix(self.stabilization, 0.0, 1.0, 0.0),
            pressure_size: curve(&self.pressure_size),
            pressure_opacity: curve(&self.pressure_opacity),
            pressure_flow: curve(&self.pressure_flow),
            grain: self
                .grain
                .filter(|g| g.scale.is_finite() && (0.5..=256.0).contains(&g.scale))
                .map(|g| Grain { scale: g.scale, strength: fix(g.strength, 0.0, 1.0, 0.0) }),
            tilt_size: self.tilt_size.filter(|t| t.max_scale.is_finite() && (0.1..=4.0).contains(&t.max_scale)),
            ..self.clone()
        };
        if p.name.len() > veskra_core::limits::MAX_NAME_BYTES {
            p.name = String::new();
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
}

/// Stable id rule: 1..=64 bytes of `[a-z0-9._-]`.
fn id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// Parse, migrate and validate preset JSON (BRH-02).
pub fn load_preset_json(json: &str) -> Result<PresetLoad, BrushError> {
    if json.len() > MAX_PRESET_JSON_BYTES {
        return Err(BrushError::TooLarge { limit: MAX_PRESET_JSON_BYTES });
    }
    let mut v: Value = serde_json::from_str(json).map_err(|e| BrushError::Parse(e.to_string()))?;
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
    let mut cur = schema;
    while cur < PRESET_SCHEMA_VERSION {
        cur = migrate_step(obj, cur)?;
    }
    obj.insert("schema".into(), Value::from(PRESET_SCHEMA_VERSION));
    let preset: BrushPreset = serde_json::from_value(v.clone()).map_err(|e| BrushError::Parse(e.to_string()))?;
    preset.validate()?;
    let canonical = serde_json::to_value(&preset).map_err(|e| BrushError::Parse(e.to_string()))?;
    let mut ignored_keys = Vec::new();
    diff_keys(&v, &canonical, "", &mut ignored_keys);
    ignored_keys.sort();
    Ok(PresetLoad { preset, ignored_keys, migrated_from: (schema < PRESET_SCHEMA_VERSION).then_some(schema) })
}

/// Migration hook: transform an object of schema `from` to schema `from + 1`.
fn migrate_step(obj: &mut Map<String, Value>, from: u32) -> Result<u32, BrushError> {
    match from {
        1 => {
            // spacing_percent (percent of diameter) → spacing (fraction). New fields default.
            if let Some(p) = obj.remove("spacing_percent") {
                let p = p.as_f64().ok_or_else(|| BrushError::Parse("spacing_percent must be a number".into()))?;
                obj.insert("spacing".into(), Value::from(p / 100.0));
            }
            Ok(2)
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

/// The built-in presets (stable ids [`BUILTIN_IDS`]). No third-party brush assets are used;
/// all textures are procedural.
pub fn builtin_presets() -> Vec<BrushPreset> {
    let base = |id: &str, name: &str, kind: BrushKind| BrushPreset {
        id: id.into(),
        name: name.into(),
        kind,
        pressure_opacity: ResponseCurve::constant(1.0),
        pressure_flow: ResponseCurve::constant(1.0),
        ..BrushPreset::default()
    };
    vec![
        BrushPreset {
            size_px: 5.0,
            min_size_ratio: 0.45,
            flow: 0.9,
            spacing: 0.1,
            hardness: 0.7,
            pressure_opacity: ResponseCurve::new(vec![[0.0, 0.15], [0.5, 0.65], [1.0, 1.0]]).unwrap_or_default(),
            grain: Some(Grain { scale: 1.6, strength: 0.6 }),
            stabilization: 0.0,
            ..base("pencil.rough", "Rough pencil", BrushKind::Pencil)
        },
        BrushPreset {
            size_px: 4.0,
            min_size_ratio: 0.35,
            opacity: 0.9,
            spacing: 0.08,
            hardness: 0.85,
            stabilization: 0.05,
            vector_eligible: true,
            ..base("pencil.smooth", "Smooth pencil", BrushKind::Pencil)
        },
        BrushPreset {
            size_px: 3.0,
            min_size_ratio: 0.85,
            spacing: 0.06,
            hardness: 1.0,
            stabilization: 0.15,
            vector_eligible: true,
            ..base("pen.technical", "Technical pen", BrushKind::Pen)
        },
        BrushPreset {
            size_px: 12.0,
            min_size_ratio: 0.06,
            spacing: 0.05,
            hardness: 0.95,
            pressure_size: ResponseCurve::power(1.6, 9),
            stabilization: 0.1,
            vector_eligible: true,
            ..base("pen.brush", "Brush pen", BrushKind::Pen)
        },
        BrushPreset {
            size_px: 16.0,
            min_size_ratio: 0.8,
            opacity: 0.7,
            spacing: 0.06,
            hardness: 0.9,
            vector_eligible: true,
            ..base("marker", "Marker", BrushKind::Marker)
        },
        BrushPreset {
            size_px: 20.0,
            min_size_ratio: 0.5,
            spacing: 0.08,
            hardness: 1.0,
            vector_eligible: true,
            ..base("round.hard", "Hard round", BrushKind::Round)
        },
        BrushPreset {
            size_px: 40.0,
            min_size_ratio: 1.0,
            flow: 0.08,
            spacing: 0.1,
            hardness: 0.0,
            pressure_flow: ResponseCurve::identity(),
            accumulation: Accumulation::BuildUp,
            ..base("round.soft", "Soft round (airbrush)", BrushKind::Airbrush)
        },
        BrushPreset {
            size_px: 14.0,
            min_size_ratio: 0.5,
            flow: 0.85,
            spacing: 0.08,
            hardness: 0.6,
            pressure_opacity: ResponseCurve::identity(),
            grain: Some(Grain { scale: 3.0, strength: 0.75 }),
            ..base("charcoal", "Charcoal", BrushKind::Charcoal)
        },
        BrushPreset {
            size_px: 20.0,
            min_size_ratio: 0.6,
            spacing: 0.08,
            hardness: 1.0,
            eraser: true,
            ..base("eraser.hard", "Hard eraser", BrushKind::Eraser)
        },
        BrushPreset {
            size_px: 40.0,
            min_size_ratio: 1.0,
            flow: 0.15,
            spacing: 0.1,
            hardness: 0.0,
            pressure_flow: ResponseCurve::identity(),
            accumulation: Accumulation::BuildUp,
            eraser: true,
            ..base("eraser.soft", "Soft eraser", BrushKind::Eraser)
        },
    ]
}

/// Built-in preset by stable id.
pub fn builtin_preset(id: &str) -> Option<BrushPreset> {
    builtin_presets().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_valid_and_ids_stable() {
        let all = builtin_presets();
        let ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, BUILTIN_IDS);
        for p in &all {
            p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
        }
    }

    #[test]
    fn dishonest_vector_claim_rejected() {
        let mut p = builtin_preset("charcoal").unwrap();
        p.vector_eligible = true;
        assert!(matches!(p.validate(), Err(BrushError::InvalidPreset(_))));
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
        p.hardness = 7.0;
        p.id = "Bad Id".into();
        p.pressure_size = ResponseCurve::constant(1.0);
        p.grain = Some(Grain { scale: 1.0, strength: 0.5 });
        let s = p.sanitized();
        s.validate().unwrap();
        assert!(!s.vector_eligible, "textured preset loses its vector claim");
        assert_eq!(s.id, "invalid");
        assert_eq!(builtin_preset("marker").unwrap().sanitized(), builtin_preset("marker").unwrap());
    }

    #[test]
    fn tilt_only_when_reported() {
        let mut p = builtin_preset("round.hard").unwrap();
        p.tilt_size = Some(TiltSize { max_scale: 2.0 });
        assert_eq!(p.diameter(20.0, 1.0, None), 20.0);
        assert!((p.diameter(20.0, 1.0, Some(std::f32::consts::FRAC_PI_2)) - 40.0).abs() < 1e-4);
    }
}
