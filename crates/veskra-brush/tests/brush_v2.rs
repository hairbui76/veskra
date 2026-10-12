//! ADR-019 brush-v2 features: image tips and textures, jitter, tilt / velocity / pen-azimuth
//! dynamics, wet edges, color jitter, asset fallback — determinism and behaviour.

mod common;

use std::sync::Arc;

use common::*;
use veskra_brush::{
    Accumulation, AssetStore, BrushAsset, BrushPreset, DynamicsTarget, Jitter, ResolvedAssets, ResponseCurve,
    StrokeMode, StrokeRenderer, Texture, TextureMode, TextureSource, Tip, TipRotation, TipShape, builtin_presets,
};
use veskra_core::{ColorRgba, StrokeSample};
use veskra_raster::TileGrid;

fn ring_asset() -> Arc<BrushAsset> {
    // 64x64 ring with a notch: orientation-sensitive.
    let mut m = vec![0u8; 64 * 64];
    for y in 0..64 {
        for x in 0..64 {
            let (dx, dy) = (x as f64 - 31.5, y as f64 - 31.5);
            let d = (dx * dx + dy * dy).sqrt();
            if (18.0..30.0).contains(&d) && !(dy < 0.0 && dx.abs() < 6.0) {
                m[y * 64 + x] = 255;
            }
        }
    }
    Arc::new(BrushAsset::from_mask(64, 64, m).expect("ring mask"))
}

fn grain_asset() -> Arc<BrushAsset> {
    let m: Vec<u8> = (0..32 * 32).map(|i| if (i * 7919 % 13) < 6 { 40 } else { 230 }).collect();
    Arc::new(BrushAsset::from_mask(32, 32, m).expect("grain mask"))
}

fn image_preset(tip: &BrushAsset, tex: &BrushAsset) -> BrushPreset {
    BrushPreset {
        id: "test.image".into(),
        size_px: 24.0,
        spacing: 0.15,
        tip: Tip {
            shape: TipShape::Image,
            image: Some(tip.hash().into()),
            rotation: TipRotation::Direction,
            ..Tip::default()
        },
        texture: Some(Texture {
            source: TextureSource::Image,
            image: Some(tex.hash().into()),
            scale: 1.5,
            depth: 0.7,
            invert: false,
            mode: TextureMode::Fixed,
        }),
        jitter: Jitter { angle: 0.2, size: 0.3, scatter: 0.4, opacity: 0.2, ..Jitter::default() },
        ..BrushPreset::default()
    }
}

fn render_with(
    p: &BrushPreset,
    assets: &ResolvedAssets,
    samples: &[StrokeSample],
    seed: u64,
    color: ColorRgba,
    splits: &[usize],
) -> veskra_brush::FinishedStroke {
    let mut r =
        StrokeRenderer::begin_resolved(p.clone(), assets.clone(), color, seed, TileGrid::new(), ctx(StrokeMode::Paint));
    let mut i = 0;
    let mut k = 0;
    while i < samples.len() {
        let n = splits[k % splits.len()].max(1).min(samples.len() - i);
        r.add_samples(&samples[i..i + n]);
        i += n;
        k += 1;
    }
    r.finish()
}

#[test]
fn image_tip_and_texture_are_deterministic_and_batching_independent() {
    let (tip, tex) = (ring_asset(), grain_asset());
    let mut store = AssetStore::new();
    store.insert(tip.clone());
    store.insert(tex.clone());
    let p = image_preset(&tip, &tex);
    p.validate().unwrap();
    assert!(!p.satisfies_vector_rule());
    let assets = ResolvedAssets::resolve(&p, &[&store]);
    assert!(assets.tip.is_some() && assets.texture.is_some());
    let samples = s_curve(140, 4);
    let a = render_with(&p, &assets, &samples, 9, INK, &[usize::MAX]);
    let b = render_with(&p, &assets, &samples, 9, INK, &[1]);
    let c = render_with(&p, &assets, &samples, 9, INK, &[3, 1, 7]);
    assert!(a.dab_count > 10);
    assert!(a.tiles == b.tiles && a.tiles == c.tiles, "batching changed the pixels");
    assert!(premul_invariant_holds(&a.tiles));
    let other_seed = render_with(&p, &assets, &samples, 10, INK, &[usize::MAX]);
    assert!(other_seed.tiles != a.tiles, "jitter depends on the seed");
    // Without the assets: documented fallback (round tip + procedural grain), still deterministic.
    let none = ResolvedAssets::default();
    let r = StrokeRenderer::begin_resolved(p.clone(), none.clone(), INK, 9, TileGrid::new(), ctx(StrokeMode::Paint));
    assert_eq!(r.asset_fallbacks().len(), 2);
    let f1 = render_with(&p, &none, &samples, 9, INK, &[5]);
    let f2 = render_with(&p, &none, &samples, 9, INK, &[2]);
    assert!(f1.tiles == f2.tiles && f1.tiles != a.tiles);
}

