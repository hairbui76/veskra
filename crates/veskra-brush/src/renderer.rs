//! [`StrokeRenderer`]: confirmed samples → stabilized path → spaced dabs → per-stroke coverage →
//! `base ⊕ stroke` composite (ADR-004, ADR-019).
//!
//! ## Compositing (integer, ADR-005 / CONTRACTS §1)
//! For every pixel: `k = cov16 [⊗ W[depth]] ⊗ opacity16 [⊗ selection8·257]` where
//! `a ⊗ b = round(a·b/65535)` (`veskra_raster::ops::mul65535`, round-half-up). Then, with
//! `src = scale(color_px16, k)` (`color_px16 = ops::color_to_px16(color)`, premultiplied, includes
//! the color's alpha):
//! * Paint → `src_over(base, src)`;
//! * Paint + alpha lock → `src_atop(base, src)` (base alpha unchanged);
//! * Erase → `erase(base, k)` (destination-out; all four channels scaled by the same factor,
//!   so straight color is preserved — PNT-004);
//! * Erase + alpha lock → no-op (alpha lock forbids alpha changes).
//!
//! All operations come from `veskra_raster::ops`, which keep `r,g,b ≤ a`. Pixels outside
//! `StrokeContext::clip` are never written and no coverage is stored outside it.
//!
//! ## Brush-v2 features (ADR-019)
//! * Presets that only use schema-2 features (round circular tip, procedural fixed grain, no
//!   wet edges, no per-dab color) render through the schema-2 dab code ([`crate::dab`]) and stay
//!   byte-identical; jitter of size/opacity/flow/scatter and dynamics only change the dab
//!   parameters. Everything else goes through [`crate::stamp`].
//! * Jitter values are `rand01(seed, dab index, channel)`: the dab index counts placed dabs in
//!   stroke order, so they are independent of batching. Per-stroke color jitter uses the index
//!   `u64::MAX`.
//! * Velocity input: speed between consecutive confirmed samples (px/ms, from `t_ns`) divided by
//!   `VELOCITY_FULL_SCALE_PX_PER_MS`, clamped to 1 and smoothed with `k = dt/(dt + 30 ms)`.
//! * Wet edges: `W[d] = round(65535·(1 − 0.75·wet·smoothstep(0.2, 0.7, d/255)))` where `depth` is
//!   the per-pixel maximum of `1 − distance/radius` over the dabs (0 at a dab rim, 255 at its
//!   center): the interior of a stroke is lighter than its rim.
//! * Per-dab color jitter: a per-stroke straight-RGB buffer holds the coverage-weighted dab color;
//!   paint then uses `src = [c ⊗ ka, ka]` with `ka = k ⊗ color.alpha` instead of `scale(color, k)`.
//! * Missing image assets (a preset whose tip/texture image could not be resolved) fall back to
//!   a round tip / procedural grain of the same scale and are listed by
//!   [`StrokeRenderer::asset_fallbacks`]; nothing panics.

use std::sync::Arc;

use veskra_core::limits::MAX_STROKE_SAMPLES;
use veskra_core::{ColorRgba, RectI, SampleFlags, StrokeSample};
use veskra_raster::ops::{self, cov8_to_16, mul65535};
use veskra_raster::{MaskGrid, Px16, TILE, Tile, TileCoord, TileGrid};

use crate::asset::{AssetStore, ResolvedAssets};
use crate::coverage::{ColorGrid, CoverageGrid, DepthGrid, T};
use crate::dab::{self, DabStyle};
use crate::geometry::{self, GeometryOut};
use crate::noise::{Grain, rand01};
use crate::path::{PathPt, Stabilizer, Walker};
use crate::preset::{
    DabInput, MAX_BRUSH_SIZE_PX, MAX_STABILIZER_ROPE_PX, TextureMode, TextureSource, TipRotation,
    VELOCITY_FULL_SCALE_PX_PER_MS, VELOCITY_SMOOTHING_MS,
};
use crate::stamp::{self, Buffers, Dab, StampStyle, TexKind, TexStyle};
use crate::{BrushError, BrushPreset};

/// Samples with `|x|` or `|y|` above this (document px) are rejected as invalid.
pub const MAX_SAMPLE_COORD: f32 = 1.0e6;
/// Dabs beyond this count in one stroke are dropped (and counted) to bound work.
pub const MAX_DABS_PER_STROKE: u64 = 8_000_000;
/// Minimum distance between dabs (document px), whatever the size/spacing.
pub const MIN_DAB_STEP_PX: f64 = 0.25;

