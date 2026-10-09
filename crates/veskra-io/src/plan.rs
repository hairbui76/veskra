//! Illustrator export plan (FMT-12/13, ADR-008, ADR-012): the per-layer vector/raster treatment
//! shared by the SVG and PDF writers and by their preflight.
//!
//! The plan is computed from an immutable document snapshot and the export options, and it is
//! the single source of truth for *what* is written: both writers only serialize the plan.
//!
//! ## Classification (visible leaf layers, paint order)
//! * A layer's strokes may become vectors only if the layer's [`VectorFidelity`] is `Exact` or
//!   `PartiallyVector` **and** the stroke has a vector form ([`StrokeRecord::has_vector_form`]).
//!   `RasterAuthoritative` layers are never emitted as vectors (ADR-008). A pixel layer with
//!   pixels but no strokes is treated as raster-authoritative (`pixels_without_strokes`).
//! * Visible output is a paint-order list of [`VisibleItem`]s: *raster runs* (maximal runs of
//!   consecutive raster-treated leaves, composited together with the reference compositor and
//!   its `layer_filter`) and *visible vector layers*. Every visible leaf belongs to exactly one
//!   visible item, so no stroke is both visible as a vector and visible in a raster.
//! * Vector-form strokes of raster-treated layers are written as **hidden edit resources**
//!   (`Editable vectors` group, hidden) when vector output is on.
//!
//! | options (vector, appearance) | `RasterVisibleVectorsHidden` (default) | `VectorsVisibleRasterResidual` |
//! |---|---|---|
//! | on, on | one raster run of all layers (exact composite) + all vector forms hidden | `Exact` layers visible as vectors (unless clipping / group opacity / isolated blend would make the split inexact, then raster); other layers raster runs; their vector forms hidden |
//! | on, off | `Exact` layers visible vectors; other layers **forced raster** (warned); their vector forms hidden | same |
//! | off, on | one raster run of all layers; vectors omitted (warned) | same |
//! | off, off | blocked (ILL-004) | blocked |

use std::collections::{BTreeMap, BTreeSet};

use crate::CancelToken;
use veskra_color::{IccProfile, RenderingIntent, RgbTransform, convert};
use veskra_compose::{ComposeOptions, composite_rect};
use veskra_core::export::{ExportFormat, ExportOptions, ExportPreflight, ExportWarning, ExportWarningKind};
use veskra_core::{BlendMode, BlendSpace, ColorRgba, LayerId, RectI, StrokeId, VeskraError};
use veskra_doc::{Document, LayerKind, VectorFidelity};

/// How vector and raster content are layered when both export options are on (ADR-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HybridStrategy {
    /// Default: visible "Original appearance" raster(s) + hidden "Editable vectors" group.
    #[default]
    RasterVisibleVectorsHidden,
    /// Vectors visible; the raster contains only layers without an exact vector form.
    VectorsVisibleRasterResidual,
}

impl HybridStrategy {
    pub const ALL: [HybridStrategy; 2] =
        [HybridStrategy::RasterVisibleVectorsHidden, HybridStrategy::VectorsVisibleRasterResidual];

    /// Stable machine name.
    pub fn as_str(self) -> &'static str {
        match self {
            HybridStrategy::RasterVisibleVectorsHidden => "raster_visible_vectors_hidden",
            HybridStrategy::VectorsVisibleRasterResidual => "vectors_visible_raster_residual",
        }
    }
}

/// What happens to one leaf layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerTreatment {
    /// Effectively hidden (own/ancestor visibility, or clipped to a hidden base): not exported.
    OmittedHidden,
    /// No strokes and no pixels: nothing to export.
    Empty,
    /// Appearance embedded in a visible raster run ("Preserve original appearance" on).
    RasterAppearance,
    /// Appearance option off, but the layer has content without a faithful vector form, so its
    /// pixels are still embedded (ADR-008, no silent loss) and a warning is issued.
    ForcedRaster,
    /// Shown as visible vector outlines; no raster is written for this layer.
    VisibleVectors,
}

impl LayerTreatment {
    pub fn is_raster(self) -> bool {
        matches!(self, LayerTreatment::RasterAppearance | LayerTreatment::ForcedRaster)
    }
    pub fn is_visible(self) -> bool {
        self.is_raster() || self == LayerTreatment::VisibleVectors
    }
}

/// Where a layer's vector-form strokes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorPlacement {
    /// Not written (vector output off, raster-authoritative layer, or no vector-form strokes).
    None,
    /// Visible paths (the layer has no raster).
    Visible,
    /// Hidden edit resources (the layer is shown by a raster).
    Hidden,
}

