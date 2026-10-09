package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertInstanceOf
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.nio.ByteBuffer

/** Fake JNI bridge recording calls; responses are scripted per command name. */
private class FakeBridge(var nextHandle: Long = 42L) : EngineBridge {
    val executed = mutableListOf<String>()
    val destroyed = mutableListOf<Long>()
    val responses = mutableMapOf<String, String>()
    var batchReturn: IntArray? = intArrayOf(0, 0)
    var lastBatchCount = -1
    var viewport: Pair<Int, Int>? = null
    var view = floatArrayOf(2f, 0f, 0f, 2f, 10f, 20f, 2f, 0f, 0f)
    var throwOnExecute = false

    override fun version() = "veskra-ffi 0.0.0 api=1"
    override fun createSession() = nextHandle
    override fun destroySession(h: Long) { destroyed += h }
    override fun execute(h: Long, json: String): String {
        if (throwOnExecute) throw IllegalStateException("boom")
        executed += json
        val cmd = Json.parseObject(json).string("cmd")!!
        return responses[cmd] ?: """{"ok":{}}"""
    }
    override fun inputBatch(h: Long, ints: IntArray, floats: FloatArray, longs: LongArray, count: Int): IntArray? {
        lastBatchCount = count
        return batchReturn
    }
    override fun getView(h: Long, out: FloatArray) { view.copyInto(out) }
    override fun setViewport(h: Long, w: Int, hgt: Int) { viewport = w to hgt }
    override fun renderRegion(h: Long, x: Int, y: Int, w: Int, hgt: Int, dst: ByteBuffer) = true
}

class EngineSessionTest {
    @Test
    fun `open fails loudly on a zero handle`() {
        val e = assertThrows(EngineException::class.java) { EngineSession.open(FakeBridge(nextHandle = 0L)) }
        assertEquals("Internal", e.error.code)
    }

    @Test
    fun `commands are sent as documented JSON`() {
        val b = FakeBridge()
        EngineSession.open(b).use { s ->
            assertInstanceOf(EngineResult.Ok::class.java, s.createDocument(2048, 2048, 300, HexColor.WHITE))
            s.run(Commands.layerAdd(LayerKind.RASTER, "Ink"))
            s.undo()
            s.run(Commands.brushSelect("pencil.rough"))
            s.run(Commands.colorSet(HexColor.parse("#1a2b3c")!!))
            s.exportPreflight(ExportFormat.SVG, ExportOptions())
            s.run(Commands.documentSave("/app-private/tmp/x.veskra"))
            s.run(Commands.layerSet("L1", opacity = 0.5f, blend = BlendMode.MULTIPLY))
        }
        assertEquals(
            listOf(
                """{"cmd":"document.create","width":2048,"height":2048,"dpi":300,"background":"#FFFFFF"}""",
                """{"cmd":"layer.add","kind":"raster","name":"Ink"}""",
                """{"cmd":"history.undo"}""",
                """{"cmd":"brush.select","preset":"pencil.rough"}""",
                """{"cmd":"color.set","hex":"#1A2B3C"}""",
                """{"cmd":"export.preflight","format":"svg","options":{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true}}""",
                """{"cmd":"document.save","path":"/app-private/tmp/x.veskra"}""",
                """{"cmd":"layer.set","id":"L1","opacity":0.5,"blend":"multiply"}""",
            ),
            b.executed,
        )
    }

    @Test
    fun `invalid canvas never reaches the engine`() {
        val b = FakeBridge()
        EngineSession.open(b).use { s ->
            val r = s.createDocument(20_000, 10, 300, null)
            assertInstanceOf(EngineResult.Err::class.java, r)
        }
        assertTrue(b.executed.isEmpty())
    }

    @Test
    fun `typed errors and malformed responses are mapped`() {
        val b = FakeBridge()
        b.responses["history.undo"] = """{"error":{"code":"NothingToUndo","message":"nothing to undo","recoverable":true}}"""
        b.responses["document.info"] = "not json"
        b.responses["layer.list"] = """{"neither":1}"""
        EngineSession.open(b).use { s ->
            val undo = s.undo() as EngineResult.Err
            assertEquals(EngineError("NothingToUndo", "nothing to undo", true), undo.error)
            assertEquals("Internal", (s.documentInfo() as EngineResult.Err).error.code)
            assertEquals("Internal", (s.layers() as EngineResult.Err).error.code)
            b.throwOnExecute = true
            assertEquals("Internal", (s.undo() as EngineResult.Err).error.code)
        }
    }

