//! Independent oracle tools (never Veskra code): detection and invocation.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `true` if `name --version` (or `-v`) can be executed.
pub fn have(name: &str) -> bool {
    Command::new(name).arg("--version").output().is_ok() || Command::new(name).arg("-v").output().is_ok()
}

/// `true` if `python3 -I -c <probe>` succeeds (e.g. `"import psd_tools, numpy, PIL"`).
pub fn python_has(probe: &str) -> bool {
    Command::new("python3").args(["-I", "-c", probe]).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Run a command, returning its output (or the spawn error as text).
pub fn run(cmd: &str, args: &[&str]) -> Result<Output, String> {
    Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))
}

/// First line of `<tool> --version` (or `-v`, which poppler prints on stderr).
pub fn version(name: &str, flag: &str) -> String {
    match Command::new(name).arg(flag).output() {
        Ok(o) => {
            let s = if o.stdout.is_empty() { o.stderr } else { o.stdout };
            String::from_utf8_lossy(&s).lines().next().unwrap_or("").trim().to_string()
        }
        Err(_) => "NOT INSTALLED".into(),
    }
}

/// Render an SVG at `w × h` with librsvg; returns straight RGBA8.
pub fn rsvg(svg: &Path, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let png = svg.with_extension("rsvg.png");
    let o = Command::new("rsvg-convert")
        .args(["-w", &w.to_string(), "-h", &h.to_string(), "-o"])
        .arg(&png)
        .arg(svg)
        .output()
        .map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("rsvg-convert failed: {}", String::from_utf8_lossy(&o.stderr)));
    }
    decode(&png, w, h)
}

/// Render a PDF with poppler's Cairo backend, one document pixel per output pixel.
pub fn pdftocairo(pdf: &Path, dpi: f32, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let prefix = pdf.with_extension("cairo");
    let o = Command::new("pdftocairo")
        .args(["-png", "-transp", "-singlefile", "-r", &format!("{dpi}")])
        .args(["-scale-to-x", &w.to_string(), "-scale-to-y", &h.to_string()])
        .arg(pdf)
        .arg(&prefix)
        .output()
        .map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("pdftocairo failed: {}", String::from_utf8_lossy(&o.stderr)));
    }
    decode(&PathBuf::from(format!("{}.png", prefix.display())), w, h)
}

/// `qpdf --check` result.
pub fn qpdf_check(pdf: &Path) -> Result<bool, String> {
    let o = Command::new("qpdf").arg("--check").arg(pdf).output().map_err(|e| e.to_string())?;
    Ok(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("No syntax or stream encoding errors"))
}

fn decode(png: &Path, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(png).map_err(|e| e.to_string())?;
    let (rw, rh, px) = veskra_testkit::decode_png_rgba8(&bytes)?;
    if (rw, rh) != (w, h) {
        return Err(format!("render size {rw}x{rh}, expected {w}x{h}"));
    }
    Ok(px)
}
