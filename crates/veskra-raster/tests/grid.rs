//! CORE-04 tile store / mask tests, PNT-005, and an ignored timing benchmark.

use std::sync::Arc;

use veskra_core::{ColorRgba, ErrorCode, RectI, VeskraError};
use veskra_raster::ops::{CompositeMode, composite_mask_color};
use veskra_raster::{
    MaskGrid, MaskOp, Px16, RasterError, TILE, TILE_BYTES, TILE_LEN, Tile, TileCoord, TileGrid, UNBOUNDED,
};

fn assert_traits<T: Clone + std::fmt::Debug + Default + Send + Sync + PartialEq>() {}

#[test]
fn grids_are_clone_debug_default_send_sync_eq() {
    assert_traits::<TileGrid>();
    assert_traits::<MaskGrid>();
    assert_eq!(TILE, 64);
    assert!(MaskGrid::default().is_empty());
    assert_eq!(
        format!("{:?}", TileGrid::default()),
        "TileGrid { tiles: 0, bounds: RectI { x0: 0, y0: 0, x1: 0, y1: 0 } }"
    );
}

/// Deterministic premultiplied test pixel for (x, y).
fn px(x: i32, y: i32) -> Px16 {
    let h = (x as u32).wrapping_mul(0x9E37_79B9) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
    let a = (h >> 16) as u16 | 1;
    [(h as u16) % a, (h.rotate_left(7) as u16) % a, (h.rotate_left(13) as u16) % a, a]
}

fn data_for(r: RectI) -> Vec<u16> {
    let mut v = Vec::new();
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            v.extend_from_slice(&px(x, y));
        }
    }
    v
}

#[test]
fn tile_coordinate_math_handles_negatives() {
    assert_eq!(TileCoord::of_pixel(0, 0), TileCoord::new(0, 0));
    assert_eq!(TileCoord::of_pixel(63, 63), TileCoord::new(0, 0));
    assert_eq!(TileCoord::of_pixel(64, 0), TileCoord::new(1, 0));
    assert_eq!(TileCoord::of_pixel(-1, -64), TileCoord::new(-1, -1));
    assert_eq!(TileCoord::of_pixel(-65, -65), TileCoord::new(-2, -2));
    assert_eq!(TileCoord::split(-1, -1), (TileCoord::new(-1, -1), 63, 63));
    assert_eq!(TileCoord::split(-64, 130), (TileCoord::new(-1, 2), 0, 2));
    assert_eq!(TileCoord::split(i32::MIN, i32::MAX), (TileCoord::new(-(1 << 25), (1 << 25) - 1), 0, 63));
    assert_eq!(TileCoord::new(-1, 2).rect(), RectI::new(-64, 128, 0, 192));
    // Extreme tile rect clamps instead of overflowing.
    assert_eq!(TileCoord::of_pixel(i32::MAX, 0).rect().x1, i32::MAX);
    let tiles: Vec<_> = TileGrid::tiles_in_rect(RectI::new(-65, -1, 1, 64)).collect();
    assert_eq!(
        tiles,
        vec![
            TileCoord::new(-2, -1),
            TileCoord::new(-1, -1),
            TileCoord::new(0, -1),
            TileCoord::new(-2, 0),
            TileCoord::new(-1, 0),
            TileCoord::new(0, 0)
        ]
    );
    assert_eq!(TileGrid::tiles_in_rect(RectI::EMPTY).count(), 0);
}

#[test]
fn iteration_is_sorted_row_major() {
    let mut g = TileGrid::new();
    for &(x, y) in &[(200, 5), (-100, 300), (5, 5), (-5, -5), (70, -5)] {
        g.put_pixel(x, y, [1, 1, 1, 1]);
    }
    let order: Vec<_> = g.iter_tiles().map(|(c, _)| (c.ty, c.tx)).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
    assert_eq!(order.first(), Some(&(-1, -1)));
}

#[test]
fn put_get_and_transparent_writes_do_not_allocate() {
    let mut g = TileGrid::new();
    g.put_pixel(-3, 7, [0; 4]);
    assert_eq!(g.tile_count(), 0);
    g.put_pixel(-3, 7, [10, 20, 30, 40]);
    assert_eq!(g.get_pixel(-3, 7), [10, 20, 30, 40]);
    assert_eq!(g.get_pixel(-2, 7), [0; 4]);
    // Invariant enforced on store.
    g.put_pixel(0, 0, [500, 1, 2, 100]);
    assert_eq!(g.get_pixel(0, 0), [100, 1, 2, 100]);
    assert_eq!(g.tile_count(), 2);
    assert_eq!(g.byte_size(), 2 * TILE_BYTES);
}

