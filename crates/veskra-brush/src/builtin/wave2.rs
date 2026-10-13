//! Second wave of built-in presets (ADR-019 amendment 1): 40 original brushes in the existing
//! families plus the `watercolor`, `calligraphy`, `nature` and `pattern` categories.
//!
//! They use only schema-3 features the engine already renders (no new rasterizer code, so the
//! first 30 built-ins are untouched). Every tip and texture image comes from
//! [`crate::procedural::wave2`]; no image is taken from any application or brush pack. Only
//! `calligraphy.brush_marker` satisfies the vector rule; every other new brush is raster-only by
//! construction (image tip, ellipse, texture, jitter, wet edges, velocity dynamics or category).

use super::{curve, grain, image_texture, image_tip, new_builtin, with_pressure};
use crate::ResponseCurve;
use crate::preset::{
    Accumulation, BrushCategory as C, BrushPreset, Dynamics, DynamicsTarget, Jitter, Texture, TextureMode, Tip,
    TipRotation,
};

/// `1 → 0`: "more input = less" (velocity → thinner / drier, pressure → less texture).
fn falling() -> ResponseCurve {
    curve(&[[0.0, 1.0], [1.0, 0.0]])
}

fn with_dynamics(mut p: BrushPreset, f: impl FnOnce(&mut Dynamics)) -> BrushPreset {
    f(&mut p.dynamics);
    p
}

/// Image texture that moves and turns with the dab.
fn follow_texture(name: &str, scale: f32, depth: f32) -> Option<Texture> {
    image_texture(name, scale, depth).map(|t| Texture { mode: TextureMode::Follow, ..t })
}

