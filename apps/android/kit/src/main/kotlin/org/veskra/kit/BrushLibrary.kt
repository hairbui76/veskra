package org.veskra.kit

/**
 * One entry of `brush.list`: built-in, custom or imported. Older engines report only id, name,
 * vectorEligible and eraser; the other fields then fall back to values derived from those.
 */
data class BrushInfo(
    val id: String,
    val name: String,
    val category: String,
    /** `builtin`, `custom` or `imported:<format>` (e.g. `imported:prbr`). */
    val source: String,
    /** Pack or folder the brush came from (imported packs), null for built-ins. */
    val folder: String?,
    val vectorEligible: Boolean,
    val eraser: Boolean,
    val version: Int,
    val editable: Boolean,
) {
    val isBuiltIn: Boolean get() = source == "builtin"
    val isImported: Boolean get() = source.startsWith("imported")

    companion object {
        /** Display order of categories; unknown ones go after these, alphabetically. */
        val CATEGORY_ORDER: List<String> =
            listOf("pencil", "ink", "marker", "paint", "airbrush", "texture", "effect", "smudge", "eraser", "custom", "imported")

        fun fromJson(o: JsonObject): BrushInfo? {
            val id = o.string("id") ?: return null
            val eraser = o.bool("eraser") ?: false
            val source = o.string("source") ?: "builtin"
            return BrushInfo(
                id = id,
                name = o.string("name") ?: id,
                category = o.string("category") ?: guessCategory(id, eraser),
                source = source,
                folder = o.string("folder"),
                vectorEligible = o.bool("vectorEligible") ?: false,
                eraser = eraser,
                version = o.int("version") ?: 1,
                editable = o.bool("editable") ?: (source != "builtin"),
            )
        }

        /** `brush.list` → `{"presets": [...]}`. */
        fun listFromJson(o: JsonObject): List<BrushInfo> =
            o.array("presets")?.items.orEmpty().mapNotNull { (it as? JsonObject)?.let(::fromJson) }

        private fun guessCategory(id: String, eraser: Boolean): String = when {
            eraser -> "eraser"
            id.startsWith("pencil") -> "pencil"
            id.startsWith("pen") -> "ink"
            id.startsWith("marker") -> "marker"
            id == "round.soft" -> "airbrush"
            id == "charcoal" -> "texture"
            else -> "paint"
        }

        /** Stand-in until the engine's list arrives (same id as [BrushPreset.DEFAULT]). */
        val FALLBACK: BrushInfo = BrushInfo(
            BrushPreset.DEFAULT.id, "Rough pencil", "pencil", "builtin", null,
            vectorEligible = false, eraser = false, version = 1, editable = false,
        )
        val FALLBACK_ERASER: BrushInfo = BrushInfo(
            BrushPreset.ERASER_HARD.id, "Hard eraser", "eraser", "builtin", null,
            vectorEligible = false, eraser = true, version = 1, editable = false,
        )
    }
}

/** `brush.import` result: what came in, what was approximated, what could not be read. */
data class BrushImportReport(
    val imported: List<ImportedBrush>,
    val failed: List<FailedBrush>,
    val warnings: List<String>,
) {
    data class ImportedBrush(val id: String, val name: String, val folder: String?, val approximated: List<String>, val unsupported: List<String>)

    data class FailedBrush(val name: String, val reason: String)

    companion object {
        private fun strings(o: JsonObject, key: String): List<String> =
            o.array(key)?.items.orEmpty().mapNotNull { (it as? JsonString)?.value }

        fun fromJson(o: JsonObject): BrushImportReport = BrushImportReport(
            imported = o.array("imported")?.items.orEmpty().mapNotNull { v ->
                val b = v as? JsonObject ?: return@mapNotNull null
                ImportedBrush(
                    id = b.string("id") ?: return@mapNotNull null,
                    name = b.string("name") ?: "",
                    folder = b.string("folder"),
                    approximated = strings(b, "approximated"),
                    unsupported = strings(b, "unsupported"),
                )
            },
            failed = o.array("failed")?.items.orEmpty().mapNotNull { v ->
                val b = v as? JsonObject ?: return@mapNotNull null
                FailedBrush(b.string("name") ?: "", b.string("reason") ?: "")
            },
            warnings = strings(o, "warnings"),
        )
    }
}

/** File types the brush importer understands (Veskra's own and Infinite Painter's). */
object BrushFiles {
    val IMPORT_EXTENSIONS: List<String> = listOf("vbrush", "prbr", "przp")
    const val EXPORT_EXTENSION: String = "vbrush"

    fun isImportable(fileName: String?): Boolean =
        fileName?.substringAfterLast('.', "")?.lowercase() in IMPORT_EXTENSIONS
}
