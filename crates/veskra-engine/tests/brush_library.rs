//! ADR-019 brush library commands: library open/list/get/save/delete/duplicate/export/import,
//! `brush.asset.add`, `brush.preview`, Infinite Painter `.prbr`/`.przp` import (synthetic
//! archives, including hostile ones) and embedding of brush assets in `.veskra` documents.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use veskra_engine::{EngineConfig, Session};
use veskra_format::zip::{Method, ZipWriter};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-brushlib-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn session() -> Session {
    Session::new(EngineConfig { deterministic_seed: Some(11), ..Default::default() })
}

fn run(s: &mut Session, cmd: Value) -> Value {
    let out: Value = serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap();
    assert!(out.get("error").is_none(), "{cmd} → {out}");
    out
}

fn ok(s: &mut Session, cmd: Value) -> Value {
    run(s, cmd)["ok"].clone()
}

fn err(s: &mut Session, cmd: Value) -> String {
    let out: Value = serde_json::from_str(&s.execute_json(&cmd.to_string())).unwrap();
    out["error"]["key"].as_str().unwrap_or_else(|| panic!("expected an error for {cmd}, got {out}")).to_string()
}

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(veskra_testkit::fixtures_dir().join("brushes").join("infinite_painter").join(name)).unwrap()
}

/// Gray PNG: a dark disc (black = paint) on white.
fn disc_png(size: u32) -> Vec<u8> {
    let c = size as f64 / 2.0;
    let px: Vec<u8> = (0..size * size)
        .flat_map(|i| {
            let (x, y) = ((i % size) as f64 + 0.5 - c, (i / size) as f64 + 0.5 - c);
            let v = if x * x + y * y < (c * 0.8) * (c * 0.8) { (x.abs() * 3.0) as u8 } else { 255 };
            [v, v, v, 255]
        })
        .collect();
    veskra_testkit::encode_png_rgba8(size, size, &px)
}

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = ZipWriter::new(Vec::new());
    for (n, d) in entries {
        z.add(n, d, Method::Stored).unwrap();
    }
    z.finish().unwrap().0
}

/// Raw ZIP writer for hostile archives: arbitrary names, method and declared sizes.
fn raw_zip(entries: &[(&str, u16, &[u8], u32)]) -> Vec<u8> {
    let put16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for (name, method, data, usize_) in entries {
        let off = out.len() as u32;
        let crc = 0u32;
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, 20);
        put16(&mut out, 0);
        put16(&mut out, *method);
        put16(&mut out, 0);
        put16(&mut out, 0x21);
        put32(&mut out, crc);
        put32(&mut out, data.len() as u32);
        put32(&mut out, *usize_);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        put32(&mut cd, 0x0201_4b50);
        put16(&mut cd, 20);
        put16(&mut cd, 20);
        put16(&mut cd, 0);
        put16(&mut cd, *method);
        put16(&mut cd, 0);
        put16(&mut cd, 0x21);
        put32(&mut cd, crc);
        put32(&mut cd, data.len() as u32);
        put32(&mut cd, *usize_);
        put16(&mut cd, name.len() as u16);
        for _ in 0..4 {
            put16(&mut cd, 0);
        }
        put32(&mut cd, 0);
        put32(&mut cd, off);
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    let cd_len = cd.len() as u32;
    out.extend_from_slice(&cd);
    put32(&mut out, 0x0605_4b50);
    put32(&mut out, 0);
    put16(&mut out, entries.len() as u16);
    put16(&mut out, entries.len() as u16);
    put32(&mut out, cd_len);
    put32(&mut out, cd_off);
    put16(&mut out, 0);
    out
}

fn b64_decode(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0,
    };
    let mut out = Vec::new();
    for chunk in s.as_bytes().chunks(4) {
        let n = chunk.iter().fold(0u32, |acc, &c| acc << 6 | val(c) as u32);
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    out
}

