package org.veskra.dev.canvas

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.os.Build
import android.view.MotionEvent
import android.view.View
import androidx.input.motionprediction.MotionEventPredictor
import org.veskra.dev.R
import org.veskra.dev.engine.EngineHost
import org.veskra.kit.PointerPacker
import org.veskra.kit.PointerPackerPool

/**
 * The drawing surface: a classic [View] hosted in Compose via `AndroidView` (Compose never
 * recomposes per input point or per pixel — AGENTS.md §3).
 *
 * Input: MotionEvents (incl. historical samples, hover, and MotionEventPredictor predictions flagged
 * PREDICTED) are packed on the UI thread into a pooled [PointerPacker] — no per-event allocation —
 * and handed to the single engine thread, which calls `nativeInputBatch`. Stylus streams request
 * unbuffered dispatch for lower latency.
 *
 * Display: draws the [CanvasDisplay] tile bitmaps through the doc→view matrix from `nativeGetView`
 * using `Canvas.concat`; only dirty tiles are re-rendered/re-uploaded (CanvasRenderer).
 */
@SuppressLint("ViewConstructor")
class CanvasView(context: Context) : View(context) {
    private var host: EngineHost? = null
    private var display: CanvasDisplay? = null

    private val adapter = MotionEventAdapter()
    private val pool = PointerPackerPool(maxPooled = 4, initialCapacity = 256)
    private var current: PointerPacker = pool.acquire()
    private val predictor: MotionEventPredictor = MotionEventPredictor.newInstance(this)

    private val sink = PointerSink { action, id, tool, buttons, flags, x, y, p, tilt, orient, major, minor, t ->
        if (!current.add(action, id, tool, buttons, flags, x, y, p, tilt, orient, major, minor, t)) {
            flush()
            current.add(action, id, tool, buttons, flags, x, y, p, tilt, orient, major, minor, t)
        }
    }

    private val tilePaint = Paint().apply { isFilterBitmap = true }
    private val tileDst = RectF()
    private val paperEdgePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 0f // hairline regardless of zoom
        color = Color.argb(96, 0, 0, 0)
    }
    private val hoverPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 1.5f * resources.displayMetrics.density
        color = Color.argb(200, 255, 255, 255)
    }
    private val backdrop = Color.rgb(0x2A, 0x2D, 0x33)

    private var hoverX = Float.NaN
    private var hoverY = Float.NaN

    /** Brush diameter in document px, for the hover ring (set by the editor). */
    var brushSizeDoc: Float = 8f

    init {
        isFocusable = true
        isClickable = true
        contentDescription = context.getString(R.string.editor_canvas_cd)
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_YES
    }

    fun bind(engineHost: EngineHost, canvasDisplay: CanvasDisplay) {
        if (host === engineHost && display === canvasDisplay) return
        display?.onInvalidate = null
        host = engineHost
        display = canvasDisplay
        canvasDisplay.onInvalidate = { postInvalidateOnAnimation() }
        if (width > 0 && height > 0) engineHost.setViewport(width, height)
        invalidate()
    }

    override fun onDetachedFromWindow() {
        display?.onInvalidate = null
        super.onDetachedFromWindow()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        host?.setViewport(w, h)
    }

    @SuppressLint("ClickableViewAccessibility") // drawing surface; all actions have accessible equivalents in the tool rail
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (host == null) return false
        val masked = event.actionMasked
        if (masked == MotionEvent.ACTION_DOWN && isPen(event) && Build.VERSION.SDK_INT >= 30) {
            requestUnbufferedDispatch(event)
        }
        predictor.record(event)
        adapter.pack(event, sink)
        if (masked == MotionEvent.ACTION_MOVE && isPen(event)) {
            predictor.predict()?.let { predicted ->
                adapter.pack(predicted, sink, predicted = true)
                predicted.recycle()
            }
        }
        if (masked == MotionEvent.ACTION_DOWN || masked == MotionEvent.ACTION_UP) hoverX = Float.NaN
        flush()
        return true
    }

    override fun onHoverEvent(event: MotionEvent): Boolean {
        if (host == null) return super.onHoverEvent(event)
        adapter.pack(event, sink)
        flush()
        if (event.actionMasked == MotionEvent.ACTION_HOVER_EXIT) {
            hoverX = Float.NaN
        } else {
            hoverX = event.x
            hoverY = event.y
        }
        invalidate()
        return true
    }

    private fun isPen(e: MotionEvent): Boolean {
        val t = e.getToolType(0)
        return t == MotionEvent.TOOL_TYPE_STYLUS || t == MotionEvent.TOOL_TYPE_ERASER
    }

    /** Hands the filled packer to the engine thread; continues with a pooled one. */
    private fun flush() {
        val h = host ?: return
        if (current.isEmpty) return
        val batch = current
        current = pool.acquire()
        h.post { s ->
            try {
                val result = s.inputBatch(batch)
                h.renderer?.onBatch(s, result)
            } finally {
                pool.release(batch)
            }
        }
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(backdrop)
        val d = display ?: return
        if (d.docWidth == 0) return
        canvas.save()
        canvas.concat(d.matrix)
        // Smooth when minified or rotated; nearest-neighbour at >= 100% for crisp pixel inspection.
        tilePaint.isFilterBitmap = d.view.zoom < 1f || d.view.rotation != 0f
        // Tile edges rarely land on whole device pixels; without a small overlap the anti-aliased
        // edges of neighbouring tiles leave hairline seams. One device pixel of overlap hides them.
        val overlap = 1f / d.view.zoom.coerceAtLeast(1e-3f)
        d.forEachTile {
            tileDst.set(it.left, it.top, it.left + it.bitmap.width + overlap, it.top + it.bitmap.height + overlap)
            canvas.drawBitmap(it.bitmap, null, tileDst, tilePaint)
        }
        canvas.drawRect(0f, 0f, d.docWidth.toFloat(), d.docHeight.toFloat(), paperEdgePaint)
        canvas.restore()
        if (!hoverX.isNaN()) {
            val r = (brushSizeDoc * d.view.zoom / 2f).coerceAtLeast(2f)
            canvas.drawCircle(hoverX, hoverY, r, hoverPaint)
        }
    }
}
