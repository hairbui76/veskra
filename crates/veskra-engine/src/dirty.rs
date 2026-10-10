//! A small set of dirty rectangles.
//!
//! One gesture can touch far-apart areas (a stroke and its symmetry replicas). Reporting their
//! bounding box would make the UI refresh almost the whole canvas for every pointer packet, so
//! the engine keeps disjoint rectangles apart: rectangles that overlap are merged, and the set
//! collapses to its bounding box beyond [`MAX_DIRTY_RECTS`]. The union of the set always covers
//! every rectangle that was added.

use veskra_core::RectI;

/// Most rectangles kept before the set collapses to its bounding box.
pub(crate) const MAX_DIRTY_RECTS: usize = 48;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DirtySet {
    rects: Vec<RectI>,
}

impl DirtySet {
    /// Add a rectangle (empty ones are ignored), merging it with every rectangle it overlaps.
    pub(crate) fn add(&mut self, r: RectI) {
        if r.is_empty() {
            return;
        }
        let mut r = r;
        // A merge can make the grown rectangle reach others: repeat until nothing overlaps.
        loop {
            let before = self.rects.len();
            let mut i = 0;
            while i < self.rects.len() {
                if self.rects[i].intersect(&r).is_empty() {
                    i += 1;
                } else {
                    r = r.union(&self.rects.swap_remove(i));
                }
            }
            if self.rects.len() == before {
                break;
            }
        }
        self.rects.push(r);
        if self.rects.len() > MAX_DIRTY_RECTS {
            let all = self.bounding();
            self.rects.clear();
            self.rects.push(all);
        }
    }

    pub(crate) fn extend(&mut self, other: &DirtySet) {
        for r in &other.rects {
            self.add(*r);
        }
    }

    pub(crate) fn rects(&self) -> &[RectI] {
        &self.rects
    }

    pub(crate) fn into_rects(self) -> Vec<RectI> {
        self.rects
    }

    /// Bounding box of the whole set (`RectI::EMPTY` when empty).
    pub(crate) fn bounding(&self) -> RectI {
        self.rects.iter().fold(RectI::EMPTY, |a, r| a.union(r))
    }
}

impl From<RectI> for DirtySet {
    fn from(r: RectI) -> Self {
        let mut s = DirtySet::default();
        s.add(r);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_rects_stay_apart_and_overlapping_merge() {
        let mut d = DirtySet::default();
        d.add(RectI::EMPTY);
        assert!(d.rects().is_empty());
        d.add(RectI::new(0, 0, 10, 10));
        d.add(RectI::new(50, 50, 60, 60));
        assert_eq!(d.rects().len(), 2);
        // Bridges both: everything becomes one rectangle.
        d.add(RectI::new(5, 5, 55, 55));
        assert_eq!(d.rects(), &[RectI::new(0, 0, 60, 60)]);
        assert_eq!(d.bounding(), RectI::new(0, 0, 60, 60));
    }

    #[test]
    fn collapses_beyond_the_cap_without_losing_coverage() {
        let mut d = DirtySet::default();
        for i in 0..(MAX_DIRTY_RECTS as i32 + 5) {
            d.add(RectI::new(i * 10, 0, i * 10 + 3, 3));
        }
        assert!(d.rects().len() <= MAX_DIRTY_RECTS);
        let b = d.bounding();
        assert_eq!((b.x0, b.x1), (0, (MAX_DIRTY_RECTS as i32 + 4) * 10 + 3));
        let mut e = DirtySet::from(RectI::new(-5, -5, 1, 1));
        e.extend(&d);
        assert_eq!(e.bounding().x0, -5);
    }
}
