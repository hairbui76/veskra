//! Deterministic document fixtures shared by the PNG/PSD export tests (FMT-07..11, CLR-05).
//! Every pixel is generated from closed-form shapes so fixtures are reproducible bit for bit.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use veskra_color::IccProfile;
use veskra_compose::ComposeOptions;
use veskra_core::{BlendMode, BlendSpace, DocumentId, LayerId, RectI};
use veskra_doc::{
    DocColor, Document, GroupContent, LayerKind, LayerProps, MaskGrid, PixelContent, Placement, TileGrid,
};

pub fn color_for(profile: &Arc<IccProfile>, space: BlendSpace) -> DocColor {
    DocColor { profile: profile.id(), icc_bytes: Arc::from(profile.bytes()), assumed: false, blend_space: space }
}

pub fn new_doc(w: u32, h: u32) -> Document {
    Document::new(DocumentId::from_u128(0xD0C), w, h, 144.0, color_for(&IccProfile::srgb(), BlendSpace::GammaEncoded))
        .unwrap()
}

pub fn id(n: u128) -> LayerId {
    LayerId::from_u128(n)
}

/// Straight RGBA8 → premultiplied RGBA16 via the contract conversion.
pub fn px(c: [u8; 4]) -> [u16; 4] {
    let mut d = [0u16; 4];
    veskra_color::convert::straight8_to_premul16(&c, &mut d);
    d
}

/// Grid painted over `r` by `f` (straight RGBA8; alpha 0 pixels are skipped).
pub fn grid(r: RectI, f: impl Fn(i32, i32) -> [u8; 4]) -> TileGrid {
    let mut g = TileGrid::new();
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            let c = f(x, y);
            if c[3] > 0 {
                g.put_pixel(x, y, px(c));
            }
        }
    }
    g
}

