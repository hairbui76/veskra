//! Host CPU timings for docs/reports/performance-host.md (HOST CPU — NOT device evidence).
//!
//! `cargo test -p veskra-qa --release --test perf_host -- --ignored --nocapture`
//! Builds the INT-02 12-layer document at 2048 × 2048 through engine commands, then times
//! save / open / validate / full composite and every export format (median of N repetitions,
//! `VESKRA_PERF_REPS`, default 5). Writes `target/qa/perf/perf_host.json`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use std::time::Instant;
use veskra_qa::drive::Driver;
use veskra_qa::{int02, qa_root};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1e3 * 100.0).round() / 100.0
}

#[test]
#[ignore = "host performance measurement; run explicitly in --release"]
fn perf_host_2048_12_layer() {
    let reps: usize = std::env::var("VESKRA_PERF_REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(5).max(1);
    let dir = qa_root().join("perf");
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Driver::new(909);
    let t = Instant::now();
    int02::build(&mut d, 2048, 2048);
    let build_ms = ms(t);
    let info = d.info();
    let path = dir.join("int02-2048.veskra");
    let p = path.to_string_lossy().into_owned();

    let mut save = Vec::new();
    let mut bytes = Value::Null;
    for _ in 0..reps {
        let t = Instant::now();
        let r = d.ok(json!({"cmd": "document.save", "path": p}));
        save.push(ms(t));
        bytes = r["bytes"].clone();
    }
    let mut open = Vec::new();
    for _ in 0..reps {
        let mut d2 = Driver::new(1);
        let t = Instant::now();
        d2.ok(json!({"cmd": "document.open", "path": p}));
        open.push(ms(t));
    }
    let mut validate = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        d.ok(json!({"cmd": "document.validate", "path": p}));
        validate.push(ms(t));
    }
    let doc = d.doc();
    let mut comp = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        let c = veskra_qa::composite8(&doc);
        comp.push(ms(t));
        assert_eq!(c.len(), 2048 * 2048 * 4);
    }
    let mut exports = serde_json::Map::new();
    for fmt in ["png", "psd", "svg", "pdf"] {
        let out = dir.join(format!("int02-2048.{fmt}")).to_string_lossy().into_owned();
        let (mut wall, mut eng) = (Vec::new(), Vec::new());
        let mut size = Value::Null;
        for _ in 0..reps {
            let t = Instant::now();
            let r = d.ok(json!({"cmd": "export.run", "format": fmt, "path": out}));
            wall.push(ms(t));
            eng.push(r["durationMs"].as_f64().unwrap_or(f64::NAN));
            size = r["fileBytes"].clone();
        }
        exports.insert(
            fmt.into(),
            json!({"wallMsMedian": median(wall.clone()), "wallMs": wall,
                                          "reportDurationMsMedian": median(eng), "fileBytes": size}),
        );
    }
    let out = json!({
        "environment": "HOST CPU (x86_64 container) - NOT device evidence",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "document": {"width": 2048, "height": 2048, "layers": info["layerCount"], "strokes": info["strokeCount"],
                     "buildMsViaCommands": build_ms, "fileBytes": bytes},
        "reps": reps,
        "saveMs": {"median": median(save.clone()), "all": save},
        "openMs": {"median": median(open.clone()), "all": open},
        "validateMs": {"median": median(validate.clone()), "all": validate},
        "compositeFullMs": {"median": median(comp.clone()), "all": comp},
        "exports": exports,
    });
    std::fs::write(dir.join("perf_host.json"), serde_json::to_vec_pretty(&out).unwrap()).unwrap();
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
