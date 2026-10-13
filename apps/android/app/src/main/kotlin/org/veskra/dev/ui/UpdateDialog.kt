package org.veskra.dev.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.update.UpdateState
import java.util.Locale

/** "Check for updates" (ADR-020): one dialog that walks through check → download → install. */
@Composable
fun UpdateDialog(state: UpdateState, vm: VeskraViewModel) {
    val context = LocalContext.current
    val uri = LocalUriHandler.current
    val dismiss = vm::dismissUpdate
    when (state) {
        UpdateState.Checking -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text(stringResource(R.string.update_title)) },
            text = { Busy(stringResource(R.string.update_checking)) },
            confirmButton = {},
            dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.action_cancel)) } },
        )

        is UpdateState.UpToDate -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text(stringResource(R.string.update_latest_title)) },
            text = { Text(stringResource(R.string.update_latest_body, state.installed)) },
            confirmButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.action_ok)) } },
        )

        is UpdateState.Available -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text(stringResource(R.string.update_available_title, state.release.version)) },
            text = {
                Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(stringResource(R.string.update_available_body, state.installed, megabytes(state.apk.size)))
                    val notes = state.release.plainNotes()
                    if (notes.isNotEmpty()) {
                        Text(
                            notes.joinToString("\n"),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Text(stringResource(R.string.update_keeps_drawings), style = MaterialTheme.typography.bodySmall)
                    state.release.pageUrl?.let { page ->
                        TextButton(onClick = { runCatching { uri.openUri(page) } }) { Text(stringResource(R.string.update_release_page)) }
                    }
                }
            },
            confirmButton = { Button(onClick = vm::downloadUpdate) { Text(stringResource(R.string.update_download)) } },
            dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.update_later)) } },
        )

        is UpdateState.Downloading -> AlertDialog(
            onDismissRequest = {},
            title = { Text(stringResource(R.string.update_downloading_title, state.release.version)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    LinearProgressIndicator(progress = { state.progress.coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
                    Text(stringResource(R.string.update_downloading_body, (state.progress * 100).toInt().coerceIn(0, 100), megabytes(state.apk.size)))
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.action_cancel)) } },
        )

        is UpdateState.ReadyToInstall -> if (state.needsPermission) {
            AlertDialog(
                onDismissRequest = dismiss,
                title = { Text(stringResource(R.string.update_permission_title)) },
                text = { Text(stringResource(R.string.update_permission_body)) },
                confirmButton = {
                    Button(onClick = { runCatching { context.startActivity(vm.installPermissionIntent()) } }) {
                        Text(stringResource(R.string.update_open_settings))
                    }
                },
                dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.action_cancel)) } },
            )
        } else {
            AlertDialog(
                onDismissRequest = dismiss,
                title = { Text(stringResource(R.string.update_ready_title, state.release.version)) },
                text = { Text(stringResource(R.string.update_ready_body)) },
                confirmButton = { Button(onClick = vm::installUpdate) { Text(stringResource(R.string.update_install)) } },
                dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.action_cancel)) } },
            )
        }

        is UpdateState.Installing -> AlertDialog(
            onDismissRequest = {},
            title = { Text(stringResource(R.string.update_ready_title, state.release.version)) },
            text = { Busy(stringResource(R.string.update_installing)) },
            confirmButton = {},
            dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.action_close)) } },
        )

        is UpdateState.Failed -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text(stringResource(R.string.update_failed_title)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(failureText(state.reason)))
                    state.detail?.takeIf(String::isNotBlank)?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            },
            confirmButton = { TextButton(onClick = vm::checkForUpdate) { Text(stringResource(R.string.update_retry)) } },
            dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.action_close)) } },
        )
    }
}

@Composable
private fun Busy(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 3.dp)
        Text(text)
    }
}

private fun megabytes(bytes: Long): String = String.format(Locale.ROOT, "%.1f", bytes / (1024.0 * 1024.0))

private fun failureText(reason: UpdateState.Reason): Int = when (reason) {
    UpdateState.Reason.NETWORK -> R.string.update_error_network
    UpdateState.Reason.BAD_RESPONSE -> R.string.update_error_response
    UpdateState.Reason.NO_APK -> R.string.update_error_no_apk
    UpdateState.Reason.CHECKSUM -> R.string.update_error_checksum
    UpdateState.Reason.NOT_AN_UPDATE -> R.string.update_error_not_update
    UpdateState.Reason.STORAGE -> R.string.update_error_storage
    UpdateState.Reason.SIGNATURE_CONFLICT -> R.string.update_error_conflict
    UpdateState.Reason.INSTALL_FAILED -> R.string.update_error_install
}
