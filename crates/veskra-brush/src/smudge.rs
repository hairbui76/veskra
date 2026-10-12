//! Smudge tool: a raster-only brush that picks up the layer's own pixels and drags them along
//! the stroke (the classic "finger painting" smudge). It never uses a paint color.
//!
//! ## Model
//! Each smudge *head* (one per symmetry replica) owns an accumulator `A`: a copy of the
//! premultiplied pixels under its square dab footprint, taken at the first dab. Dabs are placed
//! along the (stabilized) path every `max(1, 0.1·size)` px, centered on the pixel that contains
//! the path point. For every later dab and every footprint cell with canvas pixel `C`:
//!
//! 1. pick-up: `A ← C + strength·(A − C)` (strength 1 keeps the first pick-up forever,
//!    strength 0 forgets it immediately);
//! 2. deposit: `C ← C + m·(A − C)` with `m = mask · pressure · selection`, where `mask` is the
//!    round dab profile ([`SMUDGE_HARDNESS`]) and `pressure` is the sample's **hardware**
//!    pressure when the device reported one, else exactly 1 (no pressure is invented).
//!
//! Consequences that the tests pin: `strength = 0` and smudging a uniform area change no pixel;
//! with alpha lock only the color moves (alpha is restored per pixel, transparent pixels stay
//! transparent); nothing outside the clip or an unselected pixel is written. Footprint cells
//! outside the clip read the nearest pixel inside the footprint∩clip (edge replicate), so the
//! canvas border never drags transparency in. Math is `f32` add/mul only and strictly
//! sequential, so the result depends only on the sample sequence, never on batching.
//!
//! Smudge has no editable vector form and is not recorded as a stroke: the engine commits its
//! pixels as a raster edit.

use veskra_core::limits::MAX_STROKE_SAMPLES;
use veskra_core::{RectI, SampleFlags, StrokeSample};
use veskra_raster::{MaskGrid, TileGrid};

use crate::BrushError;
use crate::dab::profile;
use crate::path::{PathPt, Stabilizer, Walker};
use crate::preset::MAX_STABILIZER_ROPE_PX;
use crate::renderer::MAX_SAMPLE_COORD;

/// Largest smudge dab diameter (document px); larger brush sizes are clamped to it.
pub const MAX_SMUDGE_SIZE_PX: f32 = 256.0;
/// Hardness of the smudge dab profile (see `dab::profile`).
pub const SMUDGE_HARDNESS: f64 = 0.35;
/// Dab spacing as a fraction of the diameter (never below one pixel).
pub const SMUDGE_SPACING: f64 = 0.1;
/// Most heads (symmetry replicas) of one smudge stroke.
pub const MAX_SMUDGE_HEADS: usize = 64;
/// Footprint cells processed per stroke (all heads) before further dabs are dropped and counted.
/// Bounds the work of hostile input to a couple of seconds; a single 256 px head can still
/// smudge along roughly 50 000 px of path in one gesture (a 40 px head about 290 000 px).
pub const MAX_SMUDGE_WORK_CELLS: u64 = 1 << 27;

/// Smudge settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmudgeParams {
    /// Dab diameter in document px (`(0, 1000]`, clamped to [`MAX_SMUDGE_SIZE_PX`]).
    pub size_px: f32,
    /// How long picked-up color is carried, `0..=1`.
    pub strength: f32,
    /// Path stabilization `0..=1` (same rope stabilizer as paint strokes).
    pub stabilization: f32,
}

/// Per-stroke context supplied by the engine.
#[derive(Debug, Clone)]
pub struct SmudgeContext {
    /// Layer alpha lock: only color moves, alpha is unchanged.
    pub alpha_lock: bool,
    /// Active selection; scales how much is deposited per pixel.
    pub selection: Option<MaskGrid>,
    /// Document bounds; nothing is written outside.
    pub clip: RectI,
}

