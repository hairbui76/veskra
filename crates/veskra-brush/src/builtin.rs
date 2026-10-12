//! Built-in presets (ADR-004, ADR-019): 30 original presets in display order, and the
//! procedurally generated assets some of them use ([`builtin_assets`]).
//!
//! The ten pre-v2 ids ([`LEGACY_BUILTIN_IDS`]) keep their exact parameters (they equal the
//! migrated schema-2 JSON, so existing documents re-render byte-identically). `pen.brush`'s
//! pressure curve is spelled out as literals (the values the old `powf(1.6)` produced) so the
//! preset no longer depends on the platform libm. No third-party brush assets are used.

use std::sync::OnceLock;

use crate::asset::{AssetStore, BrushAsset};
use crate::preset::{
    Accumulation, BrushCategory, BrushPreset, BrushSource, Dynamics, DynamicsSource, DynamicsTarget, Jitter, Texture,
    TextureMode, TextureSource, Tip, TipRotation, TipShape,
};
use crate::{ResponseCurve, procedural};

/// Stable ids of the built-in presets, in display order.
pub const BUILTIN_IDS: [&str; 30] = [
    "pencil.hb",
    "pencil.2b",
    "pencil.mechanical",
    "pencil.smooth",
    "pencil.rough",
    "ink.gpen",
    "pen.technical",
    "ink.fineliner",
    "pen.brush",
    "ink.dry",
    "marker.felt",
    "marker",
    "marker.alcohol",
    "round.hard",
    "paint.round_bristle",
    "paint.flat_oil",
    "paint.gouache",
    "paint.knife",
    "airbrush.soft",
    "round.soft",
    "airbrush.spray",
    "charcoal",
    "texture.chalk",
    "texture.pastel",
    "texture.crayon",
    "texture.sponge",
    "effect.glitter",
    "eraser.hard",
    "eraser.soft",
    "eraser.textured",
];

/// The ten built-in ids of the pre-v2 engine (schema 1/2). Their behaviour is frozen.
pub const LEGACY_BUILTIN_IDS: [&str; 10] = [
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

/// Names of the built-in procedural assets (see [`crate::procedural`]).
pub const BUILTIN_ASSET_NAMES: [&str; 8] =
    ["bristle_round", "bristle_flat", "dry", "speckle", "chalk", "sponge", "paper", "canvas"];

struct Builtins {
    store: AssetStore,
    by_name: Vec<(&'static str, String)>,
}

fn builtins() -> &'static Builtins {
    static B: OnceLock<Builtins> = OnceLock::new();
    B.get_or_init(|| {
        let mut store = AssetStore::new();
        let mut by_name = Vec::new();
        for name in BUILTIN_ASSET_NAMES {
            let (w, h, m) = match name {
                "bristle_round" => procedural::bristle_round(),
                "bristle_flat" => procedural::bristle_flat(),
                "dry" => procedural::dry(),
                "speckle" => procedural::speckle(),
                "chalk" => procedural::chalk(),
                "sponge" => procedural::sponge(),
                "paper" => procedural::paper(),
                _ => procedural::canvas(),
            };
            // Generated masks are within bounds by construction; a failure would be a bug and
            // simply leaves the asset out (presets then use the documented round fallback).
            if let Ok(a) = BrushAsset::from_mask(w, h, m) {
                by_name.push((name, a.hash().to_string()));
                store.insert(std::sync::Arc::new(a));
            }
        }
        Builtins { store, by_name }
    })
}

/// The assets of the built-in presets (generated once, deterministic, identical on every
/// platform).
pub fn builtin_assets() -> &'static AssetStore {
    &builtins().store
}

/// Content hash of a built-in asset by name ([`BUILTIN_ASSET_NAMES`]).
pub fn builtin_asset_hash(name: &str) -> Option<&'static str> {
    builtins().by_name.iter().find(|(n, _)| *n == name).map(|(_, h)| h.as_str())
}

fn asset(name: &str) -> Option<String> {
    builtin_asset_hash(name).map(str::to_string)
}

fn curve(points: &[[f32; 2]]) -> ResponseCurve {
    ResponseCurve::new_shape(points.to_vec()).unwrap_or_default()
}

fn constant1() -> ResponseCurve {
    ResponseCurve::constant(1.0)
}

