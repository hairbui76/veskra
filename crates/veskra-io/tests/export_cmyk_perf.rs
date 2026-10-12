//! CMYK export timing (ADR-018). Ignored by default; run in release:
//! `cargo test --release -p veskra-io --test export_cmyk_perf -- --ignored --nocapture`.
//! Two full-canvas layers with a different color in almost every pixel (worst case for the
//! per-pixel memo), 8-bit, default options otherwise.
#![allow(clippy::unwrap_used)]

mod psd_fixtures;

use psd_fixtures as fx;
use std::time::Instant;
use veskra_core::export::{ExportFormat, ExportOptions, ExportProfile};
use veskra_core::{BlendMode, RectI};
use veskra_io::{CancelToken, exporter_for};

fn doc(n: u32) -> veskra_doc::Document {
    let mut d = fx::new_doc(n, n);
    let r = RectI::new(0, 0, n as i32, n as i32);
    let base =
        fx::grid(r, move |x, y| [(x * 255 / n as i32) as u8, (y * 255 / n as i32) as u8, ((x ^ y) & 255) as u8, 255]);
    fx::add_pixels(&mut d, 1, None, base, None, fx::props("Base", BlendMode::Normal, 1.0));
    let top =
        fx::grid(r, move |x, y| [((x * 7 + y * 3) & 255) as u8, 40, ((y * 5) & 255) as u8, ((x / 8) % 200) as u8]);
    fx::add_pixels(&mut d, 2, None, top, None, fx::props("Glaze", BlendMode::Normal, 0.8));
    d
}

#[test]
#[ignore]
fn cmyk_export_timing() {
    for n in [2048u32, 4096] {
        let d = doc(n);
        let cmyk = ExportOptions { profile: ExportProfile::Cmyk, ..Default::default() };
        for (label, format, opts) in [
            ("RGB PSD", ExportFormat::Psd, ExportOptions::default()),
            ("CMYK PSD", ExportFormat::Psd, cmyk.clone()),
            ("CMYK PSD 16-bit", ExportFormat::Psd, ExportOptions { bit_depth: 16, ..cmyk.clone() }),
            ("RGB PDF", ExportFormat::Pdf, ExportOptions::default()),
            ("CMYK PDF", ExportFormat::Pdf, cmyk.clone()),
        ] {
            let exp = exporter_for(format);
            let t = Instant::now();
            let pf = exp.preflight(&d, &opts);
            let pre = t.elapsed();
            let t = Instant::now();
            let mut out = Vec::new();
            let rep = exp.export(&d, &opts, &mut out, &CancelToken::new()).unwrap();
            let run = t.elapsed();
            let t = Instant::now();
            exp.validate(&out).unwrap();
            let val = t.elapsed();
            println!(
                "{n}x{n} {label:<16} preflight {:>7.1} ms  export {:>8.1} ms  validate {:>7.1} ms  {:>6.1} MB  TAC {:?} oog {}",
                pre.as_secs_f64() * 1e3,
                run.as_secs_f64() * 1e3,
                val.as_secs_f64() * 1e3,
                out.len() as f64 / 1e6,
                rep.total_area_coverage_max.or(pf.total_area_coverage_max),
                rep.out_of_gamut_pixels
            );
        }
    }
}
