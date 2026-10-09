//! FMT-12 plan matrix: 4 option combinations × 2 strategies × every layer fidelity kind, plus
//! ILL-004 (both off blocked) and ILL-006 (raster-only counts/warnings before export).
//!
//! Invariants asserted for every combination:
//! * no stroke is both visible as a vector and visible in a raster;
//! * no content is dropped: every stroke/pixel of a visible layer is visible either as a vector
//!   or in a raster run; hidden layers are reported;
//! * raster-authoritative layers never produce vectors; counts equal what the writers emit.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod svg_fixtures;

use svg_fixtures::{DocBuilder, rgb, taper};
use veskra_core::export::{ExportFormat, ExportOptions, ExportWarningKind};
use veskra_core::{BlendMode, ColorRgba, ErrorCode, LayerId, RectI};
use veskra_doc::{Document, Placement, VectorFidelity};
use veskra_io::plan::{HybridStrategy, IllustratorPlan, LayerTreatment, VectorPlacement, VisibleItem};
use veskra_io::{CancelToken, Exporter, export_to_path, pdf::PdfExporter, svg::SvgExporter};

struct Ids {
    ink: LayerId,
    mixed: LayerId,
    texture: LayerId,
    flat: LayerId,
    pixels: LayerId,
    empty: LayerId,
    hidden: LayerId,
    group: LayerId,
    ginkk: LayerId,
    base: LayerId,
    clipper: LayerId,
    shade: LayerId,
}

/// Bottom → top: Flat(RA fill+ink), Pixels(no strokes), Mixed(ink+texture), Texture(texture only),
/// Empty, Hidden(ink), G(opacity .6){GInk}, Base(ink) + Clipper(ink, clip), Ink(2 inks, one 50%),
/// Shade(Multiply ink).
fn matrix_doc() -> (Document, Ids) {
    let mut b = DocBuilder::new(256, 192, 72.0);
    let flat = b.raster("Flat", Placement::Top);
    b.fill(flat, RectI::new(0, 0, 256, 60), rgb(230, 220, 200));
    b.ink(flat, &taper(20.0, 30.0, 200.0, 40.0, 6.0), rgb(10, 10, 10), 1.0);
    let pixels = b.raster("Pixels", Placement::Top);
    b.doc.pixels_mut_editable(pixels).unwrap().tiles.put_pixel(5, 5, [65535, 0, 0, 65535]);
    let mixed = b.raster("Mixed", Placement::Top);
    b.ink(mixed, &taper(30.0, 80.0, 220.0, 90.0, 8.0), rgb(200, 30, 30), 1.0);
    b.texture(mixed, RectI::new(40, 100, 120, 130), rgb(30, 90, 200), 1);
    let texture = b.raster("Texture <&\"'>", Placement::Top);
    b.texture(texture, RectI::new(140, 100, 230, 140), rgb(20, 160, 60), 2);
    let empty = b.raster("Empty", Placement::Top);
    let hidden = b.vector("Hidden", Placement::Top);
    b.ink(hidden, &taper(10.0, 150.0, 100.0, 160.0, 5.0), rgb(0, 0, 0), 1.0);
    b.hide(hidden);
    let group = b.group("G", Placement::Top);
    b.opacity(group, 0.6);
    let ginkk = b.vector("GInk", Placement::IntoGroup { group, index: None });
    b.ink(ginkk, &taper(20.0, 170.0, 230.0, 175.0, 7.0), rgb(90, 0, 160), 1.0);
    let base = b.vector("Base", Placement::Top);
    b.ink(base, &taper(60.0, 20.0, 180.0, 140.0, 20.0), rgb(250, 200, 0), 1.0);
    let clipper = b.vector("Clipper", Placement::Top);
    b.ink(clipper, &taper(50.0, 80.0, 200.0, 70.0, 12.0), rgb(0, 120, 120), 1.0);
    b.clip(clipper);
    let ink = b.vector("Ink", Placement::Top);
    b.ink(ink, &taper(10.0, 10.0, 240.0, 180.0, 9.0), rgb(0, 0, 0), 1.0);
    b.ink(ink, &taper(10.0, 180.0, 240.0, 10.0, 14.0), rgb(0, 0, 255), 0.5);
    let shade = b.vector("Shade", Placement::Top);
    b.blend(shade, BlendMode::Multiply);
    b.ink(shade, &taper(100.0, 5.0, 120.0, 185.0, 25.0), rgb(128, 128, 255), 1.0);
    let doc = b.finish();
    (doc, Ids { ink, mixed, texture, flat, pixels, empty, hidden, group, ginkk, base, clipper, shade })
}

