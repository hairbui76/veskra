//! Infinite Painter brush import (`.prbr` properties + embedded PNGs → schema-3 preset).
//!
//! Independent implementation from publicly documented, reverse-engineered knowledge (see
//! `docs/interop/infinite-painter-brushes.md` for every field, its source and its certainty). The
//! archive layer (`.prbr` / `.przp` ZIP, `index.json`) lives in `veskra-engine`; this module only
//! sees `properties.json` and the optional `head` / `texture` PNG bytes.
//!
//! The result is an **approximation** of the Infinite Painter brush rendered by Veskra's own
//! engine. Every field that has no exact equivalent is named in
//! [`ImportedBrush::approximated`]; every active feature Veskra cannot reproduce is named in
//! [`ImportedBrush::unsupported`]. Built-in Infinite Painter heads/textures referenced by name are
//! **not** in the file and are never copied from the app: the brush falls back to a round tip /
//! procedural grain and says so.

use serde_json::{Map, Value};

use crate::asset::{BrushAsset, MaskChannel};
use crate::preset::{
    Accumulation, BrushCategory, BrushPreset, BrushSource, Dynamics, DynamicsSource, DynamicsTarget, Jitter,
    MAX_BRUSH_SIZE_PX, MAX_SCATTER, MAX_SPACING, MIN_SPACING, Texture, TextureMode, TextureSource, Tip, TipRotation,
    TipShape,
};
use crate::{BrushError, MAX_CURVE_POINTS, ResponseCurve};

/// `source` format id of imported Infinite Painter brushes (`imported:infinite_painter`).
pub const FORMAT_ID: &str = "infinite_painter";
/// Largest accepted `properties.json` / `index.json`.
pub const MAX_PROPERTIES_BYTES: usize = 1024 * 1024;
/// Most brushes accepted from one pack.
pub const MAX_PACK_BRUSHES: usize = 500;
/// Most folders accepted in one pack index.
pub const MAX_PACK_FOLDERS: usize = 100;
/// Observed size calibration: app size (px) ≈ 3.74 × `paint-size` × `size-maximum` (one device
/// reading, canvas-dependent; see the interop notes).
pub const SIZE_PX_PER_UNIT: f64 = 3.74;

/// The parts of one `.prbr` the mapper needs.
#[derive(Debug, Clone, Copy)]
pub struct PrbrParts<'a> {
    /// `properties.json` bytes.
    pub properties: &'a [u8],
    /// `head` entry (PNG), if present.
    pub head: Option<&'a [u8]>,
    /// `texture` entry (PNG), if present.
    pub texture: Option<&'a [u8]>,
    /// Display name fallback (file stem) when `display-name` is empty.
    pub file_name: &'a str,
}

/// Placeholder id of a freshly mapped preset; the brush library assigns the final id.
pub const PENDING_ID: &str = "imported.pending";

/// Result of [`map_prbr`]. `preset.id` is [`PENDING_ID`]: the library assigns the final id.
#[derive(Debug, Clone)]
pub struct ImportedBrush {
    pub preset: BrushPreset,
    /// Converted head / texture masks referenced by the preset.
    pub assets: Vec<BrushAsset>,
    /// Fields mapped with a known approximation (human-readable, stable wording).
    pub approximated: Vec<String>,
    /// Active features with no Veskra equivalent (dropped or replaced; named).
    pub unsupported: Vec<String>,
}

/// One folder of a `.przp` `index.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFolder {
    pub name: String,
    /// Brush file names (entry `Brushes/<name>.prbr`) and their `hidden` flag, in index order.
    pub brushes: Vec<(String, bool)>,
}

/// Parsed `index.json` of a `.przp` pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackIndex {
    pub folders: Vec<PackFolder>,
}

fn parse_err(m: impl Into<String>) -> BrushError {
    BrushError::Parse(m.into())
}

fn parse_json(bytes: &[u8], what: &str) -> Result<Map<String, Value>, BrushError> {
    if bytes.len() > MAX_PROPERTIES_BYTES {
        return Err(BrushError::TooLarge { limit: MAX_PROPERTIES_BYTES });
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|e| parse_err(format!("{what}: {e}")))?;
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(parse_err(format!("{what} is not a JSON object"))),
    }
}

