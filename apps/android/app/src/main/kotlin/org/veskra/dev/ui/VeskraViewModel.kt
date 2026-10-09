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
import kotlinx.coroutines.withContext
import org.veskra.dev.R
import org.veskra.dev.VeskraApplication
import org.veskra.dev.canvas.CanvasDisplay
import org.veskra.dev.engine.EngineHost
import org.veskra.dev.storage.AppPreferences
import org.veskra.dev.storage.ProjectEntry
import org.veskra.dev.storage.ProjectStore
import org.veskra.dev.storage.RecoveryEntry
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
import org.veskra.kit.RecentColors
import org.veskra.kit.RectI
import org.veskra.kit.SaveDestination
import org.veskra.kit.SaveFailureReason
import org.veskra.kit.SaveStateMachine
import org.veskra.kit.SaveStatus
import org.veskra.kit.SaveStep
import org.veskra.kit.SafCopyPlan
import java.io.File
import java.io.IOException

enum class Screen { GALLERY, EDITOR }

enum class EditorSheet { BRUSH, COLOR, LAYERS, EXPORT }

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
    val color: HexColor = HexColor.BLACK,
    val recentColors: List<HexColor> = emptyList(),
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
        UiState(engineAvailable = engine != null, railOnLeft = prefs.railOnLeft, recentColors = recent.colors),
    )
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    private var autosaveJob: Job? = null

    init {
        display.historyListener = { committed -> onHistoryChanged(committed) }
        refreshGallery()
    }

    override fun onCleared() {
        display.historyListener = null
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

    private suspend fun refreshDocument() {
        val pair = onEngine { s -> s.documentInfo().getOrNull() to s.layers().getOrNull() } ?: return
        val (info, layers) = pair
        if (info != null) saveMachine.onDocumentVersion(info.version)
        _ui.update { it.copy(doc = info ?: it.doc, layers = layers ?: it.layers, saveStatus = saveMachine.status) }
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
        loadDocument(name.ifBlank { UiState.DEFAULT_DOC_NAME }, savedDestination = null) { s ->
            s.createDocument(spec.width, spec.height, spec.dpi, background).let { r ->
                if (r is EngineResult.Ok) s.run(Commands.layerAdd(LayerKind.RASTER, str(R.string.layers_default_name, 1))) else r
            }
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
                    _ui.update { it.copy(screen = Screen.EDITOR, docName = name, doc = info.value, sheet = null, export = null) }
                    applyBrushAndColor()
                    refreshDocument()
                }
            }
        }
    }

    private fun applyBrushAndColor() {
        val st = _ui.value
        command(Commands.brushSelect(st.brush.id), redraw = false)
        command(Commands.brushSet(size = st.brushSize, opacity = st.brushOpacity), redraw = false)
        command(Commands.colorSet(st.color), redraw = false)
    }

    /** Editor Back: keep a recovery checkpoint if there are unsaved changes, then show the gallery. */
    fun backToGallery() {
        viewModelScope.launch {
            if (saveMachine.isDirty) autosaveNow(resetInput = true) else withContext(Dispatchers.IO) { store.clearRecovery() }
            _ui.update { it.copy(screen = Screen.GALLERY, sheet = null, export = null) }
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

    fun selectBrush(preset: BrushPreset) {
        _ui.update { it.copy(brush = preset) }
        command(Commands.brushSelect(preset.id), redraw = false)
    }

    fun setBrushSize(size: Float) {
        _ui.update { it.copy(brushSize = size) }
        command(Commands.brushSet(size = size), redraw = false)
    }

    fun setBrushOpacity(opacity: Float) {
        _ui.update { it.copy(brushOpacity = opacity) }
        command(Commands.brushSet(opacity = opacity), redraw = false)
    }

    fun setColor(c: HexColor) {
        recent.push(c)
        prefs.recentColors = recent.colors
        _ui.update { it.copy(color = c, recentColors = recent.colors) }
        command(Commands.colorSet(c), redraw = false)
    }

    fun undo() = command(Commands.historyUndo())
    fun redo() = command(Commands.historyRedo())

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

    fun addLayer(kind: LayerKind) = command(Commands.layerAdd(kind, str(R.string.layers_default_name, _ui.value.layers.size + 1)))
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
        viewModelScope.launch { refreshDocument() }
        if (strokeCommitted && autosaveJob?.isActive != true) {
            autosaveJob = viewModelScope.launch {
                delay(AUTOSAVE_DELAY_MS) // committed strokes recoverable within the 30 s target (MASTER_PLAN §8.1)
                autosaveNow(resetInput = false)
            }
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