/// Plan for one leaf (pixel) layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerPlan {
    pub layer: LayerId,
    pub name: String,
    /// Ancestor groups, outermost first.
    pub ancestors: Vec<LayerId>,
    /// Fidelity used for the decision (`pixels_without_strokes` override applied).
    pub fidelity: VectorFidelity,
    pub treatment: LayerTreatment,
    pub vectors: VectorPlacement,
    /// Strokes written as paths when `vectors != None` (vector form on a non-raster-authoritative layer).
    pub vector_strokes: Vec<StrokeId>,
    /// Strokes with no editable vector form in this export (raster-only, or on a raster-authoritative layer).
    pub raster_strokes: Vec<StrokeId>,
    /// Number of strokes intrinsically without a vector form (texture/airbrush/eraser/...).
    pub raster_only_strokes: u32,
    /// Layer has stored pixels.
    pub has_pixels: bool,
    /// Index of the visible item showing this layer.
    pub item: Option<usize>,
    /// Own opacity × ancestors' opacity (used for visible vectors).
    pub effective_opacity: f32,
    pub opacity: f32,
    pub blend: BlendMode,
}

/// One element of the visible paint-order stack.
#[derive(Debug, Clone, PartialEq)]
pub enum VisibleItem {
    /// Composite of these leaves (paint order) using the reference compositor's `layer_filter`.
    /// `forced` = the appearance option is off; the run only holds content without vector form.
    Raster { layers: Vec<LayerId>, forced: bool },
    /// A layer drawn as visible vector outlines.
    Vectors { layer: LayerId },
}

/// Tree of the hidden vector edit resources (doc group structure kept).
#[derive(Debug, Clone, PartialEq)]
pub enum VecNode {
    Group { id: LayerId, name: String, opacity: f32, blend: BlendMode, children: Vec<VecNode> },
    Layer(LayerId),
}

/// Complete export plan. Build with [`IllustratorPlan::build`].
#[derive(Debug, Clone, PartialEq)]
pub struct IllustratorPlan {
    pub strategy: HybridStrategy,
    pub options: ExportOptions,
    pub blocked_reason: Option<String>,
    /// All leaf layers (visible and hidden) in paint order.
    pub layers: Vec<LayerPlan>,
    /// Visible stack, bottom → top.
    pub items: Vec<VisibleItem>,
    /// Hidden vector edit resources, doc structure kept.
    pub hidden_vectors: Vec<VecNode>,
    pub vector_path_count: u32,
    pub raster_only_stroke_count: u32,
    pub rasterized_layer_count: u32,
    pub warnings: Vec<ExportWarning>,
    /// Document working profile is sRGB-equivalent (no conversion needed).
    pub doc_is_srgb: bool,
    /// Human-readable document profile description.
    pub doc_profile: String,
    index: BTreeMap<LayerId, usize>,
}

fn warn(kind: ExportWarningKind, message: impl Into<String>, layer: Option<LayerId>, count: u32) -> ExportWarning {
    ExportWarning { kind, message: message.into(), layer: layer.map(|l| l.to_string()), count }
}

fn layer_label(name: &str) -> String {
    if name.is_empty() { "(unnamed)".to_string() } else { name.to_string() }
}

/// Whether the document profile is sRGB (exact bytes or colorimetrically equivalent).
fn doc_profile_info(doc: &Document) -> (bool, String) {
    let srgb = IccProfile::srgb();
    if doc.color.icc_bytes.as_ref() == srgb.bytes() {
        return (true, srgb.description().to_string());
    }
    match IccProfile::parse(&doc.color.icc_bytes) {
        Ok(p) => (p.is_srgb_equivalent(), p.description().to_string()),
        Err(_) => (false, doc.color.profile.semantic_name.clone()),
    }
}

