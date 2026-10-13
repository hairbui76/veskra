//! Deterministic procedural masks of the second wave of built-in brushes (ADR-019 amendment 1).
//!
//! Same rules as [`crate::procedural`]: integer hashing and IEEE-exact arithmetic only
//! (`+ − × ÷`, `sqrt`, `floor`, `abs`, `min` / `max`). Angles go through [`sin_cos`], a fixed
//! polynomial evaluated with those operations, never the platform libm, so every platform
//! produces byte-identical masks and therefore identical asset content hashes (pinned by
//! `tests/builtin_wave2.rs`). Shapes are drawn by splatting primitives (tapered capsules, discs,
//! ellipses) into a float canvas over their bounding boxes only, which keeps generation cheap.
//! Every shape here is original; nothing is copied or traced from any application or brush pack.

use super::{disc, h01, noise, render, smoothstep, to_u8};

/// `(width, height, coverage)`.
pub(crate) type Mask = (u32, u32, Vec<u8>);

const TAU: f64 = std::f64::consts::TAU;

/// `(sin a, cos a)` from Taylor polynomials (to x²³ / x²⁴) after reduction to `[−π, π)`.
/// IEEE `+ − × ÷` and `floor` only: identical bits on every platform (absolute error < 1e-14).
pub(crate) fn sin_cos(a: f64) -> (f64, f64) {
    let x = a - (a / TAU + 0.5).floor() * TAU;
    let x2 = x * x;
    let mut s = 1.0;
    for k in (1..=11u32).rev() {
        s = 1.0 - x2 / f64::from(2 * k * (2 * k + 1)) * s;
    }
    let mut c = 1.0;
    for k in (1..=12u32).rev() {
        c = 1.0 - x2 / f64::from((2 * k - 1) * (2 * k)) * c;
    }
    (x * s, c)
}

/// Value noise with separate cell sizes and lattice periods per axis (tileable when the image is
/// `period_x · cell_x` × `period_y · cell_y`).
fn vnoise(x: f64, y: f64, cell: (f64, f64), period: (i64, i64), seed: u64) -> f64 {
    let fx = x / cell.0;
    let fy = y / cell.1;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (tx, ty) = (fx - x0, fy - y0);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let lat = |ix: i64, iy: i64| {
        let (ix, iy) = (ix.rem_euclid(period.0), iy.rem_euclid(period.1));
        h01(seed, ((ix as u64) << 32) ^ iy as u64)
    };
    let (ix, iy) = (x0 as i64, y0 as i64);
    let top = lat(ix, iy) + (lat(ix + 1, iy) - lat(ix, iy)) * sx;
    let bot = lat(ix, iy + 1) + (lat(ix + 1, iy + 1) - lat(ix, iy + 1)) * sx;
    top + (bot - top) * sy
}

/// Untiled value noise (huge lattice period).
fn nz(x: f64, y: f64, cell: f64, seed: u64) -> f64 {
    noise(x, y, cell, 1 << 20, seed)
}

fn dist(x: f64, y: f64, cx: f64, cy: f64) -> f64 {
    ((x - cx) * (x - cx) + (y - cy) * (y - cy)).sqrt()
}

/// Antialiased edge: 1 well inside (`d < r`), 0 outside, linear over `aa` px around `r`.
fn edge(d: f64, r: f64, aa: f64) -> f64 {
    ((r - d) / aa + 0.5).clamp(0.0, 1.0)
}

/// Float canvas that primitives are max-splatted into (bounding box only).
#[derive(Debug)]
struct Canvas {
    w: usize,
    h: usize,
    v: Vec<f64>,
    /// Tileable: primitives crossing an edge reappear on the opposite side.
    wrap: bool,
}

impl Canvas {
    fn new(w: u32, h: u32, wrap: bool) -> Self {
        Canvas { w: w as usize, h: h as usize, v: vec![0.0; w as usize * h as usize], wrap }
    }