#[test]
fn live_rendering_equals_the_finished_layer_for_every_builtin() {
    let samples = s_curve(100, 21);
    for p in builtin_presets() {
        let base = if p.eraser { filled_base([30_000, 12_000, 6_000, 40_000]) } else { TileGrid::new() };
        let mut r = StrokeRenderer::begin(p.clone(), INK, 3, base.clone(), ctx(StrokeMode::Paint));
        let mut live = base.clone();
        for chunk in samples.chunks(6) {
            let d = r.add_samples(chunk);
            r.render_into(&mut live, d);
            // A prediction preview never changes the confirmed state.
            let mut scratch = live.clone();
            r.prediction_preview(&s_curve(4, 99), &mut scratch);
        }
        let d = r.flush();
        r.render_into(&mut live, d);
        let fin = r.finish();
        assert!(live == fin.tiles, "{}: incremental rendering differs from the committed layer", p.id);
    }
}

#[test]
fn velocity_dynamics_thin_fast_strokes_and_tilt_needs_hardware() {
    let mut p = BrushPreset { id: "test.vel".into(), size_px: 20.0, ..BrushPreset::default() };
    p.dynamics.pressure.size.enabled = false;
    p.dynamics.velocity.size =
        DynamicsTarget::range(ResponseCurve::new_shape(vec![[0.0, 1.0], [1.0, 0.0]]).unwrap(), 0.2, 1.0);
    let line = |dt_ns: u64| -> Vec<StrokeSample> {
        (0..60).map(|i| StrokeSample::confirmed(20.0 + i as f32 * 3.0, 64.0, i * dt_ns, 1.0)).collect()
    };
    let width = |f: &veskra_brush::FinishedStroke| {
        let rgba = straight8(&f.tiles, canvas());
        (0..128).filter(|y| rgba[(y * 256 + 150) * 4 + 3] > 127).count()
    };
    let slow = render(&p, &line(20_000_000), 1, TileGrid::new(), ctx(StrokeMode::Paint), &[4]);
    let fast = render(&p, &line(500_000), 1, TileGrid::new(), ctx(StrokeMode::Paint), &[4]);
    assert!(width(&fast) + 6 < width(&slow), "fast {} slow {}", width(&fast), width(&slow));

    let mut t = BrushPreset { id: "test.tilt".into(), size_px: 10.0, ..BrushPreset::default() };
    t.dynamics.pressure.size.enabled = false;
    t.dynamics.tilt.size = DynamicsTarget::range(ResponseCurve::identity(), 1.0, 3.0);
    let mut tilted = line(4_000_000);
    for s in &mut tilted {
        s.tilt = Some(std::f32::consts::FRAC_PI_2);
    }
    let no_tilt = render(&t, &line(4_000_000), 1, TileGrid::new(), ctx(StrokeMode::Paint), &[4]);
    let with_tilt = render(&t, &tilted, 1, TileGrid::new(), ctx(StrokeMode::Paint), &[4]);
    assert!(width(&with_tilt) > 2 * width(&no_tilt));
    // Same preset without tilt dynamics equals the untilted stroke: absent tilt is never invented.
    let mut plain = t.clone();
    plain.dynamics.tilt.size.enabled = false;
    assert!(render(&plain, &line(4_000_000), 1, TileGrid::new(), ctx(StrokeMode::Paint), &[4]).tiles == no_tilt.tiles);
}

#[test]
fn pen_rotation_uses_azimuth_only_when_reported() {
    let tip = ring_asset();
    let mut store = AssetStore::new();
    store.insert(tip.clone());
    let p = BrushPreset {
        id: "test.pen".into(),
        size_px: 40.0,
        spacing: 4.0,
        tip: Tip {
            shape: TipShape::Image,
            image: Some(tip.hash().into()),
            rotation: TipRotation::Pen,
            ..Tip::default()
        },
        ..BrushPreset::default()
    };
    let assets = ResolvedAssets::resolve(&p, &[&store]);
    let tap = |az: Option<f32>| {
        let mut s = StrokeSample::confirmed(100.0, 64.0, 0, 1.0);
        s.azimuth = az;
        render_with(&p, &assets, &[s], 1, INK, &[1]).tiles
    };
    let none = tap(None);
    assert!(tap(Some(0.0)) == none, "azimuth 0 = pen pointing up = no rotation");
    assert!(tap(Some(1.2)) != none);
}

