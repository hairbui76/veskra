# Veskra cross-crate contracts (ARC-01)

**Status:** Accepted by ORCH for implementation wave 1–3 (2026-10-09). Changing a published
signature listed here requires an ADR or an ORCH-approved amendment to this file.
`veskra-core` (see rustdoc) is the frozen base; this file describes the public surface each
other crate must expose so crates can be built in parallel.

## 0. Global rules

* Edition 2024, toolchain pinned by `rust-toolchain.toml`. `unsafe_code = "deny"` everywhere
  except `veskra-ffi`, which documents every `unsafe` block.
* No `unwrap()`/`expect()`/indexing panics on data derived from files or the FFI. Use typed errors.
* All public raster buffers carry or are documented by a `PixelDescriptor`.
* Determinism: given identical inputs (samples, seed, preset), output pixels are bit-identical
  across runs on the same platform. Avoid `HashMap` iteration order in output paths
  (use `BTreeMap` or sort keys).
* Units: document pixels; time in ns; angles in radians.

## 1. Pixel conventions (ADR-005)

* Authoritative layer pixels: `[u16; 4]` RGBA, **premultiplied**, transfer-encoded in the document
  working profile (sRGB for v1), full range `0..=65535`. Invariant: `r,g,b <= a`.
* Masks / selections / stroke coverage: `u8` coverage `0..=255` (`PixelDescriptor::MASK_U8`)
  or `u16` coverage for stroke accumulation buffers (internal to brush).
* `ColorRgba` (core): straight alpha, encoded, `f32` in `[0,1]`.
* Rounding: float → u16 uses `(v * 65535.0 + 0.5).floor()` clamped; u16 → u8 uses
  `(v * 255 + 32767) / 65535` (exact round-half-up). Conversions live in `veskra-color::convert`.

## 2. `veskra-raster` (REN)

