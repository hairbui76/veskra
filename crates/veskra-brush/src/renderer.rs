//! [`StrokeRenderer`]: confirmed samples → stabilized path → spaced dabs → per-stroke coverage →
//! `base ⊕ stroke` composite (ADR-004).
//!
//! ## Compositing (integer, ADR-005 / CONTRACTS §1)
//! For every pixel: `k = cov16 ⊗ opacity16 [⊗ selection8·257]` where `a ⊗ b = round(a·b/65535)`
//! (`veskra_raster::ops::mul65535`, round-half-up). Then, with `src = scale(color_px16, k)`
//! (`color_px16 = ops::color_to_px16(color)`, premultiplied, includes the color's alpha):
//! * Paint → `src_over(base, src)`;
//! * Paint + alpha lock → `src_atop(base, src)` (base alpha unchanged);
//! * Erase → `erase(base, k)` (destination-out; all four channels scaled by the same factor,
//!   so straight color is preserved — PNT-004);
//! * Erase + alpha lock → no-op (alpha lock forbids alpha changes).
//!
//! All operations come from `veskra_raster::ops`, which keep `r,g,b ≤ a`. Pixels outside
//! `StrokeContext::clip` are never written and no coverage is stored outside it.

use std::sync::Arc;

use veskra_core::limits::MAX_STROKE_SAMPLES;
use veskra_core::{ColorRgba, RectI, SampleFlags, StrokeSample};
use veskra_raster::ops::{self, cov8_to_16, mul65535};
use veskra_raster::{MaskGrid, Px16, TILE, Tile, TileCoord, TileGrid};

use crate::coverage::{CoverageGrid, T};
use crate::dab::{self, DabStyle};
use crate::geometry::{self, GeometryOut};
use crate::path::{PathPt, Stabilizer, Walker};
use crate::preset::{MAX_BRUSH_SIZE_PX, MAX_STABILIZER_ROPE_PX};
use crate::{BrushError, BrushPreset};

/// Samples with `|x|` or `|y|` above this (document px) are rejected as invalid.
pub const MAX_SAMPLE_COORD: f32 = 1.0e6;
/// Dabs beyond this count in one stroke are dropped (and counted) to bound work.
pub const MAX_DABS_PER_STROKE: u64 = 8_000_000;
/// Minimum distance between dabs (document px), whatever the size/spacing.
pub const MIN_DAB_STEP_PX: f64 = 0.25;

/// Paint or erase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrokeMode {
    Paint,
    Erase,
}

/// Per-stroke context supplied by the engine.
#[derive(Debug, Clone)]
pub struct StrokeContext {
    /// Requested mode; an eraser preset always erases.
    pub mode: StrokeMode,
    /// Layer alpha lock (paint becomes src-atop; erase becomes a no-op).
    pub alpha_lock: bool,
    /// Active selection; multiplies coverage.
    pub selection: Option<MaskGrid>,
    /// Document bounds; nothing is stored or written outside.
    pub clip: RectI,
    /// Replaces `preset.size_px` (UI size slider).
    pub size_override: Option<f32>,
    /// Replaces `preset.opacity` (UI opacity slider).
    pub opacity_override: Option<f32>,
    /// Replaces `preset.stabilization` (user toggle, BRH-05).
    pub stabilization_override: Option<f32>,
}

impl Default for StrokeContext {
    /// Paint, no selection/alpha lock/overrides, **empty clip** (renders nothing until the
    /// caller sets `clip` to the document bounds).
    fn default() -> Self {
        Self::new(StrokeMode::Paint, RectI::EMPTY)
    }
}

impl StrokeContext {
    /// Context with no selection, no alpha lock and no overrides.
    pub fn new(mode: StrokeMode, clip: RectI) -> Self {
        Self {
            mode,
            alpha_lock: false,
            selection: None,
            clip,
            size_override: None,
            opacity_override: None,
            stabilization_override: None,
        }
    }
}

/// Sample/dab accounting for a stroke.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StrokeStats {
    /// Confirmed samples accepted into the stroke.
    pub confirmed: usize,
    /// Samples flagged `PREDICTED` passed to `add_samples` (skipped, never stored).
    pub predicted_rejected: usize,
    /// Invalid samples skipped: unconfirmed/hover, failing `validate`, out of coordinate range,
    /// non-monotonic time, over `MAX_STROKE_SAMPLES`, or arriving after `flush`.
    pub invalid_rejected: usize,
    /// Dabs placed.
    pub dabs: u64,
    /// Dabs dropped by [`MAX_DABS_PER_STROKE`].
    pub dabs_dropped: u64,
}

