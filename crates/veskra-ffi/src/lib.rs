//! # veskra-ffi — JNI boundary for the Android shell (AND-02, CONTRACTS §8)
//!
//! Design:
//! * Sessions live in a process-global registry keyed by opaque non-zero `i64` handles. Kotlin
//!   never holds Rust pointers, so a stale/double-freed handle is a no-op or a typed error, never
//!   a use-after-free.
//! * Every session is behind its own `Mutex`; calls from different threads serialize. Exports run
//!   on an immutable `Arc<Document>` snapshot **after** the session lock is released, so a long
//!   export never blocks the pen pipeline (PERF-004).
//! * All logic is in plain Rust functions ([`bridge`]) that are unit-tested without a JVM; the
//!   `Java_*` exports are thin wrappers using `jni` 0.22 `with_env`, which catches panics so
//!   nothing unwinds into the JVM. Errors become JSON error objects or Java `RuntimeException`s.
//!
//! The only `unsafe` code is the direct-`ByteBuffer` slice in `nativeRenderRegion` and the
//! `#[unsafe(no_mangle)]` export attributes; both are documented at their use sites.

#![allow(unsafe_code)]

pub mod bridge;

use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JByteBuffer, JClass, JFloatArray, JIntArray, JLongArray, JString};
use jni::sys::{jboolean, jint, jlong};

type JResult<T> = Result<T, jni::errors::Error>;

/// `nativeVersion(): String`
#[unsafe(no_mangle)] // SAFETY: unique exported symbol name required by JNI name resolution.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeVersion<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
) -> JString<'caller> {
    env.with_env(|env| -> JResult<_> { JString::from_str(env, bridge::version_string()) })
        .resolve::<ThrowRuntimeExAndDefault>()
}

/// `nativeCreateSession(): Long` — 0 on failure.
#[unsafe(no_mangle)] // SAFETY: see above.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeCreateSession<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
) -> jlong {
    env.with_env(|_env| -> JResult<_> { Ok(bridge::create_session()) }).resolve::<ThrowRuntimeExAndDefault>()
}

/// `nativeDestroySession(h: Long)` — idempotent.
#[unsafe(no_mangle)] // SAFETY: see above.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeDestroySession<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
) {
    env.with_env(|_env| -> JResult<_> {
        bridge::destroy_session(handle);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `nativeExecute(h: Long, json: String): String`
#[unsafe(no_mangle)] // SAFETY: see above.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeExecute<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    json: JString<'caller>,
) -> JString<'caller> {
    env.with_env(|env| -> JResult<_> {
        let cmd = json.try_to_string(env)?;
        let out = bridge::execute_json(handle, &cmd);
        JString::from_str(env, out)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `nativeInputBatch(h, ints, floats, longs, count): IntArray` → `[flags, nRects, x0,y0,x1,y1, …]`
#[unsafe(no_mangle)] // SAFETY: see above.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeInputBatch<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    ints: JIntArray<'caller>,
    floats: JFloatArray<'caller>,
    longs: JLongArray<'caller>,
    count: jint,
) -> JIntArray<'caller> {
    env.with_env(|env| -> JResult<_> {
        // Copy only the `count`-event prefix of the (capacity-sized, reused) Kotlin arrays.
        let (ni, nf, nl) = bridge::batch_copy_lens(count, ints.len(env)?, floats.len(env)?, longs.len(env)?);
        let mut bi = vec![0i32; ni];
        ints.get_region(env, 0, &mut bi)?;
        let mut bf = vec![0f32; nf];
        floats.get_region(env, 0, &mut bf)?;
        let mut bl = vec![0i64; nl];
        longs.get_region(env, 0, &mut bl)?;
        let out = bridge::input_batch(handle, &bi, &bf, &bl, count);
        let arr = env.new_int_array(out.len())?;
        arr.set_region(env, 0, &out)?;
        Ok(arr)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `nativeGetView(h, out: FloatArray)` — 9 floats (doc→view affine a..f, zoom, rotation, mirror).
#[unsafe(no_mangle)] // SAFETY: see above.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeGetView<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    out: JFloatArray<'caller>,
) {
    env.with_env(|env| -> JResult<_> {
        let v = bridge::get_view(handle);
        let n = out.len(env)?.min(v.len());
        out.set_region(env, 0, &v[..n])?;
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `nativeSetViewport(h, w, h)`
#[unsafe(no_mangle)] // SAFETY: see above.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeSetViewport<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    width: jint,
    height: jint,
) {
    env.with_env(|_env| -> JResult<_> {
        bridge::set_viewport(handle, width, height);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `nativeRenderRegion(h, x, y, w, hgt, dst: ByteBuffer): Boolean`
#[unsafe(no_mangle)] // SAFETY: see above.
pub extern "system" fn Java_org_veskra_kit_NativeBridge_nativeRenderRegion<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    x: jint,
    y: jint,
    w: jint,
    h: jint,
    dst: JByteBuffer<'caller>,
) -> jboolean {
    env.with_env(|env| -> JResult<_> {
        let ptr = env.get_direct_buffer_address(&dst)?;
        let cap = env.get_direct_buffer_capacity(&dst)?;
        if ptr.is_null() {
            return Ok(false);
        }
        // SAFETY: `ptr`/`cap` describe the memory of a *direct* java.nio.ByteBuffer that stays
        // reachable (local reference `dst`) for the whole native call, so the memory is valid and
        // not moved by the GC. The CONTRACTS §8 calling convention requires Kotlin to not touch
        // this buffer concurrently (it is owned by the single engine thread), so we hold the only
        // live mutable view for the duration of this call. The slice does not escape the call.
        let buf: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr, cap) };
        Ok(bridge::render_region(handle, x, y, w, h, buf))
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}
