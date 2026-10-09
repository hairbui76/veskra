//! FMT-15 (automated part) / SPK-07 interim open-source evidence for the Illustrator handoff.
//!
//! **This is NOT Adobe Illustrator evidence.** It proves, with independent open-source tools,
//! that the files are structurally valid and render without double drawing:
//! * SVG → `rsvg-convert` (librsvg, honours `display="none"`);
//! * PDF → `pdftocairo -png -transp` (poppler, honours optional content `/OFF`), `pdftoppm`,
//!   `qpdf --check`.
//!
//! Every render is compared with Veskra's own single composite (`veskra-compose`). Doubled
//! semi-transparent strokes would be visibly darker: fixtures include a 50 %-opacity stroke and
//! probe its interior, and a negative control (hidden vectors forced visible) proves the probe
//! detects doubling. Also checks ILL-007 (dimensions/viewBox/MediaBox/labels), ILL-008 (no `.ai`)
//! and ILL-006 (preflight counts). Files land in `<repo>/target/interop/illustrator/` together
//! with `metrics.json`. A missing tool is skipped with an explicit message.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod svg_fixtures;

use std::path::{Path, PathBuf};
use std::process::Command;
use svg_fixtures::{DocBuilder, rgb, taper};
use veskra_compose::{ComposeOptions, composite_rect};
use veskra_core::export::{ExportFormat, ExportOptions, ExportWarningKind};
use veskra_core::{BlendMode, ErrorCode, PointF, RectI};
use veskra_doc::{Document, Placement};
use veskra_io::plan::{HybridStrategy, IllustratorPlan, VisibleItem};
use veskra_io::{CancelToken, Exporter, export_to_path, pdf, svg};
use veskra_vector::WidthPoint;

fn out_dir() -> PathBuf {
    let d = veskra_testkit::repo_root().join("target/interop/illustrator");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tool(name: &str) -> bool {
    let ok = Command::new(name).arg("--version").output().is_ok() || Command::new(name).arg("-v").output().is_ok();
    if !ok {
        eprintln!("SKIP: '{name}' not installed; the corresponding open-source oracle check is NOT RUN");
    }
    ok
}

/// Straight RGBA8 → premultiplied RGBA8 (diffs are measured premultiplied so invisible color
/// under alpha 0 does not count).
fn premul(px: &[u8]) -> Vec<u8> {
    px.chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
            [m(p[0]), m(p[1]), m(p[2]), p[3]]
        })
        .collect()
}

fn reference(doc: &Document) -> Vec<u8> {
    let px = composite_rect(doc, doc.bounds(), &ComposeOptions::default());
    let mut s = vec![0u8; px.len()];
    veskra_color::convert::premul16_to_straight8(&px, &mut s);
    premul(&s)
}

fn write_reference_png(doc: &Document, path: &Path) {
    let px = composite_rect(doc, doc.bounds(), &ComposeOptions::default());
    let mut s = vec![0u8; px.len()];
    veskra_color::convert::premul16_to_straight8(&px, &mut s);
    std::fs::write(path, veskra_testkit::encode_png_rgba8(doc.width(), doc.height(), &s)).unwrap();
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
struct Diff {
    max: u8,
    mean: f64,
    pct_over_16: f64,
}

fn diff(a: &[u8], b: &[u8]) -> Diff {
    assert_eq!(a.len(), b.len());
    let mut d = Diff::default();
    let mut sum = 0u64;
    let mut over = 0u64;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let m = (0..4).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0);
        d.max = d.max.max(m);
        sum += (0..4).map(|k| p[k].abs_diff(q[k]) as u64).sum::<u64>();
        if m > 16 {
            over += 1;
        }
    }
    let n = (a.len() / 4).max(1) as f64;
    d.mean = sum as f64 / (n * 4.0);
    d.pct_over_16 = over as f64 * 100.0 / n;
    d
}

fn render_svg(svg: &Path, w: u32, h: u32) -> Option<Vec<u8>> {
    let png = svg.with_extension("rsvg.png");
    let st = Command::new("rsvg-convert")
        .args(["-w", &w.to_string(), "-h", &h.to_string(), "-o"])
        .arg(&png)
        .arg(svg)
        .status()
        .ok()?;
    assert!(st.success(), "rsvg-convert failed on {}", svg.display());
    let (rw, rh, px) = veskra_testkit::decode_png_rgba8(&std::fs::read(&png).unwrap()).unwrap();
    assert_eq!((rw, rh), (w, h));
    Some(premul(&px))
}