#[test]
fn wet_edges_lighten_the_interior() {
    let mut p = BrushPreset { id: "test.wet".into(), size_px: 40.0, spacing: 0.05, ..BrushPreset::default() };
    p.dynamics.pressure.size.enabled = false;
    p.tip.hardness = 0.9;
    let line: Vec<StrokeSample> =
        (0..40).map(|i| StrokeSample::confirmed(40.0 + i as f32 * 4.0, 64.0, i * 4_000_000, 1.0)).collect();
    let dry = render(&p, &line, 1, TileGrid::new(), ctx(StrokeMode::Paint), &[3]);
    p.wet_edges = 1.0;
    let wet = render(&p, &line, 1, TileGrid::new(), ctx(StrokeMode::Paint), &[3]);
    let a = |f: &veskra_brush::FinishedStroke, y: i32| f.tiles.get_pixel(120, y)[3];
    assert_eq!(a(&dry, 64), 65535);
    assert!(a(&wet, 64) < 25_000, "interior {}", a(&wet, 64));
    assert!(a(&wet, 47) > 2 * a(&wet, 64), "rim {} interior {}", a(&wet, 47), a(&wet, 64));
    assert!(premul_invariant_holds(&wet.tiles));
}

#[test]
fn color_jitter_per_dab_and_per_stroke() {
    let base = BrushPreset { id: "test.color".into(), size_px: 16.0, spacing: 0.3, ..BrushPreset::default() };
    let line: Vec<StrokeSample> =
        (0..50).map(|i| StrokeSample::confirmed(20.0 + i as f32 * 4.0, 64.0, i * 4_000_000, 1.0)).collect();
    let red = ColorRgba::new(0.8, 0.2, 0.2, 1.0);
    let colors = |f: &veskra_brush::FinishedStroke| {
        let mut v: Vec<[u16; 3]> =
            (20..220).map(|x| f.tiles.get_pixel(x, 64)).filter(|p| p[3] == 65535).map(|p| [p[0], p[1], p[2]]).collect();
        v.sort();
        v.dedup();
        v
    };
    let per_dab = BrushPreset { jitter: Jitter { hue: 0.3, brightness: 0.2, ..Jitter::default() }, ..base.clone() };
    let mut r = StrokeRenderer::begin(per_dab.clone(), red, 4, TileGrid::new(), ctx(StrokeMode::Paint));
    r.add_samples(&line);
    let f = r.finish();
    assert!(colors(&f).len() > 10, "per-dab jitter varies the color along the stroke");
    assert!(premul_invariant_holds(&f.tiles));
    let per_stroke = BrushPreset { jitter: Jitter { stroke_hue: 0.5, ..Jitter::default() }, ..base.clone() };
    let mut r = StrokeRenderer::begin(per_stroke, red, 4, TileGrid::new(), ctx(StrokeMode::Paint));
    r.add_samples(&line);
    let f = r.finish();
    let c = colors(&f);
    assert_eq!(c.len(), 1, "per-stroke jitter keeps one color");
    let mut r = StrokeRenderer::begin(base.clone(), red, 4, TileGrid::new(), ctx(StrokeMode::Paint));
    r.add_samples(&line);
    assert_ne!(colors(&r.finish()), c, "the stroke color moved");
}

#[test]
fn ellipse_scatter_buildup_strokes_stay_inside_the_clip() {
    let p = BrushPreset {
        id: "test.ellipse".into(),
        size_px: 30.0,
        spacing: 0.05,
        tip: Tip { roundness: 0.2, angle: 30.0, rotation: TipRotation::Direction, ..Tip::default() },
        jitter: Jitter { scatter: 4.0, ..Jitter::default() },
        accumulation: Accumulation::BuildUp,
        flow: 0.4,
        ..BrushPreset::default()
    };
    let f = render(&p, &spiral(128.0, 64.0, 200, 3), 2, TileGrid::new(), ctx(StrokeMode::Paint), &[5]);
    assert!(f.bounds.x0 >= 0 && f.bounds.y0 >= 0 && f.bounds.x1 <= 256 && f.bounds.y1 <= 128);
    assert!(f.tiles.iter_tiles().all(|(c, _)| c.tx >= 0 && c.ty >= 0 && c.tx < 4 && c.ty < 2));
    assert!(premul_invariant_holds(&f.tiles));
}
