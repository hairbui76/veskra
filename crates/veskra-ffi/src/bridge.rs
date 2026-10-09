//! JVM-independent implementation of the JNI functions (unit-testable on the host).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use veskra_core::{ErrorCode, RectI, VeskraError};
use veskra_engine::{Command, EngineConfig, Receipt, Session, error_json, response_json};

type Shared = Arc<Mutex<Session>>;

fn registry() -> &'static Mutex<HashMap<i64, Shared>> {
    static REG: OnceLock<Mutex<HashMap<i64, Shared>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock is caught at the JNI boundary; the session state is still
    // structurally valid (all mutations are copy-on-write and committed atomically), so recover.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn session(handle: i64) -> Option<Shared> {
    lock(registry()).get(&handle).cloned()
}

pub fn version_string() -> String {
    format!("veskra-ffi {} api={}", env!("CARGO_PKG_VERSION"), veskra_engine::API_VERSION)
}

pub fn create_session() -> i64 {
    let h = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let s = Session::new(EngineConfig::default());
    lock(registry()).insert(h, Arc::new(Mutex::new(s)));
    h
}

/// Idempotent: destroying an unknown or already-destroyed handle is a no-op.
pub fn destroy_session(handle: i64) {
    lock(registry()).remove(&handle);
}

pub fn session_count() -> usize {
    lock(registry()).len()
}

fn bad_handle() -> String {
    error_json(&VeskraError::new(ErrorCode::InvalidState, "invalid or destroyed session handle")).to_string()
}

/// Largest accepted command JSON (a maximal `stroke.samples` batch is far below this).
pub const MAX_COMMAND_BYTES: usize = 64 * 1024 * 1024;

/// `nativeExecute`. The JSON is parsed once, outside the session lock. Export commands snapshot
/// the committed document (never the live stroke) under the lock and encode/write unlocked; the
/// response uses the same envelope as every other command (`version` = snapshot version).
pub fn execute_json(handle: i64, json: &str) -> String {
    let Some(s) = session(handle) else { return bad_handle() };
    if json.len() > MAX_COMMAND_BYTES {
        return error_json(&VeskraError::alloc("command too large")).to_string();
    }
    let cmd = match Command::from_json(json) {
        Ok(c) => c,
        Err(e) => return error_json(&e).to_string(),
    };
    let out = match cmd {
        Command::ExportRun { format, path, options } => {
            let snap = {
                let g = lock(&s);
                g.snapshot().map(|d| (d, g.version()))
            };
            snap.and_then(|(snap, version)| {
                let rep = veskra_engine::export_snapshot(
                    &snap,
                    format,
                    &options,
                    &PathBuf::from(path),
                    &veskra_engine::CancelToken::new(),
                )?;
                let result = serde_json::to_value(rep)
                    .map_err(|e| VeskraError::new(ErrorCode::Internal, format!("serialization: {e}")))?;
                Ok(Receipt {
                    result,
                    document_version: version,
                    invalidated: vec![],
                    warnings: vec![],
                    history_changed: false,
                    view_changed: false,
                })
            })
        }
        cmd => lock(&s).execute(cmd),
    };
    response_json(out).to_string()
}

/// `nativeInputBatch` → `[flags, nRects, x0, y0, x1, y1, ...]`. Malformed batches return
/// `[-1, 0]` (no state change).
pub fn input_batch(handle: i64, ints: &[i32], floats: &[f32], longs: &[i64], count: i32) -> Vec<i32> {
    let Some(s) = session(handle) else { return vec![-1, 0] };
    let Ok(n) = veskra_input::batch_count_from_jni(count) else { return vec![-1, 0] };
    let events = match veskra_input::unpack_events(ints, floats, longs, n) {
        Ok(e) => e,
        Err(_) => return vec![-1, 0],
    };
    let outcome = lock(&s).input_batch(&events);
    let mut out = Vec::with_capacity(2 + outcome.dirty.len() * 4);
    out.push(outcome.flags);
    out.push(outcome.dirty.len() as i32);
    for r in outcome.dirty {
        out.extend_from_slice(&[r.x0, r.y0, r.x1, r.y1]);
    }
    out
}

/// How many elements of each JNI array `nativeInputBatch` must copy: only the `count`-event
/// prefix (Kotlin passes capacity-sized, reused arrays). Never more than the array holds; a
/// shorter array is still rejected by `unpack_events`.
pub fn batch_copy_lens(count: i32, ints_len: usize, floats_len: usize, longs_len: usize) -> (usize, usize, usize) {
    use veskra_input::event::{FLOATS_PER_EVENT, INTS_PER_EVENT};
    let n = usize::try_from(count).unwrap_or(0).min(veskra_input::MAX_EVENTS_PER_BATCH);
    (ints_len.min(n * INTS_PER_EVENT), floats_len.min(n * FLOATS_PER_EVENT), longs_len.min(n))
}

