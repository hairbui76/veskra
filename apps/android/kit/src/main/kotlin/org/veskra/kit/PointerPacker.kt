package org.veskra.kit

/**
 * Android `MotionEvent` constant VALUES needed by the packed layout (CONTRACTS §8: "action/toolType
 * use Android MotionEvent constant values"). Duplicated here because :kit must not import
 * android.*; :app asserts equality against the real SDK constants at startup (debug builds).
 */
object MotionConstants {
    const val ACTION_DOWN: Int = 0
    const val ACTION_UP: Int = 1
    const val ACTION_MOVE: Int = 2
    const val ACTION_CANCEL: Int = 3
    const val ACTION_POINTER_DOWN: Int = 5
    const val ACTION_POINTER_UP: Int = 6
    const val ACTION_HOVER_MOVE: Int = 7
    const val ACTION_HOVER_ENTER: Int = 9
    const val ACTION_HOVER_EXIT: Int = 10

    const val TOOL_TYPE_UNKNOWN: Int = 0
    const val TOOL_TYPE_FINGER: Int = 1
    const val TOOL_TYPE_STYLUS: Int = 2
    const val TOOL_TYPE_MOUSE: Int = 3
    const val TOOL_TYPE_ERASER: Int = 4
    const val TOOL_TYPE_PALM: Int = 5

    const val BUTTON_STYLUS_PRIMARY: Int = 0x20
    const val BUTTON_STYLUS_SECONDARY: Int = 0x40
}

/** Bits of the packed per-event `flags` int (CONTRACTS §8). */
object PointerFlags {
    const val HISTORICAL: Int = 1 shl 0
    const val PREDICTED: Int = 1 shl 1
    /** Android `MotionEvent.FLAG_CANCELED` (e.g. palm rejected by the system). */
    const val CANCELED: Int = 1 shl 2
}

/**
 * Platform-neutral pointer sample. :app converts each MotionEvent pointer (current, historical and
 * MotionEventPredictor output) into exactly this information. Absent axes are `Float.NaN` —
 * never invented (AGENTS.md §3: no invented pressure/tilt).
 */
data class PointerSample(
    val action: Int,
    val pointerId: Int,
    val toolType: Int,
    val buttons: Int = 0,
    val flags: Int = 0,
    val x: Float,
    val y: Float,
    val pressure: Float = Float.NaN,
    val tilt: Float = Float.NaN,
    val orientation: Float = Float.NaN,
    val touchMajor: Float = Float.NaN,
    val touchMinor: Float = Float.NaN,
    val eventTimeNanos: Long,
)

/**
 * Packs pointer events into the reusable parallel arrays of CONTRACTS §8:
 *
 * - `ints[i*5 .. i*5+5]   = [action, pointerId, toolType, buttons, flags]`
 * - `floats[i*8 .. i*8+8] = [x, y, pressure, tilt, orientation, touchMajor, touchMinor, reserved]`
 * - `longs[i]             = eventTimeNanos`
 *
 * Buffers grow geometrically up to [maxEvents] and are then reused: steady-state packing performs
 * no allocation. Not thread-safe: fill on one thread, hand the whole packer to the engine thread
 * (see [PointerPackerPool]).
 */
class PointerPacker(initialCapacity: Int = 64, val maxEvents: Int = DEFAULT_MAX_EVENTS) {
    init {
        require(initialCapacity in 1..maxEvents) { "initialCapacity must be in 1..maxEvents" }
    }

    var ints: IntArray = IntArray(initialCapacity * INTS_PER_EVENT)
        private set
    var floats: FloatArray = FloatArray(initialCapacity * FLOATS_PER_EVENT)
        private set
    var longs: LongArray = LongArray(initialCapacity)
        private set

    /** Number of packed events; pass as `count` to `nativeInputBatch`. */
    var count: Int = 0
        private set

    val capacity: Int get() = longs.size
    val isEmpty: Boolean get() = count == 0
    val isFull: Boolean get() = count >= maxEvents