fn opts(v: bool, r: bool) -> ExportOptions {
    ExportOptions { include_vector_paths: v, include_raster_appearance: r, ..Default::default() }
}

fn has_warning(p: &IllustratorPlan, k: ExportWarningKind) -> bool {
    p.warnings.iter().any(|w| w.kind == k)
}

fn check_invariants(doc: &Document, p: &IllustratorPlan, v: bool, r: bool) {
    // Every visible leaf is in exactly one item and the item kind matches its treatment.
    for lp in &p.layers {
        let holders: Vec<usize> = p
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| match it {
                VisibleItem::Raster { layers, .. } => layers.contains(&lp.layer),
                VisibleItem::Vectors { layer } => *layer == lp.layer,
            })
            .map(|(i, _)| i)
            .collect();
        if lp.treatment.is_visible() {
            assert_eq!(holders, vec![lp.item.unwrap()], "layer {} in items {holders:?}", lp.name);
            match &p.items[holders[0]] {
                VisibleItem::Raster { .. } => assert!(lp.treatment.is_raster()),
                VisibleItem::Vectors { .. } => assert_eq!(lp.treatment, LayerTreatment::VisibleVectors),
            }
        } else {
            assert!(holders.is_empty(), "invisible layer {} drawn", lp.name);
            assert_eq!(lp.vectors, VectorPlacement::None);
        }
        let layer = doc.layer(lp.layer).unwrap();
        let px = layer.pixels().unwrap();
        let content = !px.tiles.content_bounds().is_empty() || !px.strokes.is_empty();
        let visible = doc.is_effectively_visible(lp.layer);
        // No content dropped.
        if visible && content {
            assert!(lp.treatment.is_visible(), "content of {} dropped", lp.name);
        }
        // Per stroke: visible as vector XOR visible in raster.
        if lp.treatment.is_visible() {
            for s in &px.strokes {
                let rec = doc.stroke(*s).unwrap();
                let vis_vec = lp.treatment == LayerTreatment::VisibleVectors && lp.vector_strokes.contains(s);
                let vis_ras = lp.treatment.is_raster();
                assert!(vis_vec ^ vis_ras, "stroke on {} vis_vec={vis_vec} vis_ras={vis_ras}", lp.name);
                if vis_vec {
                    assert!(rec.has_vector_form());
                }
            }
        }
        if lp.treatment == LayerTreatment::VisibleVectors {
            assert_eq!(lp.fidelity, VectorFidelity::Exact);
            assert_eq!(lp.vectors, VectorPlacement::Visible);
            assert_eq!(lp.vector_strokes.len(), px.strokes.len());
        }
        if lp.vectors == VectorPlacement::Hidden {
            assert!(lp.treatment.is_raster(), "hidden vectors only back raster-shown layers");
        }
        if matches!(lp.fidelity, VectorFidelity::RasterAuthoritative { .. }) {
            assert_eq!(lp.vectors, VectorPlacement::None, "RA layer {} produced vectors", lp.name);
        }
        if !v {
            assert_eq!(lp.vectors, VectorPlacement::None);
        }
        if !r {
            assert_ne!(lp.treatment, LayerTreatment::RasterAppearance);
        }
        if lp.treatment == LayerTreatment::ForcedRaster {
            assert!(
                p.warnings.iter().any(|w| w.kind == ExportWarningKind::LayerRasterized
                    && w.layer.as_deref() == Some(&lp.layer.to_string())),
                "forced raster {} without warning",
                lp.name
            );
        }
    }
    // Raster runs are maximal (never two adjacent raster items).
    for w in p.items.windows(2) {
        assert!(!(matches!(w[0], VisibleItem::Raster { .. }) && matches!(w[1], VisibleItem::Raster { .. })));
    }
    let expected_paths: u32 =
        p.layers.iter().filter(|l| l.vectors != VectorPlacement::None).map(|l| l.vector_strokes.len() as u32).sum();
    assert_eq!(p.vector_path_count, expected_paths);
}

