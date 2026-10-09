//! PNT-001 determinism, batching independence, STY-007 confirmed-only storage, stabilizer catch-up.

mod common;

use common::*;
use veskra_brush::{StrokeMode, StrokeRenderer, builtin_preset, builtin_presets};
use veskra_core::{RectI, SampleFlags, StrokeSample};
use veskra_raster::TileGrid;
use veskra_testkit::Rng;

fn base_for(eraser: bool) -> TileGrid {
    if eraser { filled_base([30_000, 12_000, 6_000, 40_000]) } else { TileGrid::new() }
}

#[test]
fn pnt001_bit_identical_across_runs_and_batching() {
    let samples = s_curve(150, 11);
    let mut rng = Rng::new(99);
    let random_splits: Vec<usize> = (0..64).map(|_| 1 + rng.below(9) as usize).collect();
    for p in builtin_presets() {
        let base = base_for(p.eraser);
        let one = render(&p, &samples, 5, base.clone(), ctx(StrokeMode::Paint), &[usize::MAX]);
        let again = render(&p, &samples, 5, base.clone(), ctx(StrokeMode::Paint), &[usize::MAX]);
        let per_sample = render(&p, &samples, 5, base.clone(), ctx(StrokeMode::Paint), &[1]);
        let random = render(&p, &samples, 5, base.clone(), ctx(StrokeMode::Paint), &random_splits);
        assert!(one.dab_count > 0, "{}", p.id);
        assert!(one.tiles == again.tiles, "{}: not deterministic across runs", p.id);
        assert!(one.tiles == per_sample.tiles, "{}: per-sample batching differs", p.id);
        assert!(one.tiles == random.tiles, "{}: random batching differs", p.id);
        assert_eq!(one.dab_count, random.dab_count);
        assert_eq!(one.bounds, random.bounds);
        assert_eq!(one.geometry, random.geometry, "{}: geometry depends on batching", p.id);
        assert!(premul_invariant_holds(&one.tiles), "{}", p.id);
    }
}

#[test]
fn live_render_matches_finish_and_seed_changes_grain() {
    let samples = s_curve(120, 3);
    let p = builtin_preset("charcoal").unwrap();
    let mut r = StrokeRenderer::begin(p.clone(), INK, 1, TileGrid::new(), ctx(StrokeMode::Paint));
    let mut live = TileGrid::new();
    for chunk in samples.chunks(7) {
        let d = r.add_samples(chunk);
        r.render_into(&mut live, d);
    }
    let d = r.flush();
    r.render_into(&mut live, d);
    let fin = r.finish();
    assert!(live == fin.tiles, "incremental dirty-rect rendering must equal the final layer");
    let other_seed = render(&p, &samples, 2, TileGrid::new(), ctx(StrokeMode::Paint), &[usize::MAX]);
    assert!(other_seed.tiles != fin.tiles, "grain must depend on the seed");
    // Non-grain presets do not depend on the seed.
    let pen = builtin_preset("pen.technical").unwrap();
    let a = render(&pen, &samples, 1, TileGrid::new(), ctx(StrokeMode::Paint), &[5]);
    let b = render(&pen, &samples, 2, TileGrid::new(), ctx(StrokeMode::Paint), &[5]);
    assert!(a.tiles == b.tiles);
}

#[test]
fn sty007_predicted_never_stored_and_preview_is_pure() {
    let samples = s_curve(80, 4);
    let p = builtin_preset("pen.brush").unwrap();
    let reference = render(&p, &samples, 9, TileGrid::new(), ctx(StrokeMode::Paint), &[4]);

    let mut r = StrokeRenderer::begin(p.clone(), INK, 9, TileGrid::new(), ctx(StrokeMode::Paint));
    let mut predicted_total = 0;
    for (k, chunk) in samples.chunks(4).enumerate() {
        // Interleave predicted samples in the confirmed stream: they must be skipped.
        let mut batch: Vec<StrokeSample> = chunk.to_vec();
        let mut pred = *chunk.last().unwrap();
        pred.x += 15.0;
        pred.flags = SampleFlags::PREDICTED;
        batch.insert(1.min(batch.len()), pred);
        predicted_total += 1;
        r.add_samples(&batch);

        // Preview with predicted samples must not change the confirmed state.
        let before = {
            let mut t = TileGrid::new();
            r.render_into(&mut t, r.bounds());
            t
        };
        let cov_before = r.coverage_tile_count();
        // Precondition: the preview target already holds base ⊕ confirmed stroke.
        let mut preview = before.clone();
        let pr = r.prediction_preview(&[pred, StrokeSample { x: pred.x + 10.0, ..pred }], &mut preview);
        // Only the predicted-dab area is recomposited: outside `pr` the preview is untouched.
        let probe = r.bounds().union(&pr);
        for y in probe.y0..probe.y1 {
            for x in probe.x0..probe.x1 {
                if !pr.contains(x, y) {
                    assert_eq!(preview.get_pixel(x, y), before.get_pixel(x, y), "pixel outside preview rect changed");
                }
            }
        }
        if k > 2 {
            assert!(!pr.is_empty() && preview != before, "preview must show predicted dabs");
        }
        let after = {
            let mut t = TileGrid::new();
            r.render_into(&mut t, r.bounds());
            t
        };
        assert!(before == after);
        assert_eq!(cov_before, r.coverage_tile_count());
    }
    let stats = r.stats();
    assert_eq!(stats.predicted_rejected, predicted_total);
    let fin = r.finish();
    assert_eq!(fin.confirmed_samples, samples);
    assert!(fin.confirmed_samples.iter().all(|s| s.is_confirmed()));
    assert!(fin.tiles == reference.tiles, "predicted samples must not affect committed pixels");
}