/// Jitter channels of [`rand01`].
const CH_SIZE: u64 = 1;
const CH_ANGLE: u64 = 2;
const CH_OPACITY: u64 = 3;
const CH_FLOW: u64 = 4;
const CH_SCATTER: u64 = 5;
const CH_HUE: u64 = 6;
const CH_SAT: u64 = 7;
const CH_BRI: u64 = 8;
/// Dab index used for per-stroke jitter.
const STROKE_INDEX: u64 = u64::MAX;

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

/// Which rasterizer a stroke uses (fixed at `begin`).
#[derive(Debug, Clone)]
enum Route {
    /// Schema-2 round dabs (byte-identical to the pre-v2 engine).
    Legacy(DabStyle),
    General(StampStyle),
}

#[derive(Debug, Clone)]
struct Shape {
    size: f32,
    spacing: f64,
    route: Route,
    clip: RectI,
    seed: u64,
    /// Straight encoded stroke color after per-stroke jitter (per-dab color jitter base).
    rgb: [f32; 3],
    per_dab_color: bool,
}

/// Smoothed pen speed from confirmed sample timestamps (see module docs).
#[derive(Debug, Clone, Copy, Default)]
struct Velocity {
    last: Option<(f64, f64, u64)>,
    v: f64,
}

impl Velocity {
    fn update(&mut self, x: f64, y: f64, t: u64) -> f32 {
        match self.last {
            None => self.last = Some((x, y, t)),
            Some((lx, ly, lt)) => {
                let dt_ms = t.saturating_sub(lt) as f64 / 1.0e6;
                if dt_ms > 0.0 {
                    let speed = ((x - lx) * (x - lx) + (y - ly) * (y - ly)).sqrt() / dt_ms;
                    let raw = (speed / VELOCITY_FULL_SCALE_PX_PER_MS as f64).clamp(0.0, 1.0);
                    let k = dt_ms / (dt_ms + VELOCITY_SMOOTHING_MS as f64);
                    self.v += (raw - self.v) * k;
                    self.last = Some((x, y, t));
                }
            }
        }
        self.v as f32
    }
}

/// The mutable paint pipeline; cloned (cheaply, COW buffers) for prediction previews.
#[derive(Debug, Clone)]
struct Pipeline {
    stab: Stabilizer,
    walker: Walker,
    cov: CoverageGrid,
    depth: Option<DepthGrid>,
    color: Option<ColorGrid>,
    vel: Velocity,
    last_dir: Option<[f64; 2]>,
    /// First dab of a direction-following tip, waiting for the first path direction.
    pending: Option<(PathPt, u64)>,
    dabs: u64,
    dropped: u64,
}

#[inline]
fn input_of(q: &PathPt) -> DabInput {
    DabInput { pressure: q.pressure, tilt: q.tilt, velocity: q.velocity }
}

/// HSV jitter of a straight RGB color: hue ± `range[0]·180°`, saturation / value ± `range[1|2]`,
/// with `u` uniform in `[0, 1)`. Arithmetic only (deterministic across platforms).
pub(crate) fn jitter_hsv(rgb: [f32; 3], range: [f32; 3], u: [f64; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) as f64 } else { 0.0 });
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let s = if max <= 0.0 { 0.0 } else { delta / max };
    let h = (h + (2.0 * u[0] - 1.0) * range[0] as f64 * 0.5).rem_euclid(1.0);
    let s = (s + (2.0 * u[1] - 1.0) * range[1] as f64).clamp(0.0, 1.0);
    let v = (max + (2.0 * u[2] - 1.0) * range[2] as f64).clamp(0.0, 1.0);
    let hh = h * 6.0;
    let sector = hh.floor();
    let f = hh - sector;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match (sector as i64).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r as f32, g as f32, b as f32]
}

/// Dab output of one [`Pipeline::walk`] call (split borrows of the pipeline's buffers).
struct Sink<'a> {
    cov: &'a mut CoverageGrid,
    depth: &'a mut Option<DepthGrid>,
    color: &'a mut Option<ColorGrid>,
    last_dir: &'a mut Option<[f64; 2]>,
    pending: &'a mut Option<(PathPt, u64)>,
    dabs: &'a mut u64,
    dropped: &'a mut u64,
    dirty: RectI,
}

