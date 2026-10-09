//! Color-managed PNG import (MASTER_PLAN §1.1/§6.2/§8.4; COL-004, COL-005, SEC-001).
//!
//! Committed fixtures and LittleCMS expectations come from `tests/fixtures/import/gen_fixtures.py`
//! (Pillow 12.3.0 / LittleCMS 2.19); hostile and 16-bit inputs are generated here with the
//! independent `png` crate encoder or a tiny chunk writer. Measured differences are printed with
//! `--nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod psd_fixtures;

use psd_fixtures as fx;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use veskra_color::transform::GrayToRgbTransform;
use veskra_color::{IccProfile, RenderingIntent, RgbTransform, convert};
use veskra_core::export::ExportOptions;
use veskra_core::{BlendMode, ErrorCode, RectI, VeskraError};
use veskra_io::import::{
    ImportOptions, ImportedImage, InvalidProfilePolicy, MissingProfilePolicy, ProfileHandling, import_png,
    import_png_cancellable,
};
use veskra_io::png::PngExporter;
use veskra_io::{CancelToken, Exporter};
use veskra_testkit::decode_png_rgba8;

// ---------------------------------------------------------------------------------------------
// Helpers

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/import")
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(name)).unwrap()
}

fn expected() -> serde_json::Value {
    serde_json::from_slice(&fixture("expected.json")).unwrap()
}

fn ints(v: &serde_json::Value) -> Vec<u8> {
    v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect()
}

fn convert_opts() -> ImportOptions {
    ImportOptions::default()
}

fn assign_opts() -> ImportOptions {
    ImportOptions { handling: ProfileHandling::AssignDocumentProfile, ..Default::default() }
}

fn srgb() -> Arc<IccProfile> {
    IccProfile::srgb()
}

fn premul16(img: &ImportedImage) -> Vec<u16> {
    img.tiles.try_read_rect(RectI::from_size(img.width, img.height)).unwrap()
}

fn straight8(img: &ImportedImage) -> Vec<u8> {
    let p = premul16(img);
    let mut out = vec![0u8; p.len()];
    convert::premul16_to_straight8(&p, &mut out);
    out
}

/// Alpha-0 pixels carry no color (premultiplied storage): compare them as (0,0,0,0).
fn normalize_alpha0(px: &[u8]) -> Vec<u8> {
    px.chunks_exact(4).flat_map(|p| if p[3] == 0 { [0; 4] } else { [p[0], p[1], p[2], p[3]] }).collect()
}

fn err(r: Result<ImportedImage, VeskraError>) -> VeskraError {
    match r {
        Ok(i) => panic!("expected an error, imported {}x{} ({:?})", i.width, i.height, i.warnings),
        Err(e) => e,
    }
}

fn assert_premul_invariant(img: &ImportedImage) {
    for p in premul16(img).chunks_exact(4) {
        assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
    }
}

/// (max abs channel diff, count of differing channels) over the RGB channels.
fn rgb_diff(a_rgba: &[u8], b_rgb: &[u8]) -> (u8, usize) {
    let mut worst = 0u8;
    let mut n = 0;
    for (a, b) in a_rgba.chunks_exact(4).zip(b_rgb.chunks_exact(3)) {
        for c in 0..3 {
            let d = a[c].abs_diff(b[c]);
            worst = worst.max(d);
            n += (d != 0) as usize;
        }
    }
    (worst, n)
}

// --- tiny PNG writer for hostile / special inputs (independent of the code under test) ---

fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let mut h = crc32fast::Hasher::new();
    h.update(ty);
    h.update(data);
    out.extend_from_slice(&h.finalize().to_be_bytes());
}

const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// Filter-0 scanlines for `pixels` (row-major, `bpp` bytes per pixel), optionally Adam7.
fn scanlines(w: usize, h: usize, bpp: usize, pixels: &[u8], interlace: bool) -> Vec<u8> {
    let mut raw = Vec::new();
    if !interlace {
        for row in pixels.chunks_exact(w * bpp) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        return raw;
    }
    for (x0, y0, dx, dy) in ADAM7 {
        if x0 >= w || y0 >= h {
            continue;
        }
        for y in (y0..h).step_by(dy) {
            raw.push(0);
            for x in (x0..w).step_by(dx) {
                raw.extend_from_slice(&pixels[(y * w + x) * bpp..][..bpp]);
            }
        }
    }
    raw
}

/// (chunk type, chunk data)
type RawChunk = ([u8; 4], Vec<u8>);

struct Spec<'a> {
    w: u32,
    h: u32,
    depth: u8,
    color_type: u8,
    interlace: bool,
    pre: Vec<RawChunk>,
    pixels: &'a [u8],
}

