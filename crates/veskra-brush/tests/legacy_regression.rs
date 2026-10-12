//! Byte-exact regression for the ten pre-v3 built-in presets (brush system v2, ADR-019).
//!
//! `tests/fixtures/brushes/legacy_render_sha256.txt` was produced by the tree **before** the
//! schema-3 brush engine existed (commit 45b8689 + this test). Every line is the SHA-256 of the
//! full 16-bit premultiplied canvas after one stroke (little-endian `u16` RGBA, row-major) plus the
//! dab count. The test renders each preset two ways and both must match byte for byte:
//! * the current built-in preset of that id;
//! * the schema-2 JSON the old engine stored in stroke records (`tests/fixtures/brushes/v2/<id>.json`,
//!   written by the old tree), loaded through today's migration — i.e. a stroke from an existing
//!   document re-renders identically.
//!
//! Sample paths use arithmetic only (no `sin`/`powf`), so the golden does not depend on the
//! platform's libm. Bless deliberately with `VESKRA_BLESS=1` (only ever from a reviewed tree).

mod common;

use common::*;
use std::fmt::Write as _;
use std::sync::Arc;
use veskra_brush::{BrushPreset, StrokeContext, StrokeMode, StrokeRenderer, builtin_preset, load_preset_json};
use veskra_core::{ColorRgba, RectF, StrokeSample};
use veskra_raster::{Tile, TileGrid};
use veskra_testkit::Rng;

const LEGACY_IDS: [&str; 10] = [
    "pencil.rough",
    "pencil.smooth",
    "pen.technical",
    "pen.brush",
    "marker",
    "round.hard",
    "round.soft",
    "charcoal",
    "eraser.hard",
    "eraser.soft",
];

/// Cubic S-curve with a parabolic pressure taper and seeded jitter; arithmetic only.
fn poly_curve(n: usize, seed: u64, tilt: Option<f32>, azimuth: Option<f32>) -> Vec<StrokeSample> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let x = 20.0 + 214.0 * t;
            // 6.5·u·(1−u)·(1−2u) peaks near ±0.62: an S around y = 64.
            let s = 6.5 * t * (1.0 - t) * (1.0 - 2.0 * t);
            let y = 64.0 - 52.0 * s;
            let p = (0.1 + 0.9 * 4.0 * t * (1.0 - t) + rng.range(-0.03, 0.03)).clamp(0.0, 1.0);
            let mut sm = StrokeSample::confirmed(x as f32, y as f32, i as u64 * 4_166_667, p as f32);
            sm.tilt = tilt;
            sm.azimuth = azimuth;
            sm
        })
        .collect()
}

fn half_filled_base() -> TileGrid {
    let mut g = TileGrid::new();
    let t = Arc::new(Tile::new_filled(px16(ColorRgba::new(0.9, 0.5, 0.2, 0.6))));
    for c in TileGrid::tiles_in_rect(veskra_core::RectI::new(0, 0, 128, 128)) {
        g.set_tile(c, t.clone());
    }
    g
}

fn orange_base() -> TileGrid {
    filled_base(px16(ColorRgba::from_rgba8(240, 140, 40, 255)))
}

fn sha_hex(data: &[u16]) -> String {
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    veskra_testkit::sha256_hex(&bytes)
}

fn run(
    preset: &BrushPreset,
    samples: &[StrokeSample],
    seed: u64,
    base: TileGrid,
    ctx: StrokeContext,
    split: usize,
) -> String {
    let mut r = StrokeRenderer::begin(preset.clone(), INK, seed, base, ctx);
    for chunk in samples.chunks(split) {
        r.add_samples(chunk);
    }
    let fin = r.finish();
    format!("{} {}", sha_hex(&fin.tiles.read_rect(canvas())), fin.dab_count)
}