/// Result of [`StrokeRenderer::finish`].
#[derive(Debug, Clone)]
pub struct FinishedStroke {
    /// Pixels the stroke may have changed (clipped to the document).
    pub bounds: RectI,
    /// The confirmed samples, in order (what a `StrokeRecord` stores).
    pub confirmed_samples: Vec<StrokeSample>,
    pub dab_count: u64,
    /// Present only for `vector_eligible` presets in paint mode.
    pub geometry: Option<GeometryOut>,
    /// The layer after the stroke: `base ⊕ stroke`, sharing every untouched tile with `base`.
    pub tiles: TileGrid,
    pub stats: StrokeStats,
}

#[derive(Debug, Clone, Copy)]
struct Shape {
    size: f32,
    spacing: f64,
    style: DabStyle,
    clip: RectI,
}

/// The mutable paint pipeline; cloned (cheaply, COW coverage) for prediction previews.
#[derive(Debug, Clone)]
struct Pipeline {
    stab: Stabilizer,
    walker: Walker,
    cov: CoverageGrid,
    dabs: u64,
    dropped: u64,
}

impl Pipeline {
    fn walk(&mut self, preset: &BrushPreset, sh: &Shape, p: Option<PathPt>, flush: bool) -> RectI {
        let Pipeline { walker, cov, dabs, dropped, .. } = self;
        let mut dirty = RectI::EMPTY;
        let step = |q: &PathPt| (sh.spacing * preset.diameter(sh.size, q.pressure, q.tilt) as f64).max(MIN_DAB_STEP_PX);
        let mut emit = |q: PathPt| {
            if *dabs >= MAX_DABS_PER_STROKE {
                *dropped += 1;
                return;
            }
            *dabs += 1;
            let d = preset.diameter(sh.size, q.pressure, q.tilt) as f64;
            // Cull dabs whose footprint misses the clip (cheap; same pixels as rasterizing them).
            let r = d / 2.0 + 2.0;
            if q.x + r < sh.clip.x0 as f64
                || q.x - r > sh.clip.x1 as f64
                || q.y + r < sh.clip.y0 as f64
                || q.y - r > sh.clip.y1 as f64
            {
                return;
            }
            let a = preset.dab_alpha(q.pressure) as f64;
            dirty = dirty.union(&dab::rasterize(cov, q.x, q.y, d / 2.0, a, &sh.style, sh.clip));
        };
        if let Some(p) = p {
            walker.push(p, &step, &mut emit);
        }
        if flush {
            walker.flush(&mut emit);
        }
        dirty
    }

    fn feed(&mut self, preset: &BrushPreset, sh: &Shape, raw: PathPt, stab_out: Option<&mut Vec<PathPt>>) -> RectI {
        let sp = self.stab.push(raw);
        if let Some(v) = stab_out {
            v.push(sp);
        }
        self.walk(preset, sh, Some(sp), false)
    }

    fn flush(&mut self, preset: &BrushPreset, sh: &Shape, stab_out: &mut Vec<PathPt>) -> RectI {
        let tail = self.stab.flush();
        if let Some(t) = tail {
            stab_out.push(t);
        }
        self.walk(preset, sh, tail, true)
    }
}

/// Renders one stroke deterministically (PNT-001): output depends only on
/// (confirmed samples, preset, color, seed, base, context), never on batching.
#[derive(Debug)]
pub struct StrokeRenderer {
    preset: BrushPreset,
    color_px: Px16,
    seed: u64,
    base: TileGrid,
    ctx: StrokeContext,
    mode: StrokeMode,
    opacity16: u16,
    shape: Shape,
    pipe: Pipeline,
    bounds: RectI,
    confirmed: Vec<StrokeSample>,
    stab_path: Vec<PathPt>,
    last_t: Option<u64>,
    predicted_rejected: usize,
    invalid_rejected: usize,
    flushed: bool,
}

