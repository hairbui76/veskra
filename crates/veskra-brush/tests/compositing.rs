//! PNT-004 (eraser fringe), alpha lock, selection, Max vs BuildUp accumulation, opacity.

mod common;

use common::*;
use veskra_brush::{StrokeContext, StrokeMode, StrokeRenderer, builtin_preset};
use veskra_core::{ColorRgba, RectI};
use veskra_raster::{MaskGrid, TileGrid};

#[test]
fn pnt004_eraser_leaves_no_dark_or_colored_fringe() {
    let originals: [[u8; 4]; 4] = [[200, 50, 30, 128], [20, 180, 90, 200], [250, 250, 250, 40], [10, 20, 240, 7]];
    let mut worst = 0u8;
    let mut checked = 0usize;
    for orig in originals {
        let mut p16 = [0u16; 4];
        veskra_color::convert::straight8_to_premul16(&orig, &mut p16);
        for id in ["eraser.soft", "eraser.hard"] {
            let p = builtin_preset(id).unwrap();
            let samples = s_curve(100, 8);
            let fin = render(&p, &samples, 0, filled_base(p16), ctx(StrokeMode::Paint), &[4]);
            assert!(premul_invariant_holds(&fin.tiles));
            let rgba = straight8(&fin.tiles, canvas());
            let mut partially_erased = 0;
            for px in rgba.chunks_exact(4) {
                if px[3] == 0 {
                    continue;
                }
                assert!(px[3] <= orig[3], "eraser must never add alpha");
                if px[3] < orig[3] {
                    partially_erased += 1;
                }
                for c in 0..3 {
                    let d = px[c].abs_diff(orig[c]);
                    worst = worst.max(d);
                    assert!(d <= 1, "{id} over {orig:?}: straight color drifted to {px:?}");
                }
                checked += 1;
            }
            assert!(partially_erased > 100, "{id}: the test must exercise partial erasure");
        }
    }
    eprintln!("PNT-004: {checked} remaining pixels checked, worst straight-color deviation {worst} LSB");
}

#[test]
fn alpha_lock_preserves_alpha_and_transparency() {
    // Left half semitransparent red, right half transparent.
    let mut base = TileGrid::new();
    let red = px16(ColorRgba::from_rgba8(220, 20, 20, 150));
    for y in 0..H as i32 {
        for x in 0..(W as i32 / 2) {
            base.put_pixel(x, y, red);
        }
    }
    let p = builtin_preset("round.hard").unwrap();
    let mut cx = ctx(StrokeMode::Paint);
    cx.alpha_lock = true;
    let fin = render(&p, &s_curve(100, 1), 0, base.clone(), cx, &[5]);
    let mut recolored = 0;
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let (b, o) = (base.get_pixel(x, y), fin.tiles.get_pixel(x, y));
            assert_eq!(o[3], b[3], "alpha changed at {x},{y}");
            if b[3] == 0 {
                assert_eq!(o, [0; 4]);
            } else if o != b {
                recolored += 1;
            }
        }
    }
    assert!(recolored > 500);
    // Erase + alpha lock is a no-op.
    let mut ce = ctx(StrokeMode::Erase);
    ce.alpha_lock = true;
    let erased = render(&p, &s_curve(100, 1), 0, base.clone(), ce, &[5]);
    assert!(erased.tiles == base);
}

#[test]
fn selection_masks_coverage() {
    let sel = MaskGrid::new_rect(RectI::new(0, 0, 128, 128));
    let p = builtin_preset("marker").unwrap();
    let mut cx = ctx(StrokeMode::Paint);
    cx.selection = Some(sel);
    let fin = render(&p, &s_curve(100, 1), 0, TileGrid::new(), cx, &[5]);
    let b = fin.tiles.content_bounds();
    assert!(!b.is_empty() && b.x1 <= 128, "{b:?}");
}

