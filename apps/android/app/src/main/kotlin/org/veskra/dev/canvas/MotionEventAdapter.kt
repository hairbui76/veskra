package org.veskra.dev.canvas

import android.os.Build
import android.util.SparseArray
import android.view.InputDevice
import android.view.MotionEvent
import org.veskra.kit.PointerFlags

/** Receives packed samples without allocation (implemented by [CanvasView] over a PointerPacker). */
fun interface PointerSink {
    fun add(
        action: Int, pointerId: Int, toolType: Int, buttons: Int, flags: Int,
        x: Float, y: Float, pressure: Float, tilt: Float, orientation: Float,
        touchMajor: Float, touchMinor: Float, eventTimeNanos: Long,
    )
}

/**
 * Converts Android [MotionEvent]s into the CONTRACTS §8 per-pointer sample stream:
 *
 * - every historical sample of MOVE/HOVER_MOVE first (flag HISTORICAL), in time order, then the
 *   current sample — no MotionEvent data is dropped (ARCHITECTURE §D);
 * - POINTER_DOWN/POINTER_UP emit only the pointer that changed;
 * - FLAG_CANCELED (system palm rejection) is forwarded as flag CANCELED;
 * - predicted events from MotionEventPredictor are packed with flag PREDICTED and are never treated
 *   as confirmed by the engine (AGENTS.md §3);
 * - axes the input device does not report are NaN, never Android's synthetic defaults
 *   (e.g. pressure 1.0 for devices without a pressure sensor).
 *
 * Stroke/gesture arbitration (palm, two-finger, stylus vs finger) happens in Rust (veskra-input).
 */
class MotionEventAdapter {
    private class AxisSupport(
        val source: Int,
        val pressure: Boolean,
        val tilt: Boolean,
        val orientation: Boolean,
        val touchMajor: Boolean,
        val touchMinor: Boolean,
    )

    private val axisCache = SparseArray<AxisSupport>()

    private fun axes(ev: MotionEvent): AxisSupport {
        val cached = axisCache[ev.deviceId]
        if (cached != null && cached.source == ev.source) return cached
        val dev: InputDevice? = ev.device
        fun has(axis: Int) = dev?.getMotionRange(axis, ev.source) != null
        val s = AxisSupport(
            source = ev.source,
            pressure = has(MotionEvent.AXIS_PRESSURE),
            tilt = has(MotionEvent.AXIS_TILT),
            orientation = has(MotionEvent.AXIS_ORIENTATION),
            touchMajor = has(MotionEvent.AXIS_TOUCH_MAJOR),
            touchMinor = has(MotionEvent.AXIS_TOUCH_MINOR),
        )
        axisCache.put(ev.deviceId, s)
        return s
    }

    /** Device topology changed (e.g. stylus paired): forget cached axis support. */
    fun clearDeviceCache() = axisCache.clear()

    fun pack(ev: MotionEvent, sink: PointerSink, predicted: Boolean = false) {
        val masked = ev.actionMasked
        val ax = axes(ev)
        var baseFlags = if (predicted) PointerFlags.PREDICTED else 0
        if (ev.flags and MotionEvent.FLAG_CANCELED != 0) baseFlags = baseFlags or PointerFlags.CANCELED
        val buttons = ev.buttonState

        if (masked == MotionEvent.ACTION_MOVE || masked == MotionEvent.ACTION_HOVER_MOVE) {
            for (h in 0 until ev.historySize) {
                val t = historicalTimeNanos(ev, h)
                for (p in 0 until ev.pointerCount) {
                    sink.add(
                        masked, ev.getPointerId(p), ev.getToolType(p), buttons, baseFlags or PointerFlags.HISTORICAL,
                        ev.getHistoricalX(p, h), ev.getHistoricalY(p, h),
                        if (ax.pressure) ev.getHistoricalPressure(p, h) else Float.NaN,
                        if (ax.tilt) ev.getHistoricalAxisValue(MotionEvent.AXIS_TILT, p, h) else Float.NaN,
                        if (ax.orientation) ev.getHistoricalOrientation(p, h) else Float.NaN,
                        if (ax.touchMajor) ev.getHistoricalTouchMajor(p, h) else Float.NaN,
                        if (ax.touchMinor) ev.getHistoricalTouchMinor(p, h) else Float.NaN,
                        t,
                    )
                }
            }
        }

        val t = eventTimeNanos(ev)
        val isPointerChange = masked == MotionEvent.ACTION_POINTER_DOWN || masked == MotionEvent.ACTION_POINTER_UP
        for (p in 0 until ev.pointerCount) {
            if (isPointerChange && p != ev.actionIndex) continue
            sink.add(
                masked, ev.getPointerId(p), ev.getToolType(p), buttons, baseFlags,
                ev.getX(p), ev.getY(p),
                if (ax.pressure) ev.getPressure(p) else Float.NaN,
                if (ax.tilt) ev.getAxisValue(MotionEvent.AXIS_TILT, p) else Float.NaN,
                if (ax.orientation) ev.getOrientation(p) else Float.NaN,
                if (ax.touchMajor) ev.getTouchMajor(p) else Float.NaN,
                if (ax.touchMinor) ev.getTouchMinor(p) else Float.NaN,
                t,
            )
        }
    }

    private fun eventTimeNanos(ev: MotionEvent): Long =
        if (Build.VERSION.SDK_INT >= 34) ev.eventTimeNanos else ev.eventTime * NANOS_PER_MILLI

    private fun historicalTimeNanos(ev: MotionEvent, h: Int): Long =
        if (Build.VERSION.SDK_INT >= 34) ev.getHistoricalEventTimeNanos(h) else ev.getHistoricalEventTime(h) * NANOS_PER_MILLI

    companion object {
        private const val NANOS_PER_MILLI = 1_000_000L
    }
}
