package org.veskra.kit

/**
 * One display tile: index (tx, ty) and its document-space [rect], clipped to the document
 * (edge tiles are smaller than [TileUploadPlanner.tileSize]).
 */
data class DisplayTile(val tx: Int, val ty: Int, val rect: RectI)

/**
 * Decides which display tiles (default 512×512 ARGB_8888 bitmaps covering the document 1:1) must be
 * re-rendered via `nativeRenderRegion` and re-uploaded, given the dirty rects from
 * `nativeInputBatch` / commands. Pure math, no Android types.
 *
 * Dirty tiles outside the visible region stay pending and are returned once they become visible,
 * so panning never shows stale pixels and off-screen work is deferred. Not thread-safe; owned by the
 * engine thread.
 *
 * Note (REN-02 follow-up): at 1:1 a 8192² document needs 256 tiles = 256 MiB of bitmaps; zoomed-out
 * display LOD/mip tiles are out of scope here and must land before G6 memory gates.
 */
class TileUploadPlanner(val docWidth: Int, val docHeight: Int, val tileSize: Int = DEFAULT_TILE_SIZE) {
    init {
        require(docWidth > 0 && docHeight > 0) { "document must be non-empty" }
        require(tileSize > 0) { "tileSize must be > 0" }
    }

    val columns: Int = ceilDiv(docWidth, tileSize)
    val rows: Int = ceilDiv(docHeight, tileSize)
    private val dirty = BooleanArray(Math.multiplyExact(columns, rows))
    private var dirtyCount = 0

    val pendingCount: Int get() = dirtyCount
    val docRect: RectI get() = RectI.ofSize(docWidth, docHeight)

    fun tileRect(tx: Int, ty: Int): RectI {
        require(tx in 0 until columns && ty in 0 until rows) { "tile ($tx,$ty) out of range" }
        val l = tx * tileSize
        val t = ty * tileSize
        return RectI(l, t, minOf(l + tileSize, docWidth), minOf(t + tileSize, docHeight))
    }

    fun isDirty(tx: Int, ty: Int): Boolean = dirty[ty * columns + tx]

    fun markAllDirty() {
        dirty.fill(true)
        dirtyCount = dirty.size
    }

    /** Marks tiles touched by half-open document rect [r]; out-of-document parts are ignored. */
    fun markDirty(r: RectI) {
        val c = r.intersect(docRect) ?: return
        val tx0 = c.left / tileSize
        val ty0 = c.top / tileSize
        val tx1 = (c.right - 1) / tileSize
        val ty1 = (c.bottom - 1) / tileSize
        for (ty in ty0..ty1) for (tx in tx0..tx1) set(tx, ty)
    }

    fun markDirty(rects: Iterable<RectI>) = rects.forEach(::markDirty)

    /** Applies an input-batch result: specific rects, or a full refresh when no detail is available. */
    fun apply(result: InputBatchResult) {
        if (result.fullRefresh) markAllDirty() else if (result.canvasDirty) markDirty(result.rects)
    }

    /**
     * Returns dirty tiles intersecting [visible] (document space; null = whole document) in row-major
     * order, at most [limit], and marks them clean. Call [markDirty] again if an upload fails.
     */
    fun takeUploads(visible: RectI? = null, limit: Int = Int.MAX_VALUE): List<DisplayTile> {
        require(limit >= 0)
        if (dirtyCount == 0 || limit == 0) return emptyList()
        val area = (visible ?: docRect).intersect(docRect) ?: return emptyList()
        val tx0 = area.left / tileSize
        val ty0 = area.top / tileSize
        val tx1 = (area.right - 1) / tileSize
        val ty1 = (area.bottom - 1) / tileSize
        val out = ArrayList<DisplayTile>()
        loop@ for (ty in ty0..ty1) {
            for (tx in tx0..tx1) {
                val i = ty * columns + tx
                if (dirty[i]) {
                    dirty[i] = false
                    dirtyCount--
                    out.add(DisplayTile(tx, ty, tileRect(tx, ty)))
                    if (out.size >= limit) break@loop
                }
            }
        }
        return out
    }

    private fun set(tx: Int, ty: Int) {
        val i = ty * columns + tx
        if (!dirty[i]) {
            dirty[i] = true
            dirtyCount++
        }
    }

    companion object {
        const val DEFAULT_TILE_SIZE: Int = 512

        private fun ceilDiv(a: Int, b: Int): Int = (a + b - 1) / b

        /**
         * Document-space bounding box of the view rectangle `[0,w)×[0,h)` under the inverse of the
         * doc→view affine (handles rotation/mirror). Returns null if the affine is singular.
         */
        fun visibleDocRect(view: ViewState, viewWidth: Int, viewHeight: Int): RectI? {
            val det = view.a.toDouble() * view.d - view.b.toDouble() * view.c
            if (det == 0.0 || !det.isFinite()) return null
            // Inverse affine.
            val ia = view.d / det
            val ib = -view.b / det
            val ic = -view.c / det
            val id = view.a / det
            val ie = -(ia * view.e + ic * view.f)
            val iff = -(ib * view.e + id * view.f)
            var minX = Double.POSITIVE_INFINITY
            var minY = Double.POSITIVE_INFINITY
            var maxX = Double.NEGATIVE_INFINITY
            var maxY = Double.NEGATIVE_INFINITY
            for ((vx, vy) in listOf(0.0 to 0.0, viewWidth.toDouble() to 0.0, 0.0 to viewHeight.toDouble(), viewWidth.toDouble() to viewHeight.toDouble())) {
                val x = ia * vx + ic * vy + ie
                val y = ib * vx + id * vy + iff
                minX = minOf(minX, x); maxX = maxOf(maxX, x)
                minY = minOf(minY, y); maxY = maxOf(maxY, y)
            }
            fun clampInt(v: Double): Int = v.coerceIn(Int.MIN_VALUE / 2.0, Int.MAX_VALUE / 2.0).toInt()
            return RectI(clampInt(Math.floor(minX)), clampInt(Math.floor(minY)), clampInt(Math.ceil(maxX)), clampInt(Math.ceil(maxY)))
        }
    }
}
