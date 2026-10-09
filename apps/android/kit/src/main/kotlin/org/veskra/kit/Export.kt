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
 * The three independent export toggles (MASTER_PLAN §2.3); JSON names match
 * `veskra_core::export::ExportOptions` (camelCase).
 */
data class ExportOptions(
    val includeVectorPaths: Boolean = true,
    val includeRasterAppearance: Boolean = true,
    val embedColorProfile: Boolean = true,
) {
    /** For SVG/PDF at least one of vector/raster must be selected (ILL-004). */
    fun isValidFor(format: ExportFormat): Boolean =
        !format.isIllustratorHandoff || includeVectorPaths || includeRasterAppearance

    fun toJson(): JsonObject = json {
        "includeVectorPaths" to includeVectorPaths
        "includeRasterAppearance" to includeRasterAppearance
        "embedColorProfile" to embedColorProfile
    }

    companion object {
        fun fromJson(o: JsonObject?): ExportOptions? {
            if (o == null) return null
            return ExportOptions(
                includeVectorPaths = o.bool("includeVectorPaths") ?: return null,
                includeRasterAppearance = o.bool("includeRasterAppearance") ?: return null,
                embedColorProfile = o.bool("embedColorProfile") ?: return null,
            )
        }
    }
}

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
        )
    }
}

private fun parseWarnings(a: JsonArray?): List<ExportWarningInfo> =
    a?.items?.mapNotNull { w ->
        (w as? JsonObject)?.let {
            ExportWarningInfo(it.string("kind") ?: "other", it.string("message") ?: "", it.int("count") ?: 0)
        }
    }.orEmpty()
