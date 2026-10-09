package org.veskra.kit

import java.io.Closeable
import java.nio.ByteBuffer

/** Typed engine error (`{"error":{code,message,recoverable}}`, ARCHITECTURE §J). */
data class EngineError(val code: String, val message: String, val recoverable: Boolean) {
    companion object {
        const val CODE_INTERNAL: String = "Internal"
        const val CODE_INVALID_STATE: String = "InvalidState"

        fun internal(message: String): EngineError = EngineError(CODE_INTERNAL, message, false)
    }
}

class EngineException(val error: EngineError) : Exception("${error.code}: ${error.message}")

sealed interface EngineResult<out T> {
    data class Ok<T>(val value: T) : EngineResult<T>
    data class Err(val error: EngineError) : EngineResult<Nothing>

    fun getOrNull(): T? = (this as? Ok<T>)?.value

    fun getOrThrow(): T = when (this) {
        is Ok -> value
        is Err -> throw EngineException(error)
    }

    companion object {
        /** Parses a `nativeExecute` response. Malformed responses become `Internal` errors, never crashes. */
        fun parse(response: String): EngineResult<JsonValue> {
            val root = try {
                Json.parseObject(response)
            } catch (e: JsonParseException) {
                return Err(EngineError.internal("malformed engine response: ${e.message}"))
            }
            val err = root.obj("error")
            if (err != null) {
                return Err(
                    EngineError(
                        code = err.string("code") ?: EngineError.CODE_INTERNAL,
                        message = err.string("message") ?: "",
                        recoverable = err.bool("recoverable") ?: false,
                    ),
                )
            }
            val ok = root["ok"] ?: return Err(EngineError.internal("engine response has neither 'ok' nor 'error'"))
            return Ok(ok)
        }
    }
}

inline fun <T, R> EngineResult<T>.map(f: (T) -> R?): EngineResult<R> = when (this) {
    is EngineResult.Ok -> f(value)?.let { EngineResult.Ok(it) }
        ?: EngineResult.Err(EngineError.internal("unexpected engine result shape"))
    is EngineResult.Err -> this
}

/**
 * Decoded `nativeInputBatch` return value `[flags, nRects, x0,y0,x1,y1, …]`. Rects are
 * document-space, half-open (x1/y1 exclusive — see COMMANDS.md "Open contract questions").
 * A malformed array is treated as "everything dirty" so the display can never go stale.
 */
class InputBatchResult(raw: IntArray?) {
    val flags: Int
    val rects: List<RectI>
    val malformed: Boolean

    init {
        val valid: IntArray? = raw?.takeIf { r ->
            r.size >= 2 && r[1] >= 0 && r[1] <= (r.size - 2) / 4 && r.size == 2 + r[1] * 4
        }
        malformed = valid == null
        if (valid != null) {
            flags = valid[0]
            rects = (0 until valid[1]).map { i ->
                val b = 2 + i * 4
                RectI(valid[b], valid[b + 1], valid[b + 2], valid[b + 3])
            }
        } else {
            flags = NativeBridge.BatchFlags.CANVAS_DIRTY
            rects = emptyList()
        }
    }

    val canvasDirty: Boolean get() = flags and NativeBridge.BatchFlags.CANVAS_DIRTY != 0
    val viewChanged: Boolean get() = flags and NativeBridge.BatchFlags.VIEW_CHANGED != 0
    val strokeCommitted: Boolean get() = flags and NativeBridge.BatchFlags.STROKE_COMMITTED != 0
    val historyChanged: Boolean get() = flags and NativeBridge.BatchFlags.HISTORY_CHANGED != 0

    /** Dirty canvas without rect detail (or malformed) → caller must refresh everything. */
    val fullRefresh: Boolean get() = malformed || (canvasDirty && rects.isEmpty())
}

/**
 * Typed, closeable wrapper over one native session. Confined to a single thread (the app's engine
 * thread) when [confineToCreatingThread] is true; violations throw instead of racing native state.
 */