impl Sink<'_> {
    /// Count, index and place one dab. A direction-following tip defers the stroke's first dab
    /// until the first path direction is known (else it would point to 0° whatever the stroke).
    fn emit(&mut self, preset: &BrushPreset, sh: &Shape, q: PathPt) {
        if *self.dabs >= MAX_DABS_PER_STROKE {
            *self.dropped += 1;
            return;
        }
        let idx = *self.dabs;
        *self.dabs += 1;
        let needs_dir = preset.tip.rotation == TipRotation::Direction && matches!(sh.route, Route::General(_));
        if needs_dir && q.dir.is_none() && self.last_dir.is_none() && self.pending.is_none() {
            *self.pending = Some((q, idx));
            return;
        }
        if let Some(d) = q.dir {
            *self.last_dir = Some(d);
        }
        if let Some((pq, pidx)) = self.pending.take() {
            self.draw(preset, sh, pq, pidx);
        }
        self.draw(preset, sh, q, idx);
    }

    /// End of stroke: a dab still waiting for a direction (a tap) is drawn as is.
    fn flush_pending(&mut self, preset: &BrushPreset, sh: &Shape) {
        if let Some((pq, pidx)) = self.pending.take() {
            self.draw(preset, sh, pq, pidx);
        }
    }

    fn draw(&mut self, preset: &BrushPreset, sh: &Shape, q: PathPt, idx: u64) {
        let j = preset.jitter;
        let inp = input_of(&q);
        let mut d = preset.dab_diameter(sh.size, &inp) as f64;
        if j.size > 0.0 {
            d *= 1.0 - j.size as f64 * rand01(sh.seed, idx, CH_SIZE);
        }
        let dir = q.dir.or(*self.last_dir);
        let (mut x, mut y) = (q.x, q.y);
        if j.scatter > 0.0 {
            let amount = j.scatter as f64 * preset.target_factor(&inp, |s| &s.scatter) as f64;
            let off = (2.0 * rand01(sh.seed, idx, CH_SCATTER) - 1.0) * amount * d;
            let [dx, dy] = dir.unwrap_or([1.0, 0.0]);
            x += -dy * off;
            y += dx * off;
        }
        // Cull dabs whose footprint misses the clip (cheap; same pixels as rasterizing them).
        let r = match &sh.route {
            Route::Legacy(_) => d / 2.0 + 2.0,
            Route::General(st) => stamp::reach(st, d / 2.0) + 2.0,
        };
        if x + r < sh.clip.x0 as f64
            || x - r > sh.clip.x1 as f64
            || y + r < sh.clip.y0 as f64
            || y - r > sh.clip.y1 as f64
        {
            return;
        }
        let mut a = preset.dab_alpha_at(&inp) as f64;
        if j.opacity > 0.0 {
            a *= 1.0 - j.opacity as f64 * rand01(sh.seed, idx, CH_OPACITY);
        }
        if j.flow > 0.0 {
            a *= 1.0 - j.flow as f64 * rand01(sh.seed, idx, CH_FLOW);
        }
        let tex_depth = preset.texture.as_ref().map(|t| t.depth * preset.target_factor(&inp, |s| &s.texture));
        let rect = match &sh.route {
            Route::Legacy(style) => {
                let style = match (style.grain, tex_depth) {
                    (Some(g), Some(depth)) if depth != g.strength => {
                        DabStyle { grain: Some(Grain { scale: g.scale, strength: depth.clamp(0.0, 1.0) }), ..*style }
                    }
                    _ => *style,
                };
                dab::rasterize(self.cov, x, y, d / 2.0, a, &style, sh.clip)
            }
            Route::General(st) => {
                let mut theta = preset.tip.angle as f64;
                match preset.tip.rotation {
                    TipRotation::Fixed => {}
                    TipRotation::Direction => {
                        if let Some([dx, dy]) = dir {
                            theta += (-dy).atan2(dx).to_degrees();
                        }
                    }
                    TipRotation::Pen => {
                        if let Some(az) = q.azimuth.filter(|a| a.is_finite()) {
                            theta -= (az as f64).to_degrees();
                        }
                    }
                }
                if j.angle > 0.0 {
                    theta += (2.0 * rand01(sh.seed, idx, CH_ANGLE) - 1.0) * j.angle as f64 * 180.0;
                }
                let (sin, cos) = theta.to_radians().sin_cos();
                let dab_color = sh.per_dab_color.then(|| {
                    let u = [rand01(sh.seed, idx, CH_HUE), rand01(sh.seed, idx, CH_SAT), rand01(sh.seed, idx, CH_BRI)];
                    jitter_hsv(sh.rgb, [j.hue, j.saturation, j.brightness], u)
                        .map(|v| (v as f64 * 65535.0 + 0.5).floor().clamp(0.0, 65535.0) as u16)
                });
                let dab = Dab {
                    cx: x,
                    cy: y,
                    radius: d / 2.0,
                    alpha: a,
                    cos,
                    sin,
                    tex_depth: tex_depth.unwrap_or(0.0).clamp(0.0, 1.0) as f64,
                    color: dab_color,
                };
                let mut buf = Buffers { cov: self.cov, depth: self.depth.as_mut(), color: self.color.as_mut() };
                stamp::stamp(&mut buf, st, &dab, sh.clip)
            }
        };
        self.dirty = self.dirty.union(&rect);
    }
}

