//! Export color options (`ExportOptions::profile`, `ExportOptions::bit_depth`): target-profile
//! conversion with embedded ICC, 16-bit PNG/PSD, honest reports (target profile, bit depth,
//! rendering intent, out-of-gamut counts, what SVG cannot carry).
//!
//! Pixel expectations are computed independently with `RgbTransform` / `convert` on the reference
//! composite; PNGs are decoded with the independent `png` crate; PSDs with Veskra's structural
//! reader (`inspect_psd`), which shares no code with the writer except PackBits. ΔE checks use
//! CIEDE2000 on Lab D50, the same approach as `veskra-color/tests/oracle.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod psd_fixtures;
mod svg_fixtures;

use psd_fixtures as fx;
use svg_fixtures::{DocBuilder, rgb, taper};
use veskra_color::{IccProfile, RenderingIntent, RgbTransform, convert, lab};
use veskra_core::export::{ExportFormat, ExportOptions, ExportProfile, ExportReport, ExportWarningKind as Kind};
use veskra_core::{BlendMode, BlendSpace, ErrorCode, RectI};
use veskra_doc::{Document, Placement};
use veskra_io::png::inspect_png;
use veskra_io::psd::inspect_psd;
use veskra_io::{CancelToken, export_to_path, exporter_for};

fn export(doc: &Document, format: ExportFormat, opts: &ExportOptions) -> (Vec<u8>, ExportReport) {
    let exp = exporter_for(format);
    let mut out = Vec::new();
    let rep = exp.export(doc, opts, &mut out, &CancelToken::new()).unwrap();
    exp.validate(&out).unwrap();
    (out, rep)
}

fn opts(profile: ExportProfile, bit_depth: u8) -> ExportOptions {
    ExportOptions { profile, bit_depth, ..Default::default() }
}

fn has(rep: &ExportReport, k: Kind) -> bool {
    rep.warnings.iter().any(|w| w.kind == k)
}

/// Decode with the independent `png` crate: (bit depth, ICC bytes, samples as u16).
fn decode(bytes: &[u8]) -> (u8, Option<Vec<u8>>, Vec<u16>) {
    let mut r = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0u8; r.output_buffer_size().unwrap()];
    let f = r.next_frame(&mut buf).unwrap();
    assert_eq!(f.color_type, png::ColorType::Rgba);
    let icc = r.info().icc_profile.as_ref().map(|c| c.to_vec());
    let data = &buf[..f.buffer_size()];
    match f.bit_depth {
        png::BitDepth::Eight => (8, icc, data.iter().map(|&v| v as u16).collect()),
        png::BitDepth::Sixteen => (16, icc, data.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect()),
        other => panic!("unexpected bit depth {other:?}"),
    }
}

fn to(src: &IccProfile, dst: &IccProfile) -> RgbTransform {
    RgbTransform::new(src, dst, RenderingIntent::RelativeColorimetric).unwrap()
}

/// Reference: composite → transform (f64 on 16-bit premultiplied) → straight 16 / straight 8.
fn expected(doc: &Document, t: Option<&RgbTransform>) -> (Vec<u16>, Vec<u8>) {
    let mut c = fx::composite16(doc);
    if let Some(t) = t {
        t.apply_rgba16_premul(&mut c);
    }
    let mut s16 = vec![0u16; c.len()];
    convert::premul16_to_straight16(&c, &mut s16);
    let mut s8 = vec![0u8; c.len()];
    convert::premul16_to_straight8(&c, &mut s8);
    (s16, s8)
}

/// Opaque sRGB swatches (grays + a 9×9×9 cube), 1 px each.
fn swatches() -> (Document, Vec<[u8; 3]>) {
    let mut cols: Vec<[u8; 3]> = (0..=255u8).map(|g| [g, g, g]).collect();
    for r in 0..9u16 {
        for g in 0..9u16 {
            for b in 0..9u16 {
                cols.push([(r * 255 / 8) as u8, (g * 255 / 8) as u8, (b * 255 / 8) as u8]);
            }
        }
    }
    let w = 32usize;
    let h = cols.len().div_ceil(w);
    let mut d = fx::new_doc(w as u32, h as u32);
    let c2 = cols.clone();
    let g = fx::grid(RectI::new(0, 0, w as i32, h as i32), move |x, y| {
        c2.get(y as usize * w + x as usize).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255])
    });
    fx::add_pixels(&mut d, 1, None, g, None, fx::props("Swatches", BlendMode::Normal, 1.0));
    (d, cols)
}

