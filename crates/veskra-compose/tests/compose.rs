//! Compositing semantics: PNT-006, PNT-007, COL-006, DOC-01..03 (CPU oracle part of CLR-03).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use veskra_compose::{
    ComposeOptions, composite_grid, composite_rect, composite_subtree, render_display_rgba8, sample_composite,
    thumbnail_rgba8,
};
use veskra_core::{BlendMode, BlendSpace, ColorRgba, ErrorCode, RectI};
use veskra_doc::{Document, MaskGrid, TileGrid};
use veskra_testkit::Rng;

const ONE: RectI = RectI::new(0, 0, 1, 1);

fn one_px(doc: &Document) -> Px {
    at(&composite_rect(doc, ONE, &ComposeOptions::default()), ONE, 0, 0)
}

fn two_layer(b: Px, s: Px, mode: BlendMode, opacity: f32) -> Px {
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, ONE, b, props("b", BlendMode::Normal, 1.0));
    add_fill(&mut d, 2, None, ONE, s, props("s", mode, opacity));
    one_px(&d)
}

fn random_premul_grid(rng: &mut Rng, r: RectI) -> TileGrid {
    let mut g = TileGrid::new();
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            let a = rng.below(65536) as u16;
            let c = |rng: &mut Rng| (rng.below(a as u64 + 1)) as u16;
            let p = [c(rng), c(rng), c(rng), a];
            g.put_pixel(x, y, p);
        }
    }
    g
}

// ---------------------------------------------------------------- identity / Normal

#[test]
fn single_layer_identity_is_bit_exact() {
    let mut rng = Rng::new(42);
    let r = RectI::new(0, 0, 150, 70); // spans partial tiles
    let g = random_premul_grid(&mut rng, r);
    let mut d = doc(150, 70);
    add_grid(&mut d, 1, None, g.clone(), None, props("l", BlendMode::Normal, 1.0));
    for mode in BlendMode::ALL {
        d.layer_mut(id(1)).unwrap().props.blend = mode; // any mode over transparent == source
        let out = composite_rect(&d, r, &ComposeOptions::default());
        assert_eq!(out, g.read_rect(r), "{mode:?}");
    }
    let grid = composite_grid(&d, &ComposeOptions::default());
    assert_eq!(grid.read_rect(r), g.read_rect(r));
}

#[test]
fn normal_two_layers_alpha_levels() {
    let back_opaque = px8(200, 100, 30, 255);
    let back_semi = px8(200, 100, 30, 128);
    let src_color = (50u8, 180u8, 240u8);
    for back in [back_opaque, back_semi] {
        // alpha 0: backdrop unchanged (bit exact)
        assert_eq!(two_layer(back, [0; 4], BlendMode::Normal, 1.0), back);
        // alpha 1: source replaces (bit exact)
        let s1 = px8(src_color.0, src_color.1, src_color.2, 255);
        assert_eq!(two_layer(back, s1, BlendMode::Normal, 1.0), s1);
        // alpha 1/255 and 0.5: formula, exact to the f64 reference rounding
        for a in [1u8, 128] {
            let s = px8(src_color.0, src_color.1, src_color.2, a);
            let want = q16(ref_blend(to_f(back), to_f(s), BlendMode::Normal));
            assert_close(two_layer(back, s, BlendMode::Normal, 1.0), want, 0, &format!("alpha {a}"));
        }
    }
    // Hand-computed: white at alpha 32768/65535 over opaque black = 32768 gray.
    assert_eq!(two_layer([0, 0, 0, 65535], [32768; 4], BlendMode::Normal, 1.0), [32768, 32768, 32768, 65535]);
}

#[test]
fn opacity_multiplies_source() {
    let back = px8(10, 20, 30, 255);
    let s = px8(250, 128, 0, 200);
    for op in [0.0f32, 0.25, 0.5, 1.0] {
        let want = q16(ref_blend(to_f(back), scale(to_f(s), op as f64), BlendMode::Normal));
        assert_close(two_layer(back, s, BlendMode::Normal, op), want, 0, &format!("opacity {op}"));
    }
}

// ---------------------------------------------------------------- blend modes (PNT-007)