impl Pipeline {
    fn walk(&mut self, preset: &BrushPreset, sh: &Shape, p: Option<PathPt>, flush: bool) -> RectI {
        let Pipeline { walker, cov, depth, color, last_dir, pending, dabs, dropped, .. } = self;
        let mut sink = Sink { cov, depth, color, last_dir, pending, dabs, dropped, dirty: RectI::EMPTY };
        let step = |q: &PathPt| (sh.spacing * preset.dab_diameter(sh.size, &input_of(q)) as f64).max(MIN_DAB_STEP_PX);
        if let Some(p) = p {
            walker.push(p, &step, &mut |q| sink.emit(preset, sh, q));
        }
        if flush {
            walker.flush(&mut |q| sink.emit(preset, sh, q));
            sink.flush_pending(preset, sh);
        }
        sink.dirty
    }

    fn feed(
        &mut self,
        preset: &BrushPreset,
        sh: &Shape,
        s: &StrokeSample,
        stab_out: Option<&mut Vec<PathPt>>,
    ) -> RectI {
        let (x, y) = (s.x as f64, s.y as f64);
        let velocity = self.vel.update(x, y, s.t_ns);
        let raw = PathPt { x, y, pressure: s.pressure, tilt: s.tilt, azimuth: s.azimuth, velocity, dir: None };
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

/// Wet-edge factor table (see module docs).
fn wet_lut(wet: f32) -> Box<[u16; 256]> {
    let mut lut = Box::new([0u16; 256]);
    for (d, out) in lut.iter_mut().enumerate() {
        let x = ((d as f64 / 255.0 - 0.2) / 0.5).clamp(0.0, 1.0);
        let s = x * x * (3.0 - 2.0 * x);
        let f = 1.0 - 0.75 * wet as f64 * s;
        *out = (f * 65535.0 + 0.5).floor().clamp(0.0, 65535.0) as u16;
    }
    lut
}

/// Built-in procedural assets (tips and grains of the built-in presets).
fn builtin_store() -> &'static AssetStore {
    crate::builtin::builtin_assets()
}

/// Renders one stroke deterministically (PNT-001): output depends only on
/// (confirmed samples, preset, assets, color, seed, base, context), never on batching.
#[derive(Debug)]
pub struct StrokeRenderer {
    preset: BrushPreset,
    color_px: Px16,
    seed: u64,
    base: TileGrid,
    ctx: StrokeContext,
    mode: StrokeMode,
    opacity16: u16,
    wet: Option<Box<[u16; 256]>>,
    shape: Shape,
    pipe: Pipeline,
    bounds: RectI,
    confirmed: Vec<StrokeSample>,
    stab_path: Vec<PathPt>,
    last_t: Option<u64>,
    predicted_rejected: usize,
    invalid_rejected: usize,
    flushed: bool,
    fallbacks: Vec<String>,
}

impl StrokeRenderer {
    /// Start a stroke (CONTRACTS signature; infallible). `base` is the layer at stroke start (a
    /// cheap COW clone). Out-of-range inputs are made safe instead of failing: the preset is
    /// [`BrushPreset::sanitized`], color components are clamped (NaN → 0), invalid overrides are
    /// ignored and an empty clip renders nothing. Image assets are looked up among the built-in
    /// assets only; use [`StrokeRenderer::begin_resolved`] for library / document assets and
    /// [`StrokeRenderer::try_begin`] to get a typed error instead.
    pub fn begin(
        preset: BrushPreset,
        color: ColorRgba,
        seed: u64,
        base: TileGrid,
        ctx: StrokeContext,
    ) -> StrokeRenderer {
        let assets = ResolvedAssets::resolve(&preset, &[builtin_store()]);
        Self::begin_resolved(preset, assets, color, seed, base, ctx)
    }

