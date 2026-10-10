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

/** A point in document or view pixels. */
data class PointF(val x: Float, val y: Float) {
    operator fun plus(o: PointF): PointF = PointF(x + o.x, y + o.y)
    operator fun minus(o: PointF): PointF = PointF(x - o.x, y - o.y)
    operator fun times(k: Float): PointF = PointF(x * k, y * k)
    fun distanceTo(o: PointF): Float = kotlin.math.hypot(x - o.x, y - o.y)
}

/** How a new selection shape combines with the existing selection (`op` wire values). */
enum class SelectionOp(val wire: String) {
    REPLACE("replace"), ADD("add"), SUBTRACT("subtract"), INTERSECT("intersect")
}

/**
 * 2D affine in the engine's column convention: x' = a·x + c·y + e, y' = b·x + d·y + f
 * (`edit.transform` matrix `[a,b,c,d,e,f]`, same layout as [ViewState]).
 */
data class Affine(val a: Double, val b: Double, val c: Double, val d: Double, val e: Double, val f: Double) {
    fun apply(p: PointF): PointF = PointF((a * p.x + c * p.y + e).toFloat(), (b * p.x + d * p.y + f).toFloat())

    /** The transform that applies `this` first, then [next]. */
    fun then(next: Affine): Affine = Affine(
        next.a * a + next.c * b,
        next.b * a + next.d * b,
        next.a * c + next.c * d,
        next.b * c + next.d * d,
        next.a * e + next.c * f + next.e,
        next.b * e + next.d * f + next.f,
    )

    /** Null when the matrix is singular (cannot be undone geometrically). */
    fun inverse(): Affine? {
        val det = a * d - b * c
        if (!det.isFinite() || kotlin.math.abs(det) < 1e-12) return null
        val ia = d / det
        val ib = -b / det
        val ic = -c / det
        val id = a / det
        return Affine(ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f))
    }

    val isIdentity: Boolean get() = this == IDENTITY

    fun toList(): List<Double> = listOf(a, b, c, d, e, f)

    companion object {
        val IDENTITY: Affine = Affine(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)

        fun translate(dx: Double, dy: Double): Affine = Affine(1.0, 0.0, 0.0, 1.0, dx, dy)

        /** Scale by ([sx], [sy]) about the fixed point ([cx], [cy]). */
        fun scaleAbout(sx: Double, sy: Double, cx: Double, cy: Double): Affine = Affine(sx, 0.0, 0.0, sy, cx - sx * cx, cy - sy * cy)

        /** Rotate by [radians] (clockwise on screen, +y down) about ([cx], [cy]). */
        fun rotateAbout(radians: Double, cx: Double, cy: Double): Affine {
            val cs = kotlin.math.cos(radians)
            val sn = kotlin.math.sin(radians)
            return Affine(cs, sn, -sn, cs, cx - cs * cx + sn * cy, cy - sn * cx - cs * cy)
        }
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

    fun docToView(p: PointF): PointF = PointF(a * p.x + c * p.y + e, b * p.x + d * p.y + f)

    /** Inverse mapping for tool gestures; null if the view matrix is singular. */
    fun viewToDoc(p: PointF): PointF? {
        val det = a * d - b * c
        if (!det.isFinite() || kotlin.math.abs(det) < 1e-9f) return null
        val x = p.x - e
        val y = p.y - f
        return PointF((d * x - c * y) / det, (a * y - b * x) / det)
    }

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
