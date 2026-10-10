package org.veskra.dev.ui

import android.app.Application
import android.content.Intent
import android.net.Uri
import androidx.annotation.StringRes
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.veskra.dev.R
import org.veskra.dev.VeskraApplication
import org.veskra.dev.canvas.CanvasDisplay
import org.veskra.dev.canvas.TextRasterizer
import org.veskra.dev.engine.EngineHost
import org.veskra.dev.storage.AppPreferences
import org.veskra.dev.storage.ProjectEntry
import org.veskra.dev.storage.ProjectStore
import org.veskra.dev.storage.RecoveryEntry
import org.veskra.kit.Affine
import org.veskra.kit.BlendMode
import org.veskra.kit.BrushPreset
import org.veskra.kit.CanvasSpec
import org.veskra.kit.Commands
import org.veskra.kit.CopyFailedException
import org.veskra.kit.DocumentInfo
import org.veskra.kit.EngineError
import org.veskra.kit.EngineResult
import org.veskra.kit.EngineSession
import org.veskra.kit.ExportDestination
import org.veskra.kit.ExportFileNames
import org.veskra.kit.ExportSheetEvent
import org.veskra.kit.ExportSheetState
import org.veskra.kit.HexColor
import org.veskra.kit.JsonArray
import org.veskra.kit.JsonNumber
import org.veskra.kit.JsonObject
import org.veskra.kit.JsonValue
import org.veskra.kit.LayerInfo
import org.veskra.kit.LayerKind
import org.veskra.kit.Palette
import org.veskra.kit.PointF
import org.veskra.kit.RecentColors
import org.veskra.kit.RectI
import org.veskra.kit.SaveDestination
import org.veskra.kit.SaveFailureReason
import org.veskra.kit.SaveStateMachine
import org.veskra.kit.SaveStatus
import org.veskra.kit.SaveStep
import org.veskra.kit.SelectionOp
import org.veskra.kit.ViewState
import org.veskra.kit.SafCopyPlan
import java.io.File
import java.io.IOException

enum class Screen { GALLERY, EDITOR }

enum class EditorSheet { BRUSH, COLOR, LAYERS, TOOLS, EXPORT }

/**
 * What a one-finger / stylus gesture on the canvas does. [PAINT] sends input straight to the engine
 * (brush, eraser); every other tool is handled by the tool overlay, which turns the gesture into one
 * engine command.
 */
enum class Tool {
    PAINT, FILL, EYEDROPPER,
    SELECT_RECT, SELECT_ELLIPSE, SELECT_LASSO, SELECT_WAND,
    TRANSFORM, DISTORT,
    SHAPE_LINE, SHAPE_RECT, SHAPE_ELLIPSE, SHAPE_PATH,
    GRADIENT, CROP,
    GUIDE_LINE, GUIDE_ELLIPSE,
    ADJUST, CURVES,
    ;

    val isSelection: Boolean get() = this == SELECT_RECT || this == SELECT_ELLIPSE || this == SELECT_LASSO || this == SELECT_WAND
    val isShape: Boolean get() = this == SHAPE_LINE || this == SHAPE_RECT || this == SHAPE_ELLIPSE || this == SHAPE_PATH

    /**
     * Tools that show a live result while their handles or sliders move. The result is one
     * preview command on top of the undo stack that is replaced on every change, kept by Done and
     * undone by Cancel.
     */
    val hasPreview: Boolean get() = this == TRANSFORM || this == DISTORT || this == ADJUST || this == CURVES
}

/** Mirror modes of `symmetry.set`. */
enum class SymmetryMode(val wire: String) {
    OFF("off"), VERTICAL("vertical"), HORIZONTAL("horizontal"), RADIAL("radial")
}

/** A ruler that strokes snap to (`guide.set`). Geometry is in document px. */
sealed interface Guide {
    data class Line(val a: PointF, val b: PointF) : Guide

    data class Ellipse(val bounds: RectF) : Guide
}

/** Options of the shape tools. */
data class ShapeOptions(val fill: Boolean = false, val closed: Boolean = false, val smooth: Boolean = true)

/** Color adjustment being previewed ([Tool.ADJUST]); all values are deltas, 0 = unchanged. */
data class Adjustment(
    val hue: Float = 0f,
    val saturation: Float = 0f,
    val brightness: Float = 0f,
    val contrast: Float = 0f,
    val invert: Boolean = false,
) {
    val isNeutral: Boolean get() = this == Adjustment()
}

/** Which curve [Tool.CURVES] edits; [RGB] is the master curve applied to all three channels. */
enum class CurveChannel(val wire: String) { RGB("rgb"), R("r"), G("g"), B("b") }

/** Tone curves as control points in [0, 1]² (input → output), sorted by input, ends included. */
data class Curves(
    val channel: CurveChannel = CurveChannel.RGB,
    val points: Map<CurveChannel, List<PointF>> = CurveChannel.entries.associateWith { IDENTITY },
) {
    val active: List<PointF> get() = points.getValue(channel)
    val isNeutral: Boolean get() = points.values.all { it == IDENTITY }

    fun withActive(p: List<PointF>): Curves = copy(points = points + (channel to p.sortedBy { it.x }))

    companion object {
        val IDENTITY: List<PointF> = listOf(PointF(0f, 0f), PointF(1f, 1f))
    }
}

/** Bucket-fill options shown in the tool bar while [Tool.FILL] is active. */
data class FillOptions(val tolerance: Float = 0.1f, val contiguous: Boolean = true, val sampleMerged: Boolean = false)

/**
 * An in-progress Transform: [bounds] is the untransformed content box (document px), [matrix] the
 * transform accumulated so far. [applied] = the engine currently holds a preview `edit.transform`
 * step that must be undone before the next preview (so pixels are always resampled once, from the
 * original, and the whole session ends as a single undo step).
 */
data class TransformSession(val bounds: RectF, val matrix: Affine = Affine.IDENTITY)

/** An in-progress Distort: the content box [source] is mapped onto the four corners of [quad]. */
data class DistortSession(val source: RectF, val quad: List<PointF> = source.corners)

/** Float rectangle in document px (tool previews, content bounds). */
data class RectF(val left: Float, val top: Float, val right: Float, val bottom: Float) {
    val width: Float get() = right - left
    val height: Float get() = bottom - top
    val center: PointF get() = PointF((left + right) / 2f, (top + bottom) / 2f)
    val corners: List<PointF> get() = listOf(PointF(left, top), PointF(right, top), PointF(right, bottom), PointF(left, bottom))

    companion object {
        /** Normalised rectangle through two opposite corners. */
        fun of(a: PointF, b: PointF): RectF = RectF(minOf(a.x, b.x), minOf(a.y, b.y), maxOf(a.x, b.x), maxOf(a.y, b.y))
    }
}

/** A user-visible message: string resource + format args (localizable). */
data class UiMessage(@StringRes val id: Int, val args: List<Any> = emptyList())

