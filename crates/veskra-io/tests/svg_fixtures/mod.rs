//! Shared document builders for the SVG/PDF/plan integration tests (FMT-B). Not a test target.
//!
//! Strokes are built the way the engine commits them: the layer tiles hold the rendered pixels
//! (authoritative appearance) and the stroke record holds confirmed samples plus, for
//! vector-eligible brushes, the variable-width outline. Ink outlines are rasterized with
//! `veskra_raster::select::polygons` (anti-aliased, nonzero) so the raster and the vector form
//! describe the same shape.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use veskra_color::IccProfile;
use veskra_core::{
    BlendMode, BlendSpace, ColorRgba, DocumentId, LayerId, PointF, RectF, RectI, StrokeId, StrokeSample,
};
use veskra_doc::{
    DocColor, Document, GroupContent, LayerKind, LayerPatch, PixelContent, Placement, StrokeComposition,
    StrokeGeometry, StrokeMode, StrokeRecord,
};
use veskra_raster::ops::{CompositeMode, composite_mask_color};
use veskra_vector::{BezPath, OutlineOptions, WidthPoint, variable_width_outline};

pub fn srgb_color() -> DocColor {
    let p = IccProfile::srgb();
    DocColor { profile: p.id(), icc_bytes: Arc::from(p.bytes()), assumed: false, blend_space: BlendSpace::GammaEncoded }
}

#[derive(Debug)]
pub struct DocBuilder {
    pub doc: Document,
    next: u128,
    t: u64,
}

impl DocBuilder {
    pub fn new(w: u32, h: u32, dpi: f32) -> Self {
        let doc = Document::new(DocumentId::from_u128(0xD0C), w, h, dpi, srgb_color()).unwrap();
        Self { doc, next: 1, t: 1 }
    }

    fn id(&mut self) -> u128 {
        self.next += 1;
        self.next
    }

    pub fn raster(&mut self, name: &str, at: Placement) -> LayerId {
        let id = LayerId::from_u128(self.id());
        self.doc
            .add_layer(id, veskra_doc::LayerProps::named(name), LayerKind::Raster(PixelContent::empty()), at)
            .unwrap();
        id
    }

    pub fn vector(&mut self, name: &str, at: Placement) -> LayerId {
        let id = LayerId::from_u128(self.id());
        self.doc
            .add_layer(id, veskra_doc::LayerProps::named(name), LayerKind::Vector(PixelContent::empty()), at)
            .unwrap();
        id
    }

    pub fn group(&mut self, name: &str, at: Placement) -> LayerId {
        let id = LayerId::from_u128(self.id());
        let g = GroupContent { children: vec![], pass_through: false, collapsed: false };
        self.doc.add_layer(id, veskra_doc::LayerProps::named(name), LayerKind::Group(g), at).unwrap();
        id
    }

    pub fn patch(&mut self, id: LayerId, p: LayerPatch) {
        self.doc.apply_patch(id, &p).unwrap();
    }

    pub fn opacity(&mut self, id: LayerId, o: f32) {
        self.patch(id, LayerPatch { opacity: Some(o), ..Default::default() });
    }
    pub fn blend(&mut self, id: LayerId, b: BlendMode) {
        self.patch(id, LayerPatch { blend: Some(b), ..Default::default() });
    }
    pub fn hide(&mut self, id: LayerId) {
        self.patch(id, LayerPatch { visible: Some(false), ..Default::default() });
    }
    pub fn clip(&mut self, id: LayerId) {
        self.patch(id, LayerPatch { clip_to_below: Some(true), ..Default::default() });
    }

    fn samples(&mut self, pts: &[WidthPoint]) -> Vec<StrokeSample> {
        let maxw = pts.iter().map(|p| p.width).fold(1e-6, f64::max);
        pts.iter()
            .map(|p| {
                self.t += 8_000_000;
                StrokeSample::confirmed(p.p.x as f32, p.p.y as f32, self.t, (p.width / maxw).clamp(0.0, 1.0) as f32)
            })
            .collect()
    }

