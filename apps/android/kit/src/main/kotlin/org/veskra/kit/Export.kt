package org.veskra.kit

/**
 * Export formats (mirrors `veskra_core::export::ExportFormat`, serialized snake_case).
 * There is intentionally NO `.ai` format: native Illustrator export is unsupported
 * (MASTER_PLAN §7.5, AGENTS.md §3). Illustrator handoff is SVG or PDF.
 */
enum class ExportFormat(val wire: String, val extension: String, val mimeType: String) {
    PNG("png", "png", "image/png"),
    PSD("psd", "psd", "image/vnd.adobe.photoshop"),
    SVG("svg", "svg", "image/svg+xml"),
    PDF("pdf", "pdf", "application/pdf"),
    ;

    /** Whether the vector/raster toggles apply (MASTER_PLAN §2.3). */
    val isIllustratorHandoff: Boolean get() = this == SVG || this == PDF

    companion object {
        fun fromWire(s: String): ExportFormat? = entries.firstOrNull { it.wire == s }
    }
}

/**
 * Color space the exported pixels are converted to (`options.profile`). [DOCUMENT] writes the
 * canvas's own profile unconverted; the others convert (relative colorimetric, clipping what the
 * target cannot hold) and embed the matching ICC profile.
 */
enum class ExportProfile(val wire: String) {
    DOCUMENT("document"), SRGB("srgb"), DISPLAY_P3("display_p3"), ADOBE_RGB("adobe_rgb"),

    /** Print: converted through a CMYK output profile (bundled Coated FOGRA39 or the printer's own). */
    CMYK("cmyk"),
    ;

    /**
     * Wide-gamut numbers are only meaningful with their profile attached, so the engine refuses to
     * write them untagged.
     */
    val requiresEmbeddedProfile: Boolean get() = this == DISPLAY_P3 || this == ADOBE_RGB

    companion object {
        fun fromWire(s: String?): ExportProfile? = entries.firstOrNull { it.wire == s }
    }
}

/** How colors outside the target gamut are brought in (`renderingIntent`). */
enum class RenderingIntent(val wire: String) {
    /** Keeps in-gamut colors exact and clips the rest to the nearest printable color. */
    RELATIVE_COLORIMETRIC("relative_colorimetric"),

    /** Compresses the whole gamut so relationships between colors survive; everything shifts a little. */
    PERCEPTUAL("perceptual"),
    ;

    companion object {
        fun fromWire(s: String?): RenderingIntent? = entries.firstOrNull { it.wire == s }
    }
}

/**
 * Export options; JSON names match `veskra_core::export::ExportOptions` (camelCase): the three
 * independent toggles of MASTER_PLAN §2.3 plus the output color space, bit depth and, for print,
 * the CMYK profile, rendering intent and black point compensation.
 */
data class ExportOptions(
    val includeVectorPaths: Boolean = true,
    val includeRasterAppearance: Boolean = true,
    val embedColorProfile: Boolean = true,
    val profile: ExportProfile = ExportProfile.DOCUMENT,
    /** Bits per channel, 8 or 16. Only PNG and PSD can carry 16. */
    val bitDepth: Int = 8,
    /** Absolute path of an imported CMYK output profile; null = the bundled Coated FOGRA39. */
    val cmykProfile: String? = null,
    val renderingIntent: RenderingIntent = RenderingIntent.RELATIVE_COLORIMETRIC,
    val blackPointCompensation: Boolean = true,
) {
    init {
        require(bitDepth == 8 || bitDepth == 16) { "bitDepth must be 8 or 16" }
    }

    /**
     * For SVG/PDF at least one of vector/raster must be selected (ILL-004); a wide-gamut target
     * needs its profile embedded.
     */
    fun isValidFor(format: ExportFormat): Boolean =
        (!format.isIllustratorHandoff || includeVectorPaths || includeRasterAppearance) &&
            (embedColorProfile || !profile.requiresEmbeddedProfile) &&
            (profile != ExportProfile.CMYK || format.supportsCmyk)

    fun toJson(): JsonObject = json {
        "includeVectorPaths" to includeVectorPaths
        "includeRasterAppearance" to includeRasterAppearance
        "embedColorProfile" to embedColorProfile
        "profile" to profile.wire
        "bitDepth" to bitDepth
        "cmykProfile" to cmykProfile
        "renderingIntent" to renderingIntent.wire
        "blackPointCompensation" to blackPointCompensation
    }

    companion object {
        /** Settings stored before the color options existed load with their defaults. */
        fun fromJson(o: JsonObject?): ExportOptions? {
            if (o == null) return null
            return ExportOptions(
                includeVectorPaths = o.bool("includeVectorPaths") ?: return null,
                includeRasterAppearance = o.bool("includeRasterAppearance") ?: return null,
                embedColorProfile = o.bool("embedColorProfile") ?: return null,
                profile = ExportProfile.fromWire(o.string("profile")) ?: ExportProfile.DOCUMENT,
                bitDepth = o.int("bitDepth")?.takeIf { it == 8 || it == 16 } ?: 8,
                cmykProfile = o.string("cmykProfile"),
                renderingIntent = RenderingIntent.fromWire(o.string("renderingIntent")) ?: RenderingIntent.RELATIVE_COLORIMETRIC,
                blackPointCompensation = o.bool("blackPointCompensation") ?: true,
            )
        }
    }
}

