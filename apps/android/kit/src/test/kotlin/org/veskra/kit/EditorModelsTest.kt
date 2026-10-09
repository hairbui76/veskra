package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class EditorModelsTest {
    @Test
    fun `hex parsing accepts canonical and short forms only`() {
        assertEquals(HexColor(0x1A2B3C), HexColor.parse("#1a2b3c"))
        assertEquals(HexColor(0x1A2B3C), HexColor.parse(" 1A2B3C "))
        assertEquals(HexColor(0xAABBCC), HexColor.parse("#abc"))
        for (bad in listOf("", "#", "#12", "#12345", "#1234567", "#GGGGGG", "red", "#-12345")) {
            assertNull(HexColor.parse(bad), bad)
        }
        assertEquals("#1A2B3C", HexColor(0x1A2B3C).toHex())
        assertEquals(0xFF1A2B3C.toInt(), HexColor(0x1A2B3C).toArgb())
        assertEquals(HexColor(0x010203), HexColor.of(1, 2, 3))
        assertThrows(IllegalArgumentException::class.java) { HexColor(0x1000000) }
    }

    @Test
    fun `recent colors are bounded mru without duplicates`() {
        val r = RecentColors(capacity = 3)
        listOf(1, 2, 3, 2, 4).forEach { r.push(HexColor(it)) }
        assertEquals(listOf(4, 2, 3), r.colors.map { it.rgb })
        val restored = RecentColors(3, r.colors)
        assertEquals(r.colors, restored.colors)
    }

    @Test
    fun `brush presets cover the planned set`() {
        assertEquals(
            listOf(
                "pencil.rough", "pencil.smooth", "pen.technical", "pen.brush", "marker", "round.hard",
                "round.soft", "charcoal", "eraser.hard", "eraser.soft",
            ),
            BrushPreset.entries.map { it.id },
        )
        assertTrue(BrushPreset.ERASER_SOFT.isEraser)
        assertEquals(BrushPreset.MARKER, BrushPreset.fromId("marker"))
        assertNull(BrushPreset.fromId("smudge"))
    }

    @Test
    fun `canvas validation mirrors engine limits`() {
        assertNull(CanvasSpec.PRESET_SQUARE.validate())
        assertNull(CanvasSpec.PRESET_LANDSCAPE.validate())
        assertNull(CanvasSpec(8192, 8192, 300).validate())
        assertEquals(CanvasSpec.Problem.NON_POSITIVE, CanvasSpec(0, 10, 300).validate())
        assertEquals(CanvasSpec.Problem.EDGE_TOO_LARGE, CanvasSpec(16_385, 10, 300).validate())
        assertEquals(CanvasSpec.Problem.TOO_MANY_PIXELS, CanvasSpec(16_384, 8192, 300).validate())
        assertEquals(CanvasSpec.Problem.DPI_OUT_OF_RANGE, CanvasSpec(100, 100, 0).validate())
        assertEquals("sRGB", CanvasSpec.COLOR_SPACE_BADGE)
    }

    @Test
    fun `memory estimate is an upper bound in bytes`() {
        // 2048² px × (8 B × 4 layers + 4 B display) = 4,194,304 × 36.
        assertEquals(4_194_304L * 36, CanvasSpec.PRESET_SQUARE.estimatedBytes())
        assertEquals("144 MB", formatBytes(CanvasSpec.PRESET_SQUARE.estimatedBytes()))
        assertEquals("512 B", formatBytes(512))
        assertEquals("2 KB", formatBytes(2048))
        assertEquals("1.5 GB", formatBytes(1024L * 1024 * 1024 * 3 / 2))
    }
}
