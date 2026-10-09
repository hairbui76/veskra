#![allow(clippy::unwrap_used, clippy::expect_used)]
//! End-to-end tests of the `veskra` binary on the repository fixtures
//! (`tests/fixtures/documents/*.veskra`, `tests/fixtures/input/*.jsonl`).

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(rel: &str) -> String {
    root().join("tests/fixtures").join(rel).to_string_lossy().into_owned()
}

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("veskra-cli-it-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().into_owned()
}

/// Run the binary; returns (success, stdout JSON or Null, stderr).
fn veskra(args: &[&str]) -> (bool, Value, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_veskra")).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(!stderr.contains("panicked"), "veskra {args:?} panicked: {stderr}");
    let v = serde_json::from_str(&stdout).unwrap_or(Value::Null);
    (out.status.success(), v, stderr)
}

fn ok(args: &[&str]) -> Value {
    let (success, v, err) = veskra(args);
    assert!(success, "veskra {args:?} failed: {err}");
    v
}

fn fails(args: &[&str]) -> String {
    let (success, _, err) = veskra(args);
    assert!(!success, "veskra {args:?} unexpectedly succeeded");
    assert_eq!(err.trim().lines().count(), 1, "one JSON error line: {err}");
    let e: Value = serde_json::from_str(err.trim()).unwrap();
    e["error"].as_str().unwrap().to_string()
}

#[test]
fn run_script() {
    let script = tmp("script.jsonl");
    std::fs::write(
        &script,
        "# comment\n{\"cmd\":\"document.create\",\"width\":64,\"height\":48,\"background\":\"#FFFFFF\"}\n\n\
         {\"cmd\":\"stroke.begin\"}\n{\"cmd\":\"stroke.samples\",\"samples\":[{\"x\":4,\"y\":4,\"t\":0,\"p\":0.5},{\"x\":60,\"y\":40,\"t\":8000000,\"p\":0.9}]}\n\
         {\"cmd\":\"stroke.commit\"}\n{\"cmd\":\"document.info\"}\n",
    )
    .unwrap();
    let v = ok(&["run", &script, "--seed", "3"]);
    let res = v["results"].as_array().unwrap();
    assert_eq!(res.len(), 5);
    assert_eq!(res[4]["ok"]["strokeCount"], 1);
    // Deterministic IDs for a given seed.
    assert_eq!(ok(&["run", &script, "--seed", "3"]), v);
    std::fs::write(&script, "{\"cmd\":\"document.info\"}\n").unwrap();
    assert!(fails(&["run", &script]).contains("line 1"));
    let kg = ok(&["run", &script, "--keep-going"]);
    assert!(kg["results"][0]["error"].is_object());
}

#[test]
fn replay_input_fixtures() {
    for (trace, min_strokes) in [
        ("stylus_confirmed.jsonl", 1),
        ("pen_pressure_curve.jsonl", 1),
        ("stylus_cancel_and_palm.jsonl", 0),
        ("two_finger_gesture.jsonl", 0),
    ] {
        let out = tmp(&format!("{trace}.veskra"));
        let v = ok(&["replay-input", &fixture(&format!("input/{trace}")), &out, "--size", "1600x1000"]);
        assert!(v["info"]["strokeCount"].as_u64().unwrap() >= min_strokes, "{trace}: {v}");
        assert_eq!(v["info"]["dirty"], false, "{trace} saved");
        ok(&["inspect-project", &out]);
    }
}

