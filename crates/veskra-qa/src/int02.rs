//! INT-02-style 12-layer ink / flats / shadow document, built only through engine JSON commands.
//!
//! Layer stack (top-most first, 12 nodes):
//! ```text
//! Texture        raster  overlay 50 %   charcoal strokes
//! Highlights     raster  screen 70 %    soft airbrush
//! Character      group   90 %
//!   Eyes         vector                 technical pen
//!   Hair shadow  raster  multiply, clipped to Hair flats
//!   Hair flats   raster                 ellipse selection fill
//! Ink accents    raster                 brush pen (pressure)
//! Lines          vector                 technical pen: closed box + closed ellipse
//! Shadow         raster  multiply 80 %, clipped to Flats, soft brush
//! Flats          raster  alpha-locked   bucket fills (sample merged) + rect selection fill + locked shading fill
//! Sketch         raster  35 %, hidden   rough pencil
//! Background     raster                 paper fill
//! ```
//! Geometry is defined on a 1024 × 768 canvas and scaled to the requested size.

use crate::drive::{Driver, ellipse, line, wave};
use serde_json::json;

/// Probe points (document px) used by S2/S3 checks.
#[derive(Debug, Clone)]
pub struct Int02 {
    pub width: u32,
    pub height: u32,
    /// Inside the closed box (bucket-filled skin, then left half alpha-locked shade).
    pub box_inside: (i32, i32),
    /// Inside the box, right half (skin flat, shadow stroke passes here).
    pub box_right_on_shadow: (i32, i32),
    /// Outside every flat but inside the alpha-lock selection rect and on the shadow stroke row.
    pub outside_flats_on_shadow: (i32, i32),
    /// Inside the closed ellipse (blue flat).
    pub ellipse_inside: (i32, i32),
    /// Outside the box, near its left edge (must not be bucket-filled).
    pub box_outside: (i32, i32),
    pub skin: [u8; 3],
    pub shade: [u8; 3],
    pub blue: [u8; 3],
    pub paper: [u8; 3],
    /// `layer.list` order (paint order reversed; a group is listed after its children).
    pub names_top_first: Vec<&'static str>,
}

const SKIN: &str = "#E8B796";
const SHADE: &str = "#D99A7A";
const BLUE: &str = "#6A8EC9";
const PAPER: &str = "#F4F1EA";

fn rgb(hex: &str) -> [u8; 3] {
    let c = veskra_core::ColorRgba::from_hex(hex).unwrap_or(veskra_core::ColorRgba::BLACK).to_rgba8();
    [c[0], c[1], c[2]]
}

