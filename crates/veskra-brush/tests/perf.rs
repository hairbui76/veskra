//! Host-CPU performance probe (not a device measurement). Run in release:
//! `cargo test -p veskra-brush --release -- --ignored --nocapture`.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::INK;
use veskra_brush::{StrokeContext, StrokeMode, StrokeRenderer, builtin_preset};
use veskra_core::{RectI, StrokeSample};
use veskra_raster::{Tile, TileGrid};

#[test]
#[ignore = "performance probe; run in release with --ignored --nocapture"]
fn perf_2000_samples_40px_on_4096() {
    let size = 4096u32;
    let clip = RectI::from_size(size, size);
    let mut base = TileGrid::new();
    let white = Arc::new(Tile::new_filled([65535; 4]));
    for c in TileGrid::tiles_in_rect(clip) {
        base.set_tile(c, white.clone());
    }
    // Lissajous path ~2 px between samples (fast-ish 240 Hz pen), varying pressure.
    let n = 2000;
    let samples: Vec<StrokeSample> = (0..n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let a = t * std::f64::consts::TAU;
            let x = 2048.0 + 600.0 * (a * 1.0).sin();
            let y = 2048.0 + 400.0 * (a * 2.0).sin();
            let p = 0.5 + 0.45 * (a * 7.0).sin();
            StrokeSample::confirmed(x as f32, y as f32, i as u64 * 4_166_667, p as f32)
        })
        .collect();
    let len: f64 = samples.windows(2).map(|w| ((w[1].x - w[0].x).hypot(w[1].y - w[0].y)) as f64).sum();
    let mut preset = builtin_preset("round.hard").unwrap();
    preset.size_px = 40.0;
    preset.min_size_ratio = 1.0;
    for (label, ids) in
        [("round.hard 40px", preset.clone()), ("round.soft 40px (BuildUp)", builtin_preset("round.soft").unwrap())]
    {
        let mut display = base.clone();
        let t0 = Instant::now();
        let mut r =
            StrokeRenderer::begin(ids.clone(), INK, 1, base.clone(), StrokeContext::new(StrokeMode::Paint, clip));
        let mut add_ns = 0u128;
        for s in &samples {
            let ta = Instant::now();
            let d = r.add_samples(std::slice::from_ref(s));
            add_ns += ta.elapsed().as_nanos();
            r.render_into(&mut display, d);
        }
        let tiles = r.coverage_tile_count();
        let fin = r.finish();
        let total = t0.elapsed();
        eprintln!(
            "HOST CPU {label}: {n} samples, path {len:.0} px, {} dabs, {} coverage tiles: total {:.1} ms = {:.1} us/sample \
             (add_samples only {:.1} us/sample, incl. per-sample render_into + finish)",
            fin.dab_count,
            tiles,
            total.as_secs_f64() * 1e3,
            total.as_secs_f64() * 1e6 / n as f64,
            add_ns as f64 / 1e3 / n as f64,
        );
        assert!(fin.dab_count > 0);
    }
}
