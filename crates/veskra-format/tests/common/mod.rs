//! Shared builders for veskra-format integration tests. All content is synthetic (generated
//! here), no third-party artwork.
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use sha2::{Digest, Sha256};
use std::sync::Arc;
use veskra_color::IccProfile;
use veskra_core::{
    BlendMode, BlendSpace, ColorRgba, DocumentId, LayerId, PointF, ProfileId, RectI, SampleFlags, StrokeId,
    StrokeSample,
};
use veskra_doc::{
    DocColor, Document, GroupContent, LayerKind, LayerProps, PixelContent, Placement, StrokeComposition,
    StrokeGeometry, StrokeMode, StrokeRecord,
};
use veskra_format::SaveMeta;
use veskra_format::zip::{Method, ZipReader, ZipWriter};
use veskra_raster::{MaskGrid, TileCoord};
use veskra_testkit::Rng;
use veskra_vector::{BezPath, PathEl};

pub fn meta() -> SaveMeta {
    SaveMeta::new("veskra-test 0.1.0", "2026-10-09T00:00:00Z")
}

pub fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn srgb_color() -> DocColor {
    let p = IccProfile::srgb();
    DocColor { profile: p.id(), icc_bytes: Arc::from(p.bytes()), assumed: false, blend_space: BlendSpace::GammaEncoded }
}

/// Display P3 bytes under a custom (non-sRGB) semantic name.
pub fn p3_color() -> DocColor {
    let p = IccProfile::display_p3();
    let bytes = p.bytes().to_vec();
    DocColor {
        profile: ProfileId::new(sha(&bytes), "Display P3 (Veskra test)"),
        icc_bytes: Arc::from(bytes),
        assumed: true,
        blend_space: BlendSpace::Linear,
    }
}

pub fn lid(n: u128) -> LayerId {
    LayerId::from_u128(0x1000 + n)
}
pub fn sid(n: u128) -> StrokeId {
    StrokeId::from_u128(0x2000 + n)
}

fn premul(r: u16, g: u16, b: u16, a: u16) -> [u16; 4] {
    let m = |c: u16| ((c as u32 * a as u32 + 32767) / 65535) as u16;
    [m(r), m(g), m(b), a]
}

/// Smooth, well-compressing content: a soft disc of `color` inside tile `c`.
pub fn paint_disc(doc: &mut Document, layer: LayerId, c: TileCoord, color: [u16; 3]) {
    let px = doc.layer_mut(layer).unwrap().pixels_mut().unwrap();
    let t = px.tiles.tile_mut(c);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let d = (((x as f32 - 31.5).powi(2) + (y as f32 - 31.5).powi(2)).sqrt() / 32.0).min(1.0);
            let a = (((1.0 - d) * 255.0).round() as u16) * 257;
            t.set(x, y, premul(color[0], color[1], color[2], a));
        }
    }
}

/// Noisy content (stress premultiplied invariant and compression), seeded.
pub fn paint_noise(doc: &mut Document, layer: LayerId, c: TileCoord, seed: u64) {
    let mut rng = Rng::new(seed);
    let px = doc.layer_mut(layer).unwrap().pixels_mut().unwrap();
    let t = px.tiles.tile_mut(c);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let a = rng.below(65536) as u16;
            let ch = |r: &mut Rng| (r.below(a as u64 + 1)) as u16;
            let p = [ch(&mut rng), ch(&mut rng), ch(&mut rng), a];
            t.set(x, y, p);
        }
    }
}

pub fn samples(n: usize, x0: f32, y0: f32, axes: bool) -> Vec<StrokeSample> {
    (0..n)
        .map(|i| {
            let f = i as f32;
            let mut s = StrokeSample::confirmed(
                x0 + f * 3.25,
                y0 + (f * 0.4).sin() * 9.0,
                1_000_000 + i as u64 * 4_166_667,
                (0.2 + f * 0.013).min(1.0),
            );
            if axes {
                s.tilt = Some(0.1 + f * 0.001);
                s.azimuth = if i % 3 == 0 { None } else { Some(-1.5 + f * 0.01) };
                s.rotation = if i % 2 == 0 { Some(0.25) } else { None };
                s.flags = SampleFlags::CONFIRMED | SampleFlags::PRESSURE_REPORTED | SampleFlags::HISTORICAL;
            }
            s
        })
        .collect()
}