/** Formats whose pixels can be written at 16 bits per channel. */
val ExportFormat.supports16Bit: Boolean get() = this == ExportFormat.PNG || this == ExportFormat.PSD

/** PNG and SVG have no CMYK; PSD (CMYK mode) and PDF (CMYK color spaces) do. */
val ExportFormat.supportsCmyk: Boolean get() = this == ExportFormat.PSD || this == ExportFormat.PDF

data class ExportWarningInfo(val kind: String, val message: String, val count: Int)

/** Engine preflight (mirrors `veskra_core::export::ExportPreflight`). */
data class ExportPreflightInfo(
    val format: ExportFormat,
    val blockedReason: String?,
    val vectorPathCount: Int,
    val rasterOnlyStrokeCount: Int,
    val rasterizedLayerCount: Int,
    val profile: String,
    val widthPx: Int,
    val heightPx: Int,
    val estimatedBytes: Long,
    val warnings: List<ExportWarningInfo>,
    /** Rendering intent of the conversion to the target profile; null when nothing is converted. */
    val renderingIntent: String? = null,
    /** The target gamut is smaller than the canvas's in places: colors there will be clipped. */
    val gamutClippingPossible: Boolean = false,
    val bitDepth: Int = 8,
    /** Highest total ink (C+M+Y+K, percent) in the CMYK result; null when not exporting CMYK. */
    val totalAreaCoverageMax: Double? = null,
) {
    companion object {
        fun fromJson(o: JsonObject): ExportPreflightInfo? = ExportPreflightInfo(
            format = o.string("format")?.let(ExportFormat::fromWire) ?: return null,
            blockedReason = o.string("blockedReason"),
            vectorPathCount = o.int("vectorPathCount") ?: return null,
            rasterOnlyStrokeCount = o.int("rasterOnlyStrokeCount") ?: return null,
            rasterizedLayerCount = o.int("rasterizedLayerCount") ?: return null,
            profile = o.string("profile") ?: return null,
            widthPx = o.int("widthPx") ?: 0,
            heightPx = o.int("heightPx") ?: 0,
            estimatedBytes = o.long("estimatedBytes") ?: 0L,
            warnings = parseWarnings(o.array("warnings")),
            renderingIntent = o.string("renderingIntent"),
            gamutClippingPossible = o.bool("gamutClippingPossible") ?: false,
            bitDepth = o.int("bitDepth") ?: 8,
            totalAreaCoverageMax = o.double("totalAreaCoverageMax"),
        )
    }
}

/** What was actually written (mirrors `veskra_core::export::ExportReport`, MASTER_PLAN §7.6). */
data class ExportReportInfo(
    val success: Boolean,
    val format: ExportFormat,
    val profile: String,
    val profileEmbedded: Boolean,
    val vectorObjectCount: Int,
    val rasterizedObjectCount: Int,
    val flattenedFeatureList: List<String>,
    val warnings: List<ExportWarningInfo>,
    val durationMs: Long,
    val fileBytes: Long,
    /** `structurally_validated` | `not_validated` | `failed`. */
    val validation: String,
    /** Pixels that had to be clipped into the target gamut (0 when nothing was converted). */
    val outOfGamutPixels: Long = 0,
    val bitDepth: Int = 8,
    val totalAreaCoverageMax: Double? = null,
) {
    /** A file may be reported as successfully exported only if the engine says so AND it validated. */
    val isTrustworthySuccess: Boolean get() = success && validation == "structurally_validated"

    companion object {
        fun fromJson(o: JsonObject): ExportReportInfo? = ExportReportInfo(
            success = o.bool("success") ?: return null,
            format = o.string("format")?.let(ExportFormat::fromWire) ?: return null,
            profile = o.string("profile") ?: "",
            profileEmbedded = o.bool("profileEmbedded") ?: false,
            vectorObjectCount = o.int("vectorObjectCount") ?: 0,
            rasterizedObjectCount = o.int("rasterizedObjectCount") ?: 0,
            flattenedFeatureList = o.array("flattenedFeatureList")?.items?.mapNotNull { (it as? JsonString)?.value }.orEmpty(),
            warnings = parseWarnings(o.array("warnings")),
            durationMs = o.long("durationMs") ?: 0L,
            fileBytes = o.long("fileBytes") ?: 0L,
            validation = o.string("validation") ?: "failed",
            outOfGamutPixels = o.long("outOfGamutPixels") ?: 0L,
            bitDepth = o.int("bitDepth") ?: 8,
            totalAreaCoverageMax = o.double("totalAreaCoverageMax"),
        )
    }
}

private fun parseWarnings(a: JsonArray?): List<ExportWarningInfo> =
    a?.items?.mapNotNull { w ->
        (w as? JsonObject)?.let {
            ExportWarningInfo(it.string("kind") ?: "other", it.string("message") ?: "", it.int("count") ?: 0)
        }
    }.orEmpty()
