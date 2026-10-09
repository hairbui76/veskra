//! PNT-002 (AUTO part): one golden per built-in preset — a fixed seeded S-curve with pressure
//! taper on a 256×128 canvas, converted to 8-bit straight RGBA by `veskra-color::convert`.
//! Paint presets draw on a transparent canvas; erasers erase from an opaque orange canvas
//! (erasing a transparent canvas would produce an empty, meaningless golden).
//! Bless deliberately with `VESKRA_BLESS=1`.

mod common;

use common::*;
use veskra_brush::{StrokeMode, builtin_presets};
use veskra_core::ColorRgba;
use veskra_raster::TileGrid;

#[test]
fn pnt002_builtin_preset_goldens() {
    let samples = s_curve(120, 2026);
    for p in builtin_presets() {
        let base = if p.eraser { filled_base(px16(ColorRgba::from_rgba8(240, 140, 40, 255))) } else { TileGrid::new() };
        let fin = render(&p, &samples, 77, base, ctx(StrokeMode::Paint), &[3]);
        assert!(premul_invariant_holds(&fin.tiles), "{}", p.id);
        let rgba = straight8(&fin.tiles, canvas());
        let changed = if p.eraser {
            rgba.chunks_exact(4).filter(|px| px[3] < 255).count()
        } else {
            rgba.chunks_exact(4).filter(|px| px[3] > 0).count()
        };
        assert!(changed > 500, "{}: stroke too small ({changed} px)", p.id);
        // Bit-exact on one platform; the small allowance covers libm differences across targets.
        veskra_testkit::golden::assert_matches(&format!("brush_{}", p.id), W, H, &rgba, 1, 8);
    }
}
