package org.veskra.dev.update

import org.veskra.kit.ReleaseInfo
import org.veskra.kit.UpdateApk

/** The "Check for updates" dialog, step by step. Null in UiState when the dialog is closed. */
sealed interface UpdateState {
    data object Checking : UpdateState

    data class UpToDate(val installed: String, val latest: String) : UpdateState

    data class Available(val installed: String, val release: ReleaseInfo, val apk: UpdateApk) : UpdateState

    data class Downloading(val release: ReleaseInfo, val apk: UpdateApk, val progress: Float) : UpdateState

    /** Downloaded and verified. [needsPermission]: "Install unknown apps" is not yet allowed for Veskra. */
    data class ReadyToInstall(val release: ReleaseInfo, val path: String, val needsPermission: Boolean) : UpdateState

    /** Handed to the system installer, which shows its own confirmation. */
    data class Installing(val release: ReleaseInfo, val path: String) : UpdateState

    data class Failed(val reason: Reason, val detail: String?, val release: ReleaseInfo? = null) : UpdateState

    enum class Reason { NETWORK, BAD_RESPONSE, NO_APK, CHECKSUM, NOT_AN_UPDATE, STORAGE, SIGNATURE_CONFLICT, INSTALL_FAILED }
}