/// Exact expected u16 outputs computed with rational arithmetic (Python `fractions`) from the
/// W3C formula; the rounding margin of every case is ≥ 0.0076 LSB, so they are asserted exactly.
#[test]
fn blend_modes_match_rational_reference_table() {
    let b1 = [51400, 25700, 7710, 65535]; // (200,100,30) opaque
    let b2 = [20000, 10000, 3000, 40000]; // semi-transparent backdrop
    let s1 = [12850, 46260, 61680, 65535]; // (50,180,240) opaque
    let s2 = [3000, 11000, 15000, 16000]; // semi-transparent source
    let s3 = [257, 257, 257, 257]; // white at alpha 1/255
    use BlendMode::*;
    #[rustfmt::skip]
    let table: &[(BlendMode, Px, Px, Px)] = &[
        (Normal, b1, s2, [41851, 30425, 20828, 65535]),
        (Normal, b1, s3, [51455, 25856, 7937, 65535]),
        (Normal, b2, s1, [12850, 46260, 61680, 65535]),
        (Normal, b2, s2, [18117, 18559, 17268, 46234]),
        (Normal, b2, s3, [20179, 10218, 3245, 40100]),
        (Multiply, b1, s1, [10078, 18141, 7256, 65535]),
        (Multiply, b1, s2, [41204, 23739, 7592, 65535]),
        (Multiply, b1, s3, [51400, 25700, 7710, 65535]),
        (Multiply, b2, s1, [8928, 25084, 26856, 65535]),
        (Multiply, b2, s2, [17202, 13523, 8799, 46234]),
        (Multiply, b2, s3, [20100, 10100, 3100, 40100]),
        (Screen, b1, s1, [54172, 53819, 62134, 65535]),
        (Screen, b1, s2, [52047, 32386, 20945, 65535]),
        (Screen, b1, s3, [51455, 25856, 7937, 65535]),
        (Screen, b2, s1, [28928, 49201, 61856, 65535]),
        (Screen, b2, s2, [22084, 19322, 17313, 46234]),
        (Screen, b2, s3, [20179, 10218, 3245, 40100]),
        (Overlay, b1, s1, [42808, 36282, 14513, 65535]),
        (Overlay, b1, s2, [49243, 28053, 9357, 65535]),
        (Overlay, b1, s3, [51455, 25801, 7740, 65535]),
        (Overlay, b2, s1, [12850, 32142, 29680, 65535]),
        (Overlay, b2, s2, [18117, 15202, 9485, 46234]),
        (Overlay, b2, s3, [20179, 10139, 3112, 40100]),
    ];
    for &(mode, b, s, want) in table {
        assert_eq!(two_layer(b, s, mode, 1.0), want, "{mode:?} {b:?} {s:?}");
        // The independent f64 straight-form reference agrees too.
        assert_eq!(q16(ref_blend(to_f(b), to_f(s), mode)), want, "ref {mode:?}");
    }
    // Any mode over a transparent backdrop is the source itself.
    for mode in BlendMode::ALL {
        for s in [s1, s2, s3] {
            assert_eq!(two_layer([0; 4], s, mode, 1.0), s);
        }
    }
}

#[test]
fn blend_modes_hand_computed_opaque() {
    // 8-bit-exact inputs (v·257); values derived by hand from B(Cb, Cs).
    let g = |v: u8| px8(v, v, v, 255);
    // Multiply: (128/255)·(64/255) · 65535 = 8192·257/255 = 8256.25 → 8256
    assert_eq!(two_layer(g(128), g(64), BlendMode::Multiply, 1.0)[0], 8256);
    // Screen black/white: 1
    assert_eq!(two_layer(g(0), g(255), BlendMode::Screen, 1.0)[0], 65535);
    // Screen 64/64: (2·64·255 − 64²)/255² · 65535 = 28544·257/255 = 28767.87 → 28768
    assert_eq!(two_layer(g(64), g(64), BlendMode::Screen, 1.0)[0], 28768);
    // Overlay with Cb = 64/255 (≤ .5): 2·Cb·Cs, Cs = 200/255 → 25600·257/255 = 25800.78 → 25801
    assert_eq!(two_layer(g(64), g(200), BlendMode::Overlay, 1.0)[0], 25801);
    // Overlay with Cb = 1: screen(Cs, 1) = 1; Cb = 0: 0
    assert_eq!(two_layer(g(255), g(10), BlendMode::Overlay, 1.0)[0], 65535);
    assert_eq!(two_layer(g(0), g(240), BlendMode::Overlay, 1.0)[0], 0);
    // Multiply by white is identity; Screen with black is identity.
    for v in [0u8, 1, 77, 128, 254, 255] {
        assert_eq!(two_layer(g(v), g(255), BlendMode::Multiply, 1.0), g(v));
        assert_eq!(two_layer(g(v), g(0), BlendMode::Screen, 1.0), g(v));
    }
}

#[test]
fn blend_modes_random_against_reference() {
    let mut rng = Rng::new(7);
    let rp = |rng: &mut Rng| {
        let a = rng.below(65536) as u16;
        [rng.below(a as u64 + 1) as u16, rng.below(a as u64 + 1) as u16, rng.below(a as u64 + 1) as u16, a]
    };
    for _ in 0..400 {
        let (b, s) = (rp(&mut rng), rp(&mut rng));
        for mode in BlendMode::ALL {
            let want = q16(ref_blend(to_f(b), to_f(s), mode));
            assert_close(two_layer(b, s, mode, 1.0), want, 1, &format!("{mode:?} {b:?} {s:?}"));
        }
    }
}

// ---------------------------------------------------------------- blend space