    /// Pressure-tapered ink stroke with a vector form. Pixels = AA rasterization of its outline.
    pub fn ink(&mut self, layer: LayerId, pts: &[WidthPoint], color: ColorRgba, opacity: f32) -> StrokeId {
        let outline = variable_width_outline(pts, &OutlineOptions::default());
        self.paint_path(layer, &outline, color, opacity);
        let samples = self.samples(pts);
        let sid = StrokeId::from_u128(self.id());
        let centerline = {
            let mut c = BezPath::new();
            for (i, p) in pts.iter().enumerate() {
                if i == 0 { c.move_to(p.p) } else { c.line_to(p.p) }
            }
            c
        };
        let bounds = RectI::covering(&outline.bounds());
        let rec = StrokeRecord::new(
            sid,
            layer,
            samples,
            "pen.ink",
            1,
            "{}".into(),
            color,
            7,
            IccProfile::srgb().id(),
            StrokeComposition { mode: StrokeMode::Paint, opacity, alpha_locked: false, selection_clipped: false },
            true,
            Some(StrokeGeometry { centerline, outline, fit_error: 0.0 }),
            bounds,
        )
        .unwrap();
        self.doc.add_stroke(rec).unwrap();
        sid
    }

    pub fn paint_path(&mut self, layer: LayerId, path: &BezPath, color: ColorRgba, opacity: f32) {
        let polys = path.flatten(0.05);
        let refs: Vec<&[PointF]> = polys.iter().map(|p| p.as_slice()).collect();
        let bounds = self.doc.bounds();
        let mask = veskra_raster::select::polygons(&refs, bounds);
        let px = self.doc.pixels_mut_editable(layer).unwrap();
        composite_mask_color(&mut px.tiles, &mask, color, opacity, CompositeMode::Over, Some(bounds));
    }

    /// Raster-only "airbrush/texture" stroke: a speckled band of dots, no vector form.
    pub fn texture(&mut self, layer: LayerId, rect: RectI, color: ColorRgba, seed: u64) -> StrokeId {
        let mut rng = veskra_testkit::Rng::new(seed);
        let n = (rect.area() / 12).max(1);
        let px = self.doc.pixels_mut_editable(layer).unwrap();
        for _ in 0..n {
            let x = rect.x0 + rng.below(rect.width() as u64) as i32;
            let y = rect.y0 + rng.below(rect.height() as u64) as i32;
            let a = 0.25 + 0.75 * rng.f64() as f32;
            let src = veskra_raster::ops::color_to_px16(color.with_alpha(color.a * a));
            let dst = px.tiles.get_pixel(x, y);
            px.tiles.put_pixel(x, y, veskra_raster::ops::src_over(dst, src));
        }
        let pts = [
            WidthPoint::new(rect.x0 as f64, rect.y0 as f64, 10.0),
            WidthPoint::new(rect.x1 as f64, rect.y1 as f64, 10.0),
        ];
        let samples = self.samples(&pts);
        let sid = StrokeId::from_u128(self.id());
        let rec = StrokeRecord::new(
            sid,
            layer,
            samples,
            "airbrush.soft",
            1,
            "{}".into(),
            color,
            seed,
            IccProfile::srgb().id(),
            StrokeComposition { mode: StrokeMode::Paint, opacity: 1.0, alpha_locked: false, selection_clipped: false },
            false,
            None,
            rect,
        )
        .unwrap();
        self.doc.add_stroke(rec).unwrap();
        sid
    }

    /// Bucket-fill style raster edit (makes the layer raster-authoritative).
    pub fn fill(&mut self, layer: LayerId, rect: RectI, color: ColorRgba) {
        let r = RectF::new(rect.x0 as f64, rect.y0 as f64, rect.x1 as f64, rect.y1 as f64);
        let mask = veskra_raster::select::rect(r, self.doc.bounds());
        let px = self.doc.pixels_mut_editable(layer).unwrap();
        composite_mask_color(&mut px.tiles, &mask, color, 1.0, CompositeMode::Over, None);
        self.doc.mark_raster_edit(layer, "fill").unwrap();
    }

    pub fn finish(self) -> Document {
        self.doc.validate().unwrap();
        self.doc
    }
}

/// A tapered S-curve from (x0,y0) to (x1,y1) with pressure ramp up/down.
pub fn taper(x0: f64, y0: f64, x1: f64, y1: f64, max_w: f64) -> Vec<WidthPoint> {
    let n = 24;
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let x = x0 + (x1 - x0) * t;
            let y = y0 + (y1 - y0) * t + (t * std::f64::consts::TAU).sin() * 0.08 * (x1 - x0).abs().max(1.0);
            let w = max_w * (0.15 + 0.85 * (t * std::f64::consts::PI).sin());
            WidthPoint::new(x, y, w)
        })
        .collect()
}

pub fn rgb(r: u8, g: u8, b: u8) -> ColorRgba {
    ColorRgba::from_rgba8(r, g, b, 255)
}