/// Schema-2 built-in, expressed exactly as the 2 → 3 migration expresses it.
#[allow(clippy::too_many_arguments)]
fn legacy(
    id: &str,
    name: &str,
    category: BrushCategory,
    size_px: f32,
    min_size_ratio: f32,
    opacity: f32,
    flow: f32,
    spacing: f32,
    hardness: f32,
    curves: [ResponseCurve; 3],
    grain: Option<(f32, f32)>,
    accumulation: Accumulation,
    eraser: bool,
    vector_eligible: bool,
    stabilization: f32,
) -> BrushPreset {
    let [ps, po, pf] = curves;
    BrushPreset {
        id: id.into(),
        name: name.into(),
        category,
        source: BrushSource::Builtin,
        size_px,
        opacity,
        flow,
        spacing,
        tip: Tip { hardness, ..Tip::default() },
        texture: grain.map(|(scale, depth)| Texture {
            source: TextureSource::Procedural,
            image: None,
            scale,
            depth,
            invert: false,
            mode: TextureMode::Fixed,
        }),
        dynamics: Dynamics {
            pressure: DynamicsSource {
                size: DynamicsTarget::range(ps, min_size_ratio, 1.0),
                opacity: DynamicsTarget::on(po),
                flow: DynamicsTarget::on(pf),
                ..Default::default()
            },
            ..Default::default()
        },
        accumulation,
        eraser,
        vector_eligible,
        stabilization,
        ..BrushPreset::default()
    }
}

/// A new built-in: pressure → size with `min_size`, nothing else unless overridden.
fn new_builtin(id: &str, name: &str, category: BrushCategory, size_px: f32, min_size: f32) -> BrushPreset {
    BrushPreset {
        id: id.into(),
        name: name.into(),
        category,
        source: BrushSource::Builtin,
        size_px,
        dynamics: Dynamics {
            pressure: DynamicsSource {
                size: DynamicsTarget::range(ResponseCurve::identity(), min_size, 1.0),
                ..Default::default()
            },
            ..Default::default()
        },
        ..BrushPreset::default()
    }
}

fn image_tip(name: &str, rotation: TipRotation) -> Tip {
    Tip { shape: TipShape::Image, image: asset(name), rotation, ..Tip::default() }
}

fn image_texture(name: &str, scale: f32, depth: f32) -> Option<Texture> {
    Some(Texture {
        source: TextureSource::Image,
        image: asset(name),
        scale,
        depth,
        invert: false,
        mode: TextureMode::Fixed,
    })
}

fn grain(scale: f32, depth: f32) -> Option<Texture> {
    Some(Texture { scale, depth, ..Texture::default() })
}

fn with_pressure(mut p: BrushPreset, f: impl FnOnce(&mut DynamicsSource)) -> BrushPreset {
    f(&mut p.dynamics.pressure);
    p
}

