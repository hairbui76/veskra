package org.veskra.kit

/** Integer rectangle, half-open: covers x in [left, right), y in [top, bottom). Document pixels. */
data class RectI(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    val width: Int get() = right - left
    val height: Int get() = bottom - top
    val isEmpty: Boolean get() = right <= left || bottom <= top

    fun intersect(o: RectI): RectI? {
        val l = maxOf(left, o.left)
        val t = maxOf(top, o.top)
        val r = minOf(right, o.right)
        val b = minOf(bottom, o.bottom)
        return if (r > l && b > t) RectI(l, t, r, b) else null
    }

    companion object {
        fun ofSize(width: Int, height: Int): RectI = RectI(0, 0, width, height)
    }
}

/**
 * The 9 floats written by `nativeGetView`: doc→view affine `a,b,c,d,e,f` (x' = a·x + c·y + e,
 * y' = b·x + d·y + f — the SVG/kurbo column convention), zoom, rotation (radians), mirror.
 */
data class ViewState(
    val a: Float, val b: Float, val c: Float, val d: Float, val e: Float, val f: Float,
    val zoom: Float, val rotation: Float, val mirror: Boolean,
) {
    /**
     * Row-major 3×3 values in `android.graphics.Matrix.setValues` order:
     * `[MSCALE_X, MSKEW_X, MTRANS_X, MSKEW_Y, MSCALE_Y, MTRANS_Y, PERSP_0, PERSP_1, PERSP_2]`.
     */
    fun toAndroidMatrixValues(out: FloatArray = FloatArray(9)): FloatArray {
        require(out.size >= 9)
        out[0] = a; out[1] = c; out[2] = e
        out[3] = b; out[4] = d; out[5] = f
        out[6] = 0f; out[7] = 0f; out[8] = 1f
        return out
    }

    fun docToView(x: Float, y: Float): Pair<Float, Float> = Pair(a * x + c * y + e, b * x + d * y + f)

    companion object {
        val IDENTITY: ViewState = ViewState(1f, 0f, 0f, 1f, 0f, 0f, 1f, 0f, false)

        /** Parses the `nativeGetView` output; returns null if any value is non-finite or too short. */
        fun fromFloats(v: FloatArray): ViewState? {
            if (v.size < NativeBridge.VIEW_FLOATS) return null
            for (i in 0 until NativeBridge.VIEW_FLOATS) if (!v[i].isFinite()) return null
            return ViewState(v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8] != 0f)
        }
    }
}
