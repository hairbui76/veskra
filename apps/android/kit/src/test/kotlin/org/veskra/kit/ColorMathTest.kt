package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import kotlin.math.abs

class ColorMathTest {
    private val samples = listOf(
        0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0x00FFFF, 0xFF00FF,
        0x808080, 0x423D3E, 0xD03A2F, 0x2F80C0, 0x1A2B3C, 0xF5E1B9, 0x010203, 0xFEFDFC,
    ).map(::HexColor)

    @Test
    fun `hsb round trips every sample exactly`() {
        for (c in samples) assertEquals(c, ColorMath.fromHsb(ColorMath.toHsb(c)), c.toHex())
    }

    @Test
    fun `hsb matches known values`() {
        assertEquals(Hsb(0f, 1f, 1f), ColorMath.toHsb(HexColor(0xFF0000)))
        assertEquals(Hsb(120f, 1f, 1f), ColorMath.toHsb(HexColor(0x00FF00)))
        assertEquals(Hsb(240f, 1f, 1f), ColorMath.toHsb(HexColor(0x0000FF)))
        val grey = ColorMath.toHsb(HexColor(0x808080))
        assertEquals(0f, grey.s)
        assertEquals(HexColor(0xFF0000), ColorMath.fromHsb(360f, 1f, 1f))
        assertEquals(HexColor(0xFF0000), ColorMath.fromHsb(-360f, 1f, 1f))
        assertEquals(HexColor.BLACK, ColorMath.fromHsb(Float.NaN, Float.NaN, Float.NaN))
    }

    @Test
    fun `lab matches reference values and round trips within one step`() {
        val white = ColorMath.toLab(HexColor.WHITE)
        assertTrue(abs(white.l - 100f) < 0.05f && abs(white.a) < 0.05f && abs(white.b) < 0.05f, white.toString())
        val black = ColorMath.toLab(HexColor.BLACK)
        assertTrue(abs(black.l) < 0.01f, black.toString())
        // sRGB red in D50 Lab (ICC/Photoshop convention): L 54.29, a 80.81, b 69.89.
        val red = ColorMath.toLab(HexColor(0xFF0000))
        assertTrue(abs(red.l - 54.29f) < 0.1f && abs(red.a - 80.81f) < 0.2f && abs(red.b - 69.89f) < 0.2f, red.toString())
        for (c in samples) assertTrue(ColorMath.near(c, ColorMath.fromLab(ColorMath.toLab(c))), c.toHex())
        // Out-of-gamut Lab clips instead of wrapping or throwing.
        ColorMath.fromLab(Lab(50f, 127f, -128f))
        ColorMath.fromLab(Lab(200f, 0f, 0f))
    }

    @Test
    fun `cmyk round trips and handles black`() {
        assertEquals(Cmyk(0f, 0f, 0f, 1f), ColorMath.toCmyk(HexColor.BLACK))
        assertEquals(Cmyk(0f, 0f, 0f, 0f), ColorMath.toCmyk(HexColor.WHITE))
        assertEquals(Cmyk(0f, 1f, 1f, 0f), ColorMath.toCmyk(HexColor(0xFF0000)))
        for (c in samples) assertTrue(ColorMath.near(c, ColorMath.fromCmyk(ColorMath.toCmyk(c))), c.toHex())
        assertEquals(HexColor.BLACK, ColorMath.fromCmyk(Cmyk(0.3f, 0.2f, 0.9f, 1f)))
    }

    @Test
    fun `mix interpolates and clamps`() {
        assertEquals(HexColor(0x808080), ColorMath.mix(HexColor.BLACK, HexColor.WHITE, 0.5f))
        assertEquals(HexColor.BLACK, ColorMath.mix(HexColor.BLACK, HexColor.WHITE, -1f))
        assertEquals(HexColor.WHITE, ColorMath.mix(HexColor.BLACK, HexColor.WHITE, 2f))
    }

    @Test
    fun `built in palettes are well formed`() {
        for (p in Palette.BUILT_IN) {
            assertTrue(p.builtIn && p.colors.size >= 20, p.name)
            assertEquals(p.colors.size, p.colors.toSet().size, "${p.name} has duplicate swatches")
        }
        val user = Palette.user(List(100) { HexColor(it) })
        assertEquals(Palette.MAX_USER_COLORS, user.colors.size)
        assertTrue(!user.builtIn)
    }
}
