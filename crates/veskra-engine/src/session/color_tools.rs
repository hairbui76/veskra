//! Read-only color commands for the CMYK color picker and profile import (ADR-018):
//! `color.convert` (sRGB ↔ CMYK) and `profile.inspect`. Neither needs an open document, neither
//! records an undo step or changes the document version.
//!
//! The source / target RGB space is sRGB (the v1 working space). Transforms are cached per
//! session by (profile path, intent, black point compensation) and revalidated against the file's
//! length and modification time, so repeated calls while a slider is dragged only pay the
//! conversion itself (a few microseconds; measured in `tests/color_convert.rs`).

use super::{EResult, Receipt, Session, internal};
use crate::command::ColorTargetSpec;
use serde_json::json;
use std::sync::Arc;
use std::time::SystemTime;
use veskra_color::cmyk::{CmykTransform, GAMUT_DELTA_E00};
use veskra_color::{IccProfile, RenderingIntent};
use veskra_core::VeskraError;
use veskra_core::export::{ExportIntent, MAX_PROFILE_PATH_BYTES};

/// Transforms kept per session.
const CACHE_ENTRIES: usize = 4;

/// File identity used to notice a replaced profile file.
type Stamp = Option<(u64, Option<SystemTime>)>;

struct Entry {
    path: Option<String>,
    intent: RenderingIntent,
    bpc: bool,
    stamp: Stamp,
    transform: Arc<CmykTransform>,
}

/// Per-session cache of sRGB ↔ CMYK transforms for `color.convert`.
#[derive(Default)]
pub(super) struct CmykCache {
    entries: Vec<Entry>,
}

impl std::fmt::Debug for CmykCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CmykCache").field("entries", &self.entries.len()).finish()
    }
}

fn stamp(path: Option<&str>) -> Stamp {
    let m = std::fs::metadata(path?).ok()?;
    Some((m.len(), m.modified().ok()))
}

impl CmykCache {
    fn get(&mut self, path: Option<&str>, intent: RenderingIntent, bpc: bool) -> EResult<Arc<CmykTransform>> {
        let st = stamp(path);
        let same = |e: &Entry| e.path.as_deref() == path && e.intent == intent && e.bpc == bpc;
        if let Some(e) = self.entries.iter().find(|e| same(e) && e.stamp == st && (path.is_none() || st.is_some())) {
            return Ok(e.transform.clone());
        }
        let profile = veskra_io::cmyk::load_cmyk_profile(path)?;
        let transform = veskra_io::cmyk::cmyk_transform(&IccProfile::srgb(), &profile, intent, bpc)?;
        self.entries.retain(|e| !same(e));
        if self.entries.len() >= CACHE_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push(Entry {
            path: path.map(str::to_string),
            intent,
            bpc,
            stamp: st,
            transform: transform.clone(),
        });
        Ok(transform)
    }
}

/// `#RRGGBB` (or `#RRGGBBAA`, alpha ignored) → exact 8-bit components.
fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn check_profile_path(p: &Option<String>) -> EResult<()> {
    match p {
        Some(p) if p.trim().is_empty() => {
            Err(VeskraError::invalid("cmykProfile must be the path of an ICC file, or null for the bundled profile"))
        }
        Some(p) if p.len() > MAX_PROFILE_PATH_BYTES => Err(VeskraError::invalid("cmykProfile path is too long")),
        _ => Ok(()),
    }
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn color_convert(
        &mut self,
        hex: Option<String>,
        cmyk: Option<[f64; 4]>,
        to: ColorTargetSpec,
        cmyk_profile: Option<String>,
        intent: ExportIntent,
        bpc: bool,
    ) -> EResult<Receipt> {
        check_profile_path(&cmyk_profile)?;
        let intent = veskra_io::cmyk::rendering_intent(intent);
        match to {
            ColorTargetSpec::Cmyk => {
                if cmyk.is_some() {
                    return Err(VeskraError::invalid("color.convert to \"cmyk\" takes `hex`, not `cmyk`"));
                }
                let hex = hex.ok_or_else(|| VeskraError::invalid("color.convert to \"cmyk\" needs `hex`"))?;
                let rgb = parse_hex(&hex).ok_or_else(|| VeskraError::invalid("hex must be #RRGGBB"))?;
                let t = self.cmyk_cache.get(cmyk_profile.as_deref(), intent, bpc)?;
                let (k, de) = t.rgb_to_cmyk_with_delta_e(rgb.map(|v| v as f64 / 255.0));
                Ok(self.receipt(json!({
                    "cmyk": k.map(round4),
                    "inGamut": de <= GAMUT_DELTA_E00,
                    "profile": profile_name(&t),
                })))
            }
            ColorTargetSpec::Srgb => {
                if hex.is_some() {
                    return Err(VeskraError::invalid("color.convert to \"srgb\" takes `cmyk`, not `hex`"));
                }
                let k = cmyk.ok_or_else(|| VeskraError::invalid("color.convert to \"srgb\" needs `cmyk`"))?;
                if !k.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
                    return Err(VeskraError::invalid("cmyk components must be numbers in 0..=1"));
                }
                let t = self.cmyk_cache.get(cmyk_profile.as_deref(), intent, bpc)?;
                let (rgb, clipped) = t.cmyk_to_rgb(k);
                let [r, g, b] = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8);
                Ok(self.receipt(json!({
                    "hex": format!("#{r:02X}{g:02X}{b:02X}"),
                    "clipped": clipped,
                    "profile": profile_name(&t),
                })))
            }
        }
    }

    pub(super) fn profile_inspect(&self, path: Option<String>) -> EResult<Receipt> {
        if let Some(p) = &path
            && (p.trim().is_empty() || p.len() > MAX_PROFILE_PATH_BYTES)
        {
            return Err(VeskraError::invalid("path must be the path of an ICC file, or null for the bundled profile"));
        }
        let info = veskra_io::cmyk::inspect_profile(path.as_deref())?;
        Ok(self.receipt(serde_json::to_value(info).map_err(internal)?))
    }
}

fn profile_name(t: &CmykTransform) -> String {
    let d = t.profile().description();
    if d.is_empty() { "Unnamed CMYK profile".to_string() } else { d.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("#ff800080"), Some([255, 128, 0]));
        for bad in ["FF8000", "#FF80", "#GG0000", "#ＦＦ0000", ""] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }
}