fn build_png(s: &Spec<'_>) -> Vec<u8> {
    let channels = match s.color_type {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => 1,
    };
    let bpp = channels * (s.depth as usize / 8).max(1);
    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&s.w.to_be_bytes());
    ihdr.extend_from_slice(&s.h.to_be_bytes());
    ihdr.extend_from_slice(&[s.depth, s.color_type, 0, 0, s.interlace as u8]);
    chunk(&mut out, b"IHDR", &ihdr);
    for (ty, data) in &s.pre {
        chunk(&mut out, ty, data);
    }
    let raw = scanlines(s.w as usize, s.h as usize, bpp, s.pixels, s.interlace);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn iccp(icc: &[u8]) -> RawChunk {
    let mut d = b"test profile\0\0".to_vec();
    d.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(icc, 6));
    (*b"iCCP", d)
}

fn rgb8_png(w: u32, h: u32, pixels: &[u8], pre: Vec<RawChunk>) -> Vec<u8> {
    build_png(&Spec { w, h, depth: 8, color_type: 2, interlace: false, pre, pixels })
}

fn small_rgb8() -> Vec<u8> {
    (0..64u32).flat_map(|i| [(i * 4) as u8, (255 - i * 3) as u8, (i * 7 % 256) as u8]).collect()
}

/// Minimal ICC header (128 bytes) + tag table; `tags` are (sig, body) pairs.
fn icc_bytes(class: &[u8; 4], space: &[u8; 4], pcs: &[u8; 4], tags: &[RawChunk]) -> Vec<u8> {
    let table = 4 + 12 * tags.len();
    let mut body = Vec::new();
    let mut tab = (tags.len() as u32).to_be_bytes().to_vec();
    for (sig, data) in tags {
        tab.extend_from_slice(sig);
        tab.extend_from_slice(&((128 + table + body.len()) as u32).to_be_bytes());
        tab.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
    }
    let mut h = vec![0u8; 128];
    h[0..4].copy_from_slice(&((128 + table + body.len()) as u32).to_be_bytes());
    h[8] = 4;
    h[9] = 0x30;
    h[12..16].copy_from_slice(class);
    h[16..20].copy_from_slice(space);
    h[20..24].copy_from_slice(pcs);
    h[36..40].copy_from_slice(b"acsp");
    h.extend_from_slice(&tab);
    h.extend_from_slice(&body);
    h
}

// ---------------------------------------------------------------------------------------------
// COL-005: convert vs assign, LittleCMS oracle

#[test]
fn col005_p3_convert_matches_littlecms_and_assign_differs() {
    let exp = expected();
    let f = &exp["fixtures"]["p3_icc_rgb8.png"];
    let src = ints(&f["source_rgba8"]);
    let lcms_noopt = ints(&f["lcms_to_veskra_srgb_rgb8_nooptimize"]);
    let lcms_default = ints(&f["lcms_to_veskra_srgb_rgb8_default"]);
    let bytes = fixture("p3_icc_rgb8.png");

    let conv = import_png(&bytes, &srgb(), &convert_opts()).unwrap();
    assert!(conv.converted && !conv.profile_assumed);
    assert_eq!(conv.source_profile.as_ref().unwrap().semantic_name, "Display P3");
    assert_eq!(conv.source_profile.as_ref().unwrap().icc_hash, IccProfile::display_p3().id().icc_hash);
    assert!(conv.warnings.iter().any(|w| w.contains("Converted from 'Display P3'")), "{:?}", conv.warnings);
    let c8 = straight8(&conv);
    let (worst_n, diff_n) = rgb_diff(&c8, &lcms_noopt);
    let (worst_d, diff_d) = rgb_diff(&c8, &lcms_default);
    eprintln!(
        "COL-005 P3->sRGB convert vs LittleCMS 2.19 ({} px): NOOPTIMIZE max {} LSB ({} channels differ), default max {} LSB ({} differ)",
        conv.width * conv.height,
        worst_n,
        diff_n,
        worst_d,
        diff_d
    );
    assert!(worst_n <= 1 && worst_d <= 1, "must match LittleCMS within 1 LSB");

    let asg = import_png(&bytes, &srgb(), &assign_opts()).unwrap();
    assert!(!asg.converted && !asg.profile_assumed);
    assert_eq!(asg.source_profile, conv.source_profile);
    assert!(asg.warnings.iter().any(|w| w.contains("without conversion")), "{:?}", asg.warnings);
    let a8 = straight8(&asg);
    assert_eq!(a8, src, "assign keeps the encoded numbers exactly");

    // Visibly different: most non-neutral swatches change, by a lot for saturated ones.
    let mut changed = 0;
    let mut worst = 0u8;
    for (a, c) in a8.chunks_exact(4).zip(c8.chunks_exact(4)) {
        let d = (0..3).map(|i| a[i].abs_diff(c[i])).max().unwrap();
        worst = worst.max(d);
        changed += (d > 0) as usize;
    }
    eprintln!("COL-005 assign vs convert: {changed}/256 pixels differ, max {worst} LSB");
    assert!(changed > 150 && worst > 40, "convert and assign must be visibly different");

    // Same profile on both sides: exact identity, nothing converted.
    let same = import_png(&bytes, &IccProfile::display_p3(), &convert_opts()).unwrap();
    assert!(!same.converted);
    assert_eq!(straight8(&same), src);
}

