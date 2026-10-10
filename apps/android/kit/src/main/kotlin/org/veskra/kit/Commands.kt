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

    /**
     * Import a PNG (copied from SAF into app-private storage first) as a new raster layer.
     * [assign] = keep numbers instead of converting colors (COL-005); warnings must be shown.
     */
    fun layerImportImage(path: String, assign: Boolean = false, rejectUntagged: Boolean = false): JsonObject = json {
        "cmd" to "layer.import_image"
        "path" to path
        "handling" to (if (assign) "assign" else "convert")
        "missingProfile" to (if (rejectUntagged) "reject" else "assume_srgb")
    }

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

    /** Sample the visible composite (or only the active layer) at a document point; result `{hex}`. */
    fun colorPick(x: Float, y: Float, activeLayerOnly: Boolean = false): JsonObject = json {
        "cmd" to "color.pick"
        "x" to x
        "y" to y
        "layer" to activeLayerOnly
    }

    fun selectionRect(x0: Float, y0: Float, x1: Float, y1: Float, op: SelectionOp = SelectionOp.REPLACE): JsonObject =
        selectionBox("selection.rect", x0, y0, x1, y1, op)

    fun selectionEllipse(x0: Float, y0: Float, x1: Float, y1: Float, op: SelectionOp = SelectionOp.REPLACE): JsonObject =
        selectionBox("selection.ellipse", x0, y0, x1, y1, op)

    private fun selectionBox(cmd: String, x0: Float, y0: Float, x1: Float, y1: Float, op: SelectionOp): JsonObject {
        require(listOf(x0, y0, x1, y1).all(Float::isFinite)) { "selection bounds must be finite" }
        return json {
            "cmd" to cmd
            "x0" to x0
            "y0" to y0
            "x1" to x1
            "y1" to y1
            "op" to op.wire
        }
    }

    /** Freehand selection; [points] are document px, at least 3. */
    fun selectionLasso(points: List<PointF>, op: SelectionOp = SelectionOp.REPLACE): JsonObject {
        require(points.size >= 3) { "a lasso needs at least 3 points" }
        return json {
            "cmd" to "selection.lasso"
            "points" to pointArray(points)
            "op" to op.wire
        }
    }

    fun selectionAll(): JsonObject = json { "cmd" to "selection.all" }
    fun selectionClear(): JsonObject = json { "cmd" to "selection.clear" }
    fun selectionInvert(): JsonObject = json { "cmd" to "selection.invert" }

    /** Bucket fill with the foreground color on the active raster layer. */
    fun editFill(x: Float, y: Float, tolerance: Float = 0.1f, contiguous: Boolean = true, sampleMerged: Boolean = false): JsonObject {
        require(tolerance.isFinite() && tolerance in 0f..1f) { "tolerance must be in [0,1]" }
        return json {
            "cmd" to "edit.fill"
            "x" to x
            "y" to y
            "tolerance" to tolerance
            "contiguous" to contiguous
            "sampleMerged" to sampleMerged
        }
    }

    fun editFillSelection(): JsonObject = json { "cmd" to "edit.fill_selection" }
    fun editDeleteSelection(): JsonObject = json { "cmd" to "edit.delete_selection" }

    /** Affine `[a,b,c,d,e,f]` (x' = a·x + c·y + e, y' = b·x + d·y + f) applied to the selection or the layer. */
    fun editTransform(m: Affine): JsonObject = json {
        "cmd" to "edit.transform"
        "matrix" to JsonArray(m.toList().map { JsonNumber.of(it) })
    }

    fun layerMaskFromSelection(id: String): JsonObject = json { "cmd" to "layer.mask_from_selection"; "id" to id }
    fun layerMaskRemove(id: String): JsonObject = json { "cmd" to "layer.mask_remove"; "id" to id }
    fun layerMaskApply(id: String): JsonObject = json { "cmd" to "layer.mask_apply"; "id" to id }
    fun layerClear(id: String): JsonObject = json { "cmd" to "layer.clear"; "id" to id }

    /** [layerImportImage] with a layer name and a top-left placement in document px. */
    fun layerImportImageAt(path: String, name: String, x: Int, y: Int): JsonObject = json {
        "cmd" to "layer.import_image"
        "path" to path
        "name" to name
        "x" to x
        "y" to y
    }

    // ----- stroke-time helpers: symmetry, guides, shapes -----

    /** [mode] = off | vertical | horizontal | radial; [count] = number of radial copies. */
    fun symmetrySet(mode: String, count: Int): JsonObject {
        require(count in 2..16) { "symmetry count must be in 2..16" }
        return json {
            "cmd" to "symmetry.set"
            "mode" to mode
            if (mode == "radial") "count" to count
        }
    }

    fun guideOff(): JsonObject = json { "cmd" to "guide.set"; "kind" to "off" }

    /**
     * What the pen does on the input path: [tool] = brush | eraser | smudge. An eraser *preset*
     * still erases under `brush`; `smudge` drags the layer's own pixels with [strength] in [0, 1].
     */
    fun toolSet(tool: String, strength: Float? = null): JsonObject {
        if (strength != null) require(strength.isFinite() && strength in 0f..1f) { "strength must be in [0,1]" }
        return json {
            "cmd" to "tool.set"
            "tool" to tool
            if (strength != null) "strength" to strength
        }
    }

    /** Strokes are projected onto the infinite line through [a] and [b]. */
    fun guideLine(a: PointF, b: PointF): JsonObject = json {
        "cmd" to "guide.set"
        "kind" to "line"
        "x0" to a.x
        "y0" to a.y
        "x1" to b.x
        "y1" to b.y
    }

    /** Strokes are projected onto the outline of the ellipse inscribed in the box. */
    fun guideEllipse(x0: Float, y0: Float, x1: Float, y1: Float): JsonObject = json {
        "cmd" to "guide.set"
        "kind" to "ellipse"
        "x0" to x0
        "y0" to y0
        "x1" to x1
        "y1" to y1
    }

    /** A straight stroke with the current brush and color. */
    fun shapeLine(a: PointF, b: PointF): JsonObject = json {
        "cmd" to "shape.line"
        "x0" to a.x
        "y0" to a.y
        "x1" to b.x
        "y1" to b.y
    }

    fun shapeRect(x0: Float, y0: Float, x1: Float, y1: Float, fill: Boolean = false): JsonObject = shapeBox("shape.rect", x0, y0, x1, y1, fill)

    fun shapeEllipse(x0: Float, y0: Float, x1: Float, y1: Float, fill: Boolean = false): JsonObject = shapeBox("shape.ellipse", x0, y0, x1, y1, fill)

    private fun shapeBox(cmd: String, x0: Float, y0: Float, x1: Float, y1: Float, fill: Boolean): JsonObject {
        require(listOf(x0, y0, x1, y1).all(Float::isFinite)) { "shape bounds must be finite" }
        return json {
            "cmd" to cmd
            "x0" to x0
            "y0" to y0
            "x1" to x1
            "y1" to y1
            "fill" to fill
        }
    }

    /** Pen tool: one stroke through [points]; [smooth] fits a curve, [closed] joins the ends. */
    fun shapePath(points: List<PointF>, closed: Boolean = false, smooth: Boolean = true, fill: Boolean = false): JsonObject {
        require(points.size >= 2) { "a path needs at least 2 points" }
        return json {
            "cmd" to "shape.path"
            "points" to pointArray(points)
            "closed" to closed
            "smooth" to smooth
            "fill" to fill
        }
    }

    // ----- raster edits -----

    /** Gradient from [a] to [b] on the active raster layer; [stops] are (offset 0..1, color) pairs. */
    fun editGradient(a: PointF, b: PointF, radial: Boolean, stops: List<Pair<Float, HexColor>>): JsonObject {
        require(stops.size >= 2) { "a gradient needs at least 2 stops" }
        return json {
            "cmd" to "edit.gradient"
            "x0" to a.x
            "y0" to a.y
            "x1" to b.x
            "y1" to b.y
            "kind" to (if (radial) "radial" else "linear")
            "stops" to JsonArray(stops.map { (offset, color) -> json { "offset" to offset; "color" to color.toHex() } })
        }
    }

    /** Magic wand: select the region of similar color around a document point. */
    fun selectionWand(
        x: Float,
        y: Float,
        tolerance: Float = 0.1f,
        contiguous: Boolean = true,
        sampleMerged: Boolean = false,
        op: SelectionOp = SelectionOp.REPLACE,
    ): JsonObject {
        require(tolerance.isFinite() && tolerance in 0f..1f) { "tolerance must be in [0,1]" }
        return json {
            "cmd" to "selection.wand"
            "x" to x
            "y" to y
            "tolerance" to tolerance
            "contiguous" to contiguous
            "sampleMerged" to sampleMerged
            "op" to op.wire
        }
    }

    /** [hue] in degrees (−180..180), [saturation] and [brightness] in −1..1; 0 = unchanged. */
    fun editAdjustHsb(hue: Float, saturation: Float, brightness: Float): JsonObject = json {
        "cmd" to "edit.adjust"
        "kind" to "hsb"
        "hue" to hue
        "saturation" to saturation
        "brightness" to brightness
    }

    fun editAdjustBrightnessContrast(brightness: Float, contrast: Float): JsonObject = json {
        "cmd" to "edit.adjust"
        "kind" to "brightness_contrast"
        "brightness" to brightness
        "contrast" to contrast
    }

    fun editAdjustInvert(): JsonObject = json { "cmd" to "edit.adjust"; "kind" to "invert" }

    /** Tone curves; [curves] maps `rgb` / `r` / `g` / `b` to control points (input, output) in [0,1]. */
    fun editAdjustCurves(curves: Map<String, List<PointF>>): JsonObject = json {
        "cmd" to "edit.adjust"
        "kind" to "curves"
        for ((channel, points) in curves) channel to pointArray(points)
    }

    /** Projective map of the selection / layer content box onto [quad] (TL, TR, BR, BL). */
    fun editTransformQuad(quad: List<PointF>): JsonObject {
        require(quad.size == 4) { "a quad has 4 corners" }
        return json { "cmd" to "edit.transform_quad"; "quad" to pointArray(quad) }
    }

    /** The canvas becomes the half-open document rectangle [x0, x1) × [y0, y1). */
    fun canvasCrop(x0: Int, y0: Int, x1: Int, y1: Int): JsonObject {
        require(x1 > x0 && y1 > y0) { "crop rectangle must not be empty" }
        return json {
            "cmd" to "canvas.crop"
            "x0" to x0
            "y0" to y0
            "x1" to x1
            "y1" to y1
        }
    }

    /** [resample] = scale the artwork with the canvas; otherwise only the canvas extent changes. */
    fun canvasResize(width: Int, height: Int, resample: Boolean): JsonObject {
        require(width > 0 && height > 0) { "canvas size must be positive" }
        return json {
            "cmd" to "canvas.resize"
            "width" to width
            "height" to height
            "resample" to resample
        }
    }

    /** Camera lock: the engine ignores pan / zoom / rotate gestures while drawing still works. */
    fun inputConfigLockView(locked: Boolean): JsonObject = json { "cmd" to "input.config"; "lockView" to locked }

    /** Selection boundary as closed polylines + `bounds` (marching ants, transform box). */
    fun selectionOutline(maxPoints: Int = 4000): JsonObject = json { "cmd" to "selection.outline"; "maxPoints" to maxPoints }

    /** Tight bounds of the active layer's content; `{"bounds": [x0,y0,x1,y1] | null}`. */
    fun layerBounds(): JsonObject = json { "cmd" to "layer.bounds" }

    /** Absolute view placement: `view = pan + R(rotation)·zoom·doc`; null fields stay unchanged. */
    fun viewSet(zoom: Float? = null, rotation: Float? = null, panX: Float? = null, panY: Float? = null): JsonObject {
        for (v in listOfNotNull(zoom, rotation, panX, panY)) require(v.isFinite()) { "view values must be finite" }
        return json {
            "cmd" to "view.set"
            if (zoom != null) "zoom" to zoom
            if (rotation != null) "rotation" to rotation
            if (panX != null) "panX" to panX
            if (panY != null) "panY" to panY
        }
    }

    fun viewFit(): JsonObject = json { "cmd" to "view.fit" }
    fun viewMirror(enabled: Boolean): JsonObject = json { "cmd" to "view.mirror"; "enabled" to enabled }

    internal fun pointArray(points: List<PointF>): JsonArray =
        JsonArray(points.map { p -> JsonArray(listOf(JsonNumber.of(p.x), JsonNumber.of(p.y))) })

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
    val hasSelection: Boolean = false,
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
            hasSelection = o.bool("hasSelection") ?: false,
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
    val hasMask: Boolean = false,
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
            hasMask = o.bool("hasMask") ?: false,
        )

        fun listFromJson(o: JsonObject): List<LayerInfo>? =
            o.array("layers")?.items?.map { (it as? JsonObject)?.let(::fromJson) ?: return null }
    }
}
