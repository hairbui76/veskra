package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Assumptions.assumeTrue
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import java.io.File
import java.nio.file.Files

/**
 * Contract test: every [Commands] builder and every [BrushPreset] id is accepted by the real Rust
 * engine through JNI. Guards the Kotlin ↔ engine command vocabulary (docs/COMMANDS.md).
 * Runs only with `-Dveskra.ffi.lib=...`; otherwise SKIPPED (never PASSED).
 */
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class CommandVocabularyIntegrationTest {
    private val libPath: String? = System.getProperty("veskra.ffi.lib")?.takeIf { it.isNotBlank() }

    @BeforeAll
    fun loadLibrary() {
        assumeTrue(libPath != null && File(libPath).isFile, "veskra.ffi.lib not set or missing: native integration skipped")
        NativeBridge.load(File(libPath!!).absolutePath)
    }

    private fun exec(h: Long, cmd: JsonObject): JsonValue =
        when (val r = EngineResult.parse(NativeBridge.nativeExecute(h, cmd.encode()))) {
            is EngineResult.Ok -> r.value
            is EngineResult.Err -> throw AssertionError("engine rejected ${cmd.encode()}: ${r.error}")
        }

    @Test
    fun `every Kotlin command builder is understood by the engine`() {
        val h = NativeBridge.nativeCreateSession()
        val dir = Files.createTempDirectory("veskra-vocab").toFile()
        try {
            exec(h, Commands.documentCreate(256, 192, 300, "#FFFFFF"))
            val info = DocumentInfo.fromJson(exec(h, Commands.documentInfo()) as JsonObject)
            assertEquals(256, info?.width)
            assertEquals(300, info?.dpi)
            exec(h, Commands.layerAdd(LayerKind.VECTOR, "Ink"))
            exec(h, Commands.layerAdd(LayerKind.GROUP, "Group"))
            val layers = LayerInfo.listFromJson(exec(h, Commands.layerList()) as JsonObject)!!
            assertEquals(listOf("Group", "Ink", "Layer 1", "Background"), layers.map { it.name })
            val ink = layers.first { it.name == "Ink" }
            assertTrue(ink.vectorEligible, "empty vector layer is exactly vector-representable")
            val bg = layers.first { it.name == "Background" }
            exec(h, Commands.layerSelect(ink.id))
            exec(h, Commands.layerRename(ink.id, "Line art"))
            exec(h, Commands.layerSet(ink.id, visible = true, opacity = 0.8f, blend = BlendMode.MULTIPLY, alphaLock = true, clip = false, locked = false))
            val after = LayerInfo.listFromJson(exec(h, Commands.layerList()) as JsonObject)!!.first { it.id == ink.id }
            assertEquals("Line art", after.name)
            assertEquals(BlendMode.MULTIPLY, after.blend)
            assertTrue(after.alphaLock)
            exec(h, Commands.layerMoveBelow(ink.id, bg.id))
            exec(h, Commands.layerMoveAbove(ink.id, bg.id))
            exec(h, Commands.layerDuplicate(ink.id))
            for (p in BrushPreset.entries) exec(h, Commands.brushSelect(p.id))
            exec(h, Commands.brushSet(size = 10f, opacity = 0.9f, flow = 1f, stabilization = 0.2f))
            exec(h, Commands.colorSet(HexColor.BLACK))
            exec(h, Commands.viewFit())
            exec(h, Commands.viewMirror(true))
            exec(h, Commands.viewMirror(false))
            exec(h, Commands.inputReset())
            exec(h, Commands.historyUndo())
            exec(h, Commands.historyRedo())
            for (f in ExportFormat.entries) {
                exec(h, Commands.exportPreflight(f, ExportOptions()))
            }
            val svg = File(dir, "out.svg")
            exec(h, Commands.exportRun(ExportFormat.SVG, svg.absolutePath, ExportOptions()))
            assertTrue(svg.length() > 0)
            val project = File(dir, "doc.veskra")
            exec(h, Commands.documentSave(project.absolutePath))
            exec(h, Commands.documentValidate(project.absolutePath))
            exec(h, Commands.documentOpen(project.absolutePath))
            val auto = File(dir, "autosave")
            exec(h, Commands.documentAutosave(auto.absolutePath))
            exec(h, Commands.documentRecover(auto.absolutePath))
            exec(h, Commands.layerRemove(LayerInfo.listFromJson(exec(h, Commands.layerList()) as JsonObject)!!.first().id))
        } finally {
            NativeBridge.nativeDestroySession(h)
            dir.deleteRecursively()
        }
    }
}