fn p3_doc(base: Document) -> Document {
    let mut d = base;
    d.color = fx::color_for(&IccProfile::display_p3(), BlendSpace::GammaEncoded);
    d
}

// ------------------------------------------------------------------------------------ defaults

#[test]
fn explicit_srgb_and_8_bit_equal_the_default_bytes() {
    for (name, doc) in fx::all() {
        for format in [ExportFormat::Png, ExportFormat::Psd, ExportFormat::Svg, ExportFormat::Pdf] {
            let (default_bytes, rep) = export(&doc, format, &ExportOptions::default());
            let (srgb_bytes, rep2) = export(&doc, format, &opts(ExportProfile::Srgb, 8));
            assert_eq!(default_bytes, srgb_bytes, "{name} {format:?}: 'srgb' on an sRGB document is the default file");
            assert_eq!(rep.target_profile, "sRGB IEC61966-2.1", "{name} {format:?}");
            assert_eq!((rep.bit_depth, rep.rendering_intent.clone(), rep.out_of_gamut_pixels), (8, None, 0));
            assert_eq!(rep.warnings, rep2.warnings, "{name} {format:?}: no extra warnings for the explicit default");
            assert!(!has(&rep, Kind::ColorConverted) && !has(&rep, Kind::GamutClipped));
        }
    }
}

#[test]
fn display_p3_document_exported_as_display_p3_is_not_converted() {
    let doc = p3_doc(fx::six_layer());
    let p3 = IccProfile::display_p3();
    for format in [ExportFormat::Png, ExportFormat::Psd] {
        let (a, _) = export(&doc, format, &ExportOptions::default());
        let (b, rep) = export(&doc, format, &opts(ExportProfile::DisplayP3, 8));
        assert_eq!(a, b, "{format:?}: document already in the target space → identical file");
        assert_eq!((rep.target_profile.as_str(), rep.rendering_intent.clone()), ("Display P3", None));
        assert!(rep.profile_embedded && !has(&rep, Kind::ColorConverted));
    }
    let (png, _) = export(&doc, ExportFormat::Png, &opts(ExportProfile::DisplayP3, 8));
    assert_eq!(decode(&png).1.as_deref(), Some(p3.bytes()));
}

// ------------------------------------------------------------------------------------ PNG

#[test]
fn png_display_p3_converts_embeds_and_reports() {
    let doc = fx::six_layer();
    let (srgb, p3) = (IccProfile::srgb(), IccProfile::display_p3());
    let t = to(&srgb, &p3);
    for depth in [8u8, 16] {
        let o = opts(ExportProfile::DisplayP3, depth);
        let (bytes, rep) = export(&doc, ExportFormat::Png, &o);
        let (d, icc, px) = decode(&bytes);
        assert_eq!(d, depth);
        assert_eq!(icc.as_deref(), Some(p3.bytes()), "iCCP holds exactly the chosen profile");
        let info = inspect_png(&bytes).unwrap();
        assert_eq!((info.bit_depth, info.color_type, info.srgb_intent), (depth, 6, None));
        assert!(!info.chunks.contains(b"gAMA"), "no sRGB fallback chunks for a P3 file");
        let (e16, e8) = expected(&doc, Some(&t));
        if depth == 16 {
            assert_eq!(px, e16, "16-bit samples are the f64 conversion of the 16-bit composite");
        } else {
            assert_eq!(px, e8.iter().map(|&v| v as u16).collect::<Vec<_>>());
        }
        assert_ne!(e8, fx::composite8(&doc), "values really are converted");
        assert_eq!(rep.target_profile, "Display P3");
        assert_eq!(rep.bit_depth, depth);
        assert_eq!(rep.rendering_intent.as_deref(), Some("relative-colorimetric"));
        assert_eq!(rep.out_of_gamut_pixels, 0, "sRGB lies inside Display P3");
        assert!(rep.profile_embedded && rep.profile.contains("converted from sRGB"), "{}", rep.profile);
        assert!(has(&rep, Kind::ColorConverted) && !has(&rep, Kind::GamutClipped));
        let pf = exporter_for(ExportFormat::Png).preflight(&doc, &o);
        assert_eq!(pf.blocked_reason, None);
        assert_eq!((pf.target_profile.as_str(), pf.bit_depth), ("Display P3", depth));
        assert_eq!(pf.rendering_intent.as_deref(), Some("relative-colorimetric"));
        assert!(!pf.gamut_clipping_possible);
        assert!(pf.warnings.iter().any(|w| w.kind == Kind::ColorConverted));
    }
}