#[test]
fn gray_icc_converted_like_littlecms_assign_replicates() {
    let exp = expected();
    let f = &exp["fixtures"]["gray18_icc_gray8.png"];
    let lcms = ints(&f["lcms_to_veskra_srgb_rgb8_nooptimize"]);
    let lcms_default = ints(&f["lcms_to_veskra_srgb_rgb8_default"]);
    let bytes = fixture("gray18_icc_gray8.png");
    let conv = import_png(&bytes, &srgb(), &convert_opts()).unwrap();
    assert!(conv.converted);
    assert_eq!(conv.source_profile_name, "Veskra test Gray gamma 1.8");
    let c8 = straight8(&conv);
    let (wn, dn) = rgb_diff(&c8, &lcms);
    let (wd, dd) = rgb_diff(&c8, &lcms_default);
    eprintln!(
        "Gray γ1.8 -> sRGB vs LittleCMS: NOOPTIMIZE max {wn} LSB ({dn} differ), default max {wd} LSB ({dd} differ)"
    );
    assert!(wn <= 1 && wd <= 1);
    let asg = import_png(&bytes, &srgb(), &assign_opts()).unwrap();
    let a8 = straight8(&asg);
    for (i, p) in a8.chunks_exact(4).enumerate() {
        assert_eq!(p, [i as u8, i as u8, i as u8, 255]);
    }
}

// ---------------------------------------------------------------------------------------------
// Tagged / untagged sRGB, alpha

#[test]
fn srgb_chunk_rgba8_exact_and_alpha_zero_cleared() {
    let exp = expected();
    let src = ints(&exp["fixtures"]["srgb_chunk_rgba8.png"]["source_rgba8"]);
    assert!(src.chunks_exact(4).any(|p| p[3] == 0 && p[..3] != [0, 0, 0]), "fixture has colored alpha-0 pixels");
    let img = import_png(&fixture("srgb_chunk_rgba8.png"), &srgb(), &convert_opts()).unwrap();
    assert!(!img.converted && !img.profile_assumed && img.warnings.is_empty(), "{:?}", img.warnings);
    assert_eq!(img.source_profile_name, "sRGB IEC61966-2.1");
    assert_eq!(img.source_bit_depth, 8);
    let mut want = vec![0u16; src.len()];
    convert::straight8_to_premul16(&src, &mut want);
    let got = premul16(&img);
    assert_eq!(got, want, "8-bit sRGB stored exactly as the contract conversion");
    for (s, g) in src.chunks_exact(4).zip(got.chunks_exact(4)) {
        if s[3] == 0 {
            assert_eq!(g, [0, 0, 0, 0]);
        }
    }
}

#[test]
fn independent_srgb_icc_is_identity() {
    let exp = expected();
    let src = ints(&exp["fixtures"]["lcms_srgb_icc_rgb8.png"]["source_rgba8"]);
    let img = import_png(&fixture("lcms_srgb_icc_rgb8.png"), &srgb(), &convert_opts()).unwrap();
    assert!(!img.converted && !img.profile_assumed);
    assert_eq!(img.source_profile_name, "sRGB IEC61966-2.1", "LittleCMS sRGB recognised as sRGB");
    assert_ne!(img.source_profile.as_ref().unwrap().icc_hash, srgb().id().icc_hash, "but its own bytes are identified");
    assert_eq!(straight8(&img), src);
}

#[test]
fn col004_untagged_assumes_srgb_or_rejects_by_policy() {
    let bytes = fixture("untagged_rgb8.png");
    let img = import_png(&bytes, &srgb(), &convert_opts()).unwrap();
    assert!(img.profile_assumed && img.source_profile.is_none() && !img.converted);
    assert_eq!(img.source_profile_name, "sRGB IEC61966-2.1");
    assert!(img.warnings.iter().any(|w| w.contains("sRGB IEC61966-2.1 assumed")), "{:?}", img.warnings);
    let src = ints(&expected()["fixtures"]["untagged_rgb8.png"]["source_rgba8"]);
    assert_eq!(straight8(&img), src);

    // Assumed sRGB into a P3 document converts (and says so).
    let p3 = import_png(&bytes, &IccProfile::display_p3(), &convert_opts()).unwrap();
    assert!(p3.profile_assumed && p3.converted);

    let opts = ImportOptions { missing_profile: MissingProfilePolicy::Reject, ..Default::default() };
    let e = err(import_png(&bytes, &srgb(), &opts));
    assert_eq!(e.code, ErrorCode::InvalidProfile);
    assert!(e.message.contains("no usable color profile"), "{}", e.message);
}

