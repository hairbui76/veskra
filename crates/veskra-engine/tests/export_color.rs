#![allow(clippy::unwrap_used, clippy::expect_used)]
//! JSON contract of the export color options through the engine command API:
//! `export.preflight` / `export.run` with `options.profile` and `options.bitDepth`.

use serde_json::{Value, json};
use veskra_engine::{EngineConfig, Session};

fn ok(s: &mut Session, cmd: Value) -> Value {
    let out: Value = serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap();
    assert!(out.get("error").is_none(), "command {cmd} failed: {out}");
    out["ok"].clone()
}

fn err(s: &mut Session, cmd: Value) -> Value {
    let out: Value = serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap();
    assert!(out.get("error").is_some(), "command {cmd} unexpectedly succeeded: {out}");
    out["error"].clone()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-engine-export-color-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn painted() -> Session {
    let mut s = Session::new(EngineConfig { deterministic_seed: Some(3), ..Default::default() });
    ok(&mut s, json!({"cmd": "document.create", "width": 160, "height": 96, "background": "#FFFFFF"}));
    ok(&mut s, json!({"cmd": "color.set", "hex": "#FF0000"}));
    ok(&mut s, json!({"cmd": "edit.gradient", "x0": 0, "y0": 0, "x1": 160, "y1": 0}));
    ok(&mut s, json!({"cmd": "layer.add", "kind": "vector"}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": "pen.technical"}));
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    let samples: Vec<Value> =
        (0..20).map(|i| json!({"x": 10 + i * 7, "y": 20 + i * 3, "t": i as u64 * 4_000_000, "p": 0.8})).collect();
    ok(&mut s, json!({"cmd": "stroke.samples", "samples": samples}));
    ok(&mut s, json!({"cmd": "stroke.commit"}));
    s
}

#[test]
fn default_options_report_srgb_8_bit_unconverted() {
    let mut s = painted();
    for ext in ["png", "psd", "svg", "pdf"] {
        let pf = ok(&mut s, json!({"cmd": "export.preflight", "format": ext}));
        assert_eq!(pf["options"]["profile"], "document", "{ext}: {pf}");
        assert_eq!(pf["options"]["bitDepth"], 8);
        assert_eq!(pf["targetProfile"], "sRGB IEC61966-2.1");
        assert_eq!(pf["bitDepth"], 8);
        assert_eq!(pf["renderingIntent"], Value::Null);
        assert_eq!(pf["gamutClippingPossible"], false);
        let path = tmp(&format!("default.{ext}"));
        let rep = ok(&mut s, json!({"cmd": "export.run", "format": ext, "path": path.to_string_lossy()}));
        assert_eq!(rep["success"], true, "{ext}: {rep}");
        assert_eq!(rep["targetProfile"], "sRGB IEC61966-2.1");
        assert_eq!((rep["bitDepth"].as_u64(), rep["outOfGamutPixels"].as_u64()), (Some(8), Some(0)));
        assert_eq!(rep["renderingIntent"], Value::Null);
        // Old option JSON (the three checkboxes only) is the same export, byte for byte.
        let old = json!({"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": true});
        let path2 = tmp(&format!("old-options.{ext}"));
        ok(&mut s, json!({"cmd": "export.run", "format": ext, "path": path2.to_string_lossy(), "options": old}));
        assert_eq!(std::fs::read(&path).unwrap(), std::fs::read(&path2).unwrap(), "{ext}");
        // ... and so is an explicit sRGB / 8-bit request on this sRGB document.
        let explicit = json!({"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": true,
                              "profile": "srgb", "bitDepth": 8});
        let path3 = tmp(&format!("explicit.{ext}"));
        ok(&mut s, json!({"cmd": "export.run", "format": ext, "path": path3.to_string_lossy(), "options": explicit}));
        assert_eq!(std::fs::read(&path).unwrap(), std::fs::read(&path3).unwrap(), "{ext}");
    }
}

#[test]
fn wide_gamut_16_bit_export_through_json() {
    let mut s = painted();
    let options = |profile: &str, depth: u32| {
        json!({"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": true,
               "profile": profile, "bitDepth": depth})
    };
    for (profile, name) in [("display_p3", "Display P3"), ("adobe_rgb", "Veskra RGB (compatible with Adobe RGB 1998)")]
    {
        for ext in ["png", "psd", "svg", "pdf"] {
            let raster = ext == "png" || ext == "psd";
            let o = options(profile, 16);
            let pf = ok(&mut s, json!({"cmd": "export.preflight", "format": ext, "options": o}));
            assert_eq!(pf["blockedReason"], Value::Null, "{profile} {ext}: {pf}");
            assert_eq!(pf["targetProfile"], name);
            assert_eq!(pf["bitDepth"], if raster { 16 } else { 8 }, "{ext}");
            assert_eq!(pf["renderingIntent"], "relative-colorimetric");
            assert_eq!(pf["gamutClippingPossible"], false);
            let kinds = |v: &Value| -> Vec<String> {
                v["warnings"].as_array().unwrap().iter().map(|w| w["kind"].as_str().unwrap().to_string()).collect()
            };
            assert!(kinds(&pf).contains(&"color_converted".to_string()), "{ext}: {pf}");
            assert_eq!(kinds(&pf).contains(&"bit_depth_reduced".to_string()), !raster, "{ext}");
            assert_eq!(kinds(&pf).contains(&"vector_colors_srgb".to_string()), ext == "svg", "{ext}");
            let path = tmp(&format!("{profile}.{ext}"));
            let rep =
                ok(&mut s, json!({"cmd": "export.run", "format": ext, "path": path.to_string_lossy(), "options": o}));
            assert_eq!(rep["success"], true);
            assert_eq!(rep["validation"], "structurally_validated");
            assert_eq!(rep["targetProfile"], name);
            assert_eq!(rep["profileEmbedded"], true);
            assert_eq!(rep["bitDepth"], if raster { 16 } else { 8 });
            assert_eq!(rep["renderingIntent"], "relative-colorimetric");
            assert_eq!(rep["outOfGamutPixels"], 0);
            assert_eq!(kinds(&rep), kinds(&pf), "{ext}: the report repeats the preflight's color warnings");
            let bytes = std::fs::read(&path).unwrap();
            match ext {
                "png" => assert_eq!(bytes[24], 16, "IHDR bit depth"),
                "psd" => assert_eq!(&bytes[22..24], &[0, 16], "PSD header depth"),
                _ => {}
            }
        }
    }
}

#[test]
fn invalid_color_options_are_blocked_with_typed_errors() {
    let mut s = painted();
    let base = json!({"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": true});
    let with = |k: &str, v: Value| {
        let mut o = base.clone();
        o[k] = v;
        o
    };
    // Unknown profile / wrong types never reach the exporter.
    for o in [
        with("profile", json!("cmyk")),
        with("profile", json!(3)),
        with("bitDepth", json!("16")),
        with("bitDepth", json!(-8)),
    ] {
        let e = err(&mut s, json!({"cmd": "export.preflight", "format": "png", "options": o}));
        assert_eq!(e["code"], "InvalidArgument", "{o}");
    }
    // Unsupported depth: preflight explains, run refuses, no file appears.
    let o = with("bitDepth", json!(12));
    let pf = ok(&mut s, json!({"cmd": "export.preflight", "format": "png", "options": o}));
    assert!(pf["blockedReason"].as_str().unwrap().contains("bitDepth"));
    let path = tmp("depth12.png");
    let e = err(&mut s, json!({"cmd": "export.run", "format": "png", "path": path.to_string_lossy(), "options": o}));
    assert_eq!(e["code"], "InvalidArgument");
    assert!(!path.exists());
    // A wide-gamut target must be embedded.
    let mut o = with("profile", json!("display_p3"));
    o["embedColorProfile"] = json!(false);
    for ext in ["png", "psd", "svg", "pdf"] {
        let pf = ok(&mut s, json!({"cmd": "export.preflight", "format": ext, "options": o}));
        assert!(pf["blockedReason"].is_string(), "{ext}: {pf}");
        let path = tmp(&format!("untagged-p3.{ext}"));
        let e = err(&mut s, json!({"cmd": "export.run", "format": ext, "path": path.to_string_lossy(), "options": o}));
        assert_eq!(
            (e["code"].as_str(), e["key"].as_str()),
            (Some("IncompatibleOutputProfile"), Some("incompatible_output_profile"))
        );
        assert!(!path.exists());
    }
}
