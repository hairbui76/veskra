//! Anti-aliased selection rasterization into [`MaskGrid`] (EDIT-02).
//!
//! * [`rect`] is exact: coverage = (horizontal overlap) × (vertical overlap), rounded to u8.
//! * [`polygon`]/[`ellipse`]/[`polygons`] use a sparse scanline rasterizer with
//!   [`SUB_SCANLINES`] sub-scanlines per pixel row and **exact horizontal span coverage** on
//!   each sub-scanline, nonzero winding rule. Work is O(rows × (active edges · log + width))
//!   over `bbox ∩ clip` only — never a per-pixel point-in-polygon test over the canvas.
//!
//! Inputs are validated: non-finite or out-of-range coordinates (|v| > [`MAX_COORD`]) and
//! polygons with fewer than 3 points produce an empty mask. Coordinates are clipped to
//! `clip` before any per-pixel work, so huge shapes cost only the clip area.

use veskra_core::{PointF, RectF, RectI};

use crate::{MaskGrid, TILE_U, TileCoord, TileGrid};

/// Vertical sub-samples per pixel row for polygon coverage.
pub const SUB_SCANLINES: u32 = 16;
/// Largest accepted coordinate magnitude (keeps f64 edge math exact enough).
pub const MAX_COORD: f64 = 1e12;