    /// [`StrokeRenderer::begin`] with caller-resolved image assets. An image the preset needs but
    /// `assets` lacks uses the documented fallback ([`StrokeRenderer::asset_fallbacks`]).
    pub fn begin_resolved(
        preset: BrushPreset,
        assets: ResolvedAssets,
        color: ColorRgba,
        seed: u64,
        base: TileGrid,
        ctx: StrokeContext,
    ) -> StrokeRenderer {
        let preset = preset.sanitized();
        let fallbacks = assets.missing(&preset);
        let in01 = |v: &f32| v.is_finite() && (0.0..=1.0).contains(v);
        let size = ctx
            .size_override
            .filter(|s| s.is_finite() && *s > 0.0 && *s <= MAX_BRUSH_SIZE_PX)
            .unwrap_or(preset.size_px);
        let opacity = ctx.opacity_override.filter(in01).unwrap_or(preset.opacity);
        let stabilization = ctx.stabilization_override.filter(in01).unwrap_or(preset.stabilization);
        let mode = if preset.eraser { StrokeMode::Erase } else { ctx.mode };

        // Per-stroke color jitter (deterministic from the seed).
        let j = preset.jitter;
        let mut color = color;
        if j.per_stroke_color() {
            let u = [
                rand01(seed, STROKE_INDEX, CH_HUE),
                rand01(seed, STROKE_INDEX, CH_SAT),
                rand01(seed, STROKE_INDEX, CH_BRI),
            ];
            let [r, g, b] =
                jitter_hsv([color.r, color.g, color.b], [j.stroke_hue, j.stroke_saturation, j.stroke_brightness], u);
            color = ColorRgba::new(r, g, b, color.a);
        }
        let rgb = [color.r, color.g, color.b].map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });

        // Texture (an unresolved image texture falls back to procedural grain of the same scale).
        let texture = preset.texture.as_ref().map(|t| {
            let kind = match (t.source, &assets.texture) {
                (TextureSource::Image, Some(a)) => {
                    let level = a.level_index_for(t.scale as f64);
                    let lw = a.levels()[level].w as f64;
                    TexKind::Image { asset: a.clone(), level, texel: t.scale as f64 * a.width() as f64 / lw }
                }
                (TextureSource::Image, None) => TexKind::Procedural { scale: (t.scale as f64).clamp(0.5, 256.0) },
                (TextureSource::Procedural, _) => TexKind::Procedural { scale: t.scale as f64 },
            };
            TexStyle { kind, mode: t.mode, invert: t.invert, seed }
        });
        let tip_asset = assets.tip.filter(|_| preset.tip.shape == crate::TipShape::Image);
        let legacy = tip_asset.is_none()
            && preset.tip.roundness == 1.0
            && preset.wet_edges == 0.0
            && !j.per_dab_color()
            && texture.as_ref().is_none_or(|t| {
                matches!(t.kind, TexKind::Procedural { .. }) && t.mode == TextureMode::Fixed && !t.invert
            });
        let route = if legacy {
            let grain = texture.as_ref().map(|t| match t.kind {
                TexKind::Procedural { scale } => {
                    Grain { scale: scale as f32, strength: preset.texture.as_ref().map_or(0.0, |x| x.depth) }
                }
                TexKind::Image { .. } => Grain { scale: 1.0, strength: 0.0 },
            });
            Route::Legacy(DabStyle {
                hardness: preset.tip.hardness as f64,
                accumulation: preset.accumulation,
                grain,
                seed,
            })
        } else {
            Route::General(StampStyle {
                tip: tip_asset,
                hardness: preset.tip.hardness as f64,
                roundness: preset.tip.roundness as f64,
                accumulation: preset.accumulation,
                texture,
            })
        };
        let shape = Shape {
            size,
            spacing: preset.spacing as f64,
            route,
            clip: ctx.clip,
            seed,
            rgb,
            per_dab_color: j.per_dab_color(),
        };
        let wet = (preset.wet_edges > 0.0).then(|| wet_lut(preset.wet_edges));
        StrokeRenderer {
            // color_to_px16 clamps components to [0,1] and maps non-finite values to 0.
            color_px: ops::color_to_px16(color),
            seed,
            base,
            mode,
            opacity16: ops::color_to_px16(ColorRgba::new(0.0, 0.0, 0.0, opacity))[3],
            pipe: Pipeline {
                stab: Stabilizer::new((stabilization * MAX_STABILIZER_ROPE_PX) as f64),
                walker: Walker::default(),
                cov: CoverageGrid::default(),
                depth: wet.is_some().then(DepthGrid::default),
                color: shape.per_dab_color.then(ColorGrid::default),
                vel: Velocity::default(),
                last_dir: None,
                pending: None,
                dabs: 0,
                dropped: 0,
            },
            wet,
            shape,
            preset,
            ctx,
            bounds: RectI::EMPTY,
            confirmed: Vec::new(),
            stab_path: Vec::new(),
            last_t: None,
            predicted_rejected: 0,
            invalid_rejected: 0,
            flushed: false,
            fallbacks,
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
            dirty = dirty.union(&self.pipe.feed(&self.preset, &self.shape, s, Some(&mut self.stab_path)));
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
        self.composite(&self.pipe, target, region);
    }

