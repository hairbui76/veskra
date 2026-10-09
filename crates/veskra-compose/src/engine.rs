//! Tile-by-tile recursive compositor (private). See the crate docs for the normative semantics.

use std::collections::BTreeSet;
use veskra_color::convert;

use veskra_core::{BlendMode, BlendSpace, LayerId, RectI, limits};
use veskra_doc::{Document, Layer, LayerKind, MaskGrid, TileGrid};
use veskra_raster::{TILE, TILE_PIXELS, TileCoord};

use crate::ComposeOptions;
use crate::blend::{general_px, lerp_px, normal_px, overlay_fn, scale_px};

/// One tile of premultiplied `f32` RGBA in the document blend space, row-major 64×64.
pub(crate) type Buf = Vec<[f32; 4]>;

const INV_65535: f32 = 1.0 / 65535.0;
const INV_255: f32 = 1.0 / 255.0;

pub(crate) fn new_buf() -> Buf {
    vec![[0.0; 4]; TILE_PIXELS]
}

/// Mask coverage of one tile.
enum Cov<'m> {
    /// No mask or a uniformly 255 tile: coverage 1.
    Full,
    /// Uniform coverage in `(0, 1)`; 0 is represented by `None` from [`cov_of`].
    Uniform(f32),
    Tile(&'m [u8]),
}

impl Cov<'_> {
    #[inline]
    fn at(&self, i: usize) -> f32 {
        match self {
            Cov::Full => 1.0,
            Cov::Uniform(v) => *v,
            Cov::Tile(d) => d.get(i).map_or(0.0, |&m| m as f32 * INV_255),
        }
    }
    fn is_full(&self) -> bool {
        matches!(self, Cov::Full)
    }
}

/// Coverage of `mask` at tile `c`; `None` if the mask hides the whole tile.
fn cov_of(mask: Option<&MaskGrid>, c: TileCoord) -> Option<Cov<'_>> {
    let Some(m) = mask else { return Some(Cov::Full) };
    let uniform = |v: u8| match v {
        0 => None,
        255 => Some(Cov::Full),
        v => Some(Cov::Uniform(v as f32 * INV_255)),
    };
    match m.tile(c) {
        None => uniform(m.default_value()),
        Some(t) => match t.uniform_value() {
            Some(v) => uniform(v),
            None => Some(Cov::Tile(t.data())),
        },
    }
}

/// A layer is a *clipper* when it has `clip_to_below` set and is not a group: Photoshop does
/// not support clipping groups, so `clip_to_below` on a group is ignored (it acts as a base).
pub(crate) fn is_clipper(l: &Layer) -> bool {
    l.props.clip_to_below && !l.is_group()
}

/// Immutable, `Sync` compositing context. `layer_filter` is evaluated once up front (on the
/// caller's thread) so tiles can be composited in parallel without requiring `Fn + Sync`.
pub(crate) struct Engine<'a> {
    doc: &'a Document,
    overrides: &'a [(LayerId, &'a TileGrid)],
    include_hidden: bool,
    /// Leaf layers rejected by `layer_filter`.
    filtered: BTreeSet<LayerId>,
    space: BlendSpace,
}

impl<'a> Engine<'a> {
    pub(crate) fn new(doc: &'a Document, opts: &ComposeOptions<'a>) -> Self {
        let filtered = match opts.layer_filter {
            Some(f) => doc.iter_layers().filter(|l| !l.is_group() && !f(l.id)).map(|l| l.id).collect(),
            None => BTreeSet::new(),
        };
        Self {
            doc,
            overrides: opts.overrides,
            include_hidden: opts.include_hidden,
            filtered,
            space: doc.color.blend_space,
        }
    }

    fn shown(&self, l: &Layer) -> bool {
        (self.include_hidden || l.props.visible) && !self.filtered.contains(&l.id)
    }

