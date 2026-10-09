package org.veskra.kit

/**
 * Export destinations offered by the Export sheet (MASTER_PLAN §2.1 item 7, §2.3). Preferences are
 * remembered per destination; the Illustrator destination offers SVG or PDF — never `.ai`.
 */
enum class ExportDestination(val wire: String, val formats: List<ExportFormat>) {
    IMAGE("image", listOf(ExportFormat.PNG)),
    PHOTOSHOP("photoshop", listOf(ExportFormat.PSD)),
    ILLUSTRATOR("illustrator", listOf(ExportFormat.SVG, ExportFormat.PDF)),
    ;

    val defaultFormat: ExportFormat get() = formats.first()

    companion object {
        fun fromWire(s: String?): ExportDestination? = entries.firstOrNull { it.wire == s }
    }
}

/** Typed remembered settings for one destination. */
data class DestinationPrefs(val format: ExportFormat, val options: ExportOptions) {
    companion object {
        fun defaultFor(d: ExportDestination): DestinationPrefs = DestinationPrefs(d.defaultFormat, ExportOptions())
    }
}

/**
 * Per-destination remembered export preferences (MASTER_PLAN §1.1: "remember chosen settings by
 * destination without silently changing output semantics"). Serialized with the :kit JSON codec;
 * corrupt, unknown or invalid stored entries fall back to that destination's defaults.
 */
class ExportPreferences private constructor(private val byDestination: Map<ExportDestination, DestinationPrefs>) {
    operator fun get(d: ExportDestination): DestinationPrefs = byDestination[d] ?: DestinationPrefs.defaultFor(d)

    fun with(d: ExportDestination, prefs: DestinationPrefs): ExportPreferences {
        require(prefs.format in d.formats) { "${prefs.format} is not offered for $d" }
        require(prefs.options.isValidFor(prefs.format)) { "invalid options for ${prefs.format}" }
        return ExportPreferences(byDestination + (d to prefs))
    }

    fun toJson(): JsonObject = json {
        "version" to SCHEMA_VERSION
        "destinations" to JsonObject(
            LinkedHashMap<String, JsonValue>().apply {
                for (d in ExportDestination.entries) {
                    val p = byDestination[d] ?: continue
                    put(d.wire, json { "format" to p.format.wire; "options" to p.options.toJson() })
                }
            },
        )
    }

    override fun equals(other: Any?): Boolean = other is ExportPreferences &&
        ExportDestination.entries.all { this[it] == other[it] }

    override fun hashCode(): Int = ExportDestination.entries.map { this[it] }.hashCode()

    companion object {
        const val SCHEMA_VERSION: Int = 1
        val DEFAULT: ExportPreferences = ExportPreferences(emptyMap())

        fun fromJsonText(text: String?): ExportPreferences {
            if (text.isNullOrBlank()) return DEFAULT
            val root = try {
                Json.parseObject(text)
            } catch (e: JsonParseException) {
                return DEFAULT
            }
            if (root.int("version") != SCHEMA_VERSION) return DEFAULT
            val dests = root.obj("destinations") ?: return DEFAULT
            val map = LinkedHashMap<ExportDestination, DestinationPrefs>()
            for ((key, value) in dests.fields) {
                val d = ExportDestination.fromWire(key) ?: continue
                val o = value as? JsonObject ?: continue
                val f = o.string("format")?.let(ExportFormat::fromWire)?.takeIf { it in d.formats } ?: continue
                val opts = ExportOptions.fromJson(o.obj("options"))?.takeIf { it.isValidFor(f) } ?: continue
                map[d] = DestinationPrefs(f, opts)
            }
            return ExportPreferences(map)
        }
    }
}

/** Identifies which inputs a preflight was computed for; stale results are discarded. */
data class PreflightKey(val format: ExportFormat, val options: ExportOptions)

sealed interface PreflightState {
    data object Idle : PreflightState
    data class Loading(val key: PreflightKey) : PreflightState
    data class Ready(val key: PreflightKey, val info: ExportPreflightInfo) : PreflightState
    data class Failed(val key: PreflightKey, val error: EngineError) : PreflightState
}

sealed interface ExportPhase {
    data object Editing : ExportPhase
    data class Exporting(val key: PreflightKey) : ExportPhase
    data class Succeeded(val report: ExportReportInfo) : ExportPhase
    data class Failed(val message: String) : ExportPhase
}

sealed interface ExportSheetEvent {
    data class SelectDestination(val destination: ExportDestination, val prefs: ExportPreferences) : ExportSheetEvent
    data class SelectFormat(val format: ExportFormat) : ExportSheetEvent
    data class SetIncludeVectorPaths(val value: Boolean) : ExportSheetEvent
    data class SetIncludeRasterAppearance(val value: Boolean) : ExportSheetEvent
    data class SetEmbedColorProfile(val value: Boolean) : ExportSheetEvent
    data class PreflightStarted(val key: PreflightKey) : ExportSheetEvent
    data class PreflightLoaded(val key: PreflightKey, val info: ExportPreflightInfo) : ExportSheetEvent
    data class PreflightFailed(val key: PreflightKey, val error: EngineError) : ExportSheetEvent
    data object ExportStarted : ExportSheetEvent
    data class ExportFinished(val report: ExportReportInfo) : ExportSheetEvent
    data class ExportFailed(val message: String) : ExportSheetEvent
    data object Acknowledge : ExportSheetEvent
}