/// Result of [`SmudgeStroke::finish`].
#[derive(Debug, Clone)]
pub struct FinishedSmudge {
    /// The layer after the smudge, sharing every untouched tile with the base.
    pub tiles: TileGrid,
    /// Pixels the smudge may have changed (empty when nothing changed).
    pub bounds: RectI,
    /// Confirmed samples accepted by the first head (the user's own stroke).
    pub accepted_samples: usize,
    /// Dabs processed / dropped by [`MAX_SMUDGE_WORK_CELLS`].
    pub dabs: u64,
    pub dabs_dropped: u64,
}

#[derive(Debug, Clone)]
struct Head {
    stab: Stabilizer,
    walker: Walker,
    /// `n × n × 4` premultiplied pick-up buffer (valid once `primed`).
    accum: Vec<f32>,
    primed: bool,
    last_t: Option<u64>,
    accepted: usize,
}

/// One live smudge gesture working directly on a copy-on-write copy of the layer.
#[derive(Debug, Clone)]
pub struct SmudgeStroke {
    params: SmudgeParams,
    ctx: SmudgeContext,
    tiles: TileGrid,
    heads: Vec<Head>,
    /// `n × n` dab profile, centered on the middle cell.
    mask: Vec<f32>,
    half: i32,
    n: usize,
    step: f64,
    bounds: RectI,
    dabs: u64,
    dropped: u64,
    work: u64,
    invalid_rejected: usize,
    flushed: bool,
}

#[inline]
fn mix(c: f32, a: f32, t: f32) -> f32 {
    // Exact at the ends so "no pick-up" and "keep everything" never drift by rounding.
    if t <= 0.0 {
        c
    } else if t >= 1.0 {
        a
    } else {
        c + t * (a - c)
    }
}

#[inline]
fn quantize(v: f32) -> u16 {
    (v + 0.5).floor().clamp(0.0, 65535.0) as u16
}

impl SmudgeStroke {
    /// Start a smudge on `base` (a cheap COW clone of the layer) with `heads` independent heads.
    pub fn begin(
        base: TileGrid,
        params: SmudgeParams,
        ctx: SmudgeContext,
        heads: usize,
    ) -> Result<SmudgeStroke, BrushError> {
        let in01 = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        if !(params.size_px.is_finite() && params.size_px > 0.0 && params.size_px <= crate::preset::MAX_BRUSH_SIZE_PX) {
            return Err(BrushError::InvalidArgument("smudge size must be in (0, 1000]"));
        }
        if !in01(params.strength) || !in01(params.stabilization) {
            return Err(BrushError::InvalidArgument("smudge strength and stabilization must be in [0,1]"));
        }
        if heads == 0 || heads > MAX_SMUDGE_HEADS {
            return Err(BrushError::InvalidArgument("smudge head count out of range"));
        }
        if ctx.clip.is_empty() {
            return Err(BrushError::InvalidArgument("clip rectangle is empty"));
        }
        let size = f64::from(params.size_px.min(MAX_SMUDGE_SIZE_PX));
        let r = (size / 2.0).max(crate::dab::MIN_RADIUS);
        let half = (r + 0.5).ceil() as i32;
        let n = (2 * half + 1) as usize;
        let mut mask = vec![0f32; n * n];
        for j in 0..n {
            for i in 0..n {
                let (dx, dy) = (i as f64 - f64::from(half), j as f64 - f64::from(half));
                mask[j * n + i] = profile(dx.hypot(dy), r, SMUDGE_HARDNESS) as f32;
            }
        }
        let rope = f64::from(params.stabilization * MAX_STABILIZER_ROPE_PX);
        let head = Head {
            stab: Stabilizer::new(rope),
            walker: Walker::default(),
            accum: vec![0f32; n * n * 4],
            primed: false,
            last_t: None,
            accepted: 0,
        };
        Ok(SmudgeStroke {
            params,
            ctx,
            tiles: base,
            heads: vec![head; heads],
            mask,
            half,
            n,
            step: (size * SMUDGE_SPACING).max(1.0),
            bounds: RectI::EMPTY,
            dabs: 0,
            dropped: 0,
            work: 0,
            invalid_rejected: 0,
            flushed: false,
        })
    }

