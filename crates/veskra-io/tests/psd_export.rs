//! PSD writer structure, pixel exactness, warnings and cancellation (FMT-08/09/10, PSD-003/005/006/007).
//! Structural re-reads use `veskra_io::psd::inspect_psd`; independent-decoder checks live in
//! `psd_interop.rs` (psd-tools).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod psd_fixtures;

use psd_fixtures as fx;
use std::io::Write;
use std::sync::Arc;
use veskra_color::{IccProfile, RenderingIntent, RgbTransform, convert};
use veskra_core::export::{ExportFormat, ExportOptions, ExportWarningKind};
use veskra_core::{BlendMode, BlendSpace, ErrorCode, RectI};
use veskra_doc::{Document, LayerKind};
use veskra_io::psd::{PsdExporter, PsdInfo, inspect_psd};
use veskra_io::{CancelToken, Exporter, export_to_path};

fn export(doc: &Document, opts: &ExportOptions) -> (Vec<u8>, veskra_core::export::ExportReport) {
    let mut out = Vec::new();
    let rep = PsdExporter.export(doc, opts, &mut out, &CancelToken::new()).unwrap();
    PsdExporter.validate(&out).unwrap();
    (out, rep)
}

fn parse(doc: &Document) -> PsdInfo {
    inspect_psd(&export(doc, &ExportOptions::default()).0, true).unwrap()
}

#[test]
fn six_layer_tree_flags_and_resources() {
    let doc = fx::six_layer();
    let info = parse(&doc);
    assert_eq!((info.width, info.height, info.channels, info.depth, info.color_mode), (256, 192, 4, 8, 3));
    let names: Vec<&str> = info.layers.iter().map(|l| l.display_name()).collect();
    assert_eq!(
        names,
        [
            "Paper",
            "Wash (alpha)",
            "Masked \u{2014} \u{30de}\u{30b9}\u{30af}",
            "</Layer group>",
            "Base shape",
            "Clip tone",
            "Shading",
            "Multiply",
            "Hidden"
        ]
    );
    assert_eq!(info.layer_count_raw, -9, "negative count: merged image carries transparency");
    let l = |n: &str| info.layers.iter().find(|l| l.display_name() == n).unwrap();
    assert_eq!(l("Masked \u{2014} \u{30de}\u{30b9}\u{30af}").name, "Masked ? ???", "Pascal name is ASCII");
    assert_eq!(l("Wash (alpha)").opacity, 204);
    assert_eq!(l("Shading").opacity, 230);
    assert_eq!(l("Shading").section, Some((1, Some(*b"norm"))));
    assert_eq!(l("</Layer group>").section, Some((3, Some(*b"norm"))));
    assert_eq!(l("Clip tone").clipping, 1);
    assert_eq!(&l("Clip tone").blend_key, b"mul ");
    assert_eq!(&l("Multiply").blend_key, b"mul ");
    assert_eq!(l("Base shape").clipping, 0);
    assert!(l("Hidden").hidden());
    assert!(!l("Paper").hidden());
    assert_eq!(l("Paper").rect, RectI::new(8, 8, 248, 184));
    let ch: Vec<i16> = l("Masked \u{2014} \u{30de}\u{30b9}\u{30af}").channels.iter().map(|c| c.0).collect();
    assert_eq!(ch, [-1, 0, 1, 2, -2]);
    let masked = doc.layer(fx::id(3)).unwrap().pixels().unwrap().mask.as_ref().unwrap().bounds();
    assert_eq!(l("Masked \u{2014} \u{30de}\u{30b9}\u{30af}").mask, Some((masked, 0, 0)));
    // PSD-003: ICC resource 1039 = exact document profile bytes; 1005 = 144 dpi.
    assert_eq!(info.resource(1039).unwrap(), IccProfile::srgb().bytes());
    let res = info.resource(1005).unwrap();
    assert_eq!(&res[..4], &(144u32 << 16).to_be_bytes());
    assert_eq!(&res[4..8], &[0, 1, 0, 1]);
}

