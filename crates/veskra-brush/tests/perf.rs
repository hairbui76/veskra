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
    preset.dynamics.pressure.size.min = 1.0;
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

/// ADR-019 acceptance number: dab throughput of a 64-px image tip vs the 64-px round tip (and the
/// textured / wet / color-jitter general path), same path, same spacing, host CPU, release build.
#[test]
#[ignore = "performance probe; run in release with --ignored --nocapture"]
fn perf_dabs_per_second_image_tip_vs_round() {
    use veskra_brush::{BrushAsset, Jitter, ResolvedAssets, Texture, TextureMode, TextureSource, Tip, TipShape};
    let size = 2048u32;
    let clip = RectI::from_size(size, size);
    let n = 4000;
    let samples: Vec<StrokeSample> = (0..n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let a = t * std::f64::consts::TAU;
            let x = 1024.0 + 700.0 * a.sin();
            let y = 1024.0 + 500.0 * (2.0 * a).sin();
            StrokeSample::confirmed(x as f32, y as f32, i as u64 * 4_166_667, 0.8)
        })
        .collect();
    let mask: Vec<u8> = (0..64 * 64)
        .map(|i| {
            let (x, y) = ((i % 64) as f64 - 31.5, (i / 64) as f64 - 31.5);
            if x * x + y * y < 30.0 * 30.0 { (128 + (i * 37 % 127)) as u8 } else { 0 }
        })
        .collect();
    let tip = std::sync::Arc::new(BrushAsset::from_mask(64, 64, mask.clone()).unwrap());
    let tex = std::sync::Arc::new(BrushAsset::from_mask(64, 64, mask).unwrap());
    let mut round = builtin_preset("round.hard").unwrap();
    round.size_px = 64.0;
    round.dynamics.pressure.size.enabled = false;
    round.spacing = 0.1;
    round.vector_eligible = false;
    let image = veskra_brush::BrushPreset {
        tip: Tip { shape: TipShape::Image, image: Some(tip.hash().into()), ..Tip::default() },
        ..round.clone()
    };
    let rich = veskra_brush::BrushPreset {
        texture: Some(Texture {
            source: TextureSource::Image,
            image: Some(tex.hash().into()),
            scale: 1.0,
            depth: 0.6,
            invert: false,
            mode: TextureMode::Fixed,
        }),
        wet_edges: 0.4,
        jitter: Jitter { angle: 1.0, hue: 0.1, ..Jitter::default() },
        ..image.clone()
    };
    let assets = ResolvedAssets { tip: Some(tip), texture: Some(tex) };
    for (label, p, a) in [
        ("round 64px (schema-2 path)", round, ResolvedAssets::default()),
        ("image tip 64px", image, assets.clone()),
        ("image tip 64px + image texture + wet edges + color/angle jitter", rich, assets),
    ] {
        // Warm up once, then time the best of three.
        let mut best = f64::MAX;
        let mut dabs = 0;
        for _ in 0..4 {
            let t0 = Instant::now();
            let mut r = StrokeRenderer::begin_resolved(
                p.clone(),
                a.clone(),
                INK,
                1,
                TileGrid::new(),
                StrokeContext::new(StrokeMode::Paint, clip),
            );
            for c in samples.chunks(4) {
                r.add_samples(c);
            }
            r.flush();
            let el = t0.elapsed().as_secs_f64();
            dabs = r.stats().dabs;
            best = best.min(el);
        }
        eprintln!(
            "HOST CPU {label}: {dabs} dabs in {:.1} ms = {:.0} dabs/s (rasterization only, best of 4)",
            best * 1e3,
            dabs as f64 / best
        );
    }
}