    /// Feed one confirmed sample to `head`; returns the pixels that changed (conservative rect).
    /// Predicted, hover, invalid, out-of-range or time-reversed samples are skipped and counted.
    pub fn add_sample(&mut self, head: usize, s: &StrokeSample) -> RectI {
        let flushed = self.flushed;
        let Some(hd) = self.heads.get_mut(head) else { return RectI::EMPTY };
        let ok = !flushed
            && s.is_confirmed()
            && !s.flags.contains(SampleFlags::HOVER)
            && s.validate().is_ok()
            && s.x.abs() <= MAX_SAMPLE_COORD
            && s.y.abs() <= MAX_SAMPLE_COORD
            && hd.last_t.is_none_or(|t| s.t_ns >= t)
            && hd.accepted < MAX_STROKE_SAMPLES;
        if !ok {
            self.invalid_rejected += 1;
            return RectI::EMPTY;
        }
        hd.last_t = Some(s.t_ns);
        hd.accepted += 1;
        // Hardware pressure scales the deposit; without it the factor is exactly 1.
        let pressure = if s.flags.contains(SampleFlags::PRESSURE_REPORTED) { s.pressure } else { 1.0 };
        let sp = hd.stab.push(PathPt::at(f64::from(s.x), f64::from(s.y), pressure, None));
        let step = self.step;
        let mut pts = Vec::new();
        hd.walker.push(sp, &|_: &PathPt| step, &mut |q| pts.push(q));
        self.dab_all(head, &pts)
    }

    /// End of input: every head's stabilizer catches up and a final dab lands on the path end.
    /// Idempotent; samples added afterwards are rejected. [`SmudgeStroke::finish`] calls it.
    pub fn flush(&mut self) -> RectI {
        if self.flushed {
            return RectI::EMPTY;
        }
        self.flushed = true;
        let step = self.step;
        let mut dirty = RectI::EMPTY;
        for h in 0..self.heads.len() {
            let hd = &mut self.heads[h];
            let mut pts = Vec::new();
            if let Some(t) = hd.stab.flush() {
                hd.walker.push(t, &|_: &PathPt| step, &mut |q| pts.push(q));
            }
            hd.walker.flush(&mut |q| pts.push(q));
            dirty = dirty.union(&self.dab_all(h, &pts));
        }
        dirty
    }

    fn dab_all(&mut self, head: usize, pts: &[PathPt]) -> RectI {
        let mut dirty = RectI::EMPTY;
        for q in pts {
            dirty = dirty.union(&self.dab(head, *q));
        }
        dirty
    }

