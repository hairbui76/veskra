#![allow(clippy::unwrap_used, clippy::expect_used)]
//! JSON contract of the CMYK color-picker commands (ADR-018): `color.convert` and
//! `profile.inspect` — results, typed errors, no undo step, per-session caching and latency.

use serde_json::{Value, json};
use std::time::Instant;
use veskra_color::cmyk::{BUNDLED_CMYK_PROFILE, CmykProfile, CmykTransform};
use veskra_color::{IccProfile, RenderingIntent};
use veskra_engine::{EngineConfig, Session};

fn run(s: &mut Session, cmd: Value) -> Value {
    serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap()
}

fn ok(s: &mut Session, cmd: Value) -> Value {
    let out = run(s, cmd.clone());
    assert!(out.get("error").is_none(), "command {cmd} failed: {out}");
    assert_eq!(out["historyChanged"], false);
    out["ok"].clone()
}

fn err(s: &mut Session, cmd: Value) -> Value {
    let out = run(s, cmd.clone());
    assert!(out.get("error").is_some(), "command {cmd} unexpectedly succeeded: {out}");
    out["error"].clone()
}

fn session() -> Session {
    Session::new(EngineConfig { deterministic_seed: Some(1), ..Default::default() })
}

fn to_cmyk(hex: &str) -> Value {
    json!({"cmd": "color.convert", "hex": hex, "to": "cmyk", "cmykProfile": null,
           "intent": "relative_colorimetric", "blackPointCompensation": true})
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-engine-cmyk-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn color_convert_rgb_to_cmyk_and_back() {
    let mut s = session(); // no document needed
    let white = ok(&mut s, to_cmyk("#FFFFFF"));
    assert_eq!(white["cmyk"], json!([0.0, 0.0, 0.0, 0.0]));
    assert_eq!(white["inGamut"], true);
    assert_eq!(white["profile"], "Coated_Fogra39L_VIGC_300.icc");
    // Black is reproducible with BPC (mapped to the darkest CMYK) but not without.
    let black = ok(&mut s, to_cmyk("#000000"));
    assert_eq!(black["inGamut"], true);
    assert!(black["cmyk"][3].as_f64().unwrap() > 0.95);
    let mut nobpc = to_cmyk("#000000");
    nobpc["blackPointCompensation"] = json!(false);
    assert_eq!(ok(&mut s, nobpc)["inGamut"], false);
    // Saturated sRGB green is outside FOGRA39.
    assert_eq!(ok(&mut s, to_cmyk("#00FF00"))["inGamut"], false);
    // Same numbers as the color engine's direct pipeline (4 decimals).
    let t = CmykTransform::new(&IccProfile::srgb(), CmykProfile::bundled().unwrap(), RenderingIntent::Perceptual, true)
        .unwrap();
    let mut perc = to_cmyk("#3366CC");
    perc["intent"] = json!("perceptual");
    let got = ok(&mut s, perc);
    let want = t.rgb_to_cmyk([0x33 as f64 / 255.0, 0x66 as f64 / 255.0, 0xCC as f64 / 255.0]);
    for k in 0..4 {
        assert!((got["cmyk"][k].as_f64().unwrap() - want[k]).abs() <= 0.5e-4, "{got}");
    }
    // Defaults: intent relative colorimetric, BPC on, bundled profile.
    let minimal = ok(&mut s, json!({"cmd": "color.convert", "hex": "#808080", "to": "cmyk"}));
    assert_eq!(minimal, ok(&mut s, to_cmyk("#808080")));
    // Round trip through CMYK lands close to the start.
    let back = ok(&mut s, json!({"cmd": "color.convert", "cmyk": minimal["cmyk"], "to": "srgb"}));
    let hex = back["hex"].as_str().unwrap();
    let g = u8::from_str_radix(&hex[3..5], 16).unwrap() as i32;
    assert!((g - 0x80).abs() <= 2, "{back}");
    assert_eq!(back["clipped"], false);
    assert_eq!(back["profile"], "Coated_Fogra39L_VIGC_300.icc");
    // Paper white → #FFFFFF; 100 % cyan is outside sRGB and reported as clipped.
    let paper = ok(&mut s, json!({"cmd": "color.convert", "cmyk": [0, 0, 0, 0], "to": "srgb"}));
    assert_eq!(paper["hex"], "#FFFFFF");
    let cyan = ok(&mut s, json!({"cmd": "color.convert", "cmyk": [1, 0, 0, 0], "to": "srgb"}));
    assert_eq!(cyan["clipped"], true, "{cyan}");
    assert_eq!(run(&mut s, to_cmyk("#123456"))["version"], 0, "no document change");
}

#[test]
fn color_convert_errors_are_typed() {
    let mut s = session();
    for (cmd, code) in [
        (json!({"cmd": "color.convert", "to": "cmyk"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "hex": "#FFFFFF", "cmyk": [0, 0, 0, 0], "to": "cmyk"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "hex": "#FFFFFF", "to": "srgb"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "hex": "red", "to": "cmyk"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "cmyk": [0, 0, 1.5, 0], "to": "srgb"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "cmyk": [0, 0, 0], "to": "srgb"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "hex": "#FFFFFF", "to": "lab"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "hex": "#FFFFFF", "to": "cmyk", "intent": "saturation"}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "hex": "#FFFFFF", "to": "cmyk", "profile": null}), "InvalidArgument"),
        (json!({"cmd": "color.convert", "hex": "#FFFFFF", "to": "cmyk", "cmykProfile": ""}), "InvalidArgument"),
    ] {
        assert_eq!(err(&mut s, cmd.clone())["code"], code, "{cmd}");
    }
    let missing = tmp("missing.icc");
    let srgb = tmp("srgb.icc");
    std::fs::write(&srgb, IccProfile::srgb().bytes()).unwrap();
    let junk = tmp("junk.icc");
    std::fs::write(&junk, b"junk").unwrap();
    for (p, code) in [(missing, "NotFound"), (srgb, "UnsupportedColorMode"), (junk, "InvalidProfile")] {
        let mut c = to_cmyk("#FFFFFF");
        c["cmykProfile"] = json!(p.to_string_lossy());
        let e = err(&mut s, c);
        assert_eq!(e["code"], code, "{p:?}: {e}");
        assert!(!e["message"].as_str().unwrap().contains(&*p.to_string_lossy()), "no path in messages");
    }
}