#[test]
fn png_16_bit_keeps_the_composite_exactly_and_avoids_banding() {
    // Default profile: 16-bit straight samples of the composite, document ICC embedded.
    let doc = fx::twelve_layer();
    let (bytes, rep) = export(&doc, ExportFormat::Png, &opts(ExportProfile::Document, 16));
    let (d, icc, px) = decode(&bytes);
    assert_eq!((d, rep.bit_depth), (16, 16));
    assert_eq!(icc.as_deref(), Some(IccProfile::srgb().bytes()));
    assert_eq!(px, expected(&doc, None).0);
    assert_eq!(rep.rendering_intent, None);
    assert!(inspect_png(&bytes).unwrap().chunks.contains(b"gAMA"), "sRGB fallback chunks stay");
    // A smooth 16-bit ramp keeps far more than 256 levels (8-bit would band).
    let mut ramp = fx::new_doc(1024, 2);
    let mut g = veskra_doc::TileGrid::new();
    for x in 0..1024 {
        let v = 20000 + x as u16 * 3;
        g.put_pixel(x, 0, [v, v, v, 65535]);
        g.put_pixel(x, 1, [v, v, v, 65535]);
    }
    fx::add_pixels(&mut ramp, 1, None, g, None, fx::props("Ramp", BlendMode::Normal, 1.0));
    let levels = |depth: u8| {
        let (b, _) = export(&ramp, ExportFormat::Png, &opts(ExportProfile::DisplayP3, depth));
        let px = decode(&b).2;
        let mut v: Vec<u16> = px.chunks_exact(4).take(1024).map(|p| p[0]).collect();
        v.dedup();
        v.len()
    };
    assert_eq!(levels(16), 1024, "every ramp step survives in 16 bits");
    assert!(levels(8) <= 13, "the same ramp collapses to a dozen 8-bit codes: {}", levels(8));
}

#[test]
fn conversion_accuracy_delta_e_8_vs_16_bit() {
    let (doc, cols) = swatches();
    let p3 = IccProfile::display_p3();
    let adobe = IccProfile::adobe_rgb_compatible();
    for (profile, icc) in [(ExportProfile::DisplayP3, &p3), (ExportProfile::AdobeRgb, &adobe)] {
        let mut worst = [0.0f64; 2];
        for (k, depth) in [8u8, 16].into_iter().enumerate() {
            let (bytes, rep) = export(&doc, ExportFormat::Png, &opts(profile, depth));
            let (_, embedded, px) = decode(&bytes);
            assert_eq!(embedded.as_deref(), Some(icc.bytes()));
            assert_eq!(rep.out_of_gamut_pixels, 0);
            let max = if depth == 16 { 65535.0 } else { 255.0 };
            for (i, c) in cols.iter().enumerate() {
                let p = &px[i * 4..i * 4 + 4];
                // Interpret the written numbers through the embedded profile and compare in Lab.
                let got = lab::profile_rgb_to_lab_d50(icc, [p[0] as f64 / max, p[1] as f64 / max, p[2] as f64 / max]);
                let want = lab::srgb8_to_lab_d50(*c);
                worst[k] = worst[k].max(lab::ciede2000(want, got));
            }
        }
        // Thresholds: 8-bit quantization of the destination is the only error source (< 1 ΔE00);
        // 16-bit output is two orders of magnitude tighter.
        assert!(worst[0] < 1.0, "{profile:?} 8-bit worst ΔE00 {}", worst[0]);
        assert!(worst[1] < 0.01, "{profile:?} 16-bit worst ΔE00 {}", worst[1]);
        assert!(worst[1] < worst[0] / 20.0, "{profile:?}: 16-bit must beat 8-bit clearly: {worst:?}");
        eprintln!("{profile:?}: worst ΔE00 8-bit {:.4}, 16-bit {:.6}", worst[0], worst[1]);
    }
}

