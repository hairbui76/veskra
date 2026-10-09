package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Assumptions.assumeTrue
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import java.io.File
import java.nio.ByteBuffer
import java.nio.file.Files

/**
 * Real JNI round trip against a host build of `crates/veskra-ffi` (cdylib for the JVM host, e.g.
 * `target/debug/libveskra_ffi.so`). Runs ONLY when `-Pveskra.ffi.lib=/abs/path/libveskra_ffi.so`
 * points to an existing file; otherwise every test is reported SKIPPED (never PASSED).
 */
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class NativeBridgeIntegrationTest {
    private val libPath: String? = System.getProperty("veskra.ffi.lib")?.takeIf { it.isNotBlank() }

    @BeforeAll
    fun loadLibrary() {
        assumeTrue(libPath != null && File(libPath).isFile, "veskra.ffi.lib not set or missing: native integration skipped")
        NativeBridge.load(File(libPath!!).absolutePath)
    }

    private fun exec(h: Long, cmd: JsonObject): JsonValue {
        val r = EngineResult.parse(NativeBridge.nativeExecute(h, cmd.encode()))
        return when (r) {
            is EngineResult.Ok -> r.value
            is EngineResult.Err -> throw AssertionError("engine error for ${cmd.encode()}: ${r.error}")
        }
    }

    private fun render(h: Long): ByteArray {
        val buf = ByteBuffer.allocateDirect(512 * 512 * 4)
        assertTrue(NativeBridge.nativeRenderRegion(h, 0, 0, 512, 512, buf), "nativeRenderRegion returned false")
        val out = ByteArray(buf.capacity())
        buf.rewind()
        buf.get(out)
        return out
    }

    @Test
    fun `session lifecycle stroke render undo save`() {
        assertTrue(NativeBridge.isLoaded)
        val version = NativeBridge.nativeVersion()
        assertTrue(version.startsWith("veskra-ffi ") && version.contains(" api="), "version string: $version")

        val h = NativeBridge.nativeCreateSession()
        assertNotEquals(0L, h, "nativeCreateSession returned 0")
        try {
            exec(h, Commands.documentCreate(512, 512, 300, "#FFFFFF"))
            exec(h, Commands.layerAdd(LayerKind.RASTER, "Ink"))
            exec(h, Commands.brushSelect(BrushPreset.ROUND_HARD.id))
            exec(h, Commands.colorSet(HexColor.BLACK))
            exec(h, Commands.brushSet(size = 12f, opacity = 1f))
            NativeBridge.nativeSetViewport(h, 512, 512)

            val before = render(h)

            // Synthetic stylus stroke across the middle of the view (view coords), 4 ms apart.
            val p = PointerPacker()
            val t0 = 1_000_000_000L
            val n = 40
            for (i in 0..n) {
                val action = when (i) {
                    0 -> MotionConstants.ACTION_DOWN
                    n -> MotionConstants.ACTION_UP
                    else -> MotionConstants.ACTION_MOVE
                }
                val flags = if (i in 1 until n && i % 4 != 0) PointerFlags.HISTORICAL else 0
                p.add(
                    action, 0, MotionConstants.TOOL_TYPE_STYLUS, 0, flags,
                    96f + i * 8f, 256f + (i % 5), 0.4f + i * 0.01f, 0.2f, 0f, Float.NaN, Float.NaN,
                    t0 + i * 4_000_000L,
                )
            }
            val res = InputBatchResult(NativeBridge.nativeInputBatch(h, p.ints, p.floats, p.longs, p.count))
            assertTrue(!res.malformed, "batch result malformed")
            assertTrue(res.canvasDirty, "stroke did not dirty the canvas")
            assertTrue(res.strokeCommitted, "ACTION_UP did not commit the stroke")

            val view = FloatArray(NativeBridge.VIEW_FLOATS)
            NativeBridge.nativeGetView(h, view)
            assertTrue(ViewState.fromFloats(view) != null, "non-finite view: ${view.toList()}")

            val after = render(h)
            val changed = before.indices.count { before[it] != after[it] }
            assertTrue(changed > 0, "rendered pixels did not change after the stroke")

            exec(h, Commands.historyUndo())
            val undone = render(h)
            assertTrue(undone.contentEquals(before), "undo did not restore the pre-stroke composite")

            val tmp = Files.createTempDirectory("veskra-it").toFile()
            val path = File(tmp, "it.veskra").absolutePath
            exec(h, Commands.documentSave(path))
            assertTrue(File(path).isFile && File(path).length() > 0, "save produced no file")
        } finally {
            NativeBridge.nativeDestroySession(h)
            NativeBridge.nativeDestroySession(h) // contract: idempotent, must not crash
        }
        assertEquals(true, NativeBridge.isLoaded)
    }
}
