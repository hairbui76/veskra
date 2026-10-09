package org.veskra.kit

import java.nio.ByteBuffer

/**
 * JNI boundary to `crates/veskra-ffi` — implements the Kotlin side of docs/CONTRACTS.md §8
 * EXACTLY. Do not change a signature here without an ORCH-approved contract amendment.
 *
 * All functions are `external` JVM statics (`@JvmStatic` on an object), so the native symbols are
 * `Java_org_veskra_kit_NativeBridge_<name>` with a `jclass` second argument.
 * Package namespace `org.veskra.kit` is provisional (README "About the name", ADR-002).
 *
 * Threading: a session handle must only be used from ONE thread at a time (the app's engine
 * thread). Handles are opaque and never 0; 0 from [nativeCreateSession] means failure.
 */
object NativeBridge {
    /** Library name for [System.loadLibrary] → `libveskra_ffi.so`. */
    const val LIBRARY_NAME: String = "veskra_ffi"

    @Volatile
    private var loaded: Boolean = false

    /** True once [load] succeeded in this process. */
    val isLoaded: Boolean get() = loaded

    /**
     * Loads the native library once per process. With [libPath] uses [System.load] (absolute path,
     * JVM integration tests); otherwise [System.loadLibrary] (Android: from the APK's jniLibs).
     * Throws [UnsatisfiedLinkError] if loading fails; [isLoaded] stays false.
     */
    @Synchronized
    fun load(libPath: String? = null) {
        if (loaded) return
        if (libPath != null) System.load(libPath) else System.loadLibrary(LIBRARY_NAME)
        loaded = true
    }

    /** `"veskra-ffi <semver> api=<N>"`. */
    @JvmStatic external fun nativeVersion(): String

    /** New engine session; 0 on failure. */
    @JvmStatic external fun nativeCreateSession(): Long

    /** Idempotent; destroying an already-destroyed handle is a no-op. */
    @JvmStatic external fun nativeDestroySession(h: Long)

    /** Command JSON → `{"ok":…}` or `{"error":{"code","message","recoverable"}}`. */
    @JvmStatic external fun nativeExecute(h: Long, json: String): String

    /**
     * Packed pointer events (layout: [PointerPacker]). Returns `[flags, nRects, x0,y0,x1,y1, …]`;
     * flags: bit0 canvas dirty, bit1 view changed, bit2 stroke committed, bit3 history changed.
     */
    @JvmStatic external fun nativeInputBatch(h: Long, ints: IntArray, floats: FloatArray, longs: LongArray, count: Int): IntArray

    /** Writes 9 floats: doc→view affine `a,b,c,d,e,f`, zoom, rotation, mirror(0/1). */
    @JvmStatic external fun nativeGetView(h: Long, out: FloatArray)

    /**
     * View size in px (rotation/resize). CONTRACTS §8 names both size parameters `h`; the second is
     * the view height (named [hgt] here, matching [nativeRenderRegion]). Names do not affect JNI.
     */
    @JvmStatic external fun nativeSetViewport(h: Long, w: Int, hgt: Int)

    /**
     * Renders the current composite (incl. live stroke) for the document-space region into [dst]:
     * a DIRECT buffer of at least `w*hgt*4` bytes, RGBA8 premultiplied sRGB (ARGB_8888 memory order).
     */
    @JvmStatic external fun nativeRenderRegion(h: Long, x: Int, y: Int, w: Int, hgt: Int, dst: ByteBuffer): Boolean

    /** Return-flag bits of [nativeInputBatch]. */
    object BatchFlags {
        const val CANVAS_DIRTY: Int = 1 shl 0
        const val VIEW_CHANGED: Int = 1 shl 1
        const val STROKE_COMMITTED: Int = 1 shl 2
        const val HISTORY_CHANGED: Int = 1 shl 3
    }

    /** Number of floats written by [nativeGetView]. */
    const val VIEW_FLOATS: Int = 9
}

/**
 * Abstraction over [NativeBridge] so [EngineSession] can be unit-tested with a fake on a JVM.
 * Signatures mirror CONTRACTS §8 one-to-one, except that object returns are nullable: JNI code can
 * return NULL (e.g. after a pending exception) and Kotlin does not null-check returns of its own
 * `external` declarations, so the session treats null defensively instead of crashing later.
 */
interface EngineBridge {
    fun version(): String
    fun createSession(): Long
    fun destroySession(h: Long)
    fun execute(h: Long, json: String): String?
    fun inputBatch(h: Long, ints: IntArray, floats: FloatArray, longs: LongArray, count: Int): IntArray?
    fun getView(h: Long, out: FloatArray)
    fun setViewport(h: Long, w: Int, hgt: Int)
    fun renderRegion(h: Long, x: Int, y: Int, w: Int, hgt: Int, dst: ByteBuffer): Boolean
}

/** Production [EngineBridge] delegating to the JNI statics. Requires [NativeBridge.load] first. */
object JniEngineBridge : EngineBridge {
    override fun version(): String = NativeBridge.nativeVersion()
    override fun createSession(): Long = NativeBridge.nativeCreateSession()
    override fun destroySession(h: Long) = NativeBridge.nativeDestroySession(h)
    override fun execute(h: Long, json: String): String? = NativeBridge.nativeExecute(h, json)
    override fun inputBatch(h: Long, ints: IntArray, floats: FloatArray, longs: LongArray, count: Int): IntArray? =
        NativeBridge.nativeInputBatch(h, ints, floats, longs, count)
    override fun getView(h: Long, out: FloatArray) = NativeBridge.nativeGetView(h, out)
    override fun setViewport(h: Long, w: Int, hgt: Int) = NativeBridge.nativeSetViewport(h, w, hgt)
    override fun renderRegion(h: Long, x: Int, y: Int, w: Int, hgt: Int, dst: ByteBuffer): Boolean =
        NativeBridge.nativeRenderRegion(h, x, y, w, hgt, dst)
}