data class UiState(
    val screen: Screen = Screen.GALLERY,
    val engineAvailable: Boolean = true,
    val projects: List<ProjectEntry> = emptyList(),
    val recovery: RecoveryEntry? = null,
    val docName: String = DEFAULT_DOC_NAME,
    val doc: DocumentInfo? = null,
    val layers: List<LayerInfo> = emptyList(),
    val brush: BrushPreset = BrushPreset.DEFAULT,
    val brushSize: Float = 8f,
    val brushOpacity: Float = 1f,
    /** Stroke smoothing ("Lazy"), 0 = off. */
    val brushStabilization: Float = 0f,
    /** Where the eraser toggle returns to, and which eraser it picks. */
    val lastPaintBrush: BrushPreset = BrushPreset.DEFAULT,
    val lastEraser: BrushPreset = BrushPreset.ERASER_HARD,
    /** The pen smudges (drags existing paint) instead of painting with [brush]. */
    val smudge: Boolean = false,
    val smudgeStrength: Float = 0.5f,
    val color: HexColor = HexColor.BLACK,
    val secondaryColor: HexColor = HexColor.WHITE,
    val recentColors: List<HexColor> = emptyList(),
    val userPalette: List<HexColor> = emptyList(),
    val tool: Tool = Tool.PAINT,
    val selectionOp: SelectionOp = SelectionOp.REPLACE,
    val fill: FillOptions = FillOptions(),
    /** Selection boundary in document px (closed polylines) for the marching-ants overlay. */
    val selectionOutline: List<List<PointF>> = emptyList(),
    val transform: TransformSession? = null,
    val distort: DistortSession? = null,
    /** Crop frame being adjusted, document px. */
    val crop: RectF? = null,
    val symmetry: SymmetryMode = SymmetryMode.OFF,
    val symmetryCount: Int = 6,
    val guide: Guide? = null,
    val shape: ShapeOptions = ShapeOptions(),
    /** Points placed so far by the path (pen) tool, document px. */
    val pathPoints: List<PointF> = emptyList(),
    val gradientRadial: Boolean = false,
    val adjustment: Adjustment = Adjustment(),
    val curves: Curves = Curves(),
    /** Camera lock: two-finger pan / zoom / rotate is ignored. */
    val viewLocked: Boolean = false,
    val resizeDialog: Boolean = false,
    /** The "Add text" dialog is open. */
    val textDialog: Boolean = false,
    val saveStatus: SaveStatus = SaveStatus.NeverSaved(false),
    val sheet: EditorSheet? = null,
    val export: ExportSheetState? = null,
    val railOnLeft: Boolean = true,
    val mirrored: Boolean = false,
    val busy: Boolean = false,
    val message: UiMessage? = null,
) {
    companion object {
        const val DEFAULT_DOC_NAME: String = "Untitled"
    }
}

/**
 * Activity-scoped state holder. Survives configuration changes, so the native session, the display
 * tiles and all editor state survive rotation / resize (MASTER_PLAN §2.2).
 *
 * Threading: all mutable fields here are touched on the main thread only; engine work goes through
 * [EngineHost] (single engine thread), file IO through Dispatchers.IO.
 */
class VeskraViewModel(app: Application) : AndroidViewModel(app) {
    private val veskraApp = app as VeskraApplication
    private val store = ProjectStore(app)
    private val prefs = AppPreferences(app)
    private val saveMachine = SaveStateMachine()
    private val recent = RecentColors(capacity = 12, initial = prefs.recentColors)

    /** Null when libveskra_ffi.so is missing: the UI shows an explanatory screen. */
    val engine: EngineHost? = if (veskraApp.engineAvailable) EngineHost() else null
    val display = CanvasDisplay()

    private val _ui = MutableStateFlow(
        UiState(
            engineAvailable = engine != null,
            railOnLeft = prefs.railOnLeft,
            recentColors = recent.colors,
            color = prefs.primaryColor,
            secondaryColor = prefs.secondaryColor,
            userPalette = prefs.userPalette,
        ),
    )
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    /** doc→view transform of the canvas, for tool overlays (updated on the UI thread). */
    private val _view = MutableStateFlow(ViewState.IDENTITY)
    val view: StateFlow<ViewState> = _view.asStateFlow()

    private var autosaveJob: Job? = null

    init {
        display.historyListener = { committed -> onHistoryChanged(committed) }
        display.viewListener = { v -> _view.value = v }
        refreshGallery()
    }

    override fun onCleared() {
        display.historyListener = null
        display.viewListener = null
        display.release()
        engine?.close()
    }

    // ---------------------------------------------------------------- helpers

    private fun str(@StringRes id: Int, vararg args: Any): String = getApplication<Application>().getString(id, *args)

    private fun show(@StringRes id: Int, vararg args: Any) = _ui.update { it.copy(message = UiMessage(id, args.toList())) }

    fun dismissMessage() = _ui.update { it.copy(message = null) }

    private fun showEngineError(e: EngineError) = show(R.string.engine_error, e.message.ifBlank { e.code })

    /** Runs [block] on the engine thread; maps an unavailable session to a UI error. */
    private suspend fun <T> onEngine(block: (EngineSession) -> T): T? {
        val host = engine ?: return null
        return try {
            host.call(block)
        } catch (e: IllegalStateException) {
            show(R.string.engine_unavailable_title)
            null
        }
    }

    /** Optional `"dirty":[[x0,y0,x1,y1],…]` in a command's ok payload (COMMANDS.md); null → redraw all. */
    private fun dirtyRects(ok: JsonValue): List<RectI>? {
        val arr = (ok as? JsonObject)?.array("dirty") ?: return null
        return arr.items.map { r ->
            val a = r as? JsonArray ?: return null
            if (a.size != 4) return null
            val v = a.items.map { (it as? JsonNumber)?.asLongOrNull?.toInt() ?: return null }
            RectI(v[0], v[1], v[2], v[3])
        }
    }

    /** Sends a mutating command, redraws affected tiles and refreshes document/layer state. */
    private fun command(cmd: JsonObject, redraw: Boolean = true) {
        viewModelScope.launch {
            val result = onEngine { s ->
                val r = s.execute(cmd)
                if (r is EngineResult.Ok && redraw) engine?.renderer?.invalidate(s, dirtyRects(r.value))
                r
            } ?: return@launch
            if (result is EngineResult.Err) showEngineError(result.error)
            refreshDocument()
        }
    }

    /**
     * Like [command] but suspends and returns the ok payload (null after an error was shown), for
     * tools that need the result or must sequence several commands.
     */
    private suspend fun exec(cmd: JsonObject, redraw: Boolean = true): JsonValue? {
        val result = onEngine { s ->
            val r = s.execute(cmd)
            if (r is EngineResult.Ok && redraw) engine?.renderer?.invalidate(s, dirtyRects(r.value))
            r
        } ?: return null
        refreshDocument()
        return when (result) {
            is EngineResult.Ok -> result.value
            is EngineResult.Err -> {
                showEngineError(result.error)
                null
            }
        }
    }