#[test]
fn linear_vs_gamma_on_half_white_over_black() {
    for (space, want) in [(BlendSpace::GammaEncoded, 32768u16), (BlendSpace::Linear, 48192)] {
        let mut d = doc_space(4, 4, space);
        add_fill(&mut d, 1, None, ONE, [0, 0, 0, 65535], props("black", BlendMode::Normal, 1.0));
        add_fill(&mut d, 2, None, ONE, [32768; 4], props("white50", BlendMode::Normal, 1.0));
        // GammaEncoded: 0.5 encoded. Linear: 0.5 linear light → sRGB 0.7354 → 48192.
        assert_eq!(one_px(&d), [want, want, want, 65535], "{space:?}");
    }
    // Opaque single layer is the identity in both spaces (decode/re-encode within 1 code).
    let mut rng = Rng::new(3);
    let r = RectI::new(0, 0, 64, 4);
    let mut g = TileGrid::new();
    for y in 0..4 {
        for x in 0..64 {
            g.put_pixel(x, y, [rng.below(65536) as u16, rng.below(65536) as u16, rng.below(65536) as u16, 65535]);
        }
    }
    let mut d = doc_space(64, 4, BlendSpace::Linear);
    add_grid(&mut d, 1, None, g.clone(), None, props("l", BlendMode::Normal, 1.0));
    let out = composite_rect(&d, r, &ComposeOptions::default());
    for (a, b) in out.iter().zip(g.read_rect(r).iter()) {
        assert!(a.abs_diff(*b) <= 1);
    }
}

// ---------------------------------------------------------------- groups (DOC-01)

#[test]
fn isolated_vs_pass_through_with_multiply_child() {
    let red = px8(220, 40, 40, 255);
    let gray = px8(128, 128, 128, 255);
    let build = |pass: bool| {
        let mut d = doc(4, 4);
        add_fill(&mut d, 1, None, ONE, red, props("bg", BlendMode::Normal, 1.0));
        add_group(&mut d, 2, None, pass, props("g", BlendMode::Normal, 1.0));
        add_fill(&mut d, 3, Some(2), ONE, gray, props("mul", BlendMode::Multiply, 1.0));
        one_px(&d)
    };
    // Isolated: the Multiply child has a transparent backdrop inside the group ⇒ acts as Normal.
    assert_eq!(build(false), gray);
    // Pass-through: the child multiplies the document backdrop.
    let want = q16(ref_blend(to_f(red), to_f(gray), BlendMode::Multiply));
    assert_eq!(build(true), want);
    assert_ne!(build(true), build(false));
}

#[test]
fn group_opacity_isolated_and_pass_through() {
    let back = px8(0, 0, 255, 255);
    let low = px8(255, 0, 0, 255);
    let top = px8(0, 255, 0, 255);
    let build = |pass: bool, gmode: BlendMode| {
        let mut d = doc(4, 4);
        add_fill(&mut d, 1, None, ONE, back, props("bg", BlendMode::Normal, 1.0));
        add_group(&mut d, 2, None, pass, props("g", gmode, 0.5));
        add_fill(&mut d, 3, Some(2), ONE, low, props("low", BlendMode::Normal, 1.0));
        add_fill(&mut d, 4, Some(2), ONE, top, props("top", BlendMode::Normal, 1.0));
        one_px(&d)
    };
    // Isolated: group result is `top` (opaque) → applied at 50%; `low` never shows.
    let want = q16(ref_blend(to_f(back), scale(to_f(top), 0.5), BlendMode::Normal));
    assert_eq!(build(false, BlendMode::Normal), want);
    assert_eq!(want[0], 0);
    // Pass-through: lerp(before, after, 0.5) — identical here; group blend mode ignored.
    assert_eq!(build(true, BlendMode::Normal), want);
    assert_eq!(build(true, BlendMode::Screen), want);
    // Isolated group blend mode is applied to the group result.
    let want_screen = q16(ref_blend(to_f(back), scale(to_f(top), 0.5), BlendMode::Screen));
    assert_eq!(build(false, BlendMode::Screen), want_screen);
}

#[test]
fn pass_through_opacity_is_premultiplied_lerp() {
    // Semi-transparent backdrop, Multiply child at 0.6 alpha, group opacity 0.3.
    let back = [20000, 10000, 3000, 40000];
    let child = [12000, 30000, 6000, 40000];
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, ONE, back, props("bg", BlendMode::Normal, 1.0));
    add_group(&mut d, 2, None, true, props("g", BlendMode::Normal, 0.3));
    add_fill(&mut d, 3, Some(2), ONE, child, props("c", BlendMode::Multiply, 1.0));
    let before = to_f(back);
    let after = ref_blend(before, to_f(child), BlendMode::Multiply);
    let lerp: [f64; 4] = std::array::from_fn(|i| before[i] + (after[i] - before[i]) * 0.3f32 as f64);
    assert_close(one_px(&d), q16(lerp), 1, "pass-through lerp");
}

#[test]
fn nested_groups_multiply_opacity_and_respect_visibility() {
    let c = px8(10, 200, 90, 255);
    let mut d = doc(4, 4);
    add_group(&mut d, 1, None, false, props("outer", BlendMode::Normal, 0.5));
    add_group(&mut d, 2, Some(1), false, props("inner", BlendMode::Normal, 0.5));
    add_fill(&mut d, 3, Some(2), ONE, c, props("leaf", BlendMode::Normal, 1.0));
    let got = one_px(&d);
    let want = q16(scale(scale(to_f(c), 0.5), 0.5));
    assert_close(got, want, 1, "nested 0.5·0.5");
    assert!(got[3].abs_diff(16384) <= 1);
    // Nested pass-through inside isolated: same result on transparent.
    d.apply_patch(id(2), &veskra_doc::LayerPatch { pass_through: Some(true), ..Default::default() }).unwrap();
    assert_close(one_px(&d), want, 1, "nested pass-through");
    // Hidden outer group hides the subtree; include_hidden shows it.
    set_visible(&mut d, 1, false);
    assert_eq!(one_px(&d), [0; 4]);
    let o = ComposeOptions { include_hidden: true, ..Default::default() };
    assert_close(at(&composite_rect(&d, ONE, &o), ONE, 0, 0), want, 1, "include_hidden");
}