    fn dab(&mut self, h: usize, q: PathPt) -> RectI {
        self.dabs += 1;
        let (cx, cy) = (q.x.floor() as i32, q.y.floor() as i32);
        let half = self.half;
        let fp = RectI::new(cx - half, cy - half, cx + half + 1, cy + half + 1);
        let vis = fp.intersect(&self.ctx.clip);
        if vis.is_empty() {
            return RectI::EMPTY;
        }
        let n = self.n;
        let cells = (n * n) as u64;
        if self.work.saturating_add(cells) > MAX_SMUDGE_WORK_CELLS {
            self.dropped += 1;
            return RectI::EMPTY;
        }
        self.work += cells;
        let src = self.tiles.read_rect(vis);
        let vw = vis.width() as usize;
        if src.len() != vw * vis.height() as usize * 4 {
            return RectI::EMPTY;
        }
        // Buffer index of the (edge-replicated) canvas pixel under a footprint cell.
        let at = |x: i32, y: i32| -> usize {
            let xx = (x.clamp(vis.x0, vis.x1 - 1) - vis.x0) as usize;
            let yy = (y.clamp(vis.y0, vis.y1 - 1) - vis.y0) as usize;
            (yy * vw + xx) * 4
        };
        let head = &mut self.heads[h];
        if !head.primed {
            for j in 0..n {
                for i in 0..n {
                    let s = at(fp.x0 + i as i32, fp.y0 + j as i32);
                    for ch in 0..4 {
                        head.accum[(j * n + i) * 4 + ch] = f32::from(src[s + ch]);
                    }
                }
            }
            head.primed = true;
            return RectI::EMPTY;
        }
        let rate = self.params.strength;
        let alpha_lock = self.ctx.alpha_lock;
        // Copy-on-first-change: most dabs over flat color change nothing.
        let mut out: Option<Vec<u16>> = None;
        for j in 0..n {
            let y = fp.y0 + j as i32;
            let row_visible = y >= vis.y0 && y < vis.y1;
            let row = (y.clamp(vis.y0, vis.y1 - 1) - vis.y0) as usize * vw;
            for i in 0..n {
                let mk = self.mask[j * n + i];
                if mk <= 0.0 {
                    // Outside the round profile: never deposited, so its pick-up is never read.
                    continue;
                }
                let x = fp.x0 + i as i32;
                let s = (row + (x.clamp(vis.x0, vis.x1 - 1) - vis.x0) as usize) * 4;
                let c = [f32::from(src[s]), f32::from(src[s + 1]), f32::from(src[s + 2]), f32::from(src[s + 3])];
                let a = &mut head.accum[(j * n + i) * 4..(j * n + i) * 4 + 4];
                if a[..] == c[..] {
                    // Carrying exactly what is already there: nothing to pick up or deposit.
                    continue;
                }
                for ch in 0..4 {
                    a[ch] = mix(c[ch], a[ch], rate);
                }
                if !row_visible || x < vis.x0 || x >= vis.x1 {
                    continue;
                }
                let mut m = mk * q.pressure;
                if let Some(sel) = &self.ctx.selection {
                    m *= f32::from(sel.get(x, y)) / 255.0;
                }
                if m <= 0.0 {
                    continue;
                }
                let o = [mix(c[0], a[0], m), mix(c[1], a[1], m), mix(c[2], a[2], m), mix(c[3], a[3], m)];
                let px = if alpha_lock {
                    // Keep alpha; move only the (straight) color. Transparent pixels stay untouched.
                    if c[3] <= 0.0 || o[3] <= 0.0 {
                        continue;
                    }
                    let k = c[3] / o[3];
                    let al = src[s + 3];
                    [quantize(o[0] * k).min(al), quantize(o[1] * k).min(al), quantize(o[2] * k).min(al), al]
                } else {
                    let al = quantize(o[3]);
                    [quantize(o[0]).min(al), quantize(o[1]).min(al), quantize(o[2]).min(al), al]
                };
                if px != src[s..s + 4] {
                    out.get_or_insert_with(|| src.clone())[s..s + 4].copy_from_slice(&px);
                }
            }
        }
        let Some(out) = out else { return RectI::EMPTY };
        if self.tiles.write_rect(vis, &out).is_err() {
            return RectI::EMPTY;
        }
        self.bounds = self.bounds.union(&vis);
        vis
    }

    /// The layer with everything smudged so far (what the display shows while the stroke is live).
    pub fn tiles(&self) -> &TileGrid {
        &self.tiles
    }

    pub fn bounds(&self) -> RectI {
        self.bounds
    }

    pub fn head_count(&self) -> usize {
        self.heads.len()
    }

    /// Samples skipped as invalid (all heads).
    pub fn invalid_rejected(&self) -> usize {
        self.invalid_rejected
    }