    fn predicted_pipe(&self, predicted: &[StrokeSample]) -> (Pipeline, RectI) {
        let mut pipe = self.pipe.clone();
        let mut dirty = RectI::EMPTY;
        if !self.flushed {
            for s in predicted {
                if s.validate().is_err() || s.x.abs() > MAX_SAMPLE_COORD || s.y.abs() > MAX_SAMPLE_COORD {
                    continue;
                }
                dirty = dirty.union(&pipe.feed(&self.preset, &self.shape, s, None));
            }
        }
        (pipe, dirty)
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
        let (pipe, dirty) = self.predicted_pipe(predicted);
        self.composite(&pipe, target, dirty);
        dirty
    }

    /// Branch this stroke's coverage with `predicted` samples appended (display only, STY-007).
    /// The renderer's confirmed state is not changed. Used with [`composite_layered`] when one
    /// gesture drives several strokes (symmetry), where a single in-place preview is not enough.
    pub fn predicted_coverage(&self, predicted: &[StrokeSample]) -> PredictedCoverage {
        let (pipe, dirty) = self.predicted_pipe(predicted);
        PredictedCoverage { cov: pipe.cov, depth: pipe.depth, color: pipe.color, dirty }
    }

    /// Finish the stroke (flushes first). Geometry is computed only for vector-eligible presets
    /// in paint mode.
    pub fn finish(self) -> FinishedStroke {
        self.finish_inner(true)
    }

    /// [`StrokeRenderer::finish`] for a stroke whose pixels the caller composites itself with
    /// [`composite_layered`]: samples, bounds, geometry and stats are identical, but
    /// [`FinishedStroke::tiles`] is just the untouched base (the per-stroke composite is skipped).
    pub fn finish_layered(self) -> FinishedStroke {
        self.finish_inner(false)
    }