/// `nativeGetView` →`[a, b, c, d, e, f, zoom, rotation, mirror]`.
pub fn get_view(handle: i64) -> [f32; 9] {
    let Some(s) = session(handle) else { return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0] };
    let v = lock(&s).view();
    let m = v.doc_to_view();
    [
        m.a as f32,
        m.b as f32,
        m.c as f32,
        m.d as f32,
        m.e as f32,
        m.f as f32,
        v.zoom as f32,
        v.rotation as f32,
        if v.mirror { 1.0 } else { 0.0 },
    ]
}

pub fn set_viewport(handle: i64, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    if let Some(s) = session(handle) {
        let _ = lock(&s).execute(Command::ViewSize { width: w as f64, height: h as f64 });
    }
}

/// `nativeRenderRegion`: writes `w*h*4` bytes of RGBA8 premultiplied sRGB into `dst`.
pub fn render_region(handle: i64, x: i32, y: i32, w: i32, h: i32, dst: &mut [u8]) -> bool {
    let Some(s) = session(handle) else { return false };
    if w <= 0 || h <= 0 || w > 8192 || h > 8192 {
        return false;
    }
    let need = (w as usize) * (h as usize) * 4;
    if dst.len() < need {
        return false;
    }
    let (Some(x1), Some(y1)) = (x.checked_add(w), y.checked_add(h)) else { return false };
    let rect = RectI::new(x, y, x1, y1);
    lock(&s).render_display(rect, &mut dst[..need]).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(v: &str) -> serde_json::Value {
        let j: serde_json::Value = serde_json::from_str(v).unwrap();
        assert!(j.get("error").is_none(), "unexpected error: {v}");
        j
    }

    #[test]
    fn lifecycle_and_bad_handles() {
        let h = create_session();
        assert!(h > 0);
        assert!(bridge_contains(h));
        destroy_session(h);
        destroy_session(h); // double destroy is a no-op
        let e: serde_json::Value = serde_json::from_str(&execute_json(h, r#"{"cmd":"document.info"}"#)).unwrap();
        assert_eq!(e["error"]["code"], "InvalidState");
        assert_eq!(input_batch(h, &[], &[], &[], 0), vec![-1, 0]);
        let mut buf = vec![0u8; 16];
        assert!(!render_region(h, 0, 0, 2, 2, &mut buf));
        assert!(version_string().contains("api=1"));
    }

    fn bridge_contains(h: i64) -> bool {
        session(h).is_some()
    }

    #[test]
    fn malformed_json_and_batches_are_errors_not_panics() {
        let h = create_session();
        let e: serde_json::Value = serde_json::from_str(&execute_json(h, "{not json")).unwrap();
        assert_eq!(e["error"]["code"], "InvalidArgument");
        assert_eq!(input_batch(h, &[1, 2], &[0.0], &[0], 5), vec![-1, 0]);
        assert_eq!(input_batch(h, &[], &[], &[], -3), vec![-1, 0]);
        let mut small = vec![0u8; 3];
        ok(&execute_json(h, r#"{"cmd":"document.create","width":64,"height":64}"#));
        assert!(!render_region(h, 0, 0, 2, 2, &mut small));
        assert!(!render_region(h, i32::MAX, 0, 2, 2, &mut [0u8; 16]));
        destroy_session(h);
    }

    #[test]
    fn stylus_batch_paints_and_renders() {
        let h = create_session();
        ok(&execute_json(h, r##"{"cmd":"document.create","width":128,"height":128,"background":"#FFFFFF"}"##));
        set_viewport(h, 128, 128);
        ok(&execute_json(h, r#"{"cmd":"view.set","zoom":1.0,"rotation":0.0,"panX":0.0,"panY":0.0}"#));
        // Packed stylus stroke: DOWN, MOVEs, UP along y = 64.
        let mut ints = Vec::new();
        let mut floats = Vec::new();
        let mut longs = Vec::new();
        let n = 20;
        for i in 0..n {
            let action = if i == 0 {
                0
            } else if i == n - 1 {
                1
            } else {
                2
            };
            ints.extend_from_slice(&[action, 0, 2, 0, 0]);
            floats.extend_from_slice(&[10.0 + i as f32 * 5.0, 64.0, 0.8, f32::NAN, f32::NAN, f32::NAN, f32::NAN, 0.0]);
            longs.push(1_000_000 * i as i64);
        }
        let out = input_batch(h, &ints, &floats, &longs, n);
        assert_eq!(out[0] & veskra_engine::flags::DIRTY, veskra_engine::flags::DIRTY, "{out:?}");
        assert_eq!(out[0] & veskra_engine::flags::COMMITTED, veskra_engine::flags::COMMITTED, "{out:?}");
        assert!(out[1] >= 1);
        let mut buf = vec![0u8; 128 * 128 * 4];
        assert!(render_region(h, 0, 0, 128, 128, &mut buf));
        let px = |x: usize, y: usize| &buf[(y * 128 + x) * 4..(y * 128 + x) * 4 + 4];
        assert_eq!(px(5, 5), &[255, 255, 255, 255]);
        let center = px(50, 64);
        assert!(center[0] < 128 && center[3] == 255, "stroke pixel {center:?}");
        let info = ok(&execute_json(h, r#"{"cmd":"document.info"}"#));
        assert_eq!(info["ok"]["strokeCount"], 1);
        assert_eq!(info["ok"]["canUndo"], true);
        ok(&execute_json(h, r#"{"cmd":"history.undo"}"#));
        assert!(render_region(h, 0, 0, 128, 128, &mut buf));
        assert_eq!(&buf[(64 * 128 + 50) * 4..(64 * 128 + 50) * 4 + 4], &[255, 255, 255, 255]);
        destroy_session(h);
    }

    /// Defect: `export.run` through the bridge returned a bare `{"ok": report}` — no `version`,
    /// `invalidated`, `warnings`, `historyChanged`, `viewChanged` — unlike every other command
    /// and unlike the same command through `Session::execute_json` (COMMANDS.md envelope).
    #[test]
    fn export_run_uses_the_standard_envelope_and_excludes_live_stroke() {
        let h = create_session();
        ok(&execute_json(h, r##"{"cmd":"document.create","width":40,"height":30,"background":"#FFFFFF"}"##));
        ok(&execute_json(h, r#"{"cmd":"stroke.begin"}"#));
        ok(&execute_json(h, r#"{"cmd":"stroke.samples","samples":[{"x":2,"y":2,"p":0.9},{"x":38,"y":28,"p":0.9}]}"#));
        let dir = std::env::temp_dir().join(format!("veskra-ffi-review-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("e.png");
        let cmd = serde_json::json!({"cmd": "export.run", "format": "png", "path": path.to_string_lossy()});
        let out = ok(&execute_json(h, &cmd.to_string()));
        for k in ["ok", "version", "invalidated", "warnings", "historyChanged", "viewChanged"] {
            assert!(out.get(k).is_some(), "missing {k}: {out}");
        }
        assert_eq!(out["version"], 1);
        assert_eq!(out["historyChanged"], false);
        assert_eq!(out["ok"]["success"], true);
        // Same command through the session path: identical envelope keys.
        let s = session(h).unwrap();
        let direct: serde_json::Value = serde_json::from_str(&lock(&s).execute_json(&cmd.to_string())).unwrap();
        let keys = |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&out), keys(&direct));
        drop(s);
        destroy_session(h);
    }

    /// Fuzz-style: random handles, arrays, counts, rects and mutated JSON never panic, and
    /// rendering never writes outside `w*h*4` bytes.
    #[test]
    fn bridge_functions_never_panic_on_random_input() {
        let mut st = 0x1234_5678_9abc_def0u64;
        let mut rnd = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            st
        };
        let h = create_session();
        ok(&execute_json(h, r##"{"cmd":"document.create","width":70,"height":50,"background":"#336699"}"##));
        set_viewport(h, 70, 50);
        let seeds = [
            r#"{"cmd":"layer.add","kind":"vector","name":"x"}"#,
            r#"{"cmd":"selection.rect","x0":1,"y0":2,"x1":30,"y1":40,"op":"add"}"#,
            r#"{"cmd":"stroke.samples","samples":[{"x":1,"y":2,"t":3,"p":0.5}]}"#,
            r#"{"cmd":"edit.transform","matrix":[1,0,0,1,5,5]}"#,
            r#"{"cmd":"view.set","zoom":2.5,"rotation":1.0}"#,
            r#"{"cmd":"stroke.begin"}"#,
            r#"{"cmd":"history.undo"}"#,
        ];
        let junk = ["", "{", "}", "\"", "1e999", "-1", "null", "[]", "\u{0}", "NaN", "9999999999999999999999"];
        for i in 0..3000 {
            let base = seeds[(rnd() % seeds.len() as u64) as usize];
            let mut cmd = base.to_string();
            let pos = (rnd() as usize) % (cmd.len() + 1);
            if cmd.is_char_boundary(pos) && rnd() % 2 == 0 {
                cmd.insert_str(pos, junk[(rnd() % junk.len() as u64) as usize]);
            }
            let handle = if i % 50 == 0 { rnd() as i64 } else { h };
            let _ = execute_json(handle, &cmd);
            let n = (rnd() % 12) as usize;
            let ints: Vec<i32> = (0..n * 5 + (rnd() % 3) as usize).map(|_| (rnd() % 12) as i32 - 2).collect();
            let floats: Vec<f32> = (0..n * 8)
                .map(|_| match rnd() % 6 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    2 => -1e30,
                    _ => (rnd() % 100) as f32,
                })
                .collect();
            let longs: Vec<i64> =
                (0..n).map(|k| if rnd() % 20 == 0 { -1 } else { (i * 10 + k) as i64 * 1_000_000 }).collect();
            let _ = input_batch(handle, &ints, &floats, &longs, (rnd() % 16) as i32 - 2);
            let (x, y) = ((rnd() % 200) as i32 - 100, (rnd() % 200) as i32 - 100);
            let (w, hh) = ((rnd() % 90) as i32 - 5, (rnd() % 90) as i32 - 5);
            let cap = (rnd() % 40_000) as usize;
            let mut buf = vec![0xAAu8; cap + 64];
            let _ = render_region(handle, x, y, w, hh, &mut buf[..cap]);
            assert!(buf[cap..].iter().all(|b| *b == 0xAA), "wrote past the slice");
            let _ = get_view(handle);
            set_viewport(handle, (rnd() % 3000) as i32 - 100, (rnd() % 3000) as i32 - 100);
        }
        for (x, y, w, hh) in [(i32::MIN, 0, 4, 4), (i32::MAX - 1, 0, 4, 4), (0, 0, 8192, 8192), (-3, -3, 4, 4)] {
            let mut b = vec![0u8; 64];
            let _ = render_region(h, x, y, w, hh, &mut b);
        }
        // Partially outside the canvas: allowed, outside pixels are transparent.
        let mut b = vec![0xFFu8; 4 * 4 * 4];
        assert!(render_region(h, -2, -2, 4, 4, &mut b));
        assert_eq!(&b[0..4], &[0, 0, 0, 0]);
        destroy_session(h);
    }

    /// Perf defect: `nativeInputBatch` copied the packer's whole capacity (up to 65536 events,
    /// ~3.4 MB) across JNI on every pen batch. Only the `count` prefix is needed.
    #[test]
    fn input_batch_copies_only_the_count_prefix() {
        assert_eq!(batch_copy_lens(3, 5 * 4096, 8 * 4096, 4096), (15, 24, 3));
        assert_eq!(batch_copy_lens(3, 10, 24, 3), (10, 24, 3)); // short array: still rejected later
        assert_eq!(batch_copy_lens(-1, 100, 100, 100), (0, 0, 0));
        assert_eq!(batch_copy_lens(i32::MAX, 7, 9, 11), (7, 9, 11));
        // The prefix decodes exactly like the full arrays.
        let evs = veskra_input::trace::parse_jsonl(
            "{\"action\":\"Down\",\"pointer_id\":0,\"tool\":\"Stylus\",\"x\":1.0,\"y\":2.0,\"t_ns\":5,\"pressure\":0.5}",
        )
        .unwrap();
        let (mut i, mut f, mut l) = veskra_input::pack_events(&evs);
        i.resize(5 * 64, 0);
        f.resize(8 * 64, 0.0);
        l.resize(64, 0);
        let (ni, nf, nl) = batch_copy_lens(1, i.len(), f.len(), l.len());
        assert_eq!(
            veskra_input::unpack_events(&i[..ni], &f[..nf], &l[..nl], 1).unwrap(),
            veskra_input::unpack_events(&i, &f, &l, 1).unwrap()
        );
    }

    #[test]
    fn concurrent_sessions_are_independent() {
        let hs: Vec<i64> = (0..8).map(|_| create_session()).collect();
        let threads: Vec<_> = hs
            .iter()
            .map(|&h| {
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let v: serde_json::Value = serde_json::from_str(&execute_json(
                            h,
                            r#"{"cmd":"document.create","width":32,"height":32}"#,
                        ))
                        .unwrap();
                        assert!(v.get("ok").is_some());
                        let _ = get_view(h);
                    }
                    destroy_session(h);
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        for h in hs {
            assert!(session(h).is_none());
        }
    }
}
