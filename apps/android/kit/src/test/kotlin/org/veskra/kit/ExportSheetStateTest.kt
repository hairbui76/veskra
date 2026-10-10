package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertInstanceOf
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertSame
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.io.File

class ExportSheetStateTest {
    @Test
    fun `color space and bit depth options follow the format and keep the profile attached`() {
        var s = ExportSheetState.initial(ExportDestination.PHOTOSHOP)
        assertEquals(ExportProfile.DOCUMENT, s.options.profile)
        assertEquals(8, s.options.bitDepth)
        assertTrue(s.bitDepthEnabled)

        // A wide-gamut target switches embedding on and locks it: untagged P3 numbers would be wrong everywhere.
        s = s.reduce(ExportSheetEvent.SetEmbedColorProfile(false)).reduce(ExportSheetEvent.SetProfile(ExportProfile.DISPLAY_P3))
        assertTrue(s.options.embedColorProfile)
        assertTrue(s.embedProfileLocked)
        assertTrue(s.reduce(ExportSheetEvent.SetEmbedColorProfile(false)).options.embedColorProfile)
        assertFalse(ExportOptions(embedColorProfile = false, profile = ExportProfile.ADOBE_RGB).isValidFor(ExportFormat.PSD))
        s = s.reduce(ExportSheetEvent.SetProfile(ExportProfile.SRGB))
        assertFalse(s.embedProfileLocked)
        assertFalse(s.reduce(ExportSheetEvent.SetEmbedColorProfile(false)).options.embedColorProfile)

        s = s.reduce(ExportSheetEvent.SetBitDepth(16))
        assertEquals(16, s.effectiveOptions.bitDepth)
        assertEquals(s.options, s.reduce(ExportSheetEvent.SetBitDepth(12)).options, "only 8 and 16 exist")
        assertEquals(
            """{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"profile":"srgb","bitDepth":16}""",
            s.effectiveOptions.toJson().encode(),
        )

        // SVG/PDF are written at 8 bits: the choice is neither offered nor sent for them.
        var svg = ExportSheetState(ExportDestination.ILLUSTRATOR, ExportFormat.SVG, s.options)
        assertFalse(svg.bitDepthEnabled)
        assertEquals(8, svg.effectiveOptions.bitDepth)
        svg = svg.reduce(ExportSheetEvent.SetBitDepth(8))
        assertEquals(16, svg.options.bitDepth, "remembered for the formats that support it")

        // Preferences written before these options existed still load, with the defaults.
        val old = Json.parseObject("""{"includeVectorPaths":true,"includeRasterAppearance":false,"embedColorProfile":true}""")
        assertEquals(ExportOptions(includeRasterAppearance = false), ExportOptions.fromJson(old))
        assertEquals(s.options, ExportOptions.fromJson(s.options.toJson()))
    }

    private fun preflight(format: ExportFormat, blocked: String? = null) = ExportPreflightInfo(
        format = format, blockedReason = blocked, vectorPathCount = 12, rasterOnlyStrokeCount = 3,
        rasterizedLayerCount = 1, profile = "sRGB IEC61966-2.1", widthPx = 100, heightPx = 100,
        estimatedBytes = 1, warnings = emptyList(),
    )

    private fun report(success: Boolean = true, validation: String = "structurally_validated") = ExportReportInfo(
        success, ExportFormat.SVG, "sRGB", true, 1, 0, emptyList(), emptyList(), 1, 1, validation,
    )

    private fun ready(s: ExportSheetState): ExportSheetState =
        s.reduce(ExportSheetEvent.PreflightStarted(s.preflightKey))
            .reduce(ExportSheetEvent.PreflightLoaded(s.preflightKey, preflight(s.format)))

    @Test
    fun `defaults are all three toggles on with SVG for Illustrator`() {
        val s = ExportSheetState.initial(ExportDestination.ILLUSTRATOR)
        assertEquals(ExportFormat.SVG, s.format)
        assertEquals(ExportOptions(includeVectorPaths = true, includeRasterAppearance = true, embedColorProfile = true), s.options)
        assertTrue(s.vectorRasterTogglesEnabled)
        assertEquals("Export for Illustrator (SVG/PDF)", s.title)
        assertTrue(s.needsPreflight)
        assertFalse(s.canExport, "no export before preflight")
    }

    @Test
    fun `toggles are independent`() {
        var s = ExportSheetState.initial(ExportDestination.ILLUSTRATOR)
        s = s.reduce(ExportSheetEvent.SetIncludeVectorPaths(false))
        assertEquals(ExportOptions(false, true, true), s.options)
        s = s.reduce(ExportSheetEvent.SetEmbedColorProfile(false))
        assertEquals(ExportOptions(false, true, false), s.options)
        s = s.reduce(ExportSheetEvent.SetIncludeVectorPaths(true)).reduce(ExportSheetEvent.SetIncludeRasterAppearance(false))
        assertEquals(ExportOptions(true, false, false), s.options)
        assertTrue(s.isValid)
    }