/// Defect: a trace that ends mid-stroke (app paused/killed) failed `replay-input` because the
/// final save was refused while the stroke was live. The lifecycle policy commits it.
#[test]
fn replay_input_trace_ending_mid_stroke() {
    let text = std::fs::read_to_string(fixture("input/pen_pressure_curve.jsonl")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let cut = tmp("cut.jsonl");
    std::fs::write(&cut, lines[..lines.len() / 2].join("\n")).unwrap();
    let out = tmp("cut.veskra");
    let v = ok(&["replay-input", &cut, &out, "--size", "1600x1000"]);
    assert_eq!(v["info"]["strokeCount"], 1, "{v}");
}

#[test]
fn render_export_inspect_and_compare() {
    let doc = fixture("documents/12-layer-illustration.veskra");
    let png = tmp("render.png");
    let r = ok(&["render", &doc, &png]);
    assert_eq!(r["success"], true);
    for ext in ["png", "psd", "svg", "pdf"] {
        let out = tmp(&format!("export.{ext}"));
        let v = ok(&["export", &doc, &out]);
        assert_eq!(v["report"]["success"], true, "{ext}: {v}");
        assert_eq!(v["report"]["validation"], "structurally_validated");
        let i = ok(&["inspect-export", &out]);
        assert_eq!(i["structurallyValid"], true);
    }
    let no_vec = tmp("raster-only.svg");
    ok(&["export", &doc, &no_vec, "--no-vector", "--no-icc"]);
    assert!(!fails(&["export", &doc, &tmp("both-off.svg"), "--no-vector", "--no-raster"]).is_empty());
    // ILL-008: never a native .ai (nor a PDF renamed .ai).
    let ai = tmp("out.ai");
    assert!(fails(&["export", &doc, &ai]).contains(".ai"));
    assert!(!Path::new(&ai).exists());
    assert!(fails(&["inspect-export", &ai]).contains(".ai"));
    // compare-color: identical images, then two different renders.
    let c = ok(&["compare-color", &png, &png]);
    assert_eq!(c["maxAbs"], 0);
    assert_eq!(c["pixelsOver1"], 0);
    let png2 = tmp("render-simple.png");
    ok(&["render", &fixture("documents/simple-1-layer.veskra"), &png2]);
    let c2 = ok(&["compare-color", &png2, &png2]);
    // ΔE00 is computed over opaque pixels only (null when there are none).
    if c2["opaquePixelsCompared"].as_u64().unwrap() > 0 {
        assert_eq!(c2["deltaE00"]["max"], 0.0);
    } else {
        assert!(c2["deltaE00"]["max"].is_null());
    }
    let (w1, h1, _) = veskra_testkit_dims(&png);
    let (w2, h2, _) = veskra_testkit_dims(&png2);
    if (w1, h1) == (w2, h2) {
        ok(&["compare-color", &png, &png2]);
    } else {
        assert!(fails(&["compare-color", &png, &png2]).contains("size mismatch"));
    }
}

fn veskra_testkit_dims(p: &str) -> (u32, u32, usize) {
    let b = std::fs::read(p).unwrap();
    // PNG IHDR: width/height big-endian at bytes 16..24.
    (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap()), b.len())
}

#[test]
fn inspect_project_good_and_bad() {
    let v = ok(&["inspect-project", &fixture("documents/simple-1-layer.veskra")]);
    assert!(!v["layers"]["layers"].as_array().unwrap().is_empty(), "{v}");
    for bad in ["corrupt-manifest.veskra", "truncated.veskra"] {
        fails(&["inspect-project", &fixture(&format!("documents/{bad}"))]);
    }
    // Defect: a readable bundle that cannot be opened (future schema) reported `layers: null`
    // with no reason. The open error is now part of the inspection.
    let v = ok(&["inspect-project", &fixture("documents/future-schema.veskra")]);
    assert!(v["layers"].is_null());
    assert!(v["openError"].as_str().unwrap().contains("schema"), "{v}");
}

/// Defect: `benchmark-stroke --samples 0` panicked (index out of bounds on empty latencies).
#[test]
fn benchmark_stroke_small_and_degenerate() {
    let v = ok(&["benchmark-stroke", "--size", "256", "--samples", "40"]);
    assert_eq!(v["environment"], "HOST CPU (not a device measurement)");
    assert!(v["batchLatencyMs"]["p95"].as_f64().unwrap() >= 0.0);
    ok(&["benchmark-stroke", "--size", "128", "--samples", "1", "--brush", "pencil.smooth"]);
    for mode in ["--input", "--predict"] {
        let v = ok(&["benchmark-stroke", "--size", "256", "--samples", "40", mode]);
        assert!(v["path"].as_str().unwrap().starts_with("input"), "{v}");
    }
    assert!(fails(&["benchmark-stroke", "--samples", "0"]).contains("samples"));
    fails(&["benchmark-stroke", "--size", "64", "--samples", "4", "--brush", "no.such.brush"]);
    fails(&["benchmark-stroke", "--size", "0", "--samples", "4"]);
}

#[test]
fn usage_errors_are_json_not_panics() {
    fails(&[]);
    fails(&["no-such-subcommand"]);
    fails(&["render"]);
    fails(&["render", &fixture("documents/simple-1-layer.veskra")]);
    fails(&["render", &fixture("documents/simple-1-layer.veskra"), &tmp("wrong-ext.jpg")]);
    fails(&["export", "/nonexistent/in.veskra", &tmp("x.png")]);
    fails(&["compare-color", "/nonexistent/a.png", "/nonexistent/b.png"]);
}
