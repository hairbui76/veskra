package org.veskra.kit

/**
 * JSON command builders for `nativeExecute` — the command vocabulary implemented by
 * `veskra-engine` (normative list: docs/COMMANDS.md). Strings below are the single source on the
 * Kotlin side and are reconciled with the engine.
 */
object Commands {
    fun documentCreate(width: Int, height: Int, dpi: Int, background: String?): JsonObject = json {
        "cmd" to "document.create"
        "width" to width
        "height" to height
        "dpi" to dpi
        "background" to background // "#RRGGBB" or null for transparent
    }

    fun documentOpen(path: String): JsonObject = json { "cmd" to "document.open"; "path" to path }
    fun documentSave(path: String): JsonObject = json { "cmd" to "document.save"; "path" to path }
    fun documentValidate(path: String): JsonObject = json { "cmd" to "document.validate"; "path" to path }
    fun documentAutosave(dir: String): JsonObject = json { "cmd" to "document.autosave"; "dir" to dir }
    fun documentRecover(dir: String): JsonObject = json { "cmd" to "document.recover"; "dir" to dir }
    fun documentInfo(): JsonObject = json { "cmd" to "document.info" }

    fun layerAdd(kind: LayerKind, name: String): JsonObject = json {
        "cmd" to "layer.add"
        "kind" to kind.wire
        "name" to name
    }

    fun layerRemove(id: String): JsonObject = json { "cmd" to "layer.remove"; "id" to id }
    fun layerSelect(id: String): JsonObject = json { "cmd" to "layer.select"; "id" to id }
    fun layerDuplicate(id: String): JsonObject = json { "cmd" to "layer.duplicate"; "id" to id }
    fun layerRename(id: String, name: String): JsonObject = json { "cmd" to "layer.rename"; "id" to id; "name" to name }

    /** Move directly above sibling [targetId] (engine placement `{"at":"above","id":…}`). */
    fun layerMoveAbove(id: String, targetId: String): JsonObject = json {
        "cmd" to "layer.move"
        "id" to id
        "placement" to json { "at" to "above"; "id" to targetId }
    }

    /** Move directly below sibling [targetId]. */
    fun layerMoveBelow(id: String, targetId: String): JsonObject = json {
        "cmd" to "layer.move"
        "id" to id
        "placement" to json { "at" to "below"; "id" to targetId }
    }

    /** Partial, atomic property update; null fields are omitted (unchanged). */
    fun layerSet(
        id: String,
        visible: Boolean? = null,
        opacity: Float? = null,
        blend: BlendMode? = null,
        alphaLock: Boolean? = null,
        clip: Boolean? = null,
        locked: Boolean? = null,
    ): JsonObject {
        if (opacity != null) require(opacity.isFinite() && opacity in 0f..1f) { "opacity must be in [0,1]" }
        return json {
            "cmd" to "layer.set"
            "id" to id
            if (visible != null) "visible" to visible
            if (opacity != null) "opacity" to opacity
            if (blend != null) "blend" to blend.wire
            if (alphaLock != null) "alphaLocked" to alphaLock
            if (clip != null) "clip" to clip
            if (locked != null) "locked" to locked
        }
    }

    fun layerList(): JsonObject = json { "cmd" to "layer.list" }

    fun historyUndo(): JsonObject = json { "cmd" to "history.undo" }
    fun historyRedo(): JsonObject = json { "cmd" to "history.redo" }

    fun brushSelect(preset: String): JsonObject = json { "cmd" to "brush.select"; "preset" to preset }

    /** Size in document px; opacity/flow in [0,1]. Null = unchanged. */
    fun brushSet(size: Float? = null, opacity: Float? = null, flow: Float? = null, stabilization: Float? = null): JsonObject {
        if (size != null) require(size.isFinite() && size > 0f) { "size must be > 0" }
        for (v in listOfNotNull(opacity, flow, stabilization)) require(v.isFinite() && v in 0f..1f) { "value must be in [0,1]" }
        return json {
            "cmd" to "brush.set"
            if (size != null) "size" to size
            if (opacity != null) "opacity" to opacity
            if (flow != null) "flow" to flow
            if (stabilization != null) "stabilization" to stabilization
        }
    }