#[test]
fn plan_matrix_all_combinations() {
    let (doc, ids) = matrix_doc();
    for strategy in HybridStrategy::ALL {
        for (v, r) in [(true, true), (true, false), (false, true), (false, false)] {
            let p = IllustratorPlan::build(&doc, &opts(v, r), strategy);
            let ctx = format!("{strategy:?} v={v} r={r}");
            if !v && !r {
                assert!(p.blocked_reason.is_some(), "{ctx}");
                assert!(p.items.is_empty() && p.layers.is_empty(), "{ctx}");
                continue;
            }
            assert!(p.blocked_reason.is_none(), "{ctx}");
            check_invariants(&doc, &p, v, r);
            let t = |id: LayerId| p.get(id).unwrap().treatment;
            let vp = |id: LayerId| p.get(id).unwrap().vectors;
            // Fidelity kinds.
            assert_eq!(p.get(ids.ink).unwrap().fidelity, VectorFidelity::Exact);
            assert_eq!(p.get(ids.mixed).unwrap().fidelity, VectorFidelity::PartiallyVector);
            assert_eq!(p.get(ids.texture).unwrap().fidelity, VectorFidelity::PartiallyVector);
            assert!(matches!(p.get(ids.flat).unwrap().fidelity, VectorFidelity::RasterAuthoritative { .. }));
            assert_eq!(
                p.get(ids.pixels).unwrap().fidelity,
                VectorFidelity::RasterAuthoritative { reasons: vec!["pixels_without_strokes".into()] }
            );
            assert_eq!(t(ids.empty), LayerTreatment::Empty, "{ctx}");
            assert_eq!(t(ids.hidden), LayerTreatment::OmittedHidden, "{ctx}");
            assert!(has_warning(&p, ExportWarningKind::HiddenLayer), "{ctx}");
            // ILL-006 counts: 2 texture strokes, layers without vectors.
            assert_eq!(p.raster_only_stroke_count, 2, "{ctx}");
            assert!(has_warning(&p, ExportWarningKind::RasterOnlyStroke), "{ctx}");
            match (v, r, strategy) {
                (true, true, HybridStrategy::RasterVisibleVectorsHidden) => {
                    assert_eq!(p.items.len(), 1, "{ctx}: one exact composite run");
                    for id in [ids.ink, ids.mixed, ids.ginkk, ids.base, ids.clipper, ids.shade] {
                        assert_eq!(t(id), LayerTreatment::RasterAppearance);
                        assert_eq!(vp(id), VectorPlacement::Hidden, "{ctx}");
                    }
                    assert_eq!(vp(ids.flat), VectorPlacement::None);
                    assert_eq!(vp(ids.texture), VectorPlacement::None);
                    // 1 (mixed) + 1 (GInk) + 1 (Base) + 1 (Clipper) + 2 (Ink) + 1 (Shade)
                    assert_eq!(p.vector_path_count, 7);
                    assert_eq!(p.rasterized_layer_count, 3, "{ctx}: Flat, Pixels, Texture");
                    assert!(!p.has_visible_vectors());
                }
                (true, true, HybridStrategy::VectorsVisibleRasterResidual) => {
                    assert_eq!(t(ids.ink), LayerTreatment::VisibleVectors, "{ctx}");
                    assert_eq!(t(ids.shade), LayerTreatment::VisibleVectors, "{ctx}: root-level multiply is exact");
                    // Group opacity and clipping cannot be split exactly → raster + hidden vectors.
                    for id in [ids.ginkk, ids.base, ids.clipper, ids.mixed] {
                        assert_eq!(t(id), LayerTreatment::RasterAppearance, "{ctx}");
                        assert_eq!(vp(id), VectorPlacement::Hidden, "{ctx}");
                    }
                    assert_eq!(p.vector_path_count, 7);
                    assert!(has_warning(&p, ExportWarningKind::Other));
                    assert!(!has_warning(&p, ExportWarningKind::ClippingApproximated));
                    // One raster run below, then Ink, Shade vectors.
                    assert_eq!(p.items.len(), 3, "{ctx}: {:?}", p.items);
                }
                (true, false, _) => {
                    for id in [ids.ink, ids.shade, ids.ginkk, ids.base, ids.clipper] {
                        assert_eq!(t(id), LayerTreatment::VisibleVectors, "{ctx}");
                    }
                    for id in [ids.flat, ids.pixels, ids.mixed, ids.texture] {
                        assert_eq!(t(id), LayerTreatment::ForcedRaster, "{ctx}");
                    }
                    assert_eq!(vp(ids.mixed), VectorPlacement::Hidden);
                    assert!(has_warning(&p, ExportWarningKind::AppearanceOmitted), "{ctx}");
                    assert!(has_warning(&p, ExportWarningKind::ClippingApproximated), "{ctx}");
                    assert!(has_warning(&p, ExportWarningKind::BlendModeApproximated), "{ctx}: group opacity per item");
                    assert!(p.items.iter().all(|i| !matches!(i, VisibleItem::Raster { forced: false, .. })));
                }
                (false, true, _) => {
                    assert_eq!(p.items.len(), 1);
                    assert_eq!(p.vector_path_count, 0);
                    assert!(p.hidden_vectors.is_empty());
                    assert!(has_warning(&p, ExportWarningKind::VectorOmitted), "{ctx}");
                    let vo = p.warnings.iter().find(|w| w.kind == ExportWarningKind::VectorOmitted).unwrap();
                    assert_eq!(vo.count, 7);
                    assert_eq!(p.rasterized_layer_count, 9, "{ctx}: every visible content layer");
                }
                _ => unreachable!(),
            }
            let _ = ids.group;
        }
    }
}