    @Test
    fun `both vector and raster off blocks SVG and PDF with an actionable message`() {
        for (f in listOf(ExportFormat.SVG, ExportFormat.PDF)) {
            var s = ready(ExportSheetState.initial(ExportDestination.ILLUSTRATOR).reduce(ExportSheetEvent.SelectFormat(f)))
            assertTrue(s.canExport)
            s = s.reduce(ExportSheetEvent.SetIncludeVectorPaths(false)).reduce(ExportSheetEvent.SetIncludeRasterAppearance(false))
            assertFalse(s.isValid)
            assertFalse(s.canExport)
            assertFalse(s.needsPreflight, "no preflight for an invalid selection")
            assertEquals(ExportCopy.BOTH_OFF_MESSAGE, s.blockingMessage)
            assertTrue(s.blockingMessage!!.contains("Editable vector strokes") && s.blockingMessage!!.contains("Preserve original appearance"))
            assertSame(s, s.reduce(ExportSheetEvent.ExportStarted), "export start ignored while invalid")
        }
    }

    @Test
    fun `vector and raster toggles only apply to SVG and PDF`() {
        for (d in listOf(ExportDestination.IMAGE, ExportDestination.PHOTOSHOP)) {
            val s = ExportSheetState.initial(d)
            assertFalse(s.vectorRasterTogglesEnabled)
            assertSame(s, s.reduce(ExportSheetEvent.SetIncludeVectorPaths(false)))
            assertSame(s, s.reduce(ExportSheetEvent.SetIncludeRasterAppearance(false)))
            assertFalse(s.reduce(ExportSheetEvent.SetEmbedColorProfile(false)).options.embedColorProfile, "ICC toggle applies to every format")
            assertTrue(s.isValid)
            assertEquals(ExportOptions(embedColorProfile = true), s.effectiveOptions)
        }
        // A PNG never accepts an Illustrator format.
        val png = ExportSheetState.initial(ExportDestination.IMAGE)
        assertSame(png, png.reduce(ExportSheetEvent.SelectFormat(ExportFormat.SVG)))
    }

    @Test
    fun `preflight results for stale inputs are ignored and changes require a new preflight`() {
        var s = ExportSheetState.initial(ExportDestination.ILLUSTRATOR)
        val oldKey = s.preflightKey
        s = s.reduce(ExportSheetEvent.PreflightStarted(oldKey))
        s = s.reduce(ExportSheetEvent.SetEmbedColorProfile(false))
        assertTrue(s.needsPreflight)
        s = s.reduce(ExportSheetEvent.PreflightLoaded(oldKey, preflight(ExportFormat.SVG)))
        assertFalse(s.canExport, "stale preflight must not enable export")
        assertTrue(s.summaryLines.isEmpty())
        s = ready(s)
        assertTrue(s.canExport)
        s = s.reduce(ExportSheetEvent.SelectFormat(ExportFormat.PDF))
        assertFalse(s.canExport)
        assertTrue(s.needsPreflight)
    }

    @Test
    fun `summary lines show counts and profile`() {
        val s = ready(ExportSheetState.initial(ExportDestination.ILLUSTRATOR))
        assertEquals(listOf("12 vector paths", "3 raster-only strokes", "1 rasterized layer", "profile: sRGB IEC61966-2.1"), s.summaryLines)
        assertEquals("1 vector path", ExportCopy.vectorPaths(1))
        assertEquals("0 raster-only strokes", ExportCopy.rasterOnlyStrokes(0))
        assertEquals("2 rasterized layers", ExportCopy.rasterizedLayers(2))
    }

    @Test
    fun `engine blocked reason blocks export`() {
        var s = ExportSheetState.initial(ExportDestination.PHOTOSHOP)
        s = s.reduce(ExportSheetEvent.PreflightLoaded(s.preflightKey, preflight(ExportFormat.PSD, blocked = "Document too large for PSD")))
        assertFalse(s.canExport)
        assertEquals("Document too large for PSD", s.blockingMessage)
    }

    @Test
    fun `success is only reported for a validated report`() {
        val base = ready(ExportSheetState.initial(ExportDestination.ILLUSTRATOR)).reduce(ExportSheetEvent.ExportStarted)
        assertInstanceOf(ExportPhase.Exporting::class.java, base.phase)
        assertFalse(base.canExport, "no double export")
        assertSame(base, base.reduce(ExportSheetEvent.SetIncludeVectorPaths(false)), "options frozen while exporting")
        assertInstanceOf(ExportPhase.Succeeded::class.java, base.reduce(ExportSheetEvent.ExportFinished(report())).phase)
        val unvalidated = base.reduce(ExportSheetEvent.ExportFinished(report(validation = "not_validated"))).phase
        assertEquals(ExportPhase.Failed(ExportCopy.NOT_VERIFIED_MESSAGE), unvalidated)
        assertInstanceOf(ExportPhase.Failed::class.java, base.reduce(ExportSheetEvent.ExportFinished(report(success = false))).phase)
        val failed = base.reduce(ExportSheetEvent.ExportFailed("Provider write failed"))
        assertEquals(ExportPhase.Failed("Provider write failed"), failed.phase)
        assertEquals(ExportPhase.Editing, failed.reduce(ExportSheetEvent.Acknowledge).phase)
    }