    fun colorSet(color: HexColor): JsonObject = json { "cmd" to "color.set"; "hex" to color.toHex() }

    fun viewFit(): JsonObject = json { "cmd" to "view.fit" }
    fun viewMirror(enabled: Boolean): JsonObject = json { "cmd" to "view.mirror"; "enabled" to enabled }

    /** Lifecycle pause: engine applies its commit/cancel policy to any in-flight stroke (InputArbiter::reset). */
    fun inputReset(): JsonObject = json { "cmd" to "input.reset" }

    fun exportPreflight(format: ExportFormat, options: ExportOptions): JsonObject = json {
        "cmd" to "export.preflight"
        "format" to format.wire
        "options" to options.toJson()
    }

    fun exportRun(format: ExportFormat, path: String, options: ExportOptions): JsonObject = json {
        "cmd" to "export.run"
        "format" to format.wire
        "path" to path
        "options" to options.toJson()
    }
}

enum class LayerKind(val wire: String) {
    RASTER("raster"), VECTOR("vector"), GROUP("group");

    companion object {
        fun fromWire(s: String?): LayerKind? = entries.firstOrNull { it.wire == s }
    }
}

/** Compact v1 blend set (MASTER_PLAN §1.1); wire names match `veskra_core::BlendMode::as_str`. */
enum class BlendMode(val wire: String) {
    NORMAL("normal"), MULTIPLY("multiply"), SCREEN("screen"), OVERLAY("overlay");

    companion object {
        fun fromWire(s: String?): BlendMode? = entries.firstOrNull { it.wire == s }
    }
}

/** `document.info` result. */
data class DocumentInfo(
    val width: Int,
    val height: Int,
    val dpi: Int,
    /** Monotonic document version; increments on every committed history change. */
    val version: Long,
    val profile: String,
    val activeLayerId: String?,
    val canUndo: Boolean,
    val canRedo: Boolean,
) {
    companion object {
        fun fromJson(o: JsonObject): DocumentInfo? = DocumentInfo(
            width = o.int("width") ?: return null,
            height = o.int("height") ?: return null,
            dpi = o.double("dpi")?.toInt() ?: return null,
            version = o.long("version") ?: return null,
            profile = o.string("profile") ?: "sRGB",
            activeLayerId = o.string("activeLayer"),
            canUndo = o.bool("canUndo") ?: false,
            canRedo = o.bool("canRedo") ?: false,
        )
    }
}

/** One entry of `layer.list` (flat, top-most first; [depth] > 0 inside groups). */
data class LayerInfo(
    val id: String,
    val name: String,
    val kind: LayerKind,
    val visible: Boolean,
    val opacity: Float,
    val blend: BlendMode,
    val alphaLock: Boolean,
    val clip: Boolean,
    val locked: Boolean,
    val depth: Int,
    /** Engine `vectorFidelity == "exact"`: every stroke exports as an editable path. */
    val vectorEligible: Boolean,
) {
    companion object {
        fun fromJson(o: JsonObject): LayerInfo? = LayerInfo(
            id = o.string("id") ?: return null,
            name = o.string("name") ?: "",
            kind = LayerKind.fromWire(o.string("kind")) ?: return null,
            visible = o.bool("visible") ?: true,
            opacity = (o.double("opacity") ?: 1.0).toFloat().coerceIn(0f, 1f),
            blend = BlendMode.fromWire(o.string("blend")) ?: BlendMode.NORMAL,
            alphaLock = o.bool("alphaLocked") ?: false,
            clip = o.bool("clip") ?: false,
            locked = o.bool("locked") ?: false,
            depth = o.int("depth") ?: 0,
            vectorEligible = o.string("vectorFidelity") == "exact",
        )

        fun listFromJson(o: JsonObject): List<LayerInfo>? =
            o.array("layers")?.items?.map { (it as? JsonObject)?.let(::fromJson) ?: return null }
    }
}
