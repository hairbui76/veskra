//! Shared fixture builders for veskra-compose integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use veskra_core::{BlendMode, BlendSpace, ColorRgba, DocumentId, LayerId, ProfileId, RectI};
use veskra_doc::{
    DocColor, Document, GroupContent, LayerKind, LayerProps, MaskGrid, PixelContent, Placement, TileGrid,
};

pub type Px = [u16; 4];

pub fn color(space: BlendSpace) -> DocColor {
    DocColor {
        profile: ProfileId::new([7; 32], "sRGB test"),
        icc_bytes: Arc::from(&b"test-icc"[..]),
        assumed: false,
        blend_space: space,
    }
}

pub fn doc(w: u32, h: u32) -> Document {
    Document::new(DocumentId::from_u128(1), w, h, 72.0, color(BlendSpace::GammaEncoded)).unwrap()
}

pub fn doc_space(w: u32, h: u32, space: BlendSpace) -> Document {
    Document::new(DocumentId::from_u128(1), w, h, 72.0, color(space)).unwrap()
}

pub fn id(n: u128) -> LayerId {
    LayerId::from_u128(n)
}

/// Straight 8-bit RGBA → premultiplied RGBA16 (contract conversion).
pub fn px8(r: u8, g: u8, b: u8, a: u8) -> Px {
    let mut d = [0u16; 4];
    veskra_color::convert::straight8_to_premul16(&[r, g, b, a], &mut d);
    d
}

pub fn pxf(c: ColorRgba) -> Px {
    veskra_color::convert::straight_f32_to_premul16(c)
}

pub fn grid_fill(r: RectI, p: Px) -> TileGrid {
    let mut g = TileGrid::new();
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            g.put_pixel(x, y, p);
        }
    }
    g
}

pub fn props(name: &str, blend: BlendMode, opacity: f32) -> LayerProps {
    let mut p = LayerProps::named(name);
    p.blend = blend;
    p.opacity = opacity;
    p
}

pub fn raster(tiles: TileGrid, mask: Option<MaskGrid>) -> LayerKind {
    LayerKind::Raster(PixelContent { tiles, mask, strokes: Vec::new(), raster_edits: Vec::new() })
}

pub fn group(pass_through: bool) -> LayerKind {
    LayerKind::Group(GroupContent { children: Vec::new(), pass_through, collapsed: false })
}

/// Add a raster layer filled with `p` over `r` on top of `parent` (root if `None`).
pub fn add_fill(doc: &mut Document, n: u128, parent: Option<u128>, r: RectI, p: Px, pr: LayerProps) -> LayerId {
    add_grid(doc, n, parent, grid_fill(r, p), None, pr)
}

pub fn add_grid(
    doc: &mut Document,
    n: u128,
    parent: Option<u128>,
    g: TileGrid,
    mask: Option<MaskGrid>,
    pr: LayerProps,
) -> LayerId {
    let place = parent.map_or(Placement::Top, |g| Placement::IntoGroup { group: id(g), index: None });
    doc.add_layer(id(n), pr, raster(g, mask), place).unwrap();
    id(n)
}

pub fn add_group(doc: &mut Document, n: u128, parent: Option<u128>, pass_through: bool, pr: LayerProps) -> LayerId {
    let place = parent.map_or(Placement::Top, |g| Placement::IntoGroup { group: id(g), index: None });
    doc.add_layer(id(n), pr, group(pass_through), place).unwrap();
    id(n)
}

pub fn set_clip(doc: &mut Document, n: u128, clip: bool) {
    doc.layer_mut(id(n)).unwrap().props.clip_to_below = clip;
}

pub fn set_visible(doc: &mut Document, n: u128, v: bool) {
    doc.layer_mut(id(n)).unwrap().props.visible = v;
}

/// Pixel `(x, y)` of a composite buffer covering `r`.
pub fn at(buf: &[u16], r: RectI, x: i32, y: i32) -> Px {
    let i = ((y - r.y0) as usize * r.width() as usize + (x - r.x0) as usize) * 4;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

pub fn assert_premul(buf: &[u16]) {
    for p in buf.chunks_exact(4) {
        assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "premultiplied invariant violated: {p:?}");
    }
}

/// Independent f64 reference of the W3C general formula on premultiplied u16 inputs, using the
/// *straight* formulation `Cr = (1−αb)·Cs + αb·B(Cb,Cs)`, `co = αs·Cr + (1−αs)·αb·Cb`.
pub fn ref_blend(b: [f64; 4], s: [f64; 4], mode: BlendMode) -> [f64; 4] {
    let (ab, as_) = (b[3], s[3]);
    let ao = as_ + ab * (1.0 - as_);
    let mut o = [0.0; 4];
    for i in 0..3 {
        let cb = if ab > 0.0 { b[i] / ab } else { 0.0 };
        let cs = if as_ > 0.0 { s[i] / as_ } else { 0.0 };
        let bf = match mode {
            BlendMode::Multiply => cb * cs,
            BlendMode::Screen => cb + cs - cb * cs,
            BlendMode::Overlay => {
                if cb <= 0.5 {
                    2.0 * cb * cs
                } else {
                    let t = 2.0 * cb - 1.0;
                    t + cs - t * cs
                }
            }
            _ => cs,
        };
        let cr = (1.0 - ab) * cs + ab * bf;
        o[i] = as_ * cr + (1.0 - as_) * ab * cb;
    }
    o[3] = ao;
    o
}

pub fn to_f(p: Px) -> [f64; 4] {
    p.map(|v| v as f64 / 65535.0)
}

pub fn q16(v: [f64; 4]) -> Px {
    v.map(|c| (c.clamp(0.0, 1.0) * 65535.0 + 0.5).floor() as u16)
}

pub fn scale(p: [f64; 4], k: f64) -> [f64; 4] {
    p.map(|c| c * k)
}

pub fn assert_close(got: Px, want: Px, tol: u16, what: &str) {
    for i in 0..4 {
        assert!(got[i].abs_diff(want[i]) <= tol, "{what}: got {got:?}, want {want:?} (tol {tol})");
    }
}
