//! Panic-freedom of `IccProfile::parse` (and of transforms built from whatever it accepts)
//! under truncation and deterministic random mutation.

// Test-only code: fixture/IO failures should abort the test loudly.
#![allow(clippy::unwrap_used, clippy::needless_range_loop)]

use std::path::PathBuf;
use veskra_color::icc::IccProfile;
use veskra_color::transform::{RenderingIntent, RgbTransform};

/// xorshift64* — tiny deterministic PRNG so the corpus is identical on every run/platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn seeds() -> Vec<Vec<u8>> {
    let lcms = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/color/icc/lcms2-builtin-srgb.icc");
    vec![
        IccProfile::srgb().bytes().to_vec(),
        IccProfile::display_p3().bytes().to_vec(),
        std::fs::read(lcms).expect("lcms2-builtin-srgb.icc fixture"),
    ]
}

fn fix_size(b: &mut [u8]) {
    if b.len() >= 4 {
        let n = b.len() as u32;
        b[0..4].copy_from_slice(&n.to_be_bytes());
    }
}

/// Parse; if accepted, exercise it as transform source and destination. Returns acceptance.
fn exercise(bytes: &[u8]) -> bool {
    let Ok(p) = IccProfile::parse(bytes) else {
        return false;
    };
    let _ = (p.description(), p.id(), p.is_srgb_equivalent(), p.effective_media_white(), p.to_pcs_xyz([0.5; 3]));
    let srgb = IccProfile::srgb();
    for (src, dst) in [(&p, &*srgb), (&*srgb, &p)] {
        for intent in [RenderingIntent::RelativeColorimetric, RenderingIntent::AbsoluteColorimetric] {
            if let Ok(t) = RgbTransform::new(src, dst, intent) {
                for c in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.3, 0.6, 0.9]] {
                    let o = t.apply(c);
                    assert!(o.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{o:?}");
                }
                let mut px = [0u8, 128, 255, 255, 7, 7, 7, 0];
                t.apply_rgba8_straight(&mut px);
                let mut p16 = [100u16, 50, 25, 200, 65535, 65535, 65535, 65535];
                t.apply_rgba16_premul(&mut p16);
                assert!(p16[0] <= p16[3] && p16[1] <= p16[3] && p16[2] <= p16[3]);
            }
        }
    }
    true
}

#[test]
fn every_truncation_is_rejected_without_panic() {
    for seed in seeds() {
        assert!(exercise(&seed));
        for len in 0..seed.len() {
            // As-is (header size now lies) ...
            assert!(!exercise(&seed[..len]), "truncated profile of {len} bytes accepted");
            // ... and with the size field patched so parsing reaches the tag table/tag data.
            let mut t = seed[..len].to_vec();
            fix_size(&mut t);
            let _ = exercise(&t);
        }
    }
}

#[test]
fn random_mutations_never_panic() {
    let mut rng = Rng(0x5EED_C010_12C0_FFEE);
    let mut accepted = 0usize;
    let mut total = 0usize;
    let interesting = [0u32, 1, 2, 3, 4, 8, 12, 127, 128, 131, 132, 0x7fff_ffff, 0x8000_0000, 0xffff_fff0, u32::MAX];
    for seed in seeds() {
        for _ in 0..5_000 {
            let mut b = seed.clone();
            match rng.below(4) {
                // 1–8 random byte flips anywhere.
                0 => {
                    for _ in 0..1 + rng.below(8) {
                        let i = rng.below(b.len());
                        b[i] = rng.next() as u8;
                    }
                }
                // Overwrite an aligned u32 (header fields, tag offsets/sizes/counts) with an edge value.
                1 => {
                    let i = rng.below(b.len() / 4) * 4;
                    let v = interesting[rng.below(interesting.len())];
                    b[i..i + 4].copy_from_slice(&v.to_be_bytes());
                }
                // Mutate only the tag area so header checks pass more often.
                2 => {
                    for _ in 0..1 + rng.below(4) {
                        let i = 128 + rng.below(b.len() - 128);
                        b[i] ^= 1 << rng.below(8);
                    }
                }
                // Truncate or extend with random bytes, keeping the size field consistent.
                _ => {
                    let new_len = rng.below(b.len() * 2).max(1);
                    b.resize(new_len, 0);
                    for v in b.iter_mut().skip(seed.len()) {
                        *v = rng.next() as u8;
                    }
                    fix_size(&mut b);
                }
            }
            accepted += usize::from(exercise(&b));
            total += 1;
        }
    }
    println!("mutation corpus: {total} cases, {accepted} accepted as valid profiles, 0 panics");
    assert_eq!(total, 15_000);
}

#[test]
fn random_garbage_with_valid_magic() {
    let mut rng = Rng(42);
    for _ in 0..5_000 {
        let len = 132 + rng.below(1024);
        let mut b: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        b[36..40].copy_from_slice(b"acsp");
        b[8] = if rng.below(2) == 0 { 2 } else { 4 };
        b[12..16].copy_from_slice(b"mntr");
        b[16..20].copy_from_slice(if rng.below(2) == 0 { b"RGB " } else { b"GRAY" });
        b[20..24].copy_from_slice(b"XYZ ");
        let count = rng.below(12) as u32;
        b[128..132].copy_from_slice(&count.to_be_bytes());
        fix_size(&mut b);
        let _ = exercise(&b);
    }
}

/// Opt-in: parse every profile in `VESKRA_LOCAL_ICC_DIR` (third-party files are NOT committed;
/// e.g. `/usr/share/color/icc/krita`). Prints a classification table; asserts no panic and
/// checks well-known files when present.
#[test]
fn local_third_party_corpus() {
    let Ok(dir) = std::env::var("VESKRA_LOCAL_ICC_DIR") else {
        println!("VESKRA_LOCAL_ICC_DIR not set; skipping local corpus");
        return;
    };
    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        if !(name.to_ascii_lowercase().ends_with(".icc") || name.to_ascii_lowercase().ends_with(".icm")) {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let accepted = exercise(&bytes);
        match IccProfile::parse(&bytes) {
            Ok(p) => println!(
                "OK   {name:<48} v{}.{} {:?} srgb_equiv={} desc={:?}",
                p.version().major,
                p.version().minor,
                p.color_space(),
                p.is_srgb_equivalent(),
                p.description()
            ),
            Err(e) => println!("ERR  {name:<48} {e}"),
        }
        assert_eq!(accepted, IccProfile::parse(&bytes).is_ok());
        let parsed = IccProfile::parse(&bytes);
        match name.as_str() {
            "sRGB-elle-V2-srgbtrc.icc" => assert!(parsed.unwrap().is_srgb_equivalent()),
            "sRGB-elle-V2-g10.icc" | "ClayRGB-elle-V2-g22.icc" => assert!(!parsed.unwrap().is_srgb_equivalent()),
            "cmyk.icm" => assert!(matches!(parsed, Err(veskra_color::ColorError::UnsupportedColorSpace(_)))),
            _ => {}
        }
    }
}
