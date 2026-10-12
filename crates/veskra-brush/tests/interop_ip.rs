//! ADR-019: Infinite Painter `.prbr` property mapping on public-domain fixture properties
//! (`tests/fixtures/brushes/infinite_painter/`) with synthetic PNG heads / textures.

use veskra_brush::interop::infinite_painter::{PrbrParts, map_prbr};
use veskra_brush::{
    BrushCategory, BrushError, BrushSource, StrokeContext, StrokeMode, StrokeRenderer, TextureMode, TextureSource,
    TipRotation, TipShape, load_preset_json,
};
use veskra_core::{ColorRgba, RectI, StrokeSample};
use veskra_raster::TileGrid;

fn fixture(name: &str) -> Vec<u8> {
    let p = veskra_testkit::fixtures_dir().join("brushes").join("infinite_painter").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn gray_png(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let px: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect();
    veskra_testkit::encode_png_rgba8(w, h, &px.iter().flat_map(|&g| [g, g, g, 255]).collect::<Vec<u8>>())
}

#[test]
fn manga_inker_maps_with_reported_approximations() {
    let props = fixture("manga_inker.json");
    let b = map_prbr(&PrbrParts { properties: &props, head: None, texture: None, file_name: "x" }, "Manga").unwrap();
    let p = &b.preset;
    p.validate().unwrap();
    assert_eq!(p.name, "Manga Inker");
    assert_eq!(p.folder, "Manga");
    assert_eq!(p.category, BrushCategory::Imported);
    assert_eq!(p.source, BrushSource::Imported("infinite_painter".into()));
    // 85.809 × 0.2469 × 3.74 ≈ 79.2 px
    assert!((p.size_px - 79.24).abs() < 0.1, "{}", p.size_px);
    assert!((p.spacing - 0.103_890_195).abs() < 1e-6);
    assert!((p.stabilization - 0.203_333_35).abs() < 1e-6);
    assert_eq!(p.tip.shape, TipShape::Round);
    assert_eq!(p.tip.hardness, 1.0);
    // pressure → size and flow, inverted-y profiles.
    let ps = &p.dynamics.pressure.size;
    assert!(ps.enabled);
    assert!((ps.curve.eval(0.0) - 0.039_062_5).abs() < 1e-6);
    assert!((ps.curve.eval(1.0) - 1.0).abs() < 1e-6);
    assert!(p.dynamics.pressure.flow.enabled);
    assert!(p.dynamics.pressure.texture.enabled, "texture dynamics mapped even without a texture");
    assert!(p.dynamics.tilt.texture.enabled);
    assert!(!p.dynamics.pressure.scatter.enabled);
    assert!(p.texture.is_none());
    assert!(b.approximated.iter().any(|a| a.starts_with("size:")));
    assert!(b.approximated.iter().any(|a| a.contains("lazy stroke")));
    assert!(b.approximated.iter().any(|a| a.contains("tilt offset")));
    // The exported app brush uses head style 1, whose meaning is not documented.
    assert_eq!(b.unsupported, vec!["head style 1 (meaning unknown; normal style used)".to_string()]);
    // Round-trips through the schema-3 JSON.
    let mut q = p.clone();
    q.id = "imp.manga".into();
    assert_eq!(load_preset_json(&q.to_json().unwrap()).unwrap().preset, q);
}

#[test]
fn template_with_synthetic_head_and_texture() {
    let props = fixture("template-properties.json");
    // Black = paint (conversion format 1 in the template): a black disc on white.
    let head = gray_png(48, 48, |x, y| {
        let (dx, dy) = (x as f64 - 23.5, y as f64 - 23.5);
        if dx * dx + dy * dy < 400.0 { 0 } else { 255 }
    });
    let texture = gray_png(32, 32, |x, y| if (x / 4 + y / 4) % 2 == 0 { 255 } else { 40 });
    let parts = PrbrParts { properties: &props, head: Some(&head), texture: Some(&texture), file_name: "Template" };
    let b = map_prbr(&parts, "").unwrap();
    let p = &b.preset;
    p.validate().unwrap();
    assert_eq!(p.name, "Template", "empty display-name falls back to the file name");
    assert_eq!(p.tip.shape, TipShape::Image);
    assert_eq!(b.assets.len(), 2);
    let tip = &b.assets[0];
    assert_eq!(p.tip.image.as_deref(), Some(tip.hash()));
    assert_eq!(tip.mask()[24 * 48 + 24], 255, "black head pixels became paint");
    assert_eq!(tip.mask()[0], 0);
    let t = p.texture.clone().unwrap();
    assert_eq!(t.source, TextureSource::Image);
    assert_eq!(t.mode, TextureMode::Follow, "style 1 = rotate with the stroke");
    assert!((t.depth - (1.0 - 0.61)).abs() < 1e-6, "depth = 1 − pressure");
    assert_eq!(p.tip.rotation, TipRotation::Fixed);
    assert!(p.jitter.size > 0.6);
    // tilt → size is on in the template; pressure → size is off.
    assert!(p.dynamics.tilt.size.enabled && !p.dynamics.pressure.size.enabled);
    assert!(!p.vector_eligible);
    // Renders deterministically with its own assets.
    let assets = veskra_brush::ResolvedAssets {
        tip: Some(std::sync::Arc::new(b.assets[0].clone())),
        texture: Some(std::sync::Arc::new(b.assets[1].clone())),
    };
    let samples: Vec<StrokeSample> =
        (0..40).map(|i| StrokeSample::confirmed(20.0 + i as f32 * 5.0, 60.0, i * 4_000_000, 0.7)).collect();
    let run = || {
        let mut q = p.clone();
        q.id = "imp.t".into();
        let mut r = StrokeRenderer::begin_resolved(
            q,
            assets.clone(),
            ColorRgba::BLACK,
            3,
            TileGrid::new(),
            StrokeContext::new(StrokeMode::Paint, RectI::new(0, 0, 256, 128)),
        );
        r.add_samples(&samples);
        assert!(r.asset_fallbacks().is_empty());
        r.finish().tiles
    };
    let a = run();
    assert!(a == run());
    assert!(a.tile_count() > 0);
}

#[test]
fn builtin_assets_and_special_features_are_named_not_copied() {
    let mut v: serde_json::Value = serde_json::from_slice(&fixture("template-properties.json")).unwrap();
    v["source-properties"]["head"] = "brush_head_proko_pencil".into();
    v["source-properties"]["stroke texture"] = "texture_8".into();
    v["blend-properties"]["mix-wet"] = true.into();
    v["blend-properties"]["mix-in"] = 0.4.into();
    v["dynamics-properties"]["pressure - effects dilution"] = true.into();
    v["stroke-properties"]["has-size-profile"] = true.into();
    v["stroke-properties"]["blendmode"] = 3.into();
    v["head-properties"]["rotation"] = 1.into();
    v["head-properties"]["use-trajectory"] = true.into();
    v["bleed-properties"] = serde_json::json!({"amount": 0.3});
    let props = serde_json::to_vec(&v).unwrap();
    let b = map_prbr(&PrbrParts { properties: &props, head: None, texture: None, file_name: "P" }, "Pack").unwrap();
    let u = &b.unsupported;
    for want in [
        "built-in head \"brush_head_proko_pencil\" replaced by round tip",
        "built-in texture \"texture_8\" replaced by procedural grain",
        "wet mix",
        "paint mix-in",
        "pressure → dilution",
        "stroke size profile (taper along the stroke)",
        "blend mode 3",
        "watercolor bleed (bleed-properties)",
    ] {
        assert!(u.iter().any(|x| x == want), "missing {want:?} in {u:?}");
    }
    assert!(b.assets.is_empty(), "nothing is invented for built-in app assets");
    assert_eq!(b.preset.tip.shape, TipShape::Round);
    assert_eq!(b.preset.texture.as_ref().map(|t| t.source), Some(TextureSource::Procedural));
    assert_eq!(b.preset.tip.rotation, TipRotation::Pen);
    assert!(b.approximated.iter().any(|a| a.contains("pen rotation and stroke rotation")));
    b.preset.validate().unwrap();
}

#[test]
fn hostile_properties_are_typed_errors() {
    let bad = |bytes: &[u8]| map_prbr(&PrbrParts { properties: bytes, head: None, texture: None, file_name: "x" }, "");
    assert!(matches!(bad(b"not json"), Err(BrushError::Parse(_))));
    assert!(matches!(bad(b"[1,2,3]"), Err(BrushError::Parse(_))));
    let huge = vec![b' '; 1024 * 1024 + 1];
    assert!(matches!(bad(&huge), Err(BrushError::TooLarge { .. })));
    // Absurd numbers are clamped into a valid preset, never NaN.
    let wild = br#"{"display-name":"\u0007W\u0000","stroke-properties":{"paint-size":1e300,"size-maximum":1e300,"paint-opacity":-5,"wet-edges":99},
        "head-properties":{"spacing":1e-9,"angle":1e30,"flow":0},"jitter-properties":{"scatter":1e9,"size":-3},
        "dynamics-properties":{"pressure - effects size":true,"pressure profile - size":[0,"x"]}}"#;
    let b = bad(wild).unwrap();
    b.preset.validate().unwrap();
    assert_eq!(b.preset.name, "W");
    assert_eq!(b.preset.size_px, 1000.0);
    // A corrupt head PNG degrades to a round tip and is reported.
    let props = fixture("manga_inker.json");
    let b = map_prbr(
        &PrbrParts { properties: &props, head: Some(b"\x89PNG\r\n\x1a\nbroken"), texture: None, file_name: "x" },
        "",
    )
    .unwrap();
    assert_eq!(b.preset.tip.shape, TipShape::Round);
    assert!(b.unsupported.iter().any(|u| u.starts_with("custom head image unreadable")));
}