/**
 * Pure state machine behind the Export sheet (MASTER_PLAN §2.3). The Compose sheet renders this and
 * sends [ExportSheetEvent]s; engine calls are made by the ViewModel using [preflightKey]/[effectiveOptions].
 *
 * Rules:
 * - Three independent toggles, default true. Vector/raster toggles apply only to SVG/PDF.
 * - SVG/PDF with both vector and raster off is INVALID: export blocked with [ExportCopy.BOTH_OFF_MESSAGE].
 * - Export requires a fresh preflight for the exact current format+options, not blocked by the engine.
 * - Success is shown only for a report that is successful AND structurally validated (§7.6).
 */
data class ExportSheetState(
    val destination: ExportDestination,
    val format: ExportFormat,
    val options: ExportOptions,
    val preflight: PreflightState = PreflightState.Idle,
    val phase: ExportPhase = ExportPhase.Editing,
) {
    init {
        require(format in destination.formats) { "$format not offered for $destination" }
    }

    /** Vector/raster checkboxes are shown/enabled only for SVG/PDF. */
    val vectorRasterTogglesEnabled: Boolean get() = format.isIllustratorHandoff

    val isValid: Boolean get() = options.isValidFor(format)

    /** Actionable reason export is blocked, or null. */
    val blockingMessage: String?
        get() = when {
            !isValid -> ExportCopy.BOTH_OFF_MESSAGE
            preflight is PreflightState.Ready && preflight.info.blockedReason != null -> preflight.info.blockedReason
            else -> null
        }

    /** Options sent to the engine: vector/raster flags are only meaningful for SVG/PDF. */
    val effectiveOptions: ExportOptions
        get() = if (format.isIllustratorHandoff) options else ExportOptions(embedColorProfile = options.embedColorProfile)

    val preflightKey: PreflightKey get() = PreflightKey(format, effectiveOptions)

    /** True when the UI should (re)request a preflight for the current inputs. */
    val needsPreflight: Boolean
        get() = isValid && when (val p = preflight) {
            PreflightState.Idle -> true
            is PreflightState.Loading -> p.key != preflightKey
            is PreflightState.Ready -> p.key != preflightKey
            is PreflightState.Failed -> p.key != preflightKey
        }

    val canExport: Boolean
        get() = phase !is ExportPhase.Exporting && blockingMessage == null &&
            preflight is PreflightState.Ready && preflight.key == preflightKey

    val title: String get() = ExportCopy.destinationTitle(destination)

    /** Count/summary lines shown before export (MASTER_PLAN §2.3), empty until preflight is ready. */
    val summaryLines: List<String>
        get() = (preflight as? PreflightState.Ready)?.takeIf { it.key == preflightKey }?.info?.let(ExportCopy::summaryLines).orEmpty()

    /** Settings to remember for this destination (persist when the user starts an export). */
    fun prefsToRemember(): DestinationPrefs = DestinationPrefs(format, options)

    fun reduce(e: ExportSheetEvent): ExportSheetState = when (e) {
        is ExportSheetEvent.SelectDestination -> if (phase is ExportPhase.Exporting) this else initial(e.destination, e.prefs)
        is ExportSheetEvent.SelectFormat ->
            if (e.format in destination.formats && phase !is ExportPhase.Exporting) copy(format = e.format, phase = ExportPhase.Editing) else this
        is ExportSheetEvent.SetIncludeVectorPaths ->
            if (vectorRasterTogglesEnabled && phase !is ExportPhase.Exporting) copy(options = options.copy(includeVectorPaths = e.value), phase = ExportPhase.Editing) else this
        is ExportSheetEvent.SetIncludeRasterAppearance ->
            if (vectorRasterTogglesEnabled && phase !is ExportPhase.Exporting) copy(options = options.copy(includeRasterAppearance = e.value), phase = ExportPhase.Editing) else this
        is ExportSheetEvent.SetEmbedColorProfile ->
            if (phase !is ExportPhase.Exporting) copy(options = options.copy(embedColorProfile = e.value), phase = ExportPhase.Editing) else this
        is ExportSheetEvent.PreflightStarted -> if (e.key == preflightKey) copy(preflight = PreflightState.Loading(e.key)) else this
        is ExportSheetEvent.PreflightLoaded -> if (e.key == preflightKey) copy(preflight = PreflightState.Ready(e.key, e.info)) else this
        is ExportSheetEvent.PreflightFailed -> if (e.key == preflightKey) copy(preflight = PreflightState.Failed(e.key, e.error)) else this
        ExportSheetEvent.ExportStarted -> if (canExport) copy(phase = ExportPhase.Exporting(preflightKey)) else this
        is ExportSheetEvent.ExportFinished -> when {
            phase !is ExportPhase.Exporting -> this
            e.report.isTrustworthySuccess -> copy(phase = ExportPhase.Succeeded(e.report))
            else -> copy(phase = ExportPhase.Failed(ExportCopy.NOT_VERIFIED_MESSAGE))
        }
        is ExportSheetEvent.ExportFailed -> if (phase is ExportPhase.Exporting) copy(phase = ExportPhase.Failed(e.message)) else this
        ExportSheetEvent.Acknowledge -> if (phase is ExportPhase.Exporting) this else copy(phase = ExportPhase.Editing)
    }

    companion object {
        fun initial(destination: ExportDestination, prefs: ExportPreferences = ExportPreferences.DEFAULT): ExportSheetState {
            val p = prefs[destination]
            return ExportSheetState(destination, p.format, p.options)
        }
    }
}