```rust
pub type Px16 = [u16; 4];                      // premultiplied RGBA16
pub const TILE: u32 = veskra_core::TILE_SIZE;  // 64
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TileCoord { pub tx: i32, pub ty: i32 }
pub struct Tile { /* 64*64*4 u16 */ }
impl Tile {
    pub fn new_transparent() -> Tile;
    pub fn pixels(&self) -> &[u16];            // len 64*64*4, row-major RGBA
    pub fn pixels_mut(&mut self) -> &mut [u16];
    pub fn get(&self, x: u32, y: u32) -> Px16; pub fn set(&mut self, x: u32, y: u32, p: Px16);
    pub fn is_transparent(&self) -> bool;
    pub fn from_pixels(v: &[u16]) -> Result<Tile, RasterError>;
}
/// Sparse copy-on-write grid. Clone is O(tiles) Arc clones; mutation copies only touched tiles.
#[derive(Clone, Default)]
pub struct TileGrid { /* BTreeMap<TileCoord, Arc<Tile>> */ }
impl TileGrid {
    pub fn new() -> Self;
    pub fn get_pixel(&self, x: i32, y: i32) -> Px16;
    pub fn put_pixel(&mut self, x: i32, y: i32, p: Px16);
    pub fn tile(&self, c: TileCoord) -> Option<&Arc<Tile>>;
    pub fn tile_mut(&mut self, c: TileCoord) -> &mut Tile;          // allocates transparent, Arc::make_mut
    pub fn set_tile(&mut self, c: TileCoord, t: Arc<Tile>); pub fn remove_tile(&mut self, c: TileCoord);
    pub fn iter_tiles(&self) -> impl Iterator<Item = (TileCoord, &Arc<Tile>)>;   // sorted by (ty, tx)
    pub fn tile_count(&self) -> usize;
    pub fn read_rect(&self, r: RectI) -> Vec<u16>;                   // r.width*r.height*4, transparent outside
    pub fn write_rect(&mut self, r: RectI, data: &[u16]) -> Result<(), RasterError>;
    pub fn content_bounds(&self) -> RectI;                           // tight bounds of non-transparent pixels
    pub fn prune_transparent(&mut self);
    pub fn clear(&mut self);
    pub fn byte_size(&self) -> usize;                                // tiles * tile bytes
    pub fn tiles_in_rect(r: RectI) -> impl Iterator<Item = TileCoord>;
    pub fn shares_tile_with(&self, other: &TileGrid, c: TileCoord) -> bool;   // Arc::ptr_eq (undo accounting)
}
/// Sparse u8 coverage grid with the same COW semantics (selections, layer masks, fill regions).
#[derive(Clone)] pub struct MaskGrid { /* default value outside tiles: 0 or 255 */ }
impl MaskGrid {
    pub fn new_empty() -> Self;   pub fn new_full() -> Self;        // full = 255 everywhere
    pub fn get(&self, x: i32, y: i32) -> u8; pub fn set(&mut self, x: i32, y: i32, v: u8);
    pub fn bounds(&self) -> RectI; pub fn is_empty(&self) -> bool;
    pub fn invert(&mut self, within: RectI);
    pub fn combine(&mut self, other: &MaskGrid, op: MaskOp);      // Replace/Add/Subtract/Intersect
    pub fn read_rect(&self, r: RectI) -> Vec<u8>;
}
pub enum MaskOp { Replace, Add, Subtract, Intersect }
pub mod select {  // EDIT-02, anti-aliased coverage rasterization (nonzero winding)
    pub fn rect(r: RectF, clip: RectI) -> MaskGrid;
    pub fn ellipse(bounds: RectF, clip: RectI) -> MaskGrid;
    pub fn polygon(points: &[PointF], clip: RectI) -> MaskGrid;   // lasso
}
pub mod fill {    // EDIT-01, bounded scanline fill, no recursion
    pub struct FillOptions { pub tolerance: f32 /*0..1*/, pub contiguous: bool, pub antialias: bool }
    pub fn region(reference: &TileGrid, seed: (i32, i32), opts: &FillOptions, bounds: RectI,
                  selection: Option<&MaskGrid>) -> Result<MaskGrid, RasterError>;
    pub fn apply(target: &mut TileGrid, region: &MaskGrid, color: ColorRgba, alpha_lock: bool) -> RectI;
}
pub mod transform { // EDIT-03, bilinear on premultiplied data
    pub fn transform_pixels(src: &TileGrid, region: Option<&MaskGrid>, m: Affine, clip: RectI) -> TileGrid;
    pub fn transform_mask(src: &MaskGrid, m: Affine, clip: RectI) -> MaskGrid;
}
pub mod ops {
    pub fn color_to_px16(c: ColorRgba) -> Px16;   // premultiply in encoded space, documented rounding
    pub fn src_over(dst: Px16, src: Px16) -> Px16;
    pub fn erase(dst: Px16, coverage: u16) -> Px16; // destination-out
    pub fn cut(src: &mut TileGrid, region: &MaskGrid) -> TileGrid;   // move selection content out
    pub fn paste_over(dst: &mut TileGrid, src: &TileGrid) -> RectI;
}
#[derive(Debug, thiserror::Error)] pub enum RasterError { ... }   // + impl From<RasterError> for VeskraError
```

## 3. `veskra-color` (CLR)