    /** Read-only engine query; failures are returned as null without bothering the user. */
    private suspend fun query(cmd: JsonObject): JsonObject? =
        (onEngine { s -> s.execute(cmd) } as? EngineResult.Ok)?.value as? JsonObject

    private suspend fun refreshDocument() {
        val pair = onEngine { s -> s.documentInfo().getOrNull() to s.layers().getOrNull() } ?: return
        val (info, layers) = pair
        if (info != null) saveMachine.onDocumentVersion(info.version)
        val hadSelection = _ui.value.doc?.hasSelection == true || _ui.value.selectionOutline.isNotEmpty()
        val before = _ui.value.doc
        _ui.update { it.copy(doc = info ?: it.doc, layers = layers ?: it.layers, saveStatus = saveMachine.status) }
        // Fills, transforms, adjustments… change the document without a stroke: checkpoint those too.
        if (info != null && before != null && info.version != before.version && _ui.value.screen == Screen.EDITOR) scheduleAutosave()
        // Crop / resize (and undoing them) change the canvas size: rebuild the tile display for it.
        if (info != null && before != null && (info.width != before.width || info.height != before.height)) {
            onEngine { s ->
                display.postReset(info.width, info.height)
                engine?.resetRenderer(s, info.width, info.height, display.engineListener)
                s.run(Commands.viewFit())
                engine?.renderer?.let { it.refreshView(s); it.pump(s) }
            }
        }
        if (info != null && (info.hasSelection || hadSelection)) refreshSelectionOutline(info.hasSelection)
    }

    /** Fetches the selection boundary for the marching-ants overlay (empty when nothing is selected). */
    private suspend fun refreshSelectionOutline(hasSelection: Boolean) {
        val contours = if (!hasSelection) emptyList() else query(Commands.selectionOutline())?.let(::parseContours).orEmpty()
        _ui.update { it.copy(selectionOutline = contours) }
    }

    private fun parseContours(o: JsonObject): List<List<PointF>> =
        o.array("contours")?.items.orEmpty().mapNotNull { c ->
            (c as? JsonArray)?.items?.mapNotNull { p ->
                val xy = (p as? JsonArray)?.items ?: return@mapNotNull null
                val x = (xy.getOrNull(0) as? JsonNumber)?.asDouble ?: return@mapNotNull null
                val y = (xy.getOrNull(1) as? JsonNumber)?.asDouble ?: return@mapNotNull null
                PointF(x.toFloat(), y.toFloat())
            }?.takeIf { it.size >= 2 }
        }

    private fun parseBounds(o: JsonObject?): RectF? {
        val b = o?.array("bounds")?.items?.map { (it as? JsonNumber)?.asDouble?.toFloat() ?: return null } ?: return null
        return if (b.size == 4 && b[2] > b[0] && b[3] > b[1]) RectF(b[0], b[1], b[2], b[3]) else null
    }

    private fun publishSaveStatus() = _ui.update { it.copy(saveStatus = saveMachine.status) }

    // ---------------------------------------------------------------- gallery

    fun refreshGallery() {
        viewModelScope.launch {
            val (projects, recovery) = withContext(Dispatchers.IO) { store.listProjects() to store.findRecovery() }
            _ui.update { it.copy(projects = projects, recovery = recovery) }
        }
    }

    fun newCanvas(spec: CanvasSpec, background: HexColor?, name: String) {
        if (spec.validate() != null) return
        // document.create already makes "Background" + an active "Layer 1" (docs/COMMANDS.md).
        loadDocument(name.ifBlank { UiState.DEFAULT_DOC_NAME }, savedDestination = null) { s ->
            s.createDocument(spec.width, spec.height, spec.dpi, background)
        }
    }

    fun openProject(entry: ProjectEntry) {
        loadDocument(entry.name, SaveDestination.AppPrivate(entry.path)) { s -> s.run(Commands.documentOpen(entry.path)) }
    }

    fun openFromSaf(uri: Uri) {
        viewModelScope.launch {
            _ui.update { it.copy(busy = true) }
            val staged = try {
                withContext(Dispatchers.IO) { store.copyFromSaf(uri) }
            } catch (e: IOException) {
                _ui.update { it.copy(busy = false) }
                show(R.string.open_failed)
                return@launch
            }
            val name = withContext(Dispatchers.IO) { store.displayName(uri) }?.substringBeforeLast('.') ?: UiState.DEFAULT_DOC_NAME
            persistUriPermission(uri)
            loadDocument(name, SaveDestination.Saf(uri.toString())) { s ->
                s.run(Commands.documentOpen(staged.absolutePath)).also { staged.delete() }
            }
        }
    }

    fun restoreRecovery() {
        val rec = _ui.value.recovery ?: return
        loadDocument(rec.documentName, savedDestination = null) { s -> s.run(Commands.documentRecover(rec.dir)) }
    }

    fun discardRecovery() {
        viewModelScope.launch {
            withContext(Dispatchers.IO) { store.clearRecovery() }
            refreshGallery()
        }
    }

    /**
     * Runs [load] on the engine thread, then rebuilds the renderer/display for the new document.
     * [savedDestination] != null means the document on disk equals the loaded version (not dirty).
     */
    private fun loadDocument(name: String, savedDestination: SaveDestination?, load: (EngineSession) -> EngineResult<Unit>) {
        viewModelScope.launch {
            _ui.update { it.copy(busy = true) }
            val info = onEngine { s ->
                when (val r = load(s)) {
                    is EngineResult.Err -> r
                    is EngineResult.Ok -> s.documentInfo().also { infoResult ->
                        if (infoResult is EngineResult.Ok) {
                            val i = infoResult.value
                            // Drop old bitmaps BEFORE any tile of the new document is posted (same main queue).
                            display.postReset(i.width, i.height)
                            engine?.resetRenderer(s, i.width, i.height, display.engineListener)
                        }
                    }
                }
            }
            _ui.update { it.copy(busy = false) }
            when (info) {
                null -> return@launch
                is EngineResult.Err -> {
                    show(R.string.open_failed)
                    return@launch
                }
                is EngineResult.Ok -> {
                    saveMachine.reset(info.value.version, if (savedDestination != null) info.value.version else null, savedDestination)
                    _ui.update {
                        it.copy(
                            screen = Screen.EDITOR, docName = name, doc = info.value, sheet = null, export = null,
                            tool = Tool.PAINT, transform = null, selectionOutline = emptyList(), mirrored = false,
                            distort = null, crop = null, symmetry = SymmetryMode.OFF, guide = null, pathPoints = emptyList(),
                            adjustment = Adjustment(), curves = Curves(), viewLocked = false,
                        )
                    }
                    previewSteps = 0
                    applyBrushAndColor()
                    refreshDocument()
                }
            }
        }
    }