#[test]
fn gama_chrm_interpretation() {
    let px = small_rgb8();
    let encode = |gamma: Option<f32>, chrm: bool| {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 8, 8);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        if let Some(g) = gamma {
            enc.set_source_gamma(png::ScaledFloat::new(g));
        }
        if chrm {
            enc.set_source_chromaticities(png::SourceChromaticities::new(
                (0.3127, 0.3290),
                (0.64, 0.33),
                (0.30, 0.60),
                (0.15, 0.06),
            ));
        }
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&px).unwrap();
        w.finish().unwrap();
        out
    };
    let img = import_png(&encode(Some(1.0 / 2.2), true), &srgb(), &convert_opts()).unwrap();
    assert!(!img.profile_assumed && img.source_profile.is_some());
    assert!(img.warnings.iter().any(|w| w.contains("interpreted as sRGB")), "{:?}", img.warnings);
    let lin = import_png(&encode(Some(1.0), false), &srgb(), &convert_opts()).unwrap();
    assert!(lin.profile_assumed);
    assert!(lin.warnings.iter().any(|w| w.contains("gAMA 1.00000 was not modelled")), "{:?}", lin.warnings);
    let only_gamma = import_png(&encode(Some(1.0 / 2.2), false), &srgb(), &convert_opts()).unwrap();
    assert!(only_gamma.profile_assumed, "gAMA without cHRM on RGB is not enough to claim sRGB");
    let opts = ImportOptions { missing_profile: MissingProfilePolicy::Reject, ..Default::default() };
    assert_eq!(err(import_png(&encode(Some(1.0), false), &srgb(), &opts)).code, ErrorCode::InvalidProfile);
}

#[test]
fn cicp_precedence_and_unsupported_transfer() {
    let px = small_rgb8();
    let p3 = rgb8_png(8, 8, &px, vec![(*b"cICP", vec![12, 13, 0, 1])]);
    let img = import_png(&p3, &srgb(), &convert_opts()).unwrap();
    assert_eq!(img.source_profile_name, "Display P3");
    assert!(img.converted);
    let pq = rgb8_png(8, 8, &px, vec![(*b"cICP", vec![9, 16, 0, 1])]);
    assert_eq!(err(import_png(&pq, &srgb(), &convert_opts())).code, ErrorCode::UnsupportedColorMode);
    let pq_icc = rgb8_png(8, 8, &px, vec![(*b"cICP", vec![9, 16, 0, 1]), iccp(srgb().bytes())]);
    let img = import_png(&pq_icc, &srgb(), &convert_opts()).unwrap();
    assert!(img.warnings.iter().any(|w| w.contains("using the embedded ICC profile")), "{:?}", img.warnings);
    assert!(!img.profile_assumed);
}

// ---------------------------------------------------------------------------------------------
// COL-004: corrupt / unsupported embedded profiles

#[test]
fn col004_corrupt_iccp_rejected_never_silently_ignored() {
    let px = small_rgb8();
    let mut broken_zlib = b"broken\0\0".to_vec();
    broken_zlib.extend_from_slice(&[0x78, 0x9c, 0xff, 0xff, 0x00, 0x13, 0x37]);
    let garbage_icc = iccp(&[0x42; 300]);
    let mut truncated = srgb().bytes().to_vec();
    truncated.truncate(truncated.len() - 40);
    let truncated_icc = iccp(&truncated);
    let gray_on_rgb = iccp(&std::fs::read(fixture_dir().join("gray-gamma18-v4.icc")).unwrap());
    let cases: Vec<(&str, RawChunk)> = vec![
        ("corrupt zlib", (*b"iCCP", broken_zlib)),
        ("garbage profile", garbage_icc),
        ("truncated profile", truncated_icc),
        ("gray profile on RGB image", gray_on_rgb),
    ];
    for (name, chunk) in cases {
        let file = rgb8_png(8, 8, &px, vec![chunk]);
        let e = err(import_png(&file, &srgb(), &convert_opts()));
        assert_eq!(e.code, ErrorCode::InvalidProfile, "{name}: {}", e.message);
        assert!(e.message.contains("embedded ICC profile is unusable"), "{name}: {}", e.message);
        let lenient = ImportOptions { invalid_profile: InvalidProfilePolicy::AssumeSrgb, ..Default::default() };
        let img = import_png(&file, &srgb(), &lenient).unwrap();
        assert!(img.profile_assumed && img.source_profile.is_none(), "{name}");
        assert!(img.warnings.iter().any(|w| w.contains("ignored by explicit policy")), "{name}: {:?}", img.warnings);
    }
    // Why Veskra extracts iCCP itself: the png crate drops the corrupt stream without an error.
    let mut broken = b"broken\0\0".to_vec();
    broken.extend_from_slice(&[0x78, 0x9c, 0xff, 0xff]);
    let file = rgb8_png(8, 8, &px, vec![(*b"iCCP", broken)]);
    let r = png::Decoder::new(std::io::Cursor::new(&file)).read_info().unwrap();
    assert!(r.info().icc_profile.is_none());
}