/// The built-in presets in display order (stable ids [`BUILTIN_IDS`]).
pub fn builtin_presets() -> Vec<BrushPreset> {
    use BrushCategory as C;
    let id1 = ResponseCurve::identity;
    let pen_brush_curve = curve(&[
        [0.0, 0.0],
        [0.125, 0.035_896_823],
        [0.25, 0.108_818_814],
        [0.375, 0.208_185_29],
        [0.5, 0.329_876_96],
        [0.625, 0.471_420_02],
        [0.75, 0.631_099_76],
        [0.875, 0.807_630_84],
        [1.0, 1.0],
    ]);
    let mut all = vec![
        // ---------------------------------------------------------------- pencils
        with_pressure(
            BrushPreset {
                opacity: 0.85,
                spacing: 0.07,
                tip: Tip { hardness: 0.75, ..Tip::default() },
                texture: grain(1.3, 0.5),
                stabilization: 0.05,
                ..new_builtin("pencil.hb", "HB pencil", C::Pencil, 3.5, 0.55)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.1], [0.6, 0.7], [1.0, 0.95]])),
        ),
        with_pressure(
            BrushPreset {
                opacity: 0.95,
                spacing: 0.07,
                tip: Tip { hardness: 0.65, ..Tip::default() },
                texture: grain(1.8, 0.6),
                jitter: Jitter { opacity: 0.08, ..Jitter::default() },
                ..new_builtin("pencil.2b", "2B pencil", C::Pencil, 5.0, 0.45)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.15], [0.5, 0.75], [1.0, 1.0]])),
        ),
        with_pressure(
            BrushPreset {
                opacity: 0.9,
                spacing: 0.06,
                tip: Tip { hardness: 0.85, ..Tip::default() },
                texture: grain(1.0, 0.35),
                stabilization: 0.1,
                ..new_builtin("pencil.mechanical", "Mechanical pencil", C::Pencil, 2.0, 0.85)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.25], [1.0, 0.95]])),
        ),
        legacy(
            "pencil.smooth",
            "Smooth pencil",
            C::Pencil,
            4.0,
            0.35,
            0.9,
            1.0,
            0.08,
            0.85,
            [id1(), constant1(), constant1()],
            None,
            Accumulation::Max,
            false,
            true,
            0.05,
        ),
        legacy(
            "pencil.rough",
            "Rough pencil",
            C::Pencil,
            5.0,
            0.45,
            1.0,
            0.9,
            0.1,
            0.7,
            [id1(), curve(&[[0.0, 0.15], [0.5, 0.65], [1.0, 1.0]]), constant1()],
            Some((1.6, 0.6)),
            Accumulation::Max,
            false,
            false,
            0.0,
        ),
        // ---------------------------------------------------------------- inks
        with_pressure(
            BrushPreset {
                spacing: 0.04,
                tip: Tip { hardness: 0.98, ..Tip::default() },
                stabilization: 0.2,
                vector_eligible: true,
                ..new_builtin("ink.gpen", "G-pen", C::Ink, 9.0, 0.02)
            },
            |d| d.size.curve = curve(&[[0.0, 0.0], [0.25, 0.12], [0.5, 0.35], [0.75, 0.68], [1.0, 1.0]]),
        ),
        legacy(
            "pen.technical",
            "Technical pen",
            C::Ink,
            3.0,
            0.85,
            1.0,
            1.0,
            0.06,
            1.0,
            [id1(), constant1(), constant1()],
            None,
            Accumulation::Max,
            false,
            true,
            0.15,
        ),
        BrushPreset {
            spacing: 0.05,
            stabilization: 0.15,
            vector_eligible: true,
            ..new_builtin("ink.fineliner", "Fineliner", C::Ink, 2.5, 0.9)
        },
        legacy(
            "pen.brush",
            "Brush pen",
            C::Ink,
            12.0,
            0.06,
            1.0,
            1.0,
            0.05,
            0.95,
            [pen_brush_curve, constant1(), constant1()],
            None,
            Accumulation::Max,
            false,
            true,
            0.1,
        ),
        BrushPreset {
            spacing: 0.04,
            tip: image_tip("dry", TipRotation::Fixed),
            jitter: Jitter { angle: 0.08, ..Jitter::default() },
            stabilization: 0.05,
            ..new_builtin("ink.dry", "Dry ink brush", C::Ink, 18.0, 0.2)
        },
        // ---------------------------------------------------------------- markers
        BrushPreset {
            opacity: 0.85,
            spacing: 0.05,
            tip: Tip { hardness: 0.85, roundness: 0.45, angle: 45.0, ..Tip::default() },
            ..new_builtin("marker.felt", "Felt-tip marker", C::Marker, 14.0, 0.85)
        },
        legacy(
            "marker",
            "Marker",
            C::Marker,
            16.0,
            0.8,
            0.7,
            1.0,
            0.06,
            0.9,
            [id1(), constant1(), constant1()],
            None,
            Accumulation::Max,
            false,
            true,
            0.0,
        ),
        BrushPreset {
            opacity: 0.45,
            spacing: 0.04,
            tip: Tip { hardness: 0.5, ..Tip::default() },
            wet_edges: 0.35,
            ..new_builtin("marker.alcohol", "Alcohol marker (blendy)", C::Marker, 24.0, 0.7)
        },
        // ---------------------------------------------------------------- paint
        legacy(
            "round.hard",
            "Hard round",
            C::Paint,
            20.0,
            0.5,
            1.0,
            1.0,
            0.08,
            1.0,
            [id1(), constant1(), constant1()],
            None,
            Accumulation::Max,
            false,
            true,
            0.0,
        ),
        with_pressure(
            BrushPreset {
                flow: 0.9,
                spacing: 0.03,
                tip: image_tip("bristle_round", TipRotation::Direction),
                jitter: Jitter { size: 0.05, stroke_hue: 0.01, stroke_brightness: 0.04, ..Jitter::default() },
                ..new_builtin("paint.round_bristle", "Round bristle", C::Paint, 30.0, 0.4)
            },
            |d| d.flow = DynamicsTarget::range(ResponseCurve::identity(), 0.35, 1.0),
        ),
        with_pressure(
            BrushPreset {
                flow: 0.85,
                spacing: 0.03,
                tip: image_tip("bristle_flat", TipRotation::Direction),
                texture: image_texture("canvas", 1.0, 0.15),
                ..new_builtin("paint.flat_oil", "Flat oil", C::Paint, 36.0, 0.6)
            },
            |d| d.flow = DynamicsTarget::range(ResponseCurve::identity(), 0.4, 1.0),
        ),
        BrushPreset {
            opacity: 0.95,
            spacing: 0.05,
            tip: Tip { hardness: 0.7, ..Tip::default() },
            texture: image_texture("canvas", 1.0, 0.18),
            wet_edges: 0.18,
            ..new_builtin("paint.gouache", "Gouache", C::Paint, 26.0, 0.5)
        },
        BrushPreset {
            spacing: 0.04,
            tip: Tip { hardness: 0.9, roundness: 0.15, rotation: TipRotation::Direction, ..Tip::default() },
            texture: Some(Texture { scale: 6.0, depth: 0.4, mode: TextureMode::Follow, ..Texture::default() }),
            ..new_builtin("paint.knife", "Palette knife", C::Paint, 34.0, 0.7)
        },
        // ---------------------------------------------------------------- airbrush
        BrushPreset {
            flow: 0.06,
            spacing: 0.08,
            tip: Tip { hardness: 0.0, ..Tip::default() },
            accumulation: Accumulation::BuildUp,
            dynamics: Dynamics {
                pressure: DynamicsSource {
                    flow: DynamicsTarget::on(curve(&[[0.0, 0.0], [0.5, 0.3], [1.0, 1.0]])),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..new_builtin("airbrush.soft", "Soft airbrush", C::Airbrush, 80.0, 1.0)
        },
        legacy(
            "round.soft",
            "Soft round (airbrush)",
            C::Airbrush,
            40.0,
            1.0,
            1.0,
            0.08,
            0.1,
            0.0,
            [id1(), constant1(), id1()],
            None,
            Accumulation::BuildUp,
            false,
            false,
            0.0,
        ),
        BrushPreset {
            flow: 0.5,
            spacing: 0.35,
            tip: image_tip("speckle", TipRotation::Fixed),
            jitter: Jitter { angle: 1.0, size: 0.25, scatter: 0.25, ..Jitter::default() },
            accumulation: Accumulation::BuildUp,
            dynamics: Dynamics {
                pressure: DynamicsSource { flow: DynamicsTarget::on(ResponseCurve::identity()), ..Default::default() },
                ..Default::default()
            },
            ..new_builtin("airbrush.spray", "Spray / speckle", C::Airbrush, 60.0, 1.0)
        },
        // ---------------------------------------------------------------- textures
        legacy(
            "charcoal",
            "Charcoal",
            C::Texture,
            14.0,
            0.5,
            1.0,
            0.85,
            0.08,
            0.6,
            [id1(), id1(), constant1()],
            Some((3.0, 0.75)),
            Accumulation::Max,
            false,
            false,
            0.0,
        ),
        with_pressure(
            BrushPreset {
                spacing: 0.08,
                tip: image_tip("chalk", TipRotation::Fixed),
                jitter: Jitter { angle: 0.25, ..Jitter::default() },
                texture: image_texture("paper", 1.0, 0.75),
                ..new_builtin("texture.chalk", "Chalk", C::Texture, 22.0, 0.6)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.3], [1.0, 1.0]])),
        ),
        with_pressure(
            BrushPreset {
                flow: 0.45,
                spacing: 0.06,
                tip: Tip { hardness: 0.45, roundness: 0.8, angle: 30.0, ..Tip::default() },
                texture: image_texture("paper", 1.5, 0.65),
                accumulation: Accumulation::BuildUp,
                ..new_builtin("texture.pastel", "Soft pastel", C::Texture, 30.0, 0.7)
            },
            |d| d.flow = DynamicsTarget::range(ResponseCurve::identity(), 0.2, 1.0),
        ),
        with_pressure(
            BrushPreset {
                spacing: 0.06,
                tip: Tip { hardness: 0.75, roundness: 0.85, ..Tip::default() },
                texture: grain(1.8, 0.8),
                ..new_builtin("texture.crayon", "Wax crayon", C::Texture, 12.0, 0.75)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.35], [1.0, 1.0]])),
        ),
        BrushPreset {
            flow: 0.55,
            spacing: 0.22,
            tip: image_tip("sponge", TipRotation::Fixed),
            jitter: Jitter { angle: 1.0, size: 0.2, scatter: 0.12, ..Jitter::default() },
            accumulation: Accumulation::BuildUp,
            dynamics: Dynamics {
                pressure: DynamicsSource { flow: DynamicsTarget::on(ResponseCurve::identity()), ..Default::default() },
                ..Default::default()
            },
            ..new_builtin("texture.sponge", "Rough sponge", C::Texture, 50.0, 1.0)
        },
        // ---------------------------------------------------------------- effects
        BrushPreset {
            flow: 0.8,
            spacing: 0.4,
            tip: image_tip("speckle", TipRotation::Fixed),
            jitter: Jitter { angle: 1.0, size: 0.4, scatter: 0.6, hue: 0.12, brightness: 0.25, ..Jitter::default() },
            ..new_builtin("effect.glitter", "Glitter", C::Effect, 40.0, 0.8)
        },
        // ---------------------------------------------------------------- erasers
        legacy(
            "eraser.hard",
            "Hard eraser",
            C::Eraser,
            20.0,
            0.6,
            1.0,
            1.0,
            0.08,
            1.0,
            [id1(), constant1(), constant1()],
            None,
            Accumulation::Max,
            true,
            false,
            0.0,
        ),
        legacy(
            "eraser.soft",
            "Soft eraser",
            C::Eraser,
            40.0,
            1.0,
            1.0,
            0.15,
            0.1,
            0.0,
            [id1(), constant1(), id1()],
            None,
            Accumulation::BuildUp,
            true,
            false,
            0.0,
        ),
        BrushPreset {
            spacing: 0.08,
            tip: Tip { hardness: 0.6, ..Tip::default() },
            texture: grain(2.5, 0.65),
            eraser: true,
            ..new_builtin("eraser.textured", "Textured eraser", C::Eraser, 24.0, 0.6)
        },
    ];
    // An image the generator could not provide would make the preset invalid; degrade to the
    // documented fallback (round tip / procedural grain) instead of shipping an invalid preset.
    for p in &mut all {
        if p.tip.shape == TipShape::Image && p.tip.image.is_none() {
            p.tip.shape = TipShape::Round;
        }
        if let Some(t) = &mut p.texture
            && t.source == TextureSource::Image
            && t.image.is_none()
        {
            *t = Texture { scale: 2.0, ..t.clone() };
            t.source = TextureSource::Procedural;
        }
    }
    all
}