#[test]
fn residual_strategy_r_off_mixed_clip_cluster_goes_raster() {
    let mut b = DocBuilder::new(64, 64, 72.0);
    let base = b.raster("Base", Placement::Top);
    b.texture(base, RectI::new(0, 0, 40, 40), rgb(200, 0, 0), 3);
    let clipper = b.vector("Clipper", Placement::Top);
    b.ink(clipper, &taper(0.0, 10.0, 60.0, 20.0, 8.0), rgb(0, 0, 0), 1.0);
    b.clip(clipper);
    let doc = b.finish();
    for s in HybridStrategy::ALL {
        let p = IllustratorPlan::build(&doc, &opts(true, false), s);
        assert_eq!(p.get(clipper).unwrap().treatment, LayerTreatment::ForcedRaster);
        assert_eq!(p.get(clipper).unwrap().vectors, VectorPlacement::Hidden);
        assert_eq!(p.items, vec![VisibleItem::Raster { layers: vec![base, clipper], forced: true }]);
        check_invariants(&doc, &p, true, false);
    }
}

#[test]
fn residual_blend_across_run_boundary_is_warned() {
    // Vector ink at the bottom, then a multiply texture layer above it → run above a vector item.
    let mut b = DocBuilder::new(64, 64, 72.0);
    let ink = b.vector("Ink", Placement::Top);
    b.ink(ink, &taper(0.0, 10.0, 60.0, 50.0, 8.0), rgb(0, 0, 0), 1.0);
    let tex = b.raster("Shadow", Placement::Top);
    b.texture(tex, RectI::new(0, 0, 64, 64), rgb(100, 100, 200), 4);
    b.blend(tex, BlendMode::Multiply);
    let doc = b.finish();
    let p = IllustratorPlan::build(&doc, &opts(true, true), HybridStrategy::VectorsVisibleRasterResidual);
    assert_eq!(p.items.len(), 2);
    assert!(p.warnings.iter().any(|w| w.kind == ExportWarningKind::BlendModeApproximated
        && w.layer.as_deref() == Some(&tex.to_string())));
    // Default strategy: single exact run, no approximation.
    let p = IllustratorPlan::build(&doc, &opts(true, true), HybridStrategy::RasterVisibleVectorsHidden);
    assert!(!p.warnings.iter().any(|w| w.kind == ExportWarningKind::BlendModeApproximated));
}

