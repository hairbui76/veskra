//! Paint-path processing: stabilizer (BRH-05) and arc-length dab placement.
//!
//! Both stages are strictly sequential state machines fed one sample at a time, so their output
//! depends only on the sample sequence and never on how samples were batched (PNT-001).

/// A point on the (stabilized) paint path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PathPt {
    pub x: f64,
    pub y: f64,
    pub pressure: f32,
    pub tilt: Option<f32>,
}

impl PathPt {
    fn dist(&self, o: &PathPt) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }

    fn same_pos(&self, o: &PathPt) -> bool {
        self.x == o.x && self.y == o.y
    }

    fn lerp(&self, o: &PathPt, t: f64) -> PathPt {
        let tf = t as f32;
        PathPt {
            x: self.x + (o.x - self.x) * t,
            y: self.y + (o.y - self.y) * t,
            pressure: self.pressure + (o.pressure - self.pressure) * tf,
            tilt: match (self.tilt, o.tilt) {
                (Some(a), Some(b)) => Some(a + (b - a) * tf),
                _ => None,
            },
        }
    }
}

/// "Pulled rope" stabilizer: the paint point follows the pen at a rope length `rope` px and only
/// moves when the rope is taut, which removes jitter shorter than the rope and rounds corners.
/// Rate-independent (distance based). Pressure/tilt follow the latest raw sample.
/// [`Stabilizer::flush`] appends the last raw sample so no lag remains after the stroke ends.
#[derive(Debug, Clone)]
pub(crate) struct Stabilizer {
    rope: f64,
    cur: Option<PathPt>,
    last_raw: Option<PathPt>,
}

impl Stabilizer {
    pub(crate) fn new(rope: f64) -> Self {
        Self { rope: if rope.is_finite() { rope.max(0.0) } else { 0.0 }, cur: None, last_raw: None }
    }

    /// Feed one raw point; emits exactly one stabilized point.
    pub(crate) fn push(&mut self, raw: PathPt) -> PathPt {
        self.last_raw = Some(raw);
        let next = match self.cur {
            None => raw,
            Some(c) => {
                let d = c.dist(&raw);
                if d > self.rope && d > 0.0 {
                    let k = (d - self.rope) / d;
                    PathPt { x: c.x + (raw.x - c.x) * k, y: c.y + (raw.y - c.y) * k, ..raw }
                } else {
                    PathPt { x: c.x, y: c.y, ..raw }
                }
            }
        };
        self.cur = Some(next);
        next
    }

    /// Catch-up point to the last raw sample, if the stabilized point lags behind it.
    pub(crate) fn flush(&mut self) -> Option<PathPt> {
        let raw = self.last_raw?;
        let lagging = self.cur.is_none_or(|c| !c.same_pos(&raw));
        self.cur = Some(raw);
        lagging.then_some(raw)
    }
}

/// Places dabs along the path at arc-length spacing with distance carry-over between segments.
#[derive(Debug, Clone, Default)]
pub(crate) struct Walker {
    last: Option<PathPt>,
    /// Distance still to travel before the next dab.
    to_next: f64,
    /// Distance travelled since the last dab.
    since_dab: f64,
    /// Total path length so far.
    length: f64,
    /// Highest-pressure point seen while the path has zero length (tap that presses harder).
    tap_peak: Option<PathPt>,
    first_pressure: f32,
}

impl Walker {
    /// Feed a path point; `step` gives the spacing at a dab, `emit` receives dab centers.
    pub(crate) fn push(&mut self, p: PathPt, step: &impl Fn(&PathPt) -> f64, emit: &mut impl FnMut(PathPt)) {
        let Some(a) = self.last else {
            emit(p);
            self.first_pressure = p.pressure;
            self.to_next = step(&p);
            self.last = Some(p);
            return;
        };
        let seg = a.dist(&p);
        if seg <= 0.0 {
            if self.length == 0.0 && p.pressure > self.tap_peak.map_or(self.first_pressure, |t| t.pressure) {
                self.tap_peak = Some(p);
            }
            self.last = Some(p);
            return;
        }
        let mut s = 0.0;
        while s + self.to_next <= seg {
            s += self.to_next;
            let q = a.lerp(&p, s / seg);
            emit(q);
            self.since_dab = 0.0;
            self.to_next = step(&q);
        }
        self.to_next -= seg - s;
        self.since_dab += seg - s;
        self.length += seg;
        self.last = Some(p);
    }

    /// End of stroke: a dab exactly at the path end if the last dab did not land there, or one
    /// at the peak pressure of a stationary tap.
    pub(crate) fn flush(&mut self, emit: &mut impl FnMut(PathPt)) {
        if let Some(t) = self.tap_peak.take()
            && self.length == 0.0
        {
            emit(t);
            return;
        }
        if let Some(l) = self.last
            && self.since_dab > 1e-6
        {
            emit(l);
            self.since_dab = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> PathPt {
        PathPt { x, y, pressure: 1.0, tilt: None }
    }

    #[test]
    fn walker_spacing_and_carry() {
        let mut w = Walker::default();
        let mut dabs = Vec::new();
        let step = |_: &PathPt| 2.0;
        for x in [0.0, 1.5, 3.0, 3.5, 10.0] {
            w.push(pt(x, 0.0), &step, &mut |q| dabs.push(q.x));
        }
        w.flush(&mut |q| dabs.push(q.x));
        let expect = [0.0, 2.0, 4.0, 6.0, 8.0, 10.0];
        assert_eq!(dabs.len(), expect.len(), "{dabs:?}");
        assert!(dabs.iter().zip(expect).all(|(a, b)| (a - b).abs() < 1e-9), "{dabs:?}");
    }

    #[test]
    fn stabilizer_lags_then_catches_up() {
        let mut s = Stabilizer::new(5.0);
        let a = s.push(pt(0.0, 0.0));
        assert_eq!(a.x, 0.0);
        let b = s.push(pt(3.0, 0.0));
        assert_eq!(b.x, 0.0); // within rope
        let c = s.push(pt(20.0, 0.0));
        assert!((c.x - 15.0).abs() < 1e-12);
        let f = s.flush().unwrap();
        assert_eq!(f.x, 20.0);
        assert!(s.flush().is_none());
    }

    #[test]
    fn tap_uses_peak_pressure() {
        let mut w = Walker::default();
        let mut dabs = Vec::new();
        let step = |_: &PathPt| 1.0;
        for p in [0.1f32, 0.6, 0.4] {
            w.push(PathPt { pressure: p, ..pt(5.0, 5.0) }, &step, &mut |q| dabs.push(q.pressure));
        }
        w.flush(&mut |q| dabs.push(q.pressure));
        assert_eq!(dabs, vec![0.1, 0.6]);
    }
}
