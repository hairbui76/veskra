//! BRH-02 / PNT-003 / ADR-019: preset JSON round trip, tolerant loading, migration 1 → 2 → 3,
//! vector-eligibility rule.

use veskra_brush::{
    Accumulation, BUILTIN_IDS, BrushCategory, BrushError, BrushSource, LEGACY_BUILTIN_IDS, PRESET_SCHEMA_VERSION,
    TextureSource, TipShape, builtin_preset, builtin_presets, load_preset_json,
};
use veskra_core::{ErrorCode, VeskraError};

fn fixture(name: &str) -> String {
    let path = veskra_testkit::fixtures_dir().join("brushes").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn builtin_json_round_trip() {
    for p in builtin_presets() {
        for json in [p.to_json().unwrap(), p.to_json_compact().unwrap()] {
            let back = load_preset_json(&json).unwrap();
            assert_eq!(back.preset, p, "{}", p.id);
            assert!(back.ignored_keys.is_empty(), "{}: {:?}", p.id, back.ignored_keys);
            assert_eq!(back.migrated_from, None);
        }
    }
}

#[test]
fn pnt003_v1_fixture_migrates_preserving_id_and_defaults() {
    let load = load_preset_json(&fixture("pencil_smooth_v1.json")).unwrap();
    assert_eq!(load.migrated_from, Some(1));
    assert_eq!(load.ignored_keys, vec!["legacy_paper_texture".to_string()]);
    let p = load.preset;
    assert_eq!(p.id, "pencil.smooth");
    assert_eq!(p.version, 1);
    assert_eq!(p.schema, PRESET_SCHEMA_VERSION);
    assert_eq!(p.spacing, 0.08, "spacing_percent 8 → spacing 0.08");
    // Fields that did not exist in schema 1 take documented defaults.
    assert_eq!(p.stabilization, 0.0);
    assert!(!p.dynamics.tilt.size.enabled);
    // Everything else equals the current built-in apart from the new-field defaults.
    let mut builtin = builtin_preset("pencil.smooth").unwrap();
    builtin.stabilization = 0.0;
    assert_eq!(p, builtin);
}

#[test]
fn schema2_builtins_migrate_to_the_current_builtins_exactly() {
    for id in LEGACY_BUILTIN_IDS {
        let load = load_preset_json(&fixture(&format!("v2/{id}.json"))).unwrap();
        assert_eq!(load.migrated_from, Some(2), "{id}");
        assert!(load.ignored_keys.is_empty(), "{id}: {:?}", load.ignored_keys);
        assert_eq!(load.preset, builtin_preset(id).unwrap(), "{id}");
        assert_eq!(load.preset.source, BrushSource::Builtin);
    }
}

#[test]
fn schema_chain_1_2_3_round_trips() {
    // v1 → v3 → JSON → v3 is a fixed point; v2 features land in the v3 tables.
    let v1 = load_preset_json(&fixture("pencil_smooth_v1.json")).unwrap().preset;
    let again = load_preset_json(&v1.to_json().unwrap()).unwrap();
    assert_eq!(again.preset, v1);
    assert_eq!(again.migrated_from, None);
    let v2 = r#"{"schema": 2, "id": "user.tilt", "kind": "pen", "min_size_ratio": 0.25, "hardness": 0.6,
        "pressure_size": [[0,0],[0.5,0.2],[1,1]], "pressure_opacity": [[0,0.5],[1,1]],
        "tilt_size": {"max_scale": 2.5}, "grain": {"scale": 4, "strength": 0.3}, "stabilization": 0.2}"#;
    let l = load_preset_json(v2).unwrap();
    assert_eq!(l.migrated_from, Some(2));
    let p = l.preset;
    assert_eq!(p.category, BrushCategory::Ink);
    assert_eq!(p.source, BrushSource::Custom);
    assert_eq!(p.tip.hardness, 0.6);
    assert_eq!(p.tip.shape, TipShape::Round);
    let ps = &p.dynamics.pressure.size;
    assert!(ps.enabled && ps.min == 0.25 && ps.max == 1.0);
    assert_eq!(ps.curve.points(), &[[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]]);
    assert!(p.dynamics.pressure.opacity.enabled && p.dynamics.pressure.flow.enabled);
    let ts = &p.dynamics.tilt.size;
    assert!(ts.enabled && ts.min == 1.0 && ts.max == 2.5);
    let t = p.texture.clone().unwrap();
    assert_eq!((t.source, t.scale, t.depth), (TextureSource::Procedural, 4.0, 0.3));
    let back = load_preset_json(&p.to_json().unwrap()).unwrap();
    assert_eq!(back.preset, p);
}