#[test]
fn col004_cmyk_and_lut_profiles_unsupported() {
    let px = small_rgb8();
    let cmyk = icc_bytes(b"prtr", b"CMYK", b"Lab ", &[]);
    let mut a2b0 = b"mAB \0\0\0\0".to_vec();
    a2b0.extend_from_slice(&[0u8; 24]);
    let lut_rgb = icc_bytes(b"mntr", b"RGB ", b"Lab ", &[(*b"A2B0", a2b0)]);
    for (name, icc) in [("CMYK", cmyk), ("LUT-only RGB", lut_rgb)] {
        let file = rgb8_png(8, 8, &px, vec![iccp(&icc)]);
        let e = err(import_png(&file, &srgb(), &convert_opts()));
        assert_eq!(e.code, ErrorCode::UnsupportedColorMode, "{name}: {}", e.message);
    }
    // A gray document profile is not a valid working space.
    let gray = IccProfile::parse(&std::fs::read(fixture_dir().join("gray-gamma18-v4.icc")).unwrap()).unwrap();
    let e = err(import_png(&fixture("untagged_rgb8.png"), &gray, &convert_opts()));
    assert_eq!(e.code, ErrorCode::UnsupportedColorMode);
}

// ---------------------------------------------------------------------------------------------
// Bit depth, palette, interlace

fn gradient16(n: usize) -> Vec<u16> {
    (0..n)
        .flat_map(|i| {
            let v = ((i as u64 * 65535 + (n as u64 - 1) / 2) / (n as u64 - 1)) as u16;
            [v, v, (v as u32 * 15 / 16) as u16]
        })
        .collect()
}

fn encode16(w: u32, h: u32, color: png::ColorType, samples: &[u16], icc: Option<&[u8]>) -> Vec<u8> {
    let mut info = png::Info::with_size(w, h);
    info.color_type = color;
    info.bit_depth = png::BitDepth::Sixteen;
    if let Some(p) = icc {
        info.icc_profile = Some(std::borrow::Cow::Owned(p.to_vec()));
    }
    let mut out = Vec::new();
    let mut wr = png::Encoder::with_info(&mut out, info).unwrap().write_header().unwrap();
    let be: Vec<u8> = samples.iter().flat_map(|v| v.to_be_bytes()).collect();
    wr.write_image_data(&be).unwrap();
    wr.finish().unwrap();
    out
}

#[test]
fn sixteen_bit_gradient_keeps_precision() {
    let n = 4096;
    let g = gradient16(n);
    // Untagged 16-bit: exact (no 8-bit detour).
    let img = import_png(&encode16(n as u32, 1, png::ColorType::Rgb, &g, None), &srgb(), &convert_opts()).unwrap();
    assert_eq!(img.source_bit_depth, 16);
    let got = premul16(&img);
    for (i, (s, p)) in g.chunks_exact(3).zip(got.chunks_exact(4)).enumerate() {
        assert_eq!([s[0], s[1], s[2], 65535], [p[0], p[1], p[2], p[3]], "pixel {i}");
    }
    // P3-tagged 16-bit converted to sRGB: smooth, monotonic, far more than 256 levels.
    let p3 = IccProfile::display_p3();
    let img = import_png(&encode16(n as u32, 1, png::ColorType::Rgb, &g, Some(p3.bytes())), &srgb(), &convert_opts())
        .unwrap();
    assert!(img.converted);
    let got = premul16(&img);
    let t = RgbTransform::new(&p3, &srgb(), RenderingIntent::RelativeColorimetric).unwrap();
    let mut levels = std::collections::BTreeSet::new();
    let mut max_step = 0u16;
    let mut worst_vs_f64 = 0u16;
    for i in 0..n {
        let p = &got[i * 4..i * 4 + 4];
        let s = &g[i * 3..i * 3 + 3];
        let e = t.apply_f64([s[0] as f64 / 65535.0, s[1] as f64 / 65535.0, s[2] as f64 / 65535.0]);
        for c in 0..3 {
            worst_vs_f64 = worst_vs_f64.max(p[c].abs_diff(convert::f64_to_u16(e[c])));
        }
        levels.insert(p[0]);
        if i > 0 {
            let prev = got[(i - 1) * 4];
            assert!(p[0] >= prev, "monotonic at {i}");
            max_step = max_step.max(p[0] - prev);
        }
    }
    eprintln!(
        "16-bit P3->sRGB ramp: {} distinct red levels / {n}, max step {max_step}/65535, max diff vs f64 {worst_vs_f64}",
        levels.len()
    );
    assert!(levels.len() > 3500, "no banding: 16-bit precision kept");
    assert!(max_step <= 32, "input step is 16/65535; output step bounded");
    assert!(worst_vs_f64 <= 1, "table evaluation equals the f64 transform within 1/65535");

    // 16-bit gray+alpha through a gray profile (65536-entry table).
    let gicc = std::fs::read(fixture_dir().join("gray-gamma18-v4.icc")).unwrap();
    let ga: Vec<u16> =
        (0..256u32).flat_map(|i| [(i * 257) as u16, if i % 7 == 0 { 0 } else { 65535 - i as u16 }]).collect();
    let img = import_png(&encode16(16, 16, png::ColorType::GrayscaleAlpha, &ga, Some(&gicc)), &srgb(), &convert_opts())
        .unwrap();
    let gp = IccProfile::parse(&gicc).unwrap();
    let gt = GrayToRgbTransform::new(&gp, &srgb(), RenderingIntent::RelativeColorimetric).unwrap();
    for (s, p) in ga.chunks_exact(2).zip(premul16(&img).chunks_exact(4)) {
        if s[1] == 0 {
            assert_eq!(p, [0, 0, 0, 0]);
            continue;
        }
        let rgb = gt.apply(s[0] as f32 / 65535.0).map(|c| convert::f64_to_u16(c as f64));
        let mut want = [0u16; 4];
        convert::straight16_to_premul16(&[rgb[0], rgb[1], rgb[2], s[1]], &mut want);
        assert_eq!(p, want);
    }
}