/// The wave-2 presets (any order; [`super::builtin_presets`] sorts them into display order).
pub(super) fn presets() -> Vec<BrushPreset> {
    let fixed = TipRotation::Fixed;
    let along = TipRotation::Direction;
    vec![
        // ---------------------------------------------------------------- pencils
        with_pressure(
            BrushPreset {
                opacity: 0.9,
                spacing: 0.06,
                tip: Tip { hardness: 0.7, ..Tip::default() },
                texture: image_texture("paper", 0.8, 0.55),
                jitter: Jitter { opacity: 0.06, ..Jitter::default() },
                ..new_builtin("pencil.colored", "Colored pencil", C::Pencil, 6.0, 0.6)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.3], [0.6, 0.75], [1.0, 0.95]])),
        ),
        with_pressure(
            BrushPreset {
                spacing: 0.06,
                tip: Tip { hardness: 0.6, ..Tip::default() },
                texture: image_texture("paper", 0.9, 0.7),
                ..new_builtin("pencil.charcoal", "Charcoal pencil", C::Pencil, 7.0, 0.5)
            },
            |d| {
                d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.25], [1.0, 1.0]]));
                d.texture = DynamicsTarget::range(falling(), 0.55, 1.0);
            },
        ),
        with_pressure(
            BrushPreset {
                opacity: 0.8,
                spacing: 0.04,
                tip: Tip { hardness: 0.55, roundness: 0.3, angle: 90.0, rotation: along, ..Tip::default() },
                texture: image_texture("paper", 1.2, 0.7),
                ..new_builtin("pencil.graphite_stick", "Graphite stick", C::Pencil, 26.0, 0.6)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.2], [1.0, 0.9]])),
        ),
        // ---------------------------------------------------------------- inks
        with_pressure(
            BrushPreset {
                spacing: 0.06,
                tip: Tip { hardness: 0.85, ..Tip::default() },
                texture: grain(0.9, 0.22),
                stabilization: 0.1,
                ..new_builtin("ink.ballpoint", "Ballpoint pen", C::Ink, 2.4, 0.8)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.2], [0.5, 0.7], [1.0, 1.0]])),
        ),
        // ---------------------------------------------------------------- calligraphy
        with_dynamics(
            BrushPreset {
                spacing: 0.03,
                tip: Tip { hardness: 0.95, roundness: 0.55, angle: 50.0, ..Tip::default() },
                stabilization: 0.25,
                ..new_builtin("calligraphy.dip_pen", "Dip pen", C::Calligraphy, 8.0, 0.05)
            },
            |d| {
                d.pressure.size.curve = curve(&[[0.0, 0.0], [0.4, 0.18], [1.0, 1.0]]);
                d.velocity.size = DynamicsTarget::range(falling(), 0.6, 1.0);
            },
        ),
        with_pressure(
            BrushPreset {
                opacity: 0.88,
                spacing: 0.04,
                tip: Tip { hardness: 0.8, ..Tip::default() },
                stabilization: 0.15,
                vector_eligible: true,
                ..new_builtin("calligraphy.brush_marker", "Brush marker", C::Calligraphy, 16.0, 0.06)
            },
            |d| d.size.curve = curve(&[[0.0, 0.0], [0.25, 0.15], [0.6, 0.55], [1.0, 1.0]]),
        ),
        BrushPreset {
            spacing: 0.02,
            tip: Tip { hardness: 0.95, roundness: 0.12, angle: 40.0, ..Tip::default() },
            stabilization: 0.15,
            ..new_builtin("calligraphy.flat_nib", "Flat nib", C::Calligraphy, 24.0, 0.75)
        },
        with_dynamics(
            BrushPreset {
                spacing: 0.025,
                tip: image_tip("sumi", along),
                jitter: Jitter { size: 0.04, ..Jitter::default() },
                stabilization: 0.1,
                ..new_builtin("calligraphy.sumi", "Sumi-e brush", C::Calligraphy, 36.0, 0.1)
            },
            |d| {
                d.pressure.size.curve = curve(&[[0.0, 0.0], [0.3, 0.12], [0.7, 0.55], [1.0, 1.0]]);
                d.velocity.flow = DynamicsTarget::range(falling(), 0.6, 1.0);
            },
        ),
        // ---------------------------------------------------------------- markers
        BrushPreset {
            opacity: 0.4,
            spacing: 0.04,
            tip: Tip { hardness: 0.9, roundness: 0.3, angle: 80.0, ..Tip::default() },
            ..new_builtin("marker.highlighter", "Highlighter", C::Marker, 22.0, 1.0)
        },
        // ---------------------------------------------------------------- paint
        BrushPreset {
            spacing: 0.02,
            tip: image_tip("impasto", along),
            texture: follow_texture("ridges", 0.7, 0.5),
            jitter: Jitter { brightness: 0.02, stroke_brightness: 0.03, ..Jitter::default() },
            ..new_builtin("paint.impasto", "Impasto", C::Paint, 40.0, 0.6)
        },
        BrushPreset {
            flow: 0.8,
            spacing: 0.025,
            tip: image_tip("fan", along),
            jitter: Jitter { opacity: 0.15, ..Jitter::default() },
            ..new_builtin("paint.fan", "Fan brush", C::Paint, 56.0, 0.7)
        },
        // ---------------------------------------------------------------- watercolor
        with_pressure(
            BrushPreset {
                opacity: 0.8,
                flow: 0.6,
                spacing: 0.05,
                tip: Tip { hardness: 0.5, ..Tip::default() },
                texture: image_texture("paper", 1.0, 0.3),
                wet_edges: 0.45,
                jitter: Jitter { stroke_brightness: 0.02, ..Jitter::default() },
                ..new_builtin("watercolor.round", "Watercolor round", C::Watercolor, 28.0, 0.35)
            },
            |d| d.flow = DynamicsTarget::range(ResponseCurve::identity(), 0.5, 1.0),
        ),
        with_pressure(
            BrushPreset {
                opacity: 0.7,
                flow: 0.4,
                spacing: 0.06,
                tip: Tip { hardness: 0.15, ..Tip::default() },
                texture: image_texture("paper", 1.2, 0.25),
                wet_edges: 0.6,
                accumulation: Accumulation::BuildUp,
                jitter: Jitter { stroke_hue: 0.01, stroke_brightness: 0.03, ..Jitter::default() },
                ..new_builtin("watercolor.wet_wash", "Wet wash", C::Watercolor, 70.0, 0.7)
            },
            |d| d.flow = DynamicsTarget::range(ResponseCurve::identity(), 0.4, 1.0),
        ),
        BrushPreset {
            opacity: 0.75,
            flow: 0.3,
            spacing: 0.07,
            tip: Tip { hardness: 0.25, ..Tip::default() },
            texture: image_texture("granulation", 1.0, 0.65),
            wet_edges: 0.35,
            accumulation: Accumulation::BuildUp,
            jitter: Jitter { stroke_hue: 0.015, ..Jitter::default() },
            ..new_builtin("watercolor.granulating", "Granulating wash", C::Watercolor, 56.0, 0.75)
        },
        with_pressure(
            BrushPreset {
                opacity: 0.95,
                spacing: 0.03,
                tip: image_tip("dry_bristle", along),
                texture: grain(3.0, 0.85),
                ..new_builtin("watercolor.dry_brush", "Dry brush", C::Watercolor, 40.0, 0.5)
            },
            |d| d.texture = DynamicsTarget::range(falling(), 0.5, 1.0),
        ),
        BrushPreset {
            opacity: 0.75,
            flow: 0.4,
            spacing: 0.1,
            tip: image_tip("bloom", fixed),
            jitter: Jitter { angle: 1.0, size: 0.25, scatter: 0.12, ..Jitter::default() },
            texture: image_texture("paper", 1.3, 0.2),
            wet_edges: 0.85,
            accumulation: Accumulation::BuildUp,
            ..new_builtin("watercolor.bleed", "Bleeding edge", C::Watercolor, 64.0, 0.8)
        },
        BrushPreset {
            opacity: 0.65,
            flow: 0.35,
            spacing: 0.07,
            tip: Tip { hardness: 0.2, ..Tip::default() },
            texture: image_texture("salt", 1.0, 0.85),
            wet_edges: 0.4,
            accumulation: Accumulation::BuildUp,
            ..new_builtin("watercolor.salt", "Salt texture", C::Watercolor, 60.0, 0.8)
        },
        // ---------------------------------------------------------------- airbrush
        with_pressure(
            BrushPreset {
                flow: 0.1,
                spacing: 0.08,
                tip: Tip { hardness: 0.0, ..Tip::default() },
                texture: grain(1.2, 0.9),
                accumulation: Accumulation::BuildUp,
                ..new_builtin("airbrush.grainy", "Grainy airbrush", C::Airbrush, 90.0, 1.0)
            },
            |d| d.flow = DynamicsTarget::on(curve(&[[0.0, 0.0], [0.5, 0.3], [1.0, 1.0]])),
        ),
        // ---------------------------------------------------------------- textures
        with_pressure(
            BrushPreset {
                spacing: 0.05,
                tip: Tip { angle: 15.0, ..image_tip("conte", fixed) },
                texture: image_texture("paper", 1.4, 0.65),
                ..new_builtin("texture.conte", "Conté crayon", C::Texture, 18.0, 0.65)
            },
            |d| d.opacity = DynamicsTarget::on(curve(&[[0.0, 0.3], [1.0, 1.0]])),
        ),
        // ---------------------------------------------------------------- nature
        BrushPreset {
            spacing: 0.18,
            tip: image_tip("grass", fixed),
            jitter: Jitter {
                size: 0.35,
                angle: 0.06,
                scatter: 0.15,
                hue: 0.05,
                saturation: 0.08,
                brightness: 0.18,
                ..Jitter::default()
            },
            ..new_builtin("nature.grass", "Grass", C::Nature, 70.0, 0.6)
        },
        BrushPreset {
            spacing: 0.3,
            tip: image_tip("foliage", fixed),
            jitter: Jitter { angle: 1.0, size: 0.4, scatter: 0.45, hue: 0.05, brightness: 0.2, ..Jitter::default() },
            ..new_builtin("nature.foliage", "Foliage", C::Nature, 64.0, 0.7)
        },
        BrushPreset {
            spacing: 1.1,
            tip: image_tip("leaf", fixed),
            jitter: Jitter {
                angle: 1.0,
                size: 0.45,
                scatter: 1.8,
                hue: 0.1,
                saturation: 0.1,
                brightness: 0.2,
                ..Jitter::default()
            },
            ..new_builtin("nature.leaves", "Falling leaves", C::Nature, 36.0, 0.7)
        },
        BrushPreset {
            spacing: 0.95,
            tip: image_tip("flower", fixed),
            jitter: Jitter {
                angle: 1.0,
                size: 0.5,
                scatter: 1.2,
                hue: 0.18,
                saturation: 0.15,
                brightness: 0.12,
                ..Jitter::default()
            },
            ..new_builtin("nature.flowers", "Flowers", C::Nature, 36.0, 0.7)
        },
        BrushPreset {
            spacing: 0.55,
            tip: image_tip("twig", along),
            jitter: Jitter { angle: 0.1, size: 0.35, scatter: 0.2, brightness: 0.1, ..Jitter::default() },
            ..new_builtin("nature.twigs", "Twigs", C::Nature, 60.0, 0.6)
        },
        BrushPreset {
            flow: 0.9,
            spacing: 0.12,
            tip: image_tip("fur", along),
            jitter: Jitter { angle: 0.05, size: 0.2, scatter: 0.15, brightness: 0.08, ..Jitter::default() },
            ..new_builtin("nature.fur", "Fur", C::Nature, 40.0, 0.5)
        },
        BrushPreset {
            flow: 0.85,
            spacing: 0.02,
            tip: image_tip("hair", along),
            stabilization: 0.2,
            ..new_builtin("nature.hair", "Hair strands", C::Nature, 28.0, 0.6)
        },
        // ---------------------------------------------------------------- effects
        BrushPreset {
            spacing: 1.0,
            tip: image_tip("star", fixed),
            jitter: Jitter {
                angle: 0.08,
                size: 0.75,
                scatter: 1.3,
                opacity: 0.3,
                brightness: 0.15,
                ..Jitter::default()
            },
            accumulation: Accumulation::BuildUp,
            ..new_builtin("effect.sparkle", "Sparkles", C::Effect, 50.0, 0.8)
        },
        BrushPreset {
            flow: 0.4,
            spacing: 0.85,
            tip: image_tip("bokeh", fixed),
            jitter: Jitter { size: 0.65, scatter: 1.6, opacity: 0.5, hue: 0.06, brightness: 0.2, ..Jitter::default() },
            accumulation: Accumulation::BuildUp,
            ..new_builtin("effect.bokeh", "Bokeh", C::Effect, 70.0, 0.9)
        },
        BrushPreset {
            spacing: 0.04,
            tip: image_tip("glow", fixed),
            ..new_builtin("effect.glow", "Glow", C::Effect, 50.0, 0.6)
        },
        BrushPreset {
            flow: 0.28,
            spacing: 0.1,
            tip: Tip { angle: 20.0, ..image_tip("ray", fixed) },
            jitter: Jitter { size: 0.45, scatter: 0.15, opacity: 0.6, ..Jitter::default() },
            accumulation: Accumulation::BuildUp,
            ..new_builtin("effect.light_rays", "Light rays", C::Effect, 220.0, 1.0)
        },
        with_pressure(
            BrushPreset {
                flow: 0.1,
                spacing: 0.15,
                tip: image_tip("mist", fixed),
                jitter: Jitter { angle: 1.0, size: 0.3, scatter: 0.3, ..Jitter::default() },
                accumulation: Accumulation::BuildUp,
                ..new_builtin("effect.fog", "Fog / mist", C::Effect, 160.0, 1.0)
            },
            |d| d.flow = DynamicsTarget::on(ResponseCurve::identity()),
        ),
        BrushPreset {
            flow: 0.85,
            spacing: 0.4,
            tip: Tip { angle: 12.0, ..image_tip("rain", fixed) },
            jitter: Jitter { size: 0.35, scatter: 1.5, opacity: 0.5, ..Jitter::default() },
            ..new_builtin("effect.rain", "Rain", C::Effect, 80.0, 0.9)
        },
        BrushPreset {
            spacing: 0.8,
            tip: Tip { hardness: 0.35, ..Tip::default() },
            jitter: Jitter { size: 0.8, scatter: 3.0, opacity: 0.4, ..Jitter::default() },
            ..new_builtin("effect.snow", "Snow", C::Effect, 20.0, 0.9)
        },
        BrushPreset {
            spacing: 1.3,
            tip: image_tip("splatter", fixed),
            jitter: Jitter { angle: 1.0, size: 0.6, scatter: 0.8, ..Jitter::default() },
            ..new_builtin("effect.splatter", "Ink splatter", C::Effect, 70.0, 0.8)
        },
        // ---------------------------------------------------------------- patterns
        BrushPreset {
            spacing: 0.08,
            tip: Tip { hardness: 0.6, ..Tip::default() },
            texture: image_texture("halftone", 1.0, 1.0),
            ..new_builtin("pattern.halftone", "Halftone dots", C::Pattern, 80.0, 0.7)
        },
        BrushPreset {
            spacing: 0.6,
            tip: image_tip("stipple", fixed),
            jitter: Jitter { angle: 1.0, scatter: 0.1, ..Jitter::default() },
            ..new_builtin("pattern.stipple", "Stipple", C::Pattern, 50.0, 0.7)
        },
        with_pressure(
            BrushPreset {
                spacing: 0.08,
                tip: Tip { hardness: 0.5, ..Tip::default() },
                texture: image_texture("crosshatch", 0.8, 1.0),
                ..new_builtin("pattern.crosshatch", "Cross-hatch", C::Pattern, 50.0, 0.6)
            },
            |d| d.texture = DynamicsTarget::range(falling(), 0.45, 1.0),
        ),
        BrushPreset {
            spacing: 0.08,
            tip: Tip { hardness: 0.75, ..Tip::default() },
            texture: image_texture("scales", 1.0, 1.0),
            ..new_builtin("pattern.scales", "Scales", C::Pattern, 80.0, 0.7)
        },
        BrushPreset {
            spacing: 0.08,
            tip: Tip { hardness: 0.75, ..Tip::default() },
            texture: image_texture("honeycomb", 1.0, 1.0),
            ..new_builtin("pattern.honeycomb", "Honeycomb", C::Pattern, 80.0, 0.7)
        },
        BrushPreset {
            flow: 0.55,
            spacing: 0.3,
            tip: image_tip("cloud", fixed),
            jitter: Jitter { size: 0.35, scatter: 0.35, angle: 0.03, ..Jitter::default() },
            accumulation: Accumulation::BuildUp,
            ..new_builtin("pattern.clouds", "Clouds", C::Pattern, 90.0, 0.8)
        },
    ]
}
