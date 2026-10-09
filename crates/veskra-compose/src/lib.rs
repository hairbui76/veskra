//! # veskra-compose — deterministic CPU reference compositor (REN + CLR)
//!
//! The base truth for layer compositing (ARCHITECTURE §E, ADR-005, ADR-010): the presentation
//! path on Android (via [`render_display_rgba8`]), the composite embedded in exports, and the
//! oracle any future GPU renderer is compared against (PNT-012).
//!
//! ## Pixel formats
//! * Input layer tiles: premultiplied RGBA16, sRGB-encoded working space
//!   ([`veskra_core::PixelDescriptor::WORKING_V1`]); masks are u8 coverage, 255 = visible.
//! * Internal: premultiplied `f32` in the document's [`BlendSpace`], one 64×64 tile at a time.
//! * Output of [`composite_rect`] / [`composite_grid`] / [`composite_subtree`]: premultiplied
//!   RGBA16, **encoded** working space (`WORKING_V1`), rounded **once** per channel with the
//!   CONTRACTS §1 rule `floor(v·65535 + 0.5)` (GammaEncoded) or
//!   [`veskra_color::convert::linear_premul_f32_to_premul16`] (Linear); `r,g,b ≤ a` always.
//! * [`render_display_rgba8`]: `DISPLAY_RGBA8_PREMUL` via `convert::premul16_to_premul8`.
//! * [`thumbnail_rgba8`]: `RGBA8_STRAIGHT` via `convert::premul16_to_straight8` (no halos).
//!
//! Only pixels inside the canvas `[0,w)×[0,h)` are composited; everything outside is
//! transparent (layer content beyond the canvas is retained but never shown/exported).
//!
//! ## Normative semantics (PNT-006, PNT-007, DOC-01..03)
//! 1. **Paint order**: sibling lists are bottom → top (`root[0]` is the bottom-most node).
//! 2. **Blend formula** ([`blend`]): W3C Compositing Level 1 general source-over with
//!    premultiplied colors, `co = cs·(1−αb) + cb·(1−αs) + αs·αb·B(Cb,Cs)`,
//!    `αo = αs + αb·(1−αs)`, with `Cb`, `Cs` unpremultiplied inside `B`. Normal / Multiply /
//!    Screen / Overlay as in psd-tools `blend.py`.
//! 3. **Source of a layer**: its pixels (or the [`ComposeOptions::overrides`] grid) multiplied by
//!    `opacity × mask/255` (all four premultiplied channels), then blended with its mode.
//! 4. **Blend space** (`doc.color.blend_space`):
//!    * `GammaEncoded` (default, Photoshop/PSD-compatible): math on encoded values `v/65535`.
//!    * `Linear`: every layer pixel is decoded with the sRGB transfer, premultiplied-aware
//!      (unpremultiply → linearize → premultiply, `convert::premul16_to_linear_premul_f32`);
//!      blending, opacity, masks and group interpolation happen on linear premultiplied values;
//!      the result is re-encoded once on output. Exporters must warn (ADR-005).
//! 5. **Visibility**: a node with `visible == false` (unless `include_hidden`) is skipped with its
//!    whole subtree; a leaf for which `layer_filter` returns false is skipped. `locked` and
//!    `alpha_locked` are editing state and do not affect compositing (alpha lock is a
//!    paint-time rule implemented by the brush/fill).
//! 6. **Isolated groups** (`pass_through == false`, default): children are composited onto a
//!    transparent buffer; that result is then the group's source (rule 3 with the group's
//!    opacity and blend mode).
//! 7. **Pass-through groups** (`pass_through == true`, PSD `pass`): the group's blend mode is
//!    ignored. Children blend directly onto the current backdrop; then, per pixel,
//!    `result = lerp(backdrop_before, backdrop_after, opacity × mask)` in premultiplied space
//!    (identical to psd-tools `_apply_passthrough_source`, which models Photoshop). Groups carry
//!    no mask in `veskra-doc` today; when added it multiplies the interpolation factor.
//! 8. **Clipping** (`clip_to_below`) — exact rule implemented, matching psd-tools
//!    `Compositor._apply_clip_layers` / `_resolve_source` (Photoshop default
//!    "Blend Clipped Layers as Group"):
//!    * A *clipper* is a non-group layer with `clip_to_below`. Each maximal run of clippers
//!      directly above a non-clipper `B` (the *base*) clips to `B`. `clip_to_below` on a
//!      **group is ignored** (Photoshop has no clipping groups); such a group can be a base.
//!    * Let `P` be the base's own source *before* its opacity and mask (raster pixels, or the
//!      isolated group result) with alpha `αP`. Each visible clipper is composited onto `P`
//!      (bottom → top) as an ordinary source with its own mode, opacity and mask, giving `Q`.
//!      The clipped base is `Q` with its straight color kept and its alpha replaced by `αP`
//!      (premultiplied: `Q·αP/αQ`; 0 where `αQ = 0`). Clippers therefore never extend the
//!      base's coverage, and their blend modes act on the base's colors only (not on layers
//!      below the base).
//!    * Then the base's **opacity and mask are applied to the whole clipped result** and it is
//!      blended onto the backdrop with the **base's** blend mode. So base mask and base opacity
//!      both limit the clippers; the base pixel alpha (not opacity) is what is "clipped to".
//!    * Hidden/filtered base ⇒ its clippers are not drawn. Hidden clippers are skipped but
//!      keep the run (the next clipper still clips to the same base).
//!    * A clipper with no non-clipper below it in its sibling list composites as a normal
//!      layer. (psd-tools drops such a layer instead; PSD export must write it as non-clipping.)
//!    * A pass-through group acting as a base is composited as an isolated Normal group (psd-tools
//!      also stops passing through when a group has clip layers; it removes the backdrop with
//!      `result_isolated`, which differs only for non-Normal children over a non-transparent
//!      backdrop — documented limitation).
//!
//! ## Performance model
//! Work is done per 64×64 tile only for tiles intersecting the requested rect ∩ canvas; a
//! layer without a tile there is skipped in O(log tiles), and tiles where nothing draws cost
//! no pixel work. Memory is O(one tile buffer per nesting level per thread), never O(canvas)
//! beyond the requested output. Calls touching ≥ 8 tiles split tile rows over up to 8 scoped
//! threads (`available_parallelism`); each tile is computed independently with a fixed
//! traversal order, so output is bit-identical regardless of thread count or scheduling.
//! `layer_filter` is invoked only on the calling thread, once per leaf layer per call.

