//! # veskra-testkit — test support (QAP)
//!
//! * [`ImageDiff`]: per-channel statistics between two RGBA8 images.
//! * [`golden`]: golden-image assertions against `tests/fixtures/goldens/*.png`, decoded with the
//!   independent `png` crate. Set `VESKRA_BLESS=1` to (re)write goldens deliberately; blessing is a
//!   reviewed change, never automatic in CI.
//! * [`Rng`]: tiny deterministic PRNG for reproducible randomized tests.

use std::path::PathBuf;

/// Deterministic xorshift64* PRNG (not cryptographic).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

/// Statistics of the difference between two equally sized RGBA8 images.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageDiff {
    pub max_abs: u8,
    pub mean_abs: f64,
    /// Number of pixels where any channel differs by more than `threshold`.
    pub pixels_over: usize,
    pub threshold: u8,
    pub pixels: usize,
}

impl ImageDiff {
    /// Compare two RGBA8 buffers. Panics if the lengths differ (test helper).
    pub fn rgba8(a: &[u8], b: &[u8], threshold: u8) -> ImageDiff {
        assert_eq!(a.len(), b.len(), "image buffers differ in length");
        assert_eq!(a.len() % 4, 0);
        let mut max_abs = 0u8;
        let mut sum = 0u64;
        let mut over = 0usize;
        for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            let mut px_over = false;
            for c in 0..4 {
                let d = pa[c].abs_diff(pb[c]);
                max_abs = max_abs.max(d);
                sum += d as u64;
                if d > threshold {
                    px_over = true;
                }
            }
            if px_over {
                over += 1;
            }
        }
        ImageDiff {
            max_abs,
            mean_abs: sum as f64 / a.len().max(1) as f64,
            pixels_over: over,
            threshold,
            pixels: a.len() / 4,
        }
    }
}

/// Encode straight-alpha RGBA8 to PNG bytes (test artifacts only; production PNG export lives in veskra-io).
pub fn encode_png_rgba8(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(rgba).expect("png data");
    }
    out
}

/// Decode a PNG into straight RGBA8 using the independent `png` crate. Returns (w, h, rgba).
pub fn decode_png_rgba8(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG not expanded".into()),
    };
    Ok((w, h, rgba))
}

/// Repository root (two levels above this crate).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn fixtures_dir() -> PathBuf {
    repo_root().join("tests/fixtures")
}

pub mod golden {
    use super::*;

    /// Assert `rgba` (straight RGBA8) matches golden `name` within `threshold` per channel and at most
    /// `max_pixels_over` differing pixels. With `VESKRA_BLESS=1`, writes the golden instead.
    pub fn assert_matches(name: &str, w: u32, h: u32, rgba: &[u8], threshold: u8, max_pixels_over: usize) {
        let path = fixtures_dir().join("goldens").join(format!("{name}.png"));
        if std::env::var("VESKRA_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir goldens");
            std::fs::write(&path, encode_png_rgba8(w, h, rgba)).expect("write golden");
            eprintln!("blessed golden {}", path.display());
            return;
        }
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|_| panic!("missing golden {} (run with VESKRA_BLESS=1 to create)", path.display()));
        let (gw, gh, g) = decode_png_rgba8(&bytes).expect("decode golden");
        assert_eq!((gw, gh), (w, h), "golden {name} size mismatch");
        let d = ImageDiff::rgba8(rgba, &g, threshold);
        if d.pixels_over > max_pixels_over {
            let out = std::env::temp_dir().join(format!("veskra-golden-actual-{name}.png"));
            let _ = std::fs::write(&out, encode_png_rgba8(w, h, rgba));
            panic!("golden {name} mismatch: {d:?}; actual written to {}", out.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip_and_diff() {
        let mut rng = Rng::new(3);
        let img: Vec<u8> = (0..16 * 8 * 4).map(|_| rng.below(256) as u8).collect();
        let png = encode_png_rgba8(16, 8, &img);
        let (w, h, back) = decode_png_rgba8(&png).unwrap();
        assert_eq!((w, h), (16, 8));
        assert_eq!(back, img);
        let d = ImageDiff::rgba8(&img, &back, 0);
        assert_eq!(d.max_abs, 0);
        assert_eq!(d.pixels_over, 0);
        let mut other = img.clone();
        other[5] = other[5].wrapping_add(10);
        let d = ImageDiff::rgba8(&img, &other, 3);
        assert_eq!(d.pixels_over, 1);
    }

    #[test]
    fn rng_deterministic() {
        let mut a = Rng::new(9);
        let mut b = Rng::new(9);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert!((0..1000).map(|_| a.f64()).all(|v| (0.0..1.0).contains(&v)));
    }
}