    /// Finish the smudge (flushes first).
    pub fn finish(mut self) -> FinishedSmudge {
        self.flush();
        FinishedSmudge {
            bounds: self.bounds,
            accepted_samples: self.heads.first().map_or(0, |h| h.accepted),
            dabs: self.dabs,
            dabs_dropped: self.dropped,
            tiles: self.tiles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veskra_raster::Px16;

    const CLIP: RectI = RectI::new(0, 0, 96, 64);
    const RED: Px16 = [65535, 0, 0, 65535];
    const BLUE: Px16 = [0, 0, 65535, 65535];

    /// Left half red, right half blue, fully opaque.
    fn two_tone() -> TileGrid {
        let mut g = TileGrid::new();
        for y in 0..64 {
            for x in 0..96 {
                g.put_pixel(x, y, if x < 48 { RED } else { BLUE });
            }
        }
        g
    }

    fn ctx() -> SmudgeContext {
        SmudgeContext { alpha_lock: false, selection: None, clip: CLIP }
    }

    fn params(strength: f32) -> SmudgeParams {
        SmudgeParams { size_px: 16.0, strength, stabilization: 0.0 }
    }

    /// Drag from the red side into the blue side along y = 32.
    fn drag(sm: &mut SmudgeStroke, head: usize, chunk: usize) -> RectI {
        let samples: Vec<StrokeSample> =
            (0..=40).map(|i| StrokeSample::confirmed(30.0 + i as f32, 32.0, i as u64 * 1_000_000, 1.0)).collect();
        let mut d = RectI::EMPTY;
        for c in samples.chunks(chunk) {
            for s in c {
                d = d.union(&sm.add_sample(head, s));
            }
        }
        d.union(&sm.flush())
    }

    fn premul_ok(g: &TileGrid) -> bool {
        g.read_rect(CLIP).chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
    }

    #[test]
    fn drags_color_across_the_boundary() {
        let base = two_tone();
        let mut sm = SmudgeStroke::begin(base.clone(), params(0.9), ctx(), 1).unwrap();
        let dirty = drag(&mut sm, 0, 5);
        let f = sm.finish();
        assert!(premul_ok(&f.tiles));
        let p = f.tiles.get_pixel(54, 32);
        assert!(p[0] > 20_000 && p[2] < 50_000, "red must be dragged into the blue side: {p:?}");
        assert_eq!(p[3], 65535, "opaque stays opaque");
        // Far from the path nothing changes, and every change is inside the reported rect.
        assert_eq!(f.tiles.get_pixel(54, 5), BLUE);
        assert_eq!(f.bounds, dirty);
        for y in 0..64 {
            for x in 0..96 {
                if f.tiles.get_pixel(x, y) != base.get_pixel(x, y) {
                    assert!(dirty.contains(x, y), "({x},{y}) changed outside {dirty:?}");
                }
            }
        }
    }

    #[test]
    fn zero_strength_and_uniform_areas_change_nothing() {
        let base = two_tone();
        let mut sm = SmudgeStroke::begin(base.clone(), params(0.0), ctx(), 1).unwrap();
        assert!(drag(&mut sm, 0, 3).is_empty());
        let f = sm.finish();
        assert_eq!(f.tiles, base);
        assert!(f.bounds.is_empty());
        // Uniform color under the whole path: nothing to drag.
        let mut uni = TileGrid::new();
        for y in 0..64 {
            for x in 0..96 {
                uni.put_pixel(x, y, [30_000, 20_000, 10_000, 40_000]);
            }
        }
        let mut sm = SmudgeStroke::begin(uni.clone(), params(0.8), ctx(), 1).unwrap();
        assert!(drag(&mut sm, 0, 7).is_empty());
        assert_eq!(sm.finish().tiles, uni);
    }

    #[test]
    fn batching_does_not_change_the_result() {
        let run = |chunk: usize| {
            let mut sm = SmudgeStroke::begin(two_tone(), params(0.7), ctx(), 1).unwrap();
            drag(&mut sm, 0, chunk);
            sm.finish().tiles
        };
        assert_eq!(run(1), run(41));
        assert_eq!(run(1), run(6));
    }

    #[test]
    fn alpha_lock_moves_color_only() {
        // Left half opaque red, right half half-transparent blue, top rows transparent.
        let mut base = TileGrid::new();
        for y in 20..64 {
            for x in 0..96 {
                base.put_pixel(x, y, if x < 48 { RED } else { [0, 0, 30_000, 30_000] });
            }
        }
        let mut c = ctx();
        c.alpha_lock = true;
        let mut sm = SmudgeStroke::begin(base.clone(), params(0.9), c, 1).unwrap();
        drag(&mut sm, 0, 4);
        let f = sm.finish();
        assert!(premul_ok(&f.tiles));
        let mut recolored = 0;
        for y in 0..64 {
            for x in 0..96 {
                let (a, b) = (base.get_pixel(x, y), f.tiles.get_pixel(x, y));
                assert_eq!(a[3], b[3], "alpha changed at ({x},{y})");
                recolored += usize::from(a != b);
            }
        }
        assert!(recolored > 50, "{recolored}");
    }

    #[test]
    fn selection_and_clip_bound_the_writes() {
        let base = two_tone();
        let mut c = ctx();
        c.selection = Some(MaskGrid::new_rect(RectI::new(48, 0, 60, 64)));
        c.clip = RectI::new(0, 0, 96, 34);
        let mut sm = SmudgeStroke::begin(base.clone(), params(0.9), c, 1).unwrap();
        drag(&mut sm, 0, 4);
        let f = sm.finish();
        let mut changed = 0;
        for y in 0..64 {
            for x in 0..96 {
                if f.tiles.get_pixel(x, y) != base.get_pixel(x, y) {
                    assert!((48..60).contains(&x) && y < 34, "({x},{y}) is outside selection ∩ clip");
                    changed += 1;
                }
            }
        }
        assert!(changed > 20, "{changed}");
    }

    #[test]
    fn heads_are_independent_and_bad_samples_are_skipped() {
        let mut sm = SmudgeStroke::begin(two_tone(), params(0.9), ctx(), 2).unwrap();
        assert_eq!(sm.head_count(), 2);
        let mut bad = StrokeSample::confirmed(f32::NAN, 1.0, 0, 0.5);
        assert!(sm.add_sample(0, &bad).is_empty());
        bad = StrokeSample::confirmed(5.0e6, 1.0, 0, 0.5);
        assert!(sm.add_sample(1, &bad).is_empty());
        bad = StrokeSample::confirmed(5.0, 1.0, 0, 0.5);
        bad.flags = SampleFlags::PREDICTED;
        assert!(sm.add_sample(0, &bad).is_empty());
        assert!(sm.add_sample(9, &StrokeSample::confirmed(5.0, 1.0, 0, 0.5)).is_empty(), "unknown head");
        assert_eq!(sm.invalid_rejected(), 3);
        drag(&mut sm, 1, 5);
        let f = sm.finish();
        assert_eq!(f.accepted_samples, 0, "head 0 accepted nothing");
        assert!(!f.bounds.is_empty());
    }

    #[test]
    fn invalid_parameters_are_typed_errors() {
        let b = TileGrid::new;
        assert!(SmudgeStroke::begin(b(), params(f32::NAN), ctx(), 1).is_err());
        assert!(SmudgeStroke::begin(b(), params(1.5), ctx(), 1).is_err());
        assert!(SmudgeStroke::begin(b(), SmudgeParams { size_px: 0.0, ..params(0.5) }, ctx(), 1).is_err());
        assert!(SmudgeStroke::begin(b(), SmudgeParams { size_px: f32::INFINITY, ..params(0.5) }, ctx(), 1).is_err());
        assert!(SmudgeStroke::begin(b(), params(0.5), ctx(), 0).is_err());
        assert!(SmudgeStroke::begin(b(), params(0.5), ctx(), MAX_SMUDGE_HEADS + 1).is_err());
        let mut c = ctx();
        c.clip = RectI::EMPTY;
        assert!(SmudgeStroke::begin(b(), params(0.5), c, 1).is_err());
        // A huge size is clamped, not allocated.
        let sm = SmudgeStroke::begin(b(), SmudgeParams { size_px: 1000.0, ..params(0.5) }, ctx(), 1).unwrap();
        assert!(sm.n <= MAX_SMUDGE_SIZE_PX as usize + 4);
    }
}