#[test]
fn adobe_rgb_compatible_profile_and_known_values() {
    let adobe = IccProfile::adobe_rgb_compatible();
    assert_eq!(adobe.description(), "Veskra RGB (compatible with Adobe RGB 1998)");
    assert!(!adobe.is_srgb_equivalent() && !adobe.is_display_p3_equivalent());
    // Published Bradford-adapted D50 matrix of the Adobe RGB (1998) encoding (Lindbloom). The
    // tolerance covers the ICC PCS white (Z = 0.8249) vs. Lindbloom's D50 (Z = 0.82521).
    let m = adobe.to_xyz_d50_matrix();
    let reference =
        [[0.6097559, 0.2052401, 0.1492240], [0.3111242, 0.6256560, 0.0632197], [0.0194811, 0.0608902, 0.7448387]];
    for r in 0..3 {
        for c in 0..3 {
            assert!((m[r][c] - reference[r][c]).abs() < 4e-4, "m[{r}][{c}] = {}", m[r][c]);
        }
    }
    // Well-known 8-bit conversions sRGB → Adobe RGB (1998).
    let t = to(&IccProfile::srgb(), &adobe);
    let mut px = [255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255, 128, 128, 128, 255];
    let src = px;
    t.apply_rgba8_straight(&mut px);
    let known = [219u8, 0, 0, 255, 144, 255, 60, 255, 0, 0, 250, 255, 255, 255, 255, 255, 127, 127, 127, 255];
    for (i, (got, want)) in px.iter().zip(known).enumerate() {
        // Channels that are exactly 0 in theory (shared red/blue primaries) may come out as 1–2:
        // the pure 2.2 power curve magnifies the s15Fixed16 rounding of the ICC colorants (3e-5 in
        // linear light). Every other channel must match the published value exactly.
        let tol = if want == 0 { 2 } else { 0 };
        assert!(got.abs_diff(want) <= tol, "sample {i}: got {got}, published {want} ({px:?})");
    }
    // ... and that residue is colorimetrically irrelevant: interpreted through the profile the
    // converted primaries are the sRGB primaries to within 8-bit quantization.
    for (s, d) in src.chunks_exact(4).zip(px.chunks_exact(4)) {
        let want = lab::srgb8_to_lab_d50([s[0], s[1], s[2]]);
        let got = lab::profile_rgb_to_lab_d50(&adobe, [d[0] as f64 / 255.0, d[1] as f64 / 255.0, d[2] as f64 / 255.0]);
        assert!(lab::ciede2000(want, got) < 0.5, "{s:?} -> {d:?}: ΔE00 {}", lab::ciede2000(want, got));
    }
    assert!(t.source_gamut_fits(), "sRGB lies inside the Adobe RGB (1998) gamut");
    // Exported file: the embedded profile is exactly the generated one and parses back.
    let (bytes, rep) = export(&fx::simple(), ExportFormat::Png, &opts(ExportProfile::AdobeRgb, 8));
    let icc = decode(&bytes).1.unwrap();
    assert_eq!(icc, adobe.bytes());
    assert_eq!(IccProfile::parse(&icc).unwrap().id(), adobe.id());
    assert_eq!(rep.target_profile, adobe.description());
    assert_eq!(decode(&bytes).2, expected(&fx::simple(), Some(&t)).1.iter().map(|&v| v as u16).collect::<Vec<_>>());
}