/// Anti-aliased disc coverage in [0,1].
pub fn disc(x: i32, y: i32, cx: f64, cy: f64, r: f64) -> f64 {
    let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

/// Anti-aliased thick segment coverage in [0,1].
pub fn segment(x: i32, y: i32, a: (f64, f64), b: (f64, f64), half_width: f64) -> f64 {
    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((px - a.0) * dx + (py - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let d = ((px - a.0 - t * dx).powi(2) + (py - a.1 - t * dy).powi(2)).sqrt();
    (half_width - d + 0.5).clamp(0.0, 1.0)
}

pub fn a8(cov: f64, max: u8) -> u8 {
    (cov * max as f64).round() as u8
}

pub fn props(name: &str, blend: BlendMode, opacity: f32) -> LayerProps {
    let mut p = LayerProps::named(name);
    p.blend = blend;
    p.opacity = opacity;
    p
}

pub fn add_pixels(
    doc: &mut Document,
    n: u128,
    parent: Option<u128>,
    g: TileGrid,
    mask: Option<MaskGrid>,
    pr: LayerProps,
) {
    let kind = LayerKind::Raster(PixelContent { tiles: g, mask, strokes: Vec::new(), raster_edits: Vec::new() });
    add_kind(doc, n, parent, kind, pr);
}

pub fn add_vector_pixels(doc: &mut Document, n: u128, parent: Option<u128>, g: TileGrid, pr: LayerProps) {
    let kind = LayerKind::Vector(PixelContent { tiles: g, mask: None, strokes: Vec::new(), raster_edits: Vec::new() });
    add_kind(doc, n, parent, kind, pr);
}

pub fn add_group(doc: &mut Document, n: u128, parent: Option<u128>, pass_through: bool, pr: LayerProps) {
    let kind = LayerKind::Group(GroupContent { children: Vec::new(), pass_through, collapsed: false });
    add_kind(doc, n, parent, kind, pr);
}

fn add_kind(doc: &mut Document, n: u128, parent: Option<u128>, kind: LayerKind, pr: LayerProps) {
    let placement = match parent {
        None => Placement::Top,
        Some(g) => Placement::IntoGroup { group: id(g), index: None },
    };
    doc.add_layer(id(n), pr, kind, placement).unwrap();
}

// ------------------------------------------------------------------------------------------
// Fixtures
// ------------------------------------------------------------------------------------------

/// 1 layer, opaque gradient fading to transparent on the right.
pub fn simple() -> Document {
    let mut d = new_doc(64, 48);
    let g = grid(RectI::new(0, 0, 64, 48), |x, y| {
        let a = if x < 48 { 255 } else { (255 - (x - 48) * 16).max(0) as u8 };
        [(x * 4) as u8, (y * 5) as u8, 128, a]
    });
    add_pixels(&mut d, 1, None, g, None, props("Background", BlendMode::Normal, 1.0));
    d
}

/// 6 pixel layers + 1 group: alpha, mask, group, clipping, multiply, hidden, Unicode name.
pub fn six_layer() -> Document {
    let mut d = new_doc(256, 192);
    add_pixels(
        &mut d,
        1,
        None,
        grid(RectI::new(8, 8, 248, 184), |_, _| [240, 235, 220, 255]),
        None,
        props("Paper", BlendMode::Normal, 1.0),
    );
    add_pixels(
        &mut d,
        2,
        None,
        grid(RectI::new(30, 30, 150, 150), |x, y| {
            let dd = ((x as f64 + 0.5 - 90.0).powi(2) + (y as f64 + 0.5 - 90.0).powi(2)).sqrt();
            [40, 120, 200, a8((1.0 - dd / 60.0).clamp(0.0, 1.0), 255)]
        }),
        None,
        props("Wash (alpha)", BlendMode::Normal, 0.8),
    );
    let mut mask = MaskGrid::new_empty();
    for y in 50..140 {
        for x in 130..220 {
            mask.set(x, y, ((x - 130) * 255 / 89) as u8);
        }
    }
    add_pixels(
        &mut d,
        3,
        None,
        grid(RectI::new(120, 40, 230, 150), |_, _| [220, 60, 50, 255]),
        Some(mask),
        props("Masked \u{2014} \u{30de}\u{30b9}\u{30af}", BlendMode::Normal, 1.0),
    );
    add_group(&mut d, 10, None, false, props("Shading", BlendMode::Normal, 0.9));
    add_pixels(
        &mut d,
        11,
        Some(10),
        grid(RectI::new(118, 78, 202, 162), |x, y| [90, 200, 90, a8(disc(x, y, 160.0, 120.0, 40.0), 255)]),
        None,
        props("Base shape", BlendMode::Normal, 1.0),
    );
    let mut clip = props("Clip tone", BlendMode::Multiply, 1.0);
    clip.clip_to_below = true;
    add_pixels(&mut d, 12, Some(10), grid(RectI::new(100, 100, 220, 180), |_, _| [120, 80, 200, 200]), None, clip);
    add_pixels(
        &mut d,
        4,
        None,
        grid(RectI::new(20, 120, 140, 180), |_, _| [250, 200, 60, 255]),
        None,
        props("Multiply", BlendMode::Multiply, 0.7),
    );
    let mut hidden = props("Hidden", BlendMode::Normal, 1.0);
    hidden.visible = false;
    add_pixels(&mut d, 5, None, grid(RectI::new(0, 0, 256, 192), |_, _| [255, 0, 0, 255]), None, hidden);
    d
}

/// 12 pixel layers in an ink/flats/shadows structure with pass-through and Screen groups.
pub fn twelve_layer() -> Document {
    let mut d = new_doc(512, 384);
    let full = RectI::new(0, 0, 512, 384);
    add_group(&mut d, 100, None, true, props("Flats", BlendMode::Normal, 1.0));
    add_pixels(
        &mut d,
        1,
        Some(100),
        grid(full, |x, y| [200 - (y / 4) as u8, 220 - (x / 8) as u8, 240, 255]),
        None,
        props("Background flat", BlendMode::Normal, 1.0),
    );
    add_pixels(
        &mut d,
        2,
        Some(100),
        grid(RectI::new(150, 60, 362, 272), |x, y| [245, 205, 170, a8(disc(x, y, 256.0, 166.0, 100.0), 255)]),
        None,
        props("Skin", BlendMode::Normal, 1.0),
    );
    let mut sh = props("Skin shadow", BlendMode::Multiply, 0.75);
    sh.clip_to_below = true;
    add_pixels(
        &mut d,
        3,
        Some(100),
        grid(RectI::new(150, 120, 362, 272), |x, y| [180, 120, 140, a8(disc(x, y, 300.0, 230.0, 90.0), 255)]),
        None,
        sh,
    );
    let mut rim = props("Skin rim", BlendMode::Screen, 0.6);
    rim.clip_to_below = true;
    add_pixels(
        &mut d,
        4,
        Some(100),
        grid(RectI::new(150, 60, 362, 272), |x, y| [255, 240, 200, a8(disc(x, y, 200.0, 110.0, 50.0), 255)]),
        None,
        rim,
    );
    add_pixels(
        &mut d,
        5,
        Some(100),
        grid(RectI::new(140, 40, 372, 140), |x, y| {
            let c = disc(x, y, 256.0, 120.0, 112.0) * (if y < 125 { 1.0 } else { 0.0 });
            [70, 45, 30, a8(c, 255)]
        }),
        None,
        props("Hair", BlendMode::Normal, 1.0),
    );
    add_pixels(
        &mut d,
        6,
        Some(100),
        grid(RectI::new(120, 270, 392, 384), |x, y| {
            let edge = ((x - 120).min(391 - x) as f64 / 3.0).clamp(0.0, 1.0);
            let _ = y;
            [60, 90, 170, a8(edge, 255)]
        }),
        None,
        props("Shirt", BlendMode::Normal, 1.0),
    );
    add_group(&mut d, 200, None, false, props("Lighting", BlendMode::Screen, 0.85));
    add_pixels(
        &mut d,
        7,
        Some(200),
        grid(RectI::new(0, 0, 300, 250), |x, y| {
            let dd = ((x as f64 - 120.0).powi(2) + (y as f64 - 90.0).powi(2)).sqrt();
            [255, 230, 160, a8((1.0 - dd / 140.0).clamp(0.0, 1.0), 200)]
        }),
        None,
        props("Highlight", BlendMode::Normal, 1.0),
    );
    add_pixels(
        &mut d,
        8,
        Some(200),
        grid(RectI::new(380, 20, 500, 140), |x, y| [180, 220, 255, a8(disc(x, y, 440.0, 80.0, 55.0), 160)]),
        None,
        props("Highlight 2", BlendMode::Normal, 1.0),
    );
    add_pixels(
        &mut d,
        9,
        None,
        grid(full, |x, _| [(x / 2) as u8, 64, 160, 96]),
        None,
        props("Overlay tint", BlendMode::Overlay, 1.0),
    );
    let ink = |x: i32, y: i32| -> f64 {
        segment(x, y, (150.0, 170.0), (360.0, 160.0), 2.5)
            .max(segment(x, y, (256.0, 66.0), (256.0, 270.0), 1.5))
            .max(segment(x, y, (130.0, 280.0), (390.0, 300.0), 3.0))
    };
    add_vector_pixels(
        &mut d,
        10,
        None,
        grid(full, |x, y| [20, 18, 25, a8(ink(x, y), 255)]),
        props("Ink", BlendMode::Normal, 1.0),
    );
    add_pixels(
        &mut d,
        11,
        None,
        grid(RectI::new(200, 180, 320, 220), |x, y| {
            [200, 30, 60, a8(segment(x, y, (210.0, 200.0), (310.0, 200.0), 6.0), 255)]
        }),
        None,
        props("Ink color", BlendMode::Normal, 0.9),
    );
    let mut notes = props("Notes", BlendMode::Normal, 1.0);
    notes.visible = false;
    add_pixels(&mut d, 12, None, grid(RectI::new(10, 10, 100, 40), |_, _| [255, 0, 255, 255]), None, notes);
    d
}

/// Empty layer, empty group, and a layer whose mask defaults to 255 (inverse mask with a hole).
pub fn empty_and_inverse_mask() -> Document {
    let mut d = new_doc(48, 40);
    add_pixels(&mut d, 1, None, TileGrid::new(), None, props("Empty", BlendMode::Normal, 1.0));
    add_pixels(
        &mut d,
        2,
        None,
        grid(RectI::new(4, 4, 20, 20), |x, y| [10, 200, 120, a8(disc(x, y, 12.0, 12.0, 7.0), 255)]),
        None,
        props("Dot", BlendMode::Normal, 1.0),
    );
    let mut m = MaskGrid::new_full();
    for y in 10..34 {
        for x in 18..42 {
            m.set(x, y, 255 - a8(disc(x, y, 30.0, 22.0, 9.0), 255));
        }
    }
    add_pixels(
        &mut d,
        3,
        None,
        grid(RectI::new(0, 0, 48, 40), |x, y| [(x * 5) as u8, 90, (y * 6) as u8, 200]),
        Some(m),
        props("Inverse mask", BlendMode::Normal, 1.0),
    );
    add_group(&mut d, 4, None, false, props("Empty group", BlendMode::Normal, 1.0));
    d
}

/// 2048×1536 with sparse strokes, a far corner dot and off-canvas content.
pub fn large_sparse() -> Document {
    let mut d = new_doc(2048, 1536);
    let lines: [((f64, f64), (f64, f64)); 3] =
        [((100.0, 200.0), (1900.0, 300.0)), ((300.0, 1400.0), (1200.0, 900.0)), ((1700.0, 100.0), (1750.0, 1450.0))];
    for (i, (a, b)) in lines.iter().enumerate() {
        let r = RectI::new(
            (a.0.min(b.0) - 8.0) as i32,
            (a.1.min(b.1) - 8.0) as i32,
            (a.0.max(b.0) + 8.0) as i32,
            (a.1.max(b.1) + 8.0) as i32,
        );
        let col = [[30u8, 30, 40], [200, 60, 40], [40, 140, 220]][i];
        let g = grid(r, |x, y| [col[0], col[1], col[2], a8(segment(x, y, *a, *b, 3.0), 255)]);
        add_pixels(&mut d, 1 + i as u128, None, g, None, props(&format!("Stroke {}", i + 1), BlendMode::Normal, 1.0));
    }
    add_pixels(
        &mut d,
        10,
        None,
        grid(RectI::new(1980, 1470, 2048, 1536), |x, y| [250, 180, 0, a8(disc(x, y, 2010.0, 1500.0, 25.0), 255)]),
        None,
        props("Corner", BlendMode::Multiply, 1.0),
    );
    add_pixels(
        &mut d,
        11,
        None,
        grid(RectI::new(-90, 630, 50, 770), |x, y| [120, 0, 160, a8(disc(x, y, -20.0, 700.0, 60.0), 230)]),
        None,
        props("Off-canvas", BlendMode::Normal, 1.0),
    );
    d
}

/// All fixtures by name.
pub fn all() -> Vec<(&'static str, Document)> {
    vec![
        ("simple", simple()),
        ("six_layer", six_layer()),
        ("twelve_layer", twelve_layer()),
        ("empty_and_inverse_mask", empty_and_inverse_mask()),
        ("large_sparse", large_sparse()),
    ]
}

// ------------------------------------------------------------------------------------------
// Expected values
// ------------------------------------------------------------------------------------------

/// Veskra composite (premultiplied RGBA16) of the whole canvas.
pub fn composite16(doc: &Document) -> Vec<u16> {
    veskra_compose::composite_rect(doc, doc.bounds(), &ComposeOptions::default())
}

/// Veskra composite as straight RGBA8 via the contract conversion.
pub fn composite8(doc: &Document) -> Vec<u8> {
    let c = composite16(doc);
    let mut out = vec![0u8; c.len()];
    veskra_color::convert::premul16_to_straight8(&c, &mut out);
    out
}

/// Interop output directory (`$CARGO_TARGET_DIR` or `<repo>/target`) + `/interop/<sub>`.
pub fn interop_dir(sub: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let d = base.join("interop").join(sub);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}

/// Whether `python3 -c <probe>` succeeds.
pub fn python_has(probe: &str) -> bool {
    std::process::Command::new("python3").args(["-c", probe]).output().map(|o| o.status.success()).unwrap_or(false)
}