```rust
pub mod transfer { pub fn srgb_to_linear(v: f32) -> f32; pub fn linear_to_srgb(v: f32) -> f32;
                   pub fn srgb_u16_to_linear_lut() -> &'static [f32; 65536]; /* etc. */ }
pub mod icc {
    pub struct IccProfile { /* parsed matrix/TRC RGB or gray profile, original bytes kept */ }
    impl IccProfile {
        pub fn parse(bytes: &[u8]) -> Result<IccProfile, ColorError>;   // bounded, v2 + v4, rejects LUT-only with typed error
        pub fn srgb() -> Arc<IccProfile>;           // Veskra-generated sRGB IEC61966-2.1 v4 matrix/TRC profile
        pub fn display_p3() -> Arc<IccProfile>;     // generated, for gated P3 tests only
        pub fn bytes(&self) -> &[u8]; pub fn id(&self) -> ProfileId; pub fn description(&self) -> &str;
        pub fn to_xyz_d50_matrix(&self) -> [[f64; 3]; 3]; pub fn is_srgb_equivalent(&self) -> bool;
    }
}
pub mod transform {
    pub enum RenderingIntent { Perceptual, RelativeColorimetric, Saturation, AbsoluteColorimetric }
    pub struct RgbTransform { /* src -> PCS(XYZ D50) -> dst; clipping documented */ }
    impl RgbTransform { pub fn new(src: &IccProfile, dst: &IccProfile, intent: RenderingIntent) -> Result<Self, ColorError>;
        pub fn is_identity(&self) -> bool;  pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3];      // encoded in → encoded out
        pub fn apply_rgba8_straight(&self, px: &mut [u8]); pub fn apply_rgba16_premul(&self, px: &mut [u16]); }
}
pub mod lab { pub fn xyz_d50_to_lab(xyz: [f64; 3]) -> [f64; 3]; pub fn srgb8_to_lab_d50(rgb: [u8; 3]) -> [f64; 3];
              pub fn ciede2000(a: [f64; 3], b: [f64; 3]) -> f64; }
pub mod convert {   // the ONLY place alpha/depth conversions are implemented
    pub fn premul16_to_straight8(src: &[u16], dst: &mut [u8]);   // RGBA, no halos: alpha-0 → (0,0,0,0)
    pub fn premul16_to_premul8(src: &[u16], dst: &mut [u8]);     // Android ARGB_8888 memory layout
    pub fn straight8_to_premul16(src: &[u8], dst: &mut [u16]);
    pub fn straight_f32_to_premul16(c: ColorRgba) -> [u16; 4];
    pub fn premul16_to_linear_premul_f32(p: [u16;4]) -> [f32;4];  pub fn linear_premul_f32_to_premul16(p: [f32;4]) -> [u16;4];
}
pub struct DocumentColor { pub profile: Arc<IccProfile>, pub assumed: bool, pub blend_space: BlendSpace }
#[derive(Debug, thiserror::Error)] pub enum ColorError { ... }   // + From<ColorError> for VeskraError
```

## 4. `veskra-input` (AND)

Raw pointer events arrive from Android packed (see §8) and are arbitrated in Rust so the logic
is testable headless.

```rust
pub enum PointerAction { Down, Move, Up, Cancel, PointerDown, PointerUp, HoverEnter, HoverMove, HoverExit }
pub struct RawPointerEvent { pub action: PointerAction, pub pointer_id: i32, pub tool: ToolType,
    pub x: f32, pub y: f32,                       // VIEW coordinates (px)
    pub t_ns: u64, pub pressure: Option<f32>, pub tilt: Option<f32>, pub orientation: Option<f32>,
    pub touch_major: Option<f32>, pub touch_minor: Option<f32>, pub buttons: u32,
    pub historical: bool, pub predicted: bool, pub canceled_flag: bool /* Android FLAG_CANCELED (palm) */ }
pub struct InputConfig { pub finger_draws: bool, pub palm_major_threshold_px: f32,
    pub pressure_curve: PressureCurve, pub stylus_button_action: ButtonAction, ... }
pub enum InputAction {
    StrokeBegin { tool: ToolType, eraser: bool },
    StrokeSamples(Vec<StrokeSample>),             // CONFIRMED only, document coords, ordered, monotonic t
    StrokePrediction(Vec<StrokeSample>),          // PREDICTED only; replaces previous prediction
    StrokeCommit, StrokeCancel,
    Hover(Option<PointF>),                        // document coords
    ViewChanged(ViewTransform), GestureEnd,
    StylusButton { pressed: bool, action: ButtonAction },
    TwoFingerTap, ThreeFingerTap,                 // undo / redo shortcuts
}
pub struct ViewTransform { pub zoom: f64, pub rotation: f64, pub pan: PointF, pub mirror: bool,
                           pub view_w: f64, pub view_h: f64 }
impl ViewTransform { pub fn doc_to_view(&self) -> Affine; pub fn view_to_doc(&self) -> Affine;
                     pub fn fit(doc: SizeU, view_w: f64, view_h: f64) -> Self; }
pub struct InputArbiter { ... }
impl InputArbiter { pub fn new(cfg: InputConfig) -> Self;
    pub fn handle(&mut self, ev: &RawPointerEvent, view: &ViewTransform) -> Vec<InputAction>;
    pub fn reset(&mut self) -> Vec<InputAction>; }   // lifecycle pause: cancels/commits pending per policy
pub struct PressureCurve { /* monotone cubic through control points, maps [0,1]→[0,1] */ }
```
Stroke state machine `Idle → Hover → Begin → Streaming → Commit|Cancel → Idle`; exactly one
`StrokeCommit` or `StrokeCancel` per `StrokeBegin`. Two-finger gestures never emit stroke actions;
a finger touching down during a finger-stroke cancels that stroke (not commits).