#[test]
fn group_paint_order_bottom_to_top() {
    let a = px8(255, 0, 0, 255);
    let b = px8(0, 0, 255, 255);
    let mut d = doc(4, 4);
    add_group(&mut d, 1, None, false, props("g", BlendMode::Normal, 1.0));
    add_fill(&mut d, 2, Some(1), ONE, a, props("bottom", BlendMode::Normal, 1.0));
    add_fill(&mut d, 3, Some(1), ONE, b, props("top", BlendMode::Normal, 1.0));
    assert_eq!(one_px(&d), b);
    d.move_layer(id(3), veskra_doc::Placement::Below(id(2))).unwrap();
    assert_eq!(one_px(&d), a);
}

// ---------------------------------------------------------------- clipping (DOC-03)

/// psd-tools `_apply_clip_layers` reference: clippers over the base's own pixels, keep straight
/// color, restore base alpha, then base opacity·mask, then base mode onto the backdrop.
fn ref_clip(backdrop: Px, base: Px, base_k: f64, base_mode: BlendMode, clips: &[(Px, f64, BlendMode)]) -> [f64; 4] {
    let p = to_f(base);
    let mut q = p;
    for &(c, k, m) in clips {
        q = ref_blend(q, scale(to_f(c), k), m);
    }
    let clipped = if q[3] > 0.0 { scale(q, p[3] / q[3]) } else { [0.0; 4] };
    ref_blend(to_f(backdrop), scale(clipped, base_k), base_mode)
}

#[test]
fn clipping_restricts_to_base_alpha() {
    let backdrop = px8(255, 255, 255, 255);
    let base = px8(0, 0, 255, 128);
    let red = px8(255, 0, 0, 255);
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, RectI::new(0, 0, 2, 1), backdrop, props("paper", BlendMode::Normal, 1.0));
    add_fill(&mut d, 2, None, RectI::new(0, 0, 1, 1), base, props("base", BlendMode::Normal, 1.0));
    add_fill(&mut d, 3, None, RectI::new(0, 0, 2, 1), red, props("clip", BlendMode::Normal, 1.0));
    set_clip(&mut d, 3, true);
    let r = RectI::new(0, 0, 2, 1);
    let out = composite_rect(&d, r, &ComposeOptions::default());
    // Inside the base: red at the base's alpha over white.
    let want = q16(ref_clip(backdrop, base, 1.0, BlendMode::Normal, &[(red, 1.0, BlendMode::Normal)]));
    assert_close(at(&out, r, 0, 0), want, 1, "clip inside base");
    assert_eq!(at(&out, r, 0, 0), q16(ref_blend(to_f(backdrop), scale(to_f(red), 128.0 / 255.0), BlendMode::Normal)));
    // Outside the base: the clipper is invisible.
    assert_eq!(at(&out, r, 1, 0), backdrop);
}

#[test]
fn clipping_semitransparent_clipper_multiply_on_semitransparent_base() {
    let backdrop = px8(30, 160, 60, 255);
    let base = [20000, 30000, 5000, 40000];
    let c1 = [9000, 2000, 12000, 20000];
    let c2 = [30000, 30000, 1000, 50000];
    for base_mode in [BlendMode::Normal, BlendMode::Multiply] {
        let mut d = doc(4, 4);
        add_fill(&mut d, 1, None, ONE, backdrop, props("bg", BlendMode::Normal, 1.0));
        add_fill(&mut d, 2, None, ONE, base, props("base", base_mode, 0.8));
        add_fill(&mut d, 3, None, ONE, c1, props("c1", BlendMode::Multiply, 0.7));
        add_fill(&mut d, 4, None, ONE, c2, props("c2", BlendMode::Overlay, 1.0));
        set_clip(&mut d, 3, true);
        set_clip(&mut d, 4, true);
        let want = ref_clip(
            backdrop,
            base,
            0.8f32 as f64,
            base_mode,
            &[(c1, 0.7f32 as f64, BlendMode::Multiply), (c2, 1.0, BlendMode::Overlay)],
        );
        assert_close(one_px(&d), q16(want), 1, &format!("clip run, base {base_mode:?}"));
    }
}

#[test]
fn clipping_hidden_base_hides_clippers_and_hidden_clipper_keeps_run() {
    let base = px8(0, 0, 255, 255);
    let c1 = px8(255, 0, 0, 255);
    let c2 = px8(0, 255, 0, 128);
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, RectI::new(0, 0, 1, 1), base, props("base", BlendMode::Normal, 1.0));
    add_fill(&mut d, 2, None, RectI::new(0, 0, 2, 1), c1, props("c1", BlendMode::Normal, 1.0));
    add_fill(&mut d, 3, None, RectI::new(0, 0, 2, 1), c2, props("c2", BlendMode::Normal, 1.0));
    set_clip(&mut d, 2, true);
    set_clip(&mut d, 3, true);
    let r = RectI::new(0, 0, 2, 1);
    // Hidden middle clipper: c2 still clips to `base`, not drawn at x = 1.
    set_visible(&mut d, 2, false);
    let out = composite_rect(&d, r, &ComposeOptions::default());
    assert_eq!(at(&out, r, 0, 0), q16(ref_blend(to_f(base), to_f(c2), BlendMode::Normal)));
    assert_eq!(at(&out, r, 1, 0), [0; 4]);
    // Hidden base: nothing at all.
    set_visible(&mut d, 1, false);
    assert!(composite_rect(&d, r, &ComposeOptions::default()).iter().all(|&v| v == 0));
    // Filtered base behaves like hidden.
    set_visible(&mut d, 1, true);
    let f = |l: veskra_core::LayerId| l != id(1);
    let o = ComposeOptions { layer_filter: Some(&f), ..Default::default() };
    assert!(composite_rect(&d, r, &o).iter().all(|&v| v == 0));
}

