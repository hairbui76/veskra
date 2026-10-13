//! ADR-019 amendment 1: the second wave of built-in brushes — pinned procedural asset hashes
//! (cross-platform determinism of the generators), deterministic and batching-independent
//! rendering with and without hardware pressure, visible and mutually distinct strokes.
//!
//! `review_sheets` (ignored) writes contact sheets for a visual review:
//! `VESKRA_SHEET_DIR=<dir> cargo test -p veskra-brush --test builtin_wave2 -- --ignored`.

mod common;

use std::fmt::Write as _;

use common::*;
use veskra_brush::{
    BUILTIN_ASSET_NAMES, BrushCategory, BrushPreset, FIRST_WAVE_IDS, ResolvedAssets, StrokeContext, StrokeMode,
    StrokeRenderer, TextureSource, builtin_asset_hash, builtin_assets, builtin_presets, load_preset_json, render_tip,
};
use veskra_core::{ColorRgba, RectI, SampleFlags, StrokeSample};
use veskra_raster::TileGrid;

/// Smallest accepted mean alpha difference between two built-in strokes (wave 2 involved).
const MIN_MEAN_ALPHA_DIFF: f64 = 6.0;

fn wave2() -> Vec<BrushPreset> {
    builtin_presets().into_iter().filter(|p| !FIRST_WAVE_IDS.contains(&p.id.as_str())).collect()
}

/// Mouse-like input: the same S-curve with no hardware pressure (the engine's 0.5 fallback).
fn no_pressure(samples: &[StrokeSample]) -> Vec<StrokeSample> {
    samples
        .iter()
        .map(|s| {
            let mut m = *s;
            m.pressure = 0.5;
            m.flags.remove(SampleFlags::PRESSURE_REPORTED);
            m
        })
        .collect()
}

fn hashes_path() -> std::path::PathBuf {
    veskra_testkit::fixtures_dir().join("brushes").join("builtin_asset_hashes.txt")
}

#[test]
fn builtin_asset_hashes_are_pinned() {
    let mut text = String::new();
    for name in BUILTIN_ASSET_NAMES {
        let h = builtin_asset_hash(name).unwrap_or_else(|| panic!("asset {name} not generated"));
        let _ = writeln!(text, "{name} {h}");
    }
    assert_eq!(builtin_assets().len(), BUILTIN_ASSET_NAMES.len());
    if std::env::var("VESKRA_BLESS").as_deref() == Ok("1") {
        std::fs::write(hashes_path(), &text).expect("write hashes");
        return;
    }
    let want = std::fs::read_to_string(hashes_path()).expect("hash fixture missing (VESKRA_BLESS=1)");
    assert_eq!(text, want.replace("\r\n", "\n"), "a procedural brush asset changed (or differs on this platform)");
}