    /// The tiles a pixel layer is rendered from (override first). `None` for groups.
    fn pixel_tiles(&self, l: &'a Layer) -> Option<&'a TileGrid> {
        if l.is_group() {
            return None;
        }
        if let Some((_, g)) = self.overrides.iter().find(|(id, _)| *id == l.id) {
            return Some(*g);
        }
        l.pixels().map(|p| &p.tiles)
    }

    /// Premultiplied `f32` in the blend space → premultiplied encoded RGBA16 (single rounding).
    #[inline]
    pub(crate) fn store(&self, v: [f32; 4]) -> [u16; 4] {
        match self.space {
            BlendSpace::Linear => convert::linear_premul_f32_to_premul16(v),
            BlendSpace::GammaEncoded => {
                let a = quant16(v[3]);
                [quant16(v[0]).min(a), quant16(v[1]).min(a), quant16(v[2]).min(a), a]
            }
        }
    }

    /// Blend premultiplied sources onto `acc` (one tile): per pixel `k = opacity × coverage`,
    /// `acc = blend(acc, load(src)·k, mode)`. Monomorphized per blend space, mask kind and mode
    /// so the inner loop has no dispatch.
    #[allow(clippy::too_many_arguments)]
    fn blend_tile<T: Copy>(
        &self,
        acc: &mut [[f32; 4]],
        src: impl Iterator<Item = T>,
        opacity: f32,
        cov: &Cov<'_>,
        mode: BlendMode,
        load_g: impl Fn(T) -> [f32; 4] + Copy,
        load_l: impl Fn(T) -> [f32; 4] + Copy,
    ) {
        match self.space {
            BlendSpace::Linear => by_cov(acc, src, opacity, cov, mode, load_l),
            BlendSpace::GammaEncoded => by_cov(acc, src, opacity, cov, mode, load_g),
        }
    }

    /// Composite the whole document (root list) for tile `c`. `None` if nothing was drawn.
    pub(crate) fn root_tile(&self, c: TileCoord) -> Option<Buf> {
        let mut acc = new_buf();
        self.list(self.doc.root(), c, &mut acc, 0).then_some(acc)
    }

    /// Composite one node alone onto transparent (own opacity/mask, Normal mode, no clipping).
    pub(crate) fn subtree_tile(&self, layer: &'a Layer, c: TileCoord) -> Option<Buf> {
        let mut acc = new_buf();
        self.node(layer, &[], c, &mut acc, 0, Some(BlendMode::Normal)).then_some(acc)
    }

    /// Composite the sibling list `ids` (bottom → top) onto `acc`. Returns whether anything drew.
    fn list(&self, ids: &[LayerId], c: TileCoord, acc: &mut [[f32; 4]], depth: usize) -> bool {
        if depth > limits::MAX_GROUP_DEPTH + 1 {
            return false; // corrupt (cyclic) tree guard; validated documents never reach this
        }
        let mut drawn = false;
        let mut seen_base = false;
        let mut i = 0;
        while i < ids.len() {
            let Some(layer) = self.doc.layer(ids[i]) else {
                i += 1;
                continue;
            };
            if is_clipper(layer) {
                if !seen_base {
                    // Clipping layer with no base below it: composites normally.
                    drawn |= self.node(layer, &[], c, acc, depth, None);
                }
                // Otherwise it was composited by its base.
                i += 1;
                continue;
            }
            seen_base = true;
            let mut j = i + 1;
            while j < ids.len() && self.doc.layer(ids[j]).is_some_and(is_clipper) {
                j += 1;
            }
            drawn |= self.node(layer, &ids[i + 1..j], c, acc, depth, None);
            i = j;
        }
        drawn
    }

    /// Composite one node (with the clipping layers that clip to it) onto `acc`.
    fn node(
        &self,
        layer: &'a Layer,
        clips: &[LayerId],
        c: TileCoord,
        acc: &mut [[f32; 4]],
        depth: usize,
        mode_override: Option<BlendMode>,
    ) -> bool {
        if !self.shown(layer) {
            return false;
        }
        let opacity = if layer.props.opacity.is_finite() { layer.props.opacity.clamp(0.0, 1.0) } else { 0.0 };
        if opacity <= 0.0 {
            return false;
        }
        let mask = layer.pixels().and_then(|p| p.mask.as_ref());
        let Some(cov) = cov_of(mask, c) else { return false };
        let mode = mode_override.unwrap_or(layer.props.blend);

        match &layer.kind {
            LayerKind::Group(g) if g.pass_through && clips.is_empty() => {
                // Pass-through: children blend directly onto the backdrop; the group's
                // opacity (and mask) then interpolate between backdrop before and after.
                if opacity >= 1.0 && cov.is_full() {
                    return self.list(&g.children, c, acc, depth + 1);
                }
                let before = acc.to_vec();
                if !self.list(&g.children, c, acc, depth + 1) {
                    return false;
                }
                for (i, (a, b)) in acc.iter_mut().zip(before.iter()).enumerate() {
                    *a = lerp_px(*b, *a, opacity * cov.at(i));
                }
                true
            }
            LayerKind::Group(g) => {
                // Isolated (or a pass-through group acting as a clipping base, which psd-tools
                // and Photoshop composite as an ordinary source): children onto transparent.
                let mut src = new_buf();
                if !self.list(&g.children, c, &mut src, depth + 1) {
                    return false;
                }
                let mode = if g.pass_through { mode_override.unwrap_or(BlendMode::Normal) } else { mode };
                self.finish(src, clips, opacity, &cov, c, acc, depth, mode);
                true
            }
            LayerKind::Raster(_) | LayerKind::Vector(_) => {
                let Some(tile) = self.pixel_tiles(layer).and_then(|g| g.tile(c)) else { return false };
                let px = tile.pixels();
                if clips.is_empty() {
                    // Fast path: blend straight from the u16 tile.
                    self.blend_tile(acc, px.chunks_exact(4), opacity, &cov, mode, load_gamma, load_linear);
                    return true;
                }
                let src: Buf = match self.space {
                    BlendSpace::Linear => px.chunks_exact(4).map(load_linear).collect(),
                    BlendSpace::GammaEncoded => px.chunks_exact(4).map(load_gamma).collect(),
                };
                self.finish(src, clips, opacity, &cov, c, acc, depth, mode);
                true
            }
        }
    }

    /// Apply clipping layers to a resolved base source, then its opacity × mask, then blend.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        mut src: Buf,
        clips: &[LayerId],
        opacity: f32,
        cov: &Cov<'_>,
        c: TileCoord,
        acc: &mut [[f32; 4]],
        depth: usize,
        mode: BlendMode,
    ) {
        if !clips.is_empty() {
            self.apply_clips(&mut src, clips, c, depth);
        }
        let id = |p: &[f32; 4]| *p;
        self.blend_tile(acc, src.iter(), opacity, cov, mode, id, id);
    }