    /** A new session starts with engine defaults: re-apply the user's brush, its settings and color. */
    private fun applyBrushAndColor() {
        val st = _ui.value
        command(Commands.brushSelect(st.brush.id), redraw = false)
        command(Commands.brushSet(size = st.brushSize, opacity = st.brushOpacity, stabilization = st.brushStabilization), redraw = false)
        command(Commands.colorSet(st.color), redraw = false)
        command(Commands.toolSet(if (st.smudge) "smudge" else "brush", st.smudgeStrength), redraw = false)
    }

    /** Editor Back: keep a recovery checkpoint if there are unsaved changes, then show the gallery. */
    fun backToGallery() {
        viewModelScope.launch {
            if (saveMachine.isDirty) autosaveNow(resetInput = true) else withContext(Dispatchers.IO) { store.clearRecovery() }
            previewSteps = 0
            _ui.update {
                it.copy(screen = Screen.GALLERY, sheet = null, export = null, tool = Tool.PAINT, transform = null, distort = null, crop = null)
            }
            refreshGallery()
        }
    }

    // ---------------------------------------------------------------- editor tools

    fun openSheet(sheet: EditorSheet?) = _ui.update { it.copy(sheet = sheet) }

    fun toggleRailSide() {
        val left = !_ui.value.railOnLeft
        prefs.railOnLeft = left
        _ui.update { it.copy(railOnLeft = left) }
    }

    /**
     * `brush.select` resets size/opacity/smoothing to the preset's own values, so the sliders are
     * updated from the engine's answer instead of showing the previous brush's numbers.
     */
    fun selectBrush(preset: BrushPreset) {
        if (_ui.value.tool != Tool.PAINT) setTool(Tool.PAINT)
        if (_ui.value.smudge) command(Commands.toolSet("brush"), redraw = false)
        _ui.update {
            it.copy(
                brush = preset,
                smudge = false,
                lastPaintBrush = if (preset.isEraser) it.lastPaintBrush else preset,
                lastEraser = if (preset.isEraser) preset else it.lastEraser,
            )
        }
        viewModelScope.launch {
            val current = (exec(Commands.brushSelect(preset.id), redraw = false) as? JsonObject)?.obj("current") ?: return@launch
            _ui.update {
                it.copy(
                    brushSize = current.double("size")?.toFloat() ?: it.brushSize,
                    brushOpacity = current.double("opacity")?.toFloat() ?: it.brushOpacity,
                    brushStabilization = current.double("stabilization")?.toFloat() ?: it.brushStabilization,
                )
            }
        }
    }

    /** Dock brush button: back to painting with the last non-eraser brush. */
    fun usePaintBrush() {
        val st = _ui.value
        when {
            st.brush.isEraser -> selectBrush(st.lastPaintBrush)
            st.smudge -> {
                _ui.update { it.copy(smudge = false) }
                command(Commands.toolSet("brush"), redraw = false)
                setTool(Tool.PAINT)
            }
            else -> setTool(Tool.PAINT)
        }
    }

    /** Dock eraser button: switch to the last eraser (keeps the paint brush to return to). */
    fun useEraser() {
        val st = _ui.value
        if (!st.brush.isEraser || st.smudge) selectBrush(st.lastEraser) else setTool(Tool.PAINT)
    }

    /**
     * Dock smudge button. Smudging uses the brush size; an eraser preset would win over the smudge
     * tool in the engine, so the last paint brush is selected first.
     */
    fun useSmudge() {
        val st = _ui.value
        if (st.brush.isEraser) selectBrush(st.lastPaintBrush)
        _ui.update { it.copy(smudge = true) }
        command(Commands.toolSet("smudge", st.smudgeStrength), redraw = false)
        setTool(Tool.PAINT)
    }

    fun setSmudgeStrength(v: Float) {
        val s = v.coerceIn(0f, 1f)
        _ui.update { it.copy(smudgeStrength = s) }
        command(Commands.toolSet(if (_ui.value.smudge) "smudge" else "brush", s), redraw = false)
    }

    fun setBrushStabilization(v: Float) {
        val s = v.coerceIn(0f, 1f)
        _ui.update { it.copy(brushStabilization = s) }
        command(Commands.brushSet(stabilization = s), redraw = false)
    }

    fun setBrushSize(size: Float) {
        _ui.update { it.copy(brushSize = size) }
        command(Commands.brushSet(size = size), redraw = false)
    }

    fun setBrushOpacity(opacity: Float) {
        _ui.update { it.copy(brushOpacity = opacity) }
        command(Commands.brushSet(opacity = opacity), redraw = false)
    }

    /** Sets the foreground color. It joins "Recent" once it is actually used on the canvas. */
    fun setColor(c: HexColor) {
        prefs.primaryColor = c
        _ui.update { it.copy(color = c) }
        command(Commands.colorSet(c), redraw = false)
    }

    /** While dragging a picker control: update the chrome only; [setColor] follows when the drag ends. */
    fun previewColor(c: HexColor) = _ui.update { it.copy(color = c) }

    fun swapColors() {
        val st = _ui.value
        prefs.secondaryColor = st.color
        _ui.update { it.copy(secondaryColor = st.color) }
        setColor(st.secondaryColor)
    }

    private fun noteColorUsed() {
        recent.push(_ui.value.color)
        prefs.recentColors = recent.colors
        _ui.update { it.copy(recentColors = recent.colors) }
    }

    fun addToUserPalette(c: HexColor) {
        val next = (_ui.value.userPalette - c + c).takeLast(Palette.MAX_USER_COLORS)
        prefs.userPalette = next
        _ui.update { it.copy(userPalette = next) }
    }

    fun removeFromUserPalette(c: HexColor) {
        val next = _ui.value.userPalette - c
        prefs.userPalette = next
        _ui.update { it.copy(userPalette = next) }
    }

    fun undo() = command(Commands.historyUndo())
    fun redo() = command(Commands.historyRedo())

    // ---------------------------------------------------------------- tools (overlay gestures → commands)

    /** Switching tools keeps whatever the previous tool was previewing (same as its Done button). */
    fun setTool(tool: Tool) {
        val from = _ui.value.tool
        if (from == tool) {
            _ui.update { it.copy(sheet = null) }
            return
        }
        if (from.hasPreview) previewSteps = 0
        _ui.update {
            it.copy(
                tool = tool, sheet = null, transform = null, distort = null, crop = null, pathPoints = emptyList(),
                adjustment = Adjustment(), curves = Curves(),
            )
        }
        when (tool) {
            Tool.TRANSFORM, Tool.DISTORT -> beginTransform(tool)
            Tool.CROP -> {
                val doc = _ui.value.doc ?: return
                _ui.update { it.copy(crop = RectF(0f, 0f, doc.width.toFloat(), doc.height.toFloat())) }
                // Step back a little so the frame's corner handles are not under the screen edges.
                val center = _view.value.docToView(PointF(doc.width / 2f, doc.height / 2f))
                if (!_ui.value.viewLocked) {
                    navigate(center, PointF(0f, 0f), CROP_ZOOM_OUT, 0f)
                    endNavigation()
                }
            }
            else -> Unit
        }
    }