class EngineSession private constructor(
    private val bridge: EngineBridge,
    handle: Long,
    private val confineToCreatingThread: Boolean,
) : Closeable {
    private var handle: Long = handle
    private val ownerThread: Thread = Thread.currentThread()
    private val viewScratch = FloatArray(NativeBridge.VIEW_FLOATS)

    val isClosed: Boolean get() = handle == 0L

    companion object {
        /** Creates a native session. Throws [EngineException] if the engine returns handle 0. */
        fun open(bridge: EngineBridge = JniEngineBridge, confineToCreatingThread: Boolean = false): EngineSession {
            val h = bridge.createSession()
            if (h == 0L) throw EngineException(EngineError.internal("nativeCreateSession returned 0"))
            return EngineSession(bridge, h, confineToCreatingThread)
        }
    }

    private fun live(): Long {
        if (confineToCreatingThread) {
            check(Thread.currentThread() === ownerThread) { "EngineSession used off its owner thread" }
        }
        check(handle != 0L) { "EngineSession is closed" }
        return handle
    }

    /** Sends one command object. */
    fun execute(cmd: JsonObject): EngineResult<JsonValue> {
        val h = live()
        val response = try {
            bridge.execute(h, cmd.encode())
        } catch (e: RuntimeException) {
            return EngineResult.Err(EngineError.internal("native execute threw ${e.javaClass.simpleName}"))
        } ?: return EngineResult.Err(EngineError.internal("native execute returned null"))
        return EngineResult.parse(response)
    }

    /** Commands whose `ok` payload is ignored. */
    fun run(cmd: JsonObject): EngineResult<Unit> = execute(cmd).map { }

    fun createDocument(width: Int, height: Int, dpi: Int, background: HexColor?): EngineResult<Unit> {
        CanvasSpec(width, height, dpi).validate()?.let { return EngineResult.Err(EngineError("InvalidArgument", it.name, true)) }
        return run(Commands.documentCreate(width, height, dpi, background?.toHex()))
    }

    fun documentInfo(): EngineResult<DocumentInfo> = execute(Commands.documentInfo()).map { (it as? JsonObject)?.let(DocumentInfo::fromJson) }

    fun layers(): EngineResult<List<LayerInfo>> = execute(Commands.layerList()).map { (it as? JsonObject)?.let(LayerInfo::listFromJson) }

    fun undo(): EngineResult<Unit> = run(Commands.historyUndo())
    fun redo(): EngineResult<Unit> = run(Commands.historyRedo())

    fun exportPreflight(format: ExportFormat, options: ExportOptions): EngineResult<ExportPreflightInfo> =
        execute(Commands.exportPreflight(format, options)).map { (it as? JsonObject)?.let(ExportPreflightInfo::fromJson) }

    fun exportRun(format: ExportFormat, path: String, options: ExportOptions): EngineResult<ExportReportInfo> =
        execute(Commands.exportRun(format, path, options)).map { (it as? JsonObject)?.let(ExportReportInfo::fromJson) }

    /** Sends a packed batch; the packer is NOT cleared (the caller owns/recycles it). */
    fun inputBatch(p: PointerPacker): InputBatchResult {
        val h = live()
        if (p.isEmpty) return InputBatchResult(intArrayOf(0, 0))
        return InputBatchResult(bridge.inputBatch(h, p.ints, p.floats, p.longs, p.count))
    }

    fun setViewport(width: Int, height: Int) {
        require(width > 0 && height > 0) { "viewport must be non-empty" }
        bridge.setViewport(live(), width, height)
    }

    /** Current view, or null if the engine wrote non-finite values. */
    fun view(): ViewState? {
        bridge.getView(live(), viewScratch)
        return ViewState.fromFloats(viewScratch)
    }

    /**
     * Renders [region] (document px) into [dst] at position 0. Validates buffer kind and size on the
     * Kotlin side so native code never writes past a buffer.
     */
    fun renderRegion(region: RectI, dst: ByteBuffer): Boolean {
        require(!region.isEmpty) { "empty region" }
        require(dst.isDirect) { "nativeRenderRegion requires a direct ByteBuffer" }
        val bytes = Math.multiplyExact(Math.multiplyExact(region.width, region.height), 4)
        require(dst.capacity() >= bytes) { "buffer too small: ${dst.capacity()} < $bytes" }
        return bridge.renderRegion(live(), region.left, region.top, region.width, region.height, dst)
    }

    /** Idempotent. Safe to call from the owner thread only when confined. */
    override fun close() {
        val h = handle
        if (h == 0L) return
        if (confineToCreatingThread) {
            check(Thread.currentThread() === ownerThread) { "EngineSession closed off its owner thread" }
        }
        handle = 0L
        bridge.destroySession(h)
    }
}