mod engine;

pub mod blend;

use veskra_color::convert;
use veskra_core::{BlendSpace, ColorRgba, ErrorCode, LayerId, RectI, VeskraError};
use veskra_doc::{Document, TileGrid};
use veskra_raster::{MAX_RECT_PIXELS, TILE_LEN, Tile, TileCoord};

use engine::Engine;

pub use veskra_core::BlendMode;

/// Options shared by every compositing entry point. `Default`: visible layers only, no
/// filter, no overrides.
#[derive(Clone, Copy, Default)]
pub struct ComposeOptions<'a> {
    /// Composite layers/groups whose `visible` flag is false (default `false`).
    pub include_hidden: bool,
    /// Leaf (raster/vector) layers for which this returns `false` are treated as hidden.
    /// Groups are not filtered (a group whose leaves are all filtered draws nothing).
    pub layer_filter: Option<&'a dyn Fn(LayerId) -> bool>,
    /// Replace a pixel layer's tiles (live stroke / prediction preview). The layer's mask,
    /// opacity, mode and clipping still apply. Entries naming groups or unknown ids are ignored;
    /// the first entry for an id wins.
    pub overrides: &'a [(LayerId, &'a TileGrid)],
}

impl std::fmt::Debug for ComposeOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeOptions")
            .field("include_hidden", &self.include_hidden)
            .field("layer_filter", &self.layer_filter.is_some())
            .field("overrides", &self.overrides.len())
            .finish()
    }
}

/// The blend space a document composites in (`doc.color.blend_space`).
pub fn blend_space(doc: &Document) -> BlendSpace {
    doc.color.blend_space
}

/// Upper bound on worker threads for one compositing call.
const MAX_THREADS: usize = 8;
/// Below this many tiles a call stays on the caller's thread (spawn cost dominates).
const PARALLEL_MIN_TILES: usize = 8;

/// Run `work` over `items` on up to [`MAX_THREADS`] scoped threads (work-stealing queue; the
/// caller's thread participates). Results never depend on scheduling: every item writes only
/// its own output. If a thread cannot be spawned, the remaining threads drain the queue.
fn parallel_drain<T: Send>(items: Vec<T>, tiles: usize, work: impl Fn(T) + Sync) {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS).min(items.len());
    if threads <= 1 || tiles < PARALLEL_MIN_TILES {
        items.into_iter().for_each(work);
        return;
    }
    let queue = std::sync::Mutex::new(items);
    let next = || queue.lock().ok().and_then(|mut q| q.pop());
    std::thread::scope(|s| {
        for _ in 1..threads {
            let _ = std::thread::Builder::new().spawn_scoped(s, || {
                while let Some(it) = next() {
                    work(it);
                }
            });
        }
        while let Some(it) = next() {
            work(it);
        }
    });
}

