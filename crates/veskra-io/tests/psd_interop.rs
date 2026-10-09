//! PSD interop against independent tools (FMT-11 automated part; PSD-001/003/004/005).
//!
//! Writes every fixture to `$CARGO_TARGET_DIR/interop/psd/` as `<name>.psd`, `<name>.composite.png`
//! (Veskra composite, encoded by the independent `png` crate), `<name>.composite.rgba` and
//! `<name>.expected.json`, then runs `scripts/interop/psd_check.py` (psd-tools + ImageMagick) when
//! python3 + psd_tools are installed. Krita is checked by `scripts/interop/psd_krita_check.sh`.
//!
//! Thresholds (8-bit, see the script for metric definitions):
//! * merged image (psd-tools and ImageMagick): alpha exact (0), opaque color exact (0), premultiplied
//!   color ≤ 2.0. The stored matted bytes are verified bit-exact in `psd_export.rs`; the residual here
//!   is the readers' un-matting `(m + a − 255)·255/a` re-quantized to 8 bits, then re-premultiplied.
//! * psd-tools' own recomposite of the layers: per-fixture limits in [`RECOMPOSITE_LIMITS`]
//!   (premultiplied max, alpha max, opaque color max), measured and recorded in
//!   `docs/reports/psd-interop.md`; discrepancies are reported, never hidden.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod psd_fixtures;

use psd_fixtures as fx;
use serde_json::{Value, json};
use veskra_core::LayerId;
use veskra_core::export::ExportOptions;
use veskra_doc::Document;
use veskra_io::psd::PsdExporter;
use veskra_io::{CancelToken, Exporter};

/// (fixture, max premultiplied |Δ|, max alpha |Δ|, max opaque color |Δ|) for psd-tools' recomposite.
/// Measured 2026-10-09 with psd-tools 1.24.0 (max premul 1.961, alpha 1, opaque color 1).
const RECOMPOSITE_LIMITS: &[(&str, f64, i64, i64)] = &[
    ("simple", 1.0, 0, 0),
    ("six_layer", 2.0, 1, 1),
    ("twelve_layer", 2.0, 1, 1),
    ("empty_and_inverse_mask", 2.0, 1, 1),
    ("large_sparse", 2.0, 1, 1),
];

fn has_base(doc: &Document, siblings: &[LayerId], pos: usize) -> bool {
    siblings[..pos].iter().any(|s| doc.layer(*s).is_some_and(|x| x.is_group() || !x.props.clip_to_below))
}

fn expected_tree(doc: &Document, ids: &[LayerId]) -> Vec<Value> {
    ids.iter()
        .enumerate()
        .map(|(i, id)| {
            let l = doc.layer(*id).unwrap();
            let g = l.group();
            let blend = match g {
                Some(g) if g.pass_through => "PASS_THROUGH".to_string(),
                _ => format!("{:?}", l.props.blend).to_uppercase(),
            };
            json!({
                "name": l.props.name,
                "group": g.is_some(),
                "visible": l.props.visible,
                "opacity": (l.props.opacity * 255.0).round() as i64,
                "blend": blend,
                "clipping": l.props.clip_to_below && g.is_none() && has_base(doc, ids, i),
                "children": g.map(|g| expected_tree(doc, &g.children)).unwrap_or_default(),
            })
        })
        .collect()
}

