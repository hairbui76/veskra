#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use veskra_core::{BlendMode, BlendSpace, ColorRgba, DocumentId, LayerId, ProfileId, RectI, StrokeId, StrokeSample};
use veskra_doc::*;

fn color() -> DocColor {
    DocColor {
        profile: ProfileId::new([7; 32], "sRGB test"),
        icc_bytes: Arc::from(vec![0u8; 16]),
        assumed: false,
        blend_space: BlendSpace::GammaEncoded,
    }
}

fn doc() -> Document {
    Document::new(DocumentId::from_u128(1), 256, 128, 300.0, color()).unwrap()
}

fn lid(n: u128) -> LayerId {
    LayerId::from_u128(n)
}

fn raster() -> LayerKind {
    LayerKind::Raster(PixelContent::empty())
}

fn group() -> LayerKind {
    LayerKind::Group(GroupContent { children: vec![], pass_through: false, collapsed: false })
}

fn add(d: &mut Document, n: u128, kind: LayerKind, p: Placement) {
    d.add_layer(lid(n), LayerProps::named(format!("L{n}")), kind, p).unwrap();
}

#[test]
fn rejects_bad_dimensions_and_dpi() {
    assert!(Document::new(DocumentId::from_u128(1), 0, 10, 72.0, color()).is_err());
    assert!(Document::new(DocumentId::from_u128(1), 20_000, 10, 72.0, color()).is_err());
    assert!(Document::new(DocumentId::from_u128(1), 10, 10, f32::NAN, color()).is_err());
}

#[test]
fn placement_and_paint_order() {
    let mut d = doc();
    add(&mut d, 1, raster(), Placement::Top);
    add(&mut d, 2, raster(), Placement::Top);
    add(&mut d, 3, raster(), Placement::Below(lid(2)));
    add(&mut d, 10, group(), Placement::Top);
    add(&mut d, 11, raster(), Placement::IntoGroup { group: lid(10), index: None });
    add(&mut d, 12, raster(), Placement::IntoGroup { group: lid(10), index: Some(0) });
    assert_eq!(d.root(), &[lid(1), lid(3), lid(2), lid(10)]);
    assert_eq!(d.paint_order(), vec![lid(1), lid(3), lid(2), lid(10), lid(12), lid(11)]);
    assert_eq!(d.layer(lid(11)).unwrap().parent, Some(lid(10)));
    d.validate().unwrap();
}

#[test]
fn duplicate_ids_and_nonempty_new_group_rejected() {
    let mut d = doc();
    add(&mut d, 1, raster(), Placement::Top);
    assert_eq!(d.add_layer(lid(1), LayerProps::named("x"), raster(), Placement::Top), Err(DocumentError::DuplicateId));
    let bad = LayerKind::Group(GroupContent { children: vec![lid(1)], pass_through: false, collapsed: false });
    assert!(d.add_layer(lid(2), LayerProps::named("g"), bad, Placement::Top).is_err());
    d.validate().unwrap();
}

#[test]
fn move_rejects_cycles_and_is_atomic() {
    let mut d = doc();
    add(&mut d, 10, group(), Placement::Top);
    add(&mut d, 20, group(), Placement::IntoGroup { group: lid(10), index: None });
    add(&mut d, 30, raster(), Placement::IntoGroup { group: lid(20), index: None });
    let before = d.clone();
    assert_eq!(d.move_layer(lid(10), Placement::IntoGroup { group: lid(20), index: None }), Err(DocumentError::Cycle));
    assert_eq!(d.move_layer(lid(10), Placement::IntoGroup { group: lid(10), index: None }), Err(DocumentError::Cycle));
    assert_eq!(d.move_layer(lid(10), Placement::Above(lid(30))), Err(DocumentError::Cycle));
    assert_eq!(d, before);
    d.move_layer(lid(30), Placement::Top).unwrap();
    assert_eq!(d.root(), &[lid(10), lid(30)]);
    assert_eq!(d.layer(lid(30)).unwrap().parent, None);
    d.validate().unwrap();
}