#[test]
fn layer_channels_are_exact_straight8_of_layer_pixels() {
    for (name, doc) in fx::all() {
        let info = parse(&doc);
        let pixel_layers: Vec<_> =
            doc.paint_order().into_iter().filter_map(|id| doc.layer(id)).filter(|l| l.pixels().is_some()).collect();
        let recs: Vec<_> = info.layers.iter().filter(|r| r.section.is_none()).collect();
        assert_eq!(recs.len(), pixel_layers.len(), "{name}");
        for (rec, layer) in recs.iter().zip(pixel_layers) {
            let p = layer.pixels().unwrap();
            assert_eq!(rec.rect, p.tiles.content_bounds(), "{name}/{}", layer.props.name);
            let src = p.tiles.read_rect(rec.rect);
            let mut s8 = vec![0u8; src.len()];
            convert::premul16_to_straight8(&src, &mut s8);
            for (id, plane) in &rec.planes {
                if *id == -2 {
                    let m = p.mask.as_ref().unwrap();
                    assert_eq!(plane, &m.read_rect(rec.mask.unwrap().0), "{name}: mask plane");
                    continue;
                }
                let c = if *id == -1 { 3 } else { *id as usize };
                let expected: Vec<u8> = s8.chunks_exact(4).map(|px| px[c]).collect();
                assert_eq!(plane, &expected, "{name}/{} channel {id}", layer.props.name);
            }
        }
    }
}

#[test]
fn merged_image_is_veskra_composite_matted_over_white() {
    for (name, doc) in fx::all() {
        let info = parse(&doc);
        let c16 = fx::composite16(&doc);
        let c8 = fx::composite8(&doc);
        assert_eq!(info.merged.len(), 4, "{name}");
        let mut opaque_checked = 0;
        for (i, (p16, p8)) in c16.chunks_exact(4).zip(c8.chunks_exact(4)).enumerate() {
            let a = convert::u16_to_u8(p16[3]);
            assert_eq!(info.merged[3][i], a, "{name}: alpha at {i}");
            for c in 0..3 {
                let m = convert::u16_to_u8(p16[c] + (65535 - p16[3]));
                assert_eq!(info.merged[c][i], m, "{name}: matted channel {c} at {i}");
                if a == 255 {
                    assert_eq!(info.merged[c][i], p8[c]);
                }
            }
            opaque_checked += (a == 255) as usize;
        }
        assert!(opaque_checked > 0 || name == "large_sparse", "{name}");
    }
}

#[test]
fn empty_layer_empty_group_and_inverse_mask() {
    let doc = fx::empty_and_inverse_mask();
    let info = parse(&doc);
    let names: Vec<&str> = info.layers.iter().map(|l| l.display_name()).collect();
    assert_eq!(names, ["Empty", "Dot", "Inverse mask", "</Layer group>", "Empty group"]);
    let empty = &info.layers[0];
    assert_eq!(empty.rect, RectI::EMPTY);
    assert!(empty.channels.iter().all(|c| c.1 == 0));
    let inv = &info.layers[2];
    let (mr, def, _) = inv.mask.unwrap();
    assert_eq!(def, 255);
    assert!(!mr.is_empty() && mr.x0 >= 18 && mr.x1 <= 42, "mask rect covers only the hole: {mr:?}");
}

#[test]
fn twelve_layer_groups_pass_through_and_vector_rasterized() {
    let doc = fx::twelve_layer();
    let (bytes, rep) = export(&doc, &ExportOptions::default());
    let info = inspect_psd(&bytes, false).unwrap();
    let l = |n: &str| info.layers.iter().find(|l| l.display_name() == n).unwrap();
    assert_eq!(&l("Flats").blend_key, b"pass");
    assert_eq!(l("Flats").section, Some((1, Some(*b"pass"))));
    assert_eq!(&l("Lighting").blend_key, b"scrn");
    assert_eq!(l("Lighting").opacity, 217);
    assert_eq!(&l("Overlay tint").blend_key, b"over");
    assert_eq!(&l("Skin rim").blend_key, b"scrn");
    assert_eq!((l("Skin shadow").clipping, l("Skin rim").clipping, l("Hair").clipping), (1, 1, 0));
    assert!(l("Notes").hidden());
    assert_eq!(info.layers.iter().filter(|l| l.section.is_none()).count(), 12);
    assert_eq!(rep.rasterized_object_count, 1);
    assert!(rep.warnings.iter().any(|w| w.kind == ExportWarningKind::LayerRasterized && w.count == 1));
    assert!(rep.warnings.iter().any(|w| w.kind == ExportWarningKind::HiddenLayer && w.count == 1));
    assert!(rep.profile_embedded);
    assert_eq!(rep.file_bytes, bytes.len() as u64);
    assert!(!rep.success, "success is only set by export_to_path after validation");
}