/// Built-in preset by stable id.
pub fn builtin_preset(id: &str) -> Option<BrushPreset> {
    builtin_presets().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_generated_and_referenced() {
        assert_eq!(builtin_assets().len(), BUILTIN_ASSET_NAMES.len());
        for p in builtin_presets() {
            for h in p.asset_refs() {
                assert!(builtin_assets().contains(&h), "{}: {h}", p.id);
            }
        }
        // The hash is a pure function of the generator (stable across runs and platforms).
        let (w, h, m) = procedural::paper();
        assert_eq!(builtin_asset_hash("paper"), Some(crate::asset::mask_hash(w, h, &m).as_str()));
    }

    #[test]
    fn categories_cover_the_requested_families() {
        let all = builtin_presets();
        assert_eq!(all.len(), 30);
        for c in [
            BrushCategory::Pencil,
            BrushCategory::Ink,
            BrushCategory::Marker,
            BrushCategory::Paint,
            BrushCategory::Airbrush,
            BrushCategory::Texture,
            BrushCategory::Eraser,
        ] {
            assert!(all.iter().filter(|p| p.category == c).count() >= 3, "{c:?}");
        }
        for id in LEGACY_BUILTIN_IDS {
            assert!(BUILTIN_IDS.contains(&id));
        }
    }
}