    /** How many undo steps on top of the stack are the current tool's preview. Main thread only. */
    private var previewSteps = 0
    private val previewLock = Mutex()

    /**
     * Replaces the current tool's preview with [cmds] (empty = nothing to apply): undo the previous
     * preview, then run the new one, so the pixels always derive from the original.
     */
    private fun preview(cmds: List<JsonObject>) {
        viewModelScope.launch {
            previewLock.withLock {
                undoPreview()
                for (cmd in cmds) {
                    if (exec(cmd) == null) break
                    previewSteps++
                }
            }
        }
    }

    private fun preview(cmd: JsonObject?) = preview(listOfNotNull(cmd))

    private suspend fun undoPreview() {
        while (previewSteps > 0) {
            previewSteps--
            exec(Commands.historyUndo())
        }
    }

    /** Done ([keep]) or Cancel for the tools with a live preview; returns to painting. */
    fun finishPreviewTool(keep: Boolean) {
        viewModelScope.launch {
            previewLock.withLock {
                if (!keep) undoPreview()
                previewSteps = 0
            }
            _ui.update {
                it.copy(tool = Tool.PAINT, transform = null, distort = null, adjustment = Adjustment(), curves = Curves())
            }
        }
    }

    fun showTextDialog(show: Boolean) = _ui.update { it.copy(textDialog = show, sheet = null) }

    /**
     * Text tool: the text is drawn with the platform text engine in the current color, placed in the
     * middle of the canvas as a new raster layer, and handed to Transform for positioning. It is
     * pixels from then on (not editable text) and exports as a raster layer.
     */
    fun addText(text: String, sizePx: Float) {
        val doc = _ui.value.doc ?: return
        val color = _ui.value.color
        _ui.update { it.copy(textDialog = false, busy = true) }
        viewModelScope.launch {
            val staged = withContext(Dispatchers.Default) {
                try {
                    TextRasterizer.render(text, sizePx, color.toArgb())?.let { bmp ->
                        try {
                            Triple(store.stageBitmapAsPng(bmp), bmp.width, bmp.height)
                        } finally {
                            bmp.recycle()
                        }
                    }
                } catch (e: IOException) {
                    null
                }
            }
            if (staged == null) {
                _ui.update { it.copy(busy = false) }
                show(R.string.text_failed)
                return@launch
            }
            val (file, w, h) = staged
            val name = str(R.string.text_layer_name, text.lineSequence().first().take(24))
            val ok = try {
                noteColorUsed()
                exec(Commands.layerImportImageAt(file.absolutePath, name, (doc.width - w) / 2, (doc.height - h) / 2))
            } finally {
                withContext(Dispatchers.IO) { file.delete() }
                _ui.update { it.copy(busy = false) }
            }
            if (ok != null) setTool(Tool.TRANSFORM)
        }
    }

    fun setSelectionOp(op: SelectionOp) = _ui.update { it.copy(selectionOp = op) }
    fun setFillOptions(o: FillOptions) = _ui.update { it.copy(fill = o) }

    /** Eyedropper: [commit] = finger lifted (back to painting with the picked color). */
    fun pickColor(doc: PointF, commit: Boolean) {
        viewModelScope.launch {
            val hex = query(Commands.colorPick(doc.x, doc.y))?.string("color")?.let(HexColor::parse) ?: return@launch
            if (commit) prefs.primaryColor = hex
            _ui.update { it.copy(color = hex, tool = if (commit) Tool.PAINT else it.tool) }
        }
    }

    fun fillAt(doc: PointF) {
        val o = _ui.value.fill
        noteColorUsed()
        command(Commands.editFill(doc.x, doc.y, o.tolerance, o.contiguous, o.sampleMerged))
    }

    fun selectRect(r: RectF) = command(Commands.selectionRect(r.left, r.top, r.right, r.bottom, _ui.value.selectionOp))
    fun selectEllipse(r: RectF) = command(Commands.selectionEllipse(r.left, r.top, r.right, r.bottom, _ui.value.selectionOp))

    fun selectLasso(points: List<PointF>) {
        if (points.size >= 3) command(Commands.selectionLasso(points, _ui.value.selectionOp))
    }

    fun selectAll() = command(Commands.selectionAll())
    fun clearSelection() = command(Commands.selectionClear())
    fun invertSelection() = command(Commands.selectionInvert())
    fun deleteSelection() = command(Commands.editDeleteSelection())

    fun fillSelection() {
        noteColorUsed()
        command(Commands.editFillSelection())
    }

    /** Transform / Distort work on the selection if there is one, else on the active layer's content. */
    private fun beginTransform(tool: Tool) {
        viewModelScope.launch {
            val doc = _ui.value.doc ?: return@launch
            val bounds = (if (doc.hasSelection) parseBounds(query(Commands.selectionOutline())) else null)
                ?: parseBounds(query(Commands.layerBounds()))
                ?: RectF(0f, 0f, doc.width.toFloat(), doc.height.toFloat())
            _ui.update {
                when {
                    it.tool != tool -> it
                    tool == Tool.TRANSFORM -> it.copy(transform = TransformSession(bounds))
                    else -> it.copy(distort = DistortSession(bounds))
                }
            }
        }
    }

    /** Overlay drag in progress: move the handle box only (no engine work). */
    fun previewTransform(matrix: Affine) = _ui.update { st -> st.copy(transform = st.transform?.copy(matrix = matrix)) }

    /** Overlay drag finished: resample the pixels once from the original with the accumulated matrix. */
    fun applyTransform(matrix: Affine) {
        if (_ui.value.transform == null) return
        previewTransform(matrix)
        preview(if (matrix.isIdentity) null else Commands.editTransform(matrix))
    }

    fun previewDistort(quad: List<PointF>) = _ui.update { st -> st.copy(distort = st.distort?.copy(quad = quad)) }

    fun applyDistort(quad: List<PointF>) {
        val d = _ui.value.distort ?: return
        previewDistort(quad)
        preview(if (quad == d.source.corners) null else Commands.editTransformQuad(quad))
    }

    // ---- shapes, gradient, wand

    fun setShapeOptions(o: ShapeOptions) = _ui.update { it.copy(shape = o) }
    fun setGradientRadial(radial: Boolean) = _ui.update { it.copy(gradientRadial = radial) }