impl StrokeRenderer {
    /// Start a stroke (CONTRACTS signature; infallible). `base` is the layer at stroke start (a
    /// cheap COW clone). Out-of-range inputs are made safe instead of failing: the preset is
    /// [`BrushPreset::sanitized`], color components are clamped (NaN → 0), invalid overrides are
    /// ignored and an empty clip renders nothing. Use [`StrokeRenderer::try_begin`] to get a typed
    /// error instead.
    pub fn begin(
        preset: BrushPreset,
        color: ColorRgba,
        seed: u64,
        base: TileGrid,
        ctx: StrokeContext,
    ) -> StrokeRenderer {
        let preset = preset.sanitized();
        let in01 = |v: &f32| v.is_finite() && (0.0..=1.0).contains(v);
        let size = ctx
            .size_override
            .filter(|s| s.is_finite() && *s > 0.0 && *s <= MAX_BRUSH_SIZE_PX)
            .unwrap_or(preset.size_px);
        let opacity = ctx.opacity_override.filter(in01).unwrap_or(preset.opacity);
        let stabilization = ctx.stabilization_override.filter(in01).unwrap_or(preset.stabilization);
        let mode = if preset.eraser { StrokeMode::Erase } else { ctx.mode };
        let shape = Shape {
            size,
            spacing: preset.spacing as f64,
            style: DabStyle {
                hardness: preset.hardness as f64,
                accumulation: preset.accumulation,
                grain: preset.grain,
                seed,
            },
            clip: ctx.clip,
        };
        StrokeRenderer {
            // color_to_px16 clamps components to [0,1] and maps non-finite values to 0.
            color_px: ops::color_to_px16(color),
            preset,
            seed,
            base,
            mode,
            opacity16: ops::color_to_px16(ColorRgba::new(0.0, 0.0, 0.0, opacity))[3],
            shape,
            pipe: Pipeline {
                stab: Stabilizer::new((stabilization * MAX_STABILIZER_ROPE_PX) as f64),
                walker: Walker::default(),
                cov: CoverageGrid::default(),
                dabs: 0,
                dropped: 0,
            },
            ctx,
            bounds: RectI::EMPTY,
            confirmed: Vec::new(),
            stab_path: Vec::new(),
            last_t: None,
            predicted_rejected: 0,
            invalid_rejected: 0,
            flushed: false,
        }
    }

    /// Validating variant of [`StrokeRenderer::begin`]: rejects an invalid preset, color,
    /// empty clip or out-of-range override with a typed [`BrushError`].
    pub fn try_begin(
        preset: BrushPreset,
        color: ColorRgba,
        seed: u64,
        base: TileGrid,
        ctx: StrokeContext,
    ) -> Result<StrokeRenderer, BrushError> {
        preset.validate()?;
        if !color.is_valid() {
            return Err(BrushError::InvalidArgument("color components must be finite and in [0,1]"));
        }
        if ctx.clip.is_empty() {
            return Err(BrushError::InvalidArgument("clip rectangle is empty"));
        }
        let in01 = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        if let Some(s) = ctx.size_override
            && !(s.is_finite() && s > 0.0 && s <= MAX_BRUSH_SIZE_PX)
        {
            return Err(BrushError::InvalidArgument("size override must be in (0, 1000]"));
        }
        if !ctx.opacity_override.is_none_or(in01) || !ctx.stabilization_override.is_none_or(in01) {
            return Err(BrushError::InvalidArgument("opacity/stabilization override must be in [0,1]"));
        }
        Ok(Self::begin(preset, color, seed, base, ctx))
    }

    /// Append confirmed samples; returns the dirty rect (pixels whose stroke coverage may
    /// have changed). Predicted samples are skipped and counted, never stored.
    pub fn add_samples(&mut self, samples: &[StrokeSample]) -> RectI {
        let mut dirty = RectI::EMPTY;
        for s in samples {
            if s.flags.contains(SampleFlags::PREDICTED) {
                self.predicted_rejected += 1;
                continue;
            }
            let ok = !self.flushed
                && s.is_confirmed()
                && !s.flags.contains(SampleFlags::HOVER)
                && s.validate().is_ok()
                && s.x.abs() <= MAX_SAMPLE_COORD
                && s.y.abs() <= MAX_SAMPLE_COORD
                && self.last_t.is_none_or(|t| s.t_ns >= t)
                && self.confirmed.len() < MAX_STROKE_SAMPLES;
            if !ok {
                self.invalid_rejected += 1;
                continue;
            }
            self.last_t = Some(s.t_ns);
            self.confirmed.push(*s);
            let p = PathPt { x: s.x as f64, y: s.y as f64, pressure: s.pressure, tilt: s.tilt };
            dirty = dirty.union(&self.pipe.feed(&self.preset, &self.shape, p, Some(&mut self.stab_path)));
        }
        self.bounds = self.bounds.union(&dirty);
        dirty
    }