fn composite_into(
    doc: &Document,
    rect: RectI,
    eng: &Engine,
    f: impl Fn(TileCoord) -> Option<engine::Buf> + Sync,
) -> Vec<u16> {
    if rect.is_empty() || rect.area() > MAX_RECT_PIXELS {
        return Vec::new();
    }
    let mut out = vec![0u16; rect.area() as usize * 4];
    let live = rect.intersect(&doc.bounds());
    if live.is_empty() {
        return out;
    }
    // One band per tile row of `live`; each band owns a disjoint slice of whole output rows.
    let row = rect.width() as usize * 4;
    let mut bands = Vec::new();
    let mut rest = out.get_mut((live.y0 - rect.y0) as usize * row..).unwrap_or_default();
    let mut y = live.y0;
    while y < live.y1 {
        let y1 = (TileCoord::of_pixel(0, y).rect().y1).min(live.y1);
        let (band, tail) = rest.split_at_mut(((y1 - y) as usize * row).min(rest.len()));
        bands.push((RectI::new(live.x0, y, live.x1, y1), band));
        rest = tail;
        y = y1;
    }
    let tiles = TileGrid::tiles_in_rect(live).count();
    parallel_drain(bands, tiles, |(band, slice)| {
        let band_out = RectI::new(rect.x0, band.y0, rect.x1, band.y1);
        for c in TileGrid::tiles_in_rect(band) {
            if let Some(buf) = f(c) {
                eng.write_region(&buf, c, c.rect().intersect(&band), slice, band_out);
            }
        }
    });
    out
}

/// Composite the visible document into premultiplied RGBA16 (encoded working space) for `rect`
/// (document px). Returns `rect.width × rect.height × 4` values, row-major; transparent outside
/// layers and outside the canvas. Returns an empty `Vec` for an empty rect or one larger than
/// [`MAX_RECT_PIXELS`] (same contract as `TileGrid::read_rect`).
pub fn composite_rect(doc: &Document, rect: RectI, opts: &ComposeOptions) -> Vec<u16> {
    let eng = Engine::new(doc, opts);
    composite_into(doc, rect, &eng, |c| eng.root_tile(c))
}

/// Composite the whole canvas into a sparse grid (only non-transparent tiles stored; pixels
/// outside the canvas are zero). Same semantics and rounding as [`composite_rect`].
pub fn composite_grid(doc: &Document, opts: &ComposeOptions) -> TileGrid {
    let eng = Engine::new(doc, opts);
    let canvas = doc.bounds();
    let coords: Vec<TileCoord> = TileGrid::tiles_in_rect(canvas).collect();
    let n = coords.len();
    let rows: Vec<Vec<TileCoord>> = coords.chunk_by(|a, b| a.ty == b.ty).map(<[TileCoord]>::to_vec).collect();
    let done = std::sync::Mutex::new(Vec::new());
    parallel_drain(rows, n, |row| {
        let mut local = Vec::new();
        for c in row {
            let Some(buf) = eng.root_tile(c) else { continue };
            let tr = c.rect();
            let mut px = vec![0u16; TILE_LEN];
            eng.write_region(&buf, c, tr.intersect(&canvas), &mut px, tr);
            if px.iter().any(|&v| v != 0)
                && let Ok(t) = Tile::from_pixels(&px)
            {
                local.push((c, std::sync::Arc::new(t)));
            }
        }
        if let Ok(mut d) = done.lock() {
            d.extend(local);
        }
    });
    let mut grid = TileGrid::new();
    for (c, t) in done.into_inner().unwrap_or_default() {
        grid.set_tile(c, t); // BTreeMap: final order independent of thread scheduling
    }
    grid
}

/// Composite only the subtree rooted at `id` (a layer or group) as if it were alone on a
/// transparent canvas: its own opacity and mask apply, its blend mode, `clip_to_below`, the
/// clippers above it and its ancestors' visibility do **not**. Its own `visible` flag and
/// `opts` still apply (pass `include_hidden: true` to render a hidden layer). A pass-through
/// group renders like an isolated Normal group here (identical on a transparent backdrop).
/// Unknown `id` → transparent. Output format as [`composite_rect`].
pub fn composite_subtree(doc: &Document, id: LayerId, rect: RectI, opts: &ComposeOptions) -> Vec<u16> {
    let eng = Engine::new(doc, opts);
    match doc.layer(id) {
        Some(layer) => composite_into(doc, rect, &eng, |c| eng.subtree_tile(layer, c)),
        None if rect.is_empty() || rect.area() > MAX_RECT_PIXELS => Vec::new(),
        None => vec![0u16; rect.area() as usize * 4],
    }
}

