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
            val png = File(System.getProperty("veskra.repo.root") ?: "../../..", "tests/fixtures/import/untagged_rgb8.png")
            assertTrue(png.isFile, "import fixture not found at ${png.absolutePath}")
            val imp = exec(h, Commands.layerImportImage(png.absolutePath)) as JsonObject
            assertEquals(true, imp.bool("profileAssumed"))
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

    @Test
    fun `tool command builders are understood by the engine`() {
        val h = NativeBridge.nativeCreateSession()
        try {
            exec(h, Commands.documentCreate(256, 192, 300, "#FFFFFF"))
            val layer = LayerInfo.listFromJson(exec(h, Commands.layerList()) as JsonObject)!!.first { it.name == "Layer 1" }
            exec(h, Commands.colorSet(HexColor(0xD03A2F)))

            // Selection shapes and combination ops, then the read-only outline used for marching ants.
            exec(h, Commands.selectionRect(10f, 10f, 100f, 80f))
            exec(h, Commands.selectionEllipse(60f, 40f, 160f, 120f, SelectionOp.ADD))
            exec(h, Commands.selectionLasso(listOf(PointF(0f, 0f), PointF(40f, 0f), PointF(0f, 40f)), SelectionOp.SUBTRACT))
            for (op in SelectionOp.entries) exec(h, Commands.selectionRect(20f, 20f, 90f, 70f, op))
            exec(h, Commands.selectionRect(20f, 20f, 90f, 70f)) // the ops above end on an empty selection
            assertEquals(true, DocumentInfo.fromJson(exec(h, Commands.documentInfo()) as JsonObject)?.hasSelection)
            val outline = exec(h, Commands.selectionOutline()) as JsonObject
            assertEquals(false, outline.bool("empty"))
            assertEquals(4, outline.array("bounds")?.size)
            assertTrue((outline.array("contours")?.size ?: 0) >= 1)

            exec(h, Commands.editFillSelection())
            assertEquals(4, (exec(h, Commands.layerBounds()) as JsonObject).array("bounds")?.size)
            assertEquals("#D03A2F", (exec(h, Commands.colorPick(50f, 50f)) as JsonObject).string("color"))
            exec(h, Commands.editTransform(Affine.translate(12.0, -4.0)))
            exec(h, Commands.editDeleteSelection())
            exec(h, Commands.selectionInvert())
            exec(h, Commands.selectionAll())
            exec(h, Commands.selectionClear())
            exec(h, Commands.editFill(5f, 5f, tolerance = 0.2f, contiguous = false, sampleMerged = true))

            exec(h, Commands.selectionRect(0f, 0f, 128f, 96f))
            exec(h, Commands.layerMaskFromSelection(layer.id))
            assertTrue(LayerInfo.listFromJson(exec(h, Commands.layerList()) as JsonObject)!!.first { it.id == layer.id }.hasMask)
            exec(h, Commands.layerMaskApply(layer.id))
            exec(h, Commands.layerMaskFromSelection(layer.id))
            exec(h, Commands.layerMaskRemove(layer.id))
            exec(h, Commands.layerClear(layer.id))

            val png = File(System.getProperty("veskra.repo.root") ?: "../../..", "tests/fixtures/import/untagged_rgb8.png")
            exec(h, Commands.layerImportImageAt(png.absolutePath, "Photo", 16, 8))
            assertEquals("Photo", LayerInfo.listFromJson(exec(h, Commands.layerList()) as JsonObject)!!.first().name)
            exec(h, Commands.viewSet(zoom = 2f, rotation = 0.5f, panX = 10f, panY = -20f))
            exec(h, Commands.viewSet())
        } finally {
            NativeBridge.nativeDestroySession(h)
        }
    }

    @Test
    fun `raster edit, canvas and export color builders are understood by the engine`() {
        val h = NativeBridge.nativeCreateSession()
        val dir = Files.createTempDirectory("veskra-vocab-raster").toFile()
        try {
            exec(h, Commands.documentCreate(256, 192, 300, "#FFFFFF"))
            exec(h, Commands.layerAdd(LayerKind.RASTER, "Paint"))
            val red = HexColor(0xD03A2F)
            exec(h, Commands.editGradient(PointF(0f, 0f), PointF(200f, 0f), radial = false, stops = listOf(0f to red, 1f to HexColor.WHITE)))
            exec(h, Commands.editGradient(PointF(100f, 90f), PointF(160f, 90f), radial = true, stops = listOf(0f to HexColor.BLACK, 1f to red)))
            exec(h, Commands.selectionWand(100f, 90f, tolerance = 0.3f, contiguous = true, sampleMerged = false, op = SelectionOp.REPLACE))
            assertEquals(true, DocumentInfo.fromJson(exec(h, Commands.documentInfo()) as JsonObject)?.hasSelection)
            exec(h, Commands.selectionClear())

            exec(h, Commands.editAdjustHsb(hue = 30f, saturation = -0.2f, brightness = 0f))
            exec(h, Commands.editAdjustBrightnessContrast(brightness = 0.1f, contrast = 0.25f))
            exec(h, Commands.editAdjustCurves(mapOf("rgb" to listOf(PointF(0f, 0f), PointF(0.5f, 0.6f), PointF(1f, 1f)), "b" to listOf(PointF(0f, 0.1f), PointF(1f, 0.9f)))))
            exec(h, Commands.editAdjustInvert())
            exec(h, Commands.editTransformQuad(listOf(PointF(40f, 10f), PointF(200f, 20f), PointF(220f, 180f), PointF(10f, 170f))))

            exec(h, Commands.canvasCrop(16, 8, 208, 152))
            var info = DocumentInfo.fromJson(exec(h, Commands.documentInfo()) as JsonObject)!!
            assertEquals(192 to 144, info.width to info.height)
            exec(h, Commands.canvasResize(384, 288, resample = true))
            exec(h, Commands.canvasResize(400, 300, resample = false))
            info = DocumentInfo.fromJson(exec(h, Commands.documentInfo()) as JsonObject)!!
            assertEquals(400 to 300, info.width to info.height)
            exec(h, Commands.historyUndo())
            assertEquals(384, DocumentInfo.fromJson(exec(h, Commands.documentInfo()) as JsonObject)!!.width)

            exec(h, Commands.inputConfigLockView(true))
            exec(h, Commands.inputConfigLockView(false))

            // Every target profile at both bit depths; 16-bit PSD in Display P3 is the demanding case.
            for (profile in ExportProfile.entries) {
                for (bits in listOf(8, 16)) {
                    val pf = ExportPreflightInfo.fromJson(exec(h, Commands.exportPreflight(ExportFormat.PNG, ExportOptions(profile = profile, bitDepth = bits)) ) as JsonObject)!!
                    assertEquals(bits, pf.bitDepth)
                }
            }
            val psd = File(dir, "p3-16.psd")
            val report = ExportReportInfo.fromJson(
                exec(h, Commands.exportRun(ExportFormat.PSD, psd.absolutePath, ExportOptions(profile = ExportProfile.DISPLAY_P3, bitDepth = 16))) as JsonObject,
            )!!
            assertTrue(report.isTrustworthySuccess, "16-bit Display P3 PSD must validate")
            assertEquals(16, report.bitDepth)
            assertTrue(report.profileEmbedded)
            assertTrue(psd.length() > 0)
        } finally {
            NativeBridge.nativeDestroySession(h)
            dir.deleteRecursively()
        }
    }
}
