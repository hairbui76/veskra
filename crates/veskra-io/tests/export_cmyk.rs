//! CMYK print export (ADR-018): PSD color mode 4 and PDF DeviceCMYK + output intent, preflight
//! and report fields, untagged CMYK, user profiles and their errors, refusal for PNG/SVG, and
//! structural validation of CMYK files.
//!
//! Pixel expectations come from `veskra-color`'s `CmykTransform` (whose accuracy against LittleCMS
//! is measured in `crates/veskra-color/tests/cmyk_oracle.rs`): exactly through the device link the
//! writer uses (plumbing), and statistically against the direct pipeline.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::needless_range_loop)]

mod psd_fixtures;
mod svg_fixtures;

use psd_fixtures as fx;
use svg_fixtures::{DocBuilder, rgb, taper};
use veskra_color::cmyk::{BUNDLED_CMYK_PROFILE, CmykProfile, CmykTransform, GAMUT_DELTA_E00};
use veskra_color::{IccProfile, RenderingIntent, convert};
use veskra_core::export::{
    ExportFormat, ExportIntent, ExportOptions, ExportProfile, ExportReport, ExportWarningKind as Kind, ValidationStatus,
};
use veskra_core::limits::MAX_OUTPUT_PROFILE_BYTES;
use veskra_core::{BlendMode, ErrorCode, RectI};
use veskra_doc::{Document, Placement};
use veskra_io::pdf::validate_pdf;
use veskra_io::psd::inspect_psd;
use veskra_io::{CancelToken, export_to_path, exporter_for};

fn export(doc: &Document, format: ExportFormat, opts: &ExportOptions) -> (Vec<u8>, ExportReport) {
    let exp = exporter_for(format);
    let mut out = Vec::new();
    let rep = exp.export(doc, opts, &mut out, &CancelToken::new()).unwrap();
    exp.validate(&out).unwrap();
    (out, rep)
}

fn cmyk(intent: ExportIntent, bpc: bool) -> ExportOptions {
    ExportOptions {
        profile: ExportProfile::Cmyk,
        rendering_intent: intent,
        black_point_compensation: bpc,
        ..Default::default()
    }
}

fn rel() -> ExportOptions {
    cmyk(ExportIntent::RelativeColorimetric, true)
}

fn transform(intent: RenderingIntent, bpc: bool) -> CmykTransform {
    CmykTransform::new(&IccProfile::srgb(), CmykProfile::bundled().unwrap(), intent, bpc).unwrap()
}

fn has(rep: &ExportReport, k: Kind) -> bool {
    rep.warnings.iter().any(|w| w.kind == k)
}

/// Opaque sRGB swatches (grays + a 9×9×9 cube + saturated primaries), 1 px each.
fn swatches() -> (Document, Vec<[u8; 3]>) {
    let mut cols: Vec<[u8; 3]> = (0..=255u8).step_by(5).map(|g| [g, g, g]).collect();
    for r in 0..9u16 {
        for g in 0..9u16 {
            for b in 0..9u16 {
                cols.push([(r * 255 / 8) as u8, (g * 255 / 8) as u8, (b * 255 / 8) as u8]);
            }
        }
    }
    let w = 32usize;
    let h = cols.len().div_ceil(w);
    cols.resize(w * h, [255, 255, 255]);
    let mut d = fx::new_doc(w as u32, h as u32);
    let c2 = cols.clone();
    let g = fx::grid(RectI::new(0, 0, w as i32, h as i32), move |x, y| {
        let c = c2[y as usize * w + x as usize];
        [c[0], c[1], c[2], 255]
    });
    fx::add_pixels(&mut d, 1, None, g, None, fx::props("Swatches", BlendMode::Normal, 1.0));
    (d, cols)
}