#[test]
fn user_profile_is_cached_and_reloaded() {
    let mut s = session();
    let p = tmp("press.icc");
    std::fs::write(&p, BUNDLED_CMYK_PROFILE).unwrap();
    let mut c = to_cmyk("#336699");
    c["cmykProfile"] = json!(p.to_string_lossy());
    let a = ok(&mut s, c.clone());
    assert_eq!(a, ok(&mut s, to_cmyk("#336699")), "same profile bytes, same numbers");
    // Replacing the file is noticed (length / mtime change).
    std::fs::write(&p, IccProfile::srgb().bytes()).unwrap();
    assert_eq!(err(&mut s, c)["code"], "UnsupportedColorMode");
}

#[test]
fn color_convert_latency_after_warm_up() {
    let mut s = session();
    ok(&mut s, to_cmyk("#336699"));
    ok(&mut s, json!({"cmd": "color.convert", "cmyk": [0.2, 0.4, 0.6, 0.1], "to": "srgb"}));
    let n = 2000;
    let start = Instant::now();
    for i in 0..n {
        let v = (i % 256) as u8;
        let out = s.execute_json(&to_cmyk(&format!("#{v:02X}{:02X}80", 255 - v)).to_string());
        assert!(out.contains("\"ok\":") && !out.contains("\"error\""), "{out}");
    }
    let per_rgb = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let start = Instant::now();
    for i in 0..n {
        let c = (i % 100) as f64 / 100.0;
        let cmd = json!({"cmd": "color.convert", "cmyk": [c, 0.3, 1.0 - c, 0.1], "to": "srgb"});
        let out = s.execute_json(&cmd.to_string());
        assert!(out.contains("\"ok\":") && !out.contains("\"error\""), "{out}");
    }
    let per_cmyk = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("color.convert latency (JSON in/out, warm cache): to cmyk {per_rgb:.4} ms, to srgb {per_cmyk:.4} ms");
    assert!(per_rgb < 1.0 && per_cmyk < 1.0, "{per_rgb} / {per_cmyk} ms");
}

#[test]
fn profile_inspect_contract() {
    let mut s = session();
    let b = ok(&mut s, json!({"cmd": "profile.inspect", "path": null}));
    assert_eq!(
        b,
        json!({
            "description": "Coated_Fogra39L_VIGC_300.icc",
            "copyright": "Copyright X-Rite, Inc.",
            "class": "prtr",
            "colorSpace": "CMYK",
            "pcs": "Lab",
            "version": "2.1",
            "intents": ["perceptual", "relative_colorimetric", "saturation"],
            "bytes": 8_652_444,
            "bundled": true,
            "usableForCmykExport": true,
            "reason": null
        })
    );
    assert_eq!(ok(&mut s, json!({"cmd": "profile.inspect"})), b, "path defaults to null");
    let srgb = tmp("inspect-srgb.icc");
    std::fs::write(&srgb, IccProfile::srgb().bytes()).unwrap();
    let r = ok(&mut s, json!({"cmd": "profile.inspect", "path": srgb.to_string_lossy()}));
    assert_eq!(
        (r["class"].as_str(), r["colorSpace"].as_str(), r["pcs"].as_str()),
        (Some("mntr"), Some("RGB"), Some("XYZ"))
    );
    assert_eq!(r["usableForCmykExport"], false);
    assert!(r["reason"].as_str().unwrap().contains("output (printer) profile"));
    assert_eq!(r["bundled"], false);
    assert_eq!(r["version"], "4.3");
    let junk = tmp("inspect-junk.icc");
    std::fs::write(&junk, vec![7u8; 300]).unwrap();
    assert_eq!(
        err(&mut s, json!({"cmd": "profile.inspect", "path": junk.to_string_lossy()}))["code"],
        "InvalidProfile"
    );
    let missing = tmp("inspect-missing.icc");
    assert_eq!(err(&mut s, json!({"cmd": "profile.inspect", "path": missing.to_string_lossy()}))["code"], "NotFound");
    assert_eq!(err(&mut s, json!({"cmd": "profile.inspect", "file": "x"}))["code"], "InvalidArgument");
}