#[test]
fn group_depth_limit() {
    let mut d = doc();
    add(&mut d, 1, group(), Placement::Top);
    let mut last = 1;
    let mut err = None;
    for n in 2..60u128 {
        match d.add_layer(
            lid(n),
            LayerProps::named("g"),
            group(),
            Placement::IntoGroup { group: lid(last), index: None },
        ) {
            Ok(()) => last = n,
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    assert_eq!(err, Some(DocumentError::TooDeep));
    d.validate().unwrap();
}

#[test]
fn remove_group_removes_subtree_and_strokes() {
    let mut d = doc();
    add(&mut d, 10, group(), Placement::Top);
    add(&mut d, 11, raster(), Placement::IntoGroup { group: lid(10), index: None });
    d.add_stroke(stroke(1, 11)).unwrap();
    assert_eq!(d.stroke_count(), 1);
    let removed = d.remove_layer(lid(10)).unwrap();
    assert_eq!(removed, vec![lid(10), lid(11)]);
    assert_eq!(d.layer_count(), 0);
    assert_eq!(d.stroke_count(), 0);
    d.validate().unwrap();
}

fn stroke(n: u128, layer: u128) -> StrokeRecord {
    StrokeRecord::new(
        StrokeId::from_u128(n),
        lid(layer),
        vec![StrokeSample::confirmed(1.0, 2.0, 3, 0.5), StrokeSample::confirmed(5.0, 6.0, 4, 0.7)],
        "pen.technical",
        1,
        "{}".into(),
        ColorRgba::BLACK,
        9,
        ProfileId::new([7; 32], "sRGB test"),
        StrokeComposition { mode: StrokeMode::Paint, opacity: 1.0, alpha_locked: false, selection_clipped: false },
        true,
        None,
        RectI::new(0, 0, 8, 8),
    )
    .unwrap()
}

#[test]
fn duplicate_layer_deep_copies_with_new_ids() {
    let mut d = doc();
    add(&mut d, 10, group(), Placement::Top);
    add(&mut d, 11, raster(), Placement::IntoGroup { group: lid(10), index: None });
    d.add_stroke(stroke(1, 11)).unwrap();
    d.layer_mut(lid(11)).unwrap().pixels_mut().unwrap().tiles.put_pixel(3, 3, [100, 200, 300, 1000]);
    let mut next = 1000u128;
    let mut gen_id = || {
        next += 1;
        next
    };
    let new_top = d.duplicate_layer(lid(10), &mut gen_id).unwrap();
    assert_eq!(d.root(), &[lid(10), new_top]);
    assert_eq!(d.layer(new_top).unwrap().props.name, "L10 copy");
    let new_child = d.layer(new_top).unwrap().group().unwrap().children[0];
    assert_ne!(new_child, lid(11));
    assert_eq!(d.layer(new_child).unwrap().parent, Some(new_top));
    let strokes = d.strokes_of(new_child);
    assert_eq!(strokes.len(), 1);
    assert_ne!(strokes[0].id, StrokeId::from_u128(1));
    assert_eq!(strokes[0].layer, new_child);
    assert_eq!(d.layer(new_child).unwrap().pixels().unwrap().tiles.get_pixel(3, 3), [100, 200, 300, 1000]);
    assert_eq!(d.stroke_count(), 2);
    d.validate().unwrap();
}

#[test]
fn locks_respected_including_ancestors() {
    let mut d = doc();
    add(&mut d, 10, group(), Placement::Top);
    add(&mut d, 11, raster(), Placement::IntoGroup { group: lid(10), index: None });
    d.apply_patch(lid(10), &LayerPatch { locked: Some(true), ..Default::default() }).unwrap();
    assert!(d.is_effectively_locked(lid(11)));
    assert_eq!(d.pixels_mut_editable(lid(11)).err(), Some(DocumentError::Locked));
    assert_eq!(
        d.apply_patch(lid(11), &LayerPatch { opacity: Some(0.5), ..Default::default() }),
        Err(DocumentError::Locked)
    );
    // visibility may still change on a locked layer
    d.apply_patch(lid(11), &LayerPatch { visible: Some(false), ..Default::default() }).unwrap();
    assert!(!d.is_effectively_visible(lid(11)));
    assert_eq!(d.remove_layer(lid(11)), Err(DocumentError::Locked));
}

#[test]
fn patch_validation() {
    let mut d = doc();
    add(&mut d, 1, raster(), Placement::Top);
    assert!(d.apply_patch(lid(1), &LayerPatch { opacity: Some(1.5), ..Default::default() }).is_err());
    assert!(d.apply_patch(lid(1), &LayerPatch { name: Some("x".repeat(300)), ..Default::default() }).is_err());
    assert!(d.apply_patch(lid(1), &LayerPatch { pass_through: Some(true), ..Default::default() }).is_err());
    assert_eq!(d.layer(lid(1)).unwrap().props.opacity, 1.0);
    d.apply_patch(lid(1), &LayerPatch { blend: Some(BlendMode::Multiply), ..Default::default() }).unwrap();
    assert_eq!(d.layer(lid(1)).unwrap().props.blend, BlendMode::Multiply);
}

#[test]
fn vector_layer_policy_and_fidelity() {
    let mut d = doc();
    add(&mut d, 1, LayerKind::Vector(PixelContent::empty()), Placement::Top);
    add(&mut d, 2, raster(), Placement::Top);
    // stroke without geometry is not vector-representable -> rejected on vector layer
    assert!(d.add_stroke(stroke(1, 1)).is_err());
    assert!(d.mark_raster_edit(lid(1), "fill").is_err());
    d.add_stroke(stroke(2, 2)).unwrap();
    assert_eq!(d.vector_fidelity(lid(2)), Some(VectorFidelity::PartiallyVector));
    d.mark_raster_edit(lid(2), "fill").unwrap();
    assert_eq!(d.vector_fidelity(lid(2)), Some(VectorFidelity::RasterAuthoritative { reasons: vec!["fill".into()] }));
    assert_eq!(d.vector_fidelity(lid(1)), Some(VectorFidelity::Exact));
}

#[test]
fn validate_detects_corruption_via_from_parts() {
    let d = doc();
    let l = Layer { id: lid(1), parent: None, props: LayerProps::named("a"), kind: raster() };
    // orphan layer: present in map but not in tree
    let r = Document::from_parts(DocumentParts {
        id: d.id,
        size: d.size(),
        dpi: 300.0,
        color: color(),
        root: vec![],
        layers: vec![l.clone()],
        strokes: vec![],
        selection: None,
        active_layer: None,
    });
    assert!(r.is_err());
    // duplicate reference
    let r = Document::from_parts(DocumentParts {
        id: d.id,
        size: d.size(),
        dpi: 300.0,
        color: color(),
        root: vec![lid(1), lid(1)],
        layers: vec![l.clone()],
        strokes: vec![],
        selection: None,
        active_layer: None,
    });
    assert!(r.is_err());
    // dangling reference
    let r = Document::from_parts(DocumentParts {
        id: d.id,
        size: d.size(),
        dpi: 300.0,
        color: color(),
        root: vec![lid(2)],
        layers: vec![],
        strokes: vec![],
        selection: None,
        active_layer: None,
    });
    assert!(r.is_err());
    // self-cycle through group child list
    let g = Layer {
        id: lid(5),
        parent: None,
        props: LayerProps::named("g"),
        kind: LayerKind::Group(GroupContent { children: vec![lid(5)], pass_through: false, collapsed: false }),
    };
    let r = Document::from_parts(DocumentParts {
        id: d.id,
        size: d.size(),
        dpi: 300.0,
        color: color(),
        root: vec![lid(5)],
        layers: vec![g],
        strokes: vec![],
        selection: None,
        active_layer: None,
    });
    assert!(r.is_err());
    // valid
    let r = Document::from_parts(DocumentParts {
        id: d.id,
        size: d.size(),
        dpi: 300.0,
        color: color(),
        root: vec![lid(1)],
        layers: vec![l],
        strokes: vec![],
        selection: None,
        active_layer: None,
    });
    assert!(r.is_ok());
}

/// PNT-009: 1000 random operations with interleaved undo/redo restore exact states.
#[test]
fn history_exact_over_1000_operations() {
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut d = doc();
    add(&mut d, 1, raster(), Placement::Top);
    add(&mut d, 2, raster(), Placement::Top);
    let mut current = Arc::new(d);
    let mut h = History::new(HistoryConfig { max_entries: 10_000, max_bytes: usize::MAX });
    // Linear timeline of states for checking: timeline[cursor] == current
    let mut timeline: Vec<Arc<Document>> = vec![current.clone()];
    let mut cursor = 0usize;
    for i in 0..1000u32 {
        let r = next() % 10;
        if r < 6 {
            let before = current.clone();
            let mut d = (*current).clone();
            let layer = if next() % 2 == 0 { lid(1) } else { lid(2) };
            let x = (next() % 256) as i32;
            let y = (next() % 128) as i32;
            let v = (next() % 60000) as u16;
            d.layer_mut(layer).unwrap().pixels_mut().unwrap().tiles.put_pixel(x, y, [v / 2, v / 3, v / 4, v]);
            if i % 7 == 0 {
                d.apply_patch(
                    layer,
                    &LayerPatch { opacity: Some((next() % 100) as f32 / 100.0), ..Default::default() },
                )
                .unwrap();
            }
            h.record(format!("op{i}"), before, &d);
            current = Arc::new(d);
            timeline.truncate(cursor + 1);
            timeline.push(current.clone());
            cursor += 1;
        } else if r < 8 {
            if let Some((_, s)) = h.undo(current.clone()) {
                current = s;
                cursor -= 1;
            } else {
                assert_eq!(cursor, 0);
            }
        } else if let Some((_, s)) = h.redo(current.clone()) {
            current = s;
            cursor += 1;
        } else {
            assert_eq!(cursor, timeline.len() - 1);
        }
        assert_eq!(*current, *timeline[cursor], "state mismatch at op {i}");
    }
    // unwind everything
    while let Some((_, s)) = h.undo(current.clone()) {
        current = s;
        cursor -= 1;
        assert_eq!(*current, *timeline[cursor]);
    }
    assert_eq!(cursor, 0);
}

#[test]
fn history_budget_trims_oldest_and_reports() {
    let mut d = doc();
    add(&mut d, 1, raster(), Placement::Top);
    let mut current = Arc::new(d);
    // each op touches a new tile -> 32 KiB unique per entry; budget fits ~3
    let mut h = History::new(HistoryConfig { max_entries: 100, max_bytes: 3 * 32 * 1024 + 10 });
    for i in 0..8 {
        let before = current.clone();
        let mut d = (*current).clone();
        d.layer_mut(lid(1)).unwrap().pixels_mut().unwrap().tiles.put_pixel(1, 1, [i, i, i, 1000]);
        h.record("paint", before, &d);
        current = Arc::new(d);
    }
    let s = h.stats();
    assert!(s.undo_len <= 4, "{s:?}");
    assert!(s.trimmed_entries >= 4);
    assert!(s.bytes <= 3 * 32 * 1024 + 10);
}