#[test]
fn out_of_gamut_pixels_are_clipped_counted_and_warned() {
    // Display P3 document: 40 × 10 pure P3 green (outside sRGB) + 24 × 10 mid gray (inside).
    let mut doc = p3_doc(fx::new_doc(64, 10));
    let g = fx::grid(RectI::new(0, 0, 64, 10), |x, _| if x < 40 { [0, 255, 0, 255] } else { [128, 128, 128, 255] });
    fx::add_pixels(&mut doc, 1, None, g, None, fx::props("P3", BlendMode::Normal, 1.0));
    let srgb = IccProfile::srgb();
    let o = opts(ExportProfile::Srgb, 8);
    let pf = exporter_for(ExportFormat::Png).preflight(&doc, &o);
    assert!(pf.gamut_clipping_possible, "preflight says clipping can happen");
    assert!(pf.warnings.iter().any(|w| w.kind == Kind::GamutClipped));
    for format in [ExportFormat::Png, ExportFormat::Psd] {
        let (bytes, rep) = export(&doc, format, &o);
        assert_eq!(rep.out_of_gamut_pixels, 400, "{format:?}: exactly the P3-green pixels");
        let w = rep.warnings.iter().find(|w| w.kind == Kind::GamutClipped && w.count == 400);
        assert!(w.is_some(), "{format:?}: {:?}", rep.warnings);
        assert_eq!(rep.target_profile, "sRGB IEC61966-2.1");
        assert!(rep.profile_embedded);
        if format == ExportFormat::Png {
            let (_, icc, px) = decode(&bytes);
            assert_eq!(icc.as_deref(), Some(srgb.bytes()), "the sRGB profile is embedded, not the document's");
            assert_eq!(&px[..4], &[0, 255, 0, 255], "clipped to the sRGB gamut boundary");
            assert_eq!(&px[40 * 4..40 * 4 + 4], &[128, 128, 128, 255], "neutral gray is unchanged");
        } else {
            assert_eq!(inspect_psd(&bytes, false).unwrap().resource(1039), Some(srgb.bytes()));
        }
    }
    // The reverse direction cannot clip.
    let pf = exporter_for(ExportFormat::Png).preflight(&fx::simple(), &opts(ExportProfile::DisplayP3, 8));
    assert!(!pf.gamut_clipping_possible);
}