#[test]
fn list_contains_builtins_first_and_library_commands_need_an_open_library() {
    let mut s = session();
    let list = ok(&mut s, json!({"cmd": "brush.list"}));
    let presets = list["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 30);
    let ids: Vec<&str> = presets.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, veskra_brush::BUILTIN_IDS);
    for p in presets {
        for k in
            ["id", "name", "category", "source", "folder", "vectorEligible", "eraser", "version", "editable", "current"]
        {
            assert!(p.get(k).is_some(), "{k} missing in {p}");
        }
        assert_eq!(p["source"], "builtin");
        assert_eq!(p["editable"], false);
    }
    assert_eq!(list["current"]["id"], "pencil.smooth");
    assert_eq!(err(&mut s, json!({"cmd": "brush.save", "preset": {"name": "x"}})), "invalid_state");
    assert_eq!(err(&mut s, json!({"cmd": "brush.get", "id": "nope"})), "not_found");
    assert_eq!(err(&mut s, json!({"cmd": "brush.library.open", "dir": "x", "extra": 1})), "invalid_argument");
    // Every built-in is selectable and drawable.
    let g = ok(&mut s, json!({"cmd": "brush.get", "id": "paint.round_bristle"}));
    assert_eq!(g["preset"]["schema"], 3);
    assert_eq!(g["preset"]["tip"]["shape"], "image");
    assert_eq!(g["assets"][0]["available"], true);
}