#[test]
fn invalid_samples_rejected_and_counted() {
    let p = builtin_preset("round.hard").unwrap();
    let mut r = StrokeRenderer::begin(p.clone(), INK, 0, TileGrid::new(), ctx(StrokeMode::Paint));
    let good = StrokeSample::confirmed(10.0, 10.0, 100, 0.5);
    let mut nan = good;
    nan.x = f32::NAN;
    let mut unconfirmed = good;
    unconfirmed.flags = SampleFlags::empty();
    let far = StrokeSample::confirmed(5.0e6, 0.0, 200, 0.5);
    let back_in_time = StrokeSample::confirmed(12.0, 10.0, 50, 0.5);
    r.add_samples(&[good, nan, unconfirmed, far, back_in_time]);
    assert_eq!(r.stats().confirmed, 1);
    assert_eq!(r.stats().invalid_rejected, 4);
    r.flush();
    r.add_samples(&[StrokeSample::confirmed(20.0, 10.0, 300, 0.5)]);
    assert_eq!(r.stats().invalid_rejected, 5);
    assert_eq!(r.finish().dab_count, 1, "single sample = one dab");
}

#[test]
fn stabilized_path_reaches_last_sample_and_is_batch_independent() {
    let mut p = builtin_preset("pen.technical").unwrap();
    p.stabilization = 1.0; // 40 px rope
    let samples = line((20.0, 64.0), (230.0, 64.0), 60, 1.0, 1.0);
    let mut r = StrokeRenderer::begin(p.clone(), INK, 0, TileGrid::new(), ctx(StrokeMode::Paint));
    r.add_samples(&samples);
    // While live, the stabilized point lags behind the pen by the rope.
    assert_eq!(r.coverage(229, 64), 0, "stabilizer should lag before flush");
    r.flush();
    assert!(r.coverage(229, 64) > 60_000, "flush must reach the last confirmed sample");
    let a = r.finish();
    let b = render(&p, &samples, 0, TileGrid::new(), ctx(StrokeMode::Paint), &[3, 1, 7]);
    assert!(a.tiles == b.tiles);
    // Stabilization is paint-path only: recorded samples are the raw confirmed ones.
    assert_eq!(a.confirmed_samples, samples);
}

#[test]
fn base_untouched_and_untouched_tiles_shared() {
    let base = filled_base([0, 0, 0, 65535]);
    let snapshot = base.clone();
    let p = builtin_preset("round.hard").unwrap();
    let samples = line((10.0, 10.0), (40.0, 20.0), 10, 0.8, 0.8);
    let fin = render(&p, &samples, 0, base.clone(), ctx(StrokeMode::Paint), &[2]);
    assert!(base == snapshot);
    assert!(fin.bounds.x1 <= 64 && fin.bounds.y1 <= 64);
    let mut unshared = 0;
    for (c, _) in base.iter_tiles() {
        if c.rect().intersect(&fin.bounds).is_empty() {
            assert!(fin.tiles.shares_tile_with(&base, c), "untouched tile {c:?} must stay shared");
        } else if !fin.tiles.shares_tile_with(&base, c) {
            unshared += 1;
        }
    }
    assert!(unshared >= 1);
    // Nothing outside the clip.
    let mut cx = ctx(StrokeMode::Paint);
    cx.clip = RectI::new(0, 0, 32, 32);
    let clipped = render(&p, &samples, 0, TileGrid::new(), cx, &[2]);
    assert!(clipped.tiles.content_bounds().x1 <= 32 && clipped.tiles.content_bounds().y1 <= 32);
}