impl IllustratorPlan {
    /// Classify every leaf layer and build the visible stack, hidden resources, counts and
    /// warnings. Never fails; invalid options produce `blocked_reason` (ILL-004).
    pub fn build(doc: &Document, opts: &ExportOptions, strategy: HybridStrategy) -> IllustratorPlan {
        let (doc_is_srgb, doc_profile) = doc_profile_info(doc);
        let mut plan = IllustratorPlan {
            strategy,
            options: *opts,
            blocked_reason: None,
            layers: Vec::new(),
            items: Vec::new(),
            hidden_vectors: Vec::new(),
            vector_path_count: 0,
            raster_only_stroke_count: 0,
            rasterized_layer_count: 0,
            warnings: Vec::new(),
            doc_is_srgb,
            doc_profile,
            index: BTreeMap::new(),
        };
        // Both Illustrator formats share the toggle rules; validate as SVG.
        if let Err(e) = opts.validate(ExportFormat::Svg) {
            plan.blocked_reason = Some(e.message);
            return plan;
        }
        let want_vec = opts.include_vector_paths;
        let want_app = opts.include_raster_appearance;
        let split = want_vec && (!want_app || strategy == HybridStrategy::VectorsVisibleRasterResidual);

        let clusters = clip_clusters(doc);
        let mut notes = Notes::default();

        // ---- 1. per-leaf facts and initial treatment ----
        for id in doc.paint_order() {
            let Some(layer) = doc.layer(id) else { continue };
            let Some(px) = layer.pixels() else { continue };
            let ancestors = ancestors_of(doc, id);
            let has_pixels = !px.tiles.content_bounds().is_empty();
            let strokes = doc.strokes_of(id);
            let mut fidelity = doc
                .vector_fidelity(id)
                .unwrap_or(VectorFidelity::RasterAuthoritative { reasons: vec!["unknown".into()] });
            if strokes.is_empty() && has_pixels && !matches!(fidelity, VectorFidelity::RasterAuthoritative { .. }) {
                fidelity = VectorFidelity::RasterAuthoritative { reasons: vec!["pixels_without_strokes".into()] };
            }
            let ra = matches!(fidelity, VectorFidelity::RasterAuthoritative { .. });
            let mut vector_strokes = Vec::new();
            let mut raster_strokes = Vec::new();
            let mut raster_only = 0u32;
            for s in &strokes {
                if !s.has_vector_form() {
                    raster_only += 1;
                }
                if s.has_vector_form() && !ra {
                    vector_strokes.push(s.id);
                } else {
                    raster_strokes.push(s.id);
                }
            }
            let visible = doc.is_effectively_visible(id) && !clipped_to_hidden_base(doc, id);
            let content = has_pixels || !strokes.is_empty();
            let treatment = if !visible {
                LayerTreatment::OmittedHidden
            } else if !content {
                LayerTreatment::Empty
            } else if !split {
                LayerTreatment::RasterAppearance
            } else if fidelity == VectorFidelity::Exact
                && !vector_strokes.is_empty()
                && vector_strokes.iter().all(|s| stroke_has_path(doc, *s))
            {
                LayerTreatment::VisibleVectors
            } else if want_app {
                LayerTreatment::RasterAppearance
            } else {
                LayerTreatment::ForcedRaster
            };
            let mut eff = layer.props.opacity;
            for a in &ancestors {
                if let Some(g) = doc.layer(*a) {
                    eff *= g.props.opacity;
                }
            }
            plan.index.insert(id, plan.layers.len());
            plan.layers.push(LayerPlan {
                layer: id,
                name: layer.props.name.clone(),
                ancestors,
                fidelity,
                treatment,
                vectors: VectorPlacement::None,
                vector_strokes,
                raster_strokes,
                raster_only_strokes: raster_only,
                has_pixels,
                item: None,
                effective_opacity: eff,
                opacity: layer.props.opacity,
                blend: layer.props.blend,
            });
        }

        // ---- 2. exactness constraints on visible vectors ----
        if split {
            plan.apply_split_constraints(doc, &clusters, want_app, &mut notes);
        }

        // ---- 3. vector placement ----
        for lp in &mut plan.layers {
            let ra = matches!(lp.fidelity, VectorFidelity::RasterAuthoritative { .. });
            lp.vectors = if !want_vec || lp.vector_strokes.is_empty() || ra || !lp.treatment.is_visible() {
                VectorPlacement::None
            } else if lp.treatment == LayerTreatment::VisibleVectors {
                VectorPlacement::Visible
            } else {
                VectorPlacement::Hidden
            };
        }

        // ---- 4. visible stack ----
        for i in 0..plan.layers.len() {
            let (t, id) = (plan.layers[i].treatment, plan.layers[i].layer);
            if t.is_raster() {
                let forced = t == LayerTreatment::ForcedRaster;
                match plan.items.last_mut() {
                    Some(VisibleItem::Raster { layers, forced: f }) => {
                        layers.push(id);
                        *f &= forced;
                    }
                    _ => plan.items.push(VisibleItem::Raster { layers: vec![id], forced }),
                }
                plan.layers[i].item = Some(plan.items.len() - 1);
            } else if t == LayerTreatment::VisibleVectors {
                plan.items.push(VisibleItem::Vectors { layer: id });
                plan.layers[i].item = Some(plan.items.len() - 1);
            }
        }
        plan.check_run_blends(doc, &mut notes);
        plan.hidden_vectors = plan.build_hidden_tree(doc, doc.root());

        // ---- 5. counts and warnings ----
        plan.finish(doc, notes);
        plan
    }

