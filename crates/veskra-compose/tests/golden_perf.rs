//! 12-node "ink / flats / shadow" golden (DOC-01, PNT-006) and host CPU timing (ignored).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use veskra_compose::{ComposeOptions, composite_grid, composite_rect, render_display_rgba8};
use veskra_core::{BlendMode, ColorRgba, RectI};
use veskra_doc::{Document, MaskGrid, TileGrid};
use veskra_raster::{Tile, TileCoord};

const W: u32 = 256;
const H: u32 = 192;

/// Anti-aliased coverage of a shape given a signed distance (negative inside), 1px ramp.
fn cov(sd: f64) -> f64 {
    (0.5 - sd).clamp(0.0, 1.0)
}

fn shape_grid(c: ColorRgba, sd: impl Fn(f64, f64) -> f64) -> TileGrid {
    let mut g = TileGrid::new();
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let a = cov(sd(x as f64 + 0.5, y as f64 + 0.5));
            if a > 0.0 {
                g.put_pixel(x, y, pxf(c.with_alpha((c.a as f64 * a) as f32)));
            }
        }
    }
    g
}

fn circle(cx: f64, cy: f64, r: f64) -> impl Fn(f64, f64) -> f64 {
    move |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
}

fn rect_sd(x0: f64, y0: f64, x1: f64, y1: f64) -> impl Fn(f64, f64) -> f64 {
    move |x, y| (x0 - x).max(x - x1).max(y0 - y).max(y - y1)
}

/// Thick line segment (capsule) distance.
fn capsule(ax: f64, ay: f64, bx: f64, by: f64, r: f64) -> impl Fn(f64, f64) -> f64 {
    move |x, y| {
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        ((x - ax - t * dx).powi(2) + (y - ay - t * dy).powi(2)).sqrt() - r
    }
}

fn hex(s: &str) -> ColorRgba {
    ColorRgba::from_hex(s).unwrap()
}

fn fixture() -> Document {
    let mut d = doc(W, H);
    let full = d.bounds();
    // 1 paper
    add_fill(&mut d, 1, None, full, px8(250, 246, 236, 255), props("paper", BlendMode::Normal, 1.0));
    // 2 flats (isolated group) with 3 children
    add_group(&mut d, 2, None, false, props("flats", BlendMode::Normal, 1.0));
    add_grid(
        &mut d,
        3,
        Some(2),
        shape_grid(hex("#F2C6A0"), circle(110.0, 80.0, 52.0)),
        None,
        props("skin", BlendMode::Normal, 1.0),
    );
    let mut shirt_mask = MaskGrid::new_empty();
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            shirt_mask.set(x, y, (x as u32 * 255 / (W - 1)) as u8); // horizontal fade
        }
    }
    add_grid(
        &mut d,
        4,
        Some(2),
        shape_grid(hex("#3B6FB6"), rect_sd(40.0, 120.0, 200.0, 200.0)),
        Some(shirt_mask),
        props("shirt", BlendMode::Normal, 1.0),
    );
    add_grid(
        &mut d,
        5,
        Some(2),
        shape_grid(hex("#4A2C1ACC"), |x, y| circle(110.0, 50.0, 40.0)(x, y * 1.6 - 30.0)),
        None,
        props("hair", BlendMode::Normal, 0.9),
    );
    // 6 shadow (Multiply, clipped to the flats group), 7 highlight (Screen, clipped)
    add_grid(
        &mut d,
        6,
        None,
        shape_grid(hex("#8A7FB0"), circle(150.0, 110.0, 60.0)),
        None,
        props("shadow", BlendMode::Multiply, 0.6),
    );
    set_clip(&mut d, 6, true);
    add_grid(
        &mut d,
        7,
        None,
        shape_grid(hex("#FFF4C0"), circle(85.0, 60.0, 22.0)),
        None,
        props("highlight", BlendMode::Screen, 0.5),
    );
    set_clip(&mut d, 7, true);
    // 8 fx pass-through group: 9 overlay gradient, 10 masked glow
    add_group(&mut d, 8, None, true, props("fx", BlendMode::Normal, 0.8));
    let mut grad = TileGrid::new();
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let t = x as f32 / (W - 1) as f32;
            grad.put_pixel(x, y, pxf(ColorRgba::new(t, 0.2, 1.0 - t, 0.5)));
        }
    }
    add_grid(&mut d, 9, Some(8), grad, None, props("tint", BlendMode::Overlay, 1.0));
    let mut glow_mask = MaskGrid::new_empty();
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            glow_mask.set(x, y, if (x / 16 + y / 16) % 2 == 0 { 255 } else { 96 });
        }
    }
    add_grid(
        &mut d,
        10,
        Some(8),
        shape_grid(hex("#FFB070"), circle(200.0, 40.0, 30.0)),
        Some(glow_mask),
        props("glow", BlendMode::Screen, 1.0),
    );
    // 11 ink
    let mut ink = TileGrid::new();
    for seg in [
        (60.0, 80.0, 160.0, 82.0, 2.0),
        (110.0, 28.0, 64.0, 130.0, 1.5),
        (110.0, 28.0, 158.0, 130.0, 1.0),
        (40.0, 120.0, 200.0, 121.0, 2.5),
    ] {
        let g = shape_grid(hex("#1A1A22"), capsule(seg.0, seg.1, seg.2, seg.3, seg.4));
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                let p = g.get_pixel(x, y);
                if p[3] > 0 {
                    ink.put_pixel(x, y, veskra_raster::ops::src_over(ink.get_pixel(x, y), p));
                }
            }
        }
    }
    add_grid(&mut d, 11, None, ink, None, props("ink", BlendMode::Normal, 1.0));
    // 12 hidden sketch (must not appear)
    add_fill(&mut d, 12, None, full, px8(255, 0, 0, 255), props("sketch", BlendMode::Normal, 1.0));
    set_visible(&mut d, 12, false);
    d
}