#[test]
fn max_accumulation_no_self_overlap_darkening_buildup_builds() {
    // A stroke that retraces itself: Max keeps a single-pass value, BuildUp increases.
    let mut there = line((20.0, 64.0), (200.0, 64.0), 40, 0.6, 0.6);
    let back = line((200.0, 64.0), (20.0, 64.0), 40, 0.6, 0.6);
    let t0 = there.last().unwrap().t_ns;
    there.extend(back.into_iter().map(|mut s| {
        s.t_ns += t0 + 1;
        s
    }));
    let single = line((20.0, 64.0), (200.0, 64.0), 40, 0.6, 0.6);
    let marker = builtin_preset("marker").unwrap();
    let a = render(&marker, &single, 0, TileGrid::new(), ctx(StrokeMode::Paint), &[3]);
    let b = render(&marker, &there, 0, TileGrid::new(), ctx(StrokeMode::Paint), &[3]);
    assert_eq!(a.tiles.get_pixel(110, 64), b.tiles.get_pixel(110, 64), "Max must not darken on overlap");
    let alpha = a.tiles.get_pixel(110, 64)[3];
    assert!((alpha as i32 - (0.7f64 * 65535.0) as i32).abs() <= 2, "marker stroke opacity 0.7 → {alpha}");

    let air = builtin_preset("round.soft").unwrap();
    let a = render(&air, &single, 0, TileGrid::new(), ctx(StrokeMode::Paint), &[3]);
    let b = render(&air, &there, 0, TileGrid::new(), ctx(StrokeMode::Paint), &[3]);
    assert!(b.tiles.get_pixel(110, 64)[3] > a.tiles.get_pixel(110, 64)[3], "BuildUp must accumulate");
}

#[test]
fn overrides_and_invalid_arguments() {
    let p = builtin_preset("round.hard").unwrap();
    let mut cx = ctx(StrokeMode::Paint);
    cx.size_override = Some(4.0);
    cx.opacity_override = Some(0.5);
    let fin = render(&p, &line((20.0, 20.0), (100.0, 20.0), 20, 1.0, 1.0), 0, TileGrid::new(), cx, &[2]);
    let b = fin.tiles.content_bounds();
    assert!(b.height() <= 6, "size override applied: {b:?}");
    assert!((fin.tiles.get_pixel(60, 20)[3] as i32 - 32768).abs() <= 1);

    let bad_color = ColorRgba::new(2.0, 0.0, 0.0, 1.0);
    assert!(StrokeRenderer::try_begin(p.clone(), bad_color, 0, TileGrid::new(), ctx(StrokeMode::Paint)).is_err());
    let mut c = ctx(StrokeMode::Paint);
    c.size_override = Some(f32::NAN);
    assert!(StrokeRenderer::try_begin(p.clone(), INK, 0, TileGrid::new(), c).is_err());
    let mut c = ctx(StrokeMode::Paint);
    c.clip = RectI::EMPTY;
    assert!(StrokeRenderer::try_begin(p.clone(), INK, 0, TileGrid::new(), c).is_err());

    // The infallible contract entry point sanitizes instead of failing.
    let stroke = line((20.0, 20.0), (100.0, 20.0), 20, 1.0, 1.0);
    let mut c = ctx(StrokeMode::Paint);
    c.size_override = Some(f32::NAN);
    let fin = render(&p, &stroke, 0, TileGrid::new(), c, &[2]);
    assert!(fin.tiles.content_bounds().height() >= 19, "invalid override ignored");
    let none = render(&p, &stroke, 0, TileGrid::new(), StrokeContext::default(), &[2]);
    assert_eq!(none.tiles.tile_count(), 0, "default (empty) clip renders nothing");
    let mut broken = p.clone();
    broken.spacing = 0.0;
    broken.size_px = f32::INFINITY;
    let fin = render(&broken, &stroke, 0, TileGrid::new(), ctx(StrokeMode::Paint), &[2]);
    assert!(fin.dab_count > 0 && premul_invariant_holds(&fin.tiles));
}

#[test]
fn single_sample_tap_is_one_dab_and_erase_mode_forced_by_eraser_preset() {
    let p = builtin_preset("round.hard").unwrap();
    let tap = line((50.0, 50.0), (50.0, 50.0), 1, 1.0, 1.0);
    let fin = render(&p, &tap, 0, TileGrid::new(), ctx(StrokeMode::Paint), &[1]);
    assert_eq!(fin.dab_count, 1);
    let b = fin.tiles.content_bounds();
    assert!(b.width() >= 19 && b.width() <= 22, "{b:?}");
    let er = builtin_preset("eraser.hard").unwrap();
    let r = StrokeRenderer::begin(er.clone(), INK, 0, TileGrid::new(), ctx(StrokeMode::Paint));
    assert_eq!(r.mode(), StrokeMode::Erase);
}