    /** A two-point drag finished ([a] = press, [b] = release, document px): run the active tool on it. */
    fun dragTool(a: PointF, b: PointF) {
        val st = _ui.value
        val r = RectF.of(a, b)
        when (st.tool) {
            Tool.SELECT_RECT -> if (r.width >= 1f && r.height >= 1f) selectRect(r)
            Tool.SELECT_ELLIPSE -> if (r.width >= 1f && r.height >= 1f) selectEllipse(r)
            Tool.SHAPE_LINE -> if (a.distanceTo(b) >= 1f) shape(Commands.shapeLine(a, b))
            Tool.SHAPE_RECT -> if (r.width >= 1f && r.height >= 1f) shape(Commands.shapeRect(r.left, r.top, r.right, r.bottom, st.shape.fill))
            Tool.SHAPE_ELLIPSE -> if (r.width >= 1f && r.height >= 1f) shape(Commands.shapeEllipse(r.left, r.top, r.right, r.bottom, st.shape.fill))
            Tool.GRADIENT -> if (a.distanceTo(b) >= 1f) {
                noteColorUsed()
                command(Commands.editGradient(a, b, st.gradientRadial, listOf(0f to st.color, 1f to st.secondaryColor)))
            }
            Tool.GUIDE_LINE -> if (a.distanceTo(b) >= 1f) setGuide(Guide.Line(a, b))
            Tool.GUIDE_ELLIPSE -> if (r.width >= 1f && r.height >= 1f) setGuide(Guide.Ellipse(r))
            else -> Unit
        }
    }

    private fun shape(cmd: JsonObject) {
        noteColorUsed()
        command(cmd)
    }

    fun wandAt(doc: PointF) {
        val st = _ui.value
        command(Commands.selectionWand(doc.x, doc.y, st.fill.tolerance, st.fill.contiguous, st.fill.sampleMerged, st.selectionOp))
    }

    fun addPathPoint(doc: PointF) = _ui.update { it.copy(pathPoints = (it.pathPoints + doc).takeLast(MAX_PATH_POINTS)) }
    fun removeLastPathPoint() = _ui.update { it.copy(pathPoints = it.pathPoints.dropLast(1)) }

    /** Draws the placed points as one stroke with the current brush, then starts a fresh path. */
    fun commitPath() {
        val st = _ui.value
        // A closed path needs at least three points; two points are drawn as an open segment.
        val closed = st.shape.closed && st.pathPoints.size >= 3
        if (st.pathPoints.size >= 2) shape(Commands.shapePath(st.pathPoints, closed, st.shape.smooth, st.shape.fill && closed))
        _ui.update { it.copy(pathPoints = emptyList()) }
    }

    // ---- symmetry, guides, camera lock

    fun setSymmetry(mode: SymmetryMode, count: Int = _ui.value.symmetryCount) {
        val n = count.coerceIn(2, 16)
        _ui.update { it.copy(symmetry = mode, symmetryCount = n) }
        command(Commands.symmetrySet(mode.wire, n), redraw = false)
    }

    fun setGuide(guide: Guide?) {
        _ui.update { it.copy(guide = guide, tool = if (it.tool == Tool.GUIDE_LINE || it.tool == Tool.GUIDE_ELLIPSE) Tool.PAINT else it.tool) }
        command(
            when (guide) {
                null -> Commands.guideOff()
                is Guide.Line -> Commands.guideLine(guide.a, guide.b)
                is Guide.Ellipse -> Commands.guideEllipse(guide.bounds.left, guide.bounds.top, guide.bounds.right, guide.bounds.bottom)
            },
            redraw = false,
        )
    }

    fun toggleViewLock() {
        val locked = !_ui.value.viewLocked
        _ui.update { it.copy(viewLocked = locked) }
        command(Commands.inputConfigLockView(locked), redraw = false)
    }

    // ---- crop / resize

    fun setCrop(r: RectF) = _ui.update { it.copy(crop = r) }

    fun applyCrop() {
        val st = _ui.value
        val doc = st.doc ?: return
        val r = st.crop ?: return
        val x0 = r.left.toInt().coerceIn(0, doc.width - 1)
        val y0 = r.top.toInt().coerceIn(0, doc.height - 1)
        val x1 = kotlin.math.ceil(r.right).toInt().coerceIn(x0 + 1, doc.width)
        val y1 = kotlin.math.ceil(r.bottom).toInt().coerceIn(y0 + 1, doc.height)
        _ui.update { it.copy(tool = Tool.PAINT, crop = null) }
        if (x0 != 0 || y0 != 0 || x1 != doc.width || y1 != doc.height) command(Commands.canvasCrop(x0, y0, x1, y1))
    }

    fun showResizeDialog(show: Boolean) = _ui.update { it.copy(resizeDialog = show, sheet = null) }

    fun resizeCanvas(width: Int, height: Int, resample: Boolean) {
        _ui.update { it.copy(resizeDialog = false) }
        val doc = _ui.value.doc ?: return
        if (CanvasSpec(width, height, doc.dpi).validate() != null) return
        if (width != doc.width || height != doc.height) command(Commands.canvasResize(width, height, resample))
    }

    // ---- color adjustments

    fun previewAdjustment(a: Adjustment) = _ui.update { it.copy(adjustment = a) }

    /** A slider was released: show the result on the canvas. */
    fun applyAdjustment(a: Adjustment) {
        previewAdjustment(a)
        preview(
            listOfNotNull(
                if (a.hue != 0f || a.saturation != 0f) Commands.editAdjustHsb(a.hue, a.saturation, 0f) else null,
                if (a.brightness != 0f || a.contrast != 0f) Commands.editAdjustBrightnessContrast(a.brightness, a.contrast) else null,
                if (a.invert) Commands.editAdjustInvert() else null,
            ),
        )
    }

    fun previewCurves(c: Curves) = _ui.update { it.copy(curves = c) }

    fun applyCurves(c: Curves) {
        previewCurves(c)
        preview(if (c.isNeutral) null else Commands.editAdjustCurves(c.points.filterValues { it != Curves.IDENTITY }.mapKeys { it.key.wire }))
    }

    /** View placement being built up by an in-progress two-finger gesture (main thread only). */
    private var navView: NavView? = null

    private data class NavView(val zoom: Float, val rotation: Float, val panX: Float, val panY: Float)

    /**
     * Two-finger pan / pinch / twist while a tool overlay owns the canvas. [centroid] and [pan] are
     * view px, [rotation] radians; the document point under the fingers stays under them. Deltas are
     * accumulated locally because the engine's answer ([view]) arrives a frame later.
     */
    fun navigate(centroid: PointF, pan: PointF, zoom: Float, rotation: Float) {
        if (_ui.value.viewLocked || !zoom.isFinite() || zoom <= 0f || !rotation.isFinite()) return
        val v = navView ?: _view.value.let { NavView(it.zoom, it.rotation, it.e, it.f) }
        val cs = kotlin.math.cos(rotation)
        val sn = kotlin.math.sin(rotation)
        val px = centroid.x - pan.x - v.panX
        val py = centroid.y - pan.y - v.panY
        val newZoom = (v.zoom * zoom).coerceIn(MIN_ZOOM, MAX_ZOOM)
        val k = newZoom / v.zoom
        val next = NavView(
            zoom = newZoom,
            rotation = v.rotation + rotation,
            panX = centroid.x - k * (cs * px - sn * py),
            panY = centroid.y - k * (sn * px + cs * py),
        )
        if (!next.panX.isFinite() || !next.panY.isFinite()) return
        navView = next
        viewModelScope.launch {
            onEngine { s ->
                s.run(Commands.viewSet(zoom = next.zoom, rotation = next.rotation, panX = next.panX, panY = next.panY))
                engine?.renderer?.let { it.refreshView(s); it.pump(s) }
            }
        }
    }