#[test]
fn table_conversion_matches_f64_transform() {
    // 42875 8-bit colors (every 8th code per channel + 1, 254, 255), P3 -> sRGB, incl. clipping.
    let codes: Vec<u8> = (0..=255u8).step_by(8).chain([1, 254, 255]).collect();
    let mut px = Vec::new();
    for &r in &codes {
        for &g in &codes {
            for &b in &codes {
                px.extend_from_slice(&[r, g, b]);
            }
        }
    }
    let n = px.len() / 3;
    assert_eq!(n, 245 * 175);
    let (w, h) = (245, 175);
    let p3 = IccProfile::display_p3();
    for (src, dst) in [(p3.clone(), srgb()), (srgb(), p3.clone())] {
        let file = rgb8_png(w, h, &px, vec![iccp(src.bytes())]);
        let img = import_png(&file, &dst, &convert_opts()).unwrap();
        let t = RgbTransform::new(&src, &dst, RenderingIntent::RelativeColorimetric).unwrap();
        let got = premul16(&img);
        let (mut worst, mut differ) = (0u16, 0usize);
        for (s, p) in px.chunks_exact(3).zip(got.chunks_exact(4)) {
            let e = t.apply_f64([s[0] as f64 / 255.0, s[1] as f64 / 255.0, s[2] as f64 / 255.0]);
            for c in 0..3 {
                let d = p[c].abs_diff(convert::f64_to_u16(e[c]));
                worst = worst.max(d);
                differ += (d != 0) as usize;
            }
        }
        eprintln!(
            "table vs f64 {} -> {}: {n} colors, max {worst}/65535, {differ} channels differ",
            src.id().semantic_name,
            dst.id().semantic_name
        );
        assert!(worst <= 1);
    }
}

#[test]
fn palette_trns_and_low_bit_gray_expand() {
    let exp = expected();
    let src = ints(&exp["fixtures"]["palette_trns.png"]["source_rgba8"]);
    let img = import_png(&fixture("palette_trns.png"), &srgb(), &convert_opts()).unwrap();
    assert!(img.profile_assumed);
    assert_eq!(normalize_alpha0(&straight8(&img)), normalize_alpha0(&src));
    assert!(src.chunks_exact(4).any(|p| p[3] != 0 && p[3] != 255), "partial palette alpha present");

    // 2-bit gray, 3x2: values 0..3 scale to 0, 85, 170, 255.
    let packed = [0b0001_1011u8, 0b1110_0100];
    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut ihdr = 3u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&2u32.to_be_bytes());
    ihdr.extend_from_slice(&[2, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let raw = [0, packed[0], 0, packed[1]];
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    let img = import_png(&out, &srgb(), &convert_opts()).unwrap();
    assert_eq!(img.source_bit_depth, 2);
    let g: Vec<u8> = straight8(&img).chunks_exact(4).map(|p| p[0]).collect();
    assert_eq!(g, [0, 85, 170, 255, 170, 85]);
}

#[test]
fn interlaced_matches_pillow_and_progressive() {
    let exp = expected();
    let src = ints(&exp["fixtures"]["interlaced_rgba8.png"]["source_rgba8"]);
    let img = import_png(&fixture("interlaced_rgba8.png"), &srgb(), &convert_opts()).unwrap();
    assert_eq!((img.width, img.height), (13, 11));
    assert_eq!(normalize_alpha0(&straight8(&img)), normalize_alpha0(&src));

    // Adam7 vs non-interlaced for awkward sizes (empty passes, > one tile) at 16-bit RGBA.
    for (w, h) in [(1u32, 1u32), (2, 9), (9, 2), (5, 5), (70, 66)] {
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = (i.wrapping_mul(2654435761) >> 7) as u16;
                [v, v.rotate_left(5), v ^ 0x5a5a, if i % 11 == 0 { 0 } else { v | 1 }]
            })
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let mk = |interlace| build_png(&Spec { w, h, depth: 16, color_type: 6, interlace, pre: vec![], pixels: &px });
        let a = import_png(&mk(true), &IccProfile::display_p3(), &convert_opts()).unwrap();
        let b = import_png(&mk(false), &IccProfile::display_p3(), &convert_opts()).unwrap();
        assert!(a.converted);
        assert_eq!(premul16(&a), premul16(&b), "{w}x{h}");
        assert_premul_invariant(&a);
    }
}