/// What the writer stores for an opaque sRGB color: the device link's ink, 16-bit.
fn link_ink16(t: &CmykTransform, c: [u8; 3]) -> [u16; 4] {
    let v = t.link().eval(c.map(|x| (x as u16 * 257) as f32 / 65535.0));
    std::array::from_fn(|k| (v[k].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)
}

fn plane(l: &veskra_io::psd::PsdLayerInfo, id: i16) -> &[u8] {
    &l.planes.iter().find(|(i, _)| *i == id).unwrap().1
}

#[test]
fn psd_cmyk_8_bit_structure_pixels_and_report() {
    let (doc, cols) = swatches();
    let (bytes, rep) = export(&doc, ExportFormat::Psd, &rel());
    let info = inspect_psd(&bytes, true).unwrap();
    assert_eq!((info.color_mode, info.depth, info.channels), (4, 8, 5));
    assert_eq!(info.resource(1039), Some(BUNDLED_CMYK_PROFILE), "profile embedded byte for byte");
    let (rgb_bytes, _) = export(&doc, ExportFormat::Psd, &ExportOptions::default());
    assert_eq!(info.resource(1005), inspect_psd(&rgb_bytes, false).unwrap().resource(1005), "resolution unchanged");
    let layer = &info.layers[0];
    assert_eq!(layer.channels.iter().map(|c| c.0).collect::<Vec<_>>(), [-1, 0, 1, 2, 3]);

    let t = transform(RenderingIntent::RelativeColorimetric, true);
    let mut worst_link = 0i32;
    let mut direct_diffs = Vec::new();
    let mut oog = 0u64;
    let mut tac = 0.0f64;
    for (i, c) in cols.iter().enumerate() {
        let ink = link_ink16(&t, *c);
        let rgbf = c.map(|v| v as f64 / 255.0);
        let (direct, de) = t.rgb_to_cmyk_with_delta_e(rgbf);
        let lde = t.link().eval(c.map(|x| (x as u16 * 257) as f32 / 65535.0))[4];
        oog += (lde > GAMUT_DELTA_E00 as f32) as u64;
        let _ = de;
        tac = tac.max(ink.iter().map(|&v| v as f64).sum::<f64>() / 65535.0 * 100.0);
        for k in 0..4 {
            let stored = convert::u16_to_u8(65535 - ink[k]);
            // Inverted: 255 = no ink. Layer pixels and the merged image (opaque → no matting).
            for got in [plane(layer, k as i16)[i], info.merged[k][i]] {
                worst_link = worst_link.max((got as i32 - stored as i32).abs());
            }
            direct_diffs.push((255.0 - layer_value(layer, k, i)) / 255.0 * 100.0 - direct[k] * 100.0);
        }
        assert_eq!(plane(layer, -1)[i], 255);
        assert_eq!(info.merged[4][i], 255);
    }
    assert!(worst_link <= 1, "writer vs device link: {worst_link} codes");
    let mut d: Vec<f64> = direct_diffs.iter().map(|v| v.abs()).collect();
    d.sort_by(f64::total_cmp);
    let (med, p95, max) = (d[d.len() / 2], d[d.len() * 95 / 100], d[d.len() - 1]);
    println!("PSD 8-bit vs direct pipeline: ink % median {med:.3} p95 {p95:.3} max {max:.3}");
    assert!(med <= 0.5 && p95 <= 1.5 && max <= 5.0);
    // White is paper: no ink at all.
    let white = cols.iter().position(|c| *c == [255, 255, 255]).unwrap();
    assert!((0..4).all(|k| info.merged[k][white] == 255));

    assert_eq!(rep.target_profile, "Coated_Fogra39L_VIGC_300.icc");
    assert_eq!(rep.rendering_intent.as_deref(), Some("relative-colorimetric"));
    let p = rep.cmyk_profile.clone().unwrap();
    assert!(p.bundled && p.description == "Coated_Fogra39L_VIGC_300.icc");
    assert!(rep.profile_embedded);
    assert_eq!(rep.out_of_gamut_pixels, oog);
    assert!(oog > 0, "saturated sRGB swatches are outside FOGRA39");
    let got_tac = rep.total_area_coverage_max.unwrap();
    assert!((got_tac - tac).abs() < 0.02, "{got_tac} vs {tac}");
    assert!(got_tac > 250.0 && got_tac <= 305.0, "FOGRA39 VIGC limits ink to ~300 %: {got_tac}");
    assert!(has(&rep, Kind::ColorConverted) && has(&rep, Kind::GamutClipped));
    assert!(rep.profile.contains("CMYK") && rep.profile.contains("black point compensation"), "{}", rep.profile);
    let json = serde_json::to_value(&rep).unwrap();
    assert_eq!(json["cmykProfile"]["bundled"], true);
    assert!(json["totalAreaCoverageMax"].as_f64().is_some());
}

fn layer_value(l: &veskra_io::psd::PsdLayerInfo, k: usize, i: usize) -> f64 {
    plane(l, k as i16)[i] as f64
}

#[test]
fn psd_cmyk_16_bit_matches_the_link_exactly() {
    let (doc, cols) = swatches();
    let o = ExportOptions { bit_depth: 16, ..rel() };
    let (bytes, rep) = export(&doc, ExportFormat::Psd, &o);
    let info = inspect_psd(&bytes, true).unwrap();
    assert_eq!((info.color_mode, info.depth, info.channels), (4, 16, 5));
    assert_eq!(rep.bit_depth, 16);
    let t = transform(RenderingIntent::RelativeColorimetric, true);
    let layer = &info.layers[0];
    let be = |p: &[u8], i: usize| u16::from_be_bytes([p[2 * i], p[2 * i + 1]]);
    let mut worst = 0i32;
    for (i, c) in cols.iter().enumerate() {
        let ink = link_ink16(&t, *c);
        for k in 0..4 {
            let want = 65535 - ink[k];
            worst = worst.max((be(plane(layer, k as i16), i) as i32 - want as i32).abs());
            worst = worst.max((be(&info.merged[k], i) as i32 - want as i32).abs());
        }
        assert_eq!(be(&info.merged[4], i), 65535);
    }
    assert!(worst <= 1, "{worst}");
}

#[test]
fn psd_cmyk_layers_groups_masks_and_transparency() {
    for (name, doc) in fx::all() {
        let (bytes, rep) = export(&doc, ExportFormat::Psd, &rel());
        let info = inspect_psd(&bytes, true).unwrap();
        assert_eq!(info.color_mode, 4, "{name}");
        for l in &info.layers {
            let ids: Vec<i16> = l.channels.iter().map(|c| c.0).collect();
            assert!(ids.starts_with(&[-1, 0, 1, 2, 3]), "{name}: {ids:?}");
        }
        let blends = doc.iter_layers().filter(|l| l.props.blend != BlendMode::Normal).count();
        if blends > 0 {
            assert!(has(&rep, Kind::BlendModeApproximated), "{name}");
        }
        if info.channels == 5 {
            // Merged image: transparent → no ink; ink never exceeds the opaque color's ink.
            let n = (info.width * info.height) as usize;
            for i in 0..n {
                if info.merged[4][i] == 0 {
                    assert!((0..4).all(|k| info.merged[k][i] == 255), "{name}: pixel {i}");
                }
            }
        }
        // Same layer structure as the RGB file (records, names, flags, masks).
        let rgb = inspect_psd(&export(&doc, ExportFormat::Psd, &ExportOptions::default()).0, false).unwrap();
        assert_eq!(rgb.layers.len(), info.layers.len(), "{name}");
        for (a, b) in rgb.layers.iter().zip(&info.layers) {
            assert_eq!(
                (a.display_name(), a.flags, a.opacity, a.blend_key, a.mask.is_some(), a.section),
                (b.display_name(), b.flags, b.opacity, b.blend_key, b.mask.is_some(), b.section),
                "{name}"
            );
        }
    }
}

#[test]
fn intents_bpc_and_untagged_cmyk() {
    let (doc, cols) = swatches();
    let black = cols.iter().position(|c| *c == [0, 0, 0]).unwrap();
    let k_of = |o: &ExportOptions| {
        let (b, r) = export(&doc, ExportFormat::Psd, o);
        let i = inspect_psd(&b, true).unwrap();
        ((0..4).map(|k| i.merged[k][black]).collect::<Vec<_>>(), r, i)
    };
    let (rel_bpc, _, _) = k_of(&rel());
    let (rel_nobpc, r2, _) = k_of(&cmyk(ExportIntent::RelativeColorimetric, false));
    let (perc, r3, _) = k_of(&cmyk(ExportIntent::Perceptual, true));
    assert_ne!(rel_bpc, rel_nobpc, "BPC changes the black");
    assert_ne!(rel_bpc, perc, "perceptual uses different tables");
    assert_eq!(r3.rendering_intent.as_deref(), Some("perceptual"));
    assert!(!r2.profile.contains("black point compensation"), "{}", r2.profile);

    let (_, rep, info) = k_of(&ExportOptions { embed_color_profile: false, ..rel() });
    assert!(info.resource(1039).is_none());
    assert!(!rep.profile_embedded);
    assert!(has(&rep, Kind::ProfileNotEmbedded));
    assert_eq!(info.color_mode, 4);
    let pf = exporter_for(ExportFormat::Psd).preflight(&doc, &ExportOptions { embed_color_profile: false, ..rel() });
    assert!(pf.blocked_reason.is_none(), "untagged CMYK is allowed");
}

#[test]
fn preflight_reports_cmyk_target_and_estimate() {
    let (doc, _) = swatches();
    for format in [ExportFormat::Psd, ExportFormat::Pdf] {
        let pf = exporter_for(format).preflight(&doc, &rel());
        assert!(pf.blocked_reason.is_none(), "{format:?}: {:?}", pf.blocked_reason);
        assert_eq!(pf.target_profile, "Coated_Fogra39L_VIGC_300.icc");
        assert_eq!(pf.rendering_intent.as_deref(), Some("relative-colorimetric"));
        assert!(pf.gamut_clipping_possible);
        let tac = pf.total_area_coverage_max.unwrap();
        assert!(tac > 250.0 && tac <= 305.0, "{tac}");
        assert!(pf.cmyk_profile.as_ref().unwrap().bundled);
        assert!(pf.warnings.iter().any(|w| w.kind == Kind::GamutClipped && w.count > 0), "{format:?}");
        // The exact report agrees with the (full-coverage, small document) estimate.
        let (_, rep) = export(&doc, format, &rel());
        assert!((rep.total_area_coverage_max.unwrap() - tac).abs() < 0.02);
    }
    // RGB preflight: no CMYK fields.
    let pf = exporter_for(ExportFormat::Psd).preflight(&doc, &ExportOptions::default());
    assert!(pf.total_area_coverage_max.is_none() && pf.cmyk_profile.is_none());
}

#[test]
fn png_and_svg_refuse_cmyk() {
    let doc = fx::simple();
    let dir = std::env::temp_dir().join(format!("veskra-cmyk-refuse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for format in [ExportFormat::Png, ExportFormat::Svg] {
        let pf = exporter_for(format).preflight(&doc, &rel());
        let reason = pf.blocked_reason.unwrap();
        assert!(reason.contains("CMYK") && (reason.contains("PSD") || reason.contains("PDF")), "{reason}");
        let path = dir.join(format!("x.{}", format.extension()));
        let e = export_to_path(&doc, format, &rel(), &path, &CancelToken::new()).unwrap_err();
        assert_eq!(e.code, ErrorCode::IncompatibleOutputProfile);
        assert!(!path.exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn user_profiles_and_bad_profiles() {
    let doc = fx::simple();
    let dir = std::env::temp_dir().join(format!("veskra-cmyk-user-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let user = dir.join("press.icc");
    std::fs::write(&user, BUNDLED_CMYK_PROFILE).unwrap();
    let o = ExportOptions { cmyk_profile: Some(user.to_string_lossy().into_owned()), ..rel() };
    let (a, rep) = export(&doc, ExportFormat::Psd, &o);
    assert!(!rep.cmyk_profile.as_ref().unwrap().bundled);
    let (b, _) = export(&doc, ExportFormat::Psd, &rel());
    assert_eq!(a, b, "same profile bytes → same file");

    let srgb = dir.join("srgb.icc");
    std::fs::write(&srgb, IccProfile::srgb().bytes()).unwrap();
    let junk = dir.join("junk.icc");
    std::fs::write(&junk, b"this is not a profile").unwrap();
    let big = dir.join("big.icc");
    std::fs::File::create(&big).unwrap().set_len(MAX_OUTPUT_PROFILE_BYTES as u64 + 1).unwrap();
    let cases = [
        (dir.join("missing.icc"), ErrorCode::NotFound, "not found"),
        (srgb, ErrorCode::UnsupportedColorMode, "not a CMYK output"),
        (junk, ErrorCode::InvalidProfile, "not a usable ICC profile"),
        (big, ErrorCode::AllocationLimit, "larger than 32 MB"),
    ];
    for (path, code, text) in cases {
        let o = ExportOptions { cmyk_profile: Some(path.to_string_lossy().into_owned()), ..rel() };
        for format in [ExportFormat::Psd, ExportFormat::Pdf] {
            let pf = exporter_for(format).preflight(&doc, &o);
            let reason = pf.blocked_reason.unwrap_or_default();
            assert!(reason.contains(text), "{format:?} {path:?}: {reason}");
            assert!(!reason.contains(&*path.to_string_lossy()), "no path in the message");
            let out = dir.join(format!("out.{}", format.extension()));
            let e = export_to_path(&doc, format, &o, &out, &CancelToken::new()).unwrap_err();
            assert_eq!(e.code, code, "{format:?} {path:?}");
            assert!(!out.exists());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn hybrid() -> Document {
    let mut b = DocBuilder::new(320, 200, 72.0);
    let paper = b.raster("Paper", Placement::Top);
    b.fill(paper, RectI::new(10, 10, 310, 190), rgb(245, 240, 225));
    let ink = b.vector("Ink", Placement::Top);
    b.ink(ink, &taper(30.0, 60.0, 290.0, 80.0, 16.0), rgb(200, 20, 20), 1.0);
    b.ink(ink, &taper(30.0, 130.0, 290.0, 150.0, 12.0), rgb(20, 60, 200), 0.7);
    b.finish()
}

/// Inflate every Flate stream of a PDF.
fn pdf_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = pdf[i..].windows(7).position(|w| w == b"stream\n") {
        let start = i + p + 7;
        let end = start + pdf[start..].windows(10).position(|w| w == b"\nendstream").unwrap();
        if let Ok(d) = miniz_oxide::inflate::decompress_to_vec_zlib(&pdf[start..end]) {
            out.push(d);
        }
        i = end + 10;
    }
    out
}

fn contains(h: &[u8], n: &[u8]) -> bool {
    h.windows(n.len()).any(|w| w == n)
}

#[test]
fn pdf_cmyk_device_cmyk_with_output_intent() {
    let doc = hybrid();
    let (pdf, rep) = export(&doc, ExportFormat::Pdf, &rel());
    let s = validate_pdf(&pdf).unwrap();
    assert_eq!(s.output_intent_components, Some(4));
    assert_eq!(s.page_group_cs.as_deref(), Some("/DeviceCMYK"));
    assert!(s.cmyk_images >= 1, "{s:?}");
    assert!(s.cmyk_fill_ops >= 2, "editable vector fills use the k operator: {s:?}");
    assert!(!contains(&pdf, b"/ICCBased") && !contains(&pdf, b"/DeviceRGB"), "no RGB left on the page");
    assert!(contains(&pdf, b"/GTS_PDFX") && contains(&pdf, b"(FOGRA39)"));
    let streams = pdf_streams(&pdf);
    assert_eq!(streams.iter().filter(|d| d.as_slice() == BUNDLED_CMYK_PROFILE).count(), 1, "profile embedded once");
    // Vector fill color = the direct conversion of the stroke color.
    let t = transform(RenderingIntent::RelativeColorimetric, true);
    let red = t.rgb_to_cmyk([200.0 / 255.0, 20.0 / 255.0, 20.0 / 255.0]);
    let fmt = |v: f64| {
        let s = format!("{v:.4}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() || s == "-0" { "0".to_string() } else { s }
    };
    let op = format!("{} {} {} {} k", fmt(red[0]), fmt(red[1]), fmt(red[2]), fmt(red[3]));
    assert!(streams.iter().any(|d| contains(d, op.as_bytes())), "missing '{op}'");
    assert_eq!(rep.target_profile, "Coated_Fogra39L_VIGC_300.icc");
    assert!(rep.total_area_coverage_max.unwrap() > 0.0);
    assert!(rep.cmyk_profile.unwrap().bundled);

    // Untagged: DeviceCMYK without an output intent, warned.
    let (pdf, rep) = export(&doc, ExportFormat::Pdf, &ExportOptions { embed_color_profile: false, ..rel() });
    let s = validate_pdf(&pdf).unwrap();
    assert_eq!(s.output_intent_components, None);
    assert!(s.cmyk_images >= 1);
    assert!(has(&rep, Kind::ProfileNotEmbedded) && !rep.profile_embedded);
    assert!(!contains(&pdf, b"/OutputIntents"));

    // 'Preserve original appearance' only: everything as CMYK images, no fills.
    let o = ExportOptions { include_vector_paths: false, ..rel() };
    let (pdf, _) = export(&doc, ExportFormat::Pdf, &o);
    let s = validate_pdf(&pdf).unwrap();
    assert_eq!(s.cmyk_fill_ops, 0);
    assert!(s.cmyk_images >= 1);
}

#[test]
fn structural_validation_covers_cmyk() {
    // A CMYK PSD whose records only carry RGB channels is rejected.
    let doc = fx::six_layer();
    let (mut rgb, _) = export(&doc, ExportFormat::Psd, &ExportOptions::default());
    rgb[25] = 4; // color mode 3 → 4; the 4 merged planes still decode, the records lack channel 3
    let e = inspect_psd(&rgb, false).unwrap_err();
    assert!(e.message.contains("color channel"), "{e:?}");
    // A DeviceCMYK image relabelled as DeviceRGB no longer matches its sample count.
    let (mut pdf, _) = export(&hybrid(), ExportFormat::Pdf, &rel());
    let pat = b"/ColorSpace /DeviceCMYK";
    let at = pdf.windows(pat.len()).position(|w| w == pat).unwrap();
    pdf[at..at + pat.len()].copy_from_slice(b"/ColorSpace /DeviceRGB ");
    let e = validate_pdf(&pdf).unwrap_err();
    assert!(e.contains("image data"), "{e}");
    // Files written through export_to_path are structurally validated.
    let dir = std::env::temp_dir().join(format!("veskra-cmyk-valid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for format in [ExportFormat::Psd, ExportFormat::Pdf] {
        let path = dir.join(format!("print.{}", format.extension()));
        let rep = export_to_path(&doc, format, &rel(), &path, &CancelToken::new()).unwrap();
        assert!(rep.success && rep.validation == ValidationStatus::StructurallyValidated);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rgb_conversions_ignore_bpc_and_report_the_requested_intent() {
    // Intent/BPC options do not change RGB bytes (matrix/TRC: perceptual == relative, BPC no-op).
    let (doc, _) = swatches();
    for format in [ExportFormat::Png, ExportFormat::Psd, ExportFormat::Pdf] {
        let base = ExportOptions { profile: ExportProfile::DisplayP3, ..Default::default() };
        let (a, ra) = export(&doc, format, &base);
        let o = ExportOptions { rendering_intent: ExportIntent::Perceptual, black_point_compensation: false, ..base };
        let (b, rb) = export(&doc, format, &o);
        assert_eq!(a, b, "{format:?}");
        // Matrix/TRC profiles have no perceptual tables: perceptual maps to relative (LittleCMS).
        assert_eq!(ra.rendering_intent.as_deref(), Some("relative-colorimetric"));
        assert_eq!(rb.rendering_intent.as_deref(), Some("relative-colorimetric"));
        assert!(ra.total_area_coverage_max.is_none() && ra.cmyk_profile.is_none());
    }
}
