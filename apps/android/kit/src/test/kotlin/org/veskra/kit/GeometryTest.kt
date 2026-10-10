package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import kotlin.math.PI
import kotlin.math.abs

class GeometryTest {
    private fun assertNear(expected: PointF, actual: PointF, eps: Float = 1e-3f) =
        assertTrue(abs(expected.x - actual.x) <= eps && abs(expected.y - actual.y) <= eps, "expected $expected but was $actual")

    @Test
    fun `affine follows the engine column convention`() {
        // x' = a·x + c·y + e, y' = b·x + d·y + f
        val m = Affine(2.0, 3.0, 5.0, 7.0, 11.0, 13.0)
        assertNear(PointF(2f * 1 + 5f * 1 + 11f, 3f * 1 + 7f * 1 + 13f), m.apply(PointF(1f, 1f)))
        assertEquals(listOf(2.0, 3.0, 5.0, 7.0, 11.0, 13.0), m.toList())
        assertTrue(Affine.IDENTITY.isIdentity)
    }

    @Test
    fun `then applies this first and the argument second`() {
        val move = Affine.translate(10.0, 0.0)
        val grow = Affine.scaleAbout(2.0, 2.0, 0.0, 0.0)
        assertNear(PointF(22f, 2f), move.then(grow).apply(PointF(1f, 1f)))
        assertNear(PointF(12f, 2f), grow.then(move).apply(PointF(1f, 1f)))
    }

    @Test
    fun `scale and rotate keep their fixed point`() {
        val pivot = PointF(30f, -7f)
        assertNear(pivot, Affine.scaleAbout(3.0, 0.5, 30.0, -7.0).apply(pivot))
        assertNear(pivot, Affine.rotateAbout(1.234, 30.0, -7.0).apply(pivot))
        // Positive angles turn clockwise on screen (+y down): +x goes to +y after a quarter turn.
        assertNear(PointF(0f, 1f), Affine.rotateAbout(PI / 2, 0.0, 0.0).apply(PointF(1f, 0f)))
    }

    @Test
    fun `inverse undoes the transform and rejects singular matrices`() {
        val m = Affine.rotateAbout(0.7, 5.0, 9.0).then(Affine.scaleAbout(2.0, 3.0, 1.0, 1.0)).then(Affine.translate(-4.0, 8.0))
        val inv = m.inverse()!!
        val p = PointF(12.5f, -3.25f)
        assertNear(p, inv.apply(m.apply(p)))
        assertNull(Affine(1.0, 2.0, 2.0, 4.0, 0.0, 0.0).inverse())
        assertNull(Affine(Double.NaN, 0.0, 0.0, 1.0, 0.0, 0.0).inverse())
    }

    @Test
    fun `view maps document points to the screen and back`() {
        // zoom 2, quarter turn clockwise, pan (100, 50)
        val v = ViewState(a = 0f, b = 2f, c = -2f, d = 0f, e = 100f, f = 50f, zoom = 2f, rotation = (PI / 2).toFloat(), mirror = false)
        val doc = PointF(10f, 20f)
        val screen = v.docToView(doc)
        assertNear(PointF(60f, 70f), screen)
        assertNear(doc, v.viewToDoc(screen)!!)
        assertNull(ViewState(0f, 0f, 0f, 0f, 0f, 0f, 1f, 0f, false).viewToDoc(screen))
    }

    @Test
    fun `tool command builders reject values the engine would refuse`() {
        assertThrows(IllegalArgumentException::class.java) { Commands.selectionRect(Float.NaN, 0f, 1f, 1f) }
        assertThrows(IllegalArgumentException::class.java) { Commands.selectionLasso(listOf(PointF(0f, 0f), PointF(1f, 1f))) }
        assertThrows(IllegalArgumentException::class.java) { Commands.editFill(0f, 0f, tolerance = 1.5f) }
        assertThrows(IllegalArgumentException::class.java) { Commands.viewSet(zoom = Float.POSITIVE_INFINITY) }
        assertEquals("""{"cmd":"selection.rect","x0":1,"y0":2,"x1":3,"y1":4,"op":"add"}""", Commands.selectionRect(1f, 2f, 3f, 4f, SelectionOp.ADD).encode())
        assertEquals("""{"cmd":"view.set","zoom":2}""", Commands.viewSet(zoom = 2f).encode())
        assertEquals(
            """{"cmd":"selection.lasso","points":[[0,0],[4,0],[0,4]],"op":"replace"}""",
            Commands.selectionLasso(listOf(PointF(0f, 0f), PointF(4f, 0f), PointF(0f, 4f))).encode(),
        )
    }
}