pub fn geometry(x0: f64, y0: f64) -> StrokeGeometry {
    let p = |x: f64, y: f64| PointF::new(x0 + x, y0 + y);
    StrokeGeometry {
        centerline: BezPath {
            els: vec![
                PathEl::MoveTo(p(0.0, 0.0)),
                PathEl::CubicTo(p(10.1, 3.3), p(20.7, -4.1), p(31.000000000000004, 0.1)),
            ],
        },
        outline: BezPath {
            els: vec![
                PathEl::MoveTo(p(0.0, -1.0)),
                PathEl::LineTo(p(31.0, -1.5)),
                PathEl::CubicTo(p(32.0, -1.0), p(32.0, 1.0), p(31.0, 1.5)),
                PathEl::LineTo(p(0.0, 1.0)),
                PathEl::Close,
            ],
        },
        fit_error: 0.123456789012345,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn stroke(
    id: StrokeId,
    layer: LayerId,
    n: usize,
    color: ColorRgba,
    mode: StrokeMode,
    geom: Option<StrokeGeometry>,
    profile: ProfileId,
    axes: bool,
) -> StrokeRecord {
    StrokeRecord::new(
        id,
        layer,
        samples(n, 10.0, 20.0, axes),
        "pen.technical",
        3,
        r#"{"size":4.5,"taper":true}"#.into(),
        color,
        0xDEAD_BEEF_0000_0000 + id.as_u128() as u64,
        profile,
        StrokeComposition { mode, opacity: 0.85, alpha_locked: false, selection_clipped: false },
        geom.is_some(),
        geom,
        RectI::new(5, 5, 120, 60),
    )
    .unwrap()
}

fn raster() -> LayerKind {
    LayerKind::Raster(PixelContent::empty())
}
fn group(pass_through: bool) -> LayerKind {
    LayerKind::Group(GroupContent { children: vec![], pass_through, collapsed: false })
}

/// Rich document for SAV-001: groups (isolated + pass-through, nested), clipping, masks, raster +
/// vector layers, strokes with geometry and optional axes, selection, locked/hidden/blend modes,
/// non-sRGB profile, negative tile coordinates and noisy pixels.
pub fn rich_document() -> Document {
    let color = p3_color();
    let prof = color.profile.clone();
    let mut d = Document::new(DocumentId::from_u128(0xD0C), 300, 200, 300.0, color).unwrap();
    let named = |n: &str| LayerProps::named(n);
    d.add_layer(lid(1), named("Background"), raster(), Placement::Top).unwrap();
    d.add_layer(lid(2), named("Inks"), group(false), Placement::Top).unwrap();
    d.add_layer(
        lid(3),
        named("Lines"),
        LayerKind::Vector(PixelContent::empty()),
        Placement::IntoGroup { group: lid(2), index: None },
    )
    .unwrap();
    d.add_layer(lid(4), named("Shading ✓ ünïcode"), raster(), Placement::IntoGroup { group: lid(2), index: None })
        .unwrap();
    d.add_layer(lid(5), named("FX"), group(true), Placement::Top).unwrap();
    d.add_layer(lid(6), named("Glow"), raster(), Placement::IntoGroup { group: lid(5), index: None }).unwrap();
    d.add_layer(lid(7), named("Nested"), group(false), Placement::IntoGroup { group: lid(5), index: None }).unwrap();
    d.add_layer(lid(8), named("Deep"), raster(), Placement::IntoGroup { group: lid(7), index: None }).unwrap();
    d.add_layer(lid(9), named("Top"), raster(), Placement::Top).unwrap();

    paint_disc(&mut d, lid(1), TileCoord::new(0, 0), [65535, 40000, 1000]);
    paint_disc(&mut d, lid(1), TileCoord::new(4, 3), [100, 2000, 65535]);
    paint_noise(&mut d, lid(4), TileCoord::new(1, 1), 7);
    paint_disc(&mut d, lid(3), TileCoord::new(0, 0), [0, 0, 0]);
    paint_disc(&mut d, lid(6), TileCoord::new(2, 1), [65535, 65535, 0]);
    paint_disc(&mut d, lid(8), TileCoord::new(1, 0), [0, 30000, 30000]);
    paint_disc(&mut d, lid(9), TileCoord::new(-1, -1), [5000, 5000, 5000]);
    // A materialized but transparent tile (must compare equal after round-trip).
    d.layer_mut(lid(9)).unwrap().pixels_mut().unwrap().tiles.tile_mut(TileCoord::new(3, 2));

    d.add_stroke(stroke(
        sid(1),
        lid(3),
        40,
        ColorRgba::BLACK,
        StrokeMode::Paint,
        Some(geometry(10.0, 20.0)),
        prof.clone(),
        true,
    ))
    .unwrap();
    d.add_stroke(stroke(
        sid(2),
        lid(3),
        7,
        ColorRgba::new(0.2, 0.4, 0.6, 1.0),
        StrokeMode::Paint,
        Some(geometry(40.0, 80.0)),
        prof.clone(),
        false,
    ))
    .unwrap();
    d.add_stroke(stroke(sid(3), lid(9), 25, ColorRgba::WHITE, StrokeMode::Erase, None, IccProfile::srgb().id(), true))
        .unwrap();
    d.add_stroke(stroke(sid(4), lid(4), 1, ColorRgba::new(1.0, 0.0, 0.5, 0.25), StrokeMode::Paint, None, prof, false))
        .unwrap();
    d.mark_raster_edit(lid(1), "fill").unwrap();
    d.mark_raster_edit(lid(1), "transform").unwrap();

    // Layer mask: full default with a soft partial tile and a zero tile.
    let mut mask = MaskGrid::new_full();
    for y in 0..64 {
        for x in 0..64 {
            mask.set(64 + x, 64 + y, (x * 4) as u8);
        }
    }
    let zero = mask.tile_mut(TileCoord::new(0, 0));
    zero.data_mut().fill(0);
    {
        let l = d.layer_mut(lid(4)).unwrap();
        l.pixels_mut().unwrap().mask = Some(mask);
        l.props.clip_to_below = true;
        l.props.blend = BlendMode::Multiply;
        l.props.opacity = 0.6;
        l.props.alpha_locked = true;
    }
    {
        let l = d.layer_mut(lid(6)).unwrap();
        l.props.blend = BlendMode::Screen;
        l.props.visible = false;
    }
    d.layer_mut(lid(8)).unwrap().props.locked = true;
    d.layer_mut(lid(9)).unwrap().props.blend = BlendMode::Overlay;
    if let LayerKind::Group(g) = &mut d.layer_mut(lid(5)).unwrap().kind {
        g.collapsed = true;
    }
    let mut sel = MaskGrid::new_rect(RectI::new(10, 10, 150, 90));
    sel.set(5, 5, 128);
    d.selection = Some(sel);
    d.active_layer = Some(lid(3));
    d.validate().unwrap();
    d
}

/// One raster layer, sRGB, one soft disc.
pub fn simple_document() -> Document {
    let mut d = Document::new(DocumentId::from_u128(0x51), 128, 128, 144.0, srgb_color()).unwrap();
    d.add_layer(lid(1), LayerProps::named("Layer 1"), raster(), Placement::Top).unwrap();
    paint_disc(&mut d, lid(1), TileCoord::new(0, 0), [60000, 10000, 20000]);
    let prof = d.color.profile.clone();
    d.add_stroke(stroke(sid(1), lid(1), 12, ColorRgba::new(0.9, 0.15, 0.3, 1.0), StrokeMode::Paint, None, prof, true))
        .unwrap();
    d.active_layer = Some(lid(1));
    d
}

/// Synthetic 12-layer illustration (10 pixel layers + 2 groups), small and compressible.
pub fn twelve_layer_document() -> Document {
    let mut d = Document::new(DocumentId::from_u128(0x12), 256, 256, 300.0, srgb_color()).unwrap();
    let prof = d.color.profile.clone();
    d.add_layer(lid(100), LayerProps::named("Characters"), group(false), Placement::Top).unwrap();
    d.add_layer(lid(101), LayerProps::named("Effects"), group(true), Placement::Top).unwrap();
    let modes = [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay];
    for i in 0..10u128 {
        let placement = match i {
            0..=3 => Placement::Top,
            4..=7 => Placement::IntoGroup { group: lid(100), index: None },
            _ => Placement::IntoGroup { group: lid(101), index: None },
        };
        let kind = if i % 4 == 1 { LayerKind::Vector(PixelContent::empty()) } else { raster() };
        let mut props = LayerProps::named(format!("Layer {i}"));
        props.blend = modes[i as usize % 4];
        props.opacity = 1.0 - i as f32 * 0.05;
        props.clip_to_below = i == 6;
        d.add_layer(lid(i), props, kind, placement).unwrap();
        let c = TileCoord::new((i % 4) as i32, (i / 4) as i32);
        paint_disc(&mut d, lid(i), c, [(i as u16) * 6000, 65535 - (i as u16) * 5000, 30000]);
        let geom = (i % 4 == 1).then(|| geometry(i as f64 * 20.0, 30.0));
        d.add_stroke(stroke(
            sid(i),
            lid(i),
            10 + i as usize,
            ColorRgba::BLACK,
            StrokeMode::Paint,
            geom,
            prof.clone(),
            i % 2 == 0,
        ))
        .unwrap();
    }
    d.selection = Some(MaskGrid::new_rect(RectI::new(0, 0, 100, 100)));
    d.active_layer = Some(lid(5));
    d
}

/// All entries of a bundle (name, uncompressed bytes, method) in archive order.
pub fn unpack(bundle: &[u8]) -> Vec<(String, Vec<u8>, Method)> {
    let z = ZipReader::parse(bundle).unwrap();
    z.entries().iter().map(|e| (e.name.clone(), z.read(e).unwrap(), e.method)).collect()
}

/// Re-pack entries with the crate's writer, recomputing `integrity.json` when `fix_integrity`.
pub fn repack(entries: &[(String, Vec<u8>, Method)], fix_integrity: bool) -> Vec<u8> {
    let mut w = ZipWriter::new(Vec::new());
    let mut list = Vec::new();
    for (n, d, m) in entries {
        if n == "integrity.json" && fix_integrity {
            continue;
        }
        w.add(n, d, *m).unwrap();
        list.push(serde_json::json!({"name": n, "size": d.len(), "sha256": hex(&sha(d))}));
    }
    if fix_integrity {
        list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        let j = serde_json::to_vec(&serde_json::json!({"algorithm": "sha256", "entries": list})).unwrap();
        w.add("integrity.json", &j, Method::Deflated).unwrap();
    }
    w.finish().unwrap().0
}

/// Apply `f` to a bundle's entries and repack with fixed integrity.
pub fn tamper(bundle: &[u8], f: impl FnOnce(&mut Vec<(String, Vec<u8>, Method)>)) -> Vec<u8> {
    let mut e = unpack(bundle);
    f(&mut e);
    repack(&e, true)
}

/// Edit a JSON entry in place.
pub fn edit_json(entries: &mut [(String, Vec<u8>, Method)], name: &str, f: impl FnOnce(&mut serde_json::Value)) {
    let e = entries.iter_mut().find(|e| e.0 == name).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&e.1).unwrap();
    f(&mut v);
    e.1 = serde_json::to_vec_pretty(&v).unwrap();
}

/// Hand-built ZIP (independent of the crate's writer) allowing hostile names and lying sizes.
/// Each entry: (name bytes, stored payload, method code, crc, declared uncompressed size).
pub struct RawEntry {
    pub name: Vec<u8>,
    pub payload: Vec<u8>,
    pub method: u16,
    pub crc: u32,
    pub usize: u32,
}

impl RawEntry {
    pub fn stored(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            payload: data.to_vec(),
            method: 0,
            crc: crc32fast::hash(data),
            usize: data.len() as u32,
        }
    }
    pub fn deflated(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            payload: miniz_oxide::deflate::compress_to_vec(data, 6),
            method: 8,
            crc: crc32fast::hash(data),
            usize: data.len() as u32,
        }
    }
}

pub fn raw_zip(entries: &[RawEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for e in entries {
        let off = out.len() as u32;
        let hdr = |v: &mut Vec<u8>, central: bool| {
            v.extend_from_slice(&(if central { 0x0201_4b50u32 } else { 0x0403_4b50 }).to_le_bytes());
            if central {
                v.extend_from_slice(&20u16.to_le_bytes());
            }
            for x in [20u16, 0x0800, e.method, 0, 0x21] {
                v.extend_from_slice(&x.to_le_bytes());
            }
            v.extend_from_slice(&e.crc.to_le_bytes());
            v.extend_from_slice(&(e.payload.len() as u32).to_le_bytes());
            v.extend_from_slice(&e.usize.to_le_bytes());
            v.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            if central {
                for _ in 0..3 {
                    // comment length, disk start, internal attributes
                    v.extend_from_slice(&0u16.to_le_bytes());
                }
                v.extend_from_slice(&0u32.to_le_bytes());
                v.extend_from_slice(&off.to_le_bytes());
            }
            v.extend_from_slice(&e.name);
        };
        hdr(&mut out, false);
        out.extend_from_slice(&e.payload);
        hdr(&mut cd, true);
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    for x in [0u16, 0, entries.len() as u16, entries.len() as u16] {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
