//! Round dab rasterization into the per-stroke coverage buffer.
//!
//! Profile at distance `d` from the dab center (pixel centers at `+0.5`), radius `r`:
//! * hardness falloff: `1` for `d ≤ h·r`, then `1 − smoothstep((d − h·r)/(r − h·r))` to 0 at `r`;
//! * analytic 1-px antialiasing: multiplied by `clamp(r − d + 0.5, 0, 1)` (≈ pixel area inside
//!   the circle), so the 50 % iso-contour of a hard dab lies exactly on radius `r` and its
//!   integrated coverage is `π(r² + 1/12)`;
//! * sub-pixel dabs (`r < 0.5`) are rendered at `r = 0.5` with alpha scaled by `(r/0.5)²`, which
//!   conserves their area-weighted coverage instead of letting them vanish;
//! * optional grain multiplies the value by seeded document-space noise.
//!
//! Values are quantized once: `v16 = floor(v · 65535 + 0.5)`. Accumulation: `Max` →
//! `m = max(m, v16)`; `BuildUp` → `m = m + ceil(v16 · (65535 − m) / 65535)` (ceil so repeated dabs saturate
//! exactly at 65535 instead of stalling one LSB short; never exceeds 65535).

use veskra_core::{RectF, RectI};
use veskra_raster::TileGrid;

use crate::Accumulation;
use crate::coverage::{CoverageGrid, T};
use crate::noise::{Grain, grain_factor};

/// Minimum rendered radius; smaller dabs fade instead (area conservation).
pub(crate) const MIN_RADIUS: f64 = 0.5;

#[derive(Debug, Clone, Copy)]
pub(crate) struct DabStyle {
    pub hardness: f64,
    pub accumulation: Accumulation,
    pub grain: Option<Grain>,
    pub seed: u64,
}

/// Radial profile (hardness falloff × edge antialiasing) for radius `r` at distance `d`.
#[inline]
pub(crate) fn profile(d: f64, r: f64, hardness: f64) -> f64 {
    let edge = (r - d + 0.5).clamp(0.0, 1.0);
    let inner = hardness * r;
    let soft = r - inner;
    let fall = if d <= inner || soft <= 1e-9 {
        1.0
    } else {
        let t = ((d - inner) / soft).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    };
    edge * fall
}

/// Stamp one dab; returns the (conservative) touched pixel rect, already clipped.
pub(crate) fn rasterize(
    cov: &mut CoverageGrid,
    cx: f64,
    cy: f64,
    radius: f64,
    alpha: f64,
    st: &DabStyle,
    clip: RectI,
) -> RectI {
    if !(radius.is_finite() && radius > 0.0 && alpha.is_finite() && alpha > 0.0) {
        return RectI::EMPTY;
    }
    let r = radius.max(MIN_RADIUS);
    let alpha = (alpha * (radius / r).powi(2)).min(1.0);
    let outer = r + 0.5;
    let outer2 = outer * outer;
    let bbox = RectI::covering(&RectF::new(cx - outer, cy - outer, cx + outer, cy + outer)).intersect(&clip);
    if bbox.is_empty() {
        return RectI::EMPTY;
    }
    for c in TileGrid::tiles_in_rect(bbox) {
        let tr = c.rect().intersect(&bbox);
        // Skip tiles whose nearest point to the center is outside the support.
        let nx = cx.clamp(tr.x0 as f64, tr.x1 as f64);
        let ny = cy.clamp(tr.y0 as f64, tr.y1 as f64);
        if (nx - cx).powi(2) + (ny - cy).powi(2) >= outer2 {
            continue;
        }
        let origin = c.rect();
        let data = cov.tile_mut(c);
        for py in tr.y0..tr.y1 {
            let dy = py as f64 + 0.5 - cy;
            let dy2 = dy * dy;
            if dy2 >= outer2 {
                continue;
            }
            let half = (outer2 - dy2).sqrt();
            let xs = ((cx - half - 0.5).floor() as i64).max(tr.x0 as i64) as i32;
            let xe = ((cx + half + 0.5).ceil() as i64).min(tr.x1 as i64) as i32;
            let row = (py - origin.y0) as usize * T;
            for px in xs..xe {
                let dx = px as f64 + 0.5 - cx;
                let d2 = dx * dx + dy2;
                if d2 >= outer2 {
                    continue;
                }
                let mut v = profile(d2.sqrt(), r, st.hardness) * alpha;
                if let Some(g) = &st.grain {
                    v *= grain_factor(px as f64 + 0.5, py as f64 + 0.5, g, st.seed) as f64;
                }
                let v16 = (v * 65535.0 + 0.5).floor().clamp(0.0, 65535.0) as u32;
                if v16 == 0 {
                    continue;
                }
                let m = &mut data[row + (px - origin.x0) as usize];
                match st.accumulation {
                    Accumulation::Max => *m = (*m).max(v16 as u16),
                    Accumulation::BuildUp => {
                        let cur = *m as u32;
                        *m = (cur + (v16 * (65535 - cur)).div_ceil(65535)).min(65535) as u16;
                    }
                }
            }
        }
    }
    bbox
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(h: f64) -> DabStyle {
        DabStyle { hardness: h, accumulation: Accumulation::Max, grain: None, seed: 0 }
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
    fn hard_dab_area_matches_disc() {
        for r in [1.0, 2.5, 7.3, 20.0] {
            let mut cov = CoverageGrid::default();
            let b = rasterize(&mut cov, 50.3, 40.7, r, 1.0, &style(1.0), RectI::new(0, 0, 200, 200));
            let area = sum(&cov, b);
            // The linear 1-px edge ramp integrates to exactly π(r² + 1/12).
            let expect = std::f64::consts::PI * (r * r + 1.0 / 12.0);
            assert!((area - expect).abs() / expect < 0.01, "r={r} area={area} expect={expect}");
        }
    }

    #[test]
    fn subpixel_dab_fades_not_vanishes() {
        let mut cov = CoverageGrid::default();
        let b = rasterize(&mut cov, 10.5, 10.5, 0.2, 1.0, &style(1.0), RectI::new(0, 0, 64, 64));
        let s = sum(&cov, b);
        assert!(s > 0.1 && s < 0.25, "{s}");
    }

    #[test]
    fn clip_and_negative_coords() {
        let mut cov = CoverageGrid::default();
        let b = rasterize(&mut cov, -3.0, -3.0, 5.0, 1.0, &style(1.0), RectI::new(0, 0, 64, 64));
        assert_eq!(b.x0, 0);
        assert_eq!(cov.get(-1, -1), 0);
        assert!(cov.get(0, 0) > 0);
        assert_eq!(cov.tile_count(), 1);
        let none = rasterize(&mut cov, 500.0, 500.0, 5.0, 1.0, &style(1.0), RectI::new(0, 0, 64, 64));
        assert!(none.is_empty());
    }

    #[test]
    fn buildup_saturates_without_overflow() {
        let mut cov = CoverageGrid::default();
        let st = DabStyle { accumulation: Accumulation::BuildUp, ..style(1.0) };
        for _ in 0..500 {
            rasterize(&mut cov, 8.0, 8.0, 4.0, 0.3, &st, RectI::new(0, 0, 64, 64));
        }
        assert_eq!(cov.get(8, 8), 65535);
    }
}
