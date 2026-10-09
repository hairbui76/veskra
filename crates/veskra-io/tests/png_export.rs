//! PNG export (FMT-07, CLR-05, COL-001, COL-002). Decoding uses the independent `png` crate
//! (directly and via `veskra-testkit`); `scripts/interop/png_check.py` (Pillow) is run when available.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod psd_fixtures;

use psd_fixtures as fx;
use veskra_color::{IccProfile, RenderingIntent, RgbTransform, convert};
use veskra_core::export::{ExportFormat, ExportOptions, ExportReport, ExportWarningKind};
use veskra_core::{BlendMode, BlendSpace, ErrorCode, RectI};
use veskra_doc::Document;
use veskra_io::png::{PngExporter, encode_rgba8_srgb, inspect_png};
use veskra_io::{CancelToken, Exporter, export_to_path};
use veskra_testkit::{ImageDiff, decode_png_rgba8};

fn export(doc: &Document, opts: &ExportOptions) -> (Vec<u8>, ExportReport) {
    let mut out = Vec::new();
    let rep = PngExporter.export(doc, opts, &mut out, &CancelToken::new()).unwrap();
    PngExporter.validate(&out).unwrap();
    (out, rep)
}

type PngMeta = (Option<Vec<u8>>, Option<u8>, bool, bool, Option<(u32, u32)>);

/// (icc_profile, srgb intent, gAMA chunk present, cHRM chunk present, pixel dims). ICC, sRGB and
/// pHYs come from the independent `png` crate; gAMA/cHRM presence from the chunk list (the `png`
/// crate deliberately drops them when iCCP/sRGB override them).
fn png_meta(bytes: &[u8]) -> PngMeta {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let r = dec.read_info().unwrap();
    let i = r.info();
    let chunks = inspect_png(bytes).unwrap().chunks;
    (
        i.icc_profile.as_ref().map(|c| c.to_vec()),
        i.srgb.map(|s| s as u8),
        chunks.contains(b"gAMA"),
        chunks.contains(b"cHRM"),
        i.pixel_dims.map(|d| (d.xppu, d.yppu)),
    )
}

/// Opaque swatches: 256 grays + a 6×6×6 color cube, 2×2 px each.
fn swatches() -> (Document, Vec<[u8; 4]>) {
    let mut cols: Vec<[u8; 4]> = (0..=255u8).map(|g| [g, g, g, 255]).collect();
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                cols.push([r * 51, g * 51, b * 51, 255]);
            }
        }
    }
    let per_row = 32;
    let rows = cols.len().div_ceil(per_row);
    let mut d = fx::new_doc(per_row as u32 * 2, rows as u32 * 2);
    let c2 = cols.clone();
    let g = fx::grid(RectI::new(0, 0, per_row as i32 * 2, rows as i32 * 2), move |x, y| {
        c2.get((y / 2) as usize * per_row + (x / 2) as usize).copied().unwrap_or([0, 0, 0, 0])
    });
    fx::add_pixels(&mut d, 1, None, g, None, fx::props("Swatches", BlendMode::Normal, 1.0));
    (d, cols)
}

#[test]
fn col002_opaque_swatches_exact() {
    let (doc, cols) = swatches();
    for embed in [true, false] {
        let (bytes, _) = export(&doc, &ExportOptions { embed_color_profile: embed, ..Default::default() });
        let (w, h, rgba) = decode_png_rgba8(&bytes).unwrap();
        assert_eq!((w, h), (doc.width(), doc.height()));
        for (i, c) in cols.iter().enumerate() {
            let (x, y) = ((i % 32) * 2, (i / 32) * 2);
            let o = (y * w as usize + x) * 4;
            assert_eq!(&rgba[o..o + 4], c, "swatch {i} (embed={embed}) must round-trip with 0 LSB error");
        }
    }
}

#[test]
fn composite_matches_decoded_png_exactly_incl_alpha_edges() {
    for (name, doc) in fx::all() {
        if name == "large_sparse" {
            continue; // covered by the interop run; keeps this test fast
        }
        let (bytes, rep) = export(&doc, &ExportOptions::default());
        let (w, h, rgba) = decode_png_rgba8(&bytes).unwrap();
        assert_eq!((w, h), (doc.width(), doc.height()));
        let expected = fx::composite8(&doc);
        let d = ImageDiff::rgba8(&rgba, &expected, 0);
        assert_eq!(d.max_abs, 0, "{name}: {d:?}");
        // No halos: fully transparent pixels are (0,0,0,0).
        assert!(rgba.chunks_exact(4).filter(|p| p[3] == 0).all(|p| p == [0, 0, 0, 0]), "{name}");
        if matches!(name, "simple" | "empty_and_inverse_mask") {
            assert!(rgba.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255), "{name} has soft alpha edges");
        }
        assert_eq!(rep.file_bytes, bytes.len() as u64);
        assert!(!rep.success);
    }
}

