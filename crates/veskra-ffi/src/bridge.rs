//! JVM-independent implementation of the JNI functions (unit-testable on the host).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use veskra_core::{ErrorCode, RectI, VeskraError};
use veskra_engine::{Command, EngineConfig, Session, error_json};

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

/// `nativeExecute`. Export commands snapshot the document under the lock and run unlocked.
pub fn execute_json(handle: i64, json: &str) -> String {
    let Some(s) = session(handle) else { return bad_handle() };
    if json.len() > 64 * 1024 * 1024 {
        return error_json(&VeskraError::alloc("command too large")).to_string();
    }
    match Command::from_json(json) {
        Ok(Command::ExportRun { format, path, options }) => {
            let snap = match lock(&s).snapshot() {
                Ok(d) => d,
                Err(e) => return error_json(&e).to_string(),
            };
            match veskra_engine::export_snapshot(
                &snap,
                format,
                &options,
                &PathBuf::from(path),
                &veskra_engine::CancelToken::new(),
            ) {
                Ok(rep) => serde_json::json!({"ok": rep}).to_string(),
                Err(e) => error_json(&e).to_string(),
            }
        }
        Ok(_) => lock(&s).execute_json(json),
        Err(e) => error_json(&e).to_string(),
    }
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

/// `nativeGetView` → `[a, b, c, d, e, f, zoom, rotation, mirror]`.
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