#[test]
fn preflight_counts_and_blocking() {
    let doc = fx::twelve_layer();
    let pf = PsdExporter.preflight(&doc, &ExportOptions::default());
    assert_eq!(pf.blocked_reason, None);
    assert_eq!(pf.rasterized_layer_count, 1);
    assert_eq!(pf.vector_path_count, 0);
    assert_eq!((pf.width_px, pf.height_px), (512, 384));
    assert!(pf.profile.contains("sRGB") && pf.profile.contains("embedded"), "{}", pf.profile);
    assert!(pf.estimated_bytes > 512 * 384 * 4);
    let rep = export(&doc, &ExportOptions::default()).1;
    let kinds = |w: &[veskra_core::export::ExportWarning]| {
        let mut k: Vec<String> = w.iter().map(|w| format!("{:?}/{}", w.kind, w.count)).collect();
        k.sort();
        k
    };
    assert_eq!(kinds(&pf.warnings), kinds(&rep.warnings), "preflight predicts the report");

    let scaled = ExportOptions { raster_scale: 2.0, ..Default::default() };
    assert!(PsdExporter.preflight(&doc, &scaled).blocked_reason.is_some());
    let e = PsdExporter.export(&doc, &scaled, &mut Vec::new(), &CancelToken::new()).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
}

#[test]
fn psd006_unrepresentable_semantics_are_warned_not_dropped() {
    let mut doc = fx::new_doc(32, 32);
    let mut c = fx::props("Orphan clip", BlendMode::Normal, 1.0);
    c.clip_to_below = true;
    fx::add_pixels(&mut doc, 1, None, fx::grid(RectI::new(0, 0, 8, 8), |_, _| [1, 2, 3, 255]), None, c);
    let mut g = fx::props("Clipped group", BlendMode::Normal, 1.0);
    g.clip_to_below = true;
    fx::add_group(&mut doc, 2, None, false, g);
    fx::add_pixels(
        &mut doc,
        3,
        Some(2),
        fx::grid(RectI::new(4, 4, 12, 12), |_, _| [9, 9, 9, 255]),
        None,
        fx::props("In group", BlendMode::Normal, 1.0),
    );
    // Pass-through group used as a clipping base (Veskra composites it isolated) → warned.
    fx::add_group(&mut doc, 4, None, true, fx::props("Pass base", BlendMode::Normal, 1.0));
    fx::add_pixels(
        &mut doc,
        5,
        Some(4),
        fx::grid(RectI::new(0, 0, 6, 6), |_, _| [7, 7, 7, 255]),
        None,
        fx::props("Pass child", BlendMode::Normal, 1.0),
    );
    let mut c2 = fx::props("Clips pass group", BlendMode::Multiply, 1.0);
    c2.clip_to_below = true;
    fx::add_pixels(&mut doc, 6, None, fx::grid(RectI::new(0, 0, 8, 8), |_, _| [200, 9, 9, 255]), None, c2);
    doc.color.blend_space = BlendSpace::Linear;
    let (bytes, rep) = export(&doc, &ExportOptions::default());
    let info = inspect_psd(&bytes, false).unwrap();
    // Written exactly as Veskra renders: orphan clipper and clipped group unclipped; the real clipper kept.
    let clipped: Vec<&str> = info.layers.iter().filter(|l| l.clipping == 1).map(|l| l.display_name()).collect();
    assert_eq!(clipped, ["Clips pass group"]);
    let n = |k: ExportWarningKind| rep.warnings.iter().filter(|w| w.kind == k).count();
    assert_eq!(n(ExportWarningKind::ClippingApproximated), 3);
    assert_eq!(n(ExportWarningKind::BlendSpaceMismatch), 1);
    assert!(
        rep.warnings.iter().filter(|w| w.kind == ExportWarningKind::ClippingApproximated).all(|w| w.layer.is_some())
    );
}

