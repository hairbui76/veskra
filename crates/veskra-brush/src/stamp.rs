//! General dab rasterizer (brush system v2, ADR-019): rotated elliptical round tips, image tips,
//! procedural or image textures in document or dab space, the wet-edge depth buffer and per-dab
//! color. Schema-2 round dabs keep using [`crate::dab::rasterize`], so their output stays
//! byte-identical; this path only runs for presets that use a v2 feature.
//!
//! Geometry: `θ` is the tip angle, counter-clockwise on screen (document y points down). A pixel
//! offset `(dx, dy)` from the dab center maps to tip coordinates
//! `u = dx·cosθ − dy·sinθ`, `v = (dx·sinθ + dy·cosθ) / roundness`. Round tips evaluate the radial
//! profile at `√(u² + v²)`; image tips sample the mask (fit into the dab diameter, aspect kept) at
//! `(u, v)` from the mip level whose texels are at least one pixel, bilinearly, zero outside.
//! Values are quantized and accumulated exactly like [`crate::dab`] (`Max` / `BuildUp`).

use std::sync::Arc;

use veskra_core::{RectF, RectI};
use veskra_raster::TileGrid;

use crate::Accumulation;
use crate::asset::{BrushAsset, MaskLevel};
use crate::coverage::{ColorGrid, CoverageGrid, DepthGrid, T};
use crate::dab::{MIN_RADIUS, profile};
use crate::noise::grain_noise;
use crate::preset::TextureMode;

/// Texture source resolved for rendering.
#[derive(Debug, Clone)]
pub(crate) enum TexKind {
    /// Feature size in px.
    Procedural { scale: f64 },
    /// Mip level `level` of `asset`, `texel` document px per texel of that level.
    Image { asset: Arc<BrushAsset>, level: usize, texel: f64 },
}

#[derive(Debug, Clone)]
pub(crate) struct TexStyle {
    pub kind: TexKind,
    pub mode: TextureMode,
    pub invert: bool,
    pub seed: u64,
}

impl TexStyle {
    /// "Less paint" value `0..=1` at document point `(x, y)` / dab-frame point `(u, v)`.
    #[inline]
    fn value(&self, x: f64, y: f64, u: f64, v: f64) -> f64 {
        let (px, py) = match self.mode {
            TextureMode::Fixed => (x, y),
            TextureMode::Follow => (u, v),
        };
        let n = match &self.kind {
            TexKind::Procedural { scale } => grain_noise(px, py, *scale, self.seed),
            TexKind::Image { asset, level, texel } => {
                let l: &MaskLevel = &asset.levels()[*level];
                1.0 - l.sample_wrapped(px / texel, py / texel)
            }
        };
        if self.invert { 1.0 - n } else { n }
    }
}

/// Per-stroke constants of the general rasterizer.
#[derive(Debug, Clone)]
pub(crate) struct StampStyle {
    pub tip: Option<Arc<BrushAsset>>,
    pub hardness: f64,
    pub roundness: f64,
    pub accumulation: Accumulation,
    pub texture: Option<TexStyle>,
}

/// One dab.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Dab {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    pub alpha: f64,
    pub cos: f64,
    pub sin: f64,
    /// Texture depth for this dab (`0..=1`).
    pub tex_depth: f64,
    /// Straight encoded RGB of this dab (per-dab color jitter), `0..=65535`.
    pub color: Option<[u16; 3]>,
}

/// Accumulation targets of one stroke.
#[derive(Debug)]
pub(crate) struct Buffers<'a> {
    pub cov: &'a mut CoverageGrid,
    pub depth: Option<&'a mut DepthGrid>,
    pub color: Option<&'a mut ColorGrid>,
}

/// How far (px) the footprint of a dab with `radius` reaches from its center (rotation-safe).
pub(crate) fn reach(st: &StampStyle, radius: f64) -> f64 {
    let r = radius.max(MIN_RADIUS);
    match &st.tip {
        None => r + 0.5,
        Some(a) => {
            let m = a.width().max(a.height()) as f64;
            let hw = r * a.width() as f64 / m;
            let hh = r * a.height() as f64 / m * st.roundness;
            (hw * hw + hh * hh).sqrt() + 1.0
        }
    }
}

#[inline]
fn blend_color(acc: Accumulation, m_old: u16, m_new: u16, v16: u32, old: [u16; 3], dab: [u16; 3]) -> [u16; 3] {
    if m_new == 0 || m_new == m_old && acc == Accumulation::Max {
        return old;
    }
    let mo = m_old as f64;
    let mn = m_new as f64;
    let mut out = [0u16; 3];
    for i in 0..3 {
        let c = match acc {
            // The coverage the dab added takes the dab's color.
            Accumulation::Max => (old[i] as f64 * mo + dab[i] as f64 * (mn - mo)) / mn,
            // Premultiplied src-over: P' = C·v + P·(1 − v).
            Accumulation::BuildUp => {
                let v = v16 as f64 / 65535.0;
                (dab[i] as f64 * v * 65535.0 + old[i] as f64 * mo * (1.0 - v)) / mn
            }
        };
        out[i] = (c + 0.5).floor().clamp(0.0, 65535.0) as u16;
    }
    out
}