/// Clean a display string: control characters dropped, trimmed, at most 255 bytes.
pub(crate) fn clean_name(s: &str) -> String {
    let mut out: String = s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
    while out.len() > veskra_core::limits::MAX_NAME_BYTES {
        out.pop();
    }
    out
}

/// Parse a pack's `index.json`.
pub fn parse_pack_index(bytes: &[u8]) -> Result<PackIndex, BrushError> {
    let m = parse_json(bytes, "index.json")?;
    let folders =
        m.get("brush-folders").and_then(Value::as_array).ok_or_else(|| parse_err("index.json has no brush-folders"))?;
    if folders.len() > MAX_PACK_FOLDERS {
        return Err(BrushError::TooLarge { limit: MAX_PACK_FOLDERS });
    }
    let mut out = Vec::new();
    let mut total = 0usize;
    for f in folders {
        let name = clean_name(f.get("name").and_then(Value::as_str).unwrap_or(""));
        let mut brushes = Vec::new();
        for b in f.get("brushes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(n) = b.get("name").and_then(Value::as_str) else { continue };
            total += 1;
            if total > MAX_PACK_BRUSHES {
                return Err(BrushError::TooLarge { limit: MAX_PACK_BRUSHES });
            }
            brushes.push((n.to_string(), b.get("hidden").and_then(Value::as_bool).unwrap_or(false)));
        }
        out.push(PackFolder { name, brushes });
    }
    Ok(PackIndex { folders: out })
}

struct Ctx {
    approximated: Vec<String>,
    unsupported: Vec<String>,
}

impl Ctx {
    fn approx(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.approximated.contains(&s) {
            self.approximated.push(s);
        }
    }
    fn unsupported(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.unsupported.contains(&s) {
            self.unsupported.push(s);
        }
    }
}

fn section<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a Map<String, Value>> {
    m.get(k).and_then(Value::as_object)
}

fn num(m: Option<&Map<String, Value>>, k: &str) -> Option<f64> {
    m.and_then(|m| m.get(k)).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn flag(m: Option<&Map<String, Value>>, k: &str) -> bool {
    m.and_then(|m| m.get(k)).and_then(Value::as_bool).unwrap_or(false)
}

fn text<'a>(m: Option<&'a Map<String, Value>>, k: &str) -> &'a str {
    m.and_then(|m| m.get(k)).and_then(Value::as_str).unwrap_or("")
}