    @Test
    fun `parses document info layers preflight and report`() {
        val b = FakeBridge()
        b.responses["document.info"] = """{"ok":{"width":512,"height":256,"dpi":300,"version":7,"profile":"sRGB IEC61966-2.1","activeLayer":"L2","canUndo":true,"canRedo":false}}"""
        b.responses["layer.list"] = """{"ok":{"layers":[{"id":"L2","name":"Ink","kind":"vector","visible":true,"opacity":0.8,"blend":"multiply","alphaLocked":false,"clip":true,"locked":false,"depth":0,"vectorFidelity":"exact"},{"id":"L1","name":"Paper","kind":"raster"}]}}"""
        b.responses["export.preflight"] = """{"ok":{"format":"svg","options":{},"blockedReason":null,"vectorPathCount":12,"rasterOnlyStrokeCount":3,"rasterizedLayerCount":1,"profile":"sRGB","widthPx":512,"heightPx":256,"estimatedBytes":1000,"warnings":[{"kind":"raster_only_stroke","message":"3 textured strokes","count":3}]}}"""
        b.responses["export.run"] = """{"ok":{"success":true,"fileUri":null,"format":"pdf","profile":"sRGB","profileEmbedded":true,"vectorObjectCount":12,"rasterizedObjectCount":1,"flattenedFeatureList":["clip"],"warnings":[],"durationMs":5,"fileBytes":999,"widthPx":512,"heightPx":256,"validation":"structurally_validated"}}"""
        EngineSession.open(b).use { s ->
            val info = s.documentInfo().getOrThrow()
            assertEquals(DocumentInfo(512, 256, 300, 7, "sRGB IEC61966-2.1", "L2", true, false), info)
            val layers = s.layers().getOrThrow()
            assertEquals(2, layers.size)
            assertEquals(LayerKind.VECTOR, layers[0].kind)
            assertEquals(BlendMode.MULTIPLY, layers[0].blend)
            assertEquals(0.8f, layers[0].opacity)
            assertTrue(layers[0].clip)
            assertEquals(BlendMode.NORMAL, layers[1].blend)
            val pf = s.exportPreflight(ExportFormat.SVG, ExportOptions()).getOrThrow()
            assertEquals(12, pf.vectorPathCount)
            assertEquals(1, pf.warnings.size)
            val rep = s.exportRun(ExportFormat.PDF, "/tmp/x.pdf", ExportOptions()).getOrThrow()
            assertTrue(rep.isTrustworthySuccess)
            assertEquals(listOf("clip"), rep.flattenedFeatureList)
        }
    }

    @Test
    fun `input batch result decoding`() {
        val b = FakeBridge()
        b.batchReturn = intArrayOf(0b1111, 2, 0, 0, 10, 10, 600, 600, 700, 700)
        EngineSession.open(b).use { s ->
            val p = PointerPacker()
            assertFalse(s.inputBatch(p).canvasDirty, "empty packer is not sent")
            assertEquals(-1, b.lastBatchCount)
            p.add(PointerSample(action = 0, pointerId = 0, toolType = 2, x = 1f, y = 1f, eventTimeNanos = 1))
            val r = s.inputBatch(p)
            assertEquals(1, b.lastBatchCount)
            assertTrue(r.canvasDirty && r.viewChanged && r.strokeCommitted && r.historyChanged)
            assertEquals(listOf(RectI(0, 0, 10, 10), RectI(600, 600, 700, 700)), r.rects)
            assertFalse(r.fullRefresh)
        }
    }

    @Test
    fun `malformed batch results force a full refresh`() {
        for (bad in listOf(null, intArrayOf(), intArrayOf(1), intArrayOf(1, 2, 0, 0, 1, 1), intArrayOf(1, -1), intArrayOf(1, 0, 5))) {
            val r = InputBatchResult(bad)
            assertTrue(r.malformed && r.fullRefresh, "bad=${bad?.toList()}")
        }
        assertTrue(InputBatchResult(intArrayOf(1, 0)).fullRefresh, "dirty without rects")
        assertFalse(InputBatchResult(intArrayOf(0, 0)).fullRefresh)
    }

    @Test
    fun `view parsing and android matrix mapping`() {
        val b = FakeBridge()
        EngineSession.open(b).use { s ->
            s.setViewport(1280, 800)
            assertEquals(1280 to 800, b.viewport)
            val v = requireNotNull(s.view())
            val m = v.toAndroidMatrixValues()
            assertEquals(listOf(2f, 0f, 10f, 0f, 2f, 20f, 0f, 0f, 1f), m.toList())
            assertEquals(12f to 24f, v.docToView(1f, 2f))
            b.view = floatArrayOf(1f, 0f, 0f, 1f, Float.NaN, 0f, 1f, 0f, 0f)
            assertEquals(null, s.view())
        }
    }

    @Test
    fun `render region validates buffers before native code`() {
        EngineSession.open(FakeBridge()).use { s ->
            assertThrows(IllegalArgumentException::class.java) { s.renderRegion(RectI(0, 0, 4, 4), ByteBuffer.allocate(64)) }
            assertThrows(IllegalArgumentException::class.java) { s.renderRegion(RectI(0, 0, 4, 4), ByteBuffer.allocateDirect(63)) }
            assertThrows(IllegalArgumentException::class.java) { s.renderRegion(RectI(0, 0, 0, 4), ByteBuffer.allocateDirect(64)) }
            assertTrue(s.renderRegion(RectI(0, 0, 4, 4), ByteBuffer.allocateDirect(64)))
        }
    }

    @Test
    fun `close is idempotent and use after close fails`() {
        val b = FakeBridge()
        val s = EngineSession.open(b)
        s.close()
        s.close()
        assertEquals(listOf(42L), b.destroyed)
        assertTrue(s.isClosed)
        assertThrows(IllegalStateException::class.java) { s.undo() }
    }

    @Test
    fun `thread confinement is enforced when requested`() {
        val s = EngineSession.open(FakeBridge(), confineToCreatingThread = true)
        var error: Throwable? = null
        val t = Thread { error = runCatching { s.undo() }.exceptionOrNull() }
        t.start(); t.join()
        assertInstanceOf(IllegalStateException::class.java, error)
        s.close()
    }
}