#[test]
fn golden_ink_flats_shadow_12_layers() {
    let d = fixture();
    assert_eq!(d.layer_count(), 12);
    let r = d.bounds();
    let px = composite_rect(&d, r, &ComposeOptions::default());
    assert_premul(&px);
    assert_eq!(px, composite_rect(&d, r, &ComposeOptions::default()), "deterministic");
    assert_eq!(composite_grid(&d, &ComposeOptions::default()).read_rect(r), px);
    let mut rgba = vec![0u8; px.len()];
    veskra_color::convert::premul16_to_straight8(&px, &mut rgba);
    // Spot check independent of the golden file: corner = paper, Overlay tint (pass-through
    // group at 0.8), hidden red sketch absent.
    let paper = to_f(px8(250, 246, 236, 255));
    let tint = to_f(pxf(ColorRgba::new(0.0, 0.2, 1.0, 0.5)));
    let after = ref_blend(paper, tint, BlendMode::Overlay);
    let k = 0.8f32 as f64;
    let want: [f64; 4] = std::array::from_fn(|i| paper[i] + (after[i] - paper[i]) * k);
    assert_close(at(&px, r, 0, 0), q16(want), 1, "corner");
    assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
    veskra_testkit::golden::assert_matches("compose_ink_flats_shadow", W, H, &rgba, 0, 0);
}

/// HOST CPU timing (not device): 2048² document, 12 half-filled layers.
/// Run: `cargo test -p veskra-compose --release --test golden_perf -- --ignored --nocapture`.
#[test]
#[ignore]
fn host_timing_2048_twelve_half_filled_layers() {
    // `VESKRA_PERF_N` shrinks the canvas for profiling under valgrind (default 2048).
    let n: u32 = std::env::var("VESKRA_PERF_N").ok().and_then(|v| v.parse().ok()).unwrap_or(2048);
    let mut d = doc(n, n);
    let tiles = (n / 64) as i32;
    for l in 0..12u128 {
        let c = ColorRgba::new((l as f32 * 0.07) % 1.0, 0.5, 1.0 - l as f32 * 0.05, 0.4 + (l % 3) as f32 * 0.25);
        let tile = Arc::new(Tile::new_filled(pxf(c)));
        let mut g = TileGrid::new();
        for ty in 0..tiles {
            for tx in 0..tiles {
                // Half of the canvas per layer, at a per-layer diagonal offset.
                if (tx + ty + l as i32 * 3) % tiles < tiles / 2 {
                    g.set_tile(TileCoord::new(tx, ty), tile.clone());
                }
            }
        }
        let filled = g.tile_count();
        let mode = BlendMode::ALL[l as usize % 4];
        add_grid(&mut d, l + 1, None, g, None, props("l", mode, 0.9));
        eprintln!("layer {l}: {filled} / {} tiles, {mode:?}", tiles * tiles);
    }
    let o = ComposeOptions::default();
    let _ = composite_rect(&d, RectI::new(0, 0, 64, 64), &o); // warm-up
    let reps: u32 = std::env::var("VESKRA_PERF_REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    let t = Instant::now();
    for _ in 0..reps {
        std::hint::black_box(composite_grid(&d, &o));
    }
    let full = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
    let dirty = RectI::new(700, 900, 1212, 1412); // unaligned 512×512
    let t = Instant::now();
    for _ in 0..reps * 4 {
        std::hint::black_box(composite_rect(&d, dirty, &o));
    }
    let rect_ms = t.elapsed().as_secs_f64() * 1000.0 / (reps * 4) as f64;
    let mut buf = vec![0u8; 512 * 512 * 4];
    let t = Instant::now();
    for _ in 0..reps * 4 {
        render_display_rgba8(&d, dirty, &o, &mut buf).unwrap();
    }
    let disp_ms = t.elapsed().as_secs_f64() * 1000.0 / (reps * 4) as f64;
    eprintln!(
        "HOST CPU: full 2048² composite_grid {full:.1} ms; 512² dirty composite_rect {rect_ms:.2} ms; 512² render_display_rgba8 {disp_ms:.2} ms"
    );
}
