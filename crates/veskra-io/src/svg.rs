//! SVG 1.1 "Export for Illustrator" writer (FMT-13, ADR-008, ADR-012). Not a native `.ai` file.
//!
//! # Output
//! * `<svg version="1.1" width="{w}px" height="{h}px" viewBox="0 0 {w} {h}">`: one user unit =
//!   one document pixel. Namespaces: SVG, `xlink` (for `xlink:href`), `inkscape` (layer
//!   `groupmode`/`label`). No DOCTYPE, scripts, event attributes, external references or
//!   timestamps; output is byte-deterministic for a given document/options/strategy.
//! * Visible stack in paint order (see [`crate::plan`]):
//!   * raster runs → `<g id="original-appearance" inkscape:groupmode="layer" ...>` holding an
//!     `<image>` (PNG `data:` URI, straight-alpha 8-bit sRGB, `iCCP` sRGB profile when
//!     `embed_color_profile`, `preserveAspectRatio="none"`, exact x/y/width/height in document px).
//!     When "Preserve original appearance" is off the group is `raster-only-content` /
//!     "Raster-only content" and holds only content without a vector form.
//!   * visible vector layers → `<g id="editable-vectors" ...>` with one subgroup per layer.
//! * Hidden edit resources → `<g id="editable-vectors" display="none">` (or
//!   `editable-vectors-hidden` when visible vectors exist), document group structure kept.
//! * Each vector stroke is `<path d=… fill="#RRGGBB" fill-opacity=… fill-rule="nonzero"/>` of its
//!   variable-width outline; layer opacity → `opacity`, non-Normal blend →
//!   `style="mix-blend-mode:…"`. Colors are sRGB (converted with `veskra-color` if the document
//!   profile is not sRGB; warned).
//! * SVG has no portable whole-document ICC semantics: with `embed_color_profile` only the
//!   embedded PNGs carry the profile; vector colors are plain sRGB values (MASTER_PLAN §2.3).
//! * `profile = display_p3 | adobe_rgb`: the embedded PNGs are converted to and tagged with that
//!   profile (relative colorimetric); path fills **stay sRGB values** because SVG 1.1 cannot attach
//!   an ICC profile to a fill portably. This is reported (`VectorColorsSrgb` warning, profile
//!   label). `bit_depth = 16` is not available: images are 8-bit (`BitDepthReduced` warning).

mod png;
pub(crate) mod xml;

use crate::plan::{ColorOut, HybridStrategy, IllustratorPlan, VecNode, VisibleItem, render_run, stroke_alpha};
use crate::{CancelToken, Exporter};
use std::io::Write;
use veskra_core::export::{ExportFormat, ExportOptions, ExportPreflight, ExportReport, ValidationStatus};
use veskra_core::{BlendMode, ErrorCode, LayerId, VeskraError};
use veskra_doc::Document;
use veskra_vector::num::fmt_num;

/// Decimal places for path coordinates (document px): 0.0005 px max rounding error.
pub const PATH_DECIMALS: u8 = 3;

pub const SVG_NS: &str = "http://www.w3.org/2000/svg";
pub const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
pub const INKSCAPE_NS: &str = "http://www.inkscape.org/namespaces/inkscape";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SvgExporter {
    pub strategy: HybridStrategy,
}

impl SvgExporter {
    pub fn new(strategy: HybridStrategy) -> Self {
        Self { strategy }
    }
}