    /// End of input: the stabilizer catches up to the last confirmed sample and a final dab is
    /// placed at the path end. Idempotent; samples added afterwards are rejected.
    /// [`StrokeRenderer::finish`] calls it; call it earlier to display the tail before commit.
    pub fn flush(&mut self) -> RectI {
        if self.flushed {
            return RectI::EMPTY;
        }
        self.flushed = true;
        let dirty = self.pipe.flush(&self.preset, &self.shape, &mut self.stab_path);
        self.bounds = self.bounds.union(&dirty);
        dirty
    }

    /// Write `base ⊕ stroke` into `target` for `region ∩ clip` (see module docs for the math).
    /// Pixels in the region without coverage are restored to `base`.
    pub fn render_into(&self, target: &mut TileGrid, region: RectI) {
        self.composite(&self.pipe.cov, target, region);
    }

    /// Display-only preview of confirmed + `predicted` samples (STY-007). The renderer's
    /// confirmed state is not changed.
    ///
    /// **Precondition:** `target` already holds `base ⊕ confirmed stroke` (i.e. it is kept up to
    /// date with [`StrokeRenderer::render_into`] for every confirmed dirty rect). Only the area
    /// touched by the predicted dabs is recomposited and returned, so the cost is O(prediction),
    /// not O(stroke). The caller restores the previous preview's rect (e.g. by starting from a
    /// fresh copy of its confirmed tiles) so stale predicted pixels disappear.
    pub fn prediction_preview(&self, predicted: &[StrokeSample], target: &mut TileGrid) -> RectI {
        let mut pipe = self.pipe.clone();
        let mut dirty = RectI::EMPTY;
        if !self.flushed {
            for s in predicted {
                if s.validate().is_err() || s.x.abs() > MAX_SAMPLE_COORD || s.y.abs() > MAX_SAMPLE_COORD {
                    continue;
                }
                let p = PathPt { x: s.x as f64, y: s.y as f64, pressure: s.pressure, tilt: s.tilt };
                dirty = dirty.union(&pipe.feed(&self.preset, &self.shape, p, None));
            }
        }
        self.composite(&pipe.cov, target, dirty);
        dirty
    }

    /// Finish the stroke (flushes first). Geometry is computed only for vector-eligible presets
    /// in paint mode.
    pub fn finish(mut self) -> FinishedStroke {
        self.flush();
        let mut tiles = self.base.clone();
        self.composite(&self.pipe.cov, &mut tiles, self.bounds);
        // Vector geometry cost grows with path length. A stroke whose paint path leaves the
        // canvas by more than one canvas size is mostly invisible; it keeps its exact raster
        // pixels but gets no vector form (reported as raster-only on export, never silently
        // mis-exported). Bounds the cost of hostile/far-off-canvas input (QA-D3).
        let clip = self.ctx.clip;
        let margin = clip.width().max(clip.height()) as f64;
        let within_reach = self.stab_path.iter().all(|p| {
            p.x >= clip.x0 as f64 - margin
                && p.x <= clip.x1 as f64 + margin
                && p.y >= clip.y0 as f64 - margin
                && p.y <= clip.y1 as f64 + margin
        });
        let geometry = if self.preset.vector_eligible && self.mode == StrokeMode::Paint && within_reach {
            let (preset, size) = (&self.preset, self.shape.size);
            geometry::build(
                &self.stab_path,
                |p| preset.diameter(size, p.pressure, p.tilt) as f64,
                preset.hardness as f64,
            )
        } else {
            None
        };
        let stats = self.stats();
        FinishedStroke {
            bounds: self.bounds,
            confirmed_samples: self.confirmed,
            dab_count: self.pipe.dabs,
            geometry,
            tiles,
            stats,
        }
    }

    /// Accumulated stroke coverage at a pixel (`0..=65535`, before opacity/selection).
    pub fn coverage(&self, x: i32, y: i32) -> u16 {
        self.pipe.cov.get(x, y)
    }

    pub fn bounds(&self) -> RectI {
        self.bounds
    }

    pub fn mode(&self) -> StrokeMode {
        self.mode
    }