    fun endNavigation() {
        navView = null
    }

    fun fitView() {
        viewModelScope.launch {
            onEngine { s ->
                s.run(Commands.viewFit())
                engine?.renderer?.let { it.refreshView(s); it.pump(s) }
            }
        }
    }

    fun toggleMirror() {
        val m = !_ui.value.mirrored
        _ui.update { it.copy(mirrored = m) }
        viewModelScope.launch {
            onEngine { s ->
                s.run(Commands.viewMirror(m))
                engine?.renderer?.let { it.refreshView(s); it.pump(s) }
            }
        }
    }

    // ---------------------------------------------------------------- layers

    fun addLayer(kind: LayerKind) = command(Commands.layerAdd(kind, nextLayerName()))

    /** "Layer N" with the smallest unused N at or above the layer count, so names never collide. */
    private fun nextLayerName(): String {
        val names = _ui.value.layers.map { it.name }.toSet()
        var n = maxOf(1, _ui.value.layers.size)
        while (str(R.string.layers_default_name, n) in names) n++
        return str(R.string.layers_default_name, n)
    }

    fun clearLayer(id: String) = command(Commands.layerClear(id))
    fun maskLayerFromSelection(id: String) = command(Commands.layerMaskFromSelection(id))
    fun removeLayerMask(id: String) = command(Commands.layerMaskRemove(id))
    fun applyLayerMask(id: String) = command(Commands.layerMaskApply(id))

    /**
     * SAF image → new raster layer. Any format Android can decode is re-encoded as PNG in the cache
     * first (the engine imports PNG only); the copy is deleted afterwards.
     */
    fun importImage(uri: Uri) {
        viewModelScope.launch {
            _ui.update { it.copy(busy = true, sheet = null) }
            val staged = withContext(Dispatchers.IO) {
                try {
                    store.stageImageAsPng(uri)
                } catch (e: IOException) {
                    null
                }
            }
            if (staged == null) {
                _ui.update { it.copy(busy = false) }
                show(R.string.import_failed)
                return@launch
            }
            val name = withContext(Dispatchers.IO) { store.displayName(uri) }?.substringBeforeLast('.') ?: nextLayerName()
            try {
                exec(Commands.layerImportImageAt(staged.absolutePath, name.take(MAX_LAYER_NAME_CHARS), 0, 0))
            } finally {
                withContext(Dispatchers.IO) { staged.delete() }
                _ui.update { it.copy(busy = false) }
            }
        }
    }
    fun removeLayer(id: String) = command(Commands.layerRemove(id))
    fun duplicateLayer(id: String) = command(Commands.layerDuplicate(id))
    fun selectLayer(id: String) = command(Commands.layerSelect(id), redraw = false)
    fun renameLayer(id: String, name: String) = command(Commands.layerRename(id, name.trim()), redraw = false)
    fun setLayerVisible(id: String, visible: Boolean) = command(Commands.layerSet(id, visible = visible))
    fun setLayerOpacity(id: String, opacity: Float) = command(Commands.layerSet(id, opacity = opacity.coerceIn(0f, 1f)))
    fun setLayerBlend(id: String, blend: BlendMode) = command(Commands.layerSet(id, blend = blend))
    fun setLayerAlphaLock(id: String, on: Boolean) = command(Commands.layerSet(id, alphaLock = on), redraw = false)
    fun setLayerClip(id: String, on: Boolean) = command(Commands.layerSet(id, clip = on))

    /** [layers] in UiState are top-most first; move relative to the visual neighbour. */
    fun moveLayer(id: String, up: Boolean) {
        val layers = _ui.value.layers
        val pos = layers.indexOfFirst { it.id == id }
        if (pos < 0) return
        if (up) {
            val neighbour = layers.getOrNull(pos - 1) ?: return
            command(Commands.layerMoveAbove(id, neighbour.id))
        } else {
            val neighbour = layers.getOrNull(pos + 1) ?: return
            command(Commands.layerMoveBelow(id, neighbour.id))
        }
    }

    // ---------------------------------------------------------------- history / autosave

    private fun onHistoryChanged(strokeCommitted: Boolean) {
        if (strokeCommitted && !_ui.value.brush.isEraser && !_ui.value.smudge) noteColorUsed()
        viewModelScope.launch { refreshDocument() }
        if (strokeCommitted) scheduleAutosave()
    }

    /** Committed work is recoverable within the 30 s target (MASTER_PLAN §8.1), whatever tool made it. */
    private fun scheduleAutosave() {
        if (autosaveJob?.isActive == true) return
        autosaveJob = viewModelScope.launch {
            delay(AUTOSAVE_DELAY_MS)
            autosaveNow(resetInput = false)
        }
    }

    /** Lifecycle ON_PAUSE: settle any in-flight stroke and checkpoint, all off the UI thread. */
    fun onPause() {
        if (_ui.value.screen != Screen.EDITOR) return
        viewModelScope.launch { autosaveNow(resetInput = true) }
    }

    private suspend fun autosaveNow(resetInput: Boolean) {
        val dir = store.recoveryDir.absolutePath
        val r = onEngine { s ->
            if (resetInput) {
                s.run(Commands.inputReset())
                engine?.renderer?.invalidate(s, null)
            }
            s.run(Commands.documentAutosave(dir))
        } ?: return
        if (r is EngineResult.Ok) {
            val name = _ui.value.docName
            withContext(Dispatchers.IO) {
                try {
                    store.writeRecoveryMeta(name, System.currentTimeMillis())
                } catch (_: IOException) {
                    // Checkpoint exists without meta; next autosave retries. Not user-facing.
                }
            }
        }
    }

    // ---------------------------------------------------------------- save (staged, SAV-002)

    /** Save to the last destination, or into the app-private projects folder for a new document. */
    fun save() {
        val dest = saveMachine.lastDestination
            ?: SaveDestination.AppPrivate(store.projectFileFor(_ui.value.docName).absolutePath)
        runSave(dest)
    }

    /** SAF "Save a copy as .veskra" result. */
    fun saveAs(uri: Uri) {
        persistUriPermission(uri)
        runSave(SaveDestination.Saf(uri.toString()))
    }

    private fun runSave(dest: SaveDestination) {
        val plan = saveMachine.begin(dest, store.saveStagingDir.absolutePath)
        publishSaveStatus()
        if (plan == null) return // coalesced into a follow-up save
        viewModelScope.launch {
            executePlan(plan)
            if (saveMachine.consumeFollowUp()) saveMachine.lastDestination?.let(::runSave)
        }
    }