// ---------------------------------------------------------------------------------------------
// SEC-001: hostile structure

#[test]
fn hostile_structure_rejected_quickly() {
    let good = fixture("srgb_chunk_rgba8.png");
    // Truncation at every interesting length.
    for cut in [0, 7, 8, 15, 30, 33, 40, good.len() / 2, good.len() - 13, good.len() - 1] {
        let e = err(import_png(&good[..cut], &srgb(), &convert_opts()));
        assert_eq!(e.code, ErrorCode::CorruptData, "cut {cut}: {}", e.message);
    }
    // Bad CRC in IDAT and in iCCP.
    let mut bad = good.clone();
    let idat = bad.windows(4).position(|w| w == b"IDAT").unwrap();
    bad[idat + 10] ^= 0xff;
    assert_eq!(err(import_png(&bad, &srgb(), &convert_opts())).code, ErrorCode::CorruptData);
    let mut bad = fixture("p3_icc_rgb8.png");
    let at = bad.windows(4).position(|w| w == b"iCCP").unwrap();
    bad[at + 30] ^= 0x01;
    let e = err(import_png(&bad, &srgb(), &convert_opts()));
    assert!(e.code == ErrorCode::CorruptData && e.message.contains("CRC"), "{}", e.message);
    // Not a PNG.
    assert_eq!(err(import_png(b"GIF89a....", &srgb(), &convert_opts())).code, ErrorCode::CorruptData);

    // Decompression bomb: 60000x60000 declared in a ~100 byte file.
    let bomb_px = [0u8; 3];
    let mut bomb = rgb8_png(1, 1, &bomb_px, vec![]);
    bomb[16..20].copy_from_slice(&60000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&60000u32.to_be_bytes());
    let crc = crc32fast::hash(&bomb[12..29]);
    bomb[29..33].copy_from_slice(&crc.to_be_bytes());
    let t = Instant::now();
    let e = err(import_png(&bomb, &srgb(), &convert_opts()));
    assert_eq!(e.code, ErrorCode::AllocationLimit, "{}", e.message);
    assert!(t.elapsed() < Duration::from_millis(50), "rejected before decoding");
    // Over the pixel budget though each edge is legal; and zero dimensions.
    for (w, h, code) in [
        (16384u32, 16384u32, ErrorCode::AllocationLimit),
        (16385, 1, ErrorCode::AllocationLimit),
        (0, 1, ErrorCode::CorruptData),
    ] {
        let mut f = bomb.clone();
        f[16..20].copy_from_slice(&w.to_be_bytes());
        f[20..24].copy_from_slice(&h.to_be_bytes());
        let crc = crc32fast::hash(&f[12..29]);
        f[29..33].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(err(import_png(&f, &srgb(), &convert_opts())).code, code, "{w}x{h}");
    }
    // Legal 8192x8192 header but only a few rows of data: fails on the data, memory stays small.
    let rows = vec![0u8; (8192 * 4 + 1) * 3];
    let mut f = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut ihdr = 8192u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&8192u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut f, b"IHDR", &ihdr);
    chunk(&mut f, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&rows, 6));
    chunk(&mut f, b"IEND", &[]);
    let t = Instant::now();
    assert_eq!(err(import_png(&f, &srgb(), &convert_opts())).code, ErrorCode::CorruptData);
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[test]
fn cancellation_is_honoured() {
    let c = CancelToken::new();
    c.cancel();
    let e = err(import_png_cancellable(&fixture("untagged_rgb8.png"), &srgb(), &convert_opts(), &c));
    assert_eq!(e.code, ErrorCode::Cancelled);
}

// ---------------------------------------------------------------------------------------------
// Round trip: import -> veskra PNG export -> independent decode