/// Infinite Painter profile `[x0, y0, x1, y1, …]` (y inverted) → shape-preserving curve.
/// Returns the curve and whether it had to be approximated (resampled or extended).
pub fn ip_profile(v: Option<&Value>) -> Option<(ResponseCurve, bool)> {
    let a = v?.as_array()?;
    if a.len() < 4 || a.len() % 2 != 0 || a.len() > 2048 {
        return None;
    }
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(a.len() / 2);
    for p in a.chunks_exact(2) {
        let (x, y) = (p[0].as_f64()?, p[1].as_f64()?);
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        pts.push([x.clamp(0.0, 1.0) as f32, (1.0 - y).clamp(0.0, 1.0) as f32]);
    }
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    // Equal x: keep the later point.
    let mut dedup: Vec<[f32; 2]> = Vec::with_capacity(pts.len());
    for p in pts {
        match dedup.last_mut() {
            Some(l) if l[0] == p[0] => *l = p,
            _ => dedup.push(p),
        }
    }
    let mut approx = false;
    if let Some(&first) = dedup.first()
        && first[0] > 0.0
    {
        dedup.insert(0, [0.0, first[1]]);
        approx = true;
    }
    if let Some(&last) = dedup.last()
        && last[0] < 1.0
    {
        dedup.push([1.0, last[1]]);
        approx = true;
    }
    if dedup.len() < 2 {
        return None;
    }
    if dedup.len() > MAX_CURVE_POINTS {
        // Piecewise-linear resampling onto MAX_CURVE_POINTS evenly spaced inputs.
        let n = MAX_CURVE_POINTS;
        let src = dedup;
        dedup = (0..n)
            .map(|i| {
                let x = i as f32 / (n - 1) as f32;
                let k = src.iter().rposition(|p| p[0] <= x).unwrap_or(0).min(src.len() - 2);
                let (a, b) = (src[k], src[k + 1]);
                let t = if b[0] > a[0] { ((x - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0) } else { 0.0 };
                [x, (a[1] + (b[1] - a[1]) * t).clamp(0.0, 1.0)]
            })
            .collect();
        approx = true;
    }
    ResponseCurve::new_shape(dedup).ok().map(|c| (c, approx))
}

fn is_linear_profile(v: Option<&Value>) -> bool {
    match v.and_then(Value::as_array) {
        None => true,
        Some(a) => {
            let f: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            f == [0.0, 1.0, 1.0, 0.0]
        }
    }
}

/// Map one `.prbr` (properties + embedded PNGs) to a schema-3 preset. Fails only when
/// `properties.json` itself is unusable; problems with optional parts are reported and the
/// brush falls back (round tip, procedural grain).
pub fn map_prbr(parts: &PrbrParts<'_>, folder: &str) -> Result<ImportedBrush, BrushError> {
    let m = parse_json(parts.properties, "properties.json")?;
    let mut cx = Ctx { approximated: Vec::new(), unsupported: Vec::new() };
    let head = section(&m, "head-properties");
    let stroke = section(&m, "stroke-properties");
    let jit = section(&m, "jitter-properties");
    let tex = section(&m, "texture-properties");
    let src = section(&m, "source-properties");
    let dynp = section(&m, "dynamics-properties");
    let blend = section(&m, "blend-properties");

    let mut name = clean_name(m.get("display-name").and_then(Value::as_str).unwrap_or(""));
    if name.is_empty() {
        name = clean_name(parts.file_name);
    }
    if name.is_empty() {
        name = "Imported brush".into();
    }
    let mut assets = Vec::new();

    // ------------------------------------------------------------- stroke
    let paint_size = num(stroke, "paint-size").unwrap_or(10.0 / SIZE_PX_PER_UNIT);
    let size_max = num(stroke, "size-maximum").unwrap_or(1.0);
    let size_px = (paint_size * size_max * SIZE_PX_PER_UNIT).clamp(0.5, MAX_BRUSH_SIZE_PX as f64) as f32;
    cx.approx("size: paint-size × size-maximum × 3.74 px (single-device calibration)");
    let opacity = num(stroke, "paint-opacity").unwrap_or(1.0).clamp(0.0, 1.0) as f32;
    let stabilization = num(stroke, "lazy-stroke").unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    if stabilization > 0.0 {
        cx.approx("lazy stroke → Veskra stabilization (different smoothing algorithm)");
    }
    let wet_raw = num(stroke, "wet-edges").unwrap_or(0.0);
    let wet_edges = (wet_raw * 2.0).clamp(0.0, 1.0) as f32;
    if wet_edges > 0.0 {
        cx.approx("wet edges (Veskra wet-edge model; 0.5 in the file = 100 % in the app)");
    }
    if num(stroke, "blendmode").is_some_and(|b| b != 0.0) {
        cx.unsupported(format!("blend mode {}", num(stroke, "blendmode").unwrap_or(0.0)));
    }
    if num(stroke, "glaze-strokes").is_some_and(|v| v != 0.0) {
        cx.unsupported("glaze strokes");
    }
    if num(stroke, "adhesion").is_some_and(|v| v != 0.0) {
        cx.unsupported("adhesion");
    }
    if flag(stroke, "has-size-profile") {
        cx.unsupported("stroke size profile (taper along the stroke)");
    }
    if flag(stroke, "has-flow-profile") {
        cx.unsupported("stroke flow profile (fade along the stroke)");
    }

    // ------------------------------------------------------------- head
    let flow = num(head, "flow").unwrap_or(1.0).clamp(0.001, 1.0) as f32;
    let spacing = num(head, "spacing").unwrap_or(0.1);
    let spacing_c = spacing.clamp(MIN_SPACING as f64, MAX_SPACING as f64) as f32;
    if (spacing_c as f64 - spacing).abs() > 1e-6 {
        cx.approx("spacing clamped to Veskra's range");
    }
    let angle_rad = num(head, "angle").unwrap_or(0.0);
    let mut angle = (angle_rad * 180.0 / std::f64::consts::PI) % 360.0;
    let rot = num(head, "rotation").unwrap_or(0.0);
    let mut rotation = TipRotation::Fixed;
    if rot >= 0.5 {
        rotation = TipRotation::Direction;
        if rot < 1.0 {
            cx.approx(format!("head rotation {:.0} % treated as following the stroke", rot * 100.0));
        }
    } else if rot <= -0.5 {
        rotation = TipRotation::Direction;
        angle = (angle + 180.0) % 360.0;
        cx.approx("inverse head rotation treated as following the stroke + 180°");
    } else if rot != 0.0 {
        cx.approx(format!("head rotation {:.0} % treated as fixed", rot * 100.0));
    }
    if flag(head, "use-trajectory") {
        if rotation == TipRotation::Direction {
            cx.approx("pen rotation and stroke rotation both on: pen rotation used");
        }
        rotation = TipRotation::Pen;
    }
    if flag(head, "screen-aligned") {
        cx.unsupported("screen-aligned head");
    }
    for (k, label) in [("depth", "head depth"), ("structure", "head structure")] {
        if num(head, k).is_some_and(|v| v != 0.0) {
            cx.unsupported(label);
        }
    }
    if num(head, "style").is_some_and(|v| v != 0.0) {
        cx.unsupported(format!(
            "head style {} (meaning unknown; normal style used)",
            num(head, "style").unwrap_or(0.0)
        ));
    }
    let softness = num(head, "softness").unwrap_or(0.0).clamp(0.0, 1.0);
    let mut tip = Tip { hardness: (1.0 - softness) as f32, angle: angle as f32, rotation, ..Tip::default() };

    let builtin_head = text(src, "head");
    let custom_head = text(src, "custom-head");
    match parts.head {
        Some(png) => {
            let channel = match num(src, "head - conversion format") {
                Some(1.0) => MaskChannel::Darkness,
                Some(0.0) => MaskChannel::Luminance,
                _ => MaskChannel::Auto,
            };
            match BrushAsset::from_png(png, channel, false) {
                Ok(mut imp) => {
                    if let Some((w, h)) = imp.downscaled_from {
                        cx.approx(format!("head image downscaled from {w}×{h} px"));
                    }
                    let profile = src.and_then(|s| s.get("head - profile"));
                    if !is_linear_profile(profile) {
                        if let Some((c, _)) = ip_profile(profile) {
                            let data: Vec<u8> = imp
                                .asset
                                .mask()
                                .iter()
                                .map(|&v| (c.eval(v as f32 / 255.0) * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8)
                                .collect();
                            if let Ok(a) = BrushAsset::from_mask(imp.asset.width(), imp.asset.height(), data) {
                                imp.asset = a;
                            }
                            cx.approx("head profile applied to the head image");
                        } else {
                            cx.unsupported("head profile (unreadable)");
                        }
                    }
                    if flag(src, "head - color") {
                        cx.unsupported("colored head (used as a coverage mask)");
                    }
                    tip.shape = TipShape::Image;
                    tip.image = Some(imp.asset.hash().to_string());
                    assets.push(imp.asset);
                }
                Err(e) => cx.unsupported(format!("custom head image unreadable ({e}); round tip used")),
            }
        }
        None if !custom_head.is_empty() => {
            cx.unsupported("custom head image missing from the file; round tip used");
        }
        None if !builtin_head.is_empty() => {
            cx.unsupported(format!("built-in head \"{}\" replaced by round tip", clean_name(builtin_head)));
        }
        None => {}
    }
    if tip.shape == TipShape::Round && softness > 0.0 {
        cx.approx("round head softness → Veskra hardness falloff");
    }

    // ------------------------------------------------------------- texture
    let builtin_tex = text(src, "stroke texture");
    let custom_tex = text(src, "custom-stroke texture");
    let tex_scale = num(tex, "scale").unwrap_or(1.0).max(0.0);
    let depth = (1.0 - num(tex, "pressure").unwrap_or(0.5)).clamp(0.0, 1.0) as f32;
    let mode = match num(tex, "style").unwrap_or(0.0) {
        1.0 => TextureMode::Follow,
        0.0 => TextureMode::Fixed,
        s => {
            cx.unsupported(format!("texture style {s} (deform) — fixed texture used"));
            TextureMode::Fixed
        }
    };
    let invert = flag(tex, "invert");
    let mut texture: Option<Texture> = None;
    match parts.texture {
        Some(png) => match BrushAsset::from_png(png, MaskChannel::Luminance, false) {
            Ok(imp) => {
                if let Some((w, h)) = imp.downscaled_from {
                    cx.approx(format!("texture image downscaled from {w}×{h} px"));
                }
                let mut scale = tex_scale.max(0.05);
                if flag(tex, "scale-size") {
                    scale *= size_px as f64 / imp.asset.width().max(imp.asset.height()) as f64;
                    cx.approx("texture scale-size: texture sized once to the brush size");
                }
                cx.approx("texture: white = paint, scale relative to texture pixels (unverified)");
                if invert {
                    cx.approx("texture invert flag (app button state unverified)");
                }
                if flag(src, "stroke texture - color") {
                    cx.unsupported("colored texture (used as a grayscale mask)");
                }
                texture = Some(Texture {
                    source: TextureSource::Image,
                    image: Some(imp.asset.hash().to_string()),
                    scale: scale.clamp(0.05, 64.0) as f32,
                    depth,
                    invert,
                    mode,
                });
                assets.push(imp.asset);
            }
            Err(e) => cx.unsupported(format!("texture image unreadable ({e}); texture dropped")),
        },
        None if !custom_tex.is_empty() => cx.unsupported("custom texture image missing from the file; texture dropped"),
        None if !builtin_tex.is_empty() => {
            cx.unsupported(format!("built-in texture \"{}\" replaced by procedural grain", clean_name(builtin_tex)));
            texture = Some(Texture {
                source: TextureSource::Procedural,
                image: None,
                scale: (4.0 * tex_scale.max(0.2)).clamp(0.5, 256.0) as f32,
                depth,
                invert,
                mode,
            });
        }
        None => {}
    }
    if texture.is_some() {
        for (k, label) in
            [("softness", "texture softness"), ("structure", "texture structure"), ("angle", "texture angle")]
        {
            if num(tex, k).is_some_and(|v| v != 0.0) {
                cx.unsupported(label);
            }
        }
        if num(tex, "stretch").is_some_and(|v| v != 1.0) {
            cx.unsupported("texture stretch");
        }
    }

    // ------------------------------------------------------------- jitter
    let j01 = |k: &str| num(jit, k).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    let jitter = Jitter {
        size: j01("size"),
        angle: j01("angle"),
        opacity: 0.0,
        flow: j01("flow"),
        scatter: (num(jit, "scatter").unwrap_or(0.0)).clamp(0.0, MAX_SCATTER as f64) as f32,
        hue: j01("color-hue"),
        saturation: j01("color-saturation"),
        brightness: j01("color-brightness"),
        stroke_hue: j01("color-start hue"),
        stroke_saturation: j01("color-start saturation"),
        stroke_brightness: j01("color-start brightness"),
    };
    if !jitter.is_zero() {
        cx.approx("jitter: uniform random distribution and ranges are Veskra's");
    }
    if jitter.scatter > 0.0 {
        cx.approx("scatter jitter unit (× brush diameter) unverified");
    }
    for (k, label) in [
        ("start-angle", "initial angle jitter"),
        ("texture-angle", "texture angle jitter"),
        ("texture-position", "texture position jitter"),
        ("texture-scale", "texture scale jitter"),
        ("texture-start-position", "initial texture position jitter"),
    ] {
        if num(jit, k).is_some_and(|v| v != 0.0) {
            cx.unsupported(label);
        }
    }

    // ------------------------------------------------------------- dynamics
    let mut dynamics = Dynamics::default();
    let mut any_tilt = false;
    for (src_name, label) in [("pressure", "pressure"), ("tilt", "tilt"), ("velocity", "velocity")] {
        let mut set = DynamicsSource::default();
        for (target, tlabel) in [("size", "size"), ("flow", "flow"), ("texture", "texture"), ("scatter", "scatter")] {
            if !flag(dynp, &format!("{src_name} - effects {target}")) {
                continue;
            }
            let profile = dynp.and_then(|d| d.get(&format!("{src_name} profile - {target}")));
            let curve = match ip_profile(profile) {
                Some((c, approx)) => {
                    if approx {
                        cx.approx(format!("{label} → {tlabel} profile extended/resampled"));
                    }
                    c
                }
                None => {
                    cx.approx(format!("{label} → {tlabel} profile missing or unreadable: linear used"));
                    ResponseCurve::identity()
                }
            };
            let t = DynamicsTarget::on(curve);
            match target {
                "size" => set.size = t,
                "flow" => set.flow = t,
                "texture" => set.texture = t,
                _ => set.scatter = t,
            }
            if src_name == "tilt" {
                any_tilt = true;
            }
            if src_name == "velocity" {
                cx.approx(format!("velocity → {tlabel}: Veskra's speed scale (4 px/ms = full)"));
            }
        }
        for (target, tlabel) in [("dilution", "dilution"), ("head depth", "head depth")] {
            if flag(dynp, &format!("{src_name} - effects {target}")) {
                cx.unsupported(format!("{label} → {tlabel}"));
            }
        }
        match src_name {
            "pressure" => dynamics.pressure = set,
            "tilt" => dynamics.tilt = set,
            _ => dynamics.velocity = set,
        }
    }
    if any_tilt && (num(dynp, "tilt - offset").is_some_and(|v| v != 0.0) || num(dynp, "tilt - gradation").is_some()) {
        cx.approx("tilt offset / gradation ignored");
    }

    // ------------------------------------------------------------- paint mixing / special
    if num(blend, "mix-in").is_some_and(|v| v > 0.0) {
        cx.unsupported("paint mix-in");
    }
    if flag(blend, "mix-wet") {
        cx.unsupported("wet mix");
    }
    if flag(blend, "dirty-blending") {
        cx.unsupported("dirty blending");
    }
    for (k, label) in [
        ("bleed-properties", "watercolor bleed (bleed-properties)"),
        ("particles-properties", "particles (particles-properties)"),
        ("pixel-properties", "pixel / halftone mode (pixel-properties)"),
        ("filter-properties", "filters (filter-properties)"),
    ] {
        if m.contains_key(k) {
            cx.unsupported(label);
        }
    }

    let mut preset = BrushPreset {
        id: PENDING_ID.into(),
        name,
        category: BrushCategory::Imported,
        source: BrushSource::Imported(FORMAT_ID.into()),
        folder: clean_name(folder),
        size_px,
        opacity,
        flow,
        spacing: spacing_c,
        tip,
        jitter,
        texture,
        dynamics,
        wet_edges,
        accumulation: Accumulation::Max,
        eraser: false,
        vector_eligible: false,
        stabilization,
        ..BrushPreset::default()
    };
    preset.vector_eligible = preset.satisfies_vector_rule();
    Ok(ImportedBrush { preset, assets, approximated: cx.approximated, unsupported: cx.unsupported })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_invert_y_extend_and_resample() {
        let v: Value = serde_json::from_str("[0, 1, 0.33, 0.34, 1, 0]").unwrap();
        let (c, approx) = ip_profile(Some(&v)).unwrap();
        assert!(!approx);
        assert_eq!(c.points(), &[[0.0, 0.0], [0.33, 0.66], [1.0, 1.0]]);
        let v: Value = serde_json::from_str("[0.2, 0, 0.4, 0.1, 0.6, 0.9, 0.8, 1]").unwrap();
        let (c, approx) = ip_profile(Some(&v)).unwrap();
        assert!(approx);
        assert_eq!(c.points().first(), Some(&[0.0, 1.0]));
        assert_eq!(c.points().last(), Some(&[1.0, 0.0]));
        let many: Vec<f64> = (0..100).flat_map(|i| [i as f64 / 99.0, 0.5]).collect();
        let (c, approx) = ip_profile(Some(&serde_json::to_value(many).unwrap())).unwrap();
        assert!(approx && c.points().len() == MAX_CURVE_POINTS);
        for bad in ["[]", "[0,1,1]", "[\"a\",1,1,0]", "{}"] {
            assert!(ip_profile(Some(&serde_json::from_str(bad).unwrap())).is_none(), "{bad}");
        }
    }

    #[test]
    fn index_bounds() {
        let ok = br#"{"version":1,"brush-folders":[{"version":3,"brushes":[{"name":"A","id":0,"hidden":false},{"name":"B","hidden":true}],"selected":0,"name":"Pack"}]}"#;
        let idx = parse_pack_index(ok).unwrap();
        assert_eq!(idx.folders[0].name, "Pack");
        assert_eq!(idx.folders[0].brushes, vec![("A".to_string(), false), ("B".to_string(), true)]);
        let many: Vec<String> = (0..501).map(|i| format!("{{\"name\":\"b{i}\"}}")).collect();
        let big = format!("{{\"brush-folders\":[{{\"name\":\"x\",\"brushes\":[{}]}}]}}", many.join(","));
        assert!(matches!(parse_pack_index(big.as_bytes()), Err(BrushError::TooLarge { .. })));
        assert!(parse_pack_index(b"[1]").is_err());
        assert!(parse_pack_index(b"{\"x\":1}").is_err());
    }
}