fn coord_ok(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

fn normalize(r: RectF) -> Option<RectF> {
    if ![r.x0, r.y0, r.x1, r.y1].iter().all(|&v| coord_ok(v)) {
        return None;
    }
    let n = RectF::new(r.x0.min(r.x1), r.y0.min(r.y1), r.x0.max(r.x1), r.y0.max(r.y1));
    (!n.is_empty()).then_some(n)
}

/// Exact anti-aliased rectangle (corners may be given in any order). Interior tiles share
/// one full-coverage allocation.
pub fn rect(r: RectF, clip: RectI) -> MaskGrid {
    let mut m = MaskGrid::new_empty();
    let Some(r) = normalize(r) else { return m };
    let area = RectI::covering(&r).intersect(&clip);
    if area.is_empty() {
        return m;
    }
    // Pixels with full coverage.
    let inner = RectI::covering(&RectF::new(r.x0.ceil(), r.y0.ceil(), r.x1.floor(), r.y1.floor()));
    let ov = |p: i32, a: f64, b: f64| ((p as f64 + 1.0).min(b) - (p as f64).max(a)).clamp(0.0, 1.0);
    for c in TileGrid::tiles_in_rect(area) {
        let tr = c.rect();
        let ir = tr.intersect(&area);
        if inner.intersect(&tr) == tr {
            m.set_tile(c, crate::mask::uniform_mask_tile(255));
            continue;
        }
        let t = m.tile_mut(c);
        for y in ir.y0..ir.y1 {
            let cy = ov(y, r.y0, r.y1);
            for x in ir.x0..ir.x1 {
                let cx = ov(x, r.x0, r.x1);
                let v = (cx * cy * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
                t.data_mut()[(y - tr.y0) as usize * TILE_U + (x - tr.x0) as usize] = v;
            }
        }
    }
    m.compact();
    m
}

/// Anti-aliased ellipse inscribed in `bounds`, flattened to a polygon whose chord error is
/// at most ~0.01 px (capped at 65536 segments).
pub fn ellipse(bounds: RectF, clip: RectI) -> MaskGrid {
    let Some(b) = normalize(bounds) else { return MaskGrid::new_empty() };
    let (cx, cy) = ((b.x0 + b.x1) * 0.5, (b.y0 + b.y1) * 0.5);
    let (rx, ry) = ((b.x1 - b.x0) * 0.5, (b.y1 - b.y0) * 0.5);
    let rmax = rx.max(ry);
    let tol = 0.01f64;
    let step = if rmax <= tol { std::f64::consts::FRAC_PI_2 } else { (1.0 - tol / rmax).acos() * 2.0 };
    let n = ((std::f64::consts::TAU / step).ceil() as usize).clamp(16, 65_536);
    let pts: Vec<PointF> = (0..n)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            PointF::new(cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect();
    polygon(&pts, clip)
}

/// Anti-aliased closed polygon (lasso), nonzero winding. Self-intersections are allowed.
pub fn polygon(points: &[PointF], clip: RectI) -> MaskGrid {
    polygons(&[points], clip)
}

/// Several closed contours filled together with the nonzero rule (e.g. a lasso with holes
/// drawn in the opposite direction). Contours with < 3 points are ignored; any non-finite
/// or out-of-range coordinate rejects the whole input (empty mask).
pub fn polygons(contours: &[&[PointF]], clip: RectI) -> MaskGrid {
    let mut m = MaskGrid::new_empty();
    if contours.iter().flat_map(|c| c.iter()).any(|p| !coord_ok(p.x) || !coord_ok(p.y)) {
        return m;
    }
    let mut edges = Vec::new();
    let mut bb = RectF::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for c in contours.iter().filter(|c| c.len() >= 3) {
        bb = bb.union(&RectF::from_points(c.iter().copied()));
        for i in 0..c.len() {
            let (p, q) = (c[i], c[(i + 1) % c.len()]);
            if p.y == q.y {
                continue;
            }
            let (a, b, dir) = if p.y < q.y { (p, q, 1i32) } else { (q, p, -1) };
            edges.push(Edge { y0: a.y, y1: b.y, x0: a.x, dxdy: (b.x - a.x) / (b.y - a.y), dir });
        }
    }
    if edges.is_empty() {
        return m;
    }
    let area = RectI::covering(&bb).intersect(&clip);
    if area.is_empty() {
        return m;
    }
    edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let w = area.width() as usize;
    let mut acc = vec![0f64; w + 1];
    let mut diff = vec![0f64; w + 1];
    let mut row = vec![0u8; w];
    let mut active: Vec<usize> = Vec::new();
    let mut next_edge = 0usize;
    let mut xs: Vec<(f64, i32)> = Vec::new();
    let sw = 1.0 / SUB_SCANLINES as f64;
    let (fx0, fx1) = (area.x0 as f64, area.x1 as f64);
    for y in area.y0..area.y1 {
        acc.iter_mut().for_each(|v| *v = 0.0);
        diff.iter_mut().for_each(|v| *v = 0.0);
        let mut any = false;
        for s in 0..SUB_SCANLINES {
            let sy = y as f64 + (s as f64 + 0.5) * sw;
            while next_edge < edges.len() && edges[next_edge].y0 <= sy {
                active.push(next_edge);
                next_edge += 1;
            }
            active.retain(|&i| edges[i].y1 > sy);
            xs.clear();
            for &i in &active {
                let e = &edges[i];
                if e.y0 <= sy {
                    xs.push((e.x0 + (sy - e.y0) * e.dxdy, e.dir));
                }
            }
            if xs.is_empty() {
                continue;
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0i32;
            let mut start = 0f64;
            for &(x, d) in &xs {
                let was = wind;
                wind += d;
                if was == 0 && wind != 0 {
                    start = x;
                } else if was != 0 && wind == 0 {
                    let (a, b) = (start.clamp(fx0, fx1) - fx0, x.clamp(fx0, fx1) - fx0);
                    if b > a {
                        any = true;
                        add_span(&mut acc, &mut diff, a, b, sw, w);
                    }
                }
            }
        }
        if !any {
            continue;
        }
        let mut run = 0f64;
        for i in 0..w {
            run += diff[i];
            let c = (acc[i] + run).clamp(0.0, 1.0);
            row[i] = (c * 255.0 + 0.5).floor() as u8;
        }
        write_row(&mut m, area.x0, y, &row);
    }
    m.compact();
    m
}

#[derive(Debug)]
struct Edge {
    y0: f64,
    y1: f64,
    x0: f64,
    dxdy: f64,
    dir: i32,
}

/// Add coverage `weight` for the horizontal span `[a, b)` (relative pixel coordinates, `0..=w`).
fn add_span(acc: &mut [f64], diff: &mut [f64], a: f64, b: f64, weight: f64, w: usize) {
    let ia = (a.floor() as usize).min(w);
    let ib = (b.floor() as usize).min(w);
    if ia == ib {
        acc[ia] += (b - a) * weight;
        return;
    }
    acc[ia] += (ia as f64 + 1.0 - a) * weight;
    diff[ia + 1] += weight;
    diff[ib] -= weight;
    acc[ib] += (b - ib as f64) * weight;
}

/// Write a coverage row into the mask, skipping all-zero tile segments.
fn write_row(m: &mut MaskGrid, x0: i32, y: i32, row: &[u8]) {
    let mut i = 0usize;
    while i < row.len() {
        let x = x0 + i as i32;
        let (c, lx, ly) = TileCoord::split(x, y);
        let n = (TILE_U - lx as usize).min(row.len() - i);
        let seg = &row[i..i + n];
        if seg.iter().any(|&v| v != 0) {
            let t = m.tile_mut(c);
            t.data_mut()[ly as usize * TILE_U + lx as usize..][..n].copy_from_slice(seg);
        }
        i += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: RectI = RectI::new(-1000, -1000, 1000, 1000);

    fn total(m: &MaskGrid) -> f64 {
        m.iter_tiles().map(|(_, t)| t.data().iter().map(|&v| v as f64).sum::<f64>()).sum::<f64>() / 255.0
    }

    #[test]
    fn rect_integer_is_exact() {
        let m = rect(RectF::new(-10.0, -20.0, 70.0, 130.0), CLIP);
        assert_eq!(m.bounds(), RectI::new(-10, -20, 70, 130));
        assert_eq!(total(&m), 80.0 * 150.0);
        assert_eq!(m.get(-10, -20), 255);
        assert_eq!(m.get(-11, -20), 0);
        // Reversed corners are normalized.
        assert_eq!(rect(RectF::new(70.0, 130.0, -10.0, -20.0), CLIP), m);
    }

    #[test]
    fn rect_fractional_coverage() {
        let m = rect(RectF::new(0.5, 0.25, 3.0, 2.0), CLIP);
        assert_eq!(m.get(0, 0), 96); // 0.5 * 0.75 * 255 = 95.6
        assert_eq!(m.get(1, 0), 191);
        assert_eq!(m.get(1, 1), 255);
        assert_eq!(m.get(0, 1), 128);
        assert!((total(&m) - 2.5 * 1.75).abs() < 0.05);
    }

    #[test]
    fn rect_huge_is_clipped_and_shares_tiles() {
        let clip = RectI::new(0, 0, 1024, 1024);
        let m = rect(RectF::new(-1e11, -1e11, 1e11, 1e11), clip);
        assert_eq!(m.bounds(), clip);
        assert_eq!(m.tile_count(), 256);
        assert_eq!(m.byte_size(), 64 * 64, "all interior tiles share one allocation");
        assert!(rect(RectF::new(f64::NAN, 0.0, 1.0, 1.0), clip).is_empty());
        assert!(rect(RectF::new(0.0, 0.0, 1e300, 1.0), clip).is_empty());
        assert!(rect(RectF::new(5.0, 5.0, 5.0, 9.0), clip).is_empty());
    }

    #[test]
    fn ellipse_area_within_half_percent() {
        for &(rx, ry, cx, cy) in &[(50.0, 30.0, 0.3, 0.7), (7.5, 7.5, -20.0, 11.0), (300.0, 120.0, 10.0, 10.0)] {
            let m = ellipse(RectF::new(cx - rx, cy - ry, cx + rx, cy + ry), CLIP);
            let want = std::f64::consts::PI * rx * ry;
            let got = total(&m);
            assert!(((got - want) / want).abs() < 0.005, "rx={rx} got={got} want={want}");
        }
        assert_eq!(m_center(), 255);
    }

    fn m_center() -> u8 {
        ellipse(RectF::new(-10.0, -10.0, 10.0, 10.0), CLIP).get(0, 0)
    }

    #[test]
    fn polygon_triangle_area_and_axis_square_exact() {
        let tri = [PointF::new(0.0, 0.0), PointF::new(100.0, 0.0), PointF::new(0.0, 60.0)];
        let got = total(&polygon(&tri, CLIP));
        assert!((got - 3000.0).abs() / 3000.0 < 0.002, "{got}");
        // Integer-aligned square: sub-scanlines hit exactly, coverage exact.
        let sq = [PointF::new(1.0, 2.0), PointF::new(11.0, 2.0), PointF::new(11.0, 7.0), PointF::new(1.0, 7.0)];
        let m = polygon(&sq, CLIP);
        assert_eq!(m, MaskGrid::new_rect(RectI::new(1, 2, 11, 7)));
        // Orientation does not matter.
        let rev: Vec<PointF> = sq.iter().rev().copied().collect();
        assert_eq!(polygon(&rev, CLIP), m);
    }

    #[test]
    fn nonzero_winding_self_intersecting() {
        // Pentagram: nonzero rule fills the center pentagon (even-odd would leave it empty).
        let pts: Vec<PointF> = (0..5)
            .map(|i| {
                let t = std::f64::consts::TAU * (i * 2) as f64 / 5.0 - std::f64::consts::FRAC_PI_2;
                PointF::new(100.0 * t.cos(), 100.0 * t.sin())
            })
            .collect();
        let m = polygon(&pts, CLIP);
        assert_eq!(m.get(0, 0), 255);
        assert_eq!(m.get(0, -60), 255); // a star point
        // Figure-eight (bow tie): two lobes, crossing point at the center.
        let bow = [PointF::new(0.0, 0.0), PointF::new(40.0, 40.0), PointF::new(40.0, 0.0), PointF::new(0.0, 40.0)];
        let m = polygon(&bow, CLIP);
        assert_eq!(m.get(5, 20), 255);
        assert_eq!(m.get(34, 20), 255);
        assert_eq!(m.get(20, 5), 0);
        assert_eq!(m.get(20, 34), 0);
        assert!((total(&m) - 800.0).abs() < 2.0);
        // Two overlapping same-direction squares: winding 2 is still fully covered, edges not over-counted.
        let a = [PointF::new(0.0, 0.0), PointF::new(10.0, 0.0), PointF::new(10.0, 10.0), PointF::new(0.0, 10.0)];
        let b = [PointF::new(5.5, 0.0), PointF::new(15.5, 0.0), PointF::new(15.5, 10.0), PointF::new(5.5, 10.0)];
        let m = polygons(&[&a, &b], CLIP);
        assert_eq!(m.get(7, 5), 255);
        assert_eq!(m.get(15, 5), 128);
        assert!((total(&m) - 155.0).abs() < 0.05);
        // Opposite-direction inner contour makes a hole.
        let hole: Vec<PointF> =
            [PointF::new(2.0, 2.0), PointF::new(8.0, 2.0), PointF::new(8.0, 8.0), PointF::new(2.0, 8.0)]
                .into_iter()
                .rev()
                .collect();
        let m = polygons(&[&a, &hole], CLIP);
        assert_eq!(m.get(5, 5), 0);
        assert_eq!(m.get(1, 5), 255);
    }

    #[test]
    fn degenerate_and_invalid_inputs() {
        assert!(polygon(&[], CLIP).is_empty());
        assert!(polygon(&[PointF::new(0.0, 0.0), PointF::new(9.0, 9.0)], CLIP).is_empty());
        let collinear = [PointF::new(0.0, 0.0), PointF::new(5.0, 5.0), PointF::new(10.0, 10.0)];
        assert!(polygon(&collinear, CLIP).is_empty());
        let nan = [PointF::new(0.0, 0.0), PointF::new(f64::NAN, 5.0), PointF::new(10.0, 0.0)];
        assert!(polygon(&nan, CLIP).is_empty());
        let inf = [PointF::new(0.0, 0.0), PointF::new(f64::INFINITY, 5.0), PointF::new(10.0, 0.0)];
        assert!(polygon(&inf, CLIP).is_empty());
        let tri = [PointF::new(0.0, 0.0), PointF::new(10.0, 0.0), PointF::new(0.0, 10.0)];
        assert!(polygon(&tri, RectI::EMPTY).is_empty());
        assert!(polygon(&tri, RectI::new(100, 100, 200, 200)).is_empty());
    }

    #[test]
    fn huge_polygon_costs_only_clip() {
        let clip = RectI::new(0, 0, 256, 256);
        let tri = [PointF::new(-1e11, -1e11), PointF::new(1e11, -1e11), PointF::new(0.0, 1e11)];
        let m = polygon(&tri, clip);
        assert_eq!(m.bounds(), clip);
        assert_eq!(m.get(128, 128), 255);
        assert_eq!(m.byte_size(), 64 * 64);
    }

    #[test]
    fn negative_coordinates_across_tiles() {
        let m = polygon(
            &[
                PointF::new(-100.0, -100.0),
                PointF::new(-1.0, -100.0),
                PointF::new(-1.0, -1.0),
                PointF::new(-100.0, -1.0),
            ],
            CLIP,
        );
        assert_eq!(m, MaskGrid::new_rect(RectI::new(-100, -100, -1, -1)));
    }
}