#[test]
fn col001_iccp_carries_exact_document_profile() {
    let doc = fx::six_layer();
    let (bytes, rep) = export(&doc, &ExportOptions::default());
    let (icc, srgb, gama, chrm, dims) = png_meta(&bytes);
    assert_eq!(icc.as_deref(), Some(IccProfile::srgb().bytes()));
    assert_eq!(srgb, None, "sRGB chunk must not accompany iCCP");
    assert!(gama && chrm, "sRGB-equivalent profile gets gAMA/cHRM fallback");
    assert_eq!(dims, Some((5669, 5669)), "144 dpi");
    assert!(rep.profile_embedded);
    let info = inspect_png(&bytes).unwrap();
    assert_eq!(info.icc.as_ref().unwrap().0, IccProfile::srgb().description().as_bytes().to_vec());
    let order: Vec<String> = info.chunks.iter().map(|c| String::from_utf8_lossy(c).into_owned()).collect();
    assert_eq!(order.first().map(String::as_str), Some("IHDR"));
    assert_eq!(order.last().map(String::as_str), Some("IEND"));
}

#[test]
fn srgb_chunk_when_not_embedding() {
    let doc = fx::simple();
    let (bytes, rep) = export(&doc, &ExportOptions { embed_color_profile: false, ..Default::default() });
    let (icc, srgb, gama, chrm, _) = png_meta(&bytes);
    assert_eq!(icc, None);
    assert_eq!(srgb, Some(1), "relative colorimetric");
    assert!(gama && chrm);
    assert!(!rep.profile_embedded);
    assert!(rep.profile.contains("sRGB chunk"), "{}", rep.profile);
    let (_, _, rgba) = decode_png_rgba8(&bytes).unwrap();
    assert_eq!(rgba, fx::composite8(&doc));
}

#[test]
fn p3_document_embedded_or_converted() {
    let mut doc = fx::simple();
    let p3 = IccProfile::display_p3();
    doc.color = fx::color_for(&p3, BlendSpace::GammaEncoded);
    let (bytes, rep) = export(&doc, &ExportOptions::default());
    assert_eq!(png_meta(&bytes).0.as_deref(), Some(p3.bytes()));
    assert!(!png_meta(&bytes).2, "no sRGB gAMA fallback for a P3 profile");
    assert!(rep.warnings.iter().all(|w| w.kind != ExportWarningKind::ProfileNotEmbedded));
    assert_eq!(decode_png_rgba8(&bytes).unwrap().2, fx::composite8(&doc), "embedded: values unconverted");

    let (bytes, rep) = export(&doc, &ExportOptions { embed_color_profile: false, ..Default::default() });
    assert_eq!(png_meta(&bytes).1, Some(1));
    assert!(rep.warnings.iter().any(|w| w.kind == ExportWarningKind::ProfileNotEmbedded));
    let t = RgbTransform::new(&p3, &IccProfile::srgb(), RenderingIntent::RelativeColorimetric).unwrap();
    let mut c = fx::composite16(&doc);
    t.apply_rgba16_premul(&mut c);
    let mut exp = vec![0u8; c.len()];
    convert::premul16_to_straight8(&c, &mut exp);
    let got = decode_png_rgba8(&bytes).unwrap().2;
    assert_eq!(got, exp);
    assert_ne!(got, fx::composite8(&doc));
}

#[test]
fn raster_scale_resamples_in_premultiplied_space() {
    // 2×2 checker of opaque red / transparent → 0.5× must be half-alpha pure red (no dark fringe).
    let mut doc = fx::new_doc(64, 32);
    let g = fx::grid(RectI::new(0, 0, 64, 32), |x, y| if (x + y) % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 0, 0] });
    fx::add_pixels(&mut doc, 1, None, g, None, fx::props("Checker", BlendMode::Normal, 1.0));
    let (bytes, rep) = export(&doc, &ExportOptions { raster_scale: 0.5, ..Default::default() });
    let (w, h, rgba) = decode_png_rgba8(&bytes).unwrap();
    assert_eq!((w, h, rep.width_px, rep.height_px), (32, 16, 32, 16));
    assert!(rgba.chunks_exact(4).all(|p| p == [255, 0, 0, 128]), "{:?}", &rgba[..8]);
    assert_eq!(png_meta(&bytes).4, Some((2835, 2835)), "dpi halves with the pixel count");

    let (bytes, _) = export(&fx::simple(), &ExportOptions { raster_scale: 2.0, ..Default::default() });
    let (w, h, up) = decode_png_rgba8(&bytes).unwrap();
    assert_eq!((w, h), (128, 96));
    let src = fx::composite8(&fx::simple());
    // Interior of the opaque region: bilinear between neighbours stays within their range.
    let (sx, sy) = (10usize, 10usize);
    let o = (sy * 64 + sx) * 4;
    let u = ((2 * sy) * 128 + 2 * sx) * 4;
    for c in 0..4 {
        assert!(up[u + c].abs_diff(src[o + c]) <= 4, "channel {c}");
    }
    let pf = PngExporter.preflight(&fx::simple(), &ExportOptions { raster_scale: 0.25, ..Default::default() });
    assert_eq!((pf.width_px, pf.height_px), (16, 12));
}

