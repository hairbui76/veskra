//! CMYK / LUT profile parsing: a synthetic ICC v4 CMYK profile built here (`mAB `/`mBA ` with
//! analytically known tables), hostile inputs (every truncation, deterministic byte mutations of
//! the synthetic and the bundled profile, absurd sizes) and the size limit (ADR-018).
//! The parser must return typed errors and never panic or over-allocate.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use veskra_color::ColorError;
use veskra_color::cmyk::{BUNDLED_CMYK_PROFILE, CmykProfile, CmykTransform, inspect_for_cmyk_export};
use veskra_color::icc::IccProfile;
use veskra_color::transform::RenderingIntent;
use veskra_core::limits::MAX_OUTPUT_PROFILE_BYTES;

fn s15(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn pad4(mut v: Vec<u8>) -> Vec<u8> {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
    v
}

fn curv_identity() -> Vec<u8> {
    b"curv\0\0\0\0\0\0\0\0".to_vec()
}

fn curv_gamma(g256: u16) -> Vec<u8> {
    let mut v = b"curv\0\0\0\0\0\0\0\x01".to_vec();
    v.extend_from_slice(&g256.to_be_bytes());
    pad4(v)
}

fn para_gamma(g: f64) -> Vec<u8> {
    let mut v = b"para\0\0\0\0\0\0\0\0".to_vec();
    v.extend_from_slice(&s15(g));
    v
}

/// Affine A2B test function: CMYK → normalized PCS.
fn a2b_fn(c: [f64; 4]) -> [f64; 3] {
    [
        0.9 - 0.2 * c[0] - 0.15 * c[1] - 0.1 * c[2] - 0.4 * c[3],
        0.5 + 0.2 * c[1] - 0.15 * c[0],
        0.5 + 0.25 * c[2] - 0.1 * c[0],
    ]
}

/// Affine B2A test function on the CLUT input (after the matrix).
fn b2a_fn(p: [f64; 3]) -> [f64; 4] {
    [0.6 - 0.4 * p[0] + 0.1 * p[1], 0.3 + 0.2 * p[1] - 0.1 * p[2], 0.2 + 0.3 * p[2], 0.8 - 0.7 * p[0]]
}

fn clut(grid: &[u8], outputs: usize, wide: bool, f: &dyn Fn(&[f64]) -> Vec<f64>) -> Vec<u8> {
    let mut v = grid.to_vec();
    v.resize(16, 0);
    v.extend_from_slice(&[if wide { 2 } else { 1 }, 0, 0, 0]);
    let n: usize = grid.iter().map(|&g| g as usize).product();
    for idx in 0..n {
        let mut rem = idx;
        let mut coord = vec![0.0; grid.len()];
        for d in (0..grid.len()).rev() {
            let g = grid[d] as usize;
            coord[d] = (rem % g) as f64 / (g - 1) as f64;
            rem /= g;
        }
        let out = f(&coord);
        assert_eq!(out.len(), outputs);
        for o in out {
            let o = o.clamp(0.0, 1.0);
            if wide {
                v.extend_from_slice(&((o * 65535.0).round() as u16).to_be_bytes());
            } else {
                v.push((o * 255.0).round() as u8);
            }
        }
    }
    pad4(v)
}

fn lut_ab(sig: &[u8; 4], inputs: u8, outputs: u8, parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut offs = [0u32; 5];
    for (name, data) in parts {
        let slot = ["B", "matrix", "M", "clut", "A"].iter().position(|n| n == name).unwrap();
        offs[slot] = 32 + body.len() as u32;
        body.extend_from_slice(data);
    }
    let mut v = sig.to_vec();
    v.extend_from_slice(&[0, 0, 0, 0, inputs, outputs, 0, 0]);
    for o in offs {
        v.extend_from_slice(&o.to_be_bytes());
    }
    v.extend_from_slice(&body);
    pad4(v)
}

fn matrix(m: [f64; 9], off: [f64; 3]) -> Vec<u8> {
    m.iter().chain(off.iter()).flat_map(|v| s15(*v)).collect()
}

/// A small ICC v4.3 CMYK output profile: A2B0 = `mAB ` (identity A/B curves, 16-bit CLUT on a
/// non-uniform 2×3×2×2 grid holding an affine function, para M curves, matrix with offset) and
/// B2A0 = `mBA ` (matrix halving L, 8-bit 3×3×3 CLUT holding an affine function).
fn synthetic_v4() -> Vec<u8> {
    let a2b = lut_ab(
        b"mAB ",
        4,
        3,
        &[
            ("A", [curv_identity(), curv_gamma(256), para_gamma(1.0), curv_identity()].concat()),
            ("clut", clut(&[2, 3, 2, 2], 3, true, &|c| a2b_fn([c[0], c[1], c[2], c[3]]).to_vec())),
            ("M", [para_gamma(1.0), para_gamma(1.0), para_gamma(1.0)].concat()),
            ("matrix", matrix([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [0.01, 0.0, -0.01])),
            ("B", [curv_identity(), curv_identity(), curv_identity()].concat()),
        ],
    );
    let b2a = lut_ab(
        b"mBA ",
        3,
        4,
        &[
            ("B", [curv_identity(), curv_identity(), curv_identity()].concat()),
            ("matrix", matrix([0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [0.0; 3])),
            ("M", [curv_identity(), curv_identity(), curv_identity()].concat()),
            ("clut", clut(&[3, 3, 3], 4, false, &|p| b2a_fn([p[0], p[1], p[2]]).to_vec())),
            ("A", [curv_identity(), curv_identity(), curv_identity(), curv_identity()].concat()),
        ],
    );
    let desc = {
        let s: Vec<u8> = "Synthetic v4 CMYK".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let mut v = b"mluc\0\0\0\0".to_vec();
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&12u32.to_be_bytes());
        v.extend_from_slice(b"enUS");
        v.extend_from_slice(&(s.len() as u32).to_be_bytes());
        v.extend_from_slice(&28u32.to_be_bytes());
        v.extend_from_slice(&s);
        pad4(v)
    };
    let wtpt: Vec<u8> =
        [b"XYZ \0\0\0\0".to_vec(), [0.9642, 1.0, 0.8249].iter().flat_map(|v| s15(*v)).collect()].concat();
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"desc", desc), (b"wtpt", wtpt), (b"A2B0", a2b), (b"B2A0", b2a)];
    let table_len = 4 + 12 * tags.len();
    let mut data = Vec::new();
    let mut entries = Vec::new();
    for (sig, body) in &tags {
        entries.extend_from_slice(*sig);
        entries.extend_from_slice(&((128 + table_len + data.len()) as u32).to_be_bytes());
        entries.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(body);
    }
    let mut h = vec![0u8; 128];
    let size = 128 + table_len + data.len();
    h[0..4].copy_from_slice(&(size as u32).to_be_bytes());
    h[8..12].copy_from_slice(&[4, 0x30, 0, 0]);
    h[12..16].copy_from_slice(b"prtr");
    h[16..20].copy_from_slice(b"CMYK");
    h[20..24].copy_from_slice(b"Lab ");
    h[36..40].copy_from_slice(b"acsp");
    h.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    h.extend_from_slice(&entries);
    h.extend_from_slice(&data);
    h
}