/// Android display buffer for `rect`: RGBA8 **premultiplied**, sRGB-encoded, bytes R,G,B,A per
/// pixel (`ARGB_8888` memory order), row stride `w·4`. `out.len()` must be exactly
/// `w·h·4`, else `InvalidArgument`; a rect over [`MAX_RECT_PIXELS`] → `AllocationLimit`.
/// No display ICC transform is applied here (sRGB working space ⇒ sRGB surface, ADR-006).
pub fn render_display_rgba8(
    doc: &Document,
    rect: RectI,
    opts: &ComposeOptions,
    out: &mut [u8],
) -> Result<(), VeskraError> {
    if rect.area() > MAX_RECT_PIXELS {
        return Err(VeskraError::alloc("display rect too large"));
    }
    let need = rect.area() as usize * 4;
    if out.len() != need {
        return Err(VeskraError::new(ErrorCode::InvalidArgument, "display buffer size mismatch"));
    }
    if need == 0 {
        return Ok(());
    }
    let px = composite_rect(doc, rect, opts);
    convert::premul16_to_premul8(&px, out);
    Ok(())
}

/// Tile rows composited per thumbnail band (bounds memory to `w × 256 × 8` bytes).
const THUMB_BAND_TILES: u64 = 4;

/// Straight RGBA8 thumbnail of the visible composite fitting in `max_edge × max_edge`,
/// preserving aspect ratio (each side ≥ 1; never upscaled). Box filter in **premultiplied**
/// RGBA16: source pixel `(x, y)` contributes to output `(x·tw/w, y·th/h)`; the average is
/// rounded half-up, then converted with `convert::premul16_to_straight8`. Composites in
/// 256-row bands (no full-canvas buffer). `max_edge == 0` → `(0, 0, [])`.
pub fn thumbnail_rgba8(doc: &Document, max_edge: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (doc.width() as u64, doc.height() as u64);
    if max_edge == 0 || w == 0 || h == 0 {
        return (0, 0, Vec::new());
    }
    let m = w.max(h);
    let me = max_edge as u64;
    let (tw, th) = if me >= m { (w, h) } else { (((w * me + m / 2) / m).max(1), ((h * me + m / 2) / m).max(1)) };
    let mut sums = vec![[0u64; 4]; (tw * th) as usize];
    let mut counts = vec![0u64; (tw * th) as usize];
    let opts = ComposeOptions::default();
    let band = veskra_raster::TILE as u64 * THUMB_BAND_TILES;
    let mut y0 = 0u64;
    while y0 < h {
        let y1 = (y0 + band).min(h);
        let r = RectI::new(0, y0 as i32, w as i32, y1 as i32);
        let px = composite_rect(doc, r, &opts);
        for y in y0..y1 {
            let oy = y * th / h;
            let row = &px[((y - y0) * w * 4) as usize..][..(w * 4) as usize];
            for (x, p) in row.chunks_exact(4).enumerate() {
                let oi = (oy * tw + x as u64 * tw / w) as usize;
                if let (Some(s), Some(n)) = (sums.get_mut(oi), counts.get_mut(oi)) {
                    for k in 0..4 {
                        s[k] += p[k] as u64;
                    }
                    *n += 1;
                }
            }
        }
        y0 = y1;
    }
    let avg: Vec<u16> = sums
        .iter()
        .zip(counts.iter())
        .flat_map(|(s, &n)| {
            let n = n.max(1);
            let a = ((s[3] + n / 2) / n) as u16;
            let f = |v: u64| (((v + n / 2) / n) as u16).min(a);
            [f(s[0]), f(s[1]), f(s[2]), a]
        })
        .collect();
    let mut out = vec![0u8; avg.len()];
    convert::premul16_to_straight8(&avg, &mut out);
    (tw as u32, th as u32, out)
}

/// Eyedropper: the visible composite at `(x, y)` as a **straight** encoded [`ColorRgba`]
/// (`a = a16/65535`, `c = c16/a16`). Transparent (all zero) where alpha is 0 or outside the
/// canvas.
pub fn sample_composite(doc: &Document, x: i32, y: i32) -> ColorRgba {
    let r = RectI::new(x, y, x.saturating_add(1), y.saturating_add(1));
    let px = composite_rect(doc, r, &ComposeOptions::default());
    match px.as_slice() {
        [r, g, b, a] if *a > 0 => {
            let af = *a as f32;
            ColorRgba::new(*r as f32 / af, *g as f32 / af, *b as f32 / af, af / 65535.0)
        }
        _ => ColorRgba::TRANSPARENT,
    }
}