#[test]
fn clipping_respects_base_mask_and_base_opacity() {
    let base = px8(0, 0, 255, 255);
    let clip = px8(255, 0, 0, 255);
    let mut mask = MaskGrid::new_full();
    mask.set(1, 0, 0);
    mask.set(2, 0, 64);
    let mut d = doc(4, 4);
    add_grid(&mut d, 1, None, grid_fill(RectI::new(0, 0, 3, 1), base), Some(mask), props("b", BlendMode::Normal, 0.5));
    add_fill(&mut d, 2, None, RectI::new(0, 0, 3, 1), clip, props("c", BlendMode::Normal, 1.0));
    set_clip(&mut d, 2, true);
    let r = RectI::new(0, 0, 3, 1);
    let out = composite_rect(&d, r, &ComposeOptions::default());
    // x=0: clip fully covers base, base opacity 0.5 fades the clipped result too.
    assert_close(at(&out, r, 0, 0), q16(scale(to_f(clip), 0.5)), 1, "opacity");
    // x=1: base mask 0 hides the clipper.
    assert_eq!(at(&out, r, 1, 0), [0; 4]);
    // x=2: mask 64/255 · opacity 0.5.
    assert_close(at(&out, r, 2, 0), q16(scale(to_f(clip), 0.5 * 64.0 / 255.0)), 1, "mask");
}

#[test]
fn clipping_without_base_and_on_groups() {
    let a = px8(255, 0, 0, 255);
    let b = px8(0, 255, 0, 128);
    // Bottom-most clipper with no base composites normally.
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, ONE, a, props("orphan", BlendMode::Normal, 1.0));
    set_clip(&mut d, 1, true);
    assert_eq!(one_px(&d), a);
    // Two leading clippers: both normal.
    add_fill(&mut d, 2, None, ONE, b, props("orphan2", BlendMode::Normal, 1.0));
    set_clip(&mut d, 2, true);
    assert_eq!(one_px(&d), q16(ref_blend(to_f(a), to_f(b), BlendMode::Normal)));
    // clip_to_below on a group is ignored (Photoshop has no clipping groups) and the group
    // itself acts as a base for the clipper above it.
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, RectI::new(1, 0, 2, 1), a, props("under", BlendMode::Normal, 1.0));
    add_group(&mut d, 2, None, false, props("g", BlendMode::Normal, 1.0));
    add_fill(&mut d, 3, Some(2), RectI::new(0, 0, 1, 1), b, props("gc", BlendMode::Normal, 1.0));
    set_clip(&mut d, 2, true);
    add_fill(&mut d, 4, None, RectI::new(0, 0, 2, 1), a, props("clip", BlendMode::Normal, 1.0));
    set_clip(&mut d, 4, true);
    let r = RectI::new(0, 0, 2, 1);
    let out = composite_rect(&d, r, &ComposeOptions::default());
    // Group drawn at x=0 (not clipped away), clipper restricted to the group's alpha.
    assert_close(at(&out, r, 0, 0), q16(scale(to_f(a), 128.0 / 255.0)), 1, "group base");
    assert_eq!(at(&out, r, 1, 0), a); // only `under`
}

// ---------------------------------------------------------------- masks, overrides, filters

#[test]
fn layer_mask_scales_source() {
    let back = px8(255, 255, 255, 255);
    let ink = px8(20, 20, 20, 255);
    let mut mask = MaskGrid::new_empty();
    for (x, v) in [0u8, 1, 64, 128, 255].iter().enumerate() {
        mask.set(x as i32, 0, *v);
    }
    let mut d = doc(8, 2);
    add_fill(&mut d, 1, None, RectI::new(0, 0, 8, 2), back, props("paper", BlendMode::Normal, 1.0));
    add_grid(
        &mut d,
        2,
        None,
        grid_fill(RectI::new(0, 0, 8, 2), ink),
        Some(mask),
        props("ink", BlendMode::Multiply, 1.0),
    );
    let r = RectI::new(0, 0, 8, 2);
    let out = composite_rect(&d, r, &ComposeOptions::default());
    for (x, v) in [0u8, 1, 64, 128, 255].iter().enumerate() {
        let want = q16(ref_blend(to_f(back), scale(to_f(ink), *v as f64 / 255.0), BlendMode::Multiply));
        assert_close(at(&out, r, x as i32, 0), want, 1, &format!("mask {v}"));
    }
    // Outside the mask's tiles (empty default) and on the next row: hidden.
    assert_eq!(at(&out, r, 6, 0), back);
    assert_eq!(at(&out, r, 0, 1), back);
}