#[test]
fn save_update_duplicate_delete_export_import_round_trip() {
    let dir = tmp("crud");
    let mut s = session();
    let opened = ok(&mut s, json!({"cmd": "brush.library.open", "dir": p(&dir.join("lib"))}));
    assert_eq!((opened["brushes"].as_u64(), opened["assets"].as_u64()), (Some(0), Some(0)));
    let v0 = s.version();

    // brush.asset.add → hash, then a save that references it (no assets map).
    let png_path = dir.join("tip.png");
    std::fs::write(&png_path, disc_png(40)).unwrap();
    let a = ok(&mut s, json!({"cmd": "brush.asset.add", "path": p(&png_path)}));
    let hash = a["hash"].as_str().unwrap().to_string();
    assert_eq!(
        (a["width"].as_u64(), a["height"].as_u64(), a["channel"].as_str()),
        (Some(40), Some(40), Some("darkness"))
    );
    assert!(dir.join("lib/assets").join(format!("{hash}.png")).exists());
    let preset = json!({"name": "My bristle", "category": "paint", "size_px": 22, "spacing": 0.08,
        "tip": {"shape": "image", "image": hash, "rotation": "direction"},
        "jitter": {"angle": 0.1}, "vector_eligible": true});
    let saved = ok(&mut s, json!({"cmd": "brush.save", "preset": preset}));
    let id = saved["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("custom."));
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["vectorEligible"], false, "computed by the engine, not taken from the UI");
    assert!(dir.join("lib").join(format!("{id}.vbrush")).exists());

    // Update: same id, version bump; the engine computes vectorEligible (true for a plain round).
    let mut upd = ok(&mut s, json!({"cmd": "brush.get", "id": id}))["preset"].clone();
    upd["tip"] = json!({"shape": "round", "hardness": 1.0});
    upd["jitter"] = json!({});
    upd["category"] = json!("ink");
    let saved2 = ok(&mut s, json!({"cmd": "brush.save", "preset": upd}));
    assert_eq!((saved2["id"].as_str(), saved2["version"].as_u64()), (Some(id.as_str()), Some(2)));
    assert_eq!(saved2["vectorEligible"], true);
    let got = ok(&mut s, json!({"cmd": "brush.get", "id": id}));
    assert_eq!(got["preset"]["category"], "ink");
    assert_eq!(got["preset"]["source"], "custom");
    assert_eq!(got["editable"], true);

    // Placeholder assets map + channel/invert object form.
    let tex_path = dir.join("grain.png");
    std::fs::write(&tex_path, disc_png(24)).unwrap();
    let with_map = ok(
        &mut s,
        json!({"cmd": "brush.save", "preset": {"name": "Grainy",
        "texture": {"source": "image", "image": "my-grain", "scale": 1.0, "depth": 0.5}},
        "assets": {"my-grain": {"path": p(&tex_path), "channel": "luminance", "invert": true}}}),
    );
    let grainy = with_map["id"].as_str().unwrap().to_string();
    let g = ok(&mut s, json!({"cmd": "brush.get", "id": grainy}));
    assert_eq!(g["preset"]["texture"]["image"].as_str().unwrap().len(), 64);

    // Strict input: unknown fields, unknown assets, built-in ids.
    assert_eq!(err(&mut s, json!({"cmd": "brush.save", "preset": {"name": "x", "sizee": 3}})), "invalid_argument");
    assert_eq!(
        err(&mut s, json!({"cmd": "brush.save", "preset": {"name": "x", "size_px": "big"}})),
        "invalid_argument"
    );
    assert_eq!(err(&mut s, json!({"cmd": "brush.save", "preset": {"name": "x", "spacing": 9}})), "invalid_argument");
    assert_eq!(err(&mut s, json!({"cmd": "brush.save", "preset": {"schema": 4, "name": "x"}})), "unsupported_schema");
    assert_eq!(
        err(
            &mut s,
            json!({"cmd": "brush.save", "preset": {"name": "x", "tip": {"shape": "image", "image": "0".repeat(64)}}})
        ),
        "not_found"
    );
    let from_builtin = ok(&mut s, json!({"cmd": "brush.save", "preset": {"id": "round.hard", "name": "Mine"}}));
    assert_ne!(from_builtin["id"], "round.hard", "built-ins are never overwritten");
    assert!(from_builtin["warnings"][0].as_str().unwrap().contains("built-in"));

    // Duplicate (built-in) and delete.
    let dup = ok(&mut s, json!({"cmd": "brush.duplicate", "id": "texture.chalk", "name": "Chalk 2"}));
    let dup_id = dup["id"].as_str().unwrap().to_string();
    assert_eq!(ok(&mut s, json!({"cmd": "brush.get", "id": dup_id}))["preset"]["name"], "Chalk 2");
    assert_eq!(err(&mut s, json!({"cmd": "brush.delete", "id": "texture.chalk"})), "invalid_argument");
    ok(&mut s, json!({"cmd": "brush.delete", "id": dup_id}));
    assert_eq!(err(&mut s, json!({"cmd": "brush.get", "id": dup_id})), "not_found");

    // List order: built-ins, then user brushes; library edits are not document edits.
    let list = ok(&mut s, json!({"cmd": "brush.list"}));
    let ps = list["presets"].as_array().unwrap();
    assert_eq!(ps.len(), 33);
    assert!(ps[30..].iter().all(|x| x["editable"] == true && x["source"] == "custom"));
    assert_eq!(s.version(), v0);

    // Export / import into a fresh library keeps the brush and its assets.
    let out = dir.join("export.vbrush");
    assert_eq!(
        err(&mut s, json!({"cmd": "brush.export", "id": id, "path": p(&dir.join("x.prbr"))})),
        "invalid_argument"
    );
    ok(&mut s, json!({"cmd": "brush.export", "id": grainy, "path": p(&out)}));
    let mut s2 = session();
    ok(&mut s2, json!({"cmd": "brush.library.open", "dir": p(&dir.join("lib2"))}));
    let imp = ok(&mut s2, json!({"cmd": "brush.import", "path": p(&out)}));
    assert_eq!(imp["imported"][0]["id"], grainy.as_str(), "a free id is kept");
    assert_eq!(imp["failed"].as_array().unwrap().len(), 0);
    let a = ok(&mut s, json!({"cmd": "brush.get", "id": grainy}))["preset"].clone();
    let b = ok(&mut s2, json!({"cmd": "brush.get", "id": grainy}))["preset"].clone();
    assert_eq!(a, b);
    // Importing the same file again gets a new id.
    let again = ok(&mut s2, json!({"cmd": "brush.import", "path": p(&out)}));
    assert_ne!(again["imported"][0]["id"], grainy.as_str());

    // Re-open from disk in a new session: everything is back; a corrupt file is skipped.
    std::fs::write(dir.join("lib").join("broken.vbrush"), b"PK\x03\x04garbage").unwrap();
    let mut s3 = session();
    let re = ok(&mut s3, json!({"cmd": "brush.library.open", "dir": p(&dir.join("lib"))}));
    assert_eq!(re["brushes"], 3);
    assert!(re["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().starts_with("broken.vbrush")));
    assert_eq!(ok(&mut s3, json!({"cmd": "brush.get", "id": id}))["preset"]["version"], 2);
    ok(&mut s3, json!({"cmd": "brush.select", "preset": id}));
    let _ = std::fs::remove_dir_all(&dir);
}

fn prbr(props: &[u8], head: Option<&[u8]>) -> Vec<u8> {
    let preview = disc_png(8);
    let mut e: Vec<(&str, &[u8])> = vec![("properties.json", props), ("preview", &preview)];
    if let Some(h) = head {
        e.push(("head", h));
    }
    zip(&e)
}

#[test]
fn infinite_painter_prbr_and_przp_import() {
    let dir = tmp("ip");
    let mut s = session();
    ok(&mut s, json!({"cmd": "brush.library.open", "dir": p(&dir.join("lib"))}));
    let head = disc_png(32);
    let template = fixture("template-properties.json");
    let manga = fixture("manga_inker.json");

    // Single .prbr (named .bin: detection is by content).
    let single = dir.join("brush.bin");
    std::fs::write(&single, prbr(&template, Some(&head))).unwrap();
    let r = ok(&mut s, json!({"cmd": "brush.import", "path": p(&single)}));
    let item = &r["imported"][0];
    assert_eq!(item["folder"], "Imported");
    assert!(item["approximated"].as_array().unwrap().len() >= 2);
    let id = item["id"].as_str().unwrap().to_string();
    let g = ok(&mut s, json!({"cmd": "brush.get", "id": id}));
    assert_eq!(g["preset"]["source"], "imported:infinite_painter");
    assert_eq!(g["preset"]["category"], "imported");
    assert_eq!(g["preset"]["tip"]["shape"], "image");

    // Pack: index with two listed brushes (one missing), one nested pack, one unlisted brush.
    let inner_pack = zip(&[("index.json", br#"{"brush-folders":[]}"#), ("Brushes/x.prbr", &prbr(&manga, None))]);
    let index = br#"{"version":1,"brush-folders":[{"version":3,"name":"Inks","selected":0,"brushes":[
        {"name":"Manga Inker","id":0,"hidden":false},{"name":"Ghost","id":1,"hidden":true},{"name":"Nested","id":2,"hidden":false}]}]}"#;
    let pack = zip(&[
        ("index.json", index),
        ("Brushes/Manga Inker.prbr", &prbr(&manga, None)),
        ("Brushes/Nested.prbr", &inner_pack),
        ("Brushes/Extra.prbr", &prbr(&template, Some(&head))),
    ]);
    let pack_path = dir.join("My Pack.przp");
    std::fs::write(&pack_path, pack).unwrap();
    let r = ok(&mut s, json!({"cmd": "brush.import", "path": p(&pack_path)}));
    let imported = r["imported"].as_array().unwrap();
    assert_eq!(imported.len(), 2, "{r}");
    assert_eq!(imported[0]["name"], "Manga Inker");
    assert_eq!(imported[0]["folder"], "Inks");
    assert_eq!(imported[1]["folder"], "My Pack", "unlisted brushes land in the pack folder");
    assert_eq!(imported[0]["unsupported"][0], "head style 1 (meaning unknown; normal style used)");
    let failed: Vec<(String, String)> = r["failed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap().into(), f["reason"].as_str().unwrap().into()))
        .collect();
    assert!(failed.iter().any(|(n, why)| n == "Ghost" && why.contains("missing")), "{failed:?}");
    assert!(failed.iter().any(|(n, why)| n == "Nested" && why.contains("nested")), "{failed:?}");
    assert!(r["warnings"][0].as_str().unwrap().contains("not listed"));
    // Imported brushes draw.
    ok(&mut s, json!({"cmd": "document.create", "width": 128, "height": 96}));
    ok(&mut s, json!({"cmd": "brush.select", "preset": imported[1]["id"]}));
    ok(&mut s, json!({"cmd": "stroke.begin"}));
    ok(
        &mut s,
        json!({"cmd": "stroke.samples", "samples": [{"x": 10, "y": 50, "t": 0, "p": 0.6}, {"x": 110, "y": 40, "t": 50000000, "p": 0.9}]}),
    );
    assert_eq!(ok(&mut s, json!({"cmd": "stroke.commit"}))["committed"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_archives_are_typed_errors() {
    let dir = tmp("hostile");
    let mut s = session();
    ok(&mut s, json!({"cmd": "brush.library.open", "dir": p(&dir.join("lib"))}));
    let write = |name: &str, bytes: &[u8]| {
        let f = dir.join(name);
        std::fs::write(&f, bytes).unwrap();
        p(&f)
    };
    let template = fixture("template-properties.json");
    // Not an archive.
    let f = write("a.prbr", b"\x89PNG not a zip");
    assert_eq!(err(&mut s, json!({"cmd": "brush.import", "path": f})), "invalid_argument");
    // Path traversal entry name.
    let f = write("b.przp", &raw_zip(&[("../evil.prbr", 0, b"x", 1)]));
    assert_eq!(err(&mut s, json!({"cmd": "brush.import", "path": f})), "corrupt_data");
    // Zip bomb: 10 bytes of DEFLATE declared as 300 MB.
    let f = write("c.przp", &raw_zip(&[("Brushes/bomb.prbr", 8, &[0u8; 10], 300_000_000)]));
    assert_eq!(err(&mut s, json!({"cmd": "brush.import", "path": f})), "allocation_limit");
    // More than 500 brushes.
    let one = prbr(&template, None);
    let names: Vec<String> = (0..501).map(|i| format!("Brushes/b{i}.prbr")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &one[..])).collect();
    let f = write("d.przp", &zip(&entries));
    assert_eq!(err(&mut s, json!({"cmd": "brush.import", "path": f})), "allocation_limit");
    // Huge properties.json inside a pack fails that brush only.
    let huge = vec![b' '; 2 * 1024 * 1024];
    let pack = zip(&[("Brushes/huge.prbr", &prbr(&huge, None)), ("Brushes/ok.prbr", &one)]);
    let f = write("e.przp", &pack);
    let r = ok(&mut s, json!({"cmd": "brush.import", "path": f}));
    assert_eq!(r["imported"].as_array().unwrap().len(), 1);
    assert_eq!(r["failed"][0]["name"], "huge");
    // An archive with no brush.
    let f = write("f.zip", &zip(&[("readme.txt", b"hi")]));
    assert_eq!(err(&mut s, json!({"cmd": "brush.import", "path": f})), "invalid_argument");
    // Truncated archive.
    let full = zip(&[("properties.json", &template[..])]);
    let f = write("g.prbr", &full[..full.len() / 2]);
    assert!(
        ["corrupt_data", "invalid_argument"].contains(&err(&mut s, json!({"cmd": "brush.import", "path": f})).as_str())
    );
    // A .vbrush whose asset does not match its hash.
    let bad_vb = zip(&[
        ("mimetype", b"application/x-veskra-brush"),
        (
            "brush.json",
            format!(r#"{{"schema":3,"id":"x.y","tip":{{"shape":"image","image":"{}"}}}}"#, "a".repeat(64)).as_bytes(),
        ),
        (&format!("assets/{}.png", "a".repeat(64)), &disc_png(8)),
    ]);
    let f = write("h.vbrush", &bad_vb);
    assert_eq!(err(&mut s, json!({"cmd": "brush.import", "path": f})), "corrupt_data");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_is_a_bounded_png_in_the_current_color() {
    let mut s = session();
    ok(&mut s, json!({"cmd": "color.set", "hex": "#CC2200"}));
    for id in ["round.hard", "paint.round_bristle", "eraser.soft", "effect.glitter"] {
        let r = ok(&mut s, json!({"cmd": "brush.preview", "id": id, "width": 300, "height": 80}));
        assert_eq!(r["synthetic"], true);
        let png = b64_decode(r["png"].as_str().unwrap());
        let (w, h, rgba) = veskra_testkit::decode_png_rgba8(&png).unwrap();
        assert_eq!((w, h), (300, 80));
        let painted = rgba.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(painted > 300, "{id}: {painted}");
        if id == "round.hard" {
            let px = rgba.chunks_exact(4).find(|p| p[3] == 255).unwrap();
            assert_eq!(&px[..3], &[0xCC, 0x22, 0x00]);
        }
    }
    for (w, h) in [(1025, 64), (64, 257), (8, 64), (64, 0)] {
        assert_eq!(
            err(&mut s, json!({"cmd": "brush.preview", "id": "round.hard", "width": w, "height": h})),
            "invalid_argument"
        );
    }
    assert_eq!(err(&mut s, json!({"cmd": "brush.preview", "id": "nope", "width": 64, "height": 64})), "not_found");
}

#[test]
fn documents_embed_brush_assets_and_rerender_without_the_library() {
    let dir = tmp("embed");
    let mut a = session();
    ok(&mut a, json!({"cmd": "brush.library.open", "dir": p(&dir.join("lib"))}));
    let png_path = dir.join("tip.png");
    std::fs::write(&png_path, disc_png(48)).unwrap();
    let hash =
        ok(&mut a, json!({"cmd": "brush.asset.add", "path": p(&png_path)}))["hash"].as_str().unwrap().to_string();
    let saved = ok(
        &mut a,
        json!({"cmd": "brush.save", "preset": {"name": "Custom tip", "size_px": 26, "spacing": 0.1,
        "tip": {"shape": "image", "image": hash, "rotation": "direction"}, "jitter": {"angle": 0.2, "size": 0.2},
        "wet_edges": 0.3}}),
    );
    let id = saved["id"].as_str().unwrap().to_string();
    ok(&mut a, json!({"cmd": "document.create", "width": 160, "height": 120}));
    ok(&mut a, json!({"cmd": "brush.select", "preset": id}));
    ok(&mut a, json!({"cmd": "stroke.begin"}));
    let samples: Vec<Value> = (0..30)
        .map(|i| json!({"x": 12 + i * 4, "y": 60 + (i % 7) * 3, "t": i * 8_000_000, "p": 0.5 + (i as f64) / 70.0}))
        .collect();
    ok(&mut a, json!({"cmd": "stroke.samples", "samples": samples}));
    ok(&mut a, json!({"cmd": "stroke.commit"}));
    // A built-in image brush too.
    ok(&mut a, json!({"cmd": "brush.select", "preset": "texture.chalk"}));
    ok(&mut a, json!({"cmd": "stroke.begin"}));
    ok(
        &mut a,
        json!({"cmd": "stroke.samples", "samples": [{"x": 20, "y": 20, "t": 0}, {"x": 140, "y": 100, "t": 90000000}]}),
    );
    ok(&mut a, json!({"cmd": "stroke.commit"}));
    let doc_path = dir.join("art.veskra");
    ok(&mut a, json!({"cmd": "document.save", "path": p(&doc_path)}));

    // Fresh session, no library at all.
    let mut b = session();
    let opened = run(&mut b, json!({"cmd": "document.open", "path": p(&doc_path)}));
    assert!(opened["warnings"].as_array().unwrap().is_empty(), "{opened}");
    let doc = b.document().unwrap().clone();
    // Custom tip + the chalk tip + the paper grain of texture.chalk.
    assert_eq!(doc.iter_brush_assets().count(), 3);
    let info = veskra_format::inspect(&std::fs::read(&doc_path).unwrap()).unwrap();
    assert_eq!(info.manifest.schema, 2);
    for st in doc.iter_strokes() {
        assert!(!st.brush_assets.is_empty());
        let (tiles, fallbacks) = veskra_engine::rerender_stroke(&doc, st, veskra_raster::TileGrid::new()).unwrap();
        assert!(fallbacks.is_empty());
        let layer = doc.layer(st.layer).unwrap().pixels().unwrap();
        if layer.strokes.len() == 2 {
            // Both strokes on one layer: re-render them in order over each other.
            let first = doc.stroke(layer.strokes[0]).unwrap();
            let (t0, _) = veskra_engine::rerender_stroke(&doc, first, veskra_raster::TileGrid::new()).unwrap();
            let second = doc.stroke(layer.strokes[1]).unwrap();
            let (t1, _) = veskra_engine::rerender_stroke(&doc, second, t0).unwrap();
            assert!(t1 == layer.tiles, "re-rendered strokes must reproduce the saved pixels exactly");
        } else {
            assert!(tiles == layer.tiles);
        }
    }
    // Re-saving from the library-less session keeps the embedded assets.
    let resaved = dir.join("again.veskra");
    ok(&mut b, json!({"cmd": "document.save", "path": p(&resaved)}));
    let reopened = veskra_format::decode(&std::fs::read(&resaved).unwrap()).unwrap();
    assert_eq!(reopened.doc.iter_brush_assets().count(), 3);

    // Without the embedded asset (parts carry no assets) the re-render reports the documented
    // fallback instead of failing.
    let st = doc.iter_strokes().find(|s| s.brush_assets == vec![hash.clone()]).unwrap().clone();
    let no_assets = veskra_doc::Document::from_parts(doc.to_parts()).unwrap();
    assert_eq!(no_assets.iter_brush_assets().count(), 0);
    let (_, fb) = veskra_engine::rerender_stroke(&no_assets, &st, veskra_raster::TileGrid::new()).unwrap();
    assert_eq!(fb, vec![hash.clone()]);

    // A damaged embedded asset (valid PNG, wrong content for its hash) opens with a warning.
    let mut damaged = veskra_doc::Document::from_parts(doc.to_parts()).unwrap();
    let other = disc_png(20);
    for (h, png) in doc.iter_brush_assets() {
        let bytes: std::sync::Arc<[u8]> = if *h == hash { std::sync::Arc::from(other.clone()) } else { png.clone() };
        damaged.insert_brush_asset(h, bytes).unwrap();
    }
    let bad_path = dir.join("damaged.veskra");
    std::fs::write(&bad_path, veskra_format::encode(&damaged, &veskra_format::SaveMeta::new("t", "t"), None).unwrap())
        .unwrap();
    let mut c = session();
    let opened = run(&mut c, json!({"cmd": "document.open", "path": p(&bad_path)}));
    let w = opened["warnings"].as_array().unwrap();
    assert!(w.iter().any(|w| w.as_str().unwrap().contains("damaged")), "{opened}");
    let d = c.document().unwrap().clone();
    let (_, fb) = veskra_engine::rerender_stroke(&d, &st, veskra_raster::TileGrid::new()).unwrap();
    assert_eq!(fb, vec![hash.clone()], "a hash mismatch is never used for rendering");
    let _ = std::fs::remove_dir_all(&dir);
}