#[test]
fn locks_written_as_protection_flags() {
    let mut doc = fx::new_doc(16, 16);
    let mut p = fx::props("Locked", BlendMode::Normal, 1.0);
    p.locked = true;
    p.alpha_locked = true;
    fx::add_pixels(&mut doc, 1, None, fx::grid(RectI::new(0, 0, 4, 4), |_, _| [5, 5, 5, 255]), None, p);
    let info = parse(&doc);
    assert_eq!(info.layers[0].protection, Some(0x8000_0001));
    assert_eq!(info.layers[0].flags & 1, 1);
}

#[test]
fn profile_not_embedded_and_p3_documents() {
    let doc = fx::simple();
    let off = ExportOptions { embed_color_profile: false, ..Default::default() };
    let (bytes, rep) = export(&doc, &off);
    let info = inspect_psd(&bytes, true).unwrap();
    assert!(info.resource(1039).is_none());
    assert!(!rep.profile_embedded);
    assert!(rep.warnings.iter().any(|w| w.kind == ExportWarningKind::ProfileNotEmbedded));

    // Display P3 document: embedded → values unchanged + P3 bytes; not embedded → converted to sRGB.
    let mut p3 = fx::simple();
    let prof = IccProfile::display_p3();
    p3.color = fx::color_for(&prof, BlendSpace::GammaEncoded);
    let (b_emb, _) = export(&p3, &ExportOptions::default());
    let emb = inspect_psd(&b_emb, true).unwrap();
    assert_eq!(emb.resource(1039).unwrap(), prof.bytes());
    assert_eq!(emb.layers[0].planes, info.layers[0].planes);
    let (b_conv, rep) = export(&p3, &off);
    let conv = inspect_psd(&b_conv, true).unwrap();
    assert!(conv.resource(1039).is_none());
    assert!(rep.profile.contains("converted from"), "{}", rep.profile);
    let t = RgbTransform::new(&prof, &IccProfile::srgb(), RenderingIntent::RelativeColorimetric).unwrap();
    let layer = p3.layer(fx::id(1)).unwrap().pixels().unwrap();
    let mut src = layer.tiles.read_rect(conv.layers[0].rect);
    t.apply_rgba16_premul(&mut src);
    let mut s8 = vec![0u8; src.len()];
    convert::premul16_to_straight8(&src, &mut s8);
    let red: Vec<u8> = s8.chunks_exact(4).map(|p| p[0]).collect();
    assert_eq!(conv.layers[0].planes[1].1, red);
    assert_ne!(conv.layers[0].planes[1].1, emb.layers[0].planes[1].1, "P3 values were actually converted");
}

#[test]
fn deterministic_bytes() {
    let doc = fx::six_layer();
    assert_eq!(export(&doc, &ExportOptions::default()).0, export(&doc, &ExportOptions::default()).0);
}

#[test]
fn validator_rejects_corruption() {
    let (bytes, _) = export(&fx::six_layer(), &ExportOptions::default());
    for cut in [0, 10, 26, 40, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
        assert!(PsdExporter.validate(&bytes[..cut]).is_err(), "truncated at {cut}");
    }
    let mut b = bytes.clone();
    b[0] = b'X';
    assert!(PsdExporter.validate(&b).is_err());
    let mut b = bytes.clone();
    b.push(0);
    assert!(PsdExporter.validate(&b).is_err(), "trailing byte");
    // Corrupt the first merged RLE row count (header 26 B, color mode length, resources, layer section).
    let mut b = bytes.clone();
    let lm_off = 26 + 4 + 4 + u32::from_be_bytes(bytes[30..34].try_into().unwrap()) as usize;
    let lm_len = u32::from_be_bytes(bytes[lm_off..lm_off + 4].try_into().unwrap()) as usize;
    let counts = lm_off + 4 + lm_len + 2;
    b[counts] ^= 0x40;
    assert!(PsdExporter.validate(&b).is_err(), "corrupted RLE count");
}