#[test]
fn read_write_rect_round_trip_across_tile_boundaries() {
    let mut g = TileGrid::new();
    for r in [
        RectI::new(-70, -70, 70, 70),
        RectI::new(60, 3, 200, 4),
        RectI::new(-1, -1, 0, 0),
        RectI::new(127, 0, 129, 300),
    ] {
        let d = data_for(r);
        g.write_rect(r, &d).unwrap();
        assert_eq!(g.read_rect(r), d, "{r:?}");
    }
    // Reading partially outside content returns transparent there.
    let r = RectI::new(-80, -70, -60, -69);
    let v = g.read_rect(r);
    assert_eq!(v.len(), 20 * 4);
    assert_eq!(&v[..4], &[0; 4]);
    assert_eq!(&v[10 * 4..10 * 4 + 4], &px(-70, -70));
    assert!(g.read_rect(RectI::EMPTY).is_empty());
    assert!(matches!(g.try_read_rect(RectI::new(0, 0, 10_000, 10_000)), Err(RasterError::TooLarge { .. })));
    assert!(g.read_rect(RectI::new(0, 0, 10_000, 10_000)).is_empty());
    // Size mismatch is a typed error and leaves the grid untouched.
    let before = g.clone();
    let e = g.write_rect(RectI::new(0, 0, 2, 2), &[0; 15]).unwrap_err();
    assert_eq!(e, RasterError::SizeMismatch { expected: 16, got: 15 });
    assert_eq!(g, before);
    let ve: VeskraError = e.into();
    assert_eq!(ve.code, ErrorCode::InvalidArgument);
}

#[test]
fn pnt005_partial_write_does_not_touch_unrelated_tiles() {
    let mut g = TileGrid::new();
    let all = RectI::new(-128, -128, 256, 256);
    g.write_rect(all, &data_for(all)).unwrap();
    let snapshot = g.clone();
    // Write a small rect straddling 4 tiles around (64, 64).
    let r = RectI::new(60, 60, 70, 70);
    g.write_rect(r, &vec![7u16; r.area() as usize * 4]).unwrap();
    let touched: Vec<_> = TileGrid::tiles_in_rect(r).collect();
    assert_eq!(touched.len(), 4);
    for (c, _) in snapshot.iter_tiles() {
        if touched.contains(&c) {
            assert!(!g.shares_tile_with(&snapshot, c), "{c:?} must be copied");
        } else {
            assert!(g.shares_tile_with(&snapshot, c), "{c:?} must stay shared (Arc::ptr_eq)");
        }
    }
    assert_eq!(g.unshared_tile_count(&snapshot), 4);
    assert_eq!(g.unshared_byte_size(&snapshot), 4 * TILE_BYTES);
    // Pixels outside r are identical, the snapshot is unchanged.
    for y in all.y0..all.y1 {
        for x in all.x0..all.x1 {
            assert_eq!(snapshot.get_pixel(x, y), px(x, y));
            if !r.contains(x, y) {
                assert_eq!(g.get_pixel(x, y), px(x, y));
            } else {
                assert_eq!(g.get_pixel(x, y), [7; 4]);
            }
        }
    }
    // Rewriting identical data keeps sharing (no copy for no-op writes).
    let mut h = snapshot.clone();
    let r2 = RectI::new(-10, -10, 100, 100);
    h.write_rect(r2, &snapshot.read_rect(r2)).unwrap();
    assert_eq!(h.unshared_tile_count(&snapshot), 0);
    // All-transparent write into empty area does not allocate.
    let mut e = TileGrid::new();
    e.write_rect(RectI::new(0, 0, 300, 300), &vec![0; 300 * 300 * 4]).unwrap();
    assert_eq!(e.tile_count(), 0);
}

