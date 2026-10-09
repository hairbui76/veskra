package org.veskra.dev.canvas

import android.graphics.Bitmap
import android.graphics.Matrix
import android.os.Handler
import android.os.Looper
import org.veskra.dev.engine.CanvasRenderer
import org.veskra.kit.DisplayTile
import org.veskra.kit.ViewState
import java.nio.ByteBuffer

/**
 * UI-THREAD display state for the current document: one ARGB_8888 Bitmap per 512×512 display tile
 * (edge tiles clipped) plus the doc→view matrix from `nativeGetView`. Held by the ViewModel so it
 * survives configuration changes; [CanvasView] only draws it.
 */
class CanvasDisplay {
    private val main = Handler(Looper.getMainLooper())

    var docWidth: Int = 0
        private set
    var docHeight: Int = 0
        private set

    /** Key = (ty shl 16) or tx. */
    private val tiles = HashMap<Int, PlacedTile>()
    val matrix: Matrix = Matrix()
    var view: ViewState = ViewState.IDENTITY
        private set
    private val matrixValues = FloatArray(9)

    /** Set by the attached [CanvasView]; invoked on the UI thread. */
    var onInvalidate: (() -> Unit)? = null

    /** Engine-thread callbacks routed to the UI thread. */
    var historyListener: ((strokeCommitted: Boolean) -> Unit)? = null

    class PlacedTile(val left: Float, val top: Float, val bitmap: Bitmap)

    fun forEachTile(block: (PlacedTile) -> Unit) = tiles.values.forEach(block)

    /** UI thread: new document → drop old bitmaps. */
    fun reset(width: Int, height: Int) {
        tiles.values.forEach { it.bitmap.recycle() }
        tiles.clear()
        docWidth = width
        docHeight = height
        onInvalidate?.invoke()
    }

    fun release() = reset(0, 0)

    /**
     * Any thread (used from the engine thread before the new renderer posts its first tile): queues
     * [reset] on the main looper so it is ordered before every tile upload of the new document.
     */
    fun postReset(width: Int, height: Int) {
        main.post { reset(width, height) }
    }

    val engineListener: CanvasRenderer.Listener = object : CanvasRenderer.Listener {
        override fun onTileReady(tile: DisplayTile, pixels: ByteBuffer, release: () -> Unit) {
            main.post {
                try {
                    upload(tile, pixels)
                } finally {
                    release()
                }
                onInvalidate?.invoke()
            }
        }

        override fun onViewChanged(view: ViewState) {
            main.post { applyView(view) }
        }

        override fun onHistoryChanged(strokeCommitted: Boolean) {
            main.post { historyListener?.invoke(strokeCommitted) }
        }
    }

    private fun upload(tile: DisplayTile, pixels: ByteBuffer) {
        val r = tile.rect
        if (r.right > docWidth || r.bottom > docHeight) return // stale tile from a previous document
        val key = (tile.ty shl 16) or tile.tx
        var placed = tiles[key]
        if (placed == null || placed.bitmap.width != r.width || placed.bitmap.height != r.height) {
            placed?.bitmap?.recycle()
            val bmp = Bitmap.createBitmap(r.width, r.height, Bitmap.Config.ARGB_8888)
            bmp.isPremultiplied = true
            placed = PlacedTile(r.left.toFloat(), r.top.toFloat(), bmp)
            tiles[key] = placed
        }
        pixels.position(0)
        // Contract: RGBA8 premultiplied sRGB in ARGB_8888 memory order → direct copy, no conversion.
        placed.bitmap.copyPixelsFromBuffer(pixels)
    }

    private fun applyView(v: ViewState) {
        view = v
        matrix.setValues(v.toAndroidMatrixValues(matrixValues))
        onInvalidate?.invoke()
    }
}