    fn apply_split_constraints(
        &mut self,
        doc: &Document,
        clusters: &BTreeMap<LayerId, usize>,
        want_app: bool,
        notes: &mut Notes,
    ) {
        // Clip clusters: with appearance on, any vector member → raster (exact). With
        // appearance off, a cluster that already needs a forced raster goes raster as a whole
        // (a filtered base would hide its clippers); an all-vector cluster stays vector, with a
        // clipping warning.
        let mut by_cluster: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, lp) in self.layers.iter().enumerate() {
            if let Some(c) = clusters.get(&lp.layer) {
                by_cluster.entry(*c).or_default().push(i);
            }
        }
        for members in by_cluster.values() {
            let any_vec = members.iter().any(|&i| self.layers[i].treatment == LayerTreatment::VisibleVectors);
            if !any_vec {
                continue;
            }
            let any_raster = members.iter().any(|&i| self.layers[i].treatment.is_raster());
            if want_app || any_raster {
                for &i in members {
                    let lp = &mut self.layers[i];
                    if lp.treatment == LayerTreatment::VisibleVectors {
                        lp.treatment =
                            if want_app { LayerTreatment::RasterAppearance } else { LayerTreatment::ForcedRaster };
                        notes.demoted.push((lp.layer, "clipping"));
                    }
                }
            } else {
                for &i in members {
                    if self.layers[i].treatment == LayerTreatment::VisibleVectors {
                        notes.clip_approx.push(self.layers[i].layer);
                    }
                }
            }
        }
        // Group opacity / isolated group blends / own blend inside an isolated group cannot be
        // split across items exactly.
        for lp in &mut self.layers {
            if lp.treatment != LayerTreatment::VisibleVectors {
                continue;
            }
            let mut bad_group = None;
            let mut isolated_ancestor = false;
            for a in &lp.ancestors {
                let Some(g) = doc.layer(*a) else { continue };
                let pass = g.group().is_some_and(|c| c.pass_through);
                if !pass {
                    isolated_ancestor = true;
                }
                if g.props.opacity < 1.0 || (!pass && g.props.blend != BlendMode::Normal) {
                    bad_group.get_or_insert(*a);
                }
            }
            let own_blend_issue = lp.blend != BlendMode::Normal && isolated_ancestor;
            if bad_group.is_none() && !own_blend_issue {
                continue;
            }
            if want_app {
                lp.treatment = LayerTreatment::RasterAppearance;
                notes.demoted.push((lp.layer, if bad_group.is_some() { "group opacity/blend" } else { "blend mode" }));
            } else if let Some(g) = bad_group {
                notes.group_approx.insert(g);
            } else {
                notes.blend_approx.push(lp.layer);
            }
        }
    }

    /// Non-Normal blends inside a raster run that has other visible items below it are blended
    /// against the run's own backdrop only (approximation, warned).
    fn check_run_blends(&self, doc: &Document, notes: &mut Notes) {
        for (k, item) in self.items.iter().enumerate() {
            let VisibleItem::Raster { layers, .. } = item else { continue };
            if k == 0 {
                continue;
            }
            for id in layers {
                let Some(lp) = self.get(*id) else { continue };
                let group_blend = lp.ancestors.iter().any(|a| {
                    doc.layer(*a).is_some_and(|g| {
                        g.props.blend != BlendMode::Normal && !g.group().is_some_and(|c| c.pass_through)
                    })
                });
                if lp.blend != BlendMode::Normal || group_blend {
                    notes.blend_approx.push(*id);
                }
            }
        }
    }

    fn build_hidden_tree(&self, doc: &Document, ids: &[LayerId]) -> Vec<VecNode> {
        let mut out = Vec::new();
        for id in ids {
            let Some(layer) = doc.layer(*id) else { continue };
            match &layer.kind {
                LayerKind::Group(g) => {
                    let children = self.build_hidden_tree(doc, &g.children);
                    if !children.is_empty() {
                        out.push(VecNode::Group {
                            id: *id,
                            name: layer.props.name.clone(),
                            opacity: layer.props.opacity,
                            blend: layer.props.blend,
                            children,
                        });
                    }
                }
                _ => {
                    if self.get(*id).is_some_and(|lp| lp.vectors == VectorPlacement::Hidden) {
                        out.push(VecNode::Layer(*id));
                    }
                }
            }
        }
        out
    }

    fn finish(&mut self, doc: &Document, notes: Notes) {
        let opts = self.options;
        let mut w = Vec::new();
        let visible: Vec<&LayerPlan> = self.layers.iter().filter(|l| l.treatment.is_visible()).collect();
        self.vector_path_count = visible
            .iter()
            .filter(|l| l.vectors != VectorPlacement::None)
            .map(|l| l.vector_strokes.iter().filter(|s| stroke_has_path(doc, **s)).count() as u32)
            .sum();
        self.raster_only_stroke_count = visible.iter().map(|l| l.raster_only_strokes).sum();
        self.rasterized_layer_count = visible.iter().filter(|l| l.vectors == VectorPlacement::None).count() as u32;

        if self.raster_only_stroke_count > 0 {
            let msg = if opts.include_raster_appearance {
                format!(
                    "{} raster-only strokes (texture, airbrush, eraser, ...) cannot become editable vectors; they are kept as pixels in the raster appearance.",
                    self.raster_only_stroke_count
                )
            } else {
                format!(
                    "{} raster-only strokes have no vector form; they are still embedded as sRGB raster images although 'Preserve original appearance' is off.",
                    self.raster_only_stroke_count
                )
            };
            w.push(warn(ExportWarningKind::RasterOnlyStroke, msg, None, self.raster_only_stroke_count));
        }
        for lp in &visible {
            if let VectorFidelity::RasterAuthoritative { reasons } = &lp.fidelity {
                w.push(warn(
                    ExportWarningKind::LayerRasterized,
                    format!(
                        "Layer '{}' has raster edits ({}); it is exported as pixels only, no editable vectors.",
                        layer_label(&lp.name),
                        reasons.join(", ")
                    ),
                    Some(lp.layer),
                    1,
                ));
            } else if lp.treatment == LayerTreatment::ForcedRaster {
                w.push(warn(
                    ExportWarningKind::LayerRasterized,
                    format!(
                        "Layer '{}' contains content without an exact vector form; its pixels are embedded although 'Preserve original appearance' is off.",
                        layer_label(&lp.name)
                    ),
                    Some(lp.layer),
                    1,
                ));
            }
        }
        for (id, why) in &notes.demoted {
            if let Some(lp) = self.get(*id) {
                let (kind, tail) = if lp.treatment == LayerTreatment::ForcedRaster {
                    (ExportWarningKind::LayerRasterized, "its pixels are embedded")
                } else {
                    (ExportWarningKind::Other, "it is shown by the raster appearance")
                };
                w.push(warn(
                    kind,
                    format!(
                        "Layer '{}' cannot be shown as visible vectors without changing its look ({why}); {tail} and its paths are in the hidden 'Editable vectors' group.",
                        layer_label(&lp.name)
                    ),
                    Some(*id),
                    1,
                ));
            }
        }
        if !opts.include_vector_paths {
            let n: u32 = visible
                .iter()
                .filter(|l| !matches!(l.fidelity, VectorFidelity::RasterAuthoritative { .. }))
                .map(|l| l.vector_strokes.len() as u32)
                .sum();
            if n > 0 {
                w.push(warn(
                    ExportWarningKind::VectorOmitted,
                    format!("{n} strokes with an editable vector form are exported as pixels only ('Editable vector strokes' is off)."),
                    None,
                    n,
                ));
            }
        }
        let vis_vec = visible.iter().filter(|l| l.treatment == LayerTreatment::VisibleVectors).count() as u32;
        if vis_vec > 0 {
            if !opts.include_raster_appearance {
                w.push(warn(
                    ExportWarningKind::AppearanceOmitted,
                    format!("{vis_vec} layers are exported as vector outlines only; brush pixel appearance (soft edges, texture) is not embedded ('Preserve original appearance' is off)."),
                    None,
                    vis_vec,
                ));
            } else {
                w.push(warn(
                    ExportWarningKind::Other,
                    format!("{vis_vec} layers are shown as editable vector outlines (residual strategy); their look is the outline fill, not the brush pixels."),
                    None,
                    vis_vec,
                ));
            }
        }
        for id in &notes.clip_approx {
            if let Some(lp) = self.get(*id) {
                w.push(warn(
                    ExportWarningKind::ClippingApproximated,
                    format!(
                        "Layer '{}' is clipped/used as a clipping base; vector output is not clipped.",
                        layer_label(&lp.name)
                    ),
                    Some(*id),
                    1,
                ));
            }
        }
        for g in &notes.group_approx {
            let name = doc.layer(*g).map(|l| layer_label(&l.props.name)).unwrap_or_default();
            w.push(warn(
                ExportWarningKind::BlendModeApproximated,
                format!("Group '{name}' opacity/blend mode is applied to each exported item separately; overlaps may look different."),
                Some(*g),
                1,
            ));
        }
        let mut seen = BTreeSet::new();
        for id in &notes.blend_approx {
            if !seen.insert(*id) {
                continue;
            }
            if let Some(lp) = self.get(*id) {
                w.push(warn(
                    ExportWarningKind::BlendModeApproximated,
                    format!("Layer '{}' blend mode is applied without the full backdrop below it; colors where it overlaps lower content may differ.", layer_label(&lp.name)),
                    Some(*id),
                    1,
                ));
            }
        }
        let hidden = self.layers.iter().filter(|l| l.treatment == LayerTreatment::OmittedHidden).count() as u32;
        if hidden > 0 {
            w.push(warn(
                ExportWarningKind::HiddenLayer,
                format!("{hidden} hidden layers are not exported."),
                None,
                hidden,
            ));
        }
        if doc.color.blend_space == BlendSpace::Linear {
            w.push(warn(
                ExportWarningKind::BlendSpaceMismatch,
                "Document blends in linear light; Illustrator and other apps composite overlapping objects in gamma-encoded sRGB, so overlaps may differ.",
                None,
                0,
            ));
        }
        if !self.doc_is_srgb {
            w.push(warn(
                ExportWarningKind::Other,
                format!(
                    "Colors are converted from the document profile '{}' to sRGB (relative colorimetric).",
                    self.doc_profile
                ),
                None,
                0,
            ));
        }
        if !opts.embed_color_profile {
            w.push(warn(
                ExportWarningKind::ProfileNotEmbedded,
                "No ICC profile is embedded; colors are written as sRGB values.",
                None,
                0,
            ));
        }
        self.warnings = w;
    }

    /// Plan entry for a leaf layer.
    pub fn get(&self, id: LayerId) -> Option<&LayerPlan> {
        self.index.get(&id).and_then(|i| self.layers.get(*i))
    }

    /// Any visible item draws vectors.
    pub fn has_visible_vectors(&self) -> bool {
        self.items.iter().any(|i| matches!(i, VisibleItem::Vectors { .. }))
    }

    /// Profile semantics string for preflight/report (truthful per format, ILL-005).
    pub fn profile_label(&self, format: ExportFormat) -> String {
        if !self.options.embed_color_profile {
            return "sRGB values, profile not embedded".into();
        }
        match format {
            ExportFormat::Pdf => "sRGB IEC61966-2.1 (embedded ICCBased color space)".into(),
            _ => "sRGB IEC61966-2.1 (ICC embedded in raster images; vector colors are sRGB values)".into(),
        }
    }

    /// Rough output size estimate.
    pub fn estimate_bytes(&self, doc: &Document, format: ExportFormat) -> u64 {
        let mut n: u64 = 2048;
        for lp in &self.layers {
            if lp.vectors == VectorPlacement::None {
                continue;
            }
            for s in &lp.vector_strokes {
                if let Some(g) = doc.stroke(*s).and_then(|r| r.geometry.as_ref()) {
                    n += 64 + g.outline.els.len() as u64 * 28;
                }
            }
        }
        for item in &self.items {
            if let VisibleItem::Raster { layers, .. } = item {
                let r = run_bounds(doc, layers);
                // ~40% of raw RGBA8 after deflate for typical artwork; base64 adds 1/3 in SVG.
                let raw = r.area() * 4 * 2 / 5;
                n += if format == ExportFormat::Svg { raw * 4 / 3 } else { raw };
                if self.options.embed_color_profile {
                    n += IccProfile::srgb().bytes().len() as u64;
                }
            }
        }
        n
    }

    /// Preflight for `format` (SVG or PDF) derived from this plan.
    pub fn to_preflight(&self, doc: &Document, format: ExportFormat) -> ExportPreflight {
        let blocked = self.blocked_reason.is_some();
        ExportPreflight {
            format,
            options: self.options,
            blocked_reason: self.blocked_reason.clone(),
            vector_path_count: self.vector_path_count,
            raster_only_stroke_count: self.raster_only_stroke_count,
            rasterized_layer_count: self.rasterized_layer_count,
            profile: self.profile_label(format),
            width_px: doc.width(),
            height_px: doc.height(),
            estimated_bytes: if blocked { 0 } else { self.estimate_bytes(doc, format) },
            warnings: self.warnings.clone(),
        }
    }

    /// Human-readable list of flattened features for the export report.
    pub fn flattened_features(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.raster_only_stroke_count > 0 {
            out.push(format!("raster_only_strokes:{}", self.raster_only_stroke_count));
        }
        for lp in self.layers.iter().filter(|l| l.treatment.is_visible() && l.vectors == VectorPlacement::None) {
            out.push(format!("rasterized_layer:{}", layer_label(&lp.name)));
        }
        out
    }
}