#[test]
fn copy_on_write_clone_then_mutate() {
    let mut a = TileGrid::new();
    a.put_pixel(5, 5, [1, 2, 3, 4]);
    let b = a.clone();
    assert!(a.shares_tile_with(&b, TileCoord::new(0, 0)));
    a.tile_mut(TileCoord::new(0, 0)).set(5, 5, [9, 9, 9, 9]);
    assert_eq!(b.get_pixel(5, 5), [1, 2, 3, 4]);
    assert_eq!(a.get_pixel(5, 5), [9, 9, 9, 9]);
    assert!(!a.shares_tile_with(&b, TileCoord::new(0, 0)));
    assert!(!a.shares_tile_with(&b, TileCoord::new(3, 3)), "missing tiles are not shared");
    let mut c = b.clone();
    c.clear();
    assert_eq!(b.tile_count(), 1);
    c.set_tile(TileCoord::new(2, 2), b.tile(TileCoord::new(0, 0)).unwrap().clone());
    assert_eq!(c.get_pixel(133, 133), [1, 2, 3, 4]);
    c.remove_tile(TileCoord::new(2, 2));
    assert_eq!(c.tile_count(), 0);
}

#[test]
fn prune_content_bounds_and_equality() {
    let mut g = TileGrid::new();
    assert_eq!(g.content_bounds(), RectI::EMPTY);
    g.tile_mut(TileCoord::new(4, 4));
    g.put_pixel(-100, 3, [0, 0, 0, 1]);
    g.put_pixel(10, 70, [0, 0, 0, 1]);
    g.put_pixel(-99, 2, [0, 0, 0, 1]);
    assert_eq!(g.content_bounds(), RectI::new(-100, 2, 11, 71));
    // Materialized transparent tile compares equal to a missing one.
    let mut h = g.clone();
    h.prune_transparent();
    assert_eq!(h.tile_count(), g.tile_count() - 1);
    assert_eq!(g, h);
    assert_eq!(h, g);
    h.put_pixel(0, 0, [0, 0, 0, 2]);
    assert_ne!(g, h);
    // Erasing all content then pruning removes tiles.
    let mut k = TileGrid::new();
    k.put_pixel(1, 1, [3, 3, 3, 3]);
    k.put_pixel(1, 1, [0; 4]);
    assert_eq!(k.tile_count(), 1);
    k.prune_transparent();
    assert_eq!(k.tile_count(), 0);
    assert_eq!(k, TileGrid::new());
}

#[test]
fn tile_construction_validates_input() {
    assert!(Tile::new_transparent().is_transparent());
    assert_eq!(Tile::new_transparent().pixels().len(), TILE_LEN);
    assert!(matches!(Tile::from_pixels(&[0; 10]), Err(RasterError::SizeMismatch { .. })));
    let mut bad = vec![0u16; TILE_LEN];
    bad[0] = 5; // r > a
    let e = Tile::from_pixels(&bad).unwrap_err();
    assert!(matches!(e, RasterError::InvalidTileData(_)));
    assert_eq!(VeskraError::from(e).code, ErrorCode::CorruptTile);
    let t = Tile::from_pixels_clamped(&bad).unwrap();
    assert!(t.is_transparent());
    let mut good = vec![0u16; TILE_LEN];
    good[4..8].copy_from_slice(&[1, 2, 3, 4]);
    let t = Tile::from_pixels(&good).unwrap();
    assert_eq!(t.get(1, 0), [1, 2, 3, 4]);
    assert_eq!(t.local_content_bounds(), Some((1, 0, 2, 1)));
}

#[test]
fn tiles_overlapping_only_visits_materialized() {
    let mut g = TileGrid::new();
    g.put_pixel(-1_000_000, -1_000_000, [1; 4]);
    g.put_pixel(1_000_000, 1_000_000, [1; 4]);
    g.put_pixel(10, 10, [1; 4]);
    let v: Vec<_> = g.tiles_overlapping(RectI::new(-5, -5, 2_000_000, 2_000_000)).map(|(c, _)| c).collect();
    assert_eq!(v, vec![TileCoord::new(0, 0), TileCoord::of_pixel(1_000_000, 1_000_000)]);
    assert_eq!(g.tiles_overlapping(RectI::EMPTY).count(), 0);
}