#[test]
fn psd_tools_and_imagemagick_oracle() {
    let dir = fx::interop_dir("psd");
    let mut runs = Vec::new();
    for (name, doc) in fx::all() {
        let mut bytes = Vec::new();
        PsdExporter.export(&doc, &ExportOptions::default(), &mut bytes, &CancelToken::new()).unwrap();
        PsdExporter.validate(&bytes).unwrap();
        let psd = dir.join(format!("{name}.psd"));
        std::fs::write(&psd, &bytes).unwrap();
        let comp = fx::composite8(&doc);
        let raw = dir.join(format!("{name}.composite.rgba"));
        std::fs::write(&raw, &comp).unwrap();
        std::fs::write(
            dir.join(format!("{name}.composite.png")),
            veskra_testkit::encode_png_rgba8(doc.width(), doc.height(), &comp),
        )
        .unwrap();
        let exp = json!({
            "name": name, "width": doc.width(), "height": doc.height(),
            "composite_rgba": raw, "icc_hex": fx::hex(&doc.color.icc_bytes),
            "layers": expected_tree(&doc, doc.root()),
        });
        let jp = dir.join(format!("{name}.expected.json"));
        std::fs::write(&jp, serde_json::to_vec_pretty(&exp).unwrap()).unwrap();
        runs.push((name, psd, jp, bytes.len()));
    }
    if !fx::python_has("import psd_tools, numpy, PIL") {
        eprintln!("psd interop: psd_tools unavailable — oracle NOT RUN; files in {dir:?}");
        return;
    }
    let script = fx::repo_root().join("scripts/interop/psd_check.py");
    let mut all = Vec::new();
    let mut failures = Vec::new();
    for (name, psd, jp, size) in runs {
        let out = std::process::Command::new("python3").arg(&script).arg(&psd).arg(&jp).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr);
        eprintln!("{name} ({size} bytes): {stdout}");
        let v: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("{name}: bad output\n{stderr}"));
        if !out.status.success() {
            failures.push(format!("{name}: {}", v["errors"]));
        }
        for key in ["merged_psd_tools", "merged_imagemagick"] {
            if let Some(m) = v.get(key).filter(|m| !m.is_null()) {
                if m["alpha_max"].as_i64().unwrap() != 0
                    || m["opaque_color_max"].as_i64().unwrap() != 0
                    || m["premul_max"].as_f64().unwrap() > 2.0
                {
                    failures.push(format!("{name}: {key} exceeds merged threshold: {m}"));
                }
            } else if key == "merged_psd_tools" {
                failures.push(format!("{name}: no {key} result"));
            }
        }
        let (_, pmax, amax, cmax) = RECOMPOSITE_LIMITS.iter().find(|l| l.0 == name).copied().unwrap();
        let r = &v["recomposite_psd_tools"];
        if r.is_null()
            || r["premul_max"].as_f64().unwrap() > pmax
            || r["alpha_max"].as_i64().unwrap() > amax
            || r["opaque_color_max"].as_i64().unwrap() > cmax
        {
            failures.push(format!("{name}: psd-tools recomposite exceeds limit ({pmax}, {amax}, {cmax}): {r}"));
        }
        all.push(v);
    }
    std::fs::write(dir.join("psd_check.results.json"), serde_json::to_vec_pretty(&all).unwrap()).unwrap();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Single-feature PSDs used to localize Krita discrepancies (`psd_krita_check.sh <dir>/diag`).
