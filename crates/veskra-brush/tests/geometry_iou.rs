//! VEC-01/02 geometry half: the vector outline of a vector-eligible stroke must agree with its
//! raster coverage. Outline rasterized at pixel centers (nonzero) vs stroke coverage > 50 %:
//! IoU ≥ 0.9 per fixture. Also checks the centerline fit bound and eligibility gating.

mod common;

use common::*;
use veskra_brush::{CENTERLINE_TOLERANCE, StrokeMode, StrokeRenderer, builtin_preset, builtin_presets};
use veskra_core::{PointF, StrokeSample};
use veskra_raster::TileGrid;

fn iou(preset_id: &str, samples: &[StrokeSample]) -> (f64, usize, f64) {
    let p = builtin_preset(preset_id).expect("builtin");
    let mut r = StrokeRenderer::begin(p.clone(), INK, 0, TileGrid::new(), ctx(StrokeMode::Paint));
    r.add_samples(samples);
    r.flush();
    let raster: Vec<bool> =
        (0..H as i32).flat_map(|y| (0..W as i32).map(move |x| (x, y))).map(|(x, y)| r.coverage(x, y) > 32767).collect();
    let fin = r.finish();
    let g = fin.geometry.expect("vector-eligible preset must produce geometry");
    assert!(g.fit_error <= CENTERLINE_TOLERANCE + 1e-9, "{preset_id}: fit error {}", g.fit_error);
    let polys: Vec<Vec<PointF>> = g.outline.flatten(0.05);
    let vector = rasterize_nonzero(&polys, W, H);
    let inter = raster.iter().zip(&vector).filter(|(a, b)| **a && **b).count();
    let union = raster.iter().zip(&vector).filter(|(a, b)| **a || **b).count();
    (inter as f64 / union.max(1) as f64, union, g.fit_error)
}

#[test]
fn vector_outline_matches_raster_coverage() {
    let fixtures: Vec<(&str, Vec<StrokeSample>)> = vec![
        ("s_curve", s_curve(120, 2026)),
        ("taper_line", line((20.0, 30.0), (230.0, 100.0), 50, 0.05, 1.0)),
        ("spiral", spiral(128.0, 64.0, 200, 5)),
        ("tap", line((60.0, 60.0), (60.0, 60.0), 1, 0.9, 0.9)),
    ];
    let mut worst = 1.0f64;
    for id in ["pencil.smooth", "pen.technical", "pen.brush", "marker", "round.hard"] {
        for (name, samples) in &fixtures {
            let (v, union, fit) = iou(id, samples);
            eprintln!("IoU {id:<14} {name:<10} = {v:.4} (union {union} px, centerline fit error {fit:.3} px)");
            worst = worst.min(v);
            assert!(v >= 0.9, "{id}/{name}: IoU {v:.4} < 0.9");
        }
    }
    eprintln!("worst IoU = {worst:.4}");
}

#[test]
fn geometry_only_for_vector_eligible_paint() {
    let samples = s_curve(60, 1);
    for p in builtin_presets() {
        let fin = render(&p, &samples, 0, filled_base([0, 0, 0, 65535]), ctx(StrokeMode::Paint), &[4]);
        assert_eq!(fin.geometry.is_some(), p.vector_eligible, "{}", p.id);
    }
    let pen = builtin_preset("pen.technical").unwrap();
    let erased = render(&pen, &samples, 0, filled_base([0, 0, 0, 65535]), ctx(StrokeMode::Erase), &[4]);
    assert!(erased.geometry.is_none(), "erase mode never yields vector geometry");
}

/// QA-D3: a stroke reaching far beyond the canvas must stay cheap and lose only its vector form.
#[test]
fn far_off_canvas_stroke_is_cheap_and_raster_only() {
    use veskra_brush::{StrokeContext, StrokeMode, StrokeRenderer, builtin_preset};
    use veskra_core::{ColorRgba, RectI, StrokeSample};
    use veskra_raster::TileGrid;
    let p = builtin_preset("pen.technical").unwrap();
    let mut r = StrokeRenderer::begin(
        p,
        ColorRgba::BLACK,
        1,
        TileGrid::new(),
        StrokeContext::new(StrokeMode::Paint, RectI::new(0, 0, 128, 96)),
    );
    let t = std::time::Instant::now();
    r.add_samples(&[StrokeSample::confirmed(10.0, 10.0, 0, 0.5), StrokeSample::confirmed(1.0e6, 10.0, 1000, 0.5)]);
    let fin = r.finish();
    assert!(fin.geometry.is_none(), "far off-canvas stroke must not get a vector form");
    assert!(fin.tiles.get_pixel(60, 10)[3] > 0, "visible part is still painted");
    assert!(t.elapsed().as_secs_f64() < 1.0, "took {:?}", t.elapsed());
}
