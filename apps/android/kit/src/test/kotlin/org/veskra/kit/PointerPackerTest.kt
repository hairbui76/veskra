package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertSame
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class PointerPackerTest {
    private fun stylus(action: Int, x: Float, t: Long, flags: Int = 0) = PointerSample(
        action = action, pointerId = 0, toolType = MotionConstants.TOOL_TYPE_STYLUS, buttons = 0, flags = flags,
        x = x, y = 2f * x, pressure = 0.5f, tilt = 0.3f, orientation = -1.2f, touchMajor = 4f, touchMinor = 3f,
        eventTimeNanos = t,
    )

    @Test
    fun `packs the exact CONTRACTS section 8 layout`() {
        val p = PointerPacker(initialCapacity = 4)
        p.add(stylus(MotionConstants.ACTION_DOWN, 10f, 1_000L))
        p.add(stylus(MotionConstants.ACTION_MOVE, 11f, 2_000L, PointerFlags.HISTORICAL))
        p.add(stylus(MotionConstants.ACTION_MOVE, 12f, 3_000L, PointerFlags.PREDICTED))
        assertEquals(3, p.count)
        assertArrayEquals(
            intArrayOf(0, 0, 2, 0, 0, 2, 0, 2, 0, 1, 2, 0, 2, 0, 2),
            p.ints.copyOf(15),
        )
        val f = p.floats
        assertEquals(10f, f[0]); assertEquals(20f, f[1]); assertEquals(0.5f, f[2]); assertEquals(0.3f, f[3])
        assertEquals(-1.2f, f[4]); assertEquals(4f, f[5]); assertEquals(3f, f[6]); assertTrue(f[7].isNaN())
        assertEquals(11f, f[8]); assertEquals(12f, f[16])
        assertArrayEquals(longArrayOf(1_000L, 2_000L, 3_000L), p.longs.copyOf(3))
    }

    @Test
    fun `absent axes stay NaN and are never invented`() {
        val p = PointerPacker()
        p.add(PointerSample(action = MotionConstants.ACTION_DOWN, pointerId = 3, toolType = MotionConstants.TOOL_TYPE_FINGER, x = 1f, y = 2f, eventTimeNanos = 5L))
        val s = p.sampleAt(0)
        assertTrue(s.pressure.isNaN()); assertTrue(s.tilt.isNaN()); assertTrue(s.orientation.isNaN())
        assertTrue(s.touchMajor.isNaN()); assertTrue(s.touchMinor.isNaN())
        assertEquals(3, s.pointerId)
    }

    @Test
    fun `round trips samples through sampleAt`() {
        val p = PointerPacker()
        val s = stylus(MotionConstants.ACTION_MOVE, 7f, 9L, PointerFlags.HISTORICAL or PointerFlags.CANCELED)
        p.add(s)
        assertEquals(s.copy(), p.sampleAt(0).copy())
    }

    @Test
    fun `grows geometrically then reuses buffers without reallocation`() {
        val p = PointerPacker(initialCapacity = 2, maxEvents = 64)
        repeat(40) { p.add(stylus(MotionConstants.ACTION_MOVE, it.toFloat(), it.toLong())) }
        assertEquals(64, p.capacity)
        val ints = p.ints
        val floats = p.floats
        val longs = p.longs
        p.clear()
        assertTrue(p.isEmpty)
        repeat(40) { p.add(stylus(MotionConstants.ACTION_MOVE, it.toFloat(), it.toLong())) }
        assertSame(ints, p.ints)
        assertSame(floats, p.floats)
        assertSame(longs, p.longs)
        assertEquals(39L, p.longs[39])
    }

    @Test
    fun `refuses to exceed the bound so the caller flushes`() {
        val p = PointerPacker(initialCapacity = 1, maxEvents = 3)
        repeat(3) { assertTrue(p.add(stylus(MotionConstants.ACTION_MOVE, 1f, it.toLong()))) }
        assertTrue(p.isFull)
        assertFalse(p.add(stylus(MotionConstants.ACTION_MOVE, 1f, 99L)))
        assertEquals(3, p.count)
    }

    @Test
    fun `rejects non finite positions`() {
        val p = PointerPacker()
        assertThrows(IllegalArgumentException::class.java) { p.add(stylus(MotionConstants.ACTION_MOVE, Float.NaN, 1L)) }
        assertEquals(0, p.count)
    }

    @Test
    fun `pool recycles packers so steady state allocates nothing`() {
        val pool = PointerPackerPool(maxPooled = 2, initialCapacity = 8)
        repeat(100) {
            val a = pool.acquire()
            a.add(stylus(MotionConstants.ACTION_MOVE, 1f, 1L))
            pool.release(a)
        }
        assertEquals(1, pool.createdCount)
        val a = pool.acquire()
        assertTrue(a.isEmpty, "released packers are cleared")
        val b = pool.acquire()
        val c = pool.acquire()
        pool.release(a); pool.release(b); pool.release(c) // only 2 retained
        pool.release(a) // double release is ignored
        assertEquals(3, pool.createdCount)
    }

    @Test
    fun `kit constants mirror Android MotionEvent values`() {
        // Values from the Android SDK reference (android.view.MotionEvent); :app re-checks against the SDK.
        assertEquals(0, MotionConstants.ACTION_DOWN)
        assertEquals(2, MotionConstants.ACTION_MOVE)
        assertEquals(6, MotionConstants.ACTION_POINTER_UP)
        assertEquals(9, MotionConstants.ACTION_HOVER_ENTER)
        assertEquals(2, MotionConstants.TOOL_TYPE_STYLUS)
        assertEquals(4, MotionConstants.TOOL_TYPE_ERASER)
        assertEquals(1, PointerFlags.HISTORICAL)
        assertEquals(2, PointerFlags.PREDICTED)
        assertEquals(4, PointerFlags.CANCELED)
    }
}