/// A sink that cancels the token after the first write (deterministic mid-export cancellation).
struct CancelAfterFirstWrite {
    token: CancelToken,
    written: usize,
}

impl Write for CancelAfterFirstWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written += buf.len();
        self.token.cancel();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn psd007_cancellation_mid_export_and_no_file() {
    let doc = fx::twelve_layer();
    let token = CancelToken::new();
    let mut sink = CancelAfterFirstWrite { token: token.clone(), written: 0 };
    let e = PsdExporter.export(&doc, &ExportOptions::default(), &mut sink, &token).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert!(sink.written > 0, "cancellation happened after output started");

    let dir = fx::interop_dir("psd007");
    let path = dir.join("cancelled.psd");
    let _ = std::fs::remove_file(&path);
    let pre = CancelToken::new();
    pre.cancel();
    let e = export_to_path(&doc, ExportFormat::Psd, &ExportOptions::default(), &path, &pre).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert!(!path.exists());

    // Cancel from another thread while a large export runs; either outcome must be consistent.
    let big = fx::large_sparse();
    let t = CancelToken::new();
    let t2 = t.clone();
    let h = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(5));
        t2.cancel();
    });
    let res = export_to_path(&big, ExportFormat::Psd, &ExportOptions::default(), &path, &t);
    h.join().unwrap();
    match res {
        Err(e) => {
            assert_eq!(e.code, ErrorCode::Cancelled);
            assert!(!path.exists(), "no final file after cancellation");
        }
        Ok(rep) => assert!(rep.success && PsdExporter.validate(&std::fs::read(&path).unwrap()).is_ok()),
    }
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left behind");

    // Normal export_to_path: success only after validation.
    let ok = export_to_path(&doc, ExportFormat::Psd, &ExportOptions::default(), &path, &CancelToken::new()).unwrap();
    assert!(ok.success);
    assert_eq!(ok.validation, veskra_core::export::ValidationStatus::StructurallyValidated);
    assert_eq!(ok.file_bytes, std::fs::metadata(&path).unwrap().len());
}

#[test]
fn vector_layer_written_as_raster_pixels() {
    let doc = fx::twelve_layer();
    assert!(matches!(doc.layer(fx::id(10)).unwrap().kind, LayerKind::Vector(_)));
    let info = parse(&doc);
    let ink = info.layers.iter().find(|l| l.display_name() == "Ink").unwrap();
    assert!(!ink.rect.is_empty());
    assert!(ink.planes.iter().any(|(id, p)| *id == -1 && p.contains(&255)));
}

#[test]
fn shared_profile_arc_is_not_required() {
    // A document carrying sRGB bytes in a fresh allocation is still recognized as sRGB.
    let mut doc = fx::simple();
    doc.color.icc_bytes = Arc::from(IccProfile::srgb().bytes().to_vec());
    let pf = PsdExporter.preflight(&doc, &ExportOptions::default());
    assert!(pf.profile.starts_with("sRGB IEC61966-2.1"), "{}", pf.profile);
}

/// Committed reference PSDs (tests/fixtures/exports/psd) must equal the current writer output and
/// validate. Regenerate deliberately with `VESKRA_BLESS=1` (reviewed change; see the README there).
#[test]
fn committed_reference_psds_are_current() {
    let dir = fx::repo_root().join("tests/fixtures/exports/psd");
    let bless = std::env::var("VESKRA_BLESS").as_deref() == Ok("1");
    for (name, doc) in
        fx::all().into_iter().filter(|(n, _)| ["simple", "six_layer", "empty_and_inverse_mask"].contains(n))
    {
        let (bytes, _) = export(&doc, &ExportOptions::default());
        let path = dir.join(format!("{name}.psd"));
        if bless {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            continue;
        }
        let committed = std::fs::read(&path).unwrap_or_else(|_| panic!("missing {path:?}; run with VESKRA_BLESS=1"));
        PsdExporter.validate(&committed).unwrap();
        assert!(committed == bytes, "{name}.psd differs from current writer output (re-bless if intended)");
    }
}
