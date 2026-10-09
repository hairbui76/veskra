package org.veskra.dev.engine

import android.os.Handler
import android.os.HandlerThread
import android.os.Process
import kotlinx.coroutines.android.asCoroutineDispatcher
import kotlinx.coroutines.withContext
import org.veskra.kit.EngineBridge
import org.veskra.kit.EngineException
import org.veskra.kit.EngineSession
import org.veskra.kit.JniEngineBridge
import java.io.Closeable

/**
 * Owns the single dedicated engine thread and the [EngineSession] confined to it
 * (ARCHITECTURE §D thread ownership: "Document/commands — Rust document actor, serialized").
 *
 * - Input batches are posted with [post] (no coroutine overhead on the hot path).
 * - Commands from the ViewModel use [call], which suspends the caller and never blocks the UI thread.
 *
 * Lives in the ViewModel, so it survives configuration changes; [close] destroys the native session
 * on the engine thread and stops the thread.
 */
class EngineHost(private val bridge: EngineBridge = JniEngineBridge) : Closeable {
    private val thread = HandlerThread("veskra-engine", Process.THREAD_PRIORITY_DISPLAY).apply { start() }
    val handler: Handler = Handler(thread.looper)
    private val dispatcher = handler.asCoroutineDispatcher("veskra-engine")

    // Engine-thread only.
    private var session: EngineSession? = null
    private var closed = false

    /** Engine-thread-only tile renderer for the current document (null before a document exists). */
    var renderer: CanvasRenderer? = null
        private set

    /** Set (from the engine thread) if the native session could not be created. */
    @Volatile
    var startError: String? = null
        private set

    init {
        handler.post {
            session = try {
                EngineSession.open(bridge, confineToCreatingThread = true)
            } catch (e: EngineException) {
                startError = e.error.code
                null
            } catch (e: UnsatisfiedLinkError) {
                startError = "UnsatisfiedLinkError"
                null
            }
        }
    }

    val isOnEngineThread: Boolean get() = Thread.currentThread() === thread

    /** Runs [block] on the engine thread if the session is alive. Fire-and-forget. */
    fun post(block: (EngineSession) -> Unit) {
        handler.post {
            val s = session ?: return@post
            if (!s.isClosed) block(s)
        }
    }

    /** Suspends until [block] ran on the engine thread. Throws if the host is closed. */
    suspend fun <T> call(block: (EngineSession) -> T): T = withContext(dispatcher) {
        val s = checkNotNull(session) { "engine session unavailable" }
        block(s)
    }

    // Engine-thread only: last canvas view size, re-applied to every new renderer.
    private var viewportW = 0
    private var viewportH = 0

    /** Engine thread: (re)creates the renderer for a new document size (after create/open/recover). */
    fun resetRenderer(s: EngineSession, docWidth: Int, docHeight: Int, listener: CanvasRenderer.Listener): CanvasRenderer {
        check(isOnEngineThread)
        renderer?.dispose()
        val r = CanvasRenderer(this, docWidth, docHeight, listener)
        renderer = r
        if (viewportW > 0 && viewportH > 0) r.setViewport(s, viewportW, viewportH)
        return r
    }

    /** Any thread: canvas view size changed (layout, rotation, multi-window resize). */
    fun setViewport(width: Int, height: Int) {
        if (width <= 0 || height <= 0) return
        handler.post {
            viewportW = width
            viewportH = height
            val s = session ?: return@post
            val r = renderer
            if (r != null) r.setViewport(s, width, height) else s.setViewport(width, height)
        }
    }

    override fun close() {
        handler.post {
            if (closed) return@post
            closed = true
            renderer?.dispose()
            renderer = null
            session?.close() // nativeDestroySession; idempotent
            session = null
            thread.quitSafely()
        }
    }
}