#[test]
fn overrides_replace_layer_tiles_and_keep_props() {
    let a = px8(255, 0, 0, 255);
    let live = grid_fill(RectI::new(1, 0, 2, 1), px8(0, 0, 255, 255));
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, RectI::new(0, 0, 1, 1), a, props("l", BlendMode::Normal, 0.5));
    let r = RectI::new(0, 0, 2, 1);
    let ov = [(id(1), &live)];
    let o = ComposeOptions { overrides: &ov, ..Default::default() };
    let out = composite_rect(&d, r, &o);
    assert_eq!(at(&out, r, 0, 0), [0; 4]); // committed pixel replaced
    assert_close(at(&out, r, 1, 0), q16(scale(to_f(px8(0, 0, 255, 255)), 0.5)), 1, "override with opacity");
    // Overrides for unknown ids/groups are ignored.
    let ov2 = [(id(99), &live)];
    let o2 = ComposeOptions { overrides: &ov2, ..Default::default() };
    assert_eq!(composite_rect(&d, r, &o2), composite_rect(&d, r, &ComposeOptions::default()));
}

#[test]
fn layer_filter_equals_hiding() {
    let mut rng = Rng::new(11);
    let r = RectI::new(0, 0, 70, 20);
    let mut d = doc(70, 20);
    for n in 1..=4u128 {
        let mode = BlendMode::ALL[n as usize % 4];
        add_grid(&mut d, n, None, random_premul_grid(&mut rng, r), None, props("l", mode, 0.8));
    }
    let f = |l: veskra_core::LayerId| l != id(3);
    let o = ComposeOptions { layer_filter: Some(&f), ..Default::default() };
    let filtered = composite_rect(&d, r, &o);
    set_visible(&mut d, 3, false);
    assert_eq!(filtered, composite_rect(&d, r, &ComposeOptions::default()));
    // include_hidden brings it back.
    set_visible(&mut d, 3, true);
    let shown = composite_rect(&d, r, &ComposeOptions::default());
    set_visible(&mut d, 3, false);
    let o = ComposeOptions { include_hidden: true, ..Default::default() };
    assert_eq!(composite_rect(&d, r, &o), shown);
    assert_ne!(filtered, shown);
}

#[test]
fn composite_subtree_ignores_mode_clip_and_ancestors() {
    let back = px8(255, 255, 0, 255);
    let c = px8(0, 100, 200, 255);
    let mut mask = MaskGrid::new_full();
    mask.set(0, 0, 128);
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, ONE, back, props("bg", BlendMode::Normal, 1.0));
    add_group(&mut d, 2, None, false, props("g", BlendMode::Screen, 0.5));
    add_grid(&mut d, 3, Some(2), grid_fill(ONE, c), Some(mask), props("leaf", BlendMode::Multiply, 0.5));
    set_clip(&mut d, 3, true);
    let o = ComposeOptions::default();
    // Leaf: own opacity·mask, no Multiply, no group opacity.
    let leaf = at(&composite_subtree(&d, id(3), ONE, &o), ONE, 0, 0);
    assert_close(leaf, q16(scale(to_f(c), 0.5 * 128.0 / 255.0)), 1, "leaf subtree");
    // Group: its own opacity applies but not its Screen mode or the backdrop.
    let g = at(&composite_subtree(&d, id(2), ONE, &o), ONE, 0, 0);
    assert_close(g, q16(scale(to_f(c), 0.25 * 128.0 / 255.0)), 1, "group subtree");
    // Hidden ancestor is ignored; hidden node itself needs include_hidden.
    set_visible(&mut d, 2, false);
    assert_eq!(at(&composite_subtree(&d, id(3), ONE, &o), ONE, 0, 0), leaf);
    set_visible(&mut d, 3, false);
    assert_eq!(at(&composite_subtree(&d, id(3), ONE, &o), ONE, 0, 0), [0; 4]);
    let oh = ComposeOptions { include_hidden: true, ..Default::default() };
    assert_eq!(at(&composite_subtree(&d, id(3), ONE, &oh), ONE, 0, 0), leaf);
    // Unknown id: transparent buffer of the right size.
    assert_eq!(composite_subtree(&d, id(77), RectI::new(0, 0, 3, 2), &o), vec![0; 24]);
    // Pass-through group alone == isolated Normal group alone.
    let mut d2 = doc(4, 4);
    add_group(&mut d2, 1, None, true, props("g", BlendMode::Overlay, 0.4));
    add_fill(&mut d2, 2, Some(1), ONE, c, props("a", BlendMode::Normal, 1.0));
    add_fill(&mut d2, 3, Some(1), ONE, px8(9, 9, 9, 100), props("b", BlendMode::Multiply, 1.0));
    let pt = composite_subtree(&d2, id(1), ONE, &o);
    d2.apply_patch(id(1), &veskra_doc::LayerPatch { pass_through: Some(false), ..Default::default() }).unwrap();
    assert_eq!(composite_subtree(&d2, id(1), ONE, &o), pt);
}

// ---------------------------------------------------------------- outputs