#[test]
fn mask_defaults_get_set_and_rects() {
    let mut m = MaskGrid::new_empty();
    assert!(m.is_empty());
    assert_eq!(m.bounds(), RectI::EMPTY);
    m.set(-5, -5, 0);
    assert_eq!(m.tile_count(), 0);
    m.set(-5, -5, 200);
    assert_eq!(m.get(-5, -5), 200);
    assert_eq!(m.bounds(), RectI::new(-5, -5, -4, -4));
    let f = MaskGrid::new_full();
    assert_eq!(f.get(i32::MIN, i32::MAX), 255);
    assert_eq!(f.bounds(), UNBOUNDED);
    assert!(!f.is_empty());
    let r = RectI::new(-70, -2, 70, 3);
    let data: Vec<u8> = (0..r.area()).map(|i| (i % 251) as u8).collect();
    let mut w = MaskGrid::new_empty();
    w.write_rect(r, &data).unwrap();
    assert_eq!(w.read_rect(r), data);
    assert!(w.write_rect(r, &data[1..]).is_err());
    let big = RectI::new(-10, -10, 10, 10);
    assert_eq!(f.read_rect(big), vec![255; 400]);
}

#[test]
fn mask_invert_and_combine() {
    let doc = RectI::new(0, 0, 200, 100);
    let mut m = MaskGrid::new_rect(RectI::new(10, 10, 20, 20));
    m.invert(doc);
    assert_eq!(m.get(15, 15), 0);
    assert_eq!(m.get(0, 0), 255);
    assert_eq!(m.get(199, 99), 255);
    assert_eq!(m.get(200, 50), 0, "outside `within` unchanged");
    assert_eq!(m.bounds(), doc);
    // 8 tiles; the two fully covered uniform ones share a single allocation.
    assert_eq!(m.tile_count(), 8);
    assert_eq!(m.byte_size(), 7 * 64 * 64);
    m.invert(doc);
    assert_eq!(m, MaskGrid::new_rect(RectI::new(10, 10, 20, 20)));
    // Global invert flips the default.
    let mut g = MaskGrid::new_rect(RectI::new(0, 0, 5, 5));
    g.invert(UNBOUNDED);
    assert_eq!(g.default_value(), 255);
    assert_eq!(g.get(2, 2), 0);
    assert_eq!(g.get(-1_000, 7_000), 255);

    let a = MaskGrid::new_rect(RectI::new(0, 0, 10, 10));
    let b = MaskGrid::new_rect(RectI::new(5, 0, 15, 10));
    let mut u = a.clone();
    u.combine(&b, MaskOp::Add);
    assert_eq!(u, MaskGrid::new_rect(RectI::new(0, 0, 15, 10)));
    let mut s = a.clone();
    s.combine(&b, MaskOp::Subtract);
    assert_eq!(s, MaskGrid::new_rect(RectI::new(0, 0, 5, 10)));
    let mut i = a.clone();
    i.combine(&b, MaskOp::Intersect);
    assert_eq!(i, MaskGrid::new_rect(RectI::new(5, 0, 10, 10)));
    let mut r = a.clone();
    r.combine(&b, MaskOp::Replace);
    assert_eq!(r, b);
    // Idempotent operators.
    let mut twice = u.clone();
    twice.combine(&b, MaskOp::Add);
    assert_eq!(twice, u);
    // Combining with full/empty defaults.
    let mut x = a.clone();
    x.combine(&MaskGrid::new_full(), MaskOp::Intersect);
    assert_eq!(x, a);
    let mut y = MaskGrid::new_full();
    y.combine(&a, MaskOp::Subtract);
    assert_eq!(y.get(5, 5), 0);
    assert_eq!(y.get(50, 5), 255);
    // Partial values.
    let mut p = MaskGrid::new_empty();
    p.set(0, 0, 100);
    let mut q = MaskGrid::new_empty();
    q.set(0, 0, 200);
    let mut pq = p.clone();
    pq.combine(&q, MaskOp::Subtract);
    assert_eq!(pq.get(0, 0), 55);
    pq.combine(&q, MaskOp::Add);
    assert_eq!(pq.get(0, 0), 200);
}

#[test]
fn mask_cow_and_accounting() {
    let a = MaskGrid::new_rect(RectI::new(0, 0, 100, 100));
    let mut b = a.clone();
    b.set(70, 70, 3);
    assert_eq!(a.get(70, 70), 255);
    assert!(b.shares_tile_with(&a, TileCoord::new(0, 0)));
    assert!(!b.shares_tile_with(&a, TileCoord::new(1, 1)));
    assert_eq!(b.unshared_tile_count(&a), 1);
    let mut c = b.clone();
    c.scale(128);
    assert_eq!(c.get(0, 0), 128);
    assert_eq!(b.get(0, 0), 255);
}