## 5. `veskra-vector` (BRH)

```rust
pub enum PathEl { MoveTo(PointF), LineTo(PointF), CubicTo(PointF, PointF, PointF), Close }
#[derive(Clone, Default, Serialize, Deserialize)] pub struct BezPath { pub els: Vec<PathEl> }
impl BezPath { pub fn bounds(&self) -> RectF; pub fn transform(&self, m: &Affine) -> BezPath;
    pub fn flatten(&self, tolerance: f64) -> Vec<Vec<PointF>>;           // polylines per subpath
    pub fn to_svg_d(&self, decimals: u8) -> String; pub fn to_pdf_ops(&self, decimals: u8) -> String; }
pub fn fit_curve(points: &[PointF], max_error: f64) -> BezPath;           // error-bounded open centerline
pub struct WidthPoint { pub p: PointF, pub width: f64 }
pub struct OutlineOptions { pub tolerance: f64, pub cap: Cap /* Round|Butt */, pub min_width: f64 }
pub fn variable_width_outline(pts: &[WidthPoint], opts: &OutlineOptions) -> BezPath;  // closed, nonzero fill
pub fn max_distance_to_path(path: &BezPath, pts: &[PointF]) -> f64;      // PNT-010 metric
```

## 6. `veskra-doc` (ORCH) — summary; rustdoc is normative

`Document { id, size, dpi, color: DocColorMeta, root: Vec<LayerId>, layers: BTreeMap<LayerId, Arc<Layer>>,
strokes: BTreeMap<StrokeId, Arc<StrokeRecord>>, ... }` with `Layer { props: LayerProps, kind: LayerKind }`
where `LayerKind::{Raster { tiles: TileGrid, mask: Option<MaskGrid> }, Vector { tiles, ... }, Group { children, .. }}`.
Cloning a `Document` is cheap (structural sharing); history keeps `Arc<Document>` snapshots.

## 7. Export (`veskra-io`, FMT) — see `veskra_core::export`

```rust
pub trait Exporter {
    fn format(&self) -> ExportFormat;
    fn preflight(&self, doc: &Document, opts: &ExportOptions) -> ExportPreflight;
    fn export(&self, doc: &Document, opts: &ExportOptions, out: &mut dyn std::io::Write,
              cancel: &CancelToken) -> Result<ExportReport, VeskraError>;
}
```

## 8. JNI boundary (AND, `veskra-ffi` ↔ `apps/android/kit`)