    private suspend fun executePlan(plan: SafCopyPlan) {
        val staging = File(plan.stagingPath)
        var step: SaveStep? = plan.steps.first()
        while (step != null) {
            val failure: Pair<SaveFailureReason, String>? = when (step) {
                SaveStep.ENCODE_TO_STAGING -> engineStep(SaveFailureReason.ENCODE_FAILED) { s -> s.run(Commands.documentSave(staging.absolutePath)) }
                SaveStep.VALIDATE_STAGING -> engineStep(SaveFailureReason.VALIDATION_FAILED) { s -> s.run(Commands.documentValidate(staging.absolutePath)) }
                SaveStep.COPY_TO_DESTINATION -> ioStep {
                    val uri = Uri.parse((plan.destination as SaveDestination.Saf).uri)
                    store.copyToSaf(staging, uri)
                }
                SaveStep.ATOMIC_RENAME -> ioStep {
                    store.atomicMove(staging, File((plan.destination as SaveDestination.AppPrivate).path))
                }
                SaveStep.CLEANUP_STAGING -> ioStep {
                    if (staging.exists() && !staging.delete()) throw IOException("cleanup failed")
                }
            }
            if (failure != null) {
                saveMachine.onStepFailed(plan.jobId, step, failure.first, failure.second)
                publishSaveStatus()
                // The staging file is deliberately kept on copy failure: the validated bytes are a
                // second good copy until the next successful save (ARCHITECTURE §G step 4).
                return
            }
            step = saveMachine.onStepSucceeded(plan.jobId, step)
            publishSaveStatus()
        }
        // Saved (and committed): the recovery checkpoint is no longer needed if nothing changed since.
        if (!saveMachine.isDirty) withContext(Dispatchers.IO) { store.clearRecovery() }
        if (plan.destination is SaveDestination.AppPrivate) refreshGallery()
    }

    private suspend fun engineStep(reason: SaveFailureReason, block: (EngineSession) -> EngineResult<Unit>): Pair<SaveFailureReason, String>? =
        when (val r = onEngine(block)) {
            null -> reason to "engine unavailable"
            is EngineResult.Err -> reason to r.error.code
            is EngineResult.Ok -> null
        }

    private suspend fun ioStep(block: () -> Unit): Pair<SaveFailureReason, String>? = withContext(Dispatchers.IO) {
        try {
            block()
            null
        } catch (e: CopyFailedException) {
            e.reason to (e.message ?: "")
        } catch (e: SecurityException) {
            SaveFailureReason.PERMISSION_DENIED to "permission"
        } catch (e: IOException) {
            SaveFailureReason.PROVIDER_WRITE_FAILED to (e.message ?: "")
        }
    }

    private fun persistUriPermission(uri: Uri) {
        try {
            getApplication<Application>().contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
        } catch (_: SecurityException) {
            // Provider does not offer persistable grants; saving again will ask for a location.
        }
    }

    // ---------------------------------------------------------------- export (MASTER_PLAN §2.3)

    fun openExport(destination: ExportDestination = ExportDestination.ILLUSTRATOR) {
        _ui.update { it.copy(sheet = EditorSheet.EXPORT, export = ExportSheetState.initial(destination, prefs.exportPreferences)) }
        maybePreflight()
    }

    fun onExportEvent(e: ExportSheetEvent) {
        _ui.update { st -> st.copy(export = st.export?.reduce(e)) }
        maybePreflight()
    }

    fun selectExportDestination(d: ExportDestination) = onExportEvent(ExportSheetEvent.SelectDestination(d, prefs.exportPreferences))

    private fun maybePreflight() {
        val st = _ui.value.export ?: return
        if (!st.needsPreflight) return
        val key = st.preflightKey
        onExportEventNoPreflight(ExportSheetEvent.PreflightStarted(key))
        viewModelScope.launch {
            val r = onEngine { s -> s.exportPreflight(key.format, key.options) } ?: return@launch
            onExportEventNoPreflight(
                when (r) {
                    is EngineResult.Ok -> ExportSheetEvent.PreflightLoaded(key, r.value)
                    is EngineResult.Err -> ExportSheetEvent.PreflightFailed(key, r.error)
                },
            )
        }
    }

    private fun onExportEventNoPreflight(e: ExportSheetEvent) = _ui.update { st -> st.copy(export = st.export?.reduce(e)) }

    fun suggestedExportName(): String? = _ui.value.export?.let { ExportFileNames.suggest(_ui.value.docName, it.format) }

    /** SAF CreateDocument result for the export. Success is reported only after validation + stream close. */
    fun exportTo(uri: Uri) {
        val st = _ui.value.export ?: return
        if (!st.canExport) return
        prefs.exportPreferences = prefs.exportPreferences.with(st.destination, st.prefsToRemember())
        onExportEventNoPreflight(ExportSheetEvent.ExportStarted)
        val format = st.format
        val options = st.effectiveOptions
        viewModelScope.launch {
            val tmp = File(store.exportStagingDir, "export-${System.nanoTime()}.${format.extension}")
            try {
                // NOTE: export.run executes on the engine thread and blocks input meanwhile; an async
                // export job API (export.start/progress/cancel, ARCHITECTURE §C) is a contract question.
                val r = onEngine { s -> s.exportRun(format, tmp.absolutePath, options) }
                when {
                    r == null -> onExportEventNoPreflight(ExportSheetEvent.ExportFailed(str(R.string.engine_unavailable_title)))
                    r is EngineResult.Err -> onExportEventNoPreflight(ExportSheetEvent.ExportFailed(r.error.message.ifBlank { r.error.code }))
                    r is EngineResult.Ok && !r.value.isTrustworthySuccess -> onExportEventNoPreflight(ExportSheetEvent.ExportFinished(r.value))
                    r is EngineResult.Ok -> {
                        val copyFailure = ioStep { store.copyToSaf(tmp, uri) }
                        onExportEventNoPreflight(
                            if (copyFailure == null) {
                                ExportSheetEvent.ExportFinished(r.value)
                            } else {
                                ExportSheetEvent.ExportFailed(str(saveFailureText(copyFailure.first)))
                            },
                        )
                    }
                }
            } finally {
                withContext(Dispatchers.IO) { tmp.delete() }
            }
        }
    }

    fun closeExport() = _ui.update { it.copy(sheet = null, export = null) }

    companion object {
        private const val AUTOSAVE_DELAY_MS = 15_000L
        private const val MAX_LAYER_NAME_CHARS = 60
        private const val MIN_ZOOM = 0.02f
        private const val MAX_PATH_POINTS = 2000
        private const val CROP_ZOOM_OUT = 0.8f
        private const val MAX_ZOOM = 64f

        @StringRes
        fun saveFailureText(reason: SaveFailureReason): Int = when (reason) {
            SaveFailureReason.PERMISSION_DENIED -> R.string.save_failed_permission
            SaveFailureReason.STORAGE_FULL -> R.string.save_failed_storage_full
            SaveFailureReason.PROVIDER_WRITE_FAILED -> R.string.save_failed_provider
            SaveFailureReason.VALIDATION_FAILED, SaveFailureReason.ENCODE_FAILED -> R.string.save_failed_validation
            SaveFailureReason.CANCELLED, SaveFailureReason.UNKNOWN -> R.string.save_failed_generic
        }
    }
}