fn diag_docs() -> Vec<(&'static str, Document)> {
    use veskra_core::{BlendMode, RectI};
    let bg = |d: &mut Document| {
        fx::add_pixels(
            d,
            1,
            None,
            fx::grid(RectI::new(0, 0, 64, 64), |x, y| [200, (x * 3) as u8, (y * 3) as u8, 255]),
            None,
            fx::props("Background", BlendMode::Normal, 1.0),
        );
    };
    let disc = |x: i32, y: i32| [40, 160, 90, fx::a8(fx::disc(x, y, 32.0, 32.0, 16.0), 255)];
    let tone = |_: i32, _: i32| [120, 80, 200, 200];
    let mut out = Vec::new();
    // Clipping at root level.
    let mut d = fx::new_doc(64, 64);
    bg(&mut d);
    fx::add_pixels(
        &mut d,
        2,
        None,
        fx::grid(RectI::new(0, 0, 64, 64), disc),
        None,
        fx::props("Base", BlendMode::Normal, 1.0),
    );
    let mut c = fx::props("Clip", BlendMode::Normal, 1.0);
    c.clip_to_below = true;
    fx::add_pixels(&mut d, 3, None, fx::grid(RectI::new(8, 8, 56, 56), tone), None, c.clone());
    out.push(("diag_clip_root", d));
    // Clipping inside an isolated group.
    let mut d = fx::new_doc(64, 64);
    bg(&mut d);
    fx::add_group(&mut d, 10, None, false, fx::props("Group", BlendMode::Normal, 1.0));
    fx::add_pixels(
        &mut d,
        2,
        Some(10),
        fx::grid(RectI::new(0, 0, 64, 64), disc),
        None,
        fx::props("Base", BlendMode::Normal, 1.0),
    );
    fx::add_pixels(&mut d, 3, Some(10), fx::grid(RectI::new(8, 8, 56, 56), tone), None, c.clone());
    out.push(("diag_clip_in_group", d));
    // Clipping with Multiply at root.
    let mut d = fx::new_doc(64, 64);
    bg(&mut d);
    fx::add_pixels(
        &mut d,
        2,
        None,
        fx::grid(RectI::new(0, 0, 64, 64), disc),
        None,
        fx::props("Base", BlendMode::Normal, 1.0),
    );
    let mut cm = c.clone();
    cm.blend = BlendMode::Multiply;
    fx::add_pixels(&mut d, 3, None, fx::grid(RectI::new(8, 8, 56, 56), tone), None, cm);
    out.push(("diag_clip_multiply", d));
    // Isolated Screen group with opacity.
    for (name, mode, op) in [
        ("diag_group_screen_085", BlendMode::Screen, 0.85),
        ("diag_group_screen_100", BlendMode::Screen, 1.0),
        ("diag_group_normal_085", BlendMode::Normal, 0.85),
    ] {
        let mut d = fx::new_doc(64, 64);
        bg(&mut d);
        fx::add_group(&mut d, 10, None, false, fx::props("Group", mode, op));
        fx::add_pixels(
            &mut d,
            2,
            Some(10),
            fx::grid(RectI::new(0, 0, 64, 64), |x, y| [255, 230, 160, fx::a8(fx::disc(x, y, 32.0, 32.0, 24.0), 200)]),
            None,
            fx::props("Light", BlendMode::Normal, 1.0),
        );
        out.push((name, d));
    }
    // Plain layer blend modes.
    for (name, mode) in [
        ("diag_layer_screen", BlendMode::Screen),
        ("diag_layer_overlay", BlendMode::Overlay),
        ("diag_layer_multiply", BlendMode::Multiply),
    ] {
        let mut d = fx::new_doc(64, 64);
        bg(&mut d);
        fx::add_pixels(
            &mut d,
            2,
            None,
            fx::grid(RectI::new(0, 0, 64, 64), |x, _| [(x * 4) as u8, 64, 160, 96]),
            None,
            fx::props("Blend", mode, 1.0),
        );
        out.push((name, d));
    }
    // Pass-through group with a normal child.
    let mut d = fx::new_doc(64, 64);
    bg(&mut d);
    fx::add_group(&mut d, 10, None, true, fx::props("Pass", BlendMode::Normal, 1.0));
    fx::add_pixels(
        &mut d,
        2,
        Some(10),
        fx::grid(RectI::new(0, 0, 64, 64), disc),
        None,
        fx::props("Child", BlendMode::Normal, 1.0),
    );
    out.push(("diag_pass_through", d));
    out
}

#[test]
fn write_krita_diagnostic_psds() {
    let dir = fx::interop_dir("psd/diag");
    for (name, doc) in diag_docs() {
        let mut bytes = Vec::new();
        PsdExporter.export(&doc, &ExportOptions::default(), &mut bytes, &CancelToken::new()).unwrap();
        PsdExporter.validate(&bytes).unwrap();
        std::fs::write(dir.join(format!("{name}.psd")), &bytes).unwrap();
        let comp = fx::composite8(&doc);
        let raw = dir.join(format!("{name}.composite.rgba"));
        std::fs::write(&raw, &comp).unwrap();
        let exp = json!({
            "name": name, "width": doc.width(), "height": doc.height(),
            "composite_rgba": raw, "icc_hex": fx::hex(&doc.color.icc_bytes),
            "layers": expected_tree(&doc, doc.root()),
        });
        std::fs::write(dir.join(format!("{name}.expected.json")), serde_json::to_vec_pretty(&exp).unwrap()).unwrap();
    }
}
