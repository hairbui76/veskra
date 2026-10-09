//! VEC-01 / PNT-010: fitting guarantee, node counts and speed on fixtures.

use std::f64::consts::{PI, TAU};
use std::time::Instant;

use veskra_core::{Affine, PointF};
use veskra_vector::{
    BezPath, FitOptions, OutlineOptions, PathEl, WidthPoint, fit_curve, fit_curve_with, max_distance_to_path,
    variable_width_outline_report,
};

fn p(x: f64, y: f64) -> PointF {
    PointF::new(x, y)
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

fn noisy_sine(n: usize, seed: u64) -> Vec<PointF> {
    let mut r = Lcg(seed);
    (0..n)
        .map(|i| {
            let x = i as f64 * 0.1;
            p(x + 0.05 * r.next(), 50.0 * (x / 30.0).sin() + 0.2 * r.next())
        })
        .collect()
}

fn fixtures() -> Vec<(&'static str, Vec<PointF>)> {
    let circle: Vec<PointF> = (0..=360)
        .map(|i| {
            let a = i as f64 / 360.0 * TAU;
            p(100.0 + 80.0 * a.cos(), 100.0 + 80.0 * a.sin())
        })
        .collect();
    let mut zig = vec![p(0.0, 0.0)];
    for k in 1..=8 {
        zig.push(p(30.0 * k as f64, if k % 2 == 1 { 60.0 } else { 0.0 }));
    }
    let mut zig_dense = Vec::new();
    for w in zig.windows(2) {
        for i in 0..20 {
            zig_dense.push(w[0].lerp(w[1], i as f64 / 20.0));
        }
    }
    zig_dense.push(*zig.last().unwrap_or(&p(0.0, 0.0)));
    let spiral: Vec<PointF> = (0..2000)
        .map(|i| {
            let th = i as f64 * 0.01;
            let rho = 4.0 + 6.0 * th / TAU;
            p(rho * th.cos(), rho * th.sin())
        })
        .collect();
    let mut hairpin: Vec<PointF> = (0..=100).map(|i| p(2.0 * i as f64, 0.0)).collect();
    hairpin.extend((1..12).map(|k| {
        let a = -PI / 2.0 + PI * k as f64 / 12.0;
        p(200.0 + 3.0 * a.cos(), 3.0 + 3.0 * a.sin())
    }));
    hairpin.extend((0..=100).map(|i| p(200.0 - 2.0 * i as f64, 6.0)));
    vec![
        ("circle_closed", circle),
        ("zigzag_sparse", zig),
        ("zigzag_dense", zig_dense),
        ("spiral_2k", spiral),
        ("hairpin", hairpin),
        ("noisy_sine_10k", noisy_sine(10_000, 3)),
    ]
}

#[test]
fn fit_error_bound_holds_on_fixtures() {
    for (name, pts) in fixtures() {
        for &e in &[0.1, 0.5, 2.0] {
            let t0 = Instant::now();
            let path = fit_curve(&pts, e);
            let dt = t0.elapsed().as_secs_f64() * 1e3;
            let d = max_distance_to_path(&path, &pts);
            println!(
                "FIT {name:<15} n={:>5} max_error={e:<4} measured={d:.4} anchors={:>4} cubics={:>4} t={dt:.2}ms",
                pts.len(),
                path.node_count(),
                path.cubic_count()
            );
            assert!(d <= e, "{name}: max distance {d} > {e}");
            if name.starts_with("zigzag") {
                assert_eq!((path.node_count(), path.cubic_count()), (9, 0), "{name}: corners must be exact lines");
            }
        }
    }
}

#[test]
fn long_input_is_fast() {
    for &n in &[10_000usize, 100_000] {
        let pts = noisy_sine(n, 11);
        let t0 = Instant::now();
        let path = fit_curve(&pts, 0.5);
        let fit_ms = t0.elapsed().as_secs_f64() * 1e3;
        let t1 = Instant::now();
        let d = max_distance_to_path(&path, &pts);
        let check_ms = t1.elapsed().as_secs_f64() * 1e3;
        println!(
            "PERF fit n={n} max_error=0.5 -> anchors={} measured={d:.4} fit={fit_ms:.1}ms metric={check_ms:.1}ms (test profile opt-level=1)",
            path.node_count()
        );
        assert!(d <= 0.5);
        assert!(fit_ms < 5_000.0, "fit too slow: {fit_ms} ms");
    }
    // Outline of a long pressure stroke: 10k samples at 1 px spacing with 0.1 px jitter
    // (stylus-like), and the pathological 0.1 px spacing / 0.2 px jitter sine.
    let mut r = Lcg(5);
    let stylus: Vec<WidthPoint> = (0..10_000)
        .map(|i| {
            let s = i as f64;
            let (x, y) = (200.0 + (40.0 + s * 0.02) * (s / 150.0).cos(), 200.0 + (40.0 + s * 0.02) * (s / 150.0).sin());
            WidthPoint { p: p(x + 0.1 * r.next(), y + 0.1 * r.next()), width: 6.0 + 4.0 * (s / 700.0).sin() }
        })
        .collect();
    let jittery: Vec<WidthPoint> = noisy_sine(10_000, 5)
        .into_iter()
        .enumerate()
        .map(|(i, q)| WidthPoint { p: q, width: 6.0 + 4.0 * (i as f64 / 700.0).sin() })
        .collect();
    for (name, wps) in [("stylus_10k", &stylus), ("jitter_10k", &jittery)] {
        for tol in [0.25, 1.0] {
            let t0 = Instant::now();
            let (outline, rep) =
                variable_width_outline_report(wps, &OutlineOptions { tolerance: tol, ..OutlineOptions::default() });
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            println!(
                "PERF outline {name} tol={tol} -> nodes={} contour={} shortcuts={} anchors={} t={ms:.1}ms",
                rep.centerline_nodes, rep.contour_points, rep.shortcut_joins, rep.anchors
            );
            assert!(!outline.is_empty());
            assert!(ms < 5_000.0);
        }
    }
}

#[test]
fn metric_does_not_underestimate() {
    // Compare max_distance_to_path against brute-force dense sampling of the fitted path.
    let pts = noisy_sine(2_000, 9);
    let path = fit_curve(&pts, 0.3);
    let fine: Vec<PointF> = path.flatten(1e-5).concat();
    let d_metric = max_distance_to_path(&path, &pts);
    let mut d_brute: f64 = 0.0;
    for q in &pts {
        let mut best = f64::INFINITY;
        for w in fine.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((q.x - a.x) * dx + (q.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            best = best.min(q.distance(p(a.x + dx * t, a.y + dy * t)));
        }
        d_brute = d_brute.max(best);
    }
    println!("METRIC brute={d_brute:.6} metric={d_metric:.6}");
    // Brute force on a 1e-5 polyline is within 1e-5 of the truth.
    assert!((d_metric - d_brute).abs() < 1e-4, "{d_metric} vs {d_brute}");
}

#[test]
fn corners_and_closure_options() {
    // A square sampled densely: corners must be exact anchors, output closed.
    let mut sq = Vec::new();
    let c = [p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0), p(0.0, 100.0), p(0.0, 0.0)];
    for w in c.windows(2) {
        for i in 0..50 {
            sq.push(w[0].lerp(w[1], i as f64 / 50.0));
        }
    }
    sq.push(p(0.0, 0.0));
    let path = fit_curve(&sq, 0.25);
    assert_eq!(path.node_count(), 4, "{:?}", path.els);
    assert!(path.els.iter().all(|e| !matches!(e, PathEl::CubicTo(..))));
    assert_eq!(path.els.last(), Some(&PathEl::Close));
    // Forcing open keeps it open.
    let open = fit_curve_with(&sq, &FitOptions { max_error: 0.25, closed: Some(false), ..FitOptions::default() });
    assert_ne!(open.els.last(), Some(&PathEl::Close));
    // Deterministic output.
    assert_eq!(fit_curve(&sq, 0.25).to_svg_d(3), path.to_svg_d(3));
}

#[test]
fn flatten_and_transform_guarantees() {
    let pts = noisy_sine(3_000, 1);
    let path = fit_curve(&pts, 0.5);
    for &tol in &[0.5, 0.05] {
        let flat: Vec<PointF> = path.flatten(tol).concat();
        // Every polyline vertex lies on the curve; every curve point is within tol of the
        // polyline: check curve samples against a path built from the polyline.
        let mut poly = BezPath::new();
        poly.move_to(flat[0]);
        for q in &flat[1..] {
            poly.line_to(*q);
        }
        let curve_samples: Vec<PointF> = path.flatten(1e-4).concat();
        let d = max_distance_to_path(&poly, &curve_samples);
        assert!(d <= tol + 1e-9, "flatten deviation {d} > {tol}");
        assert!(max_distance_to_path(&path, &flat) < 1e-6);
    }
    // Bounds are exact: equal to bounds of a very fine flattening (within its tolerance).
    let m = Affine::rotate(0.7).then(&Affine::scale(2.0, 0.5)).then(&Affine::translate(-30.0, 12.0));
    let t = path.transform(&m);
    let b = t.bounds();
    let fb = veskra_core::RectF::from_points(t.flatten(1e-6).concat());
    for (x, y) in [(b.x0, fb.x0), (b.y0, fb.y0), (b.x1, fb.x1), (b.y1, fb.y1)] {
        assert!((x - y).abs() < 1e-5, "{b:?} vs {fb:?}");
    }
}