#[derive(Debug, Default)]
struct Notes {
    demoted: Vec<(LayerId, &'static str)>,
    clip_approx: Vec<LayerId>,
    group_approx: BTreeSet<LayerId>,
    blend_approx: Vec<LayerId>,
}

fn stroke_has_path(doc: &Document, s: StrokeId) -> bool {
    doc.stroke(s).and_then(|r| r.geometry.as_ref()).is_some_and(|g| !g.outline.is_empty())
}

fn ancestors_of(doc: &Document, id: LayerId) -> Vec<LayerId> {
    let mut out = Vec::new();
    let mut cur = doc.layer(id).and_then(|l| l.parent);
    while let Some(p) = cur {
        if out.len() > veskra_core::limits::MAX_GROUP_DEPTH + 1 {
            break;
        }
        out.push(p);
        cur = doc.layer(p).and_then(|l| l.parent);
    }
    out.reverse();
    out
}

fn leaves_of(doc: &Document, id: LayerId, out: &mut Vec<LayerId>) {
    match doc.layer(id).map(|l| &l.kind) {
        Some(LayerKind::Group(g)) => {
            for c in &g.children {
                leaves_of(doc, *c, out);
            }
        }
        Some(_) => out.push(id),
        None => {}
    }
}

fn is_clipper(doc: &Document, id: LayerId) -> bool {
    doc.layer(id).is_some_and(|l| !l.is_group() && l.props.clip_to_below)
}

/// Leaf → clip cluster index. A cluster is a base (layer or group subtree) plus the maximal run
/// of clippers directly above it in its sibling list (veskra-compose rule 8).
fn clip_clusters(doc: &Document) -> BTreeMap<LayerId, usize> {
    let mut out = BTreeMap::new();
    let mut next = 0usize;
    let mut lists: Vec<Vec<LayerId>> = vec![doc.root().to_vec()];
    for l in doc.iter_layers() {
        if let Some(g) = l.group() {
            lists.push(g.children.clone());
        }
    }
    for list in &lists {
        let mut i = 0;
        while i < list.len() {
            if is_clipper(doc, list[i]) {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < list.len() && is_clipper(doc, list[j]) {
                j += 1;
            }
            if j > i + 1 {
                let mut members = Vec::new();
                leaves_of(doc, list[i], &mut members);
                members.extend_from_slice(&list[i + 1..j]);
                for m in members {
                    out.insert(m, next);
                }
                next += 1;
            }
            i = j;
        }
    }
    out
}

/// A clipper whose base is hidden is not drawn by the compositor.
fn clipped_to_hidden_base(doc: &Document, id: LayerId) -> bool {
    if !is_clipper(doc, id) {
        return false;
    }
    let parent = doc.layer(id).and_then(|l| l.parent);
    let Ok(list) = doc.children_of(parent) else { return false };
    let Some(pos) = list.iter().position(|x| *x == id) else { return false };
    let mut k = pos;
    while k > 0 {
        k -= 1;
        if !is_clipper(doc, list[k]) {
            return !doc.layer(list[k]).is_some_and(|b| b.props.visible);
        }
    }
    false // no base: composites as a normal layer
}

/// Union of the layers' pixel content bounds, clipped to the canvas.
pub(crate) fn run_bounds(doc: &Document, layers: &[LayerId]) -> RectI {
    let mut r = RectI::EMPTY;
    for id in layers {
        if let Some(p) = doc.layer(*id).and_then(|l| l.pixels()) {
            let b = p.tiles.content_bounds();
            if !b.is_empty() {
                r = if r.is_empty() { b } else { r.union(&b) };
            }
        }
    }
    r.intersect(&doc.bounds())
}

/// sRGB output conversion for colors and pixels (identity for sRGB documents).
pub(crate) struct SrgbOut {
    transform: Option<RgbTransform>,
}

impl std::fmt::Debug for SrgbOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SrgbOut").field("converts", &self.transform.is_some()).finish()
    }
}