#[test]
fn display_rgba8_premultiplied_and_matches_convert() {
    let mut rng = Rng::new(5);
    let r = RectI::new(0, 0, 100, 90);
    let mut d = doc(100, 90);
    for n in 1..=3u128 {
        add_grid(&mut d, n, None, random_premul_grid(&mut rng, r), None, props("l", BlendMode::ALL[n as usize], 0.9));
    }
    let sub = RectI::new(7, 5, 77, 66);
    let mut out = vec![0u8; sub.area() as usize * 4];
    render_display_rgba8(&d, sub, &ComposeOptions::default(), &mut out).unwrap();
    for p in out.chunks_exact(4) {
        assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    }
    let mut want = vec![0u8; out.len()];
    veskra_color::convert::premul16_to_premul8(&composite_rect(&d, sub, &ComposeOptions::default()), &mut want);
    assert_eq!(out, want);
    let e = render_display_rgba8(&d, sub, &ComposeOptions::default(), &mut [0u8; 5]).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidArgument);
    render_display_rgba8(&d, RectI::EMPTY, &ComposeOptions::default(), &mut []).unwrap();
}

#[test]
fn thumbnail_size_aspect_and_color() {
    let mut d = doc(300, 100);
    let b = d.bounds();
    add_fill(&mut d, 1, None, b, px8(200, 100, 50, 255), props("f", BlendMode::Normal, 1.0));
    let (w, h, px) = thumbnail_rgba8(&d, 64);
    assert_eq!((w, h), (64, 21));
    assert_eq!(px.len(), 64 * 21 * 4);
    assert!(px.chunks_exact(4).all(|p| p == [200, 100, 50, 255]));
    // Never upscales; tall docs; zero edge.
    assert_eq!(thumbnail_rgba8(&d, 1000).0, 300);
    let tall = doc(10, 500);
    let (w, h, _) = thumbnail_rgba8(&tall, 100);
    assert_eq!((w, h), (2, 100));
    assert_eq!(thumbnail_rgba8(&d, 0), (0, 0, vec![]));
    // Box filter in premultiplied space: half red-opaque / half transparent → straight red,
    // alpha 50% (no dark fringe from averaging transparent black).
    let mut d = doc(2, 1);
    add_fill(&mut d, 1, None, ONE, px8(255, 0, 0, 255), props("f", BlendMode::Normal, 1.0));
    let (_, _, px) = thumbnail_rgba8(&d, 1);
    assert_eq!(px, vec![255, 0, 0, 128]);
}

#[test]
fn sample_composite_straight() {
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, ONE, px8(200, 100, 50, 128), props("f", BlendMode::Normal, 1.0));
    let c = sample_composite(&d, 0, 0);
    assert_eq!(c.to_rgba8(), [200, 100, 50, 128]);
    assert_eq!(sample_composite(&d, 1, 1), ColorRgba::TRANSPARENT);
    assert_eq!(sample_composite(&d, -1, 0), ColorRgba::TRANSPARENT);
    assert_eq!(sample_composite(&d, 4, 0), ColorRgba::TRANSPARENT);
}

/// COL-006: semi-transparent edges keep their source color after compositing and conversion
/// to straight 8-bit (no dark/colored halo), for alpha 0, 1/255, 0.01, 0.5, 1 and through
/// opacity/mask/group paths.
#[test]
fn no_halos_on_semitransparent_edges() {
    let (r8, g8, b8) = (200u8, 30u8, 90u8);
    let alphas: Vec<f32> = vec![0.0, 1.0 / 255.0, 0.01, 0.1, 0.5, 0.75, 1.0];
    let w = alphas.len() as i32;
    let mut g = TileGrid::new();
    for (x, a) in alphas.iter().enumerate() {
        g.put_pixel(x as i32, 0, pxf(ColorRgba::from_rgba8(r8, g8, b8, 255).with_alpha(*a)));
    }
    let r = RectI::new(0, 0, w, 1);
    for (opacity, in_group, mode) in [
        (1.0, false, BlendMode::Normal),
        (0.5, false, BlendMode::Normal),
        (1.0, true, BlendMode::Screen),
        (0.7, true, BlendMode::Multiply),
    ] {
        let mut d = doc(w as u32, 1);
        let parent = if in_group {
            add_group(&mut d, 10, None, false, props("g", BlendMode::Normal, 1.0));
            Some(10)
        } else {
            None
        };
        add_grid(&mut d, 1, parent, g.clone(), None, props("edge", mode, opacity));
        let px = composite_rect(&d, r, &ComposeOptions::default());
        assert_premul(&px);
        let mut s8 = vec![0u8; px.len()];
        veskra_color::convert::premul16_to_straight8(&px, &mut s8);
        for (x, p) in s8.chunks_exact(4).enumerate() {
            if alphas[x] == 0.0 {
                assert_eq!(p, [0, 0, 0, 0]);
                continue;
            }
            if p[3] == 0 {
                continue;
            }
            for (c, want) in [r8, g8, b8].iter().enumerate() {
                assert!(p[c].abs_diff(*want) <= 1, "halo at alpha {} opacity {opacity}: {p:?}", alphas[x]);
            }
        }
    }
}

// ---------------------------------------------------------------- tiling / determinism