/// `(scenario, result)` for every scenario of one preset.
fn scenarios(p: &BrushPreset) -> Vec<(&'static str, String)> {
    let paint_base = || if p.eraser { orange_base() } else { TileGrid::new() };
    let mut out = Vec::new();
    // A: plain stroke with hardware pressure.
    out.push(("paint", run(p, &poly_curve(120, 2026, None, None), 77, paint_base(), ctx(StrokeMode::Paint), 3)));
    // B: tilt + azimuth reported, UI size/opacity overrides, odd batching.
    let mut c = ctx(StrokeMode::Paint);
    c.size_override = Some(9.0);
    c.opacity_override = Some(0.5);
    out.push(("tilt", run(p, &poly_curve(90, 7, Some(0.6), Some(1.0)), 5, paint_base(), c, 7)));
    // C: alpha lock over a half-filled base, fractional rect selection, stabilization override.
    let mut c = ctx(StrokeMode::Paint);
    c.alpha_lock = true;
    c.stabilization_override = Some(0.6);
    c.selection = Some(veskra_raster::select::rect(RectF::new(10.25, 8.5, 200.75, 110.5), canvas()));
    out.push(("lock", run(p, &poly_curve(140, 31, None, None), 9, half_filled_base(), c, 5)));
    // D: erase mode request (paint presets erase too) on an opaque base.
    out.push(("erase", run(p, &poly_curve(60, 3, None, None), 13, orange_base(), ctx(StrokeMode::Erase), 2)));
    // E: single-sample tap.
    let tap = vec![StrokeSample::confirmed(100.3, 50.7, 0, 0.8)];
    out.push(("tap", run(p, &tap, 1, paint_base(), ctx(StrokeMode::Paint), 1)));
    out
}

fn golden_path() -> std::path::PathBuf {
    veskra_testkit::fixtures_dir().join("brushes").join("legacy_render_sha256.txt")
}

fn v2_path(id: &str) -> std::path::PathBuf {
    veskra_testkit::fixtures_dir().join("brushes").join("v2").join(format!("{id}.json"))
}

#[test]
fn legacy_builtins_render_byte_identical() {
    let mut text = String::new();
    for id in LEGACY_IDS {
        let p = builtin_preset(id).unwrap_or_else(|| panic!("built-in {id} missing"));
        for (sc, r) in scenarios(&p) {
            let _ = writeln!(text, "{id} {sc} {r}");
        }
    }
    if std::env::var("VESKRA_BLESS").as_deref() == Ok("1") {
        std::fs::write(golden_path(), &text).expect("write golden");
        std::fs::create_dir_all(v2_path("x").parent().expect("dir")).expect("mkdir");
        for id in LEGACY_IDS {
            let p = builtin_preset(id).expect("builtin");
            std::fs::write(v2_path(id), p.to_json().expect("json")).expect("write v2 json");
        }
        eprintln!("blessed {}", golden_path().display());
        return;
    }
    let golden = std::fs::read_to_string(golden_path()).expect("golden missing (VESKRA_BLESS=1)");
    let golden = golden.replace("\r\n", "\n");
    for (want, got) in golden.lines().zip(text.lines()) {
        assert_eq!(got, want, "built-in render changed");
    }
    assert_eq!(golden.lines().count(), text.lines().count());
}

#[test]
fn legacy_schema2_records_rerender_byte_identical() {
    if std::env::var("VESKRA_BLESS").as_deref() == Ok("1") {
        return;
    }
    let golden = std::fs::read_to_string(golden_path()).expect("golden missing").replace("\r\n", "\n");
    let mut text = String::new();
    for id in LEGACY_IDS {
        let json = std::fs::read_to_string(v2_path(id)).expect("v2 fixture");
        let p = load_preset_json(&json).unwrap_or_else(|e| panic!("{id}: {e}")).preset;
        assert_eq!(p.id, id);
        for (sc, r) in scenarios(&p) {
            let _ = writeln!(text, "{id} {sc} {r}");
        }
    }
    for (want, got) in golden.lines().zip(text.lines()) {
        assert_eq!(got, want, "schema-2 stroke record no longer re-renders identically");
    }
}
