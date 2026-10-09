//! VEC-02 coverage proof: the fitted outline (nonzero fill) must match the exact union of
//! the swept discs. For every fixture we rasterize, with point samples on the same grid:
//!   (a) the outline path (independent nonzero scanline rasterizer), and
//!   (b) the union of discs along the densely interpolated centerline (spacing 0.05 px),
//! and assert:
//!   * symmetric-difference area <= perimeter × τ  (mean boundary deviation below τ);
//!   * NO notches: every sample inside the union of discs shrunk by 2τ is covered;
//!   * NO spikes: no covered sample outside the union of discs grown by 2τ;
//!   * fitted path within 0.7τ of every vertex of the exact contour polyline.
//!
//! Then the outline is written as SVG and, if `rsvg-convert` exists, rendered by that
//! independent renderer and compared against the reference raster.

mod support;

use std::f64::consts::{PI, TAU};
use std::path::PathBuf;
use std::process::Command;

use support::*;
use veskra_vector::{OutlineOptions, WidthPoint, max_distance_to_path, outline_contour, variable_width_outline_report};

const TOL: f64 = 0.25;
const SPP: usize = 8; // samples per px per axis
const ZOOM: usize = 2; // rsvg render scale

fn wp(x: f64, y: f64, w: f64) -> WidthPoint {
    WidthPoint::new(x, y, w)
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

fn straight() -> Vec<WidthPoint> {
    (0..=150).map(|i| wp(20.0 + 2.0 * i as f64, 40.0, 12.0)).collect()
}

fn taper() -> Vec<WidthPoint> {
    (0..=200)
        .map(|i| {
            let t = i as f64 / 200.0;
            wp(20.0 + 300.0 * t, 60.0 + 30.0 * (PI * t).sin(), 30.0 * (PI * t).sin())
        })
        .collect()
}

fn s_curve() -> Vec<WidthPoint> {
    let mut rng = Lcg(7);
    (0..=300)
        .map(|i| {
            let t = i as f64 / 300.0;
            wp(20.0 + 300.0 * t + 0.3 * rng.next(), 80.0 + 50.0 * (TAU * t).sin() + 0.3 * rng.next(), 10.0 + 10.0 * t)
        })
        .collect()
}

fn hairpin() -> Vec<WidthPoint> {
    let mut v: Vec<WidthPoint> = (0..=100).map(|i| wp(20.0 + 2.0 * i as f64, 50.0, 16.0)).collect();
    for k in 1..12 {
        let a = -PI / 2.0 + PI * k as f64 / 12.0;
        v.push(wp(220.0 + 3.0 * a.cos(), 53.0 + 3.0 * a.sin(), 16.0));
    }
    v.extend((0..=100).map(|i| wp(220.0 - 2.0 * i as f64, 56.0, 16.0)));
    v
}

fn spiral() -> Vec<WidthPoint> {
    // Archimedean spiral, arm spacing 6 px, stroke width 10 px (arms overlap).
    let mut v = Vec::new();
    let mut th: f64 = 0.0;
    while th <= 6.0 * PI {
        let rho = 4.0 + 6.0 * th / TAU;
        v.push(wp(150.0 + rho * th.cos(), 150.0 + rho * th.sin(), 10.0));
        th += 1.5 / rho;
    }
    v
}

fn zigzag() -> Vec<WidthPoint> {
    // Legs alternate between +75° and -75°: 150° turning angle at each corner.
    let mut v = vec![wp(20.0, 120.0, 8.0)];
    let mut p = (20.0, 120.0);
    for leg in 0..6 {
        let ang: f64 = if leg % 2 == 0 { -75f64.to_radians() } else { 75f64.to_radians() };
        for _ in 0..30 {
            p = (p.0 + 2.0 * ang.cos(), p.1 + 2.0 * ang.sin());
            v.push(wp(p.0, p.1, 8.0));
        }
    }
    v
}

fn width_jump() -> Vec<WidthPoint> {
    (0..=90)
        .map(|i| {
            let x = 20.0 + 2.0 * i as f64;
            let w = if (102.0..=150.0).contains(&x) { 40.0 } else { 1.0 };
            wp(x, 60.0, w)
        })
        .collect()
}

fn dot() -> Vec<WidthPoint> {
    vec![wp(50.0, 50.0, 20.0)]
}

fn svg_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("veskra-vector-coverage-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

fn check(name: &str, pts: &[WidthPoint]) {
    let opts = OutlineOptions { tolerance: TOL, ..OutlineOptions::default() };
    let t0 = std::time::Instant::now();
    let (path, report) = variable_width_outline_report(pts, &opts);
    let build_ms = t0.elapsed().as_secs_f64() * 1e3;
    assert!(!path.is_empty(), "{name}: empty outline");

    // Fitting step: path within 0.7τ of the exact contour vertices.
    let contour = outline_contour(pts, &opts);
    let fit_err = max_distance_to_path(&path, &contour);
    assert!(fit_err <= 0.7 * TOL + 1e-9, "{name}: fit error {fit_err}");

    let discs = dense_discs(pts, 0.05, opts.min_width);
    let g = grid_for(&discs, 6.0, SPP);
    let reference = raster_discs(&discs, 0.0, &g);
    let inner = raster_discs(&discs, -2.0 * TOL, &g);
    let outer = raster_discs(&discs, 2.0 * TOL, &g);
    let out = raster_path(&path, &g);

    let aps = g.area_per_sample();
    let xor = reference.iter().zip(&out).filter(|(a, b)| a != b).count();
    let notches = inner.iter().zip(&out).filter(|(i, o)| **i && !**o).count();
    let spikes = outer.iter().zip(&out).filter(|(u, o)| !**u && **o).count();
    let perim = perimeter_estimate(&reference, &g);
    let sym_area = xor as f64 * aps;
    let ref_area = reference.iter().filter(|v| **v).count() as f64 * aps;

    // Independent renderer smoke test.
    let svg_note = render_with_rsvg(name, &path, &g, &reference, &out, perim);

    println!(
        "FIXTURE {name:<11} in={:>4} nodes={:>3} contour={:>5} shortcuts={:>3} anchors={:>3} | area={:>8.1} \
         symdiff={:>6.2}px² perim={:>7.1} mean_dev={:.4}px notches={} spikes={} fit_err={:.4} t={:.1}ms | {svg_note}",
        report.input_points,
        report.centerline_nodes,
        report.contour_points,
        report.shortcut_joins,
        report.anchors,
        ref_area,
        sym_area,
        perim,
        sym_area / perim,
        notches,
        spikes,
        fit_err,
        build_ms
    );
    assert_eq!(notches, 0, "{name}: uncovered samples deep inside the stroke");
    assert_eq!(spikes, 0, "{name}: covered samples far outside the stroke");
    assert!(sym_area <= perim * TOL, "{name}: symmetric difference {sym_area} > {}", perim * TOL);
}

/// Returns a note for the report; asserts agreement when the renderer is available.
fn render_with_rsvg(
    name: &str,
    path: &veskra_vector::BezPath,
    g: &Grid,
    reference: &[bool],
    own: &[bool],
    perim: f64,
) -> String {
    let dir = svg_dir();
    let svg_path = dir.join(format!("{name}.svg"));
    let png_path = dir.join(format!("{name}.png"));
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{x} {y} {vw} {vh}\">\
         <path fill=\"#000000\" fill-rule=\"nonzero\" d=\"{d}\"/></svg>\n",
        w = g.w * ZOOM,
        h = g.h * ZOOM,
        x = g.x0,
        y = g.y0,
        vw = g.w,
        vh = g.h,
        d = path.to_svg_d(3)
    );
    std::fs::write(&svg_path, svg).expect("write svg");
    let bin = "rsvg-convert";
    let status = Command::new(bin).arg("-f").arg("png").arg("-o").arg(&png_path).arg(&svg_path).status();
    match status {
        Err(_) => {
            println!("NOTE: `{bin}` not found; independent-renderer check SKIPPED for {name}");
            return "rsvg: skipped (binary missing)".into();
        }
        Ok(s) if !s.success() => panic!("{name}: rsvg-convert failed: {s}"),
        Ok(_) => {}
    }
    let img = decode_png(&std::fs::read(&png_path).expect("read png")).expect("decode png");
    assert_eq!((img.w, img.h), (g.w * ZOOM, g.h * ZOOM), "{name}: rendered size");
    // Compare per rendered pixel: rsvg alpha vs fraction of reference samples.
    let k = SPP / ZOOM; // samples per rendered pixel per axis
    let cols = g.cols();
    let frac = |mask: &[bool], px: usize, py: usize| {
        let mut n = 0;
        for sy in 0..k {
            for sx in 0..k {
                if mask[(py * k + sy) * cols + px * k + sx] {
                    n += 1;
                }
            }
        }
        n as f64 / (k * k) as f64
    };
    let pix_area = 1.0 / (ZOOM * ZOOM) as f64;
    let (mut d_ref, mut d_own) = (0.0, 0.0);
    for py in 0..img.h {
        for px in 0..img.w {
            let a = img.alpha[py * img.w + px] as f64 / 255.0;
            d_ref += (a - frac(reference, px, py)).abs() * pix_area;
            d_own += (a - frac(own, px, py)).abs() * pix_area;
        }
    }
    // Slack: τ for geometry + 0.25 px for antialiasing/sampling quantization along the edge.
    let bound = perim * (TOL + 0.25);
    assert!(d_ref <= bound, "{name}: rsvg vs reference |Δα| area {d_ref} > {bound}");
    format!("rsvg: |Δ|area vs ref={d_ref:.2}px² ({:.4}px/len) vs own raster={d_own:.2}px²", d_ref / perim)
}

#[test]
fn straight_constant_width() {
    check("straight", &straight());
}
#[test]
fn pressure_taper() {
    check("taper", &taper());
}
#[test]
fn s_curve_noisy() {
    check("s_curve", &s_curve());
}
#[test]
fn hairpin_180() {
    check("hairpin", &hairpin());
}
#[test]
fn tight_spiral() {
    check("spiral", &spiral());
}
#[test]
fn zigzag_150_corners() {
    check("zigzag", &zigzag());
}
#[test]
fn width_jump_1_to_40() {
    check("width_jump", &width_jump());
}
#[test]
fn single_dot() {
    check("dot", &dot());
}