impl SrgbOut {
    pub(crate) fn new(doc: &Document, plan: &IllustratorPlan) -> SrgbOut {
        if plan.doc_is_srgb {
            return SrgbOut { transform: None };
        }
        let t = IccProfile::parse(&doc.color.icc_bytes)
            .ok()
            .and_then(|p| RgbTransform::new(&p, &IccProfile::srgb(), RenderingIntent::RelativeColorimetric).ok())
            .filter(|t| !t.is_identity());
        SrgbOut { transform: t }
    }

    /// Straight sRGB components of a document color.
    pub(crate) fn color(&self, c: ColorRgba) -> [f32; 3] {
        let rgb = [c.r, c.g, c.b];
        let v = match &self.transform {
            Some(t) => t.apply(rgb),
            None => rgb,
        };
        v.map(|x| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 })
    }

    fn pixels(&self, px: &mut [u16]) {
        if let Some(t) = &self.transform {
            t.apply_rgba16_premul(px);
        }
    }
}

/// Composited raster for one visible run: straight-alpha RGBA8 sRGB, cropped to non-transparent
/// pixels; `rect` is in document pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RunImage {
    pub rect: RectI,
    pub rgba: Vec<u8>,
}

impl RunImage {
    pub(crate) fn is_opaque(&self) -> bool {
        self.rgba.chunks_exact(4).all(|p| p[3] == 255)
    }
}