/// Escape text for an XML attribute value or text node. Characters not allowed in XML 1.0
/// are replaced by U+FFFD.
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let b = [ch[0], ch.get(1).copied().unwrap_or(0), ch.get(2).copied().unwrap_or(0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if ch.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if ch.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn base64_decode_prefix(s: &str, max_out: usize) -> Vec<u8> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::new();
    for q in s.as_bytes().chunks_exact(4) {
        let (Some(a), Some(b), Some(c), Some(d)) = (val(q[0]), val(q[1]), val(q[2]), val(q[3])) else { break };
        let n = a << 18 | b << 12 | c << 6 | d;
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        if out.len() >= max_out {
            break;
        }
    }
    out
}

pub(crate) fn hex_color(rgb: [f32; 3]) -> String {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    format!("#{:02X}{:02X}{:02X}", q(rgb[0]), q(rgb[1]), q(rgb[2]))
}

fn num4(v: f32) -> String {
    fmt_num(v as f64, 4)
}

fn label_attrs(id: &str, label: &str) -> String {
    let l = escape_xml(label);
    format!(r#"id="{}" inkscape:groupmode="layer" inkscape:label="{l}" data-name="{l}""#, escape_xml(id))
}

fn opacity_blend_attrs(opacity: f32, blend: BlendMode) -> String {
    let mut s = String::new();
    if opacity < 1.0 {
        s.push_str(&format!(r#" opacity="{}""#, num4(opacity)));
    }
    if blend != BlendMode::Normal {
        s.push_str(&format!(r#" style="mix-blend-mode:{}""#, blend.css_name()));
    }
    s
}

struct Writer<'a> {
    doc: &'a Document,
    plan: &'a IllustratorPlan,
    color: ColorOut,
    out: String,
    paths: u32,
    images: u32,
}

impl Writer<'_> {
    fn layer_paths(&mut self, id: LayerId, cancel: &CancelToken) -> Result<(), VeskraError> {
        cancel.check()?;
        let Some(lp) = self.plan.get(id) else { return Ok(()) };
        for sid in &lp.vector_strokes {
            let Some(rec) = self.doc.stroke(*sid) else { continue };
            let Some(g) = rec.geometry.as_ref() else { continue };
            if g.outline.is_empty() {
                continue;
            }
            let hex = hex_color(self.color.srgb_color(rec.color));
            self.out.push_str(&format!(
                "<path d=\"{}\" fill=\"{hex}\" fill-opacity=\"{}\" fill-rule=\"nonzero\"/>\n",
                g.outline.to_svg_d(PATH_DECIMALS),
                num4(stroke_alpha(rec))
            ));
            self.paths += 1;
        }
        Ok(())
    }

    fn layer_group(
        &mut self,
        id: LayerId,
        opacity: f32,
        blend: BlendMode,
        cancel: &CancelToken,
    ) -> Result<(), VeskraError> {
        let name = self.plan.get(id).map(|l| l.name.clone()).unwrap_or_default();
        self.out.push_str(&format!(
            "<g {}{}>\n",
            label_attrs(&format!("layer-{id}"), &name),
            opacity_blend_attrs(opacity, blend)
        ));
        self.layer_paths(id, cancel)?;
        self.out.push_str("</g>\n");
        Ok(())
    }

    fn hidden_nodes(&mut self, nodes: &[VecNode], cancel: &CancelToken) -> Result<(), VeskraError> {
        for n in nodes {
            match n {
                VecNode::Group { id, name, opacity, blend, children } => {
                    self.out.push_str(&format!(
                        "<g {}{}>\n",
                        label_attrs(&format!("group-{id}"), name),
                        opacity_blend_attrs(*opacity, *blend)
                    ));
                    self.hidden_nodes(children, cancel)?;
                    self.out.push_str("</g>\n");
                }
                VecNode::Layer(id) => {
                    let (op, bl) = self.plan.get(*id).map(|l| (l.opacity, l.blend)).unwrap_or((1.0, BlendMode::Normal));
                    self.layer_group(*id, op, bl, cancel)?;
                }
            }
        }
        Ok(())
    }

    fn image(&mut self, k: usize, layers: &[LayerId], cancel: &CancelToken) -> Result<(), VeskraError> {
        let Some(img) = render_run(self.doc, layers, &self.color, cancel)? else { return Ok(()) };
        let target = self.color.target.clone();
        let icc = self.plan.options.embed_color_profile.then(|| (target.description(), target.bytes()));
        let png = png::encode_rgba8(img.rect.width(), img.rect.height(), &img.rgba, icc)?;
        let mut names: Vec<String> =
            layers.iter().take(8).map(|l| self.plan.get(*l).map(|p| p.name.clone()).unwrap_or_default()).collect();
        if layers.len() > 8 {
            names.push(format!("+{} more", layers.len() - 8));
        }
        let label = format!("Appearance: {}", names.join(", "));
        self.out.push_str(&format!(
            "<image id=\"appearance-{}\" inkscape:label=\"{l}\" data-name=\"{l}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" xlink:href=\"data:image/png;base64,{}\"/>\n",
            k + 1,
            img.rect.x0,
            img.rect.y0,
            img.rect.width(),
            img.rect.height(),
            base64(&png),
            l = escape_xml(&label),
        ));
        self.images += 1;
        Ok(())
    }
}

/// Serialize the plan as SVG. Returns (svg text, paths written, images written, color conversion state).
pub(crate) fn write_svg(
    doc: &Document,
    plan: &IllustratorPlan,
    cancel: &CancelToken,
) -> Result<(String, u32, u32, ColorOut), VeskraError> {
    let (w, h) = (doc.width(), doc.height());
    let mut wr = Writer { doc, plan, color: ColorOut::new(doc, plan), out: String::new(), paths: 0, images: 0 };
    wr.out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    wr.out.push_str("<!-- Exported by Veskra: Export for Illustrator (SVG 1.1). This is not a native Adobe Illustrator (.ai) file. -->\n");
    wr.out.push_str(&format!(
        "<svg xmlns=\"{SVG_NS}\" xmlns:xlink=\"{XLINK_NS}\" xmlns:inkscape=\"{INKSCAPE_NS}\" version=\"1.1\" width=\"{w}px\" height=\"{h}px\" viewBox=\"0 0 {w} {h}\" data-veskra-strategy=\"{}\" data-veskra-dpi=\"{}\">\n",
        plan.strategy.as_str(),
        fmt_num(doc.dpi as f64, 3)
    ));
    wr.out.push_str(&format!(
        "<desc>Veskra export. Canvas {w}x{h} px at {} dpi (1 SVG user unit = 1 document pixel). Profile: {}.</desc>\n",
        fmt_num(doc.dpi as f64, 3),
        escape_xml(&plan.profile_label(ExportFormat::Svg))
    ));
    let forced_label = !plan.options.include_raster_appearance;
    let (mut raster_groups, mut vector_groups) = (0usize, 0usize);
    let mut open: Option<bool> = None; // Some(true) = raster group open, Some(false) = vector group open
    for (k, item) in plan.items.iter().enumerate() {
        cancel.check()?;
        let is_raster = matches!(item, VisibleItem::Raster { .. });
        if open != Some(is_raster) {
            if open.is_some() {
                wr.out.push_str("</g>\n");
            }
            let (base_id, base_label, n) = if is_raster {
                raster_groups += 1;
                if forced_label {
                    ("raster-only-content", "Raster-only content", raster_groups)
                } else {
                    ("original-appearance", "Original appearance", raster_groups)
                }
            } else {
                vector_groups += 1;
                ("editable-vectors", "Editable vectors", vector_groups)
            };
            let (id, label) = if n == 1 {
                (base_id.to_string(), base_label.to_string())
            } else {
                (format!("{base_id}-{n}"), format!("{base_label} {n}"))
            };
            wr.out.push_str(&format!("<g {}>\n", label_attrs(&id, &label)));
            open = Some(is_raster);
        }
        match item {
            VisibleItem::Raster { layers, .. } => wr.image(k, layers, cancel)?,
            VisibleItem::Vectors { layer } => {
                let (op, bl) =
                    plan.get(*layer).map(|l| (l.effective_opacity, l.blend)).unwrap_or((1.0, BlendMode::Normal));
                wr.layer_group(*layer, op, bl, cancel)?;
            }
        }
    }
    if open.is_some() {
        wr.out.push_str("</g>\n");
    }
    if !plan.hidden_vectors.is_empty() {
        let (id, label) = if plan.has_visible_vectors() {
            ("editable-vectors-hidden", "Editable vectors (hidden edit resources)")
        } else {
            ("editable-vectors", "Editable vectors")
        };
        wr.out.push_str(&format!("<g {} display=\"none\">\n", label_attrs(id, label)));
        let nodes = plan.hidden_vectors.clone();
        wr.hidden_nodes(&nodes, cancel)?;
        wr.out.push_str("</g>\n");
    }
    wr.out.push_str("</svg>\n");
    Ok((wr.out, wr.paths, wr.images, wr.color))
}

impl Exporter for SvgExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Svg
    }

    fn preflight(&self, doc: &Document, opts: &ExportOptions) -> ExportPreflight {
        IllustratorPlan::build(doc, opts, self.strategy).to_preflight(doc, ExportFormat::Svg)
    }

    fn export(
        &self,
        doc: &Document,
        opts: &ExportOptions,
        out: &mut dyn Write,
        cancel: &CancelToken,
    ) -> Result<ExportReport, VeskraError> {
        let start = std::time::Instant::now();
        let plan = IllustratorPlan::build(doc, opts, self.strategy);
        if let Some(reason) = &plan.blocked_reason {
            return Err(VeskraError::new(ErrorCode::InvalidArgument, reason.clone()));
        }
        let (svg, paths, images, color) = write_svg(doc, &plan, cancel)?;
        let mut warnings = plan.warnings.clone();
        warnings.extend(plan.color_warnings(doc, ExportFormat::Svg));
        warnings.extend(color.clip_warnings());
        cancel.check()?;
        out.write_all(svg.as_bytes())
            .and_then(|_| out.flush())
            .map_err(|e| VeskraError::new(ErrorCode::Io, format!("could not write SVG: {}", e.kind())))?;
        Ok(ExportReport {
            success: false,
            file_uri: None,
            format: ExportFormat::Svg,
            profile: plan.profile_label(ExportFormat::Svg),
            profile_embedded: opts.embed_color_profile && images > 0,
            vector_object_count: paths,
            rasterized_object_count: images,
            flattened_feature_list: plan.flattened_features(),
            warnings,
            target_profile: color.target.id().semantic_name,
            bit_depth: 8,
            rendering_intent: color.intent(ExportFormat::Svg),
            out_of_gamut_pixels: color.clipped_pixels(),
            duration_ms: start.elapsed().as_millis() as u64,
            file_bytes: svg.len() as u64,
            width_px: doc.width(),
            height_px: doc.height(),
            validation: ValidationStatus::NotValidated,
        })
    }

    fn validate(&self, bytes: &[u8]) -> Result<(), VeskraError> {
        validate_svg(bytes).map_err(|m| VeskraError::new(ErrorCode::SvgEncodeError, format!("invalid SVG: {m}")))
    }
}

/// Structural validation of a Veskra SVG (independent of the writer's state).
pub fn validate_svg(bytes: &[u8]) -> Result<(), String> {
    let s = std::str::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    let d = xml::parse(s)?;
    let root = d.elements.first().ok_or("empty")?;
    if root.name != "svg" {
        return Err("root element is not <svg>".into());
    }
    if root.attr("xmlns") != Some(SVG_NS) {
        return Err("missing SVG namespace".into());
    }
    if root.attr("version") != Some("1.1") {
        return Err("version is not 1.1".into());
    }
    let dim = |n: &str| -> Result<f64, String> {
        root.attr(n)
            .and_then(|v| v.strip_suffix("px"))
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or(format!("bad {n}"))
    };
    let (w, h) = (dim("width")?, dim("height")?);
    let vb: Vec<f64> =
        root.attr("viewBox").ok_or("missing viewBox")?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    if vb != [0.0, 0.0, w, h] {
        return Err("viewBox does not match width/height".into());
    }
    let mut groups = 0;
    for i in d.descendants(0) {
        let Some(e) = d.elements.get(i) else { continue };
        let local = e.name.rsplit(':').next().unwrap_or("");
        if matches!(local, "script" | "foreignObject" | "handler" | "use") {
            return Err(format!("forbidden element <{}>", e.name));
        }
        for (k, v) in &e.attrs {
            let lk = k.rsplit(':').next().unwrap_or("");
            if lk.len() > 2 && lk.starts_with("on") {
                return Err(format!("event attribute '{k}'"));
            }
            if lk == "href" {
                let Some(b64) = v.strip_prefix("data:image/png;base64,") else {
                    return Err("external or non-PNG reference".into());
                };
                if base64_decode_prefix(b64, 8).get(..8) != Some(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..])
                {
                    return Err("embedded image is not PNG".into());
                }
            }
            if v.contains("url(") && !v.contains("url(#") {
                return Err("external url() reference".into());
            }
        }
        if e.name == "g" && root.children.contains(&i) {
            let id = e.attr("id").unwrap_or("");
            let known = ["original-appearance", "raster-only-content", "editable-vectors"];
            if !known.iter().any(|k| id.starts_with(k)) {
                return Err(format!("unexpected top-level group '{id}'"));
            }
            if e.attr("inkscape:groupmode") != Some("layer") || e.attr("inkscape:label").is_none() {
                return Err("top-level group without layer label".into());
            }
            groups += 1;
        }
        if e.name == "path" {
            if e.attr("d").is_none_or(|d| !d.starts_with('M')) {
                return Err("path without data".into());
            }
            if e.attr("fill-rule") != Some("nonzero") {
                return Err("path fill-rule is not nonzero".into());
            }
        }
        if e.name == "image" {
            for a in ["x", "y", "width", "height"] {
                if e.attr(a).and_then(|v| v.parse::<f64>().ok()).is_none() {
                    return Err(format!("image without numeric {a}"));
                }
            }
            if e.attr("preserveAspectRatio") != Some("none") {
                return Err("image preserveAspectRatio is not none".into());
            }
        }
    }
    if groups == 0 {
        return Err("no Veskra layer groups".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping() {
        assert_eq!(escape_xml("a&b<c>\"d'\u{1}\n"), "a&amp;b&lt;c&gt;&quot;d&apos;\u{FFFD}&#10;");
        let x = format!("<a n=\"{}\">{}</a>", escape_xml("\"]]>&<"), escape_xml("]]>"));
        assert!(xml::parse(&x).is_ok());
    }

    #[test]
    fn base64_known() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_decode_prefix("Zm9vYmFy", 100), b"foobar");
    }

    #[test]
    fn hex() {
        assert_eq!(hex_color([1.0, 0.5, 0.0]), "#FF8000");
        assert_eq!(hex_color([-1.0, 2.0, 0.2]), "#00FF33");
    }

    #[test]
    fn validator_rejects_unsafe() {
        let ok = format!(
            "<svg xmlns=\"{SVG_NS}\" xmlns:inkscape=\"{INKSCAPE_NS}\" version=\"1.1\" width=\"4px\" height=\"2px\" viewBox=\"0 0 4 2\"><g id=\"editable-vectors\" inkscape:groupmode=\"layer\" inkscape:label=\"E\"><path d=\"M0 0L1 1Z\" fill-rule=\"nonzero\"/></g></svg>"
        );
        assert!(validate_svg(ok.as_bytes()).is_ok());
        let bad = [
            ok.replace("<path", "<script/><path"),
            ok.replace("viewBox=\"0 0 4 2\"", "viewBox=\"0 0 4 3\""),
            ok.replace("fill-rule=\"nonzero\"", "fill-rule=\"evenodd\""),
            ok.replace("<path", "<path onclick=\"x\""),
            ok.replace("editable-vectors", "other"),
            ok.replace("</svg>", ""),
        ];
        for b in bad {
            assert!(validate_svg(b.as_bytes()).is_err(), "accepted {b}");
        }
    }
}
