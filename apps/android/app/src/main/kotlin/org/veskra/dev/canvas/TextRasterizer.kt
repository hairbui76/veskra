package org.veskra.dev.canvas

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import kotlin.math.ceil

/**
 * Draws text into a transparent sRGB bitmap with the platform text engine (shaping, fallback fonts
 * and emoji included), for the Text tool. Call off the UI thread; the caller recycles the bitmap.
 */
object TextRasterizer {
    /** Longest bitmap edge; larger text is rejected rather than silently scaled. */
    const val MAX_EDGE: Int = 4096
    const val MAX_CHARS: Int = 2000

    /** Null when [text] is blank or the result would exceed [MAX_EDGE]. */
    fun render(text: String, sizePx: Float, argb: Int): Bitmap? {
        val lines = text.take(MAX_CHARS).lines().map { it.trimEnd() }.dropLastWhile { it.isEmpty() }
        if (lines.none { it.isNotBlank() } || !sizePx.isFinite() || sizePx < 1f) return null
        val paint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
            color = argb
            textSize = sizePx
            typeface = Typeface.DEFAULT
        }
        val metrics = paint.fontMetrics
        val lineHeight = metrics.descent - metrics.ascent + metrics.leading
        val pad = ceil(sizePx * 0.15f).toInt()
        val width = ceil(lines.maxOf { paint.measureText(it) }).toInt() + 2 * pad
        val height = ceil(lineHeight * lines.size).toInt() + 2 * pad
        if (width <= 0 || height <= 0 || width > MAX_EDGE || height > MAX_EDGE) return null
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        lines.forEachIndexed { i, line -> canvas.drawText(line, pad.toFloat(), pad - metrics.ascent + i * lineHeight, paint) }
        return bitmap
    }
}