fn pcs_to_lab(v: [f64; 3]) -> [f64; 3] {
    [v[0] * 100.0, v[1] * 255.0 - 128.0, v[2] * 255.0 - 128.0]
}

#[test]
fn synthetic_v4_profile_evaluates_its_tables_exactly() {
    let bytes = synthetic_v4();
    let p = CmykProfile::parse(&bytes).unwrap();
    assert_eq!(p.version().major, 4);
    assert_eq!(p.description(), "Synthetic v4 CMYK");
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for _ in 0..500 {
        let c = [rnd(), rnd(), rnd(), rnd()];
        for intent in [RenderingIntent::Perceptual, RenderingIntent::RelativeColorimetric] {
            let lab = p.cmyk_to_lab(c, intent);
            let f = a2b_fn(c);
            let want = pcs_to_lab([f[0] + 0.01, f[1], f[2] - 0.01]);
            for k in 0..3 {
                assert!((lab[k] - want[k]).abs() < 0.01, "{c:?}: {lab:?} vs {want:?}");
            }
        }
        // B2A: Lab → v4-normalized PCS → L halved by the matrix → affine CLUT (8-bit nodes).
        let pcs = [rnd(), rnd(), rnd()];
        let cmyk = p.lab_to_cmyk(pcs_to_lab(pcs), RenderingIntent::RelativeColorimetric);
        let want = b2a_fn([pcs[0] * 0.5, pcs[1], pcs[2]]);
        for k in 0..4 {
            assert!((cmyk[k] - want[k].clamp(0.0, 1.0)).abs() < 3.0 / 255.0, "{pcs:?}: {cmyk:?} vs {want:?}");
        }
    }
    // A transform can be built (BPC on: v4 perceptual forces it; relative uses the Adobe method).
    for intent in [RenderingIntent::Perceptual, RenderingIntent::RelativeColorimetric] {
        let t = CmykTransform::new(&IccProfile::srgb(), Arc::new(p.clone()), intent, true).unwrap();
        let k = t.rgb_to_cmyk([0.5, 0.4, 0.3]);
        assert!(k.iter().all(|v| (0.0..=1.0).contains(v)));
        let (rgb, _) = t.cmyk_to_rgb(k);
        assert!(rgb.iter().all(|v| (0.0..=1.0).contains(v)));
        let _ = t.link();
    }
    let (summary, reason) = inspect_for_cmyk_export(&bytes).unwrap();
    assert_eq!((summary.version_string().as_str(), reason), ("4.3", None));
}

#[test]
fn every_truncation_is_a_typed_error() {
    let bytes = synthetic_v4();
    for cut in 0..bytes.len() {
        let mut v = bytes[..cut].to_vec();
        // Keep the header size consistent so the error comes from the tag data, not the size check.
        if v.len() >= 4 {
            let n = v.len() as u32;
            v[0..4].copy_from_slice(&n.to_be_bytes());
        }
        assert!(CmykProfile::parse(&v).is_err(), "truncated at {cut}");
    }
}