#[test]
fn rect_and_grid_are_consistent_and_tile_aligned_independent() {
    let mut rng = Rng::new(99);
    let (w, h) = (200u32, 130u32); // not multiples of 64
    let full = RectI::from_size(w, h);
    let mut d = doc(w, h);
    add_group(&mut d, 50, None, true, props("pt", BlendMode::Normal, 0.7));
    for n in 1..=5u128 {
        let parent = if n % 2 == 0 { Some(50) } else { None };
        let r = RectI::new(-10 + n as i32 * 13, -5 + n as i32 * 7, 150 + n as i32 * 20, 140);
        add_grid(
            &mut d,
            n,
            parent,
            random_premul_grid(&mut rng, r),
            None,
            props("l", BlendMode::ALL[n as usize % 4], 0.85),
        );
    }
    set_clip(&mut d, 3, true);
    let o = ComposeOptions::default();
    let all = composite_rect(&d, full, &o);
    assert_premul(&all);
    assert_eq!(all, composite_rect(&d, full, &o), "deterministic");
    // Sub-rects (unaligned, crossing tile borders) equal crops of the full composite.
    for sub in [RectI::new(3, 7, 131, 99), RectI::new(63, 63, 65, 65), RectI::new(150, 100, 200, 130)] {
        let part = composite_rect(&d, sub, &o);
        for y in sub.y0..sub.y1 {
            for x in sub.x0..sub.x1 {
                assert_eq!(at(&part, sub, x, y), at(&all, full, x, y));
            }
        }
    }
    // Outside the canvas is transparent even though layers have content there.
    let outside = RectI::new(-10, -5, 0, 10);
    assert!(composite_rect(&d, outside, &o).iter().all(|&v| v == 0));
    // Grid == rect over the canvas, nothing stored outside it.
    let g = composite_grid(&d, &o);
    assert_eq!(g.read_rect(full), all);
    let gb = g.content_bounds();
    assert!(gb.x0 >= 0 && gb.y0 >= 0 && gb.x1 <= w as i32 && gb.y1 <= h as i32);
    // Empty / oversized rects.
    assert!(composite_rect(&d, RectI::EMPTY, &o).is_empty());
    assert!(composite_rect(&d, RectI::new(0, 0, 10_000, 10_000), &o).is_empty());
}

#[test]
fn locked_and_alpha_locked_do_not_affect_composite() {
    let mut d = doc(4, 4);
    add_fill(&mut d, 1, None, ONE, px8(1, 2, 3, 200), props("l", BlendMode::Normal, 1.0));
    let before = one_px(&d);
    d.layer_mut(id(1)).unwrap().props.locked = true;
    d.layer_mut(id(1)).unwrap().props.alpha_locked = true;
    assert_eq!(one_px(&d), before);
}

#[test]
fn empty_document_is_transparent_and_sparse() {
    let d = doc(500, 300);
    assert_eq!(composite_grid(&d, &ComposeOptions::default()).tile_count(), 0);
    let mut d = doc(500, 300);
    add_fill(&mut d, 1, None, RectI::new(70, 70, 75, 75), px8(1, 1, 1, 255), props("dot", BlendMode::Normal, 1.0));
    add_fill(&mut d, 2, None, RectI::EMPTY, px8(1, 1, 1, 255), props("empty", BlendMode::Normal, 1.0));
    assert_eq!(composite_grid(&d, &ComposeOptions::default()).tile_count(), 1);
}

#[test]
fn parallel_bands_equal_single_threaded_small_rects() {
    // 640×320 = 50 tiles → the full-rect call is split over threads; 2-tile rects (< 8 tiles)
    // run on the caller's thread. Results must be bit-identical.
    let mut rng = Rng::new(1234);
    let full = RectI::new(0, 0, 640, 320);
    let mut d = doc(640, 320);
    add_group(&mut d, 20, None, false, props("g", BlendMode::Screen, 0.75));
    for n in 1..=6u128 {
        let parent = if n > 3 { Some(20) } else { None };
        let r = RectI::new(n as i32 * 37, n as i32 * 19, 640 - n as i32 * 41, 320 - n as i32 * 7);
        add_grid(
            &mut d,
            n,
            parent,
            random_premul_grid(&mut rng, r),
            None,
            props("l", BlendMode::ALL[n as usize % 4], 0.9),
        );
    }
    set_clip(&mut d, 2, true);
    let o = ComposeOptions::default();
    let all = composite_rect(&d, full, &o);
    for ty in 0..5 {
        for tx in (0..10).step_by(2) {
            let r = RectI::new(tx * 64, ty * 64, tx * 64 + 128, ty * 64 + 64);
            let part = composite_rect(&d, r, &o);
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    assert_eq!(at(&part, r, x, y), at(&all, full, x, y), "({x},{y})");
                }
            }
        }
    }
    assert_eq!(composite_grid(&d, &o).read_rect(full), all);
    // layer_filter is evaluated on the caller's thread only and still honoured in parallel.
    let calls = std::cell::Cell::new(0);
    let f = |l: veskra_core::LayerId| {
        calls.set(calls.get() + 1);
        l != id(1)
    };
    let filtered = composite_rect(&d, full, &ComposeOptions { layer_filter: Some(&f), ..Default::default() });
    assert_eq!(calls.get(), 6);
    set_visible(&mut d, 1, false);
    assert_eq!(filtered, composite_rect(&d, full, &o));
}