    fn finish_inner(mut self, composite: bool) -> FinishedStroke {
        self.flush();
        let mut tiles = self.base.clone();
        if composite {
            self.composite(&self.pipe, &mut tiles, self.bounds);
        }
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
                |p| preset.dab_diameter(size, &input_of(p)) as f64,
                preset.tip.hardness as f64,
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

    /// Accumulated stroke coverage at a pixel (`0..=65535`, before wet edges/opacity/selection).
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

    /// Asset hashes the preset references that could not be resolved (rendered with the
    /// fallback round tip / procedural grain).
    pub fn asset_fallbacks(&self) -> &[String] {
        &self.fallbacks
    }

    /// Number of coverage tiles held (memory: 8 KiB each, plus 4 KiB wet depth / 24 KiB color
    /// per tile for presets that use them).
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

    /// `out ⊕ stroke` at one pixel. `k` is the raw coverage, `sel` the selection value if any.
    #[inline]
    fn apply(&self, out: Px16, mut k: u16, depth: Option<u8>, color: Option<[u16; 3]>, sel: Option<u8>) -> Px16 {
        if k != 0 {
            if let (Some(lut), Some(d)) = (&self.wet, depth) {
                k = mul65535(k, lut[d as usize]);
            }
            k = mul65535(k, self.opacity16);
            if let Some(s8) = sel {
                k = mul65535(k, cov8_to_16(s8));
            }
        }
        if k == 0 {
            return out;
        }
        match (self.mode, self.ctx.alpha_lock) {
            (StrokeMode::Erase, _) => ops::erase(out, k),
            (StrokeMode::Paint, lock) => {
                let src = match color {
                    Some(c) => {
                        let ka = mul65535(k, self.color_px[3]);
                        [mul65535(c[0], ka), mul65535(c[1], ka), mul65535(c[2], ka), ka]
                    }
                    None => ops::scale(self.color_px, k),
                };
                if lock { ops::src_atop(out, src) } else { ops::src_over(out, src) }
            }
        }
    }

    fn composite(&self, pipe: &Pipeline, target: &mut TileGrid, region: RectI) {
        let area = region.intersect(&self.ctx.clip);
        if area.is_empty() {
            return;
        }
        let noop = self.mode == StrokeMode::Erase && self.ctx.alpha_lock;
        for c in TileGrid::tiles_in_rect(area) {
            let tr = c.rect();
            let ir = tr.intersect(&area);
            let base_t = self.base.tile(c);
            let cov_t = if noop { None } else { pipe.cov.tile(c) };
            // Erase/alpha-lock cannot change transparent base pixels.
            let unchanged = base_t.is_none() && (self.mode == StrokeMode::Erase || self.ctx.alpha_lock);
            let Some(cov_t) = cov_t.filter(|_| !unchanged) else {
                restore_base(target, base_t, c, ir);
                continue;
            };
            let dep_t = pipe.depth.as_ref().and_then(|d| d.tile(c));
            let col_t = pipe.color.as_ref().and_then(|g| g.tile(c));
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
                    let k = cov_t[i];
                    let s8 = if k != 0 {
                        sel.as_ref().map(|(st, dv)| st.map_or(*dv, |t| t.get(lx as u32, ly as u32)))
                    } else {
                        None
                    };
                    let out =
                        self.apply(b, k, dep_t.map(|d| d[i]).or(self.wet.as_ref().map(|_| 0)), col_t.map(|g| g[i]), s8);
                    px[i * 4..i * 4 + 4].copy_from_slice(&out);
                }
            }
            if base_t.is_none() && tt.is_transparent() {
                target.remove_tile(c);
            }
        }
    }
}

/// Confirmed + predicted coverage of one stroke (see [`StrokeRenderer::predicted_coverage`]).
#[derive(Debug, Clone)]
pub struct PredictedCoverage {
    cov: CoverageGrid,
    depth: Option<DepthGrid>,
    color: Option<ColorGrid>,
    dirty: RectI,
}

impl PredictedCoverage {
    /// Pixels whose coverage the predicted dabs may have changed.
    pub fn dirty(&self) -> RectI {
        self.dirty
    }
}

/// One stroke of a multi-stroke gesture for [`composite_layered`].
#[derive(Debug, Clone, Copy)]
pub struct LayeredPart<'a> {
    pub stroke: &'a StrokeRenderer,
    /// Use this prediction branch instead of the stroke's confirmed coverage.
    pub predicted: Option<&'a PredictedCoverage>,
}