#[test]
fn preflight_and_report_warnings() {
    let doc = fx::twelve_layer();
    let pf = PngExporter.preflight(&doc, &ExportOptions::default());
    assert_eq!(pf.blocked_reason, None);
    assert_eq!(pf.rasterized_layer_count, 1);
    let has = |k: ExportWarningKind| pf.warnings.iter().any(|w| w.kind == k);
    assert!(has(ExportWarningKind::HiddenLayer) && has(ExportWarningKind::LayerRasterized));
    assert!(has(ExportWarningKind::FeatureFlattened));
    let (_, rep) = export(&doc, &ExportOptions::default());
    assert_eq!(rep.warnings.len(), pf.warnings.len());
    assert_eq!(rep.rasterized_object_count, 1);
    let bad = ExportOptions { raster_scale: 5.0, ..Default::default() };
    assert!(PngExporter.preflight(&doc, &bad).blocked_reason.is_some());
}

#[test]
fn validator_rejects_corruption() {
    let (bytes, _) = export(&fx::simple(), &ExportOptions::default());
    for cut in [0, 8, 20, bytes.len() / 2, bytes.len() - 1] {
        assert!(PngExporter.validate(&bytes[..cut]).is_err(), "cut {cut}");
    }
    let mut b = bytes.clone();
    let n = b.len();
    b[n - 20] ^= 0xFF; // inside the last IDAT data → CRC mismatch
    assert!(PngExporter.validate(&b).is_err());
    assert_eq!(PngExporter.validate(&b).unwrap_err().code, ErrorCode::PngEncodeError);
}

#[test]
fn export_to_path_success_and_cancel() {
    let dir = fx::interop_dir("png");
    let path = dir.join("path_test.png");
    let rep = export_to_path(&fx::simple(), ExportFormat::Png, &ExportOptions::default(), &path, &CancelToken::new())
        .unwrap();
    assert!(rep.success);
    assert_eq!(rep.file_bytes, std::fs::metadata(&path).unwrap().len());
    std::fs::remove_file(&path).unwrap();
    let t = CancelToken::new();
    t.cancel();
    let e = export_to_path(&fx::simple(), ExportFormat::Png, &ExportOptions::default(), &path, &t).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert!(!path.exists());
}

#[test]
fn encode_rgba8_srgb_roundtrip() {
    let px: Vec<u8> = (0..7 * 5 * 4).map(|i| (i * 37 % 256) as u8).collect();
    let bytes = encode_rgba8_srgb(7, 5, &px).unwrap();
    assert_eq!(decode_png_rgba8(&bytes).unwrap(), (7, 5, px.clone()));
    assert_eq!(png_meta(&bytes).1, Some(1));
    assert!(inspect_png(&bytes).is_ok());
    assert!(encode_rgba8_srgb(7, 5, &px[..10]).is_err());
    assert!(encode_rgba8_srgb(0, 5, &[]).is_err());
}

/// CLR-05 interop: write PNGs + expected raw RGBA + JSON sidecars and run the Pillow oracle.
#[test]
fn png_pillow_oracle() {
    let dir = fx::interop_dir("png");
    let mut cases: Vec<(String, Document, ExportOptions)> = Vec::new();
    for (name, doc) in fx::all() {
        cases.push((name.to_string(), doc, ExportOptions::default()));
    }
    cases.push(("swatches".into(), swatches().0, ExportOptions::default()));
    cases.push((
        "simple_srgb_chunk".into(),
        fx::simple(),
        ExportOptions { embed_color_profile: false, ..Default::default() },
    ));
    let mut p3 = fx::six_layer();
    p3.color = fx::color_for(&IccProfile::display_p3(), BlendSpace::GammaEncoded);
    cases.push(("six_layer_p3_embedded".into(), p3, ExportOptions::default()));
    let mut manifest = Vec::new();
    for (name, doc, opts) in &cases {
        let (bytes, _) = export(doc, opts);
        let png_path = dir.join(format!("{name}.png"));
        let raw_path = dir.join(format!("{name}.expected.rgba"));
        std::fs::write(&png_path, &bytes).unwrap();
        std::fs::write(&raw_path, fx::composite8(doc)).unwrap();
        let icc = if opts.embed_color_profile { Some(fx::hex(&doc.color.icc_bytes)) } else { None };
        let j = serde_json::json!({
            "name": name, "png": png_path, "expected_rgba": raw_path,
            "width": doc.width(), "height": doc.height(),
            "icc_hex": icc, "srgb_chunk": !opts.embed_color_profile,
        });
        let jp = dir.join(format!("{name}.expected.json"));
        std::fs::write(&jp, serde_json::to_vec_pretty(&j).unwrap()).unwrap();
        manifest.push(jp);
    }
    if !fx::python_has("import PIL, numpy") {
        eprintln!("png_pillow_oracle: Pillow/numpy unavailable — oracle NOT RUN (sidecars written to {dir:?})");
        return;
    }
    let script = fx::repo_root().join("scripts/interop/png_check.py");
    let out = std::process::Command::new("python3").arg(&script).args(&manifest).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    std::fs::write(dir.join("png_check.results.json"), stdout.as_bytes()).unwrap();
    eprintln!("{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "Pillow oracle reported failures:\n{stdout}");
}