fn render_pdf(pdf: &Path, dpi: f32, w: u32, h: u32) -> Option<Vec<u8>> {
    let prefix = pdf.with_extension("cairo");
    let st = Command::new("pdftocairo")
        // -r {dpi} maps one document pixel to one output pixel; scale-to pins the exact size
        // (poppler rounds 368.64 pt × 300/72 up to 1537 rows otherwise).
        .args(["-png", "-transp", "-singlefile", "-r", &format!("{dpi}")])
        .args(["-scale-to-x", &w.to_string(), "-scale-to-y", &h.to_string()])
        .arg(pdf)
        .arg(&prefix)
        .status()
        .ok()?;
    assert!(st.success(), "pdftocairo failed on {}", pdf.display());
    let png = PathBuf::from(format!("{}.png", prefix.display()));
    let (rw, rh, px) = veskra_testkit::decode_png_rgba8(&std::fs::read(&png).unwrap()).unwrap();
    assert_eq!((rw, rh), (w, h), "pdftocairo size for {}", pdf.display());
    Some(premul(&px))
}

fn qpdf_check(pdf: &Path) -> Option<bool> {
    let o = Command::new("qpdf").arg("--check").arg(pdf).output().ok()?;
    let text = String::from_utf8_lossy(&o.stdout);
    Some(o.status.success() && text.contains("No syntax or stream encoding errors"))
}

/// Alpha (premultiplied buffer) at document pixel probes.
fn alpha_at(px: &[u8], w: u32, pts: &[PointF]) -> Vec<u8> {
    pts.iter().map(|p| px[((p.y as u32 * w + p.x as u32) * 4 + 3) as usize]).collect()
}