    /**
     * Appends one event without allocating (unless the buffer must grow). Returns false if the
     * packer already holds [maxEvents]; the caller must flush and retry. Non-finite x/y indicate a
     * caller bug and throw [IllegalArgumentException].
     */
    fun add(
        action: Int,
        pointerId: Int,
        toolType: Int,
        buttons: Int,
        flags: Int,
        x: Float,
        y: Float,
        pressure: Float,
        tilt: Float,
        orientation: Float,
        touchMajor: Float,
        touchMinor: Float,
        eventTimeNanos: Long,
    ): Boolean {
        require(x.isFinite() && y.isFinite()) { "pointer position must be finite" }
        if (count >= maxEvents) return false
        if (count == capacity) grow()
        val ib = count * INTS_PER_EVENT
        ints[ib] = action
        ints[ib + 1] = pointerId
        ints[ib + 2] = toolType
        ints[ib + 3] = buttons
        ints[ib + 4] = flags
        val fb = count * FLOATS_PER_EVENT
        floats[fb] = x
        floats[fb + 1] = y
        floats[fb + 2] = pressure
        floats[fb + 3] = tilt
        floats[fb + 4] = orientation
        floats[fb + 5] = touchMajor
        floats[fb + 6] = touchMinor
        floats[fb + 7] = Float.NaN // reserved
        longs[count] = eventTimeNanos
        count++
        return true
    }

    fun add(s: PointerSample): Boolean = add(
        s.action, s.pointerId, s.toolType, s.buttons, s.flags, s.x, s.y,
        s.pressure, s.tilt, s.orientation, s.touchMajor, s.touchMinor, s.eventTimeNanos,
    )

    /** Forgets packed events; keeps the buffers. */
    fun clear() {
        count = 0
    }

    /** Decodes event [i] (tests/diagnostics only — allocates). */
    fun sampleAt(i: Int): PointerSample {
        require(i in 0 until count) { "index $i out of 0 until $count" }
        val ib = i * INTS_PER_EVENT
        val fb = i * FLOATS_PER_EVENT
        return PointerSample(
            action = ints[ib], pointerId = ints[ib + 1], toolType = ints[ib + 2],
            buttons = ints[ib + 3], flags = ints[ib + 4],
            x = floats[fb], y = floats[fb + 1], pressure = floats[fb + 2], tilt = floats[fb + 3],
            orientation = floats[fb + 4], touchMajor = floats[fb + 5], touchMinor = floats[fb + 6],
            eventTimeNanos = longs[i],
        )
    }

    private fun grow() {
        val newCap = minOf(maxEvents, maxOf(capacity * 2, 1))
        ints = ints.copyOf(newCap * INTS_PER_EVENT)
        floats = floats.copyOf(newCap * FLOATS_PER_EVENT)
        longs = longs.copyOf(newCap)
    }

    companion object {
        const val INTS_PER_EVENT: Int = 5
        const val FLOATS_PER_EVENT: Int = 8
        /** Bound per JNI batch (a 240 Hz stylus with 2 pointers stays far below this per frame). */
        const val DEFAULT_MAX_EVENTS: Int = 4096
    }
}

/**
 * Bounded pool so the UI thread can fill one packer while the engine thread consumes another,
 * without per-event or per-batch allocation in steady state. Thread-safe.
 */
class PointerPackerPool(private val maxPooled: Int = 4, private val initialCapacity: Int = 256) {
    private val free = ArrayDeque<PointerPacker>()
    private var created = 0

    /** Total packers ever allocated (diagnostic for "no allocation in steady state"). */
    val createdCount: Int get() = synchronized(free) { created }

    fun acquire(): PointerPacker = synchronized(free) {
        free.removeLastOrNull() ?: PointerPacker(initialCapacity).also { created++ }
    }

    fun release(p: PointerPacker) {
        p.clear()
        synchronized(free) {
            if (free.size < maxPooled && free.none { it === p }) free.addLast(p)
        }
    }
}