#[test]
fn roundtrip_import_export_is_exact_for_8bit_srgb() {
    let exp = expected();
    for name in [
        "srgb_chunk_rgba8.png",
        "lcms_srgb_icc_rgb8.png",
        "untagged_rgb8.png",
        "palette_trns.png",
        "interlaced_rgba8.png",
    ] {
        let src = ints(&exp["fixtures"][name]["source_rgba8"]);
        let img = import_png(&fixture(name), &srgb(), &convert_opts()).unwrap();
        let mut doc = fx::new_doc(img.width, img.height);
        fx::add_pixels(&mut doc, 1, None, img.tiles.clone(), None, fx::props("Imported", BlendMode::Normal, 1.0));
        for embed in [false, true] {
            let mut out = Vec::new();
            let opts = ExportOptions { embed_color_profile: embed, ..Default::default() };
            PngExporter.export(&doc, &opts, &mut out, &CancelToken::new()).unwrap();
            let (w, h, rgba) = decode_png_rgba8(&out).unwrap();
            assert_eq!((w, h), (img.width, img.height));
            assert_eq!(rgba, normalize_alpha0(&src), "{name} embed={embed}: 0 LSB round trip");
            // And Veskra's own import of its export is lossless too.
            let back = import_png(&out, &srgb(), &convert_opts()).unwrap();
            assert!(!back.profile_assumed && !back.converted, "{name}: export is tagged sRGB");
            assert_eq!(premul16(&back), premul16(&img), "{name} embed={embed}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// SEC-001: mutation loop (fixed seed) — never panics, results always valid

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Chunk (data start, data len) positions, for CRC-preserving mutations.
fn chunks_of(f: &[u8]) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut p = 8;
    while p + 12 <= f.len() {
        let len = u32::from_be_bytes([f[p], f[p + 1], f[p + 2], f[p + 3]]) as usize;
        if p + 12 + len > f.len() {
            break;
        }
        v.push((p + 8, len));
        p += 12 + len;
    }
    v
}

#[test]
fn sec001_mutation_loop_no_panic() {
    let mut seeds: Vec<Vec<u8>> =
        ["p3_icc_rgb8.png", "srgb_chunk_rgba8.png", "gray18_icc_gray8.png", "palette_trns.png", "interlaced_rgba8.png"]
            .iter()
            .map(|n| fixture(n))
            .collect();
    seeds.push(encode16(64, 1, png::ColorType::Rgb, &gradient16(64), Some(IccProfile::display_p3().bytes())));
    // `VESKRA_IMPORT_FUZZ_ITERS` / `VESKRA_IMPORT_FUZZ_SEED` allow longer local runs.
    let iters: usize = std::env::var("VESKRA_IMPORT_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(5000);
    let seed: u64 =
        std::env::var("VESKRA_IMPORT_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5eed_1234_abcd_0001);
    let mut rng = Rng(seed | 1);
    let (mut ok, mut failed) = (0, 0);
    let opts = [convert_opts(), assign_opts()];
    let t = Instant::now();
    for it in 0..iters {
        let mut f = seeds[rng.below(seeds.len())].clone();
        let fix_crc = rng.below(3) != 0;
        for _ in 0..1 + rng.below(4) {
            let cs = chunks_of(&f);
            if f.is_empty() {
                break;
            }
            match rng.below(6) {
                0 | 1 if !cs.is_empty() => {
                    // Mutate inside a chunk's data and keep its CRC valid to reach deep parsing.
                    let (start, len) = cs[rng.below(cs.len())];
                    if len > 0 {
                        let at = start + rng.below(len);
                        f[at] = match rng.below(3) {
                            0 => f[at] ^ (1 << rng.below(8)),
                            1 => rng.next() as u8,
                            _ => [0, 0xff, 0x7f, 0x80][rng.below(4)],
                        };
                        if fix_crc {
                            let crc = crc32fast::hash(&f[start - 4..start + len]);
                            f[start + len..start + len + 4].copy_from_slice(&crc.to_be_bytes());
                        }
                    }
                }
                2 => {
                    let at = rng.below(f.len());
                    f[at] = rng.next() as u8;
                }
                3 => f.truncate(rng.below(f.len() + 1)),
                4 if !cs.is_empty() => {
                    // Corrupt a length field.
                    let (start, _) = cs[rng.below(cs.len())];
                    let v = [0u32, 1, 13, 0x7fff_ffff, 0xffff_ffff, rng.next() as u32][rng.below(6)];
                    f[start - 8..start - 4].copy_from_slice(&v.to_be_bytes());
                }
                _ => {
                    let at = rng.below(f.len() + 1);
                    let extra: Vec<u8> = (0..rng.below(16)).map(|_| rng.next() as u8).collect();
                    f.splice(at..at, extra);
                }
            }
        }
        match import_png(&f, &srgb(), &opts[it % 2]) {
            Ok(img) => {
                ok += 1;
                assert!(img.width >= 1 && img.height >= 1 && img.width <= 16384 && img.height <= 16384);
                assert!(img.tiles.content_bounds().x1 <= img.width as i32);
                assert_premul_invariant(&img);
            }
            Err(e) => {
                failed += 1;
                assert_ne!(e.code, ErrorCode::Internal, "iteration {it}: {}", e.message);
            }
        }
    }
    eprintln!("mutation loop: {iters} inputs (seed {seed:#x}), {ok} imported, {failed} rejected, {:?}", t.elapsed());
    assert!(ok > iters / 100 && failed > iters / 10, "mutations exercise both paths");
}