#[test]
fn wide_gamut_without_embedding_and_bad_depths_are_refused() {
    let doc = fx::simple();
    for format in [ExportFormat::Png, ExportFormat::Psd, ExportFormat::Svg, ExportFormat::Pdf] {
        for profile in [ExportProfile::DisplayP3, ExportProfile::AdobeRgb] {
            let o = ExportOptions { profile, embed_color_profile: false, ..Default::default() };
            let exp = exporter_for(format);
            let pf = exp.preflight(&doc, &o);
            assert!(pf.blocked_reason.as_deref().is_some_and(|r| r.contains("embed")), "{format:?}: {pf:?}");
            let mut out = Vec::new();
            assert!(exp.export(&doc, &o, &mut out, &CancelToken::new()).is_err(), "{format:?}");
            let dir = std::env::temp_dir().join(format!("veskra-export-color-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("untagged.{}", format.extension()));
            let e = export_to_path(&doc, format, &o, &path, &CancelToken::new()).unwrap_err();
            assert_eq!(e.code, ErrorCode::IncompatibleOutputProfile);
            assert!(!path.exists(), "no untagged wide-gamut file is ever written");
        }
        for depth in [0u8, 1, 12, 32] {
            let o = ExportOptions { bit_depth: depth, ..Default::default() };
            let exp = exporter_for(format);
            assert!(exp.preflight(&doc, &o).blocked_reason.is_some(), "{format:?} depth {depth}");
            let e = exp.export(&doc, &o, &mut Vec::new(), &CancelToken::new()).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidArgument, "{format:?} depth {depth}");
        }
    }
}

// ------------------------------------------------------------------------------------ PSD

fn be16(plane: &[u8]) -> Vec<u16> {
    plane.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect()
}

#[test]
fn psd_16_bit_structure_and_pixels() {
    for (name, doc) in fx::all() {
        let (b8, _) = export(&doc, ExportFormat::Psd, &ExportOptions::default());
        let (b16, rep) = export(&doc, ExportFormat::Psd, &opts(ExportProfile::Document, 16));
        // Header depth field (bytes 22..24) and the Lr16 layer block required for 16-bit files.
        assert_eq!(&b16[22..24], &[0, 16], "{name}");
        assert_eq!(&b8[22..24], &[0, 8]);
        let has_lr16 = |b: &[u8]| b.windows(8).any(|w| w == b"8BIMLr16");
        assert!(!has_lr16(&b8));
        let i8 = inspect_psd(&b8, true).unwrap();
        let i16 = inspect_psd(&b16, true).unwrap();
        assert_eq!((i16.depth, i8.depth, rep.bit_depth), (16, 8, 16), "{name}");
        assert_eq!(has_lr16(&b16), !i16.layers.is_empty(), "{name}");
        assert_eq!(
            i16.resource(1039),
            Some(IccProfile::srgb().bytes()),
            "{name}: ICC resource is the document profile"
        );
        assert_eq!((i16.width, i16.height, i16.channels, i16.color_mode), (i8.width, i8.height, i8.channels, 3));
        // Same layer tree, names, rects, blend keys, opacity, flags, masks as the 8-bit file.
        assert_eq!(i16.layer_count_raw, i8.layer_count_raw, "{name}");
        assert_eq!(i16.layers.len(), i8.layers.len());
        for (a, b) in i16.layers.iter().zip(&i8.layers) {
            assert_eq!(
                (a.rect, &a.unicode_name, a.blend_key, a.opacity, a.clipping, a.flags, a.mask, a.section, a.protection),
                (b.rect, &b.unicode_name, b.blend_key, b.opacity, b.clipping, b.flags, b.mask, b.section, b.protection),
                "{name}"
            );
        }
        // Merged image: composite matted over white, in 16 bits.
        let c = fx::composite16(&doc);
        for (ch, plane) in i16.merged.iter().enumerate() {
            let got = be16(plane);
            let want: Vec<u16> = c
                .chunks_exact(4)
                .map(|p| if ch == 3 { p[3] } else { (p[ch].min(p[3]) as u32 + (65535 - p[3] as u32)) as u16 })
                .collect();
            assert_eq!(got, want, "{name}: merged channel {ch}");
        }
        // Layer channels: straight 16-bit values of the layer's own pixels; masks are v · 257.
        for l in doc.iter_layers() {
            let Some(p) = l.pixels() else { continue };
            let rec = i16.layers.iter().find(|r| r.display_name() == l.props.name && r.section.is_none()).unwrap();
            let src = p.tiles.read_rect(rec.rect);
            let mut s16 = vec![0u16; src.len()];
            convert::premul16_to_straight16(&src, &mut s16);
            for (id, plane) in &rec.planes {
                let got = be16(plane);
                let want: Vec<u16> = match id {
                    -1 => s16.chunks_exact(4).map(|q| q[3]).collect(),
                    0..=2 => s16.chunks_exact(4).map(|q| q[*id as usize]).collect(),
                    -2 => {
                        let (mr, _, _) = rec.mask.unwrap();
                        p.mask.as_ref().unwrap().read_rect(mr).iter().map(|&v| v as u16 * 257).collect()
                    }
                    other => panic!("unexpected channel {other}"),
                };
                assert_eq!(got, want, "{name}: layer '{}' channel {id}", l.props.name);
            }
        }
    }
}

#[test]
fn psd_target_profile_converts_layers_and_merged_image() {
    let doc = fx::six_layer();
    let (srgb, p3) = (IccProfile::srgb(), IccProfile::display_p3());
    let t = to(&srgb, &p3);
    for depth in [8u8, 16] {
        let (bytes, rep) = export(&doc, ExportFormat::Psd, &opts(ExportProfile::DisplayP3, depth));
        let info = inspect_psd(&bytes, true).unwrap();
        assert_eq!(info.depth, depth as u16);
        assert_eq!(info.resource(1039), Some(p3.bytes()), "resource 1039 equals the chosen profile");
        assert_eq!((rep.target_profile.as_str(), rep.bit_depth, rep.out_of_gamut_pixels), ("Display P3", depth, 0));
        assert_eq!(rep.rendering_intent.as_deref(), Some("relative-colorimetric"));
        assert!(has(&rep, Kind::ColorConverted));
        // Merged image = converted composite over white.
        let mut c = fx::composite16(&doc);
        t.apply_rgba16_premul(&mut c);
        let want_r: Vec<u16> =
            c.chunks_exact(4).map(|p| (p[0].min(p[3]) as u32 + (65535 - p[3] as u32)) as u16).collect();
        let got_r: Vec<u16> =
            if depth == 16 { be16(&info.merged[0]) } else { info.merged[0].iter().map(|&v| v as u16).collect() };
        let want_r: Vec<u16> =
            if depth == 16 { want_r } else { want_r.iter().map(|&v| convert::u16_to_u8(v) as u16).collect() };
        assert_eq!(got_r, want_r, "depth {depth}");
        // First pixel layer: converted straight values.
        let l = doc.layer(fx::id(1)).unwrap();
        let rec = info.layers.iter().find(|r| r.display_name() == l.props.name).unwrap();
        let mut src = l.pixels().unwrap().tiles.read_rect(rec.rect);
        t.apply_rgba16_premul(&mut src);
        let green = &rec.planes.iter().find(|(id, _)| *id == 1).unwrap().1;
        if depth == 16 {
            let mut s = vec![0u16; src.len()];
            convert::premul16_to_straight16(&src, &mut s);
            assert_eq!(be16(green), s.chunks_exact(4).map(|p| p[1]).collect::<Vec<_>>());
        } else {
            let mut s = vec![0u8; src.len()];
            convert::premul16_to_straight8(&src, &mut s);
            assert_eq!(green, &s.chunks_exact(4).map(|p| p[1]).collect::<Vec<_>>());
        }
    }
}

// ------------------------------------------------------------------------------------ SVG / PDF

fn hybrid() -> Document {
    let mut b = DocBuilder::new(320, 200, 72.0);
    let paper = b.raster("Paper", Placement::Top);
    b.fill(paper, RectI::new(10, 10, 310, 190), rgb(245, 240, 225));
    let ink = b.vector("Ink", Placement::Top);
    b.ink(ink, &taper(30.0, 60.0, 290.0, 80.0, 16.0), rgb(200, 20, 20), 1.0);
    b.ink(ink, &taper(30.0, 130.0, 290.0, 150.0, 12.0), rgb(20, 60, 200), 1.0);
    b.finish()
}

fn base64_decode(s: &str) -> Vec<u8> {
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            _ => 63,
        }
    };
    let mut out = Vec::new();
    for q in s.as_bytes().chunks_exact(4) {
        let pad = q.iter().filter(|&&c| c == b'=').count();
        let n = q.iter().fold(0u32, |n, &c| n << 6 | if c == b'=' { 0 } else { val(c) });
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..3 - pad]);
    }
    out
}