#[test]
fn profile_and_blend_space_warnings() {
    let mut b = DocBuilder::new(32, 32, 72.0);
    let l = b.vector("Ink", Placement::Top);
    b.ink(l, &taper(0.0, 10.0, 30.0, 20.0, 4.0), ColorRgba::BLACK, 1.0);
    let mut doc = b.finish();
    let o = ExportOptions { embed_color_profile: false, ..Default::default() };
    let p = IllustratorPlan::build(&doc, &o, HybridStrategy::default());
    assert!(has_warning(&p, ExportWarningKind::ProfileNotEmbedded));
    assert_eq!(p.profile_label(ExportFormat::Pdf), "sRGB values, profile not embedded");
    assert!(p.doc_is_srgb);
    doc.color.blend_space = veskra_core::BlendSpace::Linear;
    let p = IllustratorPlan::build(&doc, &ExportOptions::default(), HybridStrategy::default());
    assert!(has_warning(&p, ExportWarningKind::BlendSpaceMismatch));
    assert!(!has_warning(&p, ExportWarningKind::ProfileNotEmbedded));
}

#[test]
fn ill004_both_unchecked_blocked_everywhere() {
    let (doc, _) = matrix_doc();
    let o = opts(false, false);
    let dir = std::env::temp_dir().join(format!("veskra-fmtb-ill004-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (ex, fmt) in [
        (Box::new(SvgExporter::default()) as Box<dyn Exporter>, ExportFormat::Svg),
        (Box::new(PdfExporter::default()), ExportFormat::Pdf),
    ] {
        let pf = ex.preflight(&doc, &o);
        let reason = pf.blocked_reason.expect("blocked");
        assert!(reason.contains("Editable vector strokes") && reason.contains("Preserve original appearance"));
        assert_eq!(pf.estimated_bytes, 0);
        let mut sink = Vec::new();
        let e = ex.export(&doc, &o, &mut sink, &CancelToken::new()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
        assert!(sink.is_empty());
        let path = dir.join(format!("blocked.{}", fmt.extension()));
        assert!(export_to_path(&doc, fmt, &o, &path, &CancelToken::new()).is_err());
        assert!(!path.exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ill006_preflight_counts_before_export_match_report() {
    let (doc, _) = matrix_doc();
    for s in HybridStrategy::ALL {
        for (v, r) in [(true, true), (true, false), (false, true)] {
            let o = opts(v, r);
            for ex in
                [Box::new(SvgExporter { strategy: s }) as Box<dyn Exporter>, Box::new(PdfExporter { strategy: s })]
            {
                let pf = ex.preflight(&doc, &o);
                assert!(pf.blocked_reason.is_none());
                assert_eq!(pf.raster_only_stroke_count, 2);
                assert!(pf.warnings.iter().any(|w| w.kind == ExportWarningKind::RasterOnlyStroke && w.count == 2));
                assert_eq!((pf.width_px, pf.height_px), (256, 192));
                assert!(pf.estimated_bytes > 0);
                let mut sink = Vec::new();
                let rep = ex.export(&doc, &o, &mut sink, &CancelToken::new()).unwrap();
                assert_eq!(rep.vector_object_count, pf.vector_path_count, "{s:?} {v} {r} {:?}", ex.format());
                assert_eq!(rep.warnings, pf.warnings);
                assert_eq!(rep.file_bytes as usize, sink.len());
                ex.validate(&sink).unwrap();
            }
        }
    }
}

#[test]
fn cancellation_is_honoured() {
    let (doc, _) = matrix_doc();
    let c = CancelToken::new();
    c.cancel();
    for ex in [Box::new(SvgExporter::default()) as Box<dyn Exporter>, Box::new(PdfExporter::default())] {
        let mut sink = Vec::new();
        assert_eq!(ex.export(&doc, &ExportOptions::default(), &mut sink, &c).unwrap_err().code, ErrorCode::Cancelled);
        assert!(sink.is_empty());
    }
}