    @Test
    fun `preferences are remembered per destination and survive serialization`() {
        val illus = DestinationPrefs(ExportFormat.PDF, ExportOptions(includeVectorPaths = true, includeRasterAppearance = false, embedColorProfile = false))
        val psd = DestinationPrefs(ExportFormat.PSD, ExportOptions(embedColorProfile = false))
        val prefs = ExportPreferences.DEFAULT.with(ExportDestination.ILLUSTRATOR, illus).with(ExportDestination.PHOTOSHOP, psd)
        val restored = ExportPreferences.fromJsonText(prefs.toJson().encode())
        assertEquals(prefs, restored)
        assertEquals(illus, restored[ExportDestination.ILLUSTRATOR])
        assertEquals(DestinationPrefs.defaultFor(ExportDestination.IMAGE), restored[ExportDestination.IMAGE])
        val s = ExportSheetState.initial(ExportDestination.ILLUSTRATOR, restored)
        assertEquals(ExportFormat.PDF, s.format)
        assertFalse(s.options.includeRasterAppearance)
        // Switching destination loads that destination's memory.
        val switched = s.reduce(ExportSheetEvent.SelectDestination(ExportDestination.PHOTOSHOP, restored))
        assertEquals(psd, switched.prefsToRemember())
    }

    @Test
    fun `corrupt or invalid stored preferences fall back to defaults`() {
        assertEquals(ExportPreferences.DEFAULT, ExportPreferences.fromJsonText(null))
        assertEquals(ExportPreferences.DEFAULT, ExportPreferences.fromJsonText("{garbage"))
        assertEquals(ExportPreferences.DEFAULT, ExportPreferences.fromJsonText("""{"version":99,"destinations":{}}"""))
        val invalid = """{"version":1,"destinations":{"illustrator":{"format":"svg","options":{"includeVectorPaths":false,"includeRasterAppearance":false,"embedColorProfile":true}},"photoshop":{"format":"ai","options":{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true}}}}"""
        val p = ExportPreferences.fromJsonText(invalid)
        assertEquals(DestinationPrefs.defaultFor(ExportDestination.ILLUSTRATOR), p[ExportDestination.ILLUSTRATOR])
        assertEquals(DestinationPrefs.defaultFor(ExportDestination.PHOTOSHOP), p[ExportDestination.PHOTOSHOP])
    }

    @Test
    fun `file names are sanitized and use only the real extension`() {
        assertEquals("My sketch.svg", ExportFileNames.suggest("My sketch", ExportFormat.SVG))
        assertEquals("a_b_c.pdf", ExportFileNames.suggest("a/b\\c", ExportFormat.PDF))
        assertEquals("Untitled.png", ExportFileNames.suggest("...", ExportFormat.PNG))
        assertEquals("Untitled.psd", ExportFileNames.suggest(null, ExportFormat.PSD))
        assertEquals("evil_ai.svg", ExportFileNames.suggest("evil.ai", ExportFormat.SVG))
        assertEquals("x.veskra", ExportFileNames.suggestProject("x"))
    }

    // ---- Wording audit: no native .ai claim anywhere user-facing (AGENTS.md §3). ----

    private val forbidden = listOf(
        Regex("""\.ai\b""", RegexOption.IGNORE_CASE),
        Regex("""\bAI native\b""", RegexOption.IGNORE_CASE),
        Regex("""\bnative (Adobe )?Illustrator\b""", RegexOption.IGNORE_CASE),
        Regex("""\bIllustrator (document|file|project) export\b""", RegexOption.IGNORE_CASE),
        Regex("""no colou?r shift""", RegexOption.IGNORE_CASE),
    )

    @Test
    fun `no ai extension or native ai wording in kit labels or extensions`() {
        for (f in ExportFormat.entries) {
            assertTrue(f.extension != "ai" && !f.mimeType.contains("illustrator"), "$f")
        }
        val all = ExportCopy.allFixedStrings() + ExportFormat.entries.map { "." + it.extension } +
            ExportDestination.entries.flatMap { d -> d.formats.map { ExportFileNames.suggest("x", it) } }
        for (s in all) for (re in forbidden) assertNull(re.find(s), "forbidden wording '${re.pattern}' in \"$s\"")
    }

    @Test
    fun `android string resources contain no ai extension or native ai wording`() {
        val path = System.getProperty("veskra.app.strings")
        val file = File(requireNotNull(path) { "veskra.app.strings system property not set by Gradle" })
        assertTrue(file.isFile, "strings.xml missing at $file")
        val text = file.readText()
        for (re in forbidden) assertNull(re.find(text), "forbidden wording '${re.pattern}' in strings.xml")
        // The sheet title and actionable message must ship as resources too.
        assertTrue(text.contains(ExportCopy.ILLUSTRATOR_TITLE), "strings.xml lacks the Illustrator export title")
        assertTrue(text.contains("Editable vector strokes") && text.contains("Preserve original appearance"))
    }
}