    /// `v = max(v, f(x, y))` at the pixel centers inside `[x0, x1] × [y0, y1]` (image px).
    fn splat(&mut self, bbox: [f64; 4], f: impl Fn(f64, f64) -> f64) {
        let (w, h) = (self.w as f64, self.h as f64);
        let offsets: &[f64] = if self.wrap { &[-1.0, 0.0, 1.0] } else { &[0.0] };
        for &oy in offsets {
            for &ox in offsets {
                let (ox, oy) = (ox * w, oy * h);
                let xs = ((bbox[0] + ox).floor().max(0.0) as usize).min(self.w);
                let xe = ((bbox[2] + ox).ceil().max(0.0) as usize).min(self.w);
                let ys = ((bbox[1] + oy).floor().max(0.0) as usize).min(self.h);
                let ye = ((bbox[3] + oy).ceil().max(0.0) as usize).min(self.h);
                for y in ys..ye {
                    let row = y * self.w;
                    for x in xs..xe {
                        let val = f(x as f64 + 0.5 - ox, y as f64 + 0.5 - oy);
                        let cur = &mut self.v[row + x];
                        if val > *cur {
                            *cur = val;
                        }
                    }
                }
            }
        }
    }

    /// Capsule from `a` to `b` whose radius goes linearly from `ra` to `rb`, value `ink`.
    fn taper(&mut self, a: (f64, f64), b: (f64, f64), ra: f64, rb: f64, ink: f64) {
        let m = ra.max(rb) + 1.0;
        let bbox = [a.0.min(b.0) - m, a.1.min(b.1) - m, a.0.max(b.0) + m, a.1.max(b.1) + m];
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let len2 = (ex * ex + ey * ey).max(1e-12);
        self.splat(bbox, |x, y| {
            let t = (((x - a.0) * ex + (y - a.1) * ey) / len2).clamp(0.0, 1.0);
            let d = dist(x, y, a.0 + ex * t, a.1 + ey * t);
            ink * edge(d, ra + (rb - ra) * t, 1.0)
        });
    }

    /// Quadratic Bézier `a → c → b` drawn as `n` tapered capsules (radius `r.0` → `r.1`).
    fn curve(&mut self, a: (f64, f64), c: (f64, f64), b: (f64, f64), r: (f64, f64), ink: f64, n: u32) {
        let (ra, rb) = r;
        let at = |t: f64| {
            let u = 1.0 - t;
            (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1)
        };
        for i in 0..n {
            let (t0, t1) = (f64::from(i) / f64::from(n), f64::from(i + 1) / f64::from(n));
            self.taper(at(t0), at(t1), ra + (rb - ra) * t0, ra + (rb - ra) * t1, ink);
        }
    }

    fn disc(&mut self, c: (f64, f64), r: f64, ink: f64, aa: f64) {
        let m = r + aa;
        self.splat([c.0 - m, c.1 - m, c.0 + m, c.1 + m], |x, y| ink * edge(dist(x, y, c.0, c.1), r, aa));
    }

    /// Ellipse with semi-axes `a` (along the direction `(cos, sin)`) and `b`.
    fn ellipse(&mut self, c: (f64, f64), a: f64, b: f64, (sin, cos): (f64, f64), ink: f64) {
        let m = a.max(b) + 1.0;
        self.splat([c.0 - m, c.1 - m, c.0 + m, c.1 + m], |x, y| {
            let (dx, dy) = (x - c.0, y - c.1);
            let u = dx * cos + dy * sin;
            let v = -dx * sin + dy * cos;
            let dn = ((u / a) * (u / a) + (v / b) * (v / b)).sqrt();
            ink * edge(dn * b, b, 1.0)
        });
    }

    fn finish(self) -> Vec<u8> {
        self.v.into_iter().map(to_u8).collect()
    }
}

