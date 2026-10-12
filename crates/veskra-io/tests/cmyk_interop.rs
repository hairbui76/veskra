//! CMYK interop fixtures (ADR-018): writes CMYK PSDs (8/16-bit) and PDFs plus the Veskra composite
//! to `<target>/interop/cmyk` and, when `python3` has psd-tools, pypdf and Pillow, runs
//! `scripts/interop/cmyk_check.py` (psd-tools reads mode/depth/ICC; LittleCMS round trip of the
//! pixels; pypdf output intent + DeviceCMYK images; poppler render when installed).
#![allow(clippy::unwrap_used)]

mod psd_fixtures;

use psd_fixtures as fx;
use serde_json::{Value, json};
use veskra_color::IccProfile;
use veskra_color::cmyk::BUNDLED_CMYK_PROFILE;
use veskra_core::export::{ExportFormat, ExportOptions, ExportProfile};
use veskra_core::{BlendMode, RectI};
use veskra_io::{CancelToken, exporter_for};

fn swatches() -> veskra_doc::Document {
    let w = 48i32;
    let mut d = fx::new_doc(w as u32, 48);
    let g = fx::grid(RectI::new(0, 0, w, 48), move |x, y| {
        [((x * 255) / (w - 1)) as u8, ((y * 255) / 47) as u8, (((x + y) * 255) / (w + 46)) as u8, 255]
    });
    fx::add_pixels(&mut d, 1, None, g, None, fx::props("Swatches", BlendMode::Normal, 1.0));
    d
}

#[test]
fn cmyk_files_for_independent_oracles() {
    let dir = fx::interop_dir("cmyk");
    let srgb = dir.join("veskra-srgb.icc");
    std::fs::write(&srgb, IccProfile::srgb().bytes()).unwrap();
    let profile_sha = {
        let p = fx::repo_root().join("crates/veskra-color/profiles/Coated_Fogra39L_VIGC_300.icc");
        assert_eq!(std::fs::read(&p).unwrap(), BUNDLED_CMYK_PROFILE, "embedded = committed file");
        // SHA-256 recorded in crates/veskra-color/profiles/README.md; the oracle script hashes
        // the bytes it finds in the PSD resource / PDF output intent and compares.
        "6df43849a84d2632e7900a125f657bd09364ed56775bc4740c1e39a5492d47e3"
    };
    let cmyk = ExportOptions { profile: ExportProfile::Cmyk, ..Default::default() };
    for (name, doc) in [("swatches", swatches()), ("six_layer", fx::six_layer())] {
        let mut psds = Vec::new();
        for (suffix, depth) in [("", 8u8), ("-16", 16)] {
            let o = ExportOptions { bit_depth: depth, ..cmyk.clone() };
            let mut bytes = Vec::new();
            exporter_for(ExportFormat::Psd).export(&doc, &o, &mut bytes, &CancelToken::new()).unwrap();
            let p = dir.join(format!("{name}{suffix}.psd"));
            std::fs::write(&p, bytes).unwrap();
            psds.push(p);
        }
        let mut pdf = Vec::new();
        exporter_for(ExportFormat::Pdf).export(&doc, &cmyk, &mut pdf, &CancelToken::new()).unwrap();
        let pdf_path = dir.join(format!("{name}.pdf"));
        std::fs::write(&pdf_path, pdf).unwrap();
        let raw = dir.join(format!("{name}.composite.rgba"));
        std::fs::write(&raw, fx::composite8(&doc)).unwrap();
        let exp = json!({
            "name": name, "width": doc.width(), "height": doc.height(), "composite_rgba": raw,
            "psd": psds, "pdf": pdf_path, "profile_sha256": profile_sha, "srgb_icc": srgb,
        });
        std::fs::write(dir.join(format!("{name}.expected.json")), serde_json::to_vec_pretty(&exp).unwrap()).unwrap();
    }
    if !fx::python_has("import psd_tools, pypdf, numpy, PIL") {
        eprintln!("cmyk interop: python3 with psd_tools/pypdf unavailable — oracle NOT RUN; files in {dir:?}");
        return;
    }
    let out = std::process::Command::new("python3")
        .arg(fx::repo_root().join("scripts/interop/cmyk_check.py"))
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{stdout}");
    let v: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("bad oracle output: {}", String::from_utf8_lossy(&out.stderr)));
    assert!(out.status.success(), "{}", v["errors"]);
}