/// Composite several strokes of **one gesture** (a stroke and its symmetry replicas) over `base`
/// into `target` for `region ∩ clip`: `target = base ⊕ parts[0] ⊕ parts[1] ⊕ …`, each `⊕` being
/// exactly the per-pixel operation of [`StrokeRenderer::render_into`] (module docs). The result
/// equals committing the strokes one after another, so the document's stroke records reproduce
/// the pixels. Pixels in the region that no part covers are restored to `base`.
///
/// All parts must have been begun with the same clip (the first part's clip bounds the write);
/// selection, opacity, mode and alpha lock are taken from each part.
pub fn composite_layered(base: &TileGrid, parts: &[LayeredPart<'_>], target: &mut TileGrid, region: RectI) {
    let Some(first) = parts.first() else { return };
    let area = region.intersect(&first.stroke.ctx.clip);
    if area.is_empty() {
        return;
    }
    type PartTiles<'b> = (
        &'b StrokeRenderer,
        &'b Arc<Vec<u16>>,
        Option<&'b Arc<Vec<u8>>>,
        Option<&'b Arc<Vec<[u16; 3]>>>,
        Option<(Option<&'b Arc<veskra_raster::MaskTile>>, u8)>,
    );
    for c in TileGrid::tiles_in_rect(area) {
        let tr = c.rect();
        let ir = tr.intersect(&area);
        let base_t = base.tile(c);
        // Parts that can change this tile: they have coverage here and their mode can act on it.
        let active: Vec<PartTiles<'_>> = parts
            .iter()
            .filter_map(|p| {
                let s = p.stroke;
                if s.mode == StrokeMode::Erase && s.ctx.alpha_lock {
                    return None;
                }
                let (cov, depth, color) = match p.predicted {
                    Some(q) => (&q.cov, q.depth.as_ref(), q.color.as_ref()),
                    None => (&s.pipe.cov, s.pipe.depth.as_ref(), s.pipe.color.as_ref()),
                };
                let sel = s.ctx.selection.as_ref().map(|m| (m.tile(c), m.default_value()));
                cov.tile(c).map(|t| (s, t, depth.and_then(|d| d.tile(c)), color.and_then(|g| g.tile(c)), sel))
            })
            .collect();
        if active.is_empty() {
            restore_base(target, base_t, c, ir);
            continue;
        }
        let tt = target.tile_mut(c);
        let px = tt.pixels_mut();
        for y in ir.y0..ir.y1 {
            let ly = (y - tr.y0) as usize;
            for x in ir.x0..ir.x1 {
                let lx = (x - tr.x0) as usize;
                let i = ly * T + lx;
                let mut out: Px16 = match base_t {
                    Some(t) => {
                        let s = &t.pixels()[i * 4..i * 4 + 4];
                        [s[0], s[1], s[2], s[3]]
                    }
                    None => [0; 4],
                };
                for (s, cov_t, dep_t, col_t, sel) in &active {
                    let k = cov_t[i];
                    if k == 0 {
                        continue;
                    }
                    let s8 = sel.as_ref().map(|(st, dv)| st.map_or(*dv, |t| t.get(lx as u32, ly as u32)));
                    out = s.apply(out, k, dep_t.map(|d| d[i]).or(s.wet.as_ref().map(|_| 0)), col_t.map(|g| g[i]), s8);
                }
                px[i * 4..i * 4 + 4].copy_from_slice(&out);
            }
        }
        if base_t.is_none() && tt.is_transparent() {
            target.remove_tile(c);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_jitter_identity_and_bounds() {
        let c = [0.2f32, 0.5, 0.9];
        let same = jitter_hsv(c, [0.0, 0.0, 0.0], [0.3, 0.7, 0.1]);
        for i in 0..3 {
            assert!((same[i] - c[i]).abs() < 1e-6, "{same:?}");
        }
        let mut seen_diff = false;
        for k in 0..200 {
            let u = [k as f64 / 200.0, (k * 7 % 200) as f64 / 200.0, (k * 13 % 200) as f64 / 200.0];
            let o = jitter_hsv(c, [1.0, 1.0, 1.0], u);
            assert!(o.iter().all(|v| (0.0..=1.0).contains(v)));
            seen_diff |= o != c;
        }
        assert!(seen_diff);
    }

    #[test]
    fn wet_lut_shape() {
        let l = wet_lut(1.0);
        assert_eq!(l[0], 65535);
        assert_eq!(l[40], 65535);
        assert!(l[255] < 17000 && l[255] > 16000, "{}", l[255]);
        assert!(l.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn velocity_is_smoothed_and_bounded() {
        let mut v = Velocity::default();
        assert_eq!(v.update(0.0, 0.0, 0), 0.0);
        let a = v.update(40.0, 0.0, 10_000_000); // 4 px/ms → raw 1
        assert!(a > 0.2 && a < 0.3, "{a}");
        let mut last = a;
        for i in 2..40u64 {
            let b = v.update(40.0 * i as f64, 0.0, i * 10_000_000);
            assert!(b >= last && b <= 1.0);
            last = b;
        }
        assert!(last > 0.99);
        // Same timestamp: unchanged.
        assert_eq!(v.update(9999.0, 0.0, 39 * 10_000_000), last);
    }
}