Kotlin class `org.veskra.kit.NativeBridge` (package namespace provisional per README "About the
name"; see ADR-002). All functions are `external` statics; handles are `Long` (opaque, never 0).

| Kotlin signature | Purpose |
|---|---|
| `nativeVersion(): String` | `"veskra-ffi <semver> api=<N>"` |
| `nativeCreateSession(): Long` | new engine session; 0 on failure |
| `nativeDestroySession(h: Long)` | idempotent; double destroy is a no-op |
| `nativeExecute(h: Long, json: String): String` | command JSON → `{"ok":…}` or `{"error":{code,message,recoverable}}` |
| `nativeInputBatch(h: Long, ints: IntArray, floats: FloatArray, longs: LongArray, count: Int): IntArray` | packed pointer events; returns `[flags, nRects, x0,y0,x1,y1, …]` |
| `nativeGetView(h: Long, out: FloatArray)` | 9 floats: doc→view affine `a,b,c,d,e,f`, zoom, rotation, mirror(0/1) |
| `nativeSetViewport(h: Long, w: Int, h: Int)` | view size in px (rotation/resize) |
| `nativeRenderRegion(h: Long, x: Int, y: Int, w: Int, hgt: Int, dst: java.nio.ByteBuffer): Boolean` | direct buffer, `w*hgt*4` bytes RGBA8 premultiplied sRGB (ARGB_8888 memory order) of the current composite incl. live stroke |

Packed event layout per event `i`: `ints[i*5 .. i*5+5] = [action, pointerId, toolType, buttons, flags]`
(`flags`: bit0 historical, bit1 predicted, bit2 FLAG_CANCELED); `floats[i*8 .. i*8+8] = [x, y, pressure,
tilt, orientation, touchMajor, touchMinor, reserved]` (NaN = axis not reported);
`longs[i] = eventTimeNanos`. `action`/`toolType` use Android `MotionEvent` constant values.
Return flags: bit0 canvas dirty, bit1 view changed, bit2 stroke committed, bit3 history changed.

## 9. Brush system v2 (BRH / FMT, ADR-019) — amendment of 2026-10-12

Cross-crate changes; rustdoc is normative.

* **`veskra-brush`** — preset schema 3 (`docs/COMMANDS.md` "Schema-3 preset JSON").
  `BrushPreset` now has `category: BrushCategory`, `source: BrushSource`, `folder`, `tip: Tip`,
  `jitter: Jitter`, `texture: Option<Texture>`, `dynamics: Dynamics`, `wet_edges`, and keeps
  `schema, id, version, name, size_px, opacity, flow, spacing, accumulation, eraser,
  vector_eligible, stabilization`. **Not additive:** the flat schema-2 Rust fields (`kind`,
  `hardness`, `min_size_ratio`, `pressure_size/opacity/flow`, `tilt_size`, `grain`) and the types
  `BrushKind`, `TiltSize`, `Grain` were removed from the Rust API (their JSON still loads through
  migration; no other crate used them). Additive: `asset::{BrushAsset, AssetStore, ResolvedAssets,
  MaskChannel, ImportedMask, mask_hash}`, `StrokeRenderer::begin_resolved(preset, assets, color,
  seed, base, ctx)` and `asset_fallbacks()`, `builtin_assets()`, `builtin_asset_hash()`,
  `BUILTIN_IDS` (now 30), `LEGACY_BUILTIN_IDS`, `load_preset_value`, `DabInput`,
  `BrushPreset::{dab_diameter, dab_alpha_at, asset_refs, to_json_compact}`,
  `ResponseCurve::{new_shape, validate_shape, is_monotone}`, `interop::infinite_painter`,
  `BrushError::{InvalidAsset, AssetTooLarge, MissingAsset}`. `StrokeRenderer::begin` keeps its
  signature (resolves built-in assets only). Determinism contract unchanged: identical
  (samples, preset, assets, color, seed, context) → identical pixels on one platform; schema-2-only
  presets are byte-identical to the pre-v2 engine.
* **`veskra-doc`** — additive: `StrokeRecord::brush_assets: Vec<String>` (+
  `with_brush_assets`), `Document::{insert_brush_asset, brush_asset, iter_brush_assets,
  referenced_brush_assets}`, `MAX_BRUSH_ASSET_BYTES`, `MAX_DOCUMENT_BRUSH_ASSETS`,
  `MAX_STROKE_BRUSH_ASSETS`. `DocumentParts` is unchanged (assets are inserted after
  `from_parts`; `to_parts` does not carry them).
* **`veskra-format`** — `CURRENT_SCHEMA` = 2 (schemas 1..=2 read natively, `MIN_NATIVE_SCHEMA`
  = 1); the writer emits schema 2 only for documents with brush assets. Additive:
  `FormatWarning::MissingBrushAssets`, `zip::ZipPolicy` + `ZipReader::parse_with`
  (`ZipReader::parse` = `ZipPolicy::BUNDLE`, unchanged behaviour).
* **`veskra-engine`** — additive: `brushlib` module (library, `.vbrush` codec), the brush-library
  commands, `rerender_stroke(doc, record, base)`.
