package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

class JsonEditTest {
    private val preset = Json.parseObject(
        """{"id":"b1","size_px":12,"tip":{"shape":"round","angle":0},"future_field":{"keep":true},"dynamics":{"pressure":{"size":{"enabled":true,"curve":[[0,0],[1,1]]}}}}""",
    )

    @Test
    fun `reads nested values`() {
        assertEquals(12f, preset.number("size_px"))
        assertEquals("round", preset.text("tip", "shape"))
        assertEquals(true, preset.flag("dynamics", "pressure", "size", "enabled"))
        assertEquals(listOf(PointF(0f, 0f), PointF(1f, 1f)), preset.points("dynamics", "pressure", "size", "curve"))
        assertNull(preset.number("tip", "missing"))
        assertNull(preset.points("tip"))
    }

    @Test
    fun `updates copy on write and keep unknown fields`() {
        val edited = preset.with(30f, "tip", "angle").with(0.5f, "texture", "depth").with(null as String?, "tip", "image")
        assertEquals(0f, preset.number("tip", "angle"), "original untouched")
        assertEquals(30f, edited.number("tip", "angle"))
        assertEquals(0.5f, edited.number("texture", "depth"), "missing object created")
        assertEquals(JsonNull, edited.at("tip", "image"))
        assertEquals(preset.at("future_field"), edited.at("future_field"))
        val curve = listOf(PointF(0f, 0.2f), PointF(0.5f, 0.9f), PointF(1f, 1f))
        assertEquals(curve, edited.withPoints(curve, "dynamics", "velocity", "size", "curve").points("dynamics", "velocity", "size", "curve"))
    }
}
