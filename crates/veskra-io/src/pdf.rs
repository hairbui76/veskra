//! PDF 1.7 "Export for Illustrator" writer (FMT-14, ADR-008, ADR-012). Hand-written, no
//! third-party PDF library. Not a native `.ai` file.
//!
//! # Page geometry (ILL-007)
//! One page; artboard = canvas. `MediaBox = [0 0 W·72/dpi H·72/dpi]` (points), so the physical
//! size of the canvas is preserved (2048 px at 300 dpi → 491.52 pt = 6.827 in). Vector content
//! is drawn under `s 0 0 -s 0 H cm` (`s = 72/dpi`, `H` = page height in points), so path
//! coordinates are **document pixels, origin top-left, y down**, exactly the document's own
//! convention. Images are placed directly in page space with the conventional upright matrix
//! `w·s 0 0 h·s x·s H−(y+h)·s cm` (no mirrored image matrices).
//!
//! # Content
//! * Raster runs: Image XObjects, 8-bit RGB, FlateDecode, straight color with an 8-bit
//!   `/SMask` (DeviceGray) when not fully opaque; `/ColorSpace` = `[/ICCBased sRGB]` when
//!   `embed_color_profile`, else `/DeviceRGB`.
//! * Vector strokes: filled outline paths (`f`, nonzero), color in `/CS0` (ICCBased sRGB) or
//!   `rg` (DeviceRGB); stroke alpha via ExtGState `/ca`. A layer with opacity < 1 or a non-Normal
//!   blend is drawn as an isolated transparency-group Form XObject painted with `/ca` + `/BM`
//!   (same group-opacity semantics as the compositor and SVG `opacity`).
//! * Optional content: OCGs "Original appearance" (or "Raster-only content"), "Editable vectors"
//!   and, when visible vectors also exist, "Editable vectors (hidden edit resources)"; one nested
//!   OCG per vector layer (named after the layer). `/OCProperties /D` lists `/Order`, `/ON` and
//!   `/OFF` (hidden per strategy); content is wrapped in `/OC /ocN BDC … EMC`.
//! * All streams Flate-compressed; deterministic (no dates, no `/ID`); classic xref table.
//! * `profile = display_p3 | adobe_rgb`: the single `/ICCBased` color space holds that profile and
//!   **both** image samples and vector fill colors are converted to it (relative colorimetric), so
//!   the whole page is color-managed in the target space. `bit_depth = 16` is not available:
//!   images are 8-bit (`BitDepthReduced` warning).

mod validate;

use crate::plan::{ColorOut, HybridStrategy, IllustratorPlan, VecNode, VisibleItem, render_run, stroke_alpha};
use crate::{CancelToken, Exporter};
use std::collections::BTreeMap;
use std::io::Write;
use veskra_core::export::{ExportFormat, ExportOptions, ExportPreflight, ExportReport, ValidationStatus};
use veskra_core::{BlendMode, ErrorCode, LayerId, RectF, VeskraError};
use veskra_doc::Document;
use veskra_vector::num::fmt_num;

pub use validate::{PdfSummary, validate_pdf};

/// Decimal places for path coordinates (document px).
pub const PATH_DECIMALS: u8 = 3;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PdfExporter {
    pub strategy: HybridStrategy,
}

impl PdfExporter {
    pub fn new(strategy: HybridStrategy) -> Self {
        Self { strategy }
    }
}

/// PDF text string: literal for printable ASCII, UTF-16BE hex with BOM otherwise.
pub fn pdf_text_string(s: &str) -> String {
    if s.chars().all(|c| (' '..='~').contains(&c)) {
        let mut out = String::from("(");
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        out
    } else {
        let mut out = String::from("<FEFF");
        for u in s.encode_utf16() {
            out.push_str(&format!("{u:04X}"));
        }
        out.push('>');
        out
    }
}

fn n6(v: f64) -> String {
    fmt_num(v, 6)
}

fn flate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// Stream object body: `<< dict /Length n /Filter /FlateDecode >> stream … endstream`.
fn stream_obj(dict: &str, data: &[u8]) -> Vec<u8> {
    let z = flate(data);
    let mut v = format!("<< {dict} /Filter /FlateDecode /Length {} >>\nstream\n", z.len()).into_bytes();
    v.extend_from_slice(&z);
    v.extend_from_slice(b"\nendstream");
    v
}

#[derive(Default)]
struct Objects {
    bodies: Vec<Option<Vec<u8>>>,
}