    /// psd-tools `_apply_clip_layers` semantics: the clipping layers are composited (each with
    /// its own mode/opacity/mask) onto the base's own pixels (base mask/opacity NOT yet
    /// applied); the result keeps its straight color but takes the base's alpha back.
    fn apply_clips(&self, base: &mut [[f32; 4]], clips: &[LayerId], c: TileCoord, depth: usize) {
        let base_alpha: Vec<f32> = base.iter().map(|p| p[3]).collect();
        let mut any = false;
        for id in clips {
            if let Some(l) = self.doc.layer(*id) {
                any |= self.node(l, &[], c, base, depth, None);
            }
        }
        if !any {
            return;
        }
        for (p, &ab) in base.iter_mut().zip(base_alpha.iter()) {
            let a = p[3];
            *p = if a > 0.0 && ab > 0.0 {
                let k = ab / a;
                [(p[0] * k).min(ab), (p[1] * k).min(ab), (p[2] * k).min(ab), ab]
            } else {
                [0.0; 4]
            };
        }
    }

    /// Write the part of tile buffer `buf` (tile `c`) inside `region` into `out` (row-major
    /// RGBA16 covering `out_rect`).
    pub(crate) fn write_region(&self, buf: &[[f32; 4]], c: TileCoord, region: RectI, out: &mut [u16], out_rect: RectI) {
        let tr = c.rect();
        let ow = out_rect.width() as usize;
        for y in region.y0..region.y1 {
            let ly = (y - tr.y0) as usize;
            for x in region.x0..region.x1 {
                let lx = (x - tr.x0) as usize;
                let Some(v) = buf.get(ly * TILE as usize + lx) else { continue };
                if v[3] <= 0.0 {
                    continue;
                }
                let di = ((y - out_rect.y0) as usize * ow + (x - out_rect.x0) as usize) * 4;
                if let Some(d) = out.get_mut(di..di + 4) {
                    d.copy_from_slice(&self.store(*v));
                }
            }
        }
    }
}