fn svg_images(svg: &str) -> Vec<Vec<u8>> {
    svg.split("data:image/png;base64,").skip(1).map(|s| base64_decode(s.split('"').next().unwrap())).collect()
}

fn svg_fills(svg: &str) -> Vec<String> {
    svg.split("fill=\"#").skip(1).map(|s| s[..6].to_string()).collect()
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

#[test]
fn svg_wide_gamut_tags_rasters_and_keeps_vector_colors_srgb() {
    let doc = hybrid();
    let p3 = IccProfile::display_p3();
    let (default_svg, default_rep) = export(&doc, ExportFormat::Svg, &ExportOptions::default());
    let default_svg = String::from_utf8(default_svg).unwrap();
    let (svg, rep) = export(&doc, ExportFormat::Svg, &opts(ExportProfile::DisplayP3, 16));
    let svg = String::from_utf8(svg).unwrap();
    // Vector fills are byte-for-byte the default sRGB hex values.
    let fills = svg_fills(&svg);
    assert_eq!(fills, svg_fills(&default_svg));
    assert!(fills.contains(&"C81414".to_string()) && fills.contains(&"143CC8".to_string()), "{fills:?}");
    // The embedded raster carries the Display P3 profile and converted pixels.
    let (imgs, default_imgs) = (svg_images(&svg), svg_images(&default_svg));
    assert_eq!((imgs.len(), default_imgs.len()), (1, 1));
    let info = inspect_png(&imgs[0]).unwrap();
    assert_eq!(info.icc.as_ref().map(|(_, b)| b.as_slice()), Some(p3.bytes()));
    assert_eq!(inspect_png(&default_imgs[0]).unwrap().icc.unwrap().1, IccProfile::srgb().bytes());
    let (depth, _, px) = decode(&imgs[0]);
    assert_eq!(depth, 8, "SVG images are 8-bit even when 16 was requested");
    let mut paper = [245u8, 240, 225, 255];
    to(&IccProfile::srgb(), &p3).apply_rgba8_straight(&mut paper);
    assert_eq!(&px[..4], &paper.map(|v| v as u16), "top-left pixel = paper color converted to P3");
    assert_ne!(decode(&default_imgs[0]).2, px);
    // The report says exactly that.
    assert_eq!((rep.target_profile.as_str(), rep.bit_depth, rep.out_of_gamut_pixels), ("Display P3", 8, 0));
    assert_eq!(rep.rendering_intent.as_deref(), Some("relative-colorimetric"));
    assert!(rep.profile.contains("vector colors are sRGB values"), "{}", rep.profile);
    let v = rep.warnings.iter().find(|w| w.kind == Kind::VectorColorsSrgb).expect("vector color warning");
    assert_eq!(v.count, rep.vector_object_count);
    assert!(v.message.contains("stay sRGB"), "{}", v.message);
    assert!(has(&rep, Kind::BitDepthReduced) && has(&rep, Kind::ColorConverted));
    assert!(!has(&default_rep, Kind::VectorColorsSrgb) && !has(&default_rep, Kind::BitDepthReduced));
    assert_eq!(default_rep.bit_depth, 8);
    let pf = exporter_for(ExportFormat::Svg).preflight(&doc, &opts(ExportProfile::DisplayP3, 16));
    assert_eq!((pf.target_profile.as_str(), pf.bit_depth, pf.blocked_reason.clone()), ("Display P3", 8, None));
    assert!(pf.warnings.iter().any(|w| w.kind == Kind::VectorColorsSrgb));
    assert!(pf.warnings.iter().any(|w| w.kind == Kind::BitDepthReduced));
    assert_eq!(svg.matches("<path").count(), default_svg.matches("<path").count(), "no path lost");
}

#[test]
fn pdf_wide_gamut_uses_one_iccbased_space_for_images_and_vectors() {
    let doc = hybrid();
    let adobe = IccProfile::adobe_rgb_compatible();
    let (pdf, rep) = export(&doc, ExportFormat::Pdf, &opts(ExportProfile::AdobeRgb, 8));
    let (default_pdf, _) = export(&doc, ExportFormat::Pdf, &ExportOptions::default());
    let streams = pdf_streams(&pdf);
    assert!(streams.iter().any(|s| s == adobe.bytes()), "the ICCBased stream is the chosen profile");
    assert!(!streams.iter().any(|s| s == IccProfile::srgb().bytes()));
    assert!(pdf_streams(&default_pdf).iter().any(|s| s == IccProfile::srgb().bytes()));
    // Vector fill operands are the converted colors in the ICCBased space.
    let t = to(&IccProfile::srgb(), &adobe);
    let c = t.apply_f64([200.0 / 255.0, 20.0 / 255.0, 20.0 / 255.0]);
    let operand = format!("/CS0 cs {} {} {} scn", fmt4(c[0]), fmt4(c[1]), fmt4(c[2]));
    let content: Vec<String> = streams.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    assert!(content.iter().any(|s| s.contains(&operand)), "missing '{operand}'");
    assert_eq!(rep.target_profile, adobe.description());
    assert_eq!(rep.rendering_intent.as_deref(), Some("relative-colorimetric"));
    assert!(rep.profile_embedded && rep.profile.contains("ICCBased"));
    assert!(has(&rep, Kind::ColorConverted) && !has(&rep, Kind::VectorColorsSrgb));
    assert_eq!(rep.out_of_gamut_pixels, 0);
}

/// Same 4-decimal formatting as the PDF writer's color operands.
fn fmt4(v: f64) -> String {
    let v = (v as f32) as f64;
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}