#[test]
fn unknown_and_missing_fields_tolerated() {
    let json = r#"{
        "schema": 2, "id": "user.ink", "name": "My ink", "kind": "hologram",
        "size_px": 6, "grain": {"scale": 2.0, "strength": 0.5, "octaves": 4},
        "future_flag": true, "accumulation": "max"
    }"#;
    let load = load_preset_json(json).unwrap();
    assert_eq!(load.ignored_keys, vec!["future_flag".to_string(), "grain.octaves".to_string()]);
    let p = load.preset;
    assert_eq!(p.category, BrushCategory::Custom, "unknown kind loads as custom");
    assert_eq!(p.size_px, 6.0);
    assert_eq!(p.opacity, 1.0);
    assert_eq!(p.accumulation, Accumulation::Max);
    assert!(!p.vector_eligible);
    // Missing schema field = schema 1.
    let old = load_preset_json(r#"{"id": "a.b", "spacing_percent": 25}"#).unwrap();
    assert_eq!(old.migrated_from, Some(1));
    assert_eq!(old.preset.spacing, 0.25);
    // Schema 3: unknown category loads as custom, unknown nested keys are reported.
    let v3 = r#"{"schema": 3, "id": "x.y", "category": "laser", "tip": {"shape": "round", "sparkle": 1}}"#;
    let l = load_preset_json(v3).unwrap();
    assert_eq!(l.preset.category, BrushCategory::Custom);
    assert_eq!(l.ignored_keys, vec!["tip.sparkle".to_string()]);
}

#[test]
fn errors_map_to_veskra_codes() {
    let e: VeskraError = load_preset_json(r#"{"schema": 4, "id": "x"}"#).unwrap_err().into();
    assert_eq!(e.code, ErrorCode::UnsupportedSchema);
    let e: VeskraError = load_preset_json("{not json").unwrap_err().into();
    assert_eq!(e.code, ErrorCode::CorruptData);
    let e: VeskraError = load_preset_json(r#"{"id": "Bad Id"}"#).unwrap_err().into();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    let claim =
        r#"{"schema": 2, "id": "fake.vector", "grain": {"scale": 2, "strength": 0.5}, "vector_eligible": true}"#;
    assert!(matches!(load_preset_json(claim), Err(BrushError::InvalidPreset(_))));
    let claim3 = r#"{"schema": 3, "id": "fake.vector", "jitter": {"angle": 0.5}, "vector_eligible": true}"#;
    assert!(matches!(load_preset_json(claim3), Err(BrushError::InvalidPreset(_))));
    let bad_hash = r#"{"schema": 3, "id": "x", "tip": {"shape": "image", "image": "../etc/passwd"}}"#;
    assert!(matches!(load_preset_json(bad_hash), Err(BrushError::InvalidPreset(_))));
}

#[test]
fn vector_eligibility_matches_rule() {
    let expect_eligible =
        ["pencil.smooth", "pen.technical", "pen.brush", "marker", "round.hard", "ink.gpen", "ink.fineliner"];
    for p in builtin_presets() {
        let eligible = expect_eligible.contains(&p.id.as_str());
        assert_eq!(p.vector_eligible, eligible, "{}", p.id);
        assert_eq!(p.vector_eligible, p.satisfies_vector_rule(), "{}", p.id);
        if p.texture.is_some()
            || p.eraser
            || p.accumulation == Accumulation::BuildUp
            || p.tip.shape == TipShape::Image
            || !p.jitter.is_zero()
            || p.wet_edges > 0.0
        {
            assert!(!p.vector_eligible, "{}", p.id);
        }
    }
    assert_eq!(builtin_presets().len(), BUILTIN_IDS.len());
    let soft = builtin_preset("round.soft").unwrap();
    assert_eq!(soft.accumulation, Accumulation::BuildUp);
    let marker = builtin_preset("marker").unwrap();
    assert!(marker.opacity < 1.0 && marker.accumulation == Accumulation::Max);
    assert!(builtin_preset("pencil.rough").unwrap().texture.is_some());
    assert!(builtin_preset("charcoal").unwrap().texture.is_some());
    assert!(builtin_preset("eraser.hard").unwrap().eraser && builtin_preset("eraser.soft").unwrap().eraser);
}