/// float → u16 with the CONTRACTS §1 rule `floor(clamp(v)·65535 + 0.5)`, NaN → 0. Bit-identical
/// to `veskra_color::convert::f32_to_u16` (the value is non-negative, so the truncating cast
/// is `floor`; avoids a libm `floor` call on targets without SSE4.1 / FRINTM); unit-tested.
#[inline(always)]
pub(crate) fn quant16(v: f32) -> u16 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    (v as f64 * 65535.0 + 0.5) as u16
}

#[inline(always)]
fn load_gamma(p: &[u16]) -> [f32; 4] {
    [p[0] as f32 * INV_65535, p[1] as f32 * INV_65535, p[2] as f32 * INV_65535, p[3] as f32 * INV_65535]
}

#[inline(always)]
fn load_linear(p: &[u16]) -> [f32; 4] {
    convert::premul16_to_linear_premul_f32([p[0], p[1], p[2], p[3]])
}

#[inline(always)]
fn by_cov<T: Copy>(
    acc: &mut [[f32; 4]],
    src: impl Iterator<Item = T>,
    opacity: f32,
    cov: &Cov<'_>,
    mode: BlendMode,
    load: impl Fn(T) -> [f32; 4] + Copy,
) {
    match cov {
        Cov::Full => by_mode(acc, src, std::iter::repeat(opacity), mode, load),
        Cov::Uniform(u) => by_mode(acc, src, std::iter::repeat(opacity * *u), mode, load),
        Cov::Tile(d) => by_mode(acc, src, d.iter().map(move |&m| opacity * (m as f32 * INV_255)), mode, load),
    }
}

#[inline(always)]
fn by_mode<T: Copy>(
    acc: &mut [[f32; 4]],
    src: impl Iterator<Item = T>,
    ks: impl Iterator<Item = f32>,
    mode: BlendMode,
    load: impl Fn(T) -> [f32; 4] + Copy,
) {
    match mode {
        BlendMode::Multiply => run(acc, src, ks, load, |b, s| general_px(b, s, |x, y| x * y)),
        BlendMode::Screen => run(acc, src, ks, load, |b, s| general_px(b, s, |x, y| x + y - x * y)),
        BlendMode::Overlay => run(acc, src, ks, load, |b, s| general_px(b, s, overlay_fn)),
        _ => run(acc, src, ks, load, normal_px),
    }
}

#[inline(always)]
fn run<T: Copy>(
    acc: &mut [[f32; 4]],
    src: impl Iterator<Item = T>,
    ks: impl Iterator<Item = f32>,
    load: impl Fn(T) -> [f32; 4],
    blend: impl Fn([f32; 4], [f32; 4]) -> [f32; 4],
) {
    for ((a, p), k) in acc.iter_mut().zip(src).zip(ks) {
        if k <= 0.0 {
            continue;
        }
        let s = load(p);
        if s[3] <= 0.0 {
            continue;
        }
        *a = blend(*a, if k >= 1.0 { s } else { scale_px(s, k) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quant16_is_bit_identical_to_contract_rounding() {
        let mut v = 0.0f32;
        while v <= 1.0 {
            assert_eq!(quant16(v), convert::f32_to_u16(v), "{v}");
            v = f32::from_bits(v.to_bits() + 97);
        }
        for i in 0..=65535u32 {
            for d in [-1e-7f32, 0.0, 1e-7] {
                let x = i as f32 / 65535.0 + d;
                assert_eq!(quant16(x), convert::f32_to_u16(x), "{x}");
                let h = (i as f32 + 0.5) / 65535.0 + d;
                assert_eq!(quant16(h), convert::f32_to_u16(h), "{h}");
            }
        }
        for x in [f32::NAN, -1.0, 2.0, f32::INFINITY, f32::NEG_INFINITY, -0.0, 1.0] {
            assert_eq!(quant16(x), convert::f32_to_u16(x));
        }
    }
}
