//! Several strokes of one gesture (symmetry replicas): `composite_layered` must equal committing
//! the strokes one after another, for every mode, and its prediction branch must be display-only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use veskra_brush::{LayeredPart, StrokeContext, StrokeMode, StrokeRenderer, builtin_preset, composite_layered};
use veskra_core::{ColorRgba, RectI, SampleFlags, StrokeSample};
use veskra_raster::{MaskGrid, TileGrid};

/// Three overlapping sample paths (an S-curve, a crossing diagonal and a spiral).
fn paths() -> Vec<Vec<StrokeSample>> {
    vec![s_curve(90, 3), line((30.0, 110.0), (230.0, 15.0), 40, 0.3, 1.0), spiral(128.0, 64.0, 120, 5)]
}

fn begin(id: &str, base: &TileGrid, ctx: &StrokeContext, samples: &[StrokeSample]) -> StrokeRenderer {
    let mut r = StrokeRenderer::begin(builtin_preset(id).unwrap(), INK, 7, base.clone(), ctx.clone());
    for chunk in samples.chunks(7) {
        r.add_samples(chunk);
    }
    r
}

/// Reference: each stroke finished on top of the previous stroke's result.
fn sequential(id: &str, base: &TileGrid, ctx: &StrokeContext) -> TileGrid {
    let mut tiles = base.clone();
    for p in paths() {
        tiles = begin(id, &tiles, ctx, &p).finish().tiles;
    }
    tiles
}

fn layered_result(id: &str, base: &TileGrid, ctx: &StrokeContext) -> TileGrid {
    let mut parts: Vec<StrokeRenderer> = paths().iter().map(|p| begin(id, base, ctx, p)).collect();
    let mut bounds = RectI::EMPTY;
    for p in &mut parts {
        p.flush();
        bounds = bounds.union(&p.bounds());
    }
    let refs: Vec<LayeredPart<'_>> = parts.iter().map(|p| LayeredPart { stroke: p, predicted: None }).collect();
    let mut out = base.clone();
    composite_layered(base, &refs, &mut out, bounds);
    // `finish_layered` reports the same stroke as `finish`, minus the per-stroke composite.
    for (p, path) in parts.into_iter().zip(paths()) {
        let f = p.finish_layered();
        assert_eq!(f.confirmed_samples, path);
        assert_eq!(&f.tiles, base);
    }
    out
}

#[test]
fn layered_equals_sequential_commit_for_every_mode() {
    // Half-covered semi-transparent base so paint, erase and alpha lock all have work to do.
    let mut base = TileGrid::new();
    let wash = px16(ColorRgba::from_rgba8(230, 180, 40, 170));
    for y in 0..H as i32 {
        for x in 0..(W as i32 * 2 / 3) {
            base.put_pixel(x, y, wash);
        }
    }
    let mut selection = MaskGrid::new_rect(RectI::new(20, 10, 200, 100));
    selection.set(60, 60, 128);
    for id in ["round.hard", "marker", "round.soft", "charcoal", "eraser.soft"] {
        for (alpha_lock, with_selection, mode) in [
            (false, false, StrokeMode::Paint),
            (true, false, StrokeMode::Paint),
            (false, true, StrokeMode::Paint),
            (false, false, StrokeMode::Erase),
            (true, true, StrokeMode::Erase),
        ] {
            let mut cx = ctx(mode);
            cx.alpha_lock = alpha_lock;
            cx.selection = with_selection.then(|| selection.clone());
            let want = sequential(id, &base, &cx);
            let got = layered_result(id, &base, &cx);
            assert!(premul_invariant_holds(&got));
            assert_eq!(got, want, "{id} alpha_lock={alpha_lock} selection={with_selection} {mode:?}");
        }
    }
}

#[test]
fn layered_single_part_equals_render_into_and_restores_uncovered_pixels() {
    let base = filled_base(px16(ColorRgba::from_rgba8(10, 200, 90, 255)));
    let cx = ctx(StrokeMode::Paint);
    let r = begin("pen.brush", &base, &cx, &s_curve(80, 2));
    let mut a = base.clone();
    r.render_into(&mut a, canvas());
    // Start from garbage in the target: the whole region must be recomputed from the base.
    let mut b = filled_base([65535, 0, 0, 65535]);
    composite_layered(&base, &[LayeredPart { stroke: &r, predicted: None }], &mut b, canvas());
    assert_eq!(a, b);
    // An empty part list and an empty region write nothing.
    let mut c = base.clone();
    composite_layered(&base, &[], &mut c, canvas());
    composite_layered(&base, &[LayeredPart { stroke: &r, predicted: None }], &mut c, RectI::EMPTY);
    assert_eq!(c, base);
}

#[test]
fn predicted_coverage_is_display_only_and_matches_prediction_preview() {
    let base = TileGrid::new();
    let cx = ctx(StrokeMode::Paint);
    let confirmed = line((20.0, 20.0), (120.0, 60.0), 20, 0.6, 0.9);
    let predicted: Vec<StrokeSample> = line((120.0, 60.0), (200.0, 100.0), 6, 0.9, 0.9)
        .into_iter()
        .map(|mut s| {
            s.t_ns += 1_000_000_000;
            s.flags = SampleFlags::PREDICTED;
            s
        })
        .collect();
    let r = begin("round.hard", &base, &cx, &confirmed);
    let mut live = base.clone();
    r.render_into(&mut live, canvas());

    let mut want = live.clone();
    let want_rect = r.prediction_preview(&predicted, &mut want);
    let branch = r.predicted_coverage(&predicted);
    assert_eq!(branch.dirty(), want_rect);
    let mut got = live.clone();
    composite_layered(&base, &[LayeredPart { stroke: &r, predicted: Some(&branch) }], &mut got, branch.dirty());
    assert_eq!(got, want);
    assert_ne!(got, live, "the predicted tail is visible");
    // The renderer itself is unchanged: finishing yields only the confirmed stroke.
    let f = r.finish();
    assert_eq!(f.confirmed_samples, confirmed);
    assert_eq!(f.tiles.get_pixel(190, 95), [0; 4], "no predicted dab was committed");
}