/**
 * Canonical ENGLISH copy for the export sheet. :app ships the same text as string resources
 * (localization keys in res/values/strings.xml); this object exists so wording rules are unit-tested
 * on the JVM. Never mention native `.ai` output (AGENTS.md §3).
 */
object ExportCopy {
    const val ILLUSTRATOR_TITLE: String = "Export for Illustrator (SVG/PDF)"
    const val PHOTOSHOP_TITLE: String = "Export for Photoshop (PSD)"
    const val IMAGE_TITLE: String = "Export image (PNG)"
    const val VECTOR_LABEL: String = "Editable vector strokes"
    const val RASTER_LABEL: String = "Preserve original appearance"
    const val PROFILE_LABEL: String = "Embed color profile"
    const val BOTH_OFF_MESSAGE: String =
        "Select “Editable vector strokes”, “Preserve original appearance”, or both to export SVG or PDF."
    const val NOT_VERIFIED_MESSAGE: String =
        "The exported file could not be verified, so it was not reported as saved. Try again or choose another location."
    const val ILLUSTRATOR_HINT: String =
        "Open the SVG or PDF in Illustrator and use Save As to keep an Illustrator document."

    fun destinationTitle(d: ExportDestination): String = when (d) {
        ExportDestination.IMAGE -> IMAGE_TITLE
        ExportDestination.PHOTOSHOP -> PHOTOSHOP_TITLE
        ExportDestination.ILLUSTRATOR -> ILLUSTRATOR_TITLE
    }

    fun formatLabel(f: ExportFormat): String = when (f) {
        ExportFormat.PNG -> "PNG"
        ExportFormat.PSD -> "PSD"
        ExportFormat.SVG -> "SVG"
        ExportFormat.PDF -> "PDF"
    }

    fun vectorPaths(n: Int): String = if (n == 1) "1 vector path" else "$n vector paths"
    fun rasterOnlyStrokes(n: Int): String = if (n == 1) "1 raster-only stroke" else "$n raster-only strokes"
    fun rasterizedLayers(n: Int): String = if (n == 1) "1 rasterized layer" else "$n rasterized layers"
    fun profile(p: String): String = "profile: $p"

    fun summaryLines(info: ExportPreflightInfo): List<String> = buildList {
        if (info.format.isIllustratorHandoff) {
            add(vectorPaths(info.vectorPathCount))
            add(rasterOnlyStrokes(info.rasterOnlyStrokeCount))
        }
        add(rasterizedLayers(info.rasterizedLayerCount))
        add(profile(info.profile))
        info.warnings.forEach { add(it.message) }
    }

    /** Every fixed user-facing string (for wording audits). */
    fun allFixedStrings(): List<String> = listOf(
        ILLUSTRATOR_TITLE, PHOTOSHOP_TITLE, IMAGE_TITLE, VECTOR_LABEL, RASTER_LABEL, PROFILE_LABEL,
        BOTH_OFF_MESSAGE, NOT_VERIFIED_MESSAGE, ILLUSTRATOR_HINT,
    ) + ExportFormat.entries.map(::formatLabel) + ExportDestination.entries.map(::destinationTitle)
}

/** Safe default export/save file names (no path separators, never an `.ai` extension). */
object ExportFileNames {
    private const val MAX_BASE_CHARS = 80

    fun sanitizeBase(name: String?): String {
        val cleaned = (name ?: "").map { c ->
            if (c.isLetterOrDigit() || c == ' ' || c == '-' || c == '_') c else '_'
        }.joinToString("").trim().trim('.', '_').take(MAX_BASE_CHARS).trim()
        return cleaned.ifEmpty { "Untitled" }
    }

    fun suggest(baseName: String?, format: ExportFormat): String = sanitizeBase(baseName) + "." + format.extension

    const val NATIVE_EXTENSION: String = "veskra"

    fun suggestProject(baseName: String?): String = sanitizeBase(baseName) + "." + NATIVE_EXTENSION
}