/// Benchmark: paint N random 32-px rects (src-over through a rect mask). Run with
/// `cargo test -p veskra-raster --release -- --ignored --nocapture bench_paint`.
#[test]
#[ignore]
fn bench_paint_random_rects() {
    let n = 20_000;
    let mut g = TileGrid::new();
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let (x, y) = ((next() % 4064) as i32, (next() % 4064) as i32);
        let m = MaskGrid::new_rect(RectI::new(x, y, x + 32, y + 32));
        let c = ColorRgba::new((next() % 256) as f32 / 255.0, 0.5, 0.25, 0.75);
        composite_mask_color(&mut g, &m, c, 1.0, CompositeMode::Over, None);
    }
    let dt = t0.elapsed();
    let snap = g.clone();
    let t1 = std::time::Instant::now();
    let _cb = snap.content_bounds();
    let dt2 = t1.elapsed();
    println!(
        "painted {n} 32x32 rects in {:?} ({:.2} us/rect, {:.1} Mpx/s); tiles={}, content_bounds {:?}",
        dt,
        dt.as_secs_f64() * 1e6 / n as f64,
        (n as f64 * 1024.0) / dt.as_secs_f64() / 1e6,
        g.tile_count(),
        dt2
    );
    assert!(Arc::strong_count(snap.tile(snap.iter_tiles().next().unwrap().0).unwrap()) >= 2);
}

/// Benchmark: selection / fill / transform at document scale. Run with
/// `cargo test -p veskra-raster --release -- --ignored --nocapture bench_edit`.
#[test]
#[ignore]
fn bench_edit_ops() {
    use veskra_core::{Affine, PointF, RectF};
    use veskra_raster::{fill, select, transform};
    let doc = RectI::new(0, 0, 4096, 4096);
    let t = std::time::Instant::now();
    let r = select::rect(RectF::new(10.5, 10.5, 4000.25, 4000.75), doc);
    println!("select::rect 4k: {:?} ({} tiles, {} B)", t.elapsed(), r.tile_count(), r.byte_size());
    let pts: Vec<PointF> = (0..2000)
        .map(|i| {
            let a = i as f64 * 0.0314;
            let rad = 1500.0 + 300.0 * (a * 7.0).sin();
            PointF::new(2048.0 + rad * a.cos(), 2048.0 + rad * a.sin())
        })
        .collect();
    let t = std::time::Instant::now();
    let l = select::polygon(&pts, doc);
    println!("select::polygon 2000 pts ~3000px: {:?} ({} tiles)", t.elapsed(), l.tile_count());
    let t = std::time::Instant::now();
    let (m, st) = fill::region_with_limits(
        &TileGrid::new(),
        (5, 5),
        &fill::FillOptions { tolerance: 0.1, contiguous: true, antialias: true },
        doc,
        None,
        fill::FillLimits::default(),
    )
    .unwrap();
    println!(
        "fill::region empty 4k: {:?} ({} px, work {} B, queue {})",
        t.elapsed(),
        st.filled_pixels,
        st.work_bytes,
        st.max_queue_len
    );
    let mut g = TileGrid::new();
    let t = std::time::Instant::now();
    fill::apply(&mut g, &m, ColorRgba::new(0.2, 0.4, 0.6, 1.0), false);
    println!("fill::apply 4k opaque: {:?} ({} tiles)", t.elapsed(), g.tile_count());
    let mut src = TileGrid::new();
    let area = RectI::new(0, 0, 1024, 1024);
    composite_mask_color(
        &mut src,
        &MaskGrid::new_rect(area),
        ColorRgba::new(1.0, 0.0, 0.0, 0.5),
        1.0,
        CompositeMode::Over,
        None,
    );
    src.put_pixel(3, 3, [1, 1, 1, 65535]); // break uniform sharing in one tile
    let t = std::time::Instant::now();
    let out = transform::transform_pixels(&src, None, Affine::rotate(0.3).then(&Affine::translate(500.0, 0.0)), doc);
    println!("transform 1024² rotate: {:?} ({} tiles)", t.elapsed(), out.tile_count());
}
