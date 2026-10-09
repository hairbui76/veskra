package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class TileUploadPlannerTest {
    private fun ids(tiles: List<DisplayTile>) = tiles.map { it.tx to it.ty }

    @Test
    fun `grid covers the document with clipped edge tiles`() {
        val p = TileUploadPlanner(1300, 600)
        assertEquals(3, p.columns)
        assertEquals(2, p.rows)
        assertEquals(RectI(1024, 512, 1300, 600), p.tileRect(2, 1))
        assertEquals(RectI(0, 0, 512, 512), p.tileRect(0, 0))
    }

    @Test
    fun `a small rect dirties exactly one tile`() {
        val p = TileUploadPlanner(2048, 2048)
        p.markDirty(RectI(10, 10, 20, 20))
        assertEquals(listOf(0 to 0), ids(p.takeUploads()))
        assertEquals(0, p.pendingCount)
        assertTrue(p.takeUploads().isEmpty(), "taken tiles are clean")
    }

    @Test
    fun `half open edges do not spill into the next tile`() {
        val p = TileUploadPlanner(2048, 2048)
        p.markDirty(RectI(0, 0, 512, 512))
        assertEquals(listOf(0 to 0), ids(p.takeUploads()))
        p.markDirty(RectI(511, 511, 513, 513))
        assertEquals(listOf(0 to 0, 1 to 0, 0 to 1, 1 to 1), ids(p.takeUploads()))
    }

    @Test
    fun `rects are clipped to the document and empties ignored`() {
        val p = TileUploadPlanner(1000, 1000)
        p.markDirty(RectI(-500, -500, -1, -1))
        p.markDirty(RectI(5, 5, 5, 50))
        p.markDirty(RectI(990, 990, 5000, 5000))
        assertEquals(listOf(1 to 1), ids(p.takeUploads()))
        assertEquals(RectI(512, 512, 1000, 1000), p.tileRect(1, 1))
    }

    @Test
    fun `overlapping rects are deduplicated`() {
        val p = TileUploadPlanner(2048, 1024)
        p.markDirty(listOf(RectI(0, 0, 600, 100), RectI(100, 0, 700, 50), RectI(500, 0, 520, 10)))
        assertEquals(2, p.pendingCount)
        assertEquals(listOf(0 to 0, 1 to 0), ids(p.takeUploads()))
    }

    @Test
    fun `invisible dirty tiles stay pending until visible`() {
        val p = TileUploadPlanner(2048, 2048)
        p.markAllDirty()
        assertEquals(16, p.pendingCount)
        val first = p.takeUploads(visible = RectI(0, 0, 600, 300))
        assertEquals(listOf(0 to 0, 1 to 0), ids(first))
        assertEquals(14, p.pendingCount)
        assertTrue(p.isDirty(3, 3))
        assertEquals(14, p.takeUploads().size)
    }

    @Test
    fun `limit bounds work per frame`() {
        val p = TileUploadPlanner(2048, 2048)
        p.markAllDirty()
        assertEquals(listOf(0 to 0, 1 to 0, 2 to 0), ids(p.takeUploads(limit = 3)))
        assertEquals(13, p.pendingCount)
        assertTrue(p.takeUploads(visible = RectI(5000, 5000, 6000, 6000)).isEmpty())
    }

    @Test
    fun `batch results drive the planner`() {
        val p = TileUploadPlanner(1024, 1024)
        p.apply(InputBatchResult(intArrayOf(NativeBridge.BatchFlags.CANVAS_DIRTY, 1, 600, 600, 610, 610)))
        assertEquals(listOf(1 to 1), ids(p.takeUploads()))
        p.apply(InputBatchResult(intArrayOf(NativeBridge.BatchFlags.VIEW_CHANGED, 0)))
        assertEquals(0, p.pendingCount, "view change alone re-uses uploaded tiles")
        p.apply(InputBatchResult(intArrayOf(NativeBridge.BatchFlags.CANVAS_DIRTY, 0)))
        assertEquals(4, p.pendingCount, "dirty without rects means full refresh")
        p.takeUploads()
        p.apply(InputBatchResult(null))
        assertEquals(4, p.pendingCount, "malformed result means full refresh")
    }

    @Test
    fun `visible document rect inverts the view affine`() {
        // zoom 2, translate (100, 50): doc x = (vx - 100) / 2.
        val v = ViewState(2f, 0f, 0f, 2f, 100f, 50f, 2f, 0f, false)
        assertEquals(RectI(-50, -25, 350, 275), TileUploadPlanner.visibleDocRect(v, 800, 600))
        // 90° rotation: x' = -y + 600, y' = x  → doc bounds of an 600×800 view.
        val r = ViewState(0f, 1f, -1f, 0f, 600f, 0f, 1f, (Math.PI / 2).toFloat(), false)
        assertEquals(RectI(0, 0, 800, 600), TileUploadPlanner.visibleDocRect(r, 600, 800))
        assertNull(TileUploadPlanner.visibleDocRect(ViewState(0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, false), 10, 10))
        // Mirror (a = -1) still yields a proper rect.
        val m = TileUploadPlanner.visibleDocRect(ViewState(-1f, 0f, 0f, 1f, 100f, 0f, 1f, 0f, true), 100, 100)!!
        assertEquals(RectI(0, 0, 100, 100), m)
        assertFalse(m.isEmpty)
    }
}