impl Objects {
    fn alloc(&mut self) -> u32 {
        self.bodies.push(None);
        self.bodies.len() as u32
    }
    fn set(&mut self, n: u32, body: Vec<u8>) {
        if let Some(slot) = self.bodies.get_mut(n as usize - 1) {
            *slot = Some(body);
        }
    }
    fn add(&mut self, body: Vec<u8>) -> u32 {
        let n = self.alloc();
        self.set(n, body);
        n
    }
    fn finish(self, root: u32, info: u32) -> Result<Vec<u8>, VeskraError> {
        let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::with_capacity(self.bodies.len());
        for (i, b) in self.bodies.into_iter().enumerate() {
            let body = b.ok_or_else(|| VeskraError::new(ErrorCode::PdfEncodeError, "unset PDF object"))?;
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(&body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        Ok(out)
    }
}

struct PdfWriter<'a> {
    doc: &'a Document,
    plan: &'a IllustratorPlan,
    color: ColorOut,
    objs: Objects,
    /// Color space object (`[/ICCBased n 0 R]`) when embedding.
    cs: Option<u32>,
    /// (ca, BM) → (resource name, object number).
    gstates: BTreeMap<(String, &'static str), (String, u32)>,
    xobjects: Vec<(String, u32)>,
    /// OCG resource name → object number.
    props: Vec<(String, u32)>,
    paths: u32,
    images: u32,
    form_resources: u32,
    /// Points per document pixel and page height in points.
    scale: f64,
    page_h: f64,
}

impl PdfWriter<'_> {
    fn gs(&mut self, alpha: f32, blend: BlendMode) -> String {
        let ca = fmt_num(alpha.clamp(0.0, 1.0) as f64, 4);
        let key = (ca.clone(), blend.pdf_name());
        if let Some((name, _)) = self.gstates.get(&key) {
            return name.clone();
        }
        let name = format!("GS{}", self.gstates.len());
        let n =
            self.objs.add(format!("<< /Type /ExtGState /ca {ca} /CA {ca} /BM /{} >>", blend.pdf_name()).into_bytes());
        self.gstates.insert(key, (name.clone(), n));
        name
    }

    fn ocg(&mut self, name: &str) -> (String, u32) {
        let n = self.objs.add(format!("<< /Type /OCG /Name {} >>", pdf_text_string(name)).into_bytes());
        let res = format!("oc{}", self.props.len());
        self.props.push((res.clone(), n));
        (res, n)
    }

    /// Path-painting operators for a layer's vector strokes (document px coordinates).
    fn layer_paths(&mut self, id: LayerId, cancel: &CancelToken) -> Result<(String, RectF), VeskraError> {
        cancel.check()?;
        let mut s = String::new();
        let mut bounds = RectF::new(0.0, 0.0, 0.0, 0.0);
        let mut first = true;
        let Some(lp) = self.plan.get(id) else { return Ok((s, bounds)) };
        let strokes = lp.vector_strokes.clone();
        for sid in strokes {
            let Some(rec) = self.doc.stroke(sid) else { continue };
            let Some(g) = rec.geometry.as_ref() else { continue };
            if g.outline.is_empty() {
                continue;
            }
            let b = g.outline.bounds();
            bounds = if first { b } else { bounds.union(&b) };
            first = false;
            let [r, gg, bb] = self.color.target_color(rec.color);
            let alpha = stroke_alpha(rec);
            s.push_str("q\n");
            if alpha < 1.0 {
                let name = self.gs(alpha, BlendMode::Normal);
                s.push_str(&format!("/{name} gs\n"));
            }
            let (r, gg, bb) = (fmt_num(r as f64, 4), fmt_num(gg as f64, 4), fmt_num(bb as f64, 4));
            if self.cs.is_some() {
                s.push_str(&format!("/CS0 cs {r} {gg} {bb} scn\n"));
            } else {
                s.push_str(&format!("{r} {gg} {bb} rg\n"));
            }
            s.push_str(&g.outline.to_pdf_ops(PATH_DECIMALS));
            s.push_str("f\nQ\n");
            self.paths += 1;
        }
        Ok((s, bounds))
    }

    /// Content for one vector layer: inline, or a transparency-group form for opacity/blend.
    fn vector_layer(
        &mut self,
        id: LayerId,
        opacity: f32,
        blend: BlendMode,
        cancel: &CancelToken,
    ) -> Result<String, VeskraError> {
        let name = self.plan.get(id).map(|l| l.name.clone()).unwrap_or_default();
        let (oc, _) = self.ocg(if name.is_empty() { "(unnamed layer)" } else { &name });
        let (ops, b) = self.layer_paths(id, cancel)?;
        let mut s = format!("/OC /{oc} BDC\n");
        if opacity >= 1.0 && blend == BlendMode::Normal {
            s.push_str(&ops);
        } else {
            let gs = self.gs(opacity, blend);
            let group_cs = match self.cs {
                Some(c) => format!("{c} 0 R"),
                None => "/DeviceRGB".into(),
            };
            let bbox = b.inflate(1.0);
            let dict = format!(
                "/Type /XObject /Subtype /Form /BBox [{} {} {} {}] /Group << /S /Transparency /CS {group_cs} /I true /K false >> /Resources {} 0 R",
                n6(bbox.x0.floor()),
                n6(bbox.y0.floor()),
                n6(bbox.x1.ceil()),
                n6(bbox.y1.ceil()),
                self.form_resources
            );
            let n = self.objs.add(stream_obj(&dict, ops.as_bytes()));
            let xname = format!("Fm{}", self.xobjects.len());
            self.xobjects.push((xname.clone(), n));
            s.push_str(&format!("q /{gs} gs /{xname} Do Q\n"));
        }
        s.push_str("EMC\n");
        Ok(s)
    }

    fn image(&mut self, layers: &[LayerId], cancel: &CancelToken) -> Result<String, VeskraError> {
        let Some(img) = render_run(self.doc, layers, &self.color, cancel)? else { return Ok(String::new()) };
        let (w, h) = (img.rect.width(), img.rect.height());
        let mut rgb = Vec::with_capacity(img.rgba.len() / 4 * 3);
        let mut alpha = Vec::with_capacity(img.rgba.len() / 4);
        for p in img.rgba.chunks_exact(4) {
            rgb.extend_from_slice(&p[..3]);
            alpha.push(p[3]);
        }
        cancel.check()?;
        let smask = if img.is_opaque() {
            String::new()
        } else {
            let d = format!(
                "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8"
            );
            format!(" /SMask {} 0 R", self.objs.add(stream_obj(&d, &alpha)))
        };
        let cs = match self.cs {
            Some(c) => format!("{c} 0 R"),
            None => "/DeviceRGB".into(),
        };
        let d = format!(
            "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent 8{smask}"
        );
        let n = self.objs.add(stream_obj(&d, &rgb));
        let name = format!("Im{}", self.xobjects.len());
        self.xobjects.push((name.clone(), n));
        self.images += 1;
        let (s, ph) = (self.scale, self.page_h);
        Ok(format!(
            "q {} 0 0 {} {} {} cm /{name} Do Q\n",
            n6(w as f64 * s),
            n6(h as f64 * s),
            n6(img.rect.x0 as f64 * s),
            n6(ph - img.rect.y1 as f64 * s)
        ))
    }

    fn hidden_layers(&self, nodes: &[VecNode], out: &mut Vec<LayerId>) {
        for n in nodes {
            match n {
                VecNode::Group { children, .. } => self.hidden_layers(children, out),
                VecNode::Layer(id) => out.push(*id),
            }
        }
    }
}

/// Serialize the plan as PDF. Returns (bytes, paths written, images written, color conversion state).
pub(crate) fn write_pdf(
    doc: &Document,
    plan: &IllustratorPlan,
    cancel: &CancelToken,
) -> Result<(Vec<u8>, u32, u32, ColorOut), VeskraError> {
    let mut w = PdfWriter {
        doc,
        plan,
        color: ColorOut::new(doc, plan),
        objs: Objects::default(),
        cs: None,
        gstates: BTreeMap::new(),
        xobjects: Vec::new(),
        props: Vec::new(),
        paths: 0,
        images: 0,
        form_resources: 0,
        scale: 1.0,
        page_h: 0.0,
    };
    let catalog = w.objs.alloc();
    let pages = w.objs.alloc();
    let page = w.objs.alloc();
    let info = w.objs.alloc();
    let page_resources = w.objs.alloc();
    w.form_resources = w.objs.alloc();
    if plan.options.embed_color_profile {
        let target = w.color.target.clone();
        let icc = w.objs.add(stream_obj("/N 3 /Alternate /DeviceRGB", target.bytes()));
        w.cs = Some(w.objs.add(format!("[/ICCBased {icc} 0 R]").into_bytes()));
    }

    let s = 72.0 / doc.dpi as f64;
    let (pw, ph) = (doc.width() as f64 * s, doc.height() as f64 * s);
    w.scale = s;
    w.page_h = ph;
    let flip = format!("{} 0 0 {} 0 {} cm\n", n6(s), n6(-s), n6(ph));
    let mut content = String::from("q\n");

    // Top-level OCGs, created on demand, in /Order sequence.
    let raster_label =
        if plan.options.include_raster_appearance { "Original appearance" } else { "Raster-only content" };
    let mut order: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut off: Vec<u32> = Vec::new();
    let mut app_oc: Option<(String, u32)> = None;
    let mut vec_oc: Option<(String, u32)> = None;
    for item in &plan.items {
        cancel.check()?;
        match item {
            VisibleItem::Raster { layers, .. } => {
                let (res, _) = match &app_oc {
                    Some(x) => x.clone(),
                    None => {
                        let x = w.ocg(raster_label);
                        order.push((x.1, Vec::new()));
                        app_oc = Some(x.clone());
                        x
                    }
                };
                let body = w.image(layers, cancel)?;
                if !body.is_empty() {
                    content.push_str(&format!("/OC /{res} BDC\n{body}EMC\n"));
                }
            }
            VisibleItem::Vectors { layer } => {
                let (res, n) = match &vec_oc {
                    Some(x) => x.clone(),
                    None => {
                        let x = w.ocg("Editable vectors");
                        order.push((x.1, Vec::new()));
                        vec_oc = Some(x.clone());
                        x
                    }
                };
                let (op, bl) =
                    plan.get(*layer).map(|l| (l.effective_opacity, l.blend)).unwrap_or((1.0, BlendMode::Normal));
                let body = w.vector_layer(*layer, op, bl, cancel)?;
                if let (Some(last), Some(entry)) = (w.props.last(), order.iter_mut().find(|(o, _)| *o == n)) {
                    entry.1.push(last.1);
                }
                content.push_str(&format!("/OC /{res} BDC\nq\n{flip}{body}Q\nEMC\n"));
            }
        }
    }
    let mut hidden_ids = Vec::new();
    w.hidden_layers(&plan.hidden_vectors, &mut hidden_ids);
    if !hidden_ids.is_empty() {
        let label =
            if plan.has_visible_vectors() { "Editable vectors (hidden edit resources)" } else { "Editable vectors" };
        let (res, n) = w.ocg(label);
        off.push(n);
        let mut kids = Vec::new();
        let mut body = String::new();
        for id in hidden_ids {
            let (op, bl) = plan.get(id).map(|l| (l.opacity, l.blend)).unwrap_or((1.0, BlendMode::Normal));
            body.push_str(&w.vector_layer(id, op, bl, cancel)?);
            if let Some(last) = w.props.last() {
                kids.push(last.1);
            }
        }
        order.push((n, kids));
        content.push_str(&format!("/OC /{res} BDC\nq\n{flip}{body}Q\nEMC\n"));
    }
    content.push_str("Q\n");
    cancel.check()?;

    // Resources.
    let cs_entry = w.cs.map(|c| format!(" /ColorSpace << /CS0 {c} 0 R >>")).unwrap_or_default();
    let gs_entry = if w.gstates.is_empty() {
        String::new()
    } else {
        let mut v: Vec<&(String, u32)> = w.gstates.values().collect();
        v.sort();
        format!(" /ExtGState << {} >>", v.iter().map(|(n, o)| format!("/{n} {o} 0 R")).collect::<Vec<_>>().join(" "))
    };
    let xo_entry = if w.xobjects.is_empty() {
        String::new()
    } else {
        format!(
            " /XObject << {} >>",
            w.xobjects.iter().map(|(n, o)| format!("/{n} {o} 0 R")).collect::<Vec<_>>().join(" ")
        )
    };
    let props_entry = if w.props.is_empty() {
        String::new()
    } else {
        format!(
            " /Properties << {} >>",
            w.props.iter().map(|(n, o)| format!("/{n} {o} 0 R")).collect::<Vec<_>>().join(" ")
        )
    };
    w.objs.set(w.form_resources, format!("<<{cs_entry}{gs_entry} >>").into_bytes());
    w.objs.set(page_resources, format!("<<{cs_entry}{gs_entry}{xo_entry}{props_entry} >>").into_bytes());
    let contents = w.objs.add(stream_obj("", content.as_bytes()));
    let group_cs = w.cs.map(|c| format!("{c} 0 R")).unwrap_or_else(|| "/DeviceRGB".into());
    w.objs.set(
        page,
        format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 {} {}] /Resources {page_resources} 0 R /Contents {contents} 0 R /Group << /S /Transparency /CS {group_cs} >> >>",
            n6(pw),
            n6(ph)
        )
        .into_bytes(),
    );
    w.objs.set(pages, format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>").into_bytes());
    w.objs.set(
        info,
        "<< /Producer (Veskra veskra-io) /Creator (Veskra Export for Illustrator \\(PDF 1.7\\). Not a native .ai file.) >>"
            .as_bytes()
            .to_vec(),
    );
    let all: Vec<u32> = w.props.iter().map(|(_, n)| *n).collect();
    let refs = |v: &[u32]| v.iter().map(|n| format!("{n} 0 R")).collect::<Vec<_>>().join(" ");
    let order_s = order
        .iter()
        .map(|(o, kids)| if kids.is_empty() { format!("{o} 0 R") } else { format!("{o} 0 R [{}]", refs(kids)) })
        .collect::<Vec<_>>()
        .join(" ");
    let on: Vec<u32> = all.iter().copied().filter(|n| !off.contains(n)).collect();
    let ocprops = if all.is_empty() {
        String::new()
    } else {
        format!(
            " /OCProperties << /OCGs [{}] /D << /Name (Veskra layers) /BaseState /ON /Order [{order_s}] /ON [{}] /OFF [{}] >> >> /PageMode /UseOC",
            refs(&all),
            refs(&on),
            refs(&off)
        )
    };
    w.objs.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R{ocprops} >>").into_bytes());
    let (paths, images) = (w.paths, w.images);
    Ok((w.objs.finish(catalog, info)?, paths, images, w.color))
}