    pub fn preset(&self) -> &BrushPreset {
        &self.preset
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn confirmed_samples(&self) -> &[StrokeSample] {
        &self.confirmed
    }

    /// Number of coverage tiles held (memory: 8 KiB each).
    pub fn coverage_tile_count(&self) -> usize {
        self.pipe.cov.tile_count()
    }

    pub fn stats(&self) -> StrokeStats {
        StrokeStats {
            confirmed: self.confirmed.len(),
            predicted_rejected: self.predicted_rejected,
            invalid_rejected: self.invalid_rejected,
            dabs: self.pipe.dabs,
            dabs_dropped: self.pipe.dropped,
        }
    }

    fn composite(&self, cov: &CoverageGrid, target: &mut TileGrid, region: RectI) {
        let area = region.intersect(&self.ctx.clip);
        if area.is_empty() {
            return;
        }
        let noop = self.mode == StrokeMode::Erase && self.ctx.alpha_lock;
        for c in TileGrid::tiles_in_rect(area) {
            let tr = c.rect();
            let ir = tr.intersect(&area);
            let base_t = self.base.tile(c);
            let cov_t = if noop { None } else { cov.tile(c) };
            // Erase/alpha-lock cannot change transparent base pixels.
            let unchanged = base_t.is_none() && (self.mode == StrokeMode::Erase || self.ctx.alpha_lock);
            let Some(cov_t) = cov_t.filter(|_| !unchanged) else {
                restore_base(target, base_t, c, ir);
                continue;
            };
            let sel = self.ctx.selection.as_ref().map(|m| (m.tile(c), m.default_value()));
            let tt = target.tile_mut(c);
            let px = tt.pixels_mut();
            for y in ir.y0..ir.y1 {
                let ly = (y - tr.y0) as usize;
                for x in ir.x0..ir.x1 {
                    let lx = (x - tr.x0) as usize;
                    let i = ly * T + lx;
                    let b: Px16 = match base_t {
                        Some(t) => {
                            let s = &t.pixels()[i * 4..i * 4 + 4];
                            [s[0], s[1], s[2], s[3]]
                        }
                        None => [0; 4],
                    };
                    let mut k = cov_t[i];
                    if k != 0 {
                        k = mul65535(k, self.opacity16);
                        if let Some((st, dv)) = &sel {
                            let s8 = st.map_or(*dv, |t| t.get(lx as u32, ly as u32));
                            k = mul65535(k, cov8_to_16(s8));
                        }
                    }
                    let out = if k == 0 {
                        b
                    } else {
                        match (self.mode, self.ctx.alpha_lock) {
                            (StrokeMode::Erase, _) => ops::erase(b, k),
                            (StrokeMode::Paint, false) => ops::src_over(b, ops::scale(self.color_px, k)),
                            (StrokeMode::Paint, true) => ops::src_atop(b, ops::scale(self.color_px, k)),
                        }
                    };
                    px[i * 4..i * 4 + 4].copy_from_slice(&out);
                }
            }
            if base_t.is_none() && tt.is_transparent() {
                target.remove_tile(c);
            }
        }
    }
}

/// Make `target`'s pixels in `ir` (inside tile `c`) equal to the base tile's.
fn restore_base(target: &mut TileGrid, base_t: Option<&Arc<Tile>>, c: TileCoord, ir: RectI) {
    let full = ir == c.rect();
    match base_t {
        Some(bt) => {
            if target.tile(c).is_some_and(|t| Arc::ptr_eq(t, bt)) {
                return;
            }
            if full {
                target.set_tile(c, bt.clone());
                return;
            }
            let tr = c.rect();
            let src = bt.pixels();
            let dst = target.tile_mut(c).pixels_mut();
            for y in ir.y0..ir.y1 {
                let row = (y - tr.y0) as usize * TILE as usize;
                let (a, b) = ((row + (ir.x0 - tr.x0) as usize) * 4, (row + (ir.x1 - tr.x0) as usize) * 4);
                dst[a..b].copy_from_slice(&src[a..b]);
            }
        }
        None => {
            if target.tile(c).is_none() {
                return;
            }
            if full {
                target.remove_tile(c);
                return;
            }
            let tr = c.rect();
            let dst = target.tile_mut(c).pixels_mut();
            for y in ir.y0..ir.y1 {
                let row = (y - tr.y0) as usize * TILE as usize;
                dst[(row + (ir.x0 - tr.x0) as usize) * 4..(row + (ir.x1 - tr.x0) as usize) * 4].fill(0);
            }
        }
    }
}