#[test]
fn new_categories_round_trip_and_stay_custom_when_unknown() {
    for (c, s) in [
        (BrushCategory::Watercolor, "watercolor"),
        (BrushCategory::Calligraphy, "calligraphy"),
        (BrushCategory::Nature, "nature"),
        (BrushCategory::Pattern, "pattern"),
    ] {
        assert_eq!(c.as_str(), s);
        assert_eq!(serde_json::to_value(c).unwrap(), serde_json::json!(s));
        let p = load_preset_json(&format!(r#"{{"schema": 3, "id": "x.y", "category": "{s}"}}"#)).unwrap().preset;
        assert_eq!(p.category, c);
    }
    // What a build that predates a category does with it (and this build with a future one).
    let p = load_preset_json(r#"{"schema": 3, "id": "x.y", "category": "hologram"}"#).unwrap().preset;
    assert_eq!(p.category, BrushCategory::Custom);
    // Textural categories are never vector-eligible; calligraphy is judged by its features.
    let mut round = veskra_brush::builtin_preset("round.hard").unwrap();
    for (c, ok) in [
        (BrushCategory::Calligraphy, true),
        (BrushCategory::Watercolor, false),
        (BrushCategory::Nature, false),
        (BrushCategory::Pattern, false),
    ] {
        round.category = c;
        assert_eq!(round.satisfies_vector_rule(), ok, "{c:?}");
    }
    let wave2_ids: Vec<String> = wave2().into_iter().map(|p| p.id).collect();
    let eligible: Vec<&String> =
        wave2_ids.iter().filter(|id| veskra_brush::builtin_preset(id).unwrap().vector_eligible).collect();
    assert_eq!(eligible, ["calligraphy.brush_marker"]);
}

#[test]
fn wave2_is_deterministic_and_batching_independent_with_and_without_pressure() {
    let pressure = s_curve(140, 8);
    let mouse = no_pressure(&pressure);
    assert_eq!(wave2().len(), 40);
    for p in wave2() {
        for (label, samples) in [("pressure", &pressure), ("mouse", &mouse)] {
            let a = render(&p, samples, 21, TileGrid::new(), ctx(StrokeMode::Paint), &[usize::MAX]);
            let b = render(&p, samples, 21, TileGrid::new(), ctx(StrokeMode::Paint), &[1]);
            let c = render(&p, samples, 21, TileGrid::new(), ctx(StrokeMode::Paint), &[4, 1, 9, 2]);
            assert!(a.tiles == b.tiles && a.tiles == c.tiles, "{} ({label}): batching changed the pixels", p.id);
            assert_eq!(a.dab_count, c.dab_count);
            assert!(premul_invariant_holds(&a.tiles), "{}", p.id);
            let painted = straight8(&a.tiles, canvas()).chunks_exact(4).filter(|px| px[3] > 8).count();
            assert!(painted > 400, "{} ({label}): only {painted} px painted", p.id);
            // Jitter and procedural grain are seeded; image textures are not.
            if !p.jitter.is_zero() || p.texture.as_ref().is_some_and(|t| t.source == TextureSource::Procedural) {
                let other = render(&p, samples, 22, TileGrid::new(), ctx(StrokeMode::Paint), &[usize::MAX]);
                assert!(other.tiles != a.tiles, "{} ({label}): seed has no effect", p.id);
            }
        }
    }
}

#[test]
fn all_builtins_render_mutually_distinct_strokes() {
    let samples = s_curve(120, 2026);
    let alphas: Vec<(String, Vec<u8>)> = builtin_presets()
        .iter()
        .filter(|p| !p.eraser)
        .map(|p| {
            let f = render(p, &samples, 77, TileGrid::new(), ctx(StrokeMode::Paint), &[3]);
            (p.id.clone(), straight8(&f.tiles, canvas()).chunks_exact(4).map(|px| px[3]).collect())
        })
        .collect();
    let mut pairs: Vec<(f64, String)> = Vec::new();
    for (i, (ia, a)) in alphas.iter().enumerate() {
        for (ib, b) in &alphas[i + 1..] {
            if FIRST_WAVE_IDS.contains(&ia.as_str()) && FIRST_WAVE_IDS.contains(&ib.as_str()) {
                continue;
            }
            // Mean absolute alpha difference over the pixels either stroke paints.
            let (mut diff, mut area) = (0u64, 0u64);
            for (x, y) in a.iter().zip(b) {
                if *x > 0 || *y > 0 {
                    diff += u64::from(x.abs_diff(*y));
                    area += 1;
                }
            }
            pairs.push((diff as f64 / area.max(1) as f64, format!("{ia} / {ib}")));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (mean, names) in pairs.iter().take(6) {
        eprintln!("close pair: {names} ({mean:.1} alpha levels)");
    }
    let (mean, names) = &pairs[0];
    assert!(*mean > MIN_MEAN_ALPHA_DIFF, "{names} render (almost) the same stroke ({mean:.1})");
}

// ------------------------------------------------------------------ visual review (ignored)

fn over_white(rgba: &[u8], tint: [u8; 3]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|p| {
            let a = u32::from(p[3]);
            let mix = |c: u8, t: u8| ((u32::from(c) * a + u32::from(t) * (255 - a) + 127) / 255) as u8;
            [mix(p[0], tint[0]), mix(p[1], tint[1]), mix(p[2], tint[2]), 255]
        })
        .collect()
}

fn blit(dst: &mut [u8], dw: u32, src: &[u8], sw: u32, sh: u32, ox: u32, oy: u32) {
    for y in 0..sh {
        let d = (((oy + y) * dw + ox) * 4) as usize;
        let s = (y * sw * 4) as usize;
        dst[d..d + (sw * 4) as usize].copy_from_slice(&src[s..s + (sw * 4) as usize]);
    }
}

#[test]
#[ignore = "writes contact sheets for a visual review (VESKRA_SHEET_DIR)"]
fn review_sheets() {
    let Ok(dir) = std::env::var("VESKRA_SHEET_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ink = ColorRgba::from_rgba8(40, 70, 150, 255);
    // Tips: 10 per row, 128 px each.
    let all = builtin_presets();
    let (cols, cell) = (10u32, 132u32);
    let rows = (all.len() as u32).div_ceil(cols);
    let (sw, sh) = (cols * cell, rows * cell);
    let mut sheet = vec![255u8; (sw * sh * 4) as usize];
    for (i, p) in all.iter().enumerate() {
        let t = render_tip(p, &ResolvedAssets::resolve(p, &[builtin_assets()]), 128).unwrap();
        let rgba: Vec<u8> = t.coverage.iter().flat_map(|&a| [17, 17, 17, a]).collect();
        let img = over_white(&rgba, [255, 255, 255]);
        blit(&mut sheet, sw, &img, 128, 128, (i as u32 % cols) * cell + 2, (i as u32 / cols) * cell + 2);
    }
    std::fs::write(dir.join("tips.png"), veskra_testkit::encode_png_rgba8(sw, sh, &sheet)).unwrap();
    // Strokes: one row per brush, pressure S-curve on the left, mouse wave on the right.
    let (w, h) = (1100u32, 300u32);
    let clip = RectI::from_size(w, h);
    let n = 140;
    let left: Vec<StrokeSample> = (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let x = 60.0 + 440.0 * t;
            let y = 150.0 - 80.0 * (std::f64::consts::TAU * t).sin();
            let p = 0.1 + 0.9 * (std::f64::consts::PI * t).sin();
            StrokeSample::confirmed(x as f32, y as f32, i as u64 * 4_166_667, p as f32)
        })
        .collect();
    let right: Vec<StrokeSample> = no_pressure(
        &(0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64;
                let x = 600.0 + 440.0 * t;
                let y = 150.0 + 60.0 * (std::f64::consts::TAU * 1.5 * t).sin();
                StrokeSample::confirmed(x as f32, y as f32, i as u64 * 8_000_000, 0.5)
            })
            .collect::<Vec<_>>(),
    );
    for (k, chunk) in wave2().chunks(5).enumerate() {
        let mut sheet = vec![255u8; (w * h * 5 * 4) as usize];
        for (j, p) in chunk.iter().enumerate() {
            let mut tiles = TileGrid::new();
            for samples in [&left, &right] {
                let mut r = StrokeRenderer::begin(
                    p.clone(),
                    ink,
                    5,
                    tiles.clone(),
                    StrokeContext::new(StrokeMode::Paint, clip),
                );
                r.add_samples(samples);
                tiles = r.finish().tiles;
            }
            let rgba = straight8(&tiles, clip);
            let tint = if j % 2 == 0 { [255, 255, 255] } else { [246, 246, 240] };
            blit(&mut sheet, w, &over_white(&rgba, tint), w, h, 0, j as u32 * h);
        }
        let ids: Vec<&str> = chunk.iter().map(|p| p.id.as_str()).collect();
        eprintln!("strokes_{k}.png: {ids:?}");
        std::fs::write(dir.join(format!("strokes_{k}.png")), veskra_testkit::encode_png_rgba8(w, h * 5, &sheet))
            .unwrap();
    }
}
