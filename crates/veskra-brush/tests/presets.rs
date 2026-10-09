//! BRH-02 / PNT-003: preset JSON round trip, tolerant loading, migration, vector-eligibility rule.

use veskra_brush::{
    Accumulation, BUILTIN_IDS, BrushError, BrushKind, PRESET_SCHEMA_VERSION, builtin_preset, builtin_presets,
    load_preset_json,
};
use veskra_core::{ErrorCode, VeskraError};

fn fixture(name: &str) -> String {
    let path = veskra_testkit::fixtures_dir().join("brushes").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn builtin_json_round_trip() {
    for p in builtin_presets() {
        let json = p.to_json().unwrap();
        let back = load_preset_json(&json).unwrap();
        assert_eq!(back.preset, p, "{}", p.id);
        assert!(back.ignored_keys.is_empty(), "{}: {:?}", p.id, back.ignored_keys);
        assert_eq!(back.migrated_from, None);
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
    assert_eq!(p.tilt_size, None);
    // Everything else equals the current built-in apart from the new-field defaults.
    let mut builtin = builtin_preset("pencil.smooth").unwrap();
    builtin.stabilization = 0.0;
    assert_eq!(p, builtin);
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
    assert_eq!(p.kind, BrushKind::Other, "unknown kind loads as Other");
    assert_eq!(p.size_px, 6.0);
    assert_eq!(p.opacity, 1.0);
    assert_eq!(p.accumulation, Accumulation::Max);
    assert!(!p.vector_eligible);
    // Missing schema field = schema 1.
    let old = load_preset_json(r#"{"id": "a.b", "spacing_percent": 25}"#).unwrap();
    assert_eq!(old.migrated_from, Some(1));
    assert_eq!(old.preset.spacing, 0.25);
}

#[test]
fn errors_map_to_veskra_codes() {
    let e: VeskraError = load_preset_json(r#"{"schema": 3, "id": "x"}"#).unwrap_err().into();
    assert_eq!(e.code, ErrorCode::UnsupportedSchema);
    let e: VeskraError = load_preset_json("{not json").unwrap_err().into();
    assert_eq!(e.code, ErrorCode::CorruptData);
    let e: VeskraError = load_preset_json(r#"{"id": "Bad Id"}"#).unwrap_err().into();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    let claim = r#"{"id": "fake.vector", "grain": {"scale": 2, "strength": 0.5}, "vector_eligible": true}"#;
    assert!(matches!(load_preset_json(claim), Err(BrushError::InvalidPreset(_))));
}

#[test]
fn vector_eligibility_matches_rule() {
    let expect_eligible = ["pencil.smooth", "pen.technical", "pen.brush", "marker", "round.hard"];
    for p in builtin_presets() {
        let eligible = expect_eligible.contains(&p.id.as_str());
        assert_eq!(p.vector_eligible, eligible, "{}", p.id);
        assert_eq!(p.vector_eligible, p.satisfies_vector_rule(), "{}", p.id);
        if p.grain.is_some() || p.eraser || p.accumulation == Accumulation::BuildUp {
            assert!(!p.vector_eligible, "{}", p.id);
        }
    }
    assert_eq!(builtin_presets().len(), BUILTIN_IDS.len());
    let soft = builtin_preset("round.soft").unwrap();
    assert_eq!(soft.accumulation, Accumulation::BuildUp);
    let marker = builtin_preset("marker").unwrap();
    assert!(marker.opacity < 1.0 && marker.accumulation == Accumulation::Max);
    assert!(builtin_preset("pencil.rough").unwrap().grain.is_some());
    assert!(builtin_preset("charcoal").unwrap().grain.is_some());
    assert!(builtin_preset("eraser.hard").unwrap().eraser && builtin_preset("eraser.soft").unwrap().eraser);
}
