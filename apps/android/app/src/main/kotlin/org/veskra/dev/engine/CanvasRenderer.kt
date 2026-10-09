package org.veskra.dev.engine

import org.veskra.kit.DisplayTile
import org.veskra.kit.EngineSession
import org.veskra.kit.InputBatchResult
import org.veskra.kit.RectI
import org.veskra.kit.TileUploadPlanner
import org.veskra.kit.ViewState
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * ENGINE-THREAD side of display updates: tracks dirty display tiles ([TileUploadPlanner]), renders
 * them with `nativeRenderRegion` into a small bounded pool of direct staging buffers and hands each
 * filled buffer to the UI thread ([Listener.onTileReady]), which copies it into a Bitmap with
 * `copyPixelsFromBuffer` and then releases it. When all staging buffers are in flight, remaining
 * dirty tiles wait (still marked dirty) and rendering resumes when a buffer is released, so memory
 * stays bounded and the UI thread never waits on native rendering.
 */
class CanvasRenderer(
    private val host: EngineHost,
    val docWidth: Int,
    val docHeight: Int,
    private val listener: Listener,
) {
    /** Callbacks are invoked ON THE ENGINE THREAD; implementations must hop to the UI thread. */
    interface Listener {
        /**
         * [pixels] holds `rect.width * rect.height * 4` bytes (RGBA8 premultiplied, ARGB_8888 memory
         * order) from position 0. Call [release] exactly once after copying (from any thread).
         */
        fun onTileReady(tile: DisplayTile, pixels: ByteBuffer, release: () -> Unit)

        fun onViewChanged(view: ViewState)

        /** History changed (stroke committed, undo/redo via gesture). */
        fun onHistoryChanged(strokeCommitted: Boolean)
    }

    private val planner = TileUploadPlanner(docWidth, docHeight)
    private val freeStaging = ArrayDeque<ByteBuffer>()
    private var stagingCreated = 0
    private var disposed = false
    private var viewportW = 0
    private var viewportH = 0
    private var view: ViewState = ViewState.IDENTITY

    init {
        planner.markAllDirty()
    }

    fun onBatch(s: EngineSession, result: InputBatchResult) {
        if (disposed) return
        planner.apply(result)
        if (result.viewChanged) refreshView(s)
        if (result.historyChanged || result.strokeCommitted) listener.onHistoryChanged(result.strokeCommitted)
        pump(s)
    }

    /** After a command changed pixels: specific document rects, or everything when null. */
    fun invalidate(s: EngineSession, rects: List<RectI>?) {
        if (disposed) return
        if (rects == null) planner.markAllDirty() else planner.markDirty(rects)
        pump(s)
    }

    fun setViewport(s: EngineSession, width: Int, height: Int) {
        if (disposed || width <= 0 || height <= 0) return
        viewportW = width
        viewportH = height
        s.setViewport(width, height)
        refreshView(s)
        pump(s)
    }

    fun refreshView(s: EngineSession) {
        val v = s.view() ?: return
        view = v
        listener.onViewChanged(v)
    }

    /** Renders as many visible dirty tiles as staging buffers allow. */
    fun pump(s: EngineSession) {
        if (disposed || viewportW == 0 || viewportH == 0) return
        val visible = TileUploadPlanner.visibleDocRect(view, viewportW, viewportH) ?: return
        while (true) {
            val tile = planner.takeUploads(visible, limit = 1).firstOrNull() ?: return
            val buf = acquireStaging()
            if (buf == null) {
                planner.markDirty(tile.rect) // wait for a buffer to come back
                return
            }
            buf.clear()
            val ok = try {
                s.renderRegion(tile.rect, buf)
            } catch (e: IllegalArgumentException) {
                false
            }
            if (!ok) {
                planner.markDirty(tile.rect)
                recycle(buf)
                return // retry on the next batch/command rather than spinning
            }
            buf.position(0)
            buf.limit(tile.rect.width * tile.rect.height * 4)
            var released = false
            listener.onTileReady(tile, buf) {
                host.handler.post {
                    if (released) return@post
                    released = true
                    recycle(buf)
                    host.post { session -> pump(session) }
                }
            }
        }
    }

    fun dispose() {
        disposed = true
        freeStaging.clear()
    }

    private fun acquireStaging(): ByteBuffer? = freeStaging.removeLastOrNull()
        ?: if (stagingCreated < MAX_STAGING_BUFFERS) {
            stagingCreated++
            ByteBuffer.allocateDirect(TILE_BYTES).order(ByteOrder.nativeOrder())
        } else {
            null
        }

    private fun recycle(buf: ByteBuffer) {
        if (!disposed) freeStaging.addLast(buf)
    }

    companion object {
        const val MAX_STAGING_BUFFERS: Int = 6
        private const val TILE_BYTES: Int =
            TileUploadPlanner.DEFAULT_TILE_SIZE * TileUploadPlanner.DEFAULT_TILE_SIZE * 4
    }
}
