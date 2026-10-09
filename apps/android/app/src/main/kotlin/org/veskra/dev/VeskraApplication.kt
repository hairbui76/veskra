package org.veskra.dev

import android.app.Application
import android.util.Log
import android.view.MotionEvent
import org.veskra.kit.MotionConstants
import org.veskra.kit.NativeBridge

/**
 * Process-wide setup: loads `libveskra_ffi.so` once. A missing library (e.g. a build made without
 * scripts/build-rust.sh) is reported in the UI instead of crashing.
 */
class VeskraApplication : Application() {

    /** Null when the engine loaded; otherwise a safe diagnostic (no paths). */
    var nativeLoadError: String? = null
        private set

    val engineAvailable: Boolean get() = nativeLoadError == null

    override fun onCreate() {
        super.onCreate()
        nativeLoadError = try {
            NativeBridge.load()
            null
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "native engine not available: ${e.javaClass.simpleName}")
            "UnsatisfiedLinkError"
        }
        if (BuildConfig.DEBUG) checkKitConstants()
    }

    /** :kit duplicates MotionEvent constant values (it cannot import android.*); verify against the SDK. */
    private fun checkKitConstants() {
        check(MotionConstants.ACTION_DOWN == MotionEvent.ACTION_DOWN)
        check(MotionConstants.ACTION_UP == MotionEvent.ACTION_UP)
        check(MotionConstants.ACTION_MOVE == MotionEvent.ACTION_MOVE)
        check(MotionConstants.ACTION_CANCEL == MotionEvent.ACTION_CANCEL)
        check(MotionConstants.ACTION_POINTER_DOWN == MotionEvent.ACTION_POINTER_DOWN)
        check(MotionConstants.ACTION_POINTER_UP == MotionEvent.ACTION_POINTER_UP)
        check(MotionConstants.ACTION_HOVER_MOVE == MotionEvent.ACTION_HOVER_MOVE)
        check(MotionConstants.ACTION_HOVER_ENTER == MotionEvent.ACTION_HOVER_ENTER)
        check(MotionConstants.ACTION_HOVER_EXIT == MotionEvent.ACTION_HOVER_EXIT)
        check(MotionConstants.TOOL_TYPE_FINGER == MotionEvent.TOOL_TYPE_FINGER)
        check(MotionConstants.TOOL_TYPE_STYLUS == MotionEvent.TOOL_TYPE_STYLUS)
        check(MotionConstants.TOOL_TYPE_MOUSE == MotionEvent.TOOL_TYPE_MOUSE)
        check(MotionConstants.TOOL_TYPE_ERASER == MotionEvent.TOOL_TYPE_ERASER)
        check(MotionConstants.BUTTON_STYLUS_PRIMARY == MotionEvent.BUTTON_STYLUS_PRIMARY)
        check(MotionConstants.BUTTON_STYLUS_SECONDARY == MotionEvent.BUTTON_STYLUS_SECONDARY)
    }

    companion object {
        private const val TAG = "Veskra"
    }
}