/// Composite the leaves of a run (reference compositor, `layer_filter`), convert to straight
/// sRGB RGBA8 and crop. `None` if fully transparent.
pub(crate) fn render_run(
    doc: &Document,
    layers: &[LayerId],
    color: &SrgbOut,
    cancel: &CancelToken,
) -> Result<Option<RunImage>, VeskraError> {
    let bounds = run_bounds(doc, layers);
    if bounds.is_empty() {
        return Ok(None);
    }
    let set: BTreeSet<LayerId> = layers.iter().copied().collect();
    let filter = |id: LayerId| set.contains(&id);
    let copts = ComposeOptions { layer_filter: Some(&filter), ..Default::default() };
    let w = bounds.width() as usize;
    let h = bounds.height() as usize;
    let len = veskra_core::limits::checked_buffer_len(bounds.width(), bounds.height(), 4)?;
    let mut rgba = vec![0u8; len];
    let band = veskra_raster::TILE as i32;
    let mut y = bounds.y0;
    while y < bounds.y1 {
        cancel.check()?;
        let y1 = (y + band).min(bounds.y1);
        let r = RectI::new(bounds.x0, y, bounds.x1, y1);
        let mut px = composite_rect(doc, r, &copts);
        if px.len() != r.area() as usize * 4 {
            return Err(VeskraError::alloc("composite band unavailable"));
        }
        color.pixels(&mut px);
        let off = (y - bounds.y0) as usize * w * 4;
        if let Some(dst) = rgba.get_mut(off..off + px.len()) {
            convert::premul16_to_straight8(&px, dst);
        }
        y = y1;
    }
    // Crop to alpha > 0.
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for (yy, row) in rgba.chunks_exact(w * 4).enumerate() {
        let mut any = false;
        for (xx, p) in row.chunks_exact(4).enumerate() {
            if p[3] != 0 {
                any = true;
                x0 = x0.min(xx);
                x1 = x1.max(xx + 1);
            }
        }
        if any {
            y0 = y0.min(yy);
            y1 = yy + 1;
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Ok(None);
    }
    let cw = x1 - x0;
    let mut out = Vec::with_capacity(cw * (y1 - y0) * 4);
    for row in rgba.chunks_exact(w * 4).skip(y0).take(y1 - y0) {
        out.extend_from_slice(&row[x0 * 4..x1 * 4]);
    }
    let rect = RectI::new(bounds.x0 + x0 as i32, bounds.y0 + y0 as i32, bounds.x0 + x1 as i32, bounds.y0 + y1 as i32);
    Ok(Some(RunImage { rect, rgba: out }))
}

/// Fill opacity of a stroke path (`color.a × stroke opacity`).
pub(crate) fn stroke_alpha(rec: &veskra_doc::StrokeRecord) -> f32 {
    let a = rec.color.a * rec.composition.opacity;
    if a.is_finite() { a.clamp(0.0, 1.0) } else { 1.0 }
}