fn mutate(bytes: &[u8], seed: &mut u64, region: std::ops::Range<usize>) -> Vec<u8> {
    let mut v = bytes.to_vec();
    let mut next = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    };
    let flips = 1 + (next() % 6) as usize;
    for _ in 0..flips {
        let at = region.start + (next() as usize) % region.len();
        let val = match next() % 4 {
            0 => 0x00,
            1 => 0xff,
            2 => 0x01,
            _ => (next() & 0xff) as u8,
        };
        v[at] = val;
    }
    v
}

#[test]
fn mutated_profiles_never_panic() {
    let mut seed = 0xdead_beef_cafe_f00du64;
    let synth = synthetic_v4();
    let mut ok = 0;
    for _ in 0..3000 {
        let v = mutate(&synth, &mut seed, 0..synth.len());
        if let Ok(p) = CmykProfile::parse(&v) {
            ok += 1;
            // Whatever parsed must evaluate without panicking, also for hostile values.
            for c in [[0.0; 4], [1.0; 4], [f64::NAN, 2.0, -1.0, 0.5]] {
                let lab = p.cmyk_to_lab(c, RenderingIntent::RelativeColorimetric);
                let _ = p.lab_to_cmyk(lab, RenderingIntent::Perceptual);
            }
            if let Ok(t) =
                CmykTransform::new(&IccProfile::srgb(), Arc::new(p), RenderingIntent::RelativeColorimetric, true)
            {
                let _ = t.rgb_to_cmyk_with_delta_e([0.2, 0.9, 0.4]);
                let _ = t.cmyk_to_rgb([0.3, 0.3, 0.3, 0.9]);
            }
        }
    }
    println!("synthetic: {ok} of 3000 mutated profiles still parsed");
    // The bundled profile: mutate the header, tag table and the LUT headers / curve tables (the
    // CLUT bodies are read in place and every value is valid there).
    let bundled = BUNDLED_CMYK_PROFILE;
    for _ in 0..300 {
        let v = mutate(bundled, &mut seed, 0..700);
        let _ = CmykProfile::parse(&v);
    }
    // A2B0 starts at 596: header + matrix + table counts + the start of the input tables.
    for _ in 0..300 {
        let v = mutate(bundled, &mut seed, 596..700);
        let _ = CmykProfile::parse(&v);
    }
}

#[test]
fn size_and_shape_limits() {
    let big = vec![0u8; MAX_OUTPUT_PROFILE_BYTES + 1];
    assert!(matches!(CmykProfile::parse(&big), Err(ColorError::TooLarge { .. })));
    assert!(matches!(CmykProfile::parse_vec(big), Err(ColorError::TooLarge { .. })));
    // Absurd CLUT grid claims are rejected before anything is read or allocated.
    let mut v = synthetic_v4();
    let clut_grid = v.windows(4).position(|w| w == [2, 3, 2, 2]).unwrap();
    v[clut_grid..clut_grid + 4].copy_from_slice(&[255, 255, 255, 255]);
    assert!(matches!(CmykProfile::parse(&v), Err(ColorError::InvalidTag { .. })));
    // An RGB matrix profile, a Lab abstract-like class and a non-CMYK space are typed errors.
    assert!(matches!(CmykProfile::parse(IccProfile::srgb().bytes()), Err(ColorError::UnsupportedProfileClass(_))));
    let mut rgb_space = synthetic_v4();
    rgb_space[16..20].copy_from_slice(b"RGB ");
    assert!(matches!(CmykProfile::parse(&rgb_space), Err(ColorError::UnsupportedColorSpace(_))));
    let mut abst = synthetic_v4();
    abst[12..16].copy_from_slice(b"abst");
    assert!(matches!(CmykProfile::parse(&abst), Err(ColorError::UnsupportedProfileClass(_))));
    let mut link = synthetic_v4();
    link[12..16].copy_from_slice(b"link");
    assert!(matches!(CmykProfile::parse(&link), Err(ColorError::UnsupportedProfileClass(_))));
    // `inspect` reports unsuitable classes instead of failing.
    let (s, reason) = inspect_for_cmyk_export(&abst).unwrap();
    assert_eq!(s.class, "abst");
    assert!(reason.unwrap().contains("abstract"));
    // Without B2A tables there is nothing to convert with.
    let mut no_b2a = synthetic_v4();
    let at = no_b2a.windows(4).position(|w| w == b"B2A0").unwrap();
    no_b2a[at..at + 4].copy_from_slice(b"zzzz");
    assert!(matches!(CmykProfile::parse(&no_b2a), Err(ColorError::MissingTag("B2A0"))));
    assert!(inspect_for_cmyk_export(&no_b2a).unwrap().1.unwrap().contains("B2A"));
    // Garbage is an error for inspect too.
    assert!(inspect_for_cmyk_export(b"definitely not an ICC profile").is_err());
}
