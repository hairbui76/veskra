//! # veskra-qa — QAP integration tests (test-only crate, never linked into the app)
//!
//! * [`report`]: per-scenario machine-readable results (`target/qa/<scenario>/result.json`) with
//!   `PASS` / `FAIL` / `NOT_RUN` sub-checks. Hardware/Adobe sub-checks are always `NOT_RUN` with a
//!   blocker id from `docs/BLOCKERS.md`; they are never reported as passed.
//! * [`drive`]: a JSON-command driver around [`veskra_engine::Session`] (the same command
//!   boundary Android uses over JNI) that logs every command for replay with `veskra run`.
//! * [`events`]: synthetic pointer-event builders that go through the packed JNI layout
//!   (`pack_events` → `unpack_events`) before reaching `Session::input_batch`.
//!   **SYNTHETIC — not hardware captures.**
//! * [`int02`]: the deterministic 12-layer ink/flats/shadow document shared by S2, S3 and the
//!   host performance test.
//! * [`tools`]: detection/invocation of independent oracle tools (python3 + psd-tools/Pillow,
//!   rsvg-convert, pdftocairo, qpdf).

pub mod drive;
pub mod events;
pub mod int02;
pub mod report;
pub mod tools;

use std::path::PathBuf;

/// Blocker ids from `docs/BLOCKERS.md`.
pub mod blockers {
    /// No Android SDK/NDK.
    pub const NO_ANDROID_SDK: &str = "B-1";
    /// No emulator / physical pen tablet.
    pub const NO_DEVICE: &str = "B-2";
    /// No Adobe Photoshop / Illustrator.
    pub const NO_ADOBE: &str = "B-3";
    /// No calibrated display instrumentation.
    pub const NO_DISPLAY_INSTRUMENT: &str = "B-6";
}

/// Repository root.
pub fn repo_root() -> PathBuf {
    veskra_testkit::repo_root()
}

/// Root of QA artifacts: `$VESKRA_QA_DIR` or `<repo>/target/qa`.
pub fn qa_root() -> PathBuf {
    std::env::var_os("VESKRA_QA_DIR").map(PathBuf::from).unwrap_or_else(|| repo_root().join("target/qa"))
}

/// Straight RGBA8 → premultiplied RGBA8 (renders are compared premultiplied so color under
/// alpha 0 does not count).
pub fn premul8(px: &[u8]) -> Vec<u8> {
    px.chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
            [m(p[0]), m(p[1]), m(p[2]), p[3]]
        })
        .collect()
}

/// Max / mean absolute difference and % of pixels whose worst channel differs by more than 16.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RenderDiff {
    pub max: u8,
    pub mean: f64,
    pub pct_over_16: f64,
}

impl RenderDiff {
    pub fn of(a: &[u8], b: &[u8]) -> Option<RenderDiff> {
        if a.len() != b.len() || !a.len().is_multiple_of(4) {
            return None;
        }
        let mut d = RenderDiff::default();
        let (mut sum, mut over) = (0u64, 0u64);
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
        Some(d)
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"max": self.max, "mean": (self.mean * 1e4).round() / 1e4,
                           "pctOver16": (self.pct_over_16 * 1e4).round() / 1e4})
    }
}

/// Veskra's reference composite of the whole canvas as straight RGBA8 (contract conversion).
pub fn composite8(doc: &veskra_doc::Document) -> Vec<u8> {
    let c = veskra_compose::composite_rect(doc, doc.bounds(), &veskra_compose::ComposeOptions::default());
    let mut out = vec![0u8; c.len()];
    veskra_color::convert::premul16_to_straight8(&c, &mut out);
    out
}

/// Straight RGBA8 of one pixel of a pixel layer (transparent for groups/unknown ids).
pub fn layer_px(doc: &veskra_doc::Document, id: veskra_core::LayerId, x: i32, y: i32) -> [u8; 4] {
    let p = doc.layer(id).and_then(|l| l.pixels()).map(|p| p.tiles.get_pixel(x, y)).unwrap_or([0; 4]);
    let mut out = [0u8; 4];
    veskra_color::convert::premul16_to_straight8(&p, &mut out);
    out
}

/// Pixel of a straight RGBA8 image.
pub fn px_at(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// CIEDE2000 between two opaque 8-bit sRGB colors (Veskra's verified implementation).
pub fn de00(a: [u8; 3], b: [u8; 3]) -> f64 {
    veskra_color::lab::ciede2000(veskra_color::lab::srgb8_to_lab_d50(a), veskra_color::lab::srgb8_to_lab_d50(b))
}