/// `count` points in the disc `(cx, cy, r)` (rejection sampling; deterministic).
fn points_in_disc(seed: u64, count: usize, cx: f64, cy: f64, r: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(count);
    let mut n = 0u64;
    while out.len() < count && n < 100_000 {
        let x = cx - r + 2.0 * r * h01(seed, 2 * n);
        let y = cy - r + 2.0 * r * h01(seed, 2 * n + 1);
        if dist(x, y, cx, cy) <= r {
            out.push((x, y));
        }
        n += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------- tips

/// Sumi-e brush head for a direction-following tip: an ink-loaded core whose density is broken
/// into streaks along the stroke, plus bristle lines that thin and dry out towards the rim.
pub(crate) fn sumi() -> Mask {
    let s = 0x5E11_0001;
    let mut c = Canvas::new(128, 128, false);
    c.splat([0.0, 0.0, 128.0, 128.0], |x, y| {
        let streak = vnoise(x, y, (40.0, 2.6), (1 << 20, 1 << 20), s ^ 7);
        disc(dist(x, y, 64.0, 64.0), 42.0, 14.0) * (0.55 + 0.45 * smoothstep(0.2, 0.45, streak))
    });
    for i in 0..72u64 {
        let k = |j: u64| h01(s, 8 * i + j);
        let y = 8.0 + 112.0 * k(0);
        let dy = ((y - 64.0) / 56.0).abs().min(1.0);
        let half = 58.0 * (1.0 - dy * dy).sqrt();
        let x0 = 64.0 - half * (0.55 + 0.45 * k(1));
        let x1 = 64.0 + half * (0.5 + 0.5 * k(2));
        let r = (1.2 + 2.2 * k(3)) * (1.0 - 0.4 * dy);
        let ink = (1.0 - 0.35 * dy) * (0.8 + 0.2 * k(4));
        c.taper((x0, y), (x1, y + 3.0 * (k(5) - 0.5)), r * 0.55, r, ink);
    }
    (128, 128, c.finish())
}

/// Dry flat brush (direction-following): sparse bristles in a band across the stroke, with gaps.
pub(crate) fn dry_bristle() -> Mask {
    let s = 0xD7B2_0002;
    let mut c = Canvas::new(128, 128, false);
    let n = 23u64;
    for i in 0..n {
        let k = |j: u64| h01(s, 8 * i + j);
        if k(0) < 0.16 {
            continue;
        }
        let y = 8.0 + 112.0 * (i as f64 + 0.2 + 0.6 * k(1)) / n as f64;
        let (x0, x1) = (50.0 - 10.0 * k(2), 74.0 + 10.0 * k(3));
        let r = 1.4 + 1.8 * k(4);
        c.taper((x0, y), (x1, y + 2.0 * (k(6) - 0.5)), r, r * 0.6, 0.75 + 0.25 * k(5));
    }
    (128, 128, c.finish())
}

/// Watercolor bloom: an irregular, cauliflower-edged blob with mottled interior.
pub(crate) fn bloom() -> Mask {
    let s = 0xB100_0003;
    let m = render(128, 128, |x, y| {
        let d = dist(x, y, 64.0, 64.0);
        let (ux, uy) = ((x - 64.0) / d.max(1e-9), (y - 64.0) / d.max(1e-9));
        let (ex, ey) = (64.0 + 40.0 * ux, 64.0 + 40.0 * uy);
        let e = 0.6 * nz(ex, ey, 13.0, s) + 0.4 * nz(ex, ey, 5.0, s ^ 1);
        let r = 34.0 + 26.0 * e;
        disc(d, r, 6.0) * (0.78 + 0.22 * nz(x, y, 9.0, s ^ 2))
    });
    (128, 128, m)
}

/// Grass clump: tapered, slightly curved blades rising from a base below the center.
pub(crate) fn grass() -> Mask {
    let s = 0x6A55_0006;
    let mut c = Canvas::new(128, 128, false);
    for i in 0..17u64 {
        let k = |j: u64| h01(s, 8 * i + j);
        let base = (28.0 + 72.0 * k(0), 100.0 + 8.0 * k(1));
        let height = 40.0 + 52.0 * k(2);
        let lean = (k(3) - 0.5) * 64.0;
        let tip = (base.0 + lean, base.1 - height);
        let ctrl = (base.0 + lean * 0.1 + (k(4) - 0.5) * 12.0, base.1 - height * 0.62);
        c.curve(base, ctrl, tip, (1.7 + 1.9 * k(5), 0.2), 0.75 + 0.25 * k(6), 10);
    }
    (128, 128, c.finish())
}

/// Leaf pointing up: a curved blade with a stem, midrib and lighter side veins.
pub(crate) fn leaf() -> Mask {
    let m = render(96, 96, |x, y| {
        let t = (86.0 - y) / 78.0;
        let xc = 48.0 + 7.0 * t * t;
        let ax = (x - xc).abs();
        if t < 0.0 {
            // Stem.
            return if t > -0.1 { edge(ax, 1.4, 1.0) } else { 0.0 };
        }
        if t > 1.0 {
            return 0.0;
        }
        let half = 100.0 * t * (1.0 - t) * (1.3 - 0.6 * t);
        let mut v = edge(ax, half, 1.0);
        if ax < 1.1 {
            v *= 0.55;
        } else {
            let k = (86.0 - y) - 0.9 * ax;
            let f = k / 11.0 - (k / 11.0).floor();
            if f < 0.09 && ax < half - 2.0 {
                v *= 0.72;
            }
        }
        v
    });
    (96, 96, m)
}

/// Fur tuft (direction-following): short tapered hairs that mostly point along the stroke.
pub(crate) fn fur() -> Mask {
    let s = 0xF0A2_0008;
    let mut c = Canvas::new(128, 128, false);
    let roots = points_in_disc(s, 90, 56.0, 64.0, 42.0);
    for (i, &(rx, ry)) in roots.iter().enumerate() {
        let k = |j: u64| h01(s ^ 0xFF, 8 * i as u64 + j);
        let (sn, cs) = sin_cos((k(0) - 0.5) * 0.75);
        let len = 16.0 + 26.0 * k(1);
        let tip = (rx + len * cs, ry + len * sn);
        let bend = (k(2) - 0.5) * 7.0;
        let ctrl = (rx + 0.5 * len * cs - bend * sn, ry + 0.5 * len * sn + bend * cs);
        c.curve((rx, ry), ctrl, tip, (1.3 + 0.9 * k(3), 0.15), 0.6 + 0.4 * k(4), 6);
    }
    (128, 128, c.finish())
}

/// Hair rake (direction-following): a column of fine bristles across the stroke.
pub(crate) fn hair() -> Mask {
    let s = 0x4A12_0009;
    let mut c = Canvas::new(64, 64, false);
    let n = 11u64;
    for i in 0..n {
        let k = |j: u64| h01(s, 4 * i + j);
        let y = 4.0 + 56.0 * (i as f64 + 0.5 + 0.6 * (k(0) - 0.5)) / n as f64;
        c.disc((32.0 + 4.0 * (k(1) - 0.5), y), 1.4 + 1.3 * k(2), 0.55 + 0.45 * k(3), 1.0);
    }
    (64, 64, c.finish())
}

/// Twig (direction-following): a gently curved stick with alternating side shoots.
pub(crate) fn twig() -> Mask {
    let s = 0x7716_000A;
    let mut c = Canvas::new(128, 128, false);
    let (a, ctl, b) = ((6.0, 66.0), (64.0, 57.0), (122.0, 63.0));
    c.curve(a, ctl, b, (4.6, 1.6), 1.0, 12);
    let at = |t: f64| {
        let u = 1.0 - t;
        (u * u * a.0 + 2.0 * u * t * ctl.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * ctl.1 + t * t * b.1)
    };
    for (i, &t) in [0.22, 0.42, 0.6, 0.76, 0.9].iter().enumerate() {
        let k = |j: u64| h01(s, 8 * i as u64 + j);
        let side = if i % 2 == 0 { -1.0 } else { 1.0 };
        let p = at(t);
        let (sn, cs) = sin_cos(side * (0.5 + 0.35 * k(0)));
        let len = (36.0 - 22.0 * t) * (0.75 + 0.5 * k(1));
        let end = (p.0 + len * cs, p.1 + len * sn);
        let r0 = 3.0 - 1.4 * t;
        c.curve(p, (p.0 + 0.5 * len * cs, p.1 + 0.5 * len * sn - side * 2.0), end, (r0, 0.7), 1.0, 5);
        if len > 20.0 {
            let q = (p.0 + 0.6 * len * cs, p.1 + 0.6 * len * sn);
            let (sn2, cs2) = sin_cos(side * (0.95 + 0.3 * k(2)));
            let l2 = 8.0 + 6.0 * k(3);
            c.taper(q, (q.0 + l2 * cs2, q.1 + l2 * sn2), r0 * 0.55, 0.5, 1.0);
        }
    }
    (128, 128, c.finish())
}

/// Foliage clump: many small leaves, denser in the middle, with gaps between them.
pub(crate) fn foliage() -> Mask {
    let s = 0xF011_000B;
    let mut c = Canvas::new(128, 128, false);
    let mut placed = 0u64;
    let mut n = 0u64;
    while placed < 54 && n < 10_000 {
        let i = n;
        n += 1;
        let k = |j: u64| h01(s, 8 * i + j);
        let (x, y) = (14.0 + 100.0 * k(0), 14.0 + 100.0 * k(1));
        let d = dist(x, y, 64.0, 64.0) / 50.0;
        if d > 1.0 || k(2) > 1.0 - 0.55 * d * d {
            continue;
        }
        placed += 1;
        let rot = sin_cos(TAU * k(3));
        c.ellipse((x, y), 6.5 + 5.0 * k(4), 3.2 + 2.0 * k(5), rot, 0.7 + 0.3 * k(6));
    }
    (128, 128, c.finish())
}

/// Five-petal flower with a lighter center.
pub(crate) fn flower() -> Mask {
    let mut c = Canvas::new(64, 64, false);
    for k in 0..5u32 {
        let ang = f64::from(k) * TAU / 5.0 - TAU / 4.0;
        let (sn, cs) = sin_cos(ang);
        c.ellipse((32.0 + 13.5 * cs, 32.0 + 13.5 * sn), 12.0, 7.2, (sn, cs), 1.0);
    }
    let mut v = c.v;
    for y in 0..64 {
        for x in 0..64 {
            let d = dist(x as f64 + 0.5, y as f64 + 0.5, 32.0, 32.0);
            let center = edge(d, 5.5, 1.0);
            let i = y * 64 + x;
            v[i] = v[i] * (1.0 - center) + 0.45 * center;
        }
    }
    (64, 64, v.into_iter().map(to_u8).collect())
}

/// Four-point sparkle with short diagonal rays and a soft glow.
pub(crate) fn star() -> Mask {
    let arm = |along: f64, across: f64, len: f64, width: f64| {
        if along >= len {
            return 0.0;
        }
        let k = 1.0 - along / len;
        edge(across, 0.5 + width * k * k * k, 1.0) * (0.5 + 0.5 * k)
    };
    let m = render(128, 128, |x, y| {
        let (dx, dy) = ((x - 64.0).abs(), (y - 64.0).abs());
        let main = arm(dx, dy, 62.0, 6.0).max(arm(dy, dx, 62.0, 6.0));
        let r2 = std::f64::consts::FRAC_1_SQRT_2;
        let (u, v) = ((dx + dy) * r2, (dx - dy).abs() * r2);
        let diag = arm(u, v, 26.0, 2.6);
        let d = dist(x, y, 64.0, 64.0);
        let q = d / 7.0;
        let glow = 1.0 / (1.0 + q * q);
        main.max(0.8 * diag).max(0.75 * glow).max(edge(d, 4.0, 1.5))
    });
    (128, 128, m)
}

/// Mist: a very soft blob with drifting noise.
pub(crate) fn mist() -> Mask {
    let s = 0x3157_000E;
    let m = render(128, 128, |x, y| {
        let env = 1.0 - smoothstep(8.0, 63.0, dist(x, y, 64.0, 64.0));
        let n = 0.5 * nz(x, y, 18.0, s) + 0.3 * nz(x, y, 9.0, s ^ 1) + 0.2 * nz(x, y, 4.5, s ^ 2);
        env * (0.2 + 0.8 * smoothstep(0.3, 0.75, n))
    });
    (128, 128, m)
}

/// Cumulus puff with a flattened base.
pub(crate) fn cloud() -> Mask {
    let s = 0xC10D_000F;
    let puffs: [(f64, f64, f64); 8] = [
        (34.0, 80.0, 19.0),
        (56.0, 74.0, 24.0),
        (80.0, 76.0, 22.0),
        (99.0, 84.0, 15.0),
        (46.0, 57.0, 21.0),
        (70.0, 50.0, 27.0),
        (90.0, 63.0, 18.0),
        (22.0, 88.0, 12.0),
    ];
    let m = render(128, 128, |x, y| {
        let mut v: f64 = 0.0;
        for (cx, cy, r) in puffs {
            v = v.max(disc(dist(x, y, cx, cy), r, 8.0));
        }
        v * (1.0 - smoothstep(90.0, 103.0, y)) * (0.82 + 0.18 * nz(x, y, 9.0, s))
    });
    (128, 128, m)
}

/// Rain: thin vertical streaks of different lengths, thicker at their lower end.
pub(crate) fn rain() -> Mask {
    let s = 0x8A10_0010;
    let mut c = Canvas::new(64, 128, false);
    for i in 0..9u64 {
        let k = |j: u64| h01(s, 8 * i + j);
        let x = 5.0 + 54.0 * k(0);
        let y0 = 4.0 + 52.0 * k(1);
        let y1 = (y0 + 28.0 + 62.0 * k(2)).min(124.0);
        let r = 0.7 + 0.8 * k(3);
        c.taper((x, y0), (x, y1), r * 0.25, r, 0.45 + 0.55 * k(4));
    }
    (64, 128, c.finish())
}

/// Bokeh disc: a lighter interior with a bright rim.
pub(crate) fn bokeh() -> Mask {
    let m = render(96, 96, |x, y| {
        let d = dist(x, y, 48.0, 48.0);
        (0.48 + 0.52 * smoothstep(30.0, 41.0, d)) * disc(d, 43.5, 2.4)
    });
    (96, 96, m)
}

/// Light ray: a long soft beam fading in and out along its length.
pub(crate) fn ray() -> Mask {
    let m = render(32, 256, |x, y| {
        let ax = (x - 16.0).abs();
        let across = 0.6 * (1.0 - smoothstep(0.0, 15.5, ax)) + 0.4 * (1.0 - smoothstep(0.0, 6.0, ax));
        let t = y / 256.0;
        across * smoothstep(0.0, 0.3, t) * (1.0 - smoothstep(0.5, 1.0, t))
    });
    (32, 256, m)
}

/// Ink splatter: an irregular drop, satellite droplets and a few streaks.
pub(crate) fn splatter() -> Mask {
    let s = 0x5B1A_0013;
    let mut c = Canvas::new(128, 128, false);
    c.splat([30.0, 30.0, 98.0, 98.0], |x, y| {
        let d = dist(x, y, 64.0, 64.0);
        let (ux, uy) = ((x - 64.0) / d.max(1e-9), (y - 64.0) / d.max(1e-9));
        let e = nz(64.0 + 30.0 * ux, 64.0 + 30.0 * uy, 8.0, s);
        edge(d, 13.0 + 10.0 * e, 1.2)
    });
    for i in 0..30u64 {
        let k = |j: u64| h01(s ^ 1, 8 * i + j);
        let (sn, cs) = sin_cos(TAU * k(0));
        let r = 22.0 + 38.0 * k(1);
        let size = (0.9 + 4.6 * k(2)) * (1.0 - 0.6 * (r - 22.0) / 38.0);
        c.disc((64.0 + r * cs, 64.0 + r * sn), size, 1.0, 1.0);
    }
    for i in 0..5u64 {
        let k = |j: u64| h01(s ^ 2, 8 * i + j);
        let (sn, cs) = sin_cos(TAU * (f64::from(i as u32) + k(0)) / 5.0);
        let len = 30.0 + 26.0 * k(1);
        c.taper((64.0 + 12.0 * cs, 64.0 + 12.0 * sn), (64.0 + len * cs, 64.0 + len * sn), 3.2, 0.9, 1.0);
    }
    (128, 128, c.finish())
}

/// Glow: a hard core inside a long soft halo.
pub(crate) fn glow() -> Mask {
    let m = render(128, 128, |x, y| {
        let d = dist(x, y, 64.0, 64.0);
        let t = 1.0 - smoothstep(6.0, 64.0, d);
        edge(d, 11.0, 1.5).max(0.8 * t * t.sqrt())
    });
    (128, 128, m)
}

/// Stipple: crisp, evenly spaced dots (minimum distance between dots) inside a disc.
pub(crate) fn stipple() -> Mask {
    let s = 0x5717_0016;
    let mut c = Canvas::new(128, 128, false);
    let mut dots: Vec<(f64, f64)> = Vec::new();
    let mut n = 0u64;
    while dots.len() < 40 && n < 20_000 {
        let (x, y) = (6.0 + 116.0 * h01(s, 2 * n), 6.0 + 116.0 * h01(s, 2 * n + 1));
        n += 1;
        if dist(x, y, 64.0, 64.0) > 56.0 || dots.iter().any(|&(px, py)| dist(x, y, px, py) < 14.0) {
            continue;
        }
        dots.push((x, y));
    }
    for (i, &p) in dots.iter().enumerate() {
        c.disc(p, 3.4 + 1.6 * h01(s ^ 3, i as u64), 1.0, 1.0);
    }
    (128, 128, c.finish())
}

/// Impasto head (direction-following): a dense blob with a ragged rim and streaks along the
/// stroke.
pub(crate) fn impasto() -> Mask {
    let s = 0x1A90_0019;
    let m = render(128, 128, |x, y| {
        let d = dist(x, y, 64.0, 64.0);
        let (ux, uy) = ((x - 64.0) / d.max(1e-9), (y - 64.0) / d.max(1e-9));
        let e = nz(64.0 + 40.0 * ux, 64.0 + 40.0 * uy, 9.0, s);
        let streak = vnoise(x, y, (48.0, 3.0), (1 << 20, 1 << 20), s ^ 1);
        disc(d, 48.0 + 12.0 * e, 3.0) * (0.72 + 0.28 * smoothstep(0.25, 0.7, streak))
    });
    (128, 128, m)
}

/// Fan brush (direction-following): short bristles spread along an arc across the stroke.
pub(crate) fn fan() -> Mask {
    let s = 0xFA90_001B;
    let mut c = Canvas::new(128, 128, false);
    let n = 30u64;
    for i in 0..n {
        let k = |j: u64| h01(s, 8 * i + j);
        if k(0) < 0.15 {
            continue;
        }
        let y = 8.0 + 112.0 * (i as f64 + 0.5 + 0.5 * (k(1) - 0.5)) / n as f64;
        let t = (y - 64.0) / 58.0;
        let x = 54.0 + 18.0 * (1.0 - t * t);
        let (dx, dy) = (x + 60.0, y - 64.0);
        let l = (dx * dx + dy * dy).sqrt();
        let (ux, uy) = (dx / l, dy / l);
        let half = 5.0 + 6.0 * k(2);
        c.taper(
            (x - half * ux, y - half * uy),
            (x + half * ux, y + half * uy),
            1.0 + 1.0 * k(3),
            0.6,
            0.5 + 0.5 * k(4),
        );
    }
    (128, 128, c.finish())
}

/// Conté crayon: a rough rounded square with pitted interior.
pub(crate) fn conte() -> Mask {
    let s = 0xC07E_001C;
    let m = render(64, 64, |x, y| {
        let (dx, dy) = ((x - 32.0).abs(), (y - 32.0).abs());
        let half = 23.0 + 5.0 * (nz(x, y, 6.0, s) - 0.5);
        let rc = 5.0;
        let (qx, qy) = (dx - (half - rc), dy - (half - rc));
        let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
        let sd = outside + qx.max(qy).min(0.0) - rc;
        let pits = 0.68 + 0.32 * smoothstep(0.28, 0.6, nz(x, y, 2.5, s ^ 1));
        (0.5 - sd / 1.2).clamp(0.0, 1.0) * pits
    });
    (64, 64, m)
}

// ------------------------------------------------------------------------- tileable textures

/// Watercolor granulation (128 px tile): pigment gathering in clumps; white = paint passes.
pub(crate) fn granulation() -> Mask {
    let s = 0x6A4A_0004;
    let m = render(128, 128, |x, y| {
        let n = 0.5 * noise(x, y, 4.0, 32, s) + 0.3 * noise(x, y, 2.0, 64, s ^ 1) + 0.2 * noise(x, y, 8.0, 16, s ^ 2);
        0.3 + 0.7 * smoothstep(0.4, 0.66, n)
    });
    (128, 128, m)
}

/// Watercolor salt (128 px tile): feathery pale spots where salt pulled the pigment away.
pub(crate) fn salt() -> Mask {
    let s = 0x5A17_0005;
    let mut c = Canvas::new(128, 128, true);
    for i in 0..36u64 {
        let k = |j: u64| h01(s, 8 * i + j);
        let (cx, cy, r) = (128.0 * k(0), 128.0 * k(1), 2.5 + 5.0 * k(2));
        let seed = s ^ (i + 1);
        c.splat([cx - 2.0 * r, cy - 2.0 * r, cx + 2.0 * r, cy + 2.0 * r], |x, y| {
            let d = dist(x, y, cx, cy);
            let (ux, uy) = ((x - cx) / d.max(1e-9), (y - cy) / d.max(1e-9));
            let spike = nz(20.0 + 6.0 * ux, 20.0 + 6.0 * uy, 1.6, seed);
            let de = d / (0.6 + 0.9 * spike);
            1.0 - smoothstep(0.35 * r, r, de)
        });
    }
    let v: Vec<u8> = c.v.into_iter().map(|v| to_u8(1.0 - 0.85 * v)).collect();
    (128, 128, v)
}

/// Halftone screen (12 px tile): round dots on a 45° lattice; white = paint passes.
pub(crate) fn halftone() -> Mask {
    let m = render(12, 12, |x, y| {
        let (dx, dy) = (x.min(12.0 - x), y.min(12.0 - y));
        let d = (dx * dx + dy * dy).sqrt().min(dist(x, y, 6.0, 6.0));
        edge(d, 3.0, 1.0)
    });
    (12, 12, m)
}

/// Fish scales (32 px tile): overlapping arcs, lower rows on top.
pub(crate) fn scales() -> Mask {
    let m = render(32, 32, |x, y| {
        let row0 = (y / 16.0).floor() as i64;
        for j in (row0 - 1..=row0 + 2).rev() {
            let off = if j.rem_euclid(2) == 1 { 16.0 } else { 0.0 };
            for k in -1..=2i64 {
                let d = dist(x, y, 32.0 * k as f64 + off, 16.0 * j as f64);
                if d < 16.0 {
                    let line = edge((16.0 - d).abs(), 1.25, 1.0);
                    return line.max(0.3 * smoothstep(9.0, 15.0, d));
                }
            }
        }
        0.0
    });
    (32, 32, m)
}

/// Cross-hatching (16 px tile): fine lines along both diagonals.
pub(crate) fn crosshatch() -> Mask {
    let m = render(16, 16, |x, y| {
        let line = |a: f64| {
            let r = a.rem_euclid(16.0);
            edge(r.min(16.0 - r) * std::f64::consts::FRAC_1_SQRT_2, 0.85, 0.9)
        };
        line(x - y).max(line(x + y))
    });
    (16, 16, m)
}

/// Honeycomb (26 × 45 px tile, regular hexagons to 0.1 %): cell outlines.
pub(crate) fn honeycomb() -> Mask {
    let (w, rh) = (26.0, 22.5);
    let m = render(26, 45, |x, y| {
        // Distance to the Voronoi edge between the two nearest cell centers.
        let (mut d1, mut d2) = (f64::MAX, f64::MAX);
        let (mut c1, mut c2) = ((0.0, 0.0), (0.0, 0.0));
        for j in -1..=3i64 {
            let off = if j.rem_euclid(2) == 1 { w / 2.0 } else { 0.0 };
            for i in -1..=2i64 {
                let c = (w * i as f64 + off, rh * j as f64);
                let d = dist(x, y, c.0, c.1);
                if d < d1 {
                    (d2, c2) = (d1, c1);
                    (d1, c1) = (d, c);
                } else if d < d2 {
                    (d2, c2) = (d, c);
                }
            }
        }
        let gap = (d2 * d2 - d1 * d1) / (2.0 * dist(c1.0, c1.1, c2.0, c2.1));
        edge(gap, 1.1, 1.0)
    });
    (26, 45, m)
}

/// Paint ridges (64 px tile) for a texture that follows the dab: streaks along the stroke.
pub(crate) fn ridges() -> Mask {
    let s = 0x41D6_001A;
    let m = render(64, 64, |x, y| {
        let n = 0.6 * vnoise(x, y, (16.0, 4.0), (4, 16), s) + 0.4 * vnoise(x, y, (8.0, 2.0), (8, 32), s ^ 1);
        0.4 + 0.6 * smoothstep(0.25, 0.75, n)
    });
    (64, 64, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_cos_is_accurate() {
        for i in -400..=400 {
            let a = i as f64 * 0.0371;
            let (s, c) = sin_cos(a);
            assert!((s - a.sin()).abs() < 1e-12 && (c - a.cos()).abs() < 1e-12, "{a}");
        }
    }

    #[test]
    fn tiles_wrap_seamlessly() {
        // Opposite edges of every tile differ no more than neighbouring columns/rows inside.
        for (name, f) in [
            ("granulation", granulation as fn() -> Mask),
            ("salt", salt),
            ("halftone", halftone),
            ("scales", scales),
            ("crosshatch", crosshatch),
            ("honeycomb", honeycomb),
            ("ridges", ridges),
        ] {
            let (w, h, m) = f();
            let at = |x: u32, y: u32| i32::from(m[(y % h * w + x % w) as usize]);
            let seam_x: i32 = (0..h).map(|y| (at(w - 1, y) - at(0, y)).abs()).max().unwrap_or(0);
            let seam_y: i32 = (0..w).map(|x| (at(x, h - 1) - at(x, 0)).abs()).max().unwrap_or(0);
            let inner: i32 = (0..h)
                .flat_map(|y| (0..w - 1).map(move |x| (x, y)))
                .map(|(x, y)| (at(x, y) - at(x + 1, y)).abs().max((at(x, y) - at(x, y + 1)).abs()))
                .max()
                .unwrap_or(0);
            assert!(seam_x <= inner + 8 && seam_y <= inner + 8, "{name}: seams {seam_x}/{seam_y}, inner {inner}");
        }
    }
}