struct Fixture {
    name: &'static str,
    doc: Document,
    /// Interior points of a 50 %-opacity stroke not overlapped by anything else.
    probes: Vec<PointF>,
    /// Shown as vectors (outline render differs at AA edges from brush pixels) when vector-visible.
    combos: Vec<(&'static str, ExportOptions, HybridStrategy)>,
}

fn o(v: bool, r: bool) -> ExportOptions {
    ExportOptions { include_vector_paths: v, include_raster_appearance: r, ..Default::default() }
}

const DEF: HybridStrategy = HybridStrategy::RasterVisibleVectorsHidden;
const RES: HybridStrategy = HybridStrategy::VectorsVisibleRasterResidual;

fn all_combos() -> Vec<(&'static str, ExportOptions, HybridStrategy)> {
    vec![
        ("both-default", o(true, true), DEF),
        ("both-residual", o(true, true), RES),
        ("vector-only", o(true, false), DEF),
        ("appearance-only", o(false, true), DEF),
        ("both-default-noicc", ExportOptions { embed_color_profile: false, ..o(true, true) }, DEF),
    ]
}

/// Horizontal 50 % stroke along y = `y`, constant width, from x0 to x1.
fn half_stroke(b: &mut DocBuilder, layer: veskra_core::LayerId, x0: f64, x1: f64, y: f64) -> Vec<PointF> {
    let pts: Vec<WidthPoint> = (0..=10).map(|i| WidthPoint::new(x0 + (x1 - x0) * i as f64 / 10.0, y, 16.0)).collect();
    b.ink(layer, &pts, rgb(20, 40, 200), 0.5);
    (1..10).map(|i| PointF::new(x0 + (x1 - x0) * i as f64 / 10.0, y)).collect()
}

fn fixtures() -> Vec<Fixture> {
    let mut v = Vec::new();
    {
        // Vector-only ink strokes with pressure taper (+ 50 % stroke).
        let mut b = DocBuilder::new(400, 300, 72.0);
        let ink = b.vector("Ink", Placement::Top);
        b.ink(ink, &taper(20.0, 40.0, 380.0, 60.0, 14.0), rgb(0, 0, 0), 1.0);
        b.ink(ink, &taper(30.0, 120.0, 370.0, 100.0, 22.0), rgb(200, 20, 20), 1.0);
        let probes = half_stroke(&mut b, ink, 40.0, 360.0, 240.0);
        v.push(Fixture { name: "ink_vector_only", doc: b.finish(), probes, combos: all_combos() });
    }
    {
        // Textured / airbrush raster-only strokes on a filled paper layer.
        let mut b = DocBuilder::new(300, 200, 72.0);
        let paper = b.raster("Paper", Placement::Top);
        b.fill(paper, RectI::new(10, 10, 290, 190), rgb(245, 240, 225));
        let air = b.raster("Airbrush", Placement::Top);
        b.texture(air, RectI::new(30, 30, 270, 120), rgb(40, 120, 220), 11);
        b.texture(air, RectI::new(60, 90, 240, 170), rgb(220, 60, 60), 12);
        v.push(Fixture { name: "textured_raster_only", doc: b.finish(), probes: vec![], combos: all_combos() });
    }
    {
        // Hybrid: paper fill, mixed color layer (ink + texture), vector ink with a 50 % stroke.
        let mut b = DocBuilder::new(480, 320, 72.0);
        let paper = b.raster("Paper", Placement::Top);
        b.fill(paper, RectI::new(0, 0, 480, 140), rgb(250, 245, 230));
        let color = b.raster("Color", Placement::Top);
        b.ink(color, &taper(40.0, 60.0, 440.0, 90.0, 30.0), rgb(250, 200, 60), 1.0);
        b.texture(color, RectI::new(60, 100, 300, 150), rgb(60, 160, 90), 5);
        let ink = b.vector("Ink lines", Placement::Top);
        b.ink(ink, &taper(20.0, 20.0, 460.0, 130.0, 8.0), rgb(10, 10, 10), 1.0);
        let probes = half_stroke(&mut b, ink, 40.0, 440.0, 260.0);
        v.push(Fixture { name: "hybrid", doc: b.finish(), probes, combos: all_combos() });
    }
    {
        // Non-normal blend: multiply vector layer over a filled raster layer.
        let mut b = DocBuilder::new(320, 240, 72.0);
        let paper = b.raster("Base color", Placement::Top);
        b.fill(paper, RectI::new(0, 0, 320, 240), rgb(240, 200, 120));
        let shade = b.vector("Shade (multiply)", Placement::Top);
        b.blend(shade, BlendMode::Multiply);
        b.ink(shade, &taper(20.0, 60.0, 300.0, 180.0, 40.0), rgb(90, 90, 200), 1.0);
        v.push(Fixture { name: "blend_multiply", doc: b.finish(), probes: vec![], combos: all_combos() });
    }
    {
        // Group with opacity containing two overlapping vector layers.
        let mut b = DocBuilder::new(320, 240, 72.0);
        let g = b.group("Group 60%", Placement::Top);
        b.opacity(g, 0.6);
        let a = b.vector("A", Placement::IntoGroup { group: g, index: None });
        b.ink(a, &taper(20.0, 60.0, 300.0, 80.0, 40.0), rgb(200, 0, 0), 1.0);
        let c = b.vector("B", Placement::IntoGroup { group: g, index: None });
        b.ink(c, &taper(20.0, 180.0, 300.0, 40.0, 40.0), rgb(0, 0, 200), 1.0);
        v.push(Fixture { name: "group_opacity", doc: b.finish(), probes: vec![], combos: all_combos() });
    }
    {
        // 2048 x 1536 at 300 dpi (ILL-007 physical size).
        let mut b = DocBuilder::new(2048, 1536, 300.0);
        let ink = b.vector("Ink", Placement::Top);
        b.ink(ink, &taper(100.0, 200.0, 1900.0, 400.0, 30.0), rgb(0, 0, 0), 1.0);
        let probes = half_stroke(&mut b, ink, 200.0, 1800.0, 1200.0);
        let tex = b.raster("Texture", Placement::Top);
        b.texture(tex, RectI::new(300, 600, 1700, 900), rgb(200, 80, 30), 9);
        v.push(Fixture {
            name: "large_2048x1536_300dpi",
            doc: b.finish(),
            probes,
            combos: vec![("both-default", o(true, true), DEF), ("both-residual", o(true, true), RES)],
        });
    }
    v
}

#[derive(Debug, serde::Serialize)]
struct Row {
    file: String,
    bytes: u64,
    vector_paths: u32,
    raster_images: u32,
    renderer: &'static str,
    diff: Option<Diff>,
    probe_ref: Vec<u8>,
    probe_render: Vec<u8>,
    qpdf_ok: Option<bool>,
}

/// Whether visible output for this combination is drawn by the external renderer from vectors
/// (AA edges differ from brush pixels) rather than from Veskra's embedded composite.
fn vector_visible(doc: &Document, opts: &ExportOptions, s: HybridStrategy) -> bool {
    IllustratorPlan::build(doc, opts, s).has_visible_vectors()
}

#[test]
fn interop_open_source_oracles_no_double_rendering() {
    let have_rsvg = tool("rsvg-convert");
    let have_cairo = tool("pdftocairo");
    let have_qpdf = tool("qpdf");
    let dir = out_dir();
    let mut rows = Vec::new();
    for fx in fixtures() {
        let (w, h) = (fx.doc.width(), fx.doc.height());
        let refpx = reference(&fx.doc);
        write_reference_png(&fx.doc, &dir.join(format!("{}-reference.png", fx.name)));
        let probe_ref = alpha_at(&refpx, w, &fx.probes);
        for p in &probe_ref {
            assert!((118..=138).contains(p), "{}: probe reference alpha {p} is not ~50%", fx.name);
        }
        for (label, opts, strategy) in &fx.combos {
            for format in [ExportFormat::Svg, ExportFormat::Pdf] {
                let file = dir.join(format!("{}-{label}.{}", fx.name, format.extension()));
                let rep = export_to_path(&fx.doc, format, opts, &file, &CancelToken::new())
                    .unwrap_or_else(|e| panic!("{}: {e:?}", file.display()));
                // export_to_path dispatches to the default strategy; residual uses the exporter directly.
                let rep = if *strategy == RES {
                    let ex: Box<dyn Exporter> = match format {
                        ExportFormat::Svg => Box::new(svg::SvgExporter { strategy: RES }),
                        _ => Box::new(pdf::PdfExporter { strategy: RES }),
                    };
                    let mut buf = Vec::new();
                    let r = ex.export(&fx.doc, opts, &mut buf, &CancelToken::new()).unwrap();
                    ex.validate(&buf).unwrap();
                    std::fs::write(&file, &buf).unwrap();
                    veskra_core::export::ExportReport { file_bytes: buf.len() as u64, ..r }
                } else {
                    assert!(rep.success);
                    rep
                };
                let bytes = std::fs::read(&file).unwrap();
                // ILL-007 geometry + labels.
                match format {
                    ExportFormat::Svg => {
                        let s = String::from_utf8(bytes.clone()).unwrap();
                        assert!(s.contains(&format!("width=\"{w}px\" height=\"{h}px\" viewBox=\"0 0 {w} {h}\"")));
                        let hidden = s.contains("display=\"none\"");
                        assert_eq!(hidden, !IllustratorPlan::build(&fx.doc, opts, *strategy).hidden_vectors.is_empty());
                        let plan = IllustratorPlan::build(&fx.doc, opts, *strategy);
                        let has_raster = plan.items.iter().any(|i| matches!(i, VisibleItem::Raster { .. }));
                        if opts.include_raster_appearance && has_raster {
                            assert!(s.contains("inkscape:label=\"Original appearance\""));
                        }
                        if plan.has_visible_vectors() {
                            assert!(s.contains("<g id=\"editable-vectors\" inkscape:groupmode=\"layer\" inkscape:label=\"Editable vectors\" data-name=\"Editable vectors\">"));
                        }
                        assert_eq!(s.contains(";base64,"), rep.rasterized_object_count > 0);
                        assert_eq!(s.matches("<path ").count() as u32, rep.vector_object_count);
                    }
                    _ => {
                        let sum = pdf::validate_pdf(&bytes).unwrap();
                        let k = 72.0 / fx.doc.dpi as f64;
                        let mb = sum.media_box.unwrap();
                        assert!(mb[0] == 0.0 && mb[1] == 0.0);
                        assert!((mb[2] - w as f64 * k).abs() < 1e-3 && (mb[3] - h as f64 * k).abs() < 1e-3);
                        let plan = IllustratorPlan::build(&fx.doc, opts, *strategy);
                        if !plan.hidden_vectors.is_empty() {
                            let hidden_name = if plan.has_visible_vectors() {
                                "Editable vectors (hidden edit resources)"
                            } else {
                                "Editable vectors"
                            };
                            assert_eq!(sum.off_ocgs, vec![hidden_name.to_string()]);
                        } else {
                            assert!(sum.off_ocgs.is_empty());
                        }
                        assert_eq!(sum.fill_ops as u32, rep.vector_object_count);
                        assert_eq!(bytes.windows(10).any(|x| x == b"/ICCBased "), opts.embed_color_profile);
                    }
                }
                // Render with the independent tool and compare with the single composite.
                let rendered = match format {
                    ExportFormat::Svg if have_rsvg => render_svg(&file, w, h),
                    ExportFormat::Pdf if have_cairo => render_pdf(&file, fx.doc.dpi, w, h),
                    _ => None,
                };
                let qpdf_ok = if format == ExportFormat::Pdf && have_qpdf { qpdf_check(&file) } else { None };
                if let Some(ok) = qpdf_ok {
                    assert!(ok, "qpdf --check failed for {}", file.display());
                }
                let mut row = Row {
                    file: file.file_name().unwrap().to_string_lossy().into_owned(),
                    bytes: bytes.len() as u64,
                    vector_paths: rep.vector_object_count,
                    raster_images: rep.rasterized_object_count,
                    renderer: if format == ExportFormat::Svg { "rsvg-convert" } else { "pdftocairo" },
                    diff: None,
                    probe_ref: probe_ref.clone(),
                    probe_render: vec![],
                    qpdf_ok,
                };
                if let Some(px) = rendered {
                    let d = diff(&px, &refpx);
                    row.probe_render = alpha_at(&px, w, &fx.probes);
                    let vv = vector_visible(&fx.doc, opts, *strategy);
                    // No double rendering: the semi-transparent stroke keeps its single alpha.
                    for (r, q) in row.probe_render.iter().zip(&probe_ref) {
                        assert!(r.abs_diff(*q) <= 12, "{}: probe alpha {r} vs composite {q} (doubling?)", row.file);
                    }
                    // Raster-shown output must match the composite to rounding; exact vector splits
                    // differ only at anti-aliased outline edges; approximated splits (warned in
                    // preflight) get a looser bound.
                    let approx = IllustratorPlan::build(&fx.doc, opts, *strategy).warnings.iter().any(|w| {
                        matches!(
                            w.kind,
                            ExportWarningKind::BlendModeApproximated | ExportWarningKind::ClippingApproximated
                        )
                    });
                    let ok = match (vv, approx) {
                        (false, _) => d.max <= 2,
                        (true, false) => d.pct_over_16 <= 0.1 && d.mean <= 0.1,
                        (true, true) => d.pct_over_16 <= 6.0 && d.mean <= 3.0,
                    };
                    assert!(ok, "{}: render differs from composite: {d:?}", row.file);
                    row.diff = Some(d);
                }
                rows.push(row);
            }
        }
    }
    // Negative control: forcing the hidden vector group visible must be detected as doubling.
    if have_rsvg || have_cairo {
        let fx = fixtures().into_iter().find(|f| f.name == "hybrid").unwrap();
        let (w, h) = (fx.doc.width(), fx.doc.height());
        let probe_ref = alpha_at(&reference(&fx.doc), w, &fx.probes);
        if have_rsvg {
            let src = std::fs::read_to_string(dir.join("hybrid-both-default.svg")).unwrap();
            let bad = dir.join("negative-control-hybrid-vectors-forced-visible.svg");
            std::fs::write(&bad, src.replace(" display=\"none\"", "")).unwrap();
            let px = render_svg(&bad, w, h).unwrap();
            let probes = alpha_at(&px, w, &fx.probes);
            let min_gap = probes.iter().zip(&probe_ref).map(|(a, b)| a.abs_diff(*b)).min().unwrap();
            eprintln!("negative control SVG: probe alpha {probes:?} vs single {probe_ref:?}");
            assert!(min_gap >= 40, "doubling probe failed to detect forced-visible vectors");
        }
        if have_cairo {
            let mut src = std::fs::read(dir.join("hybrid-both-default.pdf")).unwrap();
            let at = src.windows(6).position(|x| x == b"/OFF [").unwrap();
            src[at..at + 6].copy_from_slice(b"/XXX [");
            let bad = dir.join("negative-control-hybrid-vectors-forced-visible.pdf");
            std::fs::write(&bad, &src).unwrap();
            let px = render_pdf(&bad, fx.doc.dpi, w, h).unwrap();
            let probes = alpha_at(&px, w, &fx.probes);
            let min_gap = probes.iter().zip(&probe_ref).map(|(a, b)| a.abs_diff(*b)).min().unwrap();
            eprintln!("negative control PDF: probe alpha {probes:?} vs single {probe_ref:?}");
            assert!(min_gap >= 40, "doubling probe failed to detect forced-visible PDF vectors");
        }
    }
    let json = serde_json::to_string_pretty(&rows).unwrap();
    std::fs::write(dir.join("metrics.json"), json).unwrap();
    for r in &rows {
        eprintln!(
            "{:<52} {:>8} B  paths={:<3} images={} {:<12} {:?} probes={:?} qpdf={:?}",
            r.file, r.bytes, r.vector_paths, r.raster_images, r.renderer, r.diff, r.probe_render, r.qpdf_ok
        );
    }
}

#[test]
fn pdftoppm_renders_white_background_at_72dpi() {
    if !tool("pdftoppm") {
        return;
    }
    let fx = fixtures().into_iter().find(|f| f.name == "hybrid").unwrap();
    let dir = out_dir();
    let file = dir.join("hybrid-pdftoppm.pdf");
    export_to_path(&fx.doc, ExportFormat::Pdf, &ExportOptions::default(), &file, &CancelToken::new()).unwrap();
    let prefix = dir.join("hybrid-pdftoppm");
    let st =
        Command::new("pdftoppm").args(["-r", "72", "-png", "-singlefile"]).arg(&file).arg(&prefix).status().unwrap();
    assert!(st.success());
    let (w, h, px) = veskra_testkit::decode_png_rgba8(&std::fs::read(prefix.with_extension("png")).unwrap()).unwrap();
    assert_eq!((w, h), (fx.doc.width(), fx.doc.height()));
    // Composite over white for comparison.
    let refpx = reference(&fx.doc);
    let over_white: Vec<u8> = refpx
        .chunks_exact(4)
        .flat_map(|p| {
            let ia = 255 - p[3] as u32;
            [p[0] as u32 + ia, p[1] as u32 + ia, p[2] as u32 + ia].map(|v| v.min(255) as u8).into_iter().chain([255])
        })
        .collect();
    let d = diff(&px, &over_white);
    // No double rendering at the 50 % stroke probes (over white: single ≈ 137,147,227; a doubled
    // stroke would be ≈ 78,94,214).
    for p in &fx.probes {
        let i = ((p.y as u32 * w + p.x as u32) * 4) as usize;
        for k in 0..3 {
            assert!(
                px[i + k].abs_diff(over_white[i + k]) <= 12,
                "probe {p:?}: {:?} vs {:?}",
                &px[i..i + 3],
                &over_white[i..i + 3]
            );
        }
    }
    // poppler's Splash backend (pdftoppm) resamples image XObjects even at 1:1 (measured: edges
    // shifted/blurred by up to one pixel; vectors are not affected), so only a loose global bound
    // is asserted here. Cairo (pdftocairo, test above) is the exact oracle.
    eprintln!("pdftoppm -r 72 hybrid-both-default vs composite over white: {d:?}");
    assert!(d.pct_over_16 <= 6.0 && d.mean <= 3.0, "{d:?}");
}

#[test]
fn ill008_no_ai_extension_or_mime() {
    let fx = fixtures().into_iter().next().unwrap();
    let dir = out_dir();
    for fmt in [ExportFormat::Svg, ExportFormat::Pdf] {
        let bad = dir.join("artwork.ai");
        let e = export_to_path(&fx.doc, fmt, &ExportOptions::default(), &bad, &CancelToken::new()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
        assert!(!bad.exists());
        assert_ne!(fmt.extension(), "ai");
        assert!(!fmt.mime_type().contains("illustrator") && !fmt.mime_type().contains("postscript"));
        let renamed = dir.join("artwork.pdf.ai");
        assert!(export_to_path(&fx.doc, fmt, &ExportOptions::default(), &renamed, &CancelToken::new()).is_err());
    }
    let mut buf = Vec::new();
    svg::SvgExporter::default().export(&fx.doc, &ExportOptions::default(), &mut buf, &CancelToken::new()).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("not a native Adobe Illustrator (.ai) file"));
    let mut buf = Vec::new();
    pdf::PdfExporter::default().export(&fx.doc, &ExportOptions::default(), &mut buf, &CancelToken::new()).unwrap();
    assert!(buf.windows(19).any(|w| w == b"Not a native .ai fi"));
    assert!(buf.starts_with(b"%PDF-1.7"));
}

#[test]
fn ill005_profile_semantics_truthful() {
    let fx = fixtures().into_iter().find(|f| f.name == "hybrid").unwrap();
    let icc = veskra_color::IccProfile::srgb();
    for embed in [true, false] {
        let opts = ExportOptions { embed_color_profile: embed, ..Default::default() };
        let mut buf = Vec::new();
        let rep = svg::SvgExporter::default().export(&fx.doc, &opts, &mut buf, &CancelToken::new()).unwrap();
        assert_eq!(rep.profile_embedded, embed);
        assert_eq!(rep.warnings.iter().any(|w| w.kind == ExportWarningKind::ProfileNotEmbedded), !embed);
        // The embedded PNG carries iCCP only when embedding.
        let s = String::from_utf8(buf).unwrap();
        let b64 = s.split(";base64,").nth(1).unwrap().split('"').next().unwrap();
        let png = decode_b64(b64);
        assert_eq!(png.windows(4).any(|w| w == b"iCCP"), embed);
        if embed {
            assert!(rep.profile.contains("ICC embedded in raster images; vector colors are sRGB values"));
        }
        let mut buf = Vec::new();
        let rep = pdf::PdfExporter::default().export(&fx.doc, &opts, &mut buf, &CancelToken::new()).unwrap();
        assert_eq!(rep.profile_embedded, embed);
        let has_icc_stream = buf.windows(28).any(|w| w == b"/N 3 /Alternate /DeviceRGB /");
        assert_eq!(has_icc_stream, embed);
        if embed {
            // The ICC stream decompresses to exactly Veskra's sRGB profile bytes.
            let at = buf.windows(28).position(|w| w == b"/N 3 /Alternate /DeviceRGB /").unwrap();
            let s0 = at + buf[at..].windows(7).position(|w| w == b"stream\n").unwrap() + 7;
            let e0 = s0 + buf[s0..].windows(10).position(|w| w == b"\nendstream").unwrap();
            let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&buf[s0..e0]).unwrap();
            assert_eq!(raw, icc.bytes());
        }
    }
}

fn decode_b64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    } as u32;
    let bytes: Vec<u8> = s.bytes().filter(|c| *c != b'=').collect();
    let mut out = Vec::new();
    for q in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in q.iter().enumerate() {
            n |= val(*c) << (18 - 6 * i);
        }
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..q.len() - 1]);
    }
    out
}

#[test]
fn deterministic_bytes() {
    for fx in fixtures().into_iter().filter(|f| f.name != "large_2048x1536_300dpi") {
        for s in HybridStrategy::ALL {
            let ex: [Box<dyn Exporter>; 2] =
                [Box::new(svg::SvgExporter { strategy: s }), Box::new(pdf::PdfExporter { strategy: s })];
            for e in ex {
                let mut a = Vec::new();
                let mut b = Vec::new();
                e.export(&fx.doc, &ExportOptions::default(), &mut a, &CancelToken::new()).unwrap();
                e.export(&fx.doc, &ExportOptions::default(), &mut b, &CancelToken::new()).unwrap();
                assert_eq!(a, b, "{} {:?} not deterministic", fx.name, e.format());
            }
        }
    }
}