/// Stamp one dab; returns the (conservative) touched pixel rect, already clipped.
pub(crate) fn stamp(buf: &mut Buffers<'_>, st: &StampStyle, dab: &Dab, clip: RectI) -> RectI {
    if !(dab.radius.is_finite() && dab.radius > 0.0 && dab.alpha.is_finite() && dab.alpha > 0.0) {
        return RectI::EMPTY;
    }
    let (cx, cy) = (dab.cx, dab.cy);
    let r = dab.radius.max(MIN_RADIUS);
    let alpha = (dab.alpha * (dab.radius / r).powi(2)).min(1.0);
    let rho = st.roundness.clamp(0.05, 1.0);
    // Image tip geometry: level, texel sizes and (unsquashed) half extents.
    let img = st.tip.as_ref().map(|a| {
        let m = a.width().max(a.height()) as f64;
        let hw = r * a.width() as f64 / m;
        let hh0 = r * a.height() as f64 / m;
        let px_per_texel = 2.0 * r / m;
        let l = a.level_for(px_per_texel * rho);
        let sx = 2.0 * hw / l.w as f64;
        let sy = 2.0 * hh0 / l.h as f64;
        (l, hw, hh0, sx, sy)
    });
    let reach = reach(st, dab.radius);
    let reach2 = reach * reach;
    let bbox = RectI::covering(&RectF::new(cx - reach, cy - reach, cx + reach, cy + reach)).intersect(&clip);
    if bbox.is_empty() {
        return RectI::EMPTY;
    }
    // Per-dab constants of the inner loop. Image tips: texel coordinates are
    // `u·inv_sx + w/2`, `w_rot·inv_sy + h/2`; pixels beyond one texel outside the image are 0.
    let outer2 = (r + 0.5) * (r + 0.5);
    let inv_rho = 1.0 / rho;
    let img = img.map(|(l, hw, hh0, sx, sy)| {
        (l, 1.0 / sx, inv_rho / sy, l.w as f64 * 0.5, l.h as f64 * 0.5, hw + sx, (hh0 + sy) * rho, 1.0 / hw, 1.0 / hh0)
    });
    let want_depth = buf.depth.is_some();
    let Buffers { cov, depth, color } = buf;
    for c in TileGrid::tiles_in_rect(bbox) {
        let tr = c.rect().intersect(&bbox);
        let nx = cx.clamp(tr.x0 as f64, tr.x1 as f64);
        let ny = cy.clamp(tr.y0 as f64, tr.y1 as f64);
        if (nx - cx).powi(2) + (ny - cy).powi(2) >= reach2 {
            continue;
        }
        let origin = c.rect();
        let cov_t = cov.tile_mut(c);
        let mut dep_t = depth.as_mut().map(|d| d.tile_mut(c));
        let mut col_t = color.as_mut().map(|g| g.tile_mut(c));
        for py in tr.y0..tr.y1 {
            let dy = py as f64 + 0.5 - cy;
            let dy2 = dy * dy;
            if dy2 >= reach2 {
                continue;
            }
            // Row span of the reach circle.
            let half = (reach2 - dy2).sqrt();
            let xs = ((cx - half - 0.5).floor() as i64).max(tr.x0 as i64) as i32;
            let xe = ((cx + half + 0.5).ceil() as i64).min(tr.x1 as i64) as i32;
            let row = (py - origin.y0) as usize * T;
            for px in xs..xe {
                let dx = px as f64 + 0.5 - cx;
                let u = dx * dab.cos - dy * dab.sin;
                let w = dx * dab.sin + dy * dab.cos;
                let (val, dist_norm) = match &img {
                    None => {
                        let v = w * inv_rho;
                        let d2 = u * u + v * v;
                        if d2 >= outer2 {
                            continue;
                        }
                        let d = d2.sqrt();
                        (profile(d, r, st.hardness), d / r)
                    }
                    Some((l, inv_sx, inv_sy, half_w, half_h, lim_u, lim_w, inv_hw, inv_hh)) => {
                        if u.abs() > *lim_u || w.abs() > *lim_w {
                            continue;
                        }
                        let s = l.sample_clamped(u * inv_sx + half_w, w * inv_sy + half_h);
                        let dn = if want_depth {
                            let (a, b) = (u * inv_hw, w * inv_rho * inv_hh);
                            (a * a + b * b).sqrt()
                        } else {
                            0.0
                        };
                        (s, dn)
                    }
                };
                if val <= 0.0 {
                    continue;
                }
                let mut value = val * alpha;
                if let Some(t) = &st.texture {
                    value *= 1.0 - dab.tex_depth * t.value(px as f64 + 0.5, py as f64 + 0.5, u, w);
                }
                let v16 = (value * 65535.0 + 0.5).floor().clamp(0.0, 65535.0) as u32;
                if v16 == 0 {
                    continue;
                }
                let i = row + (px - origin.x0) as usize;
                let m = &mut cov_t[i];
                let old = *m;
                match st.accumulation {
                    Accumulation::Max => *m = (*m).max(v16 as u16),
                    Accumulation::BuildUp => {
                        let cur = *m as u32;
                        *m = (cur + (v16 * (65535 - cur)).div_ceil(65535)).min(65535) as u16;
                    }
                }
                if let Some(dt) = dep_t.as_deref_mut() {
                    let dep = ((1.0 - dist_norm).clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
                    dt[i] = dt[i].max(dep);
                }
                if let (Some(ct), Some(dc)) = (col_t.as_deref_mut(), dab.color) {
                    ct[i] = blend_color(st.accumulation, old, *m, v16, ct[i], dc);
                }
            }
        }
    }
    bbox
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(tip: Option<Arc<BrushAsset>>, roundness: f64) -> StampStyle {
        StampStyle { tip, hardness: 1.0, roundness, accumulation: Accumulation::Max, texture: None }
    }

    fn dab(cx: f64, cy: f64, radius: f64, angle_deg: f64) -> Dab {
        let (s, c) = angle_deg.to_radians().sin_cos();
        Dab { cx, cy, radius, alpha: 1.0, cos: c, sin: s, tex_depth: 0.0, color: None }
    }

    fn sum(cov: &CoverageGrid, r: RectI) -> f64 {
        let mut s = 0.0;
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                s += cov.get(x, y) as f64 / 65535.0;
            }
        }
        s
    }

    #[test]
    fn ellipse_area_scales_with_roundness() {
        let mut a = CoverageGrid::default();
        let mut b = CoverageGrid::default();
        let clip = RectI::new(0, 0, 200, 200);
        let ra = stamp(
            &mut Buffers { cov: &mut a, depth: None, color: None },
            &style(None, 1.0),
            &dab(100.3, 90.6, 30.0, 0.0),
            clip,
        );
        let rb = stamp(
            &mut Buffers { cov: &mut b, depth: None, color: None },
            &style(None, 0.5),
            &dab(100.3, 90.6, 30.0, 33.0),
            clip,
        );
        let (sa, sb) = (sum(&a, ra), sum(&b, rb));
        assert!((sb / sa - 0.5).abs() < 0.02, "{sa} {sb}");
    }

    #[test]
    fn image_tip_orientation_is_counter_clockwise() {
        // 3x3 tip with only the top-center texel painted (an "arrow" pointing up).
        let tip = Arc::new(BrushAsset::from_mask(3, 3, vec![0, 255, 0, 0, 0, 0, 0, 0, 0]).unwrap());
        let clip = RectI::new(0, 0, 64, 64);
        let at = |deg: f64| {
            let mut g = CoverageGrid::default();
            stamp(
                &mut Buffers { cov: &mut g, depth: None, color: None },
                &style(Some(tip.clone()), 1.0),
                &dab(32.0, 32.0, 15.0, deg),
                clip,
            );
            g
        };
        let up = at(0.0);
        assert!(up.get(31, 23) > up.get(31, 40), "0°: paint above the center");
        let left = at(90.0);
        assert!(left.get(23, 31) > left.get(40, 31), "90° ccw: the arrow points left");
    }

    #[test]
    fn wet_depth_and_color_buffers_fill() {
        let mut cov = CoverageGrid::default();
        let mut depth = DepthGrid::default();
        let mut col = ColorGrid::default();
        let clip = RectI::new(0, 0, 64, 64);
        let mut d = dab(32.0, 32.0, 10.0, 0.0);
        d.color = Some([65535, 0, 0]);
        stamp(
            &mut Buffers { cov: &mut cov, depth: Some(&mut depth), color: Some(&mut col) },
            &style(None, 1.0),
            &d,
            clip,
        );
        assert!(depth.get(32, 32) > 225 && depth.get(41, 32) < 40, "{} {}", depth.get(32, 32), depth.get(41, 32));
        assert_eq!(col.get(32, 32), [65535, 0, 0]);
        let mut d2 = dab(36.0, 32.0, 10.0, 0.0);
        d2.color = Some([0, 0, 65535]);
        d2.alpha = 1.0;
        stamp(
            &mut Buffers { cov: &mut cov, depth: Some(&mut depth), color: Some(&mut col) },
            &style(None, 1.0),
            &d2,
            clip,
        );
        // Max accumulation: full coverage already there keeps the first color.
        assert_eq!(col.get(32, 32), [65535, 0, 0]);
        // Newly covered pixels take the second dab's color.
        assert_eq!(col.get(45, 32), [0, 0, 65535]);
    }
}