impl Exporter for PdfExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Pdf
    }

    fn preflight(&self, doc: &Document, opts: &ExportOptions) -> ExportPreflight {
        IllustratorPlan::build(doc, opts, self.strategy).to_preflight(doc, ExportFormat::Pdf)
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
        let (bytes, paths, images, color) = write_pdf(doc, &plan, cancel)?;
        let mut warnings = plan.warnings.clone();
        warnings.extend(plan.color_warnings(doc, ExportFormat::Pdf));
        warnings.extend(color.clip_warnings());
        cancel.check()?;
        out.write_all(&bytes)
            .and_then(|_| out.flush())
            .map_err(|e| VeskraError::new(ErrorCode::Io, format!("could not write PDF: {}", e.kind())))?;
        Ok(ExportReport {
            success: false,
            file_uri: None,
            format: ExportFormat::Pdf,
            profile: plan.profile_label(ExportFormat::Pdf),
            profile_embedded: opts.embed_color_profile,
            vector_object_count: paths,
            rasterized_object_count: images,
            flattened_feature_list: plan.flattened_features(),
            warnings,
            target_profile: color.target.id().semantic_name,
            bit_depth: 8,
            rendering_intent: color.intent(ExportFormat::Pdf),
            out_of_gamut_pixels: color.clipped_pixels(),
            duration_ms: start.elapsed().as_millis() as u64,
            file_bytes: bytes.len() as u64,
            width_px: doc.width(),
            height_px: doc.height(),
            validation: ValidationStatus::NotValidated,
        })
    }

    fn validate(&self, bytes: &[u8]) -> Result<(), VeskraError> {
        validate_pdf(bytes)
            .map(|_| ())
            .map_err(|m| VeskraError::new(ErrorCode::PdfEncodeError, format!("invalid PDF: {m}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_strings() {
        assert_eq!(pdf_text_string("Ink (1)\\"), "(Ink \\(1\\)\\\\)");
        assert_eq!(pdf_text_string("é"), "<FEFF00E9>");
        assert_eq!(pdf_text_string("線"), "<FEFF7DDA>");
    }

    #[test]
    fn minimal_file_validates() {
        let mut o = Objects::default();
        let c = o.alloc();
        let p = o.alloc();
        let i = o.add(b"<< >>".to_vec());
        o.set(c, format!("<< /Type /Catalog /Pages {p} 0 R >>").into_bytes());
        o.set(p, b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec());
        let bytes = o.finish(c, i).unwrap();
        let s = validate_pdf(&bytes).unwrap();
        assert_eq!(s.objects, 3);
        // Corrupt an xref offset → rejected.
        let pat = b"0000000015 00000 n";
        let at = bytes.windows(pat.len()).position(|w| w == pat).unwrap();
        let mut bad = bytes.clone();
        bad[at + 9] = b'6';
        assert!(validate_pdf(&bad).is_err());
        assert!(validate_pdf(b"not a pdf").is_err());
        assert!(validate_pdf(&bytes[..bytes.len() - 7]).is_err());
    }
}