/// Build the document on a fresh `Driver` (creates it). Every step is one engine command.
pub fn build(d: &mut Driver, width: u32, height: u32) -> Int02 {
    let sx = width as f32 / 1024.0;
    let sy = height as f32 / 768.0;
    let s = |x: f32, y: f32| (x * sx, y * sy);
    let sc = |v: Vec<(f32, f32, f32)>| v.into_iter().map(|(x, y, p)| (x * sx, y * sy, p)).collect::<Vec<_>>();
    let k = sx.min(sy);

    d.ok(json!({"cmd": "document.create", "width": width, "height": height, "dpi": 300, "background": PAPER}));
    let sketch = d.info()["activeLayer"].as_str().map(str::to_owned).unwrap_or_default();
    d.ok(json!({"cmd": "layer.rename", "id": sketch, "name": "Sketch"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pencil.rough"}));
    d.ok(json!({"cmd": "color.set", "hex": "#5A5A66"}));
    d.stroke(&sc(wave(180.0, 640.0, 120.0, 14.0, 90)));
    d.stroke(&sc(ellipse((760.0, 300.0), (135.0, 115.0), 80, 0.05, 0.4)));
    d.stroke(&sc(line((190.0, 560.0), (610.0, 570.0), 60, 0.3, 0.6)));

    // Line art on a vector layer: closed box (segments overlap at the corners) + closed ellipse.
    let lines = d.add_layer("vector", "Lines", json!({"at": "top"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.technical"}));
    d.ok(json!({"cmd": "brush.set", "size": (6.0 * k).max(2.0)}));
    d.ok(json!({"cmd": "color.set", "hex": "#1E1B2E"}));
    let (x0, y0, x1, y1) = (200.0, 150.0, 600.0, 550.0);
    d.stroke(&sc(line((x0 - 12.0, y0), (x1 + 12.0, y0), 60, 0.8, 0.8)));
    d.stroke(&sc(line((x1, y0 - 12.0), (x1, y1 + 12.0), 60, 0.8, 0.8)));
    d.stroke(&sc(line((x1 + 12.0, y1), (x0 - 12.0, y1), 60, 0.8, 0.8)));
    d.stroke(&sc(line((x0, y1 + 12.0), (x0, y0 - 12.0), 60, 0.8, 0.8)));
    d.stroke(&sc(ellipse((760.0, 300.0), (120.0, 100.0), 120, 0.08, 0.8)));

    // Artist hides the rough before flatting (it stays hidden: exercised as a hidden layer).
    d.ok(json!({"cmd": "layer.set", "id": sketch, "visible": false, "opacity": 0.35}));

    let flats = d.add_layer("raster", "Flats", json!({"at": "below", "id": lines}));
    d.ok(json!({"cmd": "color.set", "hex": SKIN}));
    let (bx, by) = s(400.0, 350.0);
    d.ok(json!({"cmd": "edit.fill", "x": bx, "y": by, "tolerance": 0.1, "sampleMerged": true}));
    d.ok(json!({"cmd": "color.set", "hex": BLUE}));
    let (ex, ey) = s(760.0, 300.0);
    d.ok(json!({"cmd": "edit.fill", "x": ex, "y": ey, "tolerance": 0.1, "sampleMerged": true}));
    let (r0, r1) = (s(660.0, 520.0), s(940.0, 720.0));
    d.ok(json!({"cmd": "selection.rect", "x0": r0.0, "y0": r0.1, "x1": r1.0, "y1": r1.1}));
    d.ok(json!({"cmd": "color.set", "hex": "#7BAA6A"}));
    d.ok(json!({"cmd": "edit.fill_selection"}));
    d.ok(json!({"cmd": "selection.clear"}));

    // Shadow: clipped to Flats, Multiply.
    let shadow = d.add_layer("raster", "Shadow", json!({"at": "above", "id": flats}));
    d.ok(json!({"cmd": "layer.set", "id": shadow, "clip": true, "blend": "multiply", "opacity": 0.8}));
    d.ok(json!({"cmd": "brush.select", "preset": "round.soft"}));
    d.ok(json!({"cmd": "brush.set", "size": 60.0 * k}));
    d.ok(json!({"cmd": "color.set", "hex": "#8A6F9E"}));
    d.stroke(&sc(line((120.0, 450.0), (960.0, 450.0), 120, 0.9, 0.9)));

    // Alpha-locked shading on Flats: a rect selection larger than the box, left half.
    d.ok(json!({"cmd": "layer.set", "id": flats, "alphaLocked": true}));
    d.select(&flats);
    let (a0, a1) = (s(100.0, 100.0), s(400.0, 600.0));
    d.ok(json!({"cmd": "selection.rect", "x0": a0.0, "y0": a0.1, "x1": a1.0, "y1": a1.1}));
    d.ok(json!({"cmd": "color.set", "hex": SHADE}));
    d.ok(json!({"cmd": "edit.fill_selection"}));
    d.ok(json!({"cmd": "selection.clear"}));

    // Ink accents (raster, pressure brush pen).
    let ink = d.add_layer("raster", "Ink accents", json!({"at": "above", "id": lines}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.brush"}));
    d.ok(json!({"cmd": "color.set", "hex": "#2B1D14"}));
    d.stroke(&sc(wave(230.0, 570.0, 200.0, 18.0, 140)));
    d.stroke(&sc(wave(660.0, 880.0, 430.0, 10.0, 90)));

    // Character group: hair flats + clipped multiply shadow + vector eyes; group opacity 90 %.
    let group = d.add_layer("group", "Character", json!({"at": "top"}));
    let hair = d.add_layer("raster", "Hair flats", json!({"at": "into", "group": group}));
    let (h0, h1) = (s(300.0, 60.0), s(520.0, 210.0));
    d.ok(json!({"cmd": "selection.ellipse", "x0": h0.0, "y0": h0.1, "x1": h1.0, "y1": h1.1}));
    d.ok(json!({"cmd": "color.set", "hex": "#4A3328"}));
    d.ok(json!({"cmd": "edit.fill_selection"}));
    d.ok(json!({"cmd": "selection.clear"}));
    let hair_shadow = d.add_layer("raster", "Hair shadow", json!({"at": "above", "id": hair}));
    d.ok(json!({"cmd": "layer.set", "id": hair_shadow, "clip": true, "blend": "multiply"}));
    d.ok(json!({"cmd": "brush.select", "preset": "round.hard"}));
    d.ok(json!({"cmd": "brush.set", "size": 30.0 * k}));
    d.ok(json!({"cmd": "color.set", "hex": "#6B5A80"}));
    d.stroke(&sc(line((280.0, 170.0), (540.0, 120.0), 60, 0.7, 0.7)));
    d.add_layer("vector", "Eyes", json!({"at": "above", "id": hair_shadow}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.technical"}));
    d.ok(json!({"cmd": "brush.set", "size": (5.0 * k).max(2.0)}));
    d.ok(json!({"cmd": "color.set", "hex": "#101018"}));
    d.stroke(&sc(line((340.0, 260.0), (370.0, 262.0), 12, 0.6, 1.0)));
    d.stroke(&sc(line((430.0, 262.0), (460.0, 260.0), 12, 1.0, 0.6)));
    d.ok(json!({"cmd": "layer.set", "id": group, "opacity": 0.9}));

    let hi = d.add_layer("raster", "Highlights", json!({"at": "top"}));
    d.ok(json!({"cmd": "layer.set", "id": hi, "blend": "screen", "opacity": 0.7}));
    d.ok(json!({"cmd": "brush.select", "preset": "round.soft"}));
    d.ok(json!({"cmd": "brush.set", "size": 40.0 * k}));
    d.ok(json!({"cmd": "color.set", "hex": "#FFF2C0"}));
    d.stroke(&sc(wave(260.0, 520.0, 300.0, 30.0, 80)));

    let tex = d.add_layer("raster", "Texture", json!({"at": "top"}));
    d.ok(json!({"cmd": "layer.set", "id": tex, "blend": "overlay", "opacity": 0.5}));
    d.ok(json!({"cmd": "brush.select", "preset": "charcoal"}));
    d.ok(json!({"cmd": "color.set", "hex": "#3C3C46"}));
    d.stroke(&sc(wave(120.0, 900.0, 640.0, 20.0, 160)));
    d.stroke(&sc(wave(150.0, 880.0, 90.0, 12.0, 160)));
    let _ = ink;

    let pt = |x: f32, y: f32| ((x * sx) as i32, (y * sy) as i32);
    Int02 {
        width,
        height,
        box_inside: pt(300.0, 380.0),
        box_right_on_shadow: pt(520.0, 450.0),
        outside_flats_on_shadow: pt(150.0, 450.0),
        ellipse_inside: pt(760.0, 360.0),
        box_outside: pt(170.0, 380.0),
        skin: rgb(SKIN),
        shade: rgb(SHADE),
        blue: rgb(BLUE),
        paper: rgb(PAPER),
        names_top_first: vec![
            "Texture",
            "Highlights",
            "Eyes",
            "Hair shadow",
            "Hair flats",
            "Character",
            "Ink accents",
            "Lines",
            "Shadow",
            "Flats",
            "Sketch",
            "Background",
        ],
    }
}
